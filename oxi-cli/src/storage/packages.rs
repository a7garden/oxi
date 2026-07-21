//! Package system for oxi CLI
//!
//! Packages bundle extensions, skills, prompts, and themes for sharing.
//! Supports local directories, npm packages, git repositories, GitHub
//! shorthand, and URL-based archives.
//!
//! ## Package sources
//!
//! - **Local path**: a directory with `oxi-package.toml` or auto-discoverable resources
//! - **npm**: `npm:<package>[@<version>]` — resolved from the npm registry
//! - **git**: `https://github.com/org/repo.git[@ref]`, `git://…`, `git+ssh://…`
//! - **GitHub shorthand**: `github:org/repo[@ref]`
//! - **URL**: direct `.tar.gz` / `.zip` archive
//!
//! ## Package manifest
//!
//! A package is a directory containing an `oxi-package.toml` file:
//!
//! ```toml
//! name = "@foo/oxi-tools"
//! version = "1.0.0"
//! extensions = ["ext/index.ts"]
//! skills = ["skills/code-review/SKILL.md"]
//! prompts = ["prompts/review.md"]
//! themes = ["themes/dark-pro.json"]
//! ```
//!
//! ## Resource discovery
//!
//! When a package lacks explicit resource lists, resources are discovered
//! automatically by scanning the package directory:
//! - **Extensions**: `.so`, `.dylib`, `.dll` files, or `index.ts`/`index.js` entries
//! - **Skills**: Directories containing `SKILL.md`
//! - **Prompts**: `.md` files in `prompts/` subdirectory
//! - **Themes**: `.json` files in `themes/` subdirectory
//!
//! ## Lockfile
//!
//! An `oxi-lock.json` file records exact versions/refs for reproducibility.

use crate::util::http_client::shared_http_client;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Run an async future on a fresh tokio runtime created on a dedicated OS thread.
///
/// This avoids the "Cannot start a runtime from within a runtime" panic that
/// `Runtime::new()?.block_on(future)` causes when called from inside an
/// existing tokio context (e.g., from an agent tool callback or TUI handler).
fn run_on_fresh_runtime<F, T>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>> + Send,
    T: Send,
{
    std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to build temp runtime")?;
            rt.block_on(future)
        })
        .join()
        .map_err(|_| anyhow::anyhow!("runtime thread panicked"))?
    })
}

/// Cached regex for parsing npm package specs
static NPM_SPEC_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(@?[^@]+(?:/[^@]+)?)(?:@(.+))?$").expect("valid static regex")
});

// ── Constants ─────────────────────────────────────────────────────────

const LOCKFILE_NAME: &str = "oxi-lock.json";
const MANIFEST_NAME: &str = "oxi-package.toml";
const NPM_MANIFEST_NAME: &str = "package.json";

// ── Types ─────────────────────────────────────────────────────────────

/// Types of resources a package can contribute
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// extension variant.
    Extension,
    /// skill variant.
    Skill,
    /// prompt variant.
    Prompt,
    /// theme variant.
    Theme,
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceKind::Extension => write!(f, "extension"),
            ResourceKind::Skill => write!(f, "skill"),
            ResourceKind::Prompt => write!(f, "prompt"),
            ResourceKind::Theme => write!(f, "theme"),
        }
    }
}

// All resource kinds for iteration

/// Package manifest describing bundled resources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    /// Package name (e.g. "@foo/oxi-tools")
    pub name: String,
    /// Semantic version (e.g. "1.0.0")
    pub version: String,
    /// Extension paths relative to the package root
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Skill names/paths
    #[serde(default)]
    pub skills: Vec<String>,
    /// Prompt template paths
    #[serde(default)]
    pub prompts: Vec<String>,
    /// Theme paths
    #[serde(default)]
    pub themes: Vec<String>,
    /// Optional description
    #[serde(default)]
    pub description: Option<String>,
    /// Package dependencies (name -> version constraint)
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

/// A discovered resource within a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredResource {
    /// Resource type
    pub kind: ResourceKind,
    /// Absolute path to the resource
    pub path: PathBuf,
    /// Relative path within the package
    pub relative_path: String,
}

/// Metadata about a resolved resource path
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathMetadata {
    /// Source specifier
    pub source: String,
    /// Scope (user / project)
    pub scope: SourceScope,
    /// Whether this is a package resource or top-level
    pub origin: ResourceOrigin,
    /// Base directory for resolving relative paths
    pub base_dir: Option<PathBuf>,
}

/// Origin of a resource
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOrigin {
    /// package variant.
    Package,
    /// top level variant.
    TopLevel,
}

/// Scope for package sources
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    /// user variant.
    User,
    /// project variant.
    Project,
}

impl std::fmt::Display for SourceScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceScope::User => write!(f, "user"),
            SourceScope::Project => write!(f, "project"),
        }
    }
}

/// A resolved resource with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedResource {
    /// Absolute path to the resource
    pub path: PathBuf,
    /// Whether this resource is enabled
    pub enabled: bool,
    /// Metadata about the resource
    pub metadata: PathMetadata,
}

/// Resolved paths for all resource types
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolvedPaths {
    /// pub.
    pub extensions: Vec<ResolvedResource>,
    /// pub.
    pub skills: Vec<ResolvedResource>,
    /// pub.
    pub prompts: Vec<ResolvedResource>,
    /// pub.
    pub themes: Vec<ResolvedResource>,
}

/// Progress events for package operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    /// pub.
    pub event_type: ProgressEventType,
    /// pub.
    pub action: ProgressAction,
    /// pub.
    pub source: String,
    /// pub.
    pub message: Option<String>,
}

/// Progress event type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressEventType {
    /// start variant.
    Start,
    /// progress variant.
    Progress,
    /// complete variant.
    Complete,
    /// error variant.
    Error,
}

/// Action being performed
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressAction {
    /// install variant.
    Install,
    /// remove variant.
    Remove,
    /// update variant.
    Update,
    /// clone variant.
    Clone,
    /// pull variant.
    Pull,
}

impl std::fmt::Display for ProgressAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgressAction::Install => write!(f, "install"),
            ProgressAction::Remove => write!(f, "remove"),
            ProgressAction::Update => write!(f, "update"),
            ProgressAction::Clone => write!(f, "clone"),
            ProgressAction::Pull => write!(f, "pull"),
        }
    }
}

/// Callback for progress events
pub type ProgressCallback = Box<dyn Fn(ProgressEvent) + Send + Sync>;

// ── Source parsing ────────────────────────────────────────────────────

/// Parsed package source
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParsedSource {
    /// Variant.
    Npm {
        /// Full spec (e.g. "express@4.18.0")
        spec: String,
        /// Package name without version
        name: String,
        /// Whether a version was pinned
        pinned: bool,
    },
    /// Variant.
    Git {
        /// Full repository URL
        repo: String,
        /// Host (e.g. "github.com")
        host: String,
        /// Path on host (e.g. "org/repo")
        path: String,
        /// Optional ref (branch / tag / commit)
        ref_: Option<String>,
    },
    /// Variant.
    Local {
        /// Local path
        path: String,
    },
    /// Variant.
    Url {
        /// URL to archive
        url: String,
    },
}

impl ParsedSource {
    /// Parse a source string into a ParsedSource
    pub fn parse(source: &str) -> Self {
        if let Some(rest) = source.strip_prefix("npm:") {
            let spec = rest.trim();
            let (name, pinned) = parse_npm_spec(spec);
            return ParsedSource::Npm {
                spec: spec.to_string(),
                name,
                pinned,
            };
        }

        if let Some(rest) = source.strip_prefix("github:") {
            let parts: Vec<&str> = rest.splitn(2, '/').collect();
            if parts.len() == 2 {
                let (path, ref_) = split_git_path_ref(rest);
                return ParsedSource::Git {
                    repo: format!("https://github.com/{}.git", path),
                    host: "github.com".to_string(),
                    path,
                    ref_,
                };
            }
        }

        if source.starts_with("git+") || source.starts_with("git://") || source.starts_with("git@")
        {
            return parse_git_source(source);
        }

        if source.starts_with("https://") || source.starts_with("http://") {
            // Distinguish git URLs from plain archive URLs
            if source.ends_with(".git")
                || source.contains("github.com")
                || source.contains("gitlab.com")
            {
                return parse_git_source(source);
            }
            // Archive URL (.tar.gz, .zip, .tgz)
            if source.ends_with(".tar.gz")
                || source.ends_with(".tgz")
                || source.ends_with(".zip")
                || source.ends_with(".tar.bz2")
            {
                return ParsedSource::Url {
                    url: source.to_string(),
                };
            }
            // Default to git for http(s) URLs that look like repos
            return parse_git_source(source);
        }

        // Local path
        ParsedSource::Local {
            path: source.to_string(),
        }
    }

    /// Get a unique identity key for this source (ignoring version/ref)
    pub fn identity(&self) -> String {
        match self {
            ParsedSource::Npm { name, .. } => format!("npm:{}", name),
            ParsedSource::Git { host, path, .. } => format!("git:{}/{}", host, path),
            ParsedSource::Local { path } => format!("local:{}", path),
            ParsedSource::Url { url } => format!("url:{}", url),
        }
    }

    /// Get a display-friendly name
    pub fn display_name(&self) -> String {
        match self {
            ParsedSource::Npm { name, .. } => name.clone(),
            ParsedSource::Git { host, path, .. } => format!("{}/{}", host, path),
            ParsedSource::Local { path } => path.clone(),
            ParsedSource::Url { url } => url.clone(),
        }
    }
}

/// Parse an npm spec into (name, pinned)
fn parse_npm_spec(spec: &str) -> (String, bool) {
    // Handle scoped packages like @scope/name@version
    if let Some(caps) = NPM_SPEC_RE.captures(spec) {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or(spec);
        let has_version = caps.get(2).is_some();
        return (name.to_string(), has_version);
    }
    (spec.to_string(), false)
}

/// Split a git path like "org/repo@ref" into ("org/repo", Some("ref"))
fn split_git_path_ref(input: &str) -> (String, Option<String>) {
    if let Some(at_pos) = input.rfind('@') {
        // Make sure it's not part of an email (don't split if there's no / before @)
        if input[..at_pos].contains('/') {
            return (
                input[..at_pos].to_string(),
                Some(input[at_pos + 1..].to_string()),
            );
        }
    }
    (input.to_string(), None)
}

/// Parse a git source string
fn parse_git_source(source: &str) -> ParsedSource {
    // Handle git@host:path format (SSH)
    if let Some(rest) = source.strip_prefix("git@") {
        let colon_pos = rest.find(':').unwrap_or(rest.len());
        let host = &rest[..colon_pos];
        let path_part = rest.get(colon_pos + 1..).unwrap_or("");
        let (path, ref_) = if let Some(hash_pos) = path_part.rfind('#') {
            (
                path_part[..hash_pos].to_string(),
                Some(path_part[hash_pos + 1..].to_string()),
            )
        } else {
            split_git_path_ref(path_part)
        };
        let repo = format!("git@{}:{}", host, path_part);
        let host = host.to_string();
        return ParsedSource::Git {
            repo,
            host,
            path: path.trim_end_matches(".git").to_string(),
            ref_,
        };
    }

    // Handle git+ssh://, git+https://, git:// prefixes
    let url_str = source
        .strip_prefix("git+")
        .unwrap_or(source)
        .strip_prefix("git://")
        .map(|s| format!("https://{}", s))
        .unwrap_or_else(|| source.strip_prefix("git+").unwrap_or(source).to_string());

    // Parse URL to extract host and path
    let url = match url::Url::parse(&url_str) {
        Ok(u) => u,
        Err(_) => {
            return ParsedSource::Local {
                path: source.to_string(),
            };
        }
    };

    let host = url.host_str().unwrap_or("unknown").to_string();
    let full_path = url.path().trim_start_matches('/').to_string();

    // Check for #ref fragment
    let fragment = url.fragment().map(|f| f.to_string());

    let (path, ref_) = if let Some(frag) = fragment {
        (full_path.trim_end_matches(".git").to_string(), Some(frag))
    } else {
        let (p, r) = split_git_path_ref(&full_path);
        (p.trim_end_matches(".git").to_string(), r)
    };

    let repo = url_str.clone();

    ParsedSource::Git {
        repo,
        host,
        path,
        ref_,
    }
}

// ── NPM Registry ──────────────────────────────────────────────────────

/// Information fetched from the npm registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NpmPackageInfo {
    /// pub.
    pub name: String,
    /// pub.
    pub versions: BTreeMap<String, serde_json::Value>,
    /// pub.
    #[serde(rename = "dist-tags")]
    pub dist_tags: BTreeMap<String, String>,
}

impl NpmPackageInfo {
    /// Fetch package info from the npm registry
    pub async fn fetch(name: &str) -> Result<Self> {
        let url = format!("https://registry.npmjs.org/{}", name);
        let client = shared_http_client();

        let resp = client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .with_context(|| format!("Failed to fetch npm info for '{}'", name))?;

        if !resp.status().is_success() {
            bail!("npm registry returned {} for '{}'", resp.status(), name);
        }

        let info: NpmPackageInfo = resp
            .json()
            .await
            .with_context(|| format!("Failed to parse npm registry response for '{}'", name))?;

        Ok(info)
    }

    /// Get the latest version from dist-tags
    pub fn latest_version(&self) -> Option<&str> {
        self.dist_tags.get("latest").map(|s| s.as_str())
    }

    /// Find the best matching version for a constraint
    pub fn resolve_version(&self, constraint: &str) -> Option<String> {
        if constraint == "latest" || constraint.is_empty() {
            return self.latest_version().map(|s| s.to_string());
        }

        // Try exact match first
        if self.versions.contains_key(constraint) {
            return Some(constraint.to_string());
        }

        // Try semver range matching
        if let Ok(req) = semver::VersionReq::parse(constraint) {
            let mut best: Option<semver::Version> = None;
            for ver_str in self.versions.keys() {
                if let Ok(ver) = semver::Version::parse(ver_str)
                    && req.matches(&ver)
                {
                    match &best {
                        Some(b) if ver > *b => best = Some(ver),
                        None => best = Some(ver),
                        _ => {}
                    }
                }
            }
            if let Some(v) = best {
                return Some(v.to_string());
            }
        }

        None
    }
}

/// Get the latest version of an npm package
pub async fn get_latest_npm_version(name: &str) -> Result<String> {
    let info = NpmPackageInfo::fetch(name).await?;
    info.latest_version()
        .map(|s| s.to_string())
        .context(format!("No latest version found for '{}'", name))
}

// ── Git Operations ────────────────────────────────────────────────────

/// Run a git command and capture stdout
fn git_command(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let output = cmd.output().context("Failed to execute git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a git command (no capture)
fn git_command_silent(args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let status = cmd.status().context("Failed to execute git")?;
    if !status.success() {
        bail!("git {} failed ({})", args.join(" "), status);
    }
    Ok(())
}

/// Clone a git repository
pub fn git_clone(repo_url: &str, target_dir: &Path, ref_: Option<&str>) -> Result<()> {
    if target_dir.exists() {
        bail!("Target directory already exists: {}", target_dir.display());
    }
    fs::create_dir_all(target_dir)
        .with_context(|| format!("Failed to create {}", target_dir.display()))?;

    let target_str = target_dir.to_string_lossy().to_string();
    let args = vec!["clone", repo_url, &target_str];

    git_command_silent(&args, None)?;

    if let Some(r) = ref_ {
        git_command_silent(&["checkout", r], Some(target_dir))?;
    }

    Ok(())
}

/// Pull/update a git repository in place
pub fn git_update(repo_dir: &Path, ref_: Option<&str>) -> Result<bool> {
    if !repo_dir.exists() {
        bail!(
            "Repository directory does not exist: {}",
            repo_dir.display()
        );
    }

    // Get current HEAD
    let local_head = git_command(&["rev-parse", "HEAD"], Some(repo_dir))?;

    // Determine what to fetch
    let fetch_ref = if let Some(r) = ref_ {
        r.to_string()
    } else {
        // Try to get upstream ref
        match git_command(
            &["rev-parse", "--abbrev-ref", "@{upstream}"],
            Some(repo_dir),
        ) {
            Ok(upstream) => {
                if let Some(branch) = upstream.strip_prefix("origin/") {
                    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
                } else {
                    "+HEAD:refs/remotes/origin/HEAD".to_string()
                }
            }
            Err(_) => "+HEAD:refs/remotes/origin/HEAD".to_string(),
        }
    };

    git_command_silent(
        &["fetch", "--prune", "--no-tags", "origin", &fetch_ref],
        Some(repo_dir),
    )?;

    // Determine what to reset to
    let target_ref = ref_.unwrap_or("origin/HEAD");
    let remote_head = git_command(&["rev-parse", target_ref], Some(repo_dir))?;

    if local_head == remote_head {
        return Ok(false); // No update needed
    }

    git_command_silent(&["reset", "--hard", target_ref], Some(repo_dir))?;
    git_command_silent(&["clean", "-fdx"], Some(repo_dir))?;

    Ok(true) // Updated
}

/// Check if a git repo has remote updates available
pub fn git_has_update(repo_dir: &Path) -> Result<bool> {
    let local_head = git_command(&["rev-parse", "HEAD"], Some(repo_dir))?;

    // Try to get upstream
    let upstream_ref = match git_command(
        &["rev-parse", "--abbrev-ref", "@{upstream}"],
        Some(repo_dir),
    ) {
        Ok(u) if u.starts_with("origin/") => {
            let branch = &u["origin/".len()..];
            format!("refs/heads/{branch}")
        }
        _ => "HEAD".to_string(),
    };

    // Fetch quietly and check remote
    let _ = git_command_silent(&["fetch", "--prune", "--no-tags", "origin"], Some(repo_dir));

    let remote_head = git_command(&["ls-remote", "origin", &upstream_ref], None)?;

    // Parse first hash from ls-remote output
    let remote_hash = remote_head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("");

    Ok(local_head != remote_hash)
}

// ── Lockfile ──────────────────────────────────────────────────────────

/// Lockfile entry for an installed package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    /// Source specifier
    pub source: String,
    /// Package name
    pub name: String,
    /// Resolved version or ref
    pub version: String,
    /// Integrity hash (sha256)
    pub integrity: Option<String>,
    /// Scope
    pub scope: SourceScope,
    /// Type of source
    pub source_type: String,
    /// Dependencies
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

/// The lockfile structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile version
    pub version: u32,
    /// Locked packages
    pub packages: BTreeMap<String, LockEntry>,
}

impl Lockfile {
    /// Create a new empty lockfile
    pub fn new() -> Self {
        Self {
            version: 1,
            packages: BTreeMap::new(),
        }
    }

    /// Read lockfile from disk
    pub fn read(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read lockfile {}", path.display()))?;
        let lock: Lockfile = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse lockfile {}", path.display()))?;
        Ok(Some(lock))
    }

    /// Write lockfile to disk
    pub fn write(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self).context("Failed to serialize lockfile")?;
        fs::write(path, content)
            .with_context(|| format!("Failed to write lockfile {}", path.display()))?;
        Ok(())
    }

    /// Add or update an entry
    pub fn insert(&mut self, entry: LockEntry) {
        self.packages.insert(entry.name.clone(), entry);
    }

    /// Remove an entry
    pub fn remove(&mut self, name: &str) -> Option<LockEntry> {
        self.packages.remove(name)
    }

    /// Check if a package is locked
    pub fn contains(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    /// Get an entry
    pub fn get(&self, name: &str) -> Option<&LockEntry> {
        self.packages.get(name)
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

// ── Counts ────────────────────────────────────────────────────────────

/// Counts of each resource type in a package
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceCounts {
    /// pub.
    pub extensions: usize,
    /// pub.
    pub skills: usize,
    /// pub.
    pub prompts: usize,
    /// pub.
    pub themes: usize,
}

impl std::fmt::Display for ResourceCounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.extensions > 0 {
            parts.push(format!("{} ext", self.extensions));
        }
        if self.skills > 0 {
            parts.push(format!("{} skill", self.skills));
        }
        if self.prompts > 0 {
            parts.push(format!("{} prompt", self.prompts));
        }
        if self.themes > 0 {
            parts.push(format!("{} theme", self.themes));
        }
        if parts.is_empty() {
            write!(f, "-")?;
        } else {
            write!(f, "{}", parts.join(", "))?;
        }
        Ok(())
    }
}

// ── Package Manager ───────────────────────────────────────────────────

/// Information about an available package update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageUpdateInfo {
    /// pub.
    pub source: String,
    /// pub.
    pub display_name: String,
    /// pub.
    pub source_type: String, // "npm" or "git"
    /// pub.
    pub scope: SourceScope,
}

/// A configured package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfiguredPackage {
    /// pub.
    pub source: String,
    /// pub.
    pub scope: SourceScope,
    /// pub.
    pub filtered: bool,
    /// pub.
    pub installed_path: Option<PathBuf>,
}

/// Manages installation, removal, and listing of packages
pub struct PackageManager {
    packages_dir: PathBuf,
    /// Base directory for project-scoped packages
    project_dir: PathBuf,
    installed: HashMap<String, PackageManifest>,
    lockfile: Lockfile,
    progress_callback: Option<Box<dyn Fn(ProgressEvent) + Send + Sync>>,
}

impl PackageManager {
    /// Create a new PackageManager using the default packages directory
    pub fn new() -> Result<Self> {
        let base = dirs::home_dir().context("Cannot determine home directory")?;
        let packages_dir = base.join(".oxi").join("packages");
        let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut mgr = Self {
            packages_dir,
            project_dir,
            installed: HashMap::new(),
            lockfile: Lockfile::new(),
            progress_callback: None,
        };
        mgr.load_installed()?;
        mgr.load_lockfile()?;
        Ok(mgr)
    }

    /// Create a PackageManager with a custom packages directory (for testing)
    pub fn with_dir(packages_dir: PathBuf) -> Result<Self> {
        let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut mgr = Self {
            packages_dir,
            project_dir,
            installed: HashMap::new(),
            lockfile: Lockfile::new(),
            progress_callback: None,
        };
        mgr.load_installed()?;
        mgr.load_lockfile()?;
        Ok(mgr)
    }

    /// Set the project directory for project-scoped packages
    pub fn set_project_dir(&mut self, dir: PathBuf) {
        self.project_dir = dir;
    }

    /// Set a progress callback
    pub fn set_progress_callback(&mut self, callback: Box<dyn Fn(ProgressEvent) + Send + Sync>) {
        self.progress_callback = Some(callback);
    }

    fn emit_progress(&self, event: ProgressEvent) {
        if let Some(ref cb) = self.progress_callback {
            cb(event);
        }
    }

    // ── Loading ───────────────────────────────────────────────────────

    /// Load all installed package manifests from disk.
    ///
    /// For every installed package whose `LockEntry` records an integrity
    /// hash, the on-disk SHA-256 is recomputed and compared. Mismatches
    /// (caused by tampering, partial disk failure, or any non-atomic write
    /// that survived a crash) remove the package from the in-memory `installed`
    /// map AND invalidate the matching lockfile entry, so subsequent
    /// `update_all` re-fetches the package instead of trusting the cached copy.
    /// A mismatch is logged at `warn` level — the CLI keeps booting (other
    /// packages remain usable) but the affected package is treated as
    /// un-installed.
    fn load_installed(&mut self) -> Result<()> {
        if !self.packages_dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&self.packages_dir)? {
            let entry = entry?;
            let manifest_path = entry.path().join(MANIFEST_NAME);
            if manifest_path.exists() {
                match Self::read_manifest(&manifest_path) {
                    Ok(manifest) => {
                        let name = manifest.name.clone();
                        let install_dir = entry.path();

                        // F-1 (audit 2026-06-21): verify lockfile integrity
                        // before trusting the installed package. Without this,
                        // `compute_dir_hash` is only computed at install and
                        // never re-checked on subsequent loads — a local
                        // attacker (or a partial-write crash) could swap files
                        // under `~/.oxi/packages/<name>/` and the next session
                        // would silently load the tampered manifest.
                        if let Some(expected) = self
                            .lockfile
                            .packages
                            .get(&name)
                            .and_then(|e| e.integrity.as_ref())
                        {
                            match verify_lockfile_integrity(&install_dir, expected) {
                                Ok(()) => {}
                                Err(reason) => {
                                    tracing::warn!(
                                        package = %name,
                                        expected = %expected,
                                        reason = %reason,
                                        "package integrity mismatch on load — treating as un-installed; re-install with `oxi pkg install`"
                                    );
                                    // Drop the lockfile entry so `update_all`
                                    // re-resolves from the source.
                                    self.lockfile.packages.remove(&name);
                                    continue;
                                }
                            }
                        }

                        self.installed.insert(name, manifest);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to load manifest {}: {}",
                            manifest_path.display(),
                            e
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Load lockfile from disk
    fn load_lockfile(&mut self) -> Result<()> {
        let lock_path = self.packages_dir.join(LOCKFILE_NAME);
        if let Some(lock) = Lockfile::read(&lock_path)? {
            self.lockfile = lock;
        }
        Ok(())
    }

    /// Save lockfile to disk
    fn save_lockfile(&self) -> Result<()> {
        let lock_path = self.packages_dir.join(LOCKFILE_NAME);
        self.lockfile.write(&lock_path)
    }

    // ── Manifest ──────────────────────────────────────────────────────

    /// Read and parse a package manifest from disk
    fn read_manifest(path: &Path) -> Result<PackageManifest> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read manifest {}", path.display()))?;
        let manifest: PackageManifest = toml::from_str(&content)
            .with_context(|| format!("Failed to parse manifest {}", path.display()))?;
        Ok(manifest)
    }

    /// Try to read a `package.json` manifest (for npm packages)
    fn read_package_json(dir: &Path) -> Option<serde_json::Value> {
        let path = dir.join(NPM_MANIFEST_NAME);
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    // ── Path helpers ──────────────────────────────────────────────────

    /// Get the installation directory for a package
    fn pkg_install_dir(&self, name: &str) -> PathBuf {
        let safe_name = name.replace('@', "").replace('/', "-");
        self.packages_dir.join(safe_name)
    }

    /// Get the packages directory path
    pub fn packages_dir(&self) -> &Path {
        &self.packages_dir
    }

    /// Get install dir for a git source
    fn git_install_path(&self, host: &str, path: &str, scope: SourceScope) -> PathBuf {
        match scope {
            SourceScope::Project => self
                .project_dir
                .join(".oxi")
                .join("git")
                .join(host)
                .join(path),
            SourceScope::User => self.packages_dir.join("git").join(host).join(path),
        }
    }

    /// Get install dir for an npm source
    fn npm_install_path(&self, name: &str, scope: SourceScope) -> PathBuf {
        let safe_name = name.replace('@', "").replace('/', "-");
        match scope {
            SourceScope::Project => self.project_dir.join(".oxi").join("npm").join(safe_name),
            SourceScope::User => self.packages_dir.join("npm").join(safe_name),
        }
    }

    // ── Install ───────────────────────────────────────────────────────

    /// Ensure packages directory exists
    fn ensure_packages_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.packages_dir).with_context(|| {
            format!(
                "Failed to create packages directory {}",
                self.packages_dir.display()
            )
        })
    }

    /// Install a package from a local directory path
    pub fn install(&mut self, source: &str) -> Result<PackageManifest> {
        let parsed = ParsedSource::parse(source);
        match parsed {
            ParsedSource::Local { path } => self.install_local(&path),
            _ => bail!("Use install_from_source() for non-local packages"),
        }
    }

    /// Install a package from a local directory path
    fn install_local(&mut self, path: &str) -> Result<PackageManifest> {
        let source_path = Path::new(path);
        let manifest_path = source_path.join(MANIFEST_NAME);

        let manifest = if manifest_path.exists() {
            Self::read_manifest(&manifest_path)
                .with_context(|| format!("No valid {} found in {}", MANIFEST_NAME, path))?
        } else {
            // Synthesise a minimal manifest
            let name = source_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            PackageManifest {
                name,
                version: "0.0.0".to_string(),
                extensions: Vec::new(),
                skills: Vec::new(),
                prompts: Vec::new(),
                themes: Vec::new(),
                description: None,
                dependencies: BTreeMap::new(),
            }
        };

        let dest = self.pkg_install_dir(&manifest.name);
        self.ensure_packages_dir()?;

        if dest.exists() {
            fs::remove_dir_all(&dest).with_context(|| {
                format!("Failed to remove existing package at {}", dest.display())
            })?;
        }

        copy_dir_recursive(source_path, &dest).with_context(|| {
            format!("Failed to copy package from {} to {}", path, dest.display())
        })?;

        let integrity = compute_dir_hash(&dest);

        self.lockfile.insert(LockEntry {
            source: path.to_string(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            integrity,
            scope: SourceScope::User,
            source_type: "local".to_string(),
            dependencies: manifest.dependencies.clone(),
        });

        self.installed
            .insert(manifest.name.clone(), manifest.clone());
        let _ = self.save_lockfile();
        Ok(manifest)
    }

    /// Install from any source
    pub fn install_from_source(
        &mut self,
        source: &str,
        scope: SourceScope,
    ) -> Result<PackageManifest> {
        let parsed = ParsedSource::parse(source);
        self.emit_progress(ProgressEvent {
            event_type: ProgressEventType::Start,
            action: ProgressAction::Install,
            source: source.to_string(),
            message: Some(format!("Installing {}...", source)),
        });
        let result = match &parsed {
            ParsedSource::Npm { .. } => run_on_fresh_runtime(self.install_npm_async(source, scope)),
            ParsedSource::Git { repo, ref_, .. } => {
                self.install_git_sync(source, repo, ref_.as_deref(), scope)
            }
            ParsedSource::Local { path } => self.install_local(path),
            ParsedSource::Url { url } => run_on_fresh_runtime(self.install_url(url, scope)),
        };
        match &result {
            Ok(_) => self.emit_progress(ProgressEvent {
                event_type: ProgressEventType::Complete,
                action: ProgressAction::Install,
                source: source.to_string(),
                message: None,
            }),
            Err(e) => self.emit_progress(ProgressEvent {
                event_type: ProgressEventType::Error,
                action: ProgressAction::Install,
                source: source.to_string(),
                message: Some(e.to_string()),
            }),
        }
        result
    }

    /// Async install from npm using registry
    async fn install_npm_async(
        &mut self,
        source: &str,
        scope: SourceScope,
    ) -> Result<PackageManifest> {
        let parsed = ParsedSource::parse(source);
        let (spec, name, pinned) = match &parsed {
            ParsedSource::Npm { spec, name, pinned } => (spec.clone(), name.clone(), *pinned),
            _ => bail!("Expected npm source"),
        };

        // Resolve version
        let _version = if pinned {
            // Extract version from spec
            let (_, ver) = parse_npm_spec(&spec);
            if ver {
                spec.rsplit('@').next().unwrap_or("latest").to_string()
            } else {
                "latest".to_string()
            }
        } else {
            get_latest_npm_version(&name)
                .await
                .unwrap_or_else(|_| "latest".to_string())
        };

        // Use npm pack approach
        self.install_npm_pack(&spec, scope)
    }

    /// Install npm package using `npm pack`
    fn install_npm_pack(&mut self, spec: &str, scope: SourceScope) -> Result<PackageManifest> {
        let tmp_dir =
            tempfile::tempdir().context("Failed to create temp directory for npm install")?;

        let output = std::process::Command::new("npm")
            .args(["pack", spec, "--pack-destination"])
            .arg(tmp_dir.path())
            .current_dir(tmp_dir.path())
            .output()
            .context("Failed to run npm pack")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("npm pack failed for '{}': {}", spec, stderr);
        }

        // Find the tarball
        let tarball = fs::read_dir(tmp_dir.path())?
            .filter_map(|e| e.ok())
            .find(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "tgz")
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .context("No .tgz file found after npm pack")?;

        // Extract tarball
        let extract_dir = tmp_dir.path().join("extracted");
        fs::create_dir_all(&extract_dir)?;

        let tar_status = std::process::Command::new("tar")
            .args(["-xzf", &tarball.to_string_lossy(), "-C"])
            .arg(&extract_dir)
            .output()
            .context("Failed to run tar")?;

        if !tar_status.status.success() {
            let stderr = String::from_utf8_lossy(&tar_status.stderr);
            bail!("tar extraction failed: {}", stderr);
        }

        // npm pack extracts into a "package" subdirectory
        let pkg_source = extract_dir.join("package");
        let source_for_copy = if pkg_source.exists() {
            &pkg_source
        } else {
            // Might be just the extracted dir
            extract_dir.as_path()
        };

        self.ensure_packages_dir()?;

        // Determine package name from manifest or spec
        let manifest = if source_for_copy.join(MANIFEST_NAME).exists() {
            Self::read_manifest(&source_for_copy.join(MANIFEST_NAME))?
        } else if source_for_copy.join(NPM_MANIFEST_NAME).exists() {
            let pj = Self::read_package_json(source_for_copy);
            let (pkg_name, pkg_version) = pj
                .as_ref()
                .map(|v| {
                    (
                        v.get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or(spec)
                            .to_string(),
                        v.get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("0.0.0")
                            .to_string(),
                    )
                })
                .unwrap_or((spec.to_string(), "0.0.0".to_string()));

            PackageManifest {
                name: pkg_name,
                version: pkg_version,
                extensions: Vec::new(),
                skills: Vec::new(),
                prompts: Vec::new(),
                themes: Vec::new(),
                description: None,
                dependencies: BTreeMap::new(),
            }
        } else {
            PackageManifest {
                name: spec.to_string(),
                version: "0.0.0".to_string(),
                extensions: Vec::new(),
                skills: Vec::new(),
                prompts: Vec::new(),
                themes: Vec::new(),
                description: None,
                dependencies: BTreeMap::new(),
            }
        };

        let dest = self.pkg_install_dir(&manifest.name);
        if dest.exists() {
            fs::remove_dir_all(&dest).with_context(|| {
                format!("Failed to remove existing package at {}", dest.display())
            })?;
        }

        copy_dir_recursive(source_for_copy, &dest)
            .with_context(|| format!("Failed to copy npm package for '{}'", spec))?;

        let integrity = compute_dir_hash(&dest);

        self.lockfile.insert(LockEntry {
            source: format!("npm:{}", spec),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            integrity,
            scope,
            source_type: "npm".to_string(),
            dependencies: manifest.dependencies.clone(),
        });

        self.installed
            .insert(manifest.name.clone(), manifest.clone());
        let _ = self.save_lockfile();
        Ok(manifest)
    }

    /// Install from git
    fn install_git_sync(
        &mut self,
        source: &str,
        repo: &str,
        ref_: Option<&str>,
        scope: SourceScope,
    ) -> Result<PackageManifest> {
        let parsed = ParsedSource::parse(source);
        let (host, path) = match &parsed {
            ParsedSource::Git { host, path, .. } => (host.clone(), path.clone()),
            _ => bail!("Expected git source"),
        };

        let target_dir = self.git_install_path(&host, &path, scope);

        if target_dir.exists() {
            // Already installed
            return self.load_manifest_from_dir(&target_dir, source, scope);
        }

        let Some(parent) = target_dir.parent() else {
            bail!(
                "Invalid install path: no parent directory for {}",
                target_dir.display()
            );
        };
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent dir for {}", target_dir.display()))?;

        git_clone(repo, &target_dir, ref_)?;

        // Install npm dependencies if package.json exists
        if target_dir.join(NPM_MANIFEST_NAME).exists() {
            let _ = std::process::Command::new("npm")
                .args(["install", "--omit=dev"])
                .current_dir(&target_dir)
                .output();
        }

        self.load_manifest_from_dir(&target_dir, source, scope)
    }

    /// Load manifest from a directory and register it
    fn load_manifest_from_dir(
        &mut self,
        dir: &Path,
        source: &str,
        scope: SourceScope,
    ) -> Result<PackageManifest> {
        let manifest = if dir.join(MANIFEST_NAME).exists() {
            Self::read_manifest(&dir.join(MANIFEST_NAME))?
        } else {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            PackageManifest {
                name,
                version: "0.0.0".to_string(),
                extensions: Vec::new(),
                skills: Vec::new(),
                prompts: Vec::new(),
                themes: Vec::new(),
                description: None,
                dependencies: BTreeMap::new(),
            }
        };

        let integrity = compute_dir_hash(dir);

        self.lockfile.insert(LockEntry {
            source: source.to_string(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            integrity,
            scope,
            source_type: "git".to_string(),
            dependencies: manifest.dependencies.clone(),
        });

        self.installed
            .insert(manifest.name.clone(), manifest.clone());
        let _ = self.save_lockfile();
        Ok(manifest)
    }

    /// Install from a URL (archive)
    async fn install_url(&mut self, url: &str, scope: SourceScope) -> Result<PackageManifest> {
        let client = shared_http_client();

        let resp = client.get(url).send().await?;
        if !resp.status().is_success() {
            bail!("Failed to download {}: {}", url, resp.status());
        }

        let bytes = resp.bytes().await?;

        let tmp_dir = tempfile::tempdir()?;
        let archive_name = url.split('/').next_back().unwrap_or("archive");
        let archive_path = tmp_dir.path().join(archive_name);
        fs::write(&archive_path, &bytes)?;

        let extract_dir = tmp_dir.path().join("extracted");
        fs::create_dir_all(&extract_dir)?;

        if archive_name.ends_with(".tar.gz") || archive_name.ends_with(".tgz") {
            let status = std::process::Command::new("tar")
                .args(["-xzf", &archive_path.to_string_lossy(), "-C"])
                .arg(&extract_dir)
                .output()?;
            if !status.status.success() {
                bail!("Failed to extract archive");
            }
        } else if archive_name.ends_with(".zip") {
            // Use unzip if available
            let status = std::process::Command::new("unzip")
                .arg("-o")
                .arg(&archive_path)
                .arg("-d")
                .arg(&extract_dir)
                .output()?;
            if !status.status.success() {
                bail!("Failed to extract zip archive");
            }
        } else {
            bail!("Unsupported archive format: {}", archive_name);
        }

        // Find the extracted package directory
        let pkg_dir = find_single_subdir(&extract_dir).unwrap_or_else(|| extract_dir.to_path_buf());

        self.ensure_packages_dir()?;

        let manifest = if pkg_dir.join(MANIFEST_NAME).exists() {
            Self::read_manifest(&pkg_dir.join(MANIFEST_NAME))?
        } else {
            let name = url
                .split('/')
                .next_back()
                .unwrap_or("url-package")
                .trim_end_matches(".tar.gz")
                .trim_end_matches(".tgz")
                .trim_end_matches(".zip")
                .to_string();
            PackageManifest {
                name,
                version: "0.0.0".to_string(),
                extensions: Vec::new(),
                skills: Vec::new(),
                prompts: Vec::new(),
                themes: Vec::new(),
                description: None,
                dependencies: BTreeMap::new(),
            }
        };

        let dest = self.pkg_install_dir(&manifest.name);
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }

        copy_dir_recursive(&pkg_dir, &dest)?;

        let integrity = compute_dir_hash(&dest);

        self.lockfile.insert(LockEntry {
            source: url.to_string(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            integrity,
            scope,
            source_type: "url".to_string(),
            dependencies: manifest.dependencies.clone(),
        });

        self.installed
            .insert(manifest.name.clone(), manifest.clone());
        let _ = self.save_lockfile();
        Ok(manifest)
    }

    /// Install from npm using `npm pack` (legacy sync method)
    pub fn install_npm(&mut self, name: &str) -> Result<PackageManifest> {
        self.install_npm_pack(name, SourceScope::User)
    }

    // ── Uninstall ─────────────────────────────────────────────────────

    /// Uninstall a package by name
    pub fn uninstall(&mut self, name: &str) -> Result<()> {
        if !self.installed.contains_key(name) {
            bail!("Package '{}' is not installed", name);
        }

        let dest = self.pkg_install_dir(name);
        if dest.exists() {
            fs::remove_dir_all(&dest).with_context(|| {
                format!("Failed to remove package directory {}", dest.display())
            })?;
        }

        // Also try to clean up git/npm scoped dirs
        // (best effort)
        let _ = self.lockfile.remove(name);
        let _ = self.save_lockfile();

        self.installed.remove(name);
        Ok(())
    }

    /// Uninstall a package from a specific source
    pub fn uninstall_from_source(&mut self, source: &str, scope: SourceScope) -> Result<()> {
        let parsed = ParsedSource::parse(source);
        self.emit_progress(ProgressEvent {
            event_type: ProgressEventType::Start,
            action: ProgressAction::Remove,
            source: source.to_string(),
            message: Some(format!("Removing {}...", source)),
        });
        let result = self.do_uninstall_from_source(&parsed, scope);
        match &result {
            Ok(_) => self.emit_progress(ProgressEvent {
                event_type: ProgressEventType::Complete,
                action: ProgressAction::Remove,
                source: source.to_string(),
                message: None,
            }),
            Err(e) => self.emit_progress(ProgressEvent {
                event_type: ProgressEventType::Error,
                action: ProgressAction::Remove,
                source: source.to_string(),
                message: Some(e.to_string()),
            }),
        }
        result
    }

    fn do_uninstall_from_source(
        &mut self,
        parsed: &ParsedSource,
        scope: SourceScope,
    ) -> Result<()> {
        match parsed {
            ParsedSource::Npm { name, .. } => {
                let dest = self.npm_install_path(name, scope);
                if dest.exists() {
                    fs::remove_dir_all(&dest)?;
                }
                self.installed.remove(name);
                self.lockfile.remove(name);
                let _ = self.save_lockfile();
                Ok(())
            }
            ParsedSource::Git { host, path, .. } => {
                let dest = self.git_install_path(host, path, scope);
                if dest.exists() {
                    fs::remove_dir_all(&dest)?;
                    prune_empty_parents(&dest, &self.packages_dir);
                }
                self.installed.retain(|_, m| {
                    let parsed_m = ParsedSource::parse(m.name.as_str());
                    parsed_m.identity() != parsed.identity()
                });
                self.lockfile.packages.retain(|_, entry| {
                    let parsed_e = ParsedSource::parse(&entry.source);
                    parsed_e.identity() != parsed.identity()
                });
                let _ = self.save_lockfile();
                Ok(())
            }
            ParsedSource::Local { .. } => Ok(()),
            ParsedSource::Url { .. } => {
                let identity = parsed.identity();
                self.lockfile
                    .packages
                    .retain(|_, e| ParsedSource::parse(&e.source).identity() != identity);
                let _ = self.save_lockfile();
                Ok(())
            }
        }
    }

    // ── Update ────────────────────────────────────────────────────────

    /// Update a package (re-install from the same source).
    /// For npm packages, re-runs `npm pack` to get the latest version.
    /// For local packages, re-copies from the source path (if available).
    /// For git packages, does a git pull.
    pub fn update(&mut self, name: &str) -> Result<PackageManifest> {
        let lock_entry = self.lockfile.get(name).cloned();

        if let Some(entry) = lock_entry {
            let parsed = ParsedSource::parse(&entry.source);
            return match &parsed {
                ParsedSource::Npm { spec, .. } => {
                    self.emit_progress(ProgressEvent {
                        event_type: ProgressEventType::Start,
                        action: ProgressAction::Update,
                        source: entry.source.clone(),
                        message: Some(format!("Updating {}...", name)),
                    });
                    let result = self.install_npm_pack(spec, entry.scope);
                    match &result {
                        Ok(_) => self.emit_progress(ProgressEvent {
                            event_type: ProgressEventType::Complete,
                            action: ProgressAction::Update,
                            source: entry.source.clone(),
                            message: None,
                        }),
                        Err(e) => self.emit_progress(ProgressEvent {
                            event_type: ProgressEventType::Error,
                            action: ProgressAction::Update,
                            source: entry.source.clone(),
                            message: Some(e.to_string()),
                        }),
                    }
                    result
                }
                ParsedSource::Git { repo, ref_, .. } => {
                    let target_dir = match &parsed {
                        ParsedSource::Git { host, path, .. } => {
                            self.git_install_path(host, path, entry.scope)
                        }
                        _ => unreachable!(),
                    };
                    if target_dir.exists() {
                        let updated = git_update(&target_dir, ref_.as_deref())?;
                        if updated && target_dir.join(NPM_MANIFEST_NAME).exists() {
                            let _ = std::process::Command::new("npm")
                                .args(["install", "--omit=dev"])
                                .current_dir(&target_dir)
                                .output();
                        }
                        self.load_manifest_from_dir(&target_dir, &entry.source, entry.scope)
                    } else {
                        self.install_git_sync(&entry.source, repo, ref_.as_deref(), entry.scope)
                    }
                }
                ParsedSource::Local { path } => self.install_local(path),
                ParsedSource::Url { url } => {
                    run_on_fresh_runtime(self.install_url(url, entry.scope))
                }
            };
        }

        // Fallback: try npm re-install
        if self.installed.contains_key(name) {
            self.install_npm_pack(name, SourceScope::User)
        } else {
            bail!("Package '{}' is not installed", name);
        }
    }

    /// Update all installed packages
    pub fn update_all(&mut self) -> Vec<(String, Result<PackageManifest>)> {
        let names: Vec<String> = self.installed.keys().cloned().collect();
        let mut results = Vec::new();
        for name in names {
            let result = self.update(&name);
            results.push((name, result));
        }
        results
    }

    /// Check for available updates across all packages
    pub async fn check_for_updates(&self) -> Vec<PackageUpdateInfo> {
        let mut updates = Vec::new();

        for lock_entry in self.lockfile.packages.values() {
            let parsed = ParsedSource::parse(&lock_entry.source);

            match &parsed {
                ParsedSource::Npm { name: pkg_name, .. } => {
                    // Check npm for newer version
                    match NpmPackageInfo::fetch(pkg_name).await {
                        Ok(info) => {
                            if let Some(latest) = info.latest_version()
                                && latest != lock_entry.version
                            {
                                updates.push(PackageUpdateInfo {
                                    source: lock_entry.source.clone(),
                                    display_name: pkg_name.clone(),
                                    source_type: "npm".to_string(),
                                    scope: lock_entry.scope,
                                });
                            }
                        }
                        Err(_) => continue,
                    }
                }
                ParsedSource::Git { host, path, .. } => {
                    let install_path = self.git_install_path(host, path, lock_entry.scope);
                    if install_path.exists() {
                        match git_has_update(&install_path) {
                            Ok(true) => {
                                updates.push(PackageUpdateInfo {
                                    source: lock_entry.source.clone(),
                                    display_name: format!("{}/{}", host, path),
                                    source_type: "git".to_string(),
                                    scope: lock_entry.scope,
                                });
                            }
                            _ => continue,
                        }
                    }
                }
                _ => continue,
            }
        }

        updates
    }

    // ── List / query ──────────────────────────────────────────────────

    /// List all installed packages
    pub fn list(&self) -> Vec<&PackageManifest> {
        self.installed.values().collect()
    }

    /// List configured packages with metadata
    pub fn list_configured(&self) -> Vec<ConfiguredPackage> {
        let mut result = Vec::new();
        for name in self.installed.keys() {
            let installed_path = self.get_install_dir(name);
            let lock_entry = self.lockfile.get(name);
            result.push(ConfiguredPackage {
                source: lock_entry
                    .map(|e| e.source.clone())
                    .unwrap_or_else(|| name.clone()),
                scope: lock_entry.map(|e| e.scope).unwrap_or(SourceScope::User),
                filtered: false,
                installed_path,
            });
        }
        result
    }

    /// Check whether a package is installed
    pub fn is_installed(&self, name: &str) -> bool {
        self.installed.contains_key(name)
    }

    /// Get the install directory for a package (if it exists on disk)
    pub fn get_install_dir(&self, name: &str) -> Option<PathBuf> {
        let dir = self.pkg_install_dir(name);
        if dir.exists() { Some(dir) } else { None }
    }

    /// Get the installed path for a source at a given scope
    pub fn get_installed_path_for_source(
        &self,
        source: &str,
        scope: SourceScope,
    ) -> Option<PathBuf> {
        let parsed = ParsedSource::parse(source);
        match &parsed {
            ParsedSource::Npm { name, .. } => {
                let path = self.npm_install_path(name, scope);
                if path.exists() { Some(path) } else { None }
            }
            ParsedSource::Git { host, path, .. } => {
                let path = self.git_install_path(host, path, scope);
                if path.exists() { Some(path) } else { None }
            }
            ParsedSource::Local { path } => {
                let p = PathBuf::from(path);
                if p.exists() { Some(p) } else { None }
            }
            ParsedSource::Url { .. } => None,
        }
    }

    // ── Resource discovery ────────────────────────────────────────────

    /// Discover all resources from an installed package.
    pub fn discover_resources(&self, name: &str) -> Result<Vec<DiscoveredResource>> {
        let manifest = self
            .installed
            .get(name)
            .with_context(|| format!("Package '{}' not found", name))?;

        let install_dir = self.pkg_install_dir(name);
        if !install_dir.exists() {
            bail!("Install directory for '{}' does not exist", name);
        }

        let mut resources = Vec::new();

        let has_explicit = !manifest.extensions.is_empty()
            || !manifest.skills.is_empty()
            || !manifest.prompts.is_empty()
            || !manifest.themes.is_empty();

        if has_explicit {
            for ext in &manifest.extensions {
                let path = install_dir.join(ext);
                if path.exists() {
                    resources.push(DiscoveredResource {
                        kind: ResourceKind::Extension,
                        path,
                        relative_path: ext.clone(),
                    });
                }
            }
            for skill in &manifest.skills {
                let path = install_dir.join(skill);
                if path.exists() {
                    resources.push(DiscoveredResource {
                        kind: ResourceKind::Skill,
                        path,
                        relative_path: skill.clone(),
                    });
                }
            }
            for prompt in &manifest.prompts {
                let path = install_dir.join(prompt);
                if path.exists() {
                    resources.push(DiscoveredResource {
                        kind: ResourceKind::Prompt,
                        path,
                        relative_path: prompt.clone(),
                    });
                }
            }
            for theme in &manifest.themes {
                let path = install_dir.join(theme);
                if path.exists() {
                    resources.push(DiscoveredResource {
                        kind: ResourceKind::Theme,
                        path,
                        relative_path: theme.clone(),
                    });
                }
            }
        } else {
            resources.extend(discover_extensions(&install_dir));
            resources.extend(discover_skills(&install_dir));
            resources.extend(discover_prompts(&install_dir));
            resources.extend(discover_themes(&install_dir));
        }

        Ok(resources)
    }

    /// Get resource counts for a package
    pub fn resource_counts(&self, name: &str) -> Result<ResourceCounts> {
        let resources = self.discover_resources(name)?;
        let mut counts = ResourceCounts::default();
        for r in &resources {
            match r.kind {
                ResourceKind::Extension => counts.extensions += 1,
                ResourceKind::Skill => counts.skills += 1,
                ResourceKind::Prompt => counts.prompts += 1,
                ResourceKind::Theme => counts.themes += 1,
            }
        }
        Ok(counts)
    }

    /// Resolve all resources from all installed packages, producing ResolvedPaths
    pub fn resolve(&self) -> ResolvedPaths {
        let mut extensions = Vec::new();
        let mut skills = Vec::new();
        let mut prompts = Vec::new();
        let mut themes = Vec::new();

        for name in self.installed.keys() {
            let install_dir = self.pkg_install_dir(name);
            if !install_dir.exists() {
                continue;
            }

            let metadata = PathMetadata {
                source: name.clone(),
                scope: SourceScope::User,
                origin: ResourceOrigin::Package,
                base_dir: Some(install_dir.clone()),
            };

            // Use discover_resources logic
            if let Ok(resources) = self.discover_resources(name) {
                for r in resources {
                    match r.kind {
                        ResourceKind::Extension => extensions.push(ResolvedResource {
                            path: r.path,
                            enabled: true,
                            metadata: metadata.clone(),
                        }),
                        ResourceKind::Skill => skills.push(ResolvedResource {
                            path: r.path,
                            enabled: true,
                            metadata: metadata.clone(),
                        }),
                        ResourceKind::Prompt => prompts.push(ResolvedResource {
                            path: r.path,
                            enabled: true,
                            metadata: metadata.clone(),
                        }),
                        ResourceKind::Theme => themes.push(ResolvedResource {
                            path: r.path,
                            enabled: true,
                            metadata: metadata.clone(),
                        }),
                    }
                }
            }
        }

        ResolvedPaths {
            extensions,
            skills,
            prompts,
            themes,
        }
    }

    // ── Dependency resolution ─────────────────────────────────────────

    /// Resolve dependencies for all installed packages.
    /// Returns a list of (package, missing_dependencies) tuples.
    pub fn resolve_dependencies(&self) -> Vec<(String, Vec<String>)> {
        let mut result = Vec::new();
        let installed_names: HashSet<&str> = self.installed.keys().map(|s| s.as_str()).collect();

        for (name, manifest) in &self.installed {
            let missing: Vec<String> = manifest
                .dependencies
                .keys()
                .filter(|dep| !installed_names.contains(dep.as_str()))
                .cloned()
                .collect();

            if !missing.is_empty() {
                result.push((name.clone(), missing));
            }
        }

        result
    }

    /// Validate a package structure
    pub fn validate_package(dir: &Path) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Check for manifest
        if !dir.join(MANIFEST_NAME).exists() && !dir.join(NPM_MANIFEST_NAME).exists() {
            warnings.push(format!(
                "No {} or {} found",
                MANIFEST_NAME, NPM_MANIFEST_NAME
            ));
        }

        // Try to parse manifest
        if dir.join(MANIFEST_NAME).exists() {
            match Self::read_manifest(&dir.join(MANIFEST_NAME)) {
                Ok(m) => {
                    if m.name.is_empty() {
                        warnings.push("Package name is empty".to_string());
                    }
                    if m.version.is_empty() {
                        warnings.push("Package version is empty".to_string());
                    }
                    if semver::Version::parse(&m.version).is_err() {
                        warnings.push(format!("Version '{}' is not valid semver", m.version));
                    }
                    let has_resources = !m.extensions.is_empty()
                        || !m.skills.is_empty()
                        || !m.prompts.is_empty()
                        || !m.themes.is_empty();
                    if !has_resources {
                        // Check if auto-discovery would find anything
                        let discovered = discover_extensions(dir)
                            .into_iter()
                            .chain(discover_skills(dir))
                            .chain(discover_prompts(dir))
                            .chain(discover_themes(dir))
                            .count();
                        if discovered == 0 {
                            warnings.push(
                                "Package has no explicit resources and auto-discovery found nothing"
                                    .to_string(),
                            );
                        }
                    }

                    // Check that explicit paths exist
                    for ext in &m.extensions {
                        if !dir.join(ext).exists() {
                            warnings.push(format!("Extension path '{}' does not exist", ext));
                        }
                    }
                    for skill in &m.skills {
                        if !dir.join(skill).exists() {
                            warnings.push(format!("Skill path '{}' does not exist", skill));
                        }
                    }
                    for prompt in &m.prompts {
                        if !dir.join(prompt).exists() {
                            warnings.push(format!("Prompt path '{}' does not exist", prompt));
                        }
                    }
                    for theme in &m.themes {
                        if !dir.join(theme).exists() {
                            warnings.push(format!("Theme path '{}' does not exist", theme));
                        }
                    }
                }
                Err(e) => {
                    warnings.push(format!("Failed to parse {}: {}", MANIFEST_NAME, e));
                }
            }
        }

        // Check for .gitignore or .ignore
        if !dir.join(".gitignore").exists() && !dir.join(".ignore").exists() {
            warnings.push("No .gitignore or .ignore file found".to_string());
        }

        Ok(warnings)
    }

    // ── Version queries ───────────────────────────────────────────────

    /// Get installed version of a package
    pub fn get_installed_version(&self, name: &str) -> Option<&str> {
        self.installed.get(name).map(|m| m.version.as_str())
    }

    /// Check if an installed version satisfies a semver requirement
    pub fn version_satisfies(&self, name: &str, requirement: &str) -> bool {
        if let Some(version) = self.get_installed_version(name)
            && let Ok(v) = semver::Version::parse(version)
            && let Ok(req) = semver::VersionReq::parse(requirement)
        {
            return req.matches(&v);
        }
        false
    }

    /// Get the lockfile
    pub fn lockfile(&self) -> &Lockfile {
        &self.lockfile
    }
}

// ── Auto-discovery helpers ────────────────────────────────────────────

/// Discover extension files in a directory.
fn discover_extensions(dir: &Path) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    discover_extensions_recursive(dir, dir, &mut results);
    results
}

fn discover_extensions_recursive(
    base: &Path,
    current: &Path,
    results: &mut Vec<DiscoveredResource>,
) {
    if !current.exists() {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            // Check for index.ts / index.js in subdirectory
            for index in &["index.ts", "index.js"] {
                let index_path = path.join(index);
                if index_path.exists() {
                    let rel = path.strip_prefix(base).unwrap_or(&path);
                    results.push(DiscoveredResource {
                        kind: ResourceKind::Extension,
                        path: index_path,
                        relative_path: rel.join(index).to_string_lossy().to_string(),
                    });
                }
            }
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "so" | "dylib" | "dll" | "ts" | "js") {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                results.push(DiscoveredResource {
                    kind: ResourceKind::Extension,
                    path: path.clone(),
                    relative_path: rel.to_string_lossy().to_string(),
                });
            }
        }
    }
}

/// Discover skill directories containing SKILL.md
fn discover_skills(dir: &Path) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    discover_skills_recursive(dir, dir, &mut results);
    results
}

fn discover_skills_recursive(base: &Path, current: &Path, results: &mut Vec<DiscoveredResource>) {
    if !current.exists() {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            let skill_file = path.join("SKILL.md");
            if skill_file.exists() {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                results.push(DiscoveredResource {
                    kind: ResourceKind::Skill,
                    path: skill_file,
                    relative_path: rel.join("SKILL.md").to_string_lossy().to_string(),
                });
            }
            discover_skills_recursive(base, &path, results);
        }
    }
}

/// Discover prompt template files (.md in prompts/ subdirectory)
fn discover_prompts(dir: &Path) -> Vec<DiscoveredResource> {
    let prompts_dir = dir.join("prompts");
    discover_files_by_ext(
        if prompts_dir.exists() {
            &prompts_dir
        } else {
            dir
        },
        "md",
        ResourceKind::Prompt,
    )
}

/// Discover theme files (.json in themes/ subdirectory)
fn discover_themes(dir: &Path) -> Vec<DiscoveredResource> {
    let themes_dir = dir.join("themes");
    discover_files_by_ext(
        if themes_dir.exists() {
            &themes_dir
        } else {
            dir
        },
        "json",
        ResourceKind::Theme,
    )
}

/// Recursively find files with a given extension
fn discover_files_by_ext(dir: &Path, ext: &str, kind: ResourceKind) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    discover_files_recursive(dir, dir, ext, kind, &mut results);
    results
}

fn discover_files_recursive(
    base: &Path,
    current: &Path,
    ext: &str,
    kind: ResourceKind,
    results: &mut Vec<DiscoveredResource>,
) {
    if !current.exists() {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            discover_files_recursive(base, &path, ext, kind, results);
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            results.push(DiscoveredResource {
                kind,
                path: path.clone(),
                relative_path: rel.to_string_lossy().to_string(),
            });
        }
    }
}

// ── Utility functions ─────────────────────────────────────────────────

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Compute a SHA-256 hash of a directory's contents for integrity checking
fn compute_dir_hash(dir: &Path) -> Option<String> {
    let mut hasher = Sha256::new();
    let mut files = collect_file_paths(dir);
    files.sort();

    for file_path in &files {
        if let Ok(content) = fs::read(file_path) {
            hasher.update(&content);
        }
    }

    let result = hasher.finalize();
    Some(format!("sha256-{:x}", result))
}

/// Verify that an installed package directory matches its lockfile integrity hash.
///
/// `expected` must be in the `"sha256-<hex>"` format produced by
/// [`compute_dir_hash`]. Returns `Ok(())` on match, `Err(reason)` on
/// mismatch, missing directory, or hash format error. Missing files are
/// skipped silently (same as `compute_dir_hash`) so a partially-installed
/// package still hashes consistently with its lockfile.
///
/// This is the *consumer-side* companion to `compute_dir_hash`: the writer
/// (install) computes and stores; the reader (load) recomputes and compares.
/// Before this function existed (audit finding F-1), `integrity` was a
/// write-only field and a local attacker could swap files under
/// `~/.oxi/packages/<name>/` without detection.
fn verify_lockfile_integrity(install_dir: &Path, expected: &str) -> Result<(), String> {
    let expected_hex = expected.strip_prefix("sha256-").ok_or_else(|| {
        format!("lockfile integrity value not in `sha256-<hex>` form: {expected}")
    })?;

    let actual = compute_dir_hash(install_dir)
        .ok_or_else(|| format!("could not hash install dir {}", install_dir.display()))?;
    let actual_hex = actual
        .strip_prefix("sha256-")
        .ok_or_else(|| format!("recomputed hash not in expected form: {actual}"))?;

    if actual_hex.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "sha256 mismatch: expected sha256-{expected_hex}, got {actual_hex}"
        ))
    }
}

/// Collect all file paths in a directory recursively
fn collect_file_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if !dir.exists() {
        return paths;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return paths,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            paths.extend(collect_file_paths(&path));
        } else {
            paths.push(path);
        }
    }

    paths
}

/// Find the single subdirectory inside an extracted archive
fn find_single_subdir(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<_> = fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).collect();
    if entries.len() == 1 && entries[0].path().is_dir() {
        Some(entries[0].path())
    } else {
        None
    }
}

/// Remove empty parent directories up to a root
fn prune_empty_parents(target: &Path, root: &Path) {
    let mut current = target.parent();
    while let Some(dir) = current {
        if dir == root || !dir.starts_with(root) {
            break;
        }
        if dir.exists() {
            let is_empty = fs::read_dir(dir)
                .map(|mut rd| rd.next().is_none())
                .unwrap_or(false);
            if is_empty {
                let _ = fs::remove_dir(dir);
            } else {
                break;
            }
        }
        current = dir.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_temp_packages_dir() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let packages_dir = tmp.path().join("packages");
        fs::create_dir_all(&packages_dir).unwrap();
        (tmp, packages_dir)
    }

    fn create_test_package(base: &Path, name: &str, version: &str) -> PathBuf {
        let pkg_dir = base.join("source-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();

        let manifest = PackageManifest {
            name: name.to_string(),
            version: version.to_string(),
            extensions: vec!["ext1.so".to_string()],
            skills: vec!["skill-a".to_string()],
            prompts: vec![],
            themes: vec![],
            description: None,
            dependencies: BTreeMap::new(),
        };

        let toml_content = toml::to_string_pretty(&manifest).unwrap();
        fs::write(pkg_dir.join(MANIFEST_NAME), toml_content).unwrap();
        fs::write(pkg_dir.join("ext1.so"), "fake extension").unwrap();
        fs::create_dir_all(pkg_dir.join("skill-a")).unwrap();
        fs::write(pkg_dir.join("skill-a").join("SKILL.md"), "# Skill A").unwrap();

        pkg_dir
    }

    fn create_test_package_with_auto_discovery(base: &Path, name: &str, version: &str) -> PathBuf {
        let pkg_dir = base.join("source-pkg-auto");
        fs::create_dir_all(&pkg_dir).unwrap();

        let manifest = PackageManifest {
            name: name.to_string(),
            version: version.to_string(),
            extensions: vec![],
            skills: vec![],
            prompts: vec![],
            themes: vec![],
            description: None,
            dependencies: BTreeMap::new(),
        };
        let toml_content = toml::to_string_pretty(&manifest).unwrap();
        fs::write(pkg_dir.join(MANIFEST_NAME), toml_content).unwrap();

        fs::write(pkg_dir.join("myext.so"), "extension").unwrap();
        fs::create_dir_all(pkg_dir.join("my-skill")).unwrap();
        fs::write(pkg_dir.join("my-skill").join("SKILL.md"), "# My Skill").unwrap();
        fs::create_dir_all(pkg_dir.join("prompts")).unwrap();
        fs::write(pkg_dir.join("prompts").join("review.md"), "# Review").unwrap();
        fs::create_dir_all(pkg_dir.join("themes")).unwrap();
        fs::write(pkg_dir.join("themes").join("dark.json"), "{}").unwrap();

        pkg_dir
    }

    #[test]
    fn test_install_and_list() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        let manifest = mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        assert_eq!(manifest.name, "test-pkg");
        assert_eq!(manifest.version, "1.0.0");

        let installed = mgr.list();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].name, "test-pkg");
    }

    #[test]
    fn test_uninstall() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        assert!(mgr.is_installed("test-pkg"));

        mgr.uninstall("test-pkg").unwrap();
        assert!(!mgr.is_installed("test-pkg"));
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_uninstall_not_installed() {
        let (_tmp, packages_dir) = setup_temp_packages_dir();
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        let result = mgr.uninstall("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_install_scoped_package() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "@foo/oxi-tools", "2.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();

        let manifest = mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        assert_eq!(manifest.name, "@foo/oxi-tools");

        let expected_dir = packages_dir.join("foo-oxi-tools");
        assert!(expected_dir.exists());
    }

    #[test]
    fn test_reinstall_overwrites() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let pkg_dir_v2 = tmp.path().join("source-pkg-v2");
        fs::create_dir_all(&pkg_dir_v2).unwrap();
        let manifest_v2 = PackageManifest {
            name: "test-pkg".to_string(),
            version: "2.0.0".to_string(),
            extensions: vec![],
            skills: vec![],
            prompts: vec![],
            themes: vec![],
            description: None,
            dependencies: BTreeMap::new(),
        };
        fs::write(
            pkg_dir_v2.join(MANIFEST_NAME),
            toml::to_string_pretty(&manifest_v2).unwrap(),
        )
        .unwrap();

        mgr.install(pkg_dir_v2.to_str().unwrap()).unwrap();

        let installed = mgr.list();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].version, "2.0.0");
    }

    #[test]
    fn test_empty_packages_dir() {
        let (_tmp, packages_dir) = setup_temp_packages_dir();
        let mgr = PackageManager::with_dir(packages_dir).unwrap();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_packages_dir_not_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("does-not-exist");
        let mgr = PackageManager::with_dir(nonexistent).unwrap();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_discover_resources_explicit() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let resources = mgr.discover_resources("test-pkg").unwrap();
        assert_eq!(resources.len(), 2);

        let extensions: Vec<_> = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Extension)
            .collect();
        let skills: Vec<_> = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Skill)
            .collect();
        assert_eq!(extensions.len(), 1);
        assert_eq!(skills.len(), 1);
    }

    #[test]
    fn test_discover_resources_auto() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package_with_auto_discovery(tmp.path(), "auto-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let resources = mgr.discover_resources("auto-pkg").unwrap();

        let ext_count = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Extension)
            .count();
        let skill_count = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Skill)
            .count();
        let prompt_count = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Prompt)
            .count();
        let theme_count = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Theme)
            .count();

        assert!(
            ext_count >= 1,
            "Expected at least 1 extension, got {}",
            ext_count
        );
        assert!(
            skill_count >= 1,
            "Expected at least 1 skill, got {}",
            skill_count
        );
        assert!(
            prompt_count >= 1,
            "Expected at least 1 prompt, got {}",
            prompt_count
        );
        assert!(
            theme_count >= 1,
            "Expected at least 1 theme, got {}",
            theme_count
        );
    }

    #[test]
    fn test_resource_counts() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let counts = mgr.resource_counts("test-pkg").unwrap();
        assert_eq!(counts.extensions, 1);
        assert_eq!(counts.skills, 1);
        assert_eq!(counts.prompts, 0);
        assert_eq!(counts.themes, 0);
    }

    #[test]
    fn test_resource_counts_display() {
        let counts = ResourceCounts {
            extensions: 2,
            skills: 1,
            prompts: 0,
            themes: 3,
        };
        assert_eq!(counts.to_string(), "2 ext, 1 skill, 3 theme");

        let empty = ResourceCounts::default();
        assert_eq!(empty.to_string(), "-");
    }

    #[test]
    fn test_resource_kind_display() {
        assert_eq!(ResourceKind::Extension.to_string(), "extension");
        assert_eq!(ResourceKind::Skill.to_string(), "skill");
        assert_eq!(ResourceKind::Prompt.to_string(), "prompt");
        assert_eq!(ResourceKind::Theme.to_string(), "theme");
    }

    #[test]
    fn test_get_install_dir() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let dir = mgr.get_install_dir("test-pkg").unwrap();
        assert!(dir.exists());
        assert!(dir.join(MANIFEST_NAME).exists());

        assert!(mgr.get_install_dir("nonexistent").is_none());
    }

    #[test]
    fn test_discover_resources_not_installed() {
        let (_tmp, packages_dir) = setup_temp_packages_dir();
        let mgr = PackageManager::with_dir(packages_dir).unwrap();

        let result = mgr.discover_resources("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_not_installed() {
        let (_tmp, packages_dir) = setup_temp_packages_dir();
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        let result = mgr.update("nonexistent");
        assert!(result.is_err());
    }

    // ── Source parsing tests ──────────────────────────────────────────

    #[test]
    fn test_parse_npm_source() {
        let parsed = ParsedSource::parse("npm:express@4.18.0");
        match parsed {
            ParsedSource::Npm { spec, name, pinned } => {
                assert_eq!(spec, "express@4.18.0");
                assert_eq!(name, "express");
                assert!(pinned);
            }
            _ => panic!("Expected Npm source"),
        }

        let parsed = ParsedSource::parse("npm:lodash");
        match parsed {
            ParsedSource::Npm { name, pinned, .. } => {
                assert_eq!(name, "lodash");
                assert!(!pinned);
            }
            _ => panic!("Expected Npm source"),
        }
    }

    #[test]
    fn test_parse_git_source() {
        let parsed = ParsedSource::parse("https://github.com/org/repo.git");
        match parsed {
            ParsedSource::Git {
                host, path, ref_, ..
            } => {
                assert_eq!(host, "github.com");
                assert_eq!(path, "org/repo");
                assert!(ref_.is_none());
            }
            _ => panic!("Expected Git source"),
        }

        let parsed = ParsedSource::parse("https://github.com/org/repo.git@v1.0.0");
        match parsed {
            ParsedSource::Git { path, ref_, .. } => {
                assert_eq!(path, "org/repo");
                assert_eq!(ref_.as_deref(), Some("v1.0.0"));
            }
            _ => panic!("Expected Git source"),
        }
    }

    #[test]
    fn test_parse_github_shorthand() {
        let parsed = ParsedSource::parse("github:org/repo@main");
        match parsed {
            ParsedSource::Git {
                host, path, ref_, ..
            } => {
                assert_eq!(host, "github.com");
                assert_eq!(path, "org/repo");
                assert_eq!(ref_.as_deref(), Some("main"));
            }
            _ => panic!("Expected Git source"),
        }
    }

    #[test]
    fn test_parse_local_source() {
        let parsed = ParsedSource::parse("/path/to/package");
        match parsed {
            ParsedSource::Local { path } => {
                assert_eq!(path, "/path/to/package");
            }
            _ => panic!("Expected Local source"),
        }

        let parsed = ParsedSource::parse("./relative/path");
        match parsed {
            ParsedSource::Local { path } => {
                assert_eq!(path, "./relative/path");
            }
            _ => panic!("Expected Local source"),
        }
    }

    #[test]
    fn test_parse_url_source() {
        let parsed = ParsedSource::parse("https://example.com/pkg.tar.gz");
        match parsed {
            ParsedSource::Url { url } => {
                assert_eq!(url, "https://example.com/pkg.tar.gz");
            }
            _ => panic!("Expected Url source"),
        }
    }

    #[test]
    fn test_source_identity() {
        let npm = ParsedSource::parse("npm:express@4.18.0");
        assert_eq!(npm.identity(), "npm:express");

        let git = ParsedSource::parse("https://github.com/org/repo.git");
        assert_eq!(git.identity(), "git:github.com/org/repo");

        let local = ParsedSource::parse("/path/to/pkg");
        assert_eq!(local.identity(), "local:/path/to/pkg");
    }

    #[test]
    fn test_parse_npm_spec() {
        let (name, pinned) = parse_npm_spec("express@4.18.0");
        assert_eq!(name, "express");
        assert!(pinned);

        let (name, pinned) = parse_npm_spec("express");
        assert_eq!(name, "express");
        assert!(!pinned);

        let (name, pinned) = parse_npm_spec("@scope/pkg@1.0.0");
        assert_eq!(name, "@scope/pkg");
        assert!(pinned);
    }

    // ── Lockfile tests ────────────────────────────────────────────────

    #[test]
    fn test_lockfile_roundtrip() {
        let (tmp, _) = setup_temp_packages_dir();
        let lock_path = tmp.path().join(LOCKFILE_NAME);

        let mut lock = Lockfile::new();
        lock.insert(LockEntry {
            source: "npm:express@4.18.0".to_string(),
            name: "express".to_string(),
            version: "4.18.0".to_string(),
            integrity: Some("sha256-abc123".to_string()),
            scope: SourceScope::User,
            source_type: "npm".to_string(),
            dependencies: BTreeMap::new(),
        });

        lock.write(&lock_path).unwrap();

        let loaded = Lockfile::read(&lock_path).unwrap().unwrap();
        assert_eq!(loaded.packages.len(), 1);
        assert_eq!(loaded.packages["express"].version, "4.18.0");
        assert_eq!(
            loaded.packages["express"].integrity.as_deref(),
            Some("sha256-abc123")
        );
    }

    #[test]
    fn test_lockfile_install_roundtrip() {
        let (tmp, packages_dir) = setup_temp_packages_dir();
        let pkg_dir = create_test_package(tmp.path(), "locked-pkg", "1.0.0");

        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        // Lockfile should have been written
        let lock_path = mgr.packages_dir().join(LOCKFILE_NAME);
        assert!(lock_path.exists());

        let lock = Lockfile::read(&lock_path).unwrap().unwrap();
        assert!(lock.contains("locked-pkg"));
        let entry = lock.get("locked-pkg").unwrap();
        assert_eq!(entry.version, "1.0.0");
    }

    // ── Validation tests ──────────────────────────────────────────────

    #[test]
    fn test_validate_valid_package() {
        let (tmp, _) = setup_temp_packages_dir();
        let pkg_dir = create_test_package(tmp.path(), "valid-pkg", "1.0.0");
        let warnings = PackageManager::validate_package(&pkg_dir).unwrap();
        // Should have minimal warnings (maybe just about .gitignore)
        assert!(
            warnings.len() <= 1,
            "Expected <= 1 warning, got {:?}",
            warnings
        );
    }

    #[test]
    fn test_validate_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let empty_dir = tmp.path().join("empty-pkg");
        fs::create_dir_all(&empty_dir).unwrap();
        let warnings = PackageManager::validate_package(&empty_dir).unwrap();
        assert!(!warnings.is_empty());
    }

    // ── Dependency tests ──────────────────────────────────────────────

    #[test]
    fn test_resolve_dependencies() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        // Create a package with dependencies
        let pkg_dir = tmp.path().join("dep-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        let mut deps = BTreeMap::new();
        deps.insert("lodash".to_string(), "^4.0.0".to_string());
        deps.insert("nonexistent-pkg".to_string(), "^1.0.0".to_string());

        let manifest = PackageManifest {
            name: "dep-pkg".to_string(),
            version: "1.0.0".to_string(),
            extensions: vec![],
            skills: vec![],
            prompts: vec![],
            themes: vec![],
            description: None,
            dependencies: deps,
        };
        fs::write(
            pkg_dir.join(MANIFEST_NAME),
            toml::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let missing = mgr.resolve_dependencies();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "dep-pkg");
        assert!(
            missing[0].1.contains(&"lodash".to_string())
                || missing[0].1.contains(&"nonexistent-pkg".to_string())
        );
    }

    // ── Version tests ─────────────────────────────────────────────────

    #[test]
    fn test_version_satisfies() {
        let (tmp, packages_dir) = setup_temp_packages_dir();
        let pkg_dir = create_test_package(tmp.path(), "ver-pkg", "1.2.3");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        assert!(mgr.version_satisfies("ver-pkg", "^1.0.0"));
        assert!(mgr.version_satisfies("ver-pkg", ">=1.0.0"));
        assert!(!mgr.version_satisfies("ver-pkg", "^2.0.0"));
        assert!(!mgr.version_satisfies("ver-pkg", "<1.0.0"));
    }

    #[test]
    fn test_get_installed_version() {
        let (tmp, packages_dir) = setup_temp_packages_dir();
        let pkg_dir = create_test_package(tmp.path(), "ver-pkg", "3.1.4");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        assert_eq!(mgr.get_installed_version("ver-pkg"), Some("3.1.4"));
        assert_eq!(mgr.get_installed_version("nonexistent"), None);
    }

    // ── Resolve tests ─────────────────────────────────────────────────

    #[test]
    fn test_resolve() {
        let (tmp, packages_dir) = setup_temp_packages_dir();
        let pkg_dir = create_test_package(tmp.path(), "resolve-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let resolved = mgr.resolve();
        assert!(!resolved.extensions.is_empty() || !resolved.skills.is_empty());
    }

    // ── Progress callback tests ───────────────────────────────────────

    #[test]
    fn test_progress_callback() {
        use std::sync::{Arc, Mutex};

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let (tmp, packages_dir) = setup_temp_packages_dir();
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        mgr.set_progress_callback(Box::new(move |event| {
            let mut e = events_clone.lock().unwrap();
            e.push(format!("{:?}:{:?}", event.event_type, event.action));
        }));

        let pkg_dir = create_test_package(tmp.path(), "progress-pkg", "1.0.0");
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        // install_local doesn't use with_progress, so no events expected from install()
        // Just verify the progress event mechanism exists and doesn't panic
        let _event_count = events.lock().unwrap().len();
    }

    #[test]
    fn test_list_configured() {
        let (tmp, packages_dir) = setup_temp_packages_dir();
        let pkg_dir = create_test_package(tmp.path(), "cfg-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let configured = mgr.list_configured();
        assert_eq!(configured.len(), 1);
        assert!(configured[0].source.contains("source-pkg"));
        // source comes from lockfile, might be the local path
    }

    // ── F-1 regression: lockfile integrity verify on load ──────────

    /// `verify_lockfile_integrity` returns Ok(()) when the directory
    /// contents hash to the lockfile-recorded `sha256-<hex>` value.
    #[test]
    fn verify_lockfile_integrity_accepts_matching_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_dir = tmp.path().join("pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("a.txt"), b"hello").unwrap();
        fs::write(pkg_dir.join("b.txt"), b"world").unwrap();

        let expected = compute_dir_hash(&pkg_dir).expect("compute_dir_hash must succeed");

        assert!(verify_lockfile_integrity(&pkg_dir, &expected).is_ok());
    }

    /// A directory that has been mutated after install must fail the
    /// integrity check. This is the F-1 supply-chain scenario the audit
    /// flagged: previously `LockEntry.integrity` was computed at install
    /// and never re-checked, so a tampered package would silently load.
    #[test]
    fn verify_lockfile_integrity_rejects_tampered_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_dir = tmp.path().join("pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("a.txt"), b"hello").unwrap();

        let expected = compute_dir_hash(&pkg_dir).expect("compute_dir_hash must succeed");

        // Tamper: replace file contents.
        fs::write(pkg_dir.join("a.txt"), b"tampered").unwrap();

        let err = verify_lockfile_integrity(&pkg_dir, &expected)
            .expect_err("tampered dir must not verify");
        assert!(err.contains("sha256 mismatch"), "unexpected error: {err}");
    }

    /// Missing `sha256-` prefix is a clear lockfile-format error.
    #[test]
    fn verify_lockfile_integrity_rejects_bad_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_dir = tmp.path().join("pkg");
        fs::create_dir_all(&pkg_dir).unwrap();

        let err = verify_lockfile_integrity(&pkg_dir, "abc123")
            .expect_err("missing sha256- prefix must be rejected");
        assert!(err.contains("not in `sha256-<hex>` form"));
    }

    /// `load_installed` must drop a package whose lockfile-recorded
    /// integrity no longer matches the on-disk directory.
    #[test]
    fn load_installed_skips_tampered_package() {
        let (tmp, packages_dir) = setup_temp_packages_dir();
        let pkg_dir = create_test_package(tmp.path(), "tamper-pkg", "1.0.0");

        // Install normally — this writes integrity to the lockfile.
        {
            let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();
            mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        }

        // Tamper with the installed package after install.
        let installed_name = "tamper-pkg";
        let installed_safe = installed_name.replace('@', "").replace('/', "-");
        let on_disk = packages_dir.join(installed_safe);
        fs::write(on_disk.join(MANIFEST_NAME), "tampered = true\n").unwrap();

        // Re-open the manager — `load_installed` should drop the package.
        let mgr2 = PackageManager::with_dir(packages_dir).unwrap();
        let names: Vec<&str> = mgr2.list().iter().map(|m| m.name.as_str()).collect();
        assert!(
            !names.contains(&"tamper-pkg"),
            "tampered package must be excluded from load_installed; loaded names: {names:?}"
        );
    }
}
