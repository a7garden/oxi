//! Scout skill for oxi — fast codebase reconnaissance
//!
//! Produces a compressed snapshot of a codebase optimized for handoff to another
//! agent or for quick orientation. Three core capabilities:
//!
//! 1. **Fast codebase mapping** — directory tree, file counts, size metrics
//! 2. **Compressed context for handoff** — a [`CodebaseSnapshot`] that captures
//!    structure, key files, dependency graph, and detected patterns in a compact
//!    form designed to fit within context windows.
//! 3. **Pattern detection** — identifies architectural patterns, frameworks,
//!    languages, conventions, and anti-patterns.
//!
//! Usage:
//! ```ignore
//! use oxi::skills::scout::Scout;
//!
//! let scout = Scout::new("/path/to/project");
//! let snapshot = scout.scan()?;
//! println!("{}", Scout::render_compact(&snapshot));
//! ```

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Public types ─────────────────────────────────────────────────────

/// Configuration for a scout scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoutConfig {
    /// Root directory to scan.
    pub root: PathBuf,

    /// Maximum directory depth to traverse (default: 6).
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,

    /// Maximum total bytes of file content to sample for pattern detection
    /// (default: 512 KiB).
    #[serde(default = "default_max_sample_bytes")]
    pub max_sample_bytes: usize,

    /// Maximum number of files to include in the tree summary (default: 200).
    #[serde(default = "default_max_tree_files")]
    pub max_tree_files: usize,

    /// Directory names to ignore (case-insensitive match).
    #[serde(default = "default_ignores")]
    pub ignore: Vec<String>,
}

fn default_max_depth() -> usize {
    6
}
fn default_max_sample_bytes() -> usize {
    512 * 1024
}
fn default_max_tree_files() -> usize {
    200
}
fn default_ignores() -> Vec<String> {
    [
        ".git".into(),
        "node_modules".into(),
        "target".into(),
        "dist".into(),
        "build".into(),
        "__pycache__".into(),
        ".next".into(),
        "vendor".into(),
        "coverage".into(),
        ".cache".into(),
        ".turbo".into(),
        "bazel-bin".into(),
        "bazel-out".into(),
        ".dart_tool".into(),
        ".gradle".into(),
    ]
    .to_vec()
}

impl Default for ScoutConfig {
    fn default() -> Self {
        Self {
            root: std::env::current_dir().unwrap_or_default(),
            max_depth: default_max_depth(),
            max_sample_bytes: default_max_sample_bytes(),
            max_tree_files: default_max_tree_files(),
            ignore: default_ignores(),
        }
    }
}

/// A single detected pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    /// Human-readable name (e.g. "Rust project (Cargo)").
    pub name: String,
    /// Category: "language", "framework", "tooling", "architecture", "convention", "anti-pattern".
    pub category: String,
    /// Confidence: 0-100.
    pub confidence: u8,
    /// Short evidence for why this pattern was detected.
    pub evidence: String,
}

/// Per-language stats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageStats {
    /// Language name (e.g. "Rust").
    pub language: String,
    /// Number of files.
    pub file_count: usize,
    /// Total bytes across files.
    pub total_bytes: u64,
    /// File extensions that contributed.
    pub extensions: BTreeSet<String>,
}

/// A node in the directory tree summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// Relative path from root.
    pub path: String,
    /// File extension (empty for dirs).
    pub ext: String,
    /// Size in bytes (0 for dirs).
    pub size: u64,
    /// True if this is a directory.
    pub is_dir: bool,
    /// Number of direct children (files + dirs).
    pub child_count: usize,
}

/// The compressed result of a scout scan — everything an agent needs for orientation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodebaseSnapshot {
    /// Absolute path of the scanned root.
    pub root: String,

    /// Top-level directory tree (bounded by `max_tree_files`).
    pub tree: Vec<TreeNode>,

    /// Per-language statistics, sorted by file count descending.
    pub languages: Vec<LanguageStats>,

    /// Total file count (excluding ignored dirs).
    pub total_files: usize,

    /// Total bytes across all scanned files.
    pub total_bytes: u64,

    /// Detected patterns.
    pub patterns: Vec<Pattern>,

    /// Key files identified (config, entry points, important docs).
    pub key_files: Vec<KeyFile>,

    /// Dependency names extracted from config files.
    pub dependencies: Vec<String>,

    /// Generated at timestamp (ISO 8601).
    pub scanned_at: String,

    /// Scan duration in milliseconds.
    pub scan_ms: u64,
}

/// A key file worth highlighting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFile {
    /// Relative path from root.
    pub path: String,
    /// Role: "config", "entrypoint", "readme", "license", "ci", "test", "docs", "schema".
    pub role: String,
    /// First meaningful line (for quick orientation).
    pub summary: Option<String>,
}

// ── Scout engine ─────────────────────────────────────────────────────

/// The scout engine — performs a fast, read-only scan of a codebase.
pub struct Scout {
    config: ScoutConfig,
}

impl Scout {
    /// Create a new scout for the given root directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            config: ScoutConfig {
                root: root.into(),
                ..Default::default()
            },
        }
    }

    /// Create a scout with full configuration.
    pub fn with_config(config: ScoutConfig) -> Self {
        Self { config }
    }

    /// Run the scan and produce a [`CodebaseSnapshot`].
    pub fn scan(&self) -> Result<CodebaseSnapshot> {
        let root = &self.config.root;
        if !root.exists() {
            anyhow::bail!("Root directory does not exist: {}", root.display());
        }
        if !root.is_dir() {
            anyhow::bail!("Root is not a directory: {}", root.display());
        }

        let start = std::time::Instant::now();

        // Phase 1: Walk the tree and collect raw file info
        let mut files: Vec<FileEntry> = Vec::new();
        let mut tree: Vec<TreeNode> = Vec::new();
        self.walk(root, root, 0, &mut files, &mut tree)?;

        // Truncate tree to budget
        tree.truncate(self.config.max_tree_files);

        // Phase 2: Compute language stats
        let languages = self.compute_language_stats(&files);

        // Phase 3: Identify key files
        let key_files = self.identify_key_files(&files, root);

        // Phase 4: Extract dependencies
        let dependencies = self.extract_dependencies(&files, root);

        // Phase 5: Detect patterns
        let patterns = self.detect_patterns(&files, &key_files, &dependencies, root);

        let total_bytes: u64 = files.iter().map(|f| f.size).sum();
        let scan_ms = start.elapsed().as_millis() as u64;

        Ok(CodebaseSnapshot {
            root: root.to_string_lossy().to_string(),
            tree,
            languages,
            total_files: files.len(),
            total_bytes,
            patterns,
            key_files,
            dependencies,
            scanned_at: chrono::Utc::now().to_rfc3339(),
            scan_ms,
        })
    }

    // ── Internal: walking ────────────────────────────────────────

    fn walk(
        &self,
        root: &Path,
        dir: &Path,
        depth: usize,
        files: &mut Vec<FileEntry>,
        tree: &mut Vec<TreeNode>,
    ) -> Result<()> {
        if depth > self.config.max_depth {
            return Ok(());
        }

        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(()), // skip unreadable dirs
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();

            // Skip ignored names
            if self.should_ignore(&name) {
                continue;
            }

            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();

            if path.is_dir() {
                tree.push(TreeNode {
                    path: rel.clone(),
                    ext: String::new(),
                    size: 0,
                    is_dir: true,
                    child_count: 0,
                });
                self.walk(root, &path, depth + 1, files, tree)?;
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let ext = Path::new(&name)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();

                files.push(FileEntry {
                    rel_path: rel.clone(),
                    name,
                    ext: ext.clone(),
                    size,
                });

                if tree.len() < self.config.max_tree_files {
                    tree.push(TreeNode {
                        path: rel,
                        ext,
                        size,
                        is_dir: false,
                        child_count: 0,
                    });
                }
            }
        }

        Ok(())
    }

    fn should_ignore(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();
        // Hidden files/dirs (but allow .env.example etc.)
        if name_lower.starts_with('.')
            && name_lower != ".env.example"
            && name_lower != ".env.local.example"
        {
            return true;
        }
        // Configured ignores
        for ignore in &self.config.ignore {
            if name_lower == ignore.to_lowercase() {
                return true;
            }
        }
        false
    }

    // ── Internal: language stats ─────────────────────────────────

    fn compute_language_stats(&self, files: &[FileEntry]) -> Vec<LanguageStats> {
        let mut lang_map: BTreeMap<String, LanguageStats> = BTreeMap::new();

        for file in files {
            if let Some(lang) = self.ext_to_language(&file.ext) {
                let stats = lang_map.entry(lang.to_string()).or_insert_with(|| {
                    LanguageStats {
                        language: lang.to_string(),
                        file_count: 0,
                        total_bytes: 0,
                        extensions: BTreeSet::new(),
                    }
                });
                stats.file_count += 1;
                stats.total_bytes += file.size;
                stats.extensions.insert(file.ext.clone());
            }
        }

        let mut v: Vec<LanguageStats> = lang_map.into_values().collect();
        v.sort_by(|a, b| b.file_count.cmp(&a.file_count));
        v
    }

    fn ext_to_language(&self, ext: &str) -> Option<&'static str> {
        match ext {
            ".rs" => Some("Rust"),
            ".ts" | ".tsx" => Some("TypeScript"),
            ".js" | ".jsx" | ".mjs" | ".cjs" => Some("JavaScript"),
            ".py" | ".pyi" => Some("Python"),
            ".go" => Some("Go"),
            ".java" => Some("Java"),
            ".kt" | ".kts" => Some("Kotlin"),
            ".rb" => Some("Ruby"),
            ".php" => Some("PHP"),
            ".c" | ".h" => Some("C"),
            ".cpp" | ".cc" | ".cxx" | ".hpp" => Some("C++"),
            ".cs" => Some("C#"),
            ".swift" => Some("Swift"),
            ".scala" => Some("Scala"),
            ".sh" | ".bash" | ".zsh" => Some("Shell"),
            ".sql" => Some("SQL"),
            ".html" | ".htm" => Some("HTML"),
            ".css" | ".scss" | ".sass" | ".less" => Some("CSS"),
            ".vue" => Some("Vue"),
            ".svelte" => Some("Svelte"),
            ".dart" => Some("Dart"),
            ".lua" => Some("Lua"),
            ".r" | ".R" => Some("R"),
            ".zig" => Some("Zig"),
            ".nim" => Some("Nim"),
            ".ex" | ".exs" => Some("Elixir"),
            ".erl" => Some("Erlang"),
            ".hs" => Some("Haskell"),
            ".ml" | ".mli" => Some("OCaml"),
            ".toml" => Some("TOML"),
            ".yaml" | ".yml" => Some("YAML"),
            ".json" => Some("JSON"),
            ".xml" => Some("XML"),
            ".md" | ".mdx" => Some("Markdown"),
            _ => None,
        }
    }

    // ── Internal: key files ──────────────────────────────────────

    fn identify_key_files(&self, files: &[FileEntry], root: &Path) -> Vec<KeyFile> {
        let mut key_files: Vec<KeyFile> = Vec::new();

        let key_patterns: &[(&str, &str)] = &[
            // Config
            ("Cargo.toml", "config"),
            ("package.json", "config"),
            ("pyproject.toml", "config"),
            ("go.mod", "config"),
            ("build.gradle", "config"),
            ("build.gradle.kts", "config"),
            ("pom.xml", "config"),
            ("Makefile", "config"),
            ("CMakeLists.txt", "config"),
            ("docker-compose.yml", "config"),
            ("docker-compose.yaml", "config"),
            ("tsconfig.json", "config"),
            (".env.example", "config"),
            // Entrypoints
            ("main.rs", "entrypoint"),
            ("main.go", "entrypoint"),
            ("main.py", "entrypoint"),
            ("main.java", "entrypoint"),
            ("main.ts", "entrypoint"),
            ("main.js", "entrypoint"),
            ("index.ts", "entrypoint"),
            ("index.js", "entrypoint"),
            ("index.py", "entrypoint"),
            ("app.rs", "entrypoint"),
            ("lib.rs", "entrypoint"),
            ("mod.rs", "entrypoint"),
            // Docs
            ("README.md", "readme"),
            ("README", "readme"),
            ("README.txt", "readme"),
            ("README.rst", "readme"),
            ("LICENSE", "license"),
            ("LICENSE.md", "license"),
            ("LICENSE.txt", "license"),
            ("CHANGELOG.md", "docs"),
            ("CONTRIBUTING.md", "docs"),
            // CI
            (".github/workflows", "ci"),
            (".gitlab-ci.yml", "ci"),
            ("Jenkinsfile", "ci"),
            // Test dirs
            ("tests", "test"),
            ("test", "test"),
            ("spec", "test"),
            ("__tests__", "test"),
        ];

        for file in files {
            let name_lower = file.name.to_lowercase();

            for (pattern, role) in key_patterns {
                if name_lower == *pattern || file.rel_path.contains(pattern) {
                    let summary = self.read_file_summary(root, &file.rel_path);
                    key_files.push(KeyFile {
                        path: file.rel_path.clone(),
                        role: role.to_string(),
                        summary,
                    });
                    break;
                }
            }
        }

        // Also check for CI workflow files
        let ci_dir = root.join(".github").join("workflows");
        if ci_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&ci_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".yml") || name.ends_with(".yaml") {
                        let rel = format!(".github/workflows/{}", name);
                        if !key_files.iter().any(|kf| kf.path == rel) {
                            key_files.push(KeyFile {
                                path: rel,
                                role: "ci".to_string(),
                                summary: None,
                            });
                        }
                    }
                }
            }
        }

        // Deduplicate by path
        let mut seen = BTreeSet::new();
        key_files.retain(|kf| seen.insert(kf.path.clone()));

        // Sort: config first, then entrypoint, then readme, then rest
        key_files.sort_by(|a, b| {
            let rank = |r: &str| -> u8 {
                match r {
                    "config" => 0,
                    "entrypoint" => 1,
                    "readme" => 2,
                    "license" => 3,
                    "ci" => 4,
                    "test" => 5,
                    "docs" => 6,
                    _ => 7,
                }
            };
            rank(&a.role)
                .cmp(&rank(&b.role))
                .then_with(|| a.path.cmp(&b.path))
        });

        key_files
    }

    /// Read the first meaningful line of a file for a summary.
    fn read_file_summary(&self, root: &Path, rel_path: &str) -> Option<String> {
        let path = root.join(rel_path);
        let content = fs::read_to_string(&path).ok()?;

        for line in content.lines() {
            let trimmed = line.trim();
            // Skip empty lines, comments, frontmatter, shebangs
            if trimmed.is_empty()
                || trimmed.starts_with('#')
                || trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with("--")
                || trimmed.starts_with("---")
                || trimmed.starts_with("!")
            {
                continue;
            }
            // Truncate long lines
            if trimmed.len() > 120 {
                return Some(format!("{}…", &trimmed[..120]));
            }
            return Some(trimmed.to_string());
        }
        None
    }

    // ── Internal: dependencies ───────────────────────────────────

    fn extract_dependencies(&self, files: &[FileEntry], root: &Path) -> Vec<String> {
        let mut deps: Vec<String> = Vec::new();

        for file in files {
            match file.name.as_str() {
                "Cargo.toml" => {
                    let path = root.join(&file.rel_path);
                    if let Ok(content) = fs::read_to_string(&path) {
                        self.extract_cargo_deps(&content, &mut deps);
                    }
                }
                "package.json" => {
                    let path = root.join(&file.rel_path);
                    if let Ok(content) = fs::read_to_string(&path) {
                        self.extract_npm_deps(&content, &mut deps);
                    }
                }
                "go.mod" => {
                    let path = root.join(&file.rel_path);
                    if let Ok(content) = fs::read_to_string(&path) {
                        self.extract_go_deps(&content, &mut deps);
                    }
                }
                "pyproject.toml" => {
                    let path = root.join(&file.rel_path);
                    if let Ok(content) = fs::read_to_string(&path) {
                        self.extract_python_deps(&content, &mut deps);
                    }
                }
                _ => {}
            }
        }

        deps.sort();
        deps.dedup();
        deps
    }

    fn extract_cargo_deps(&self, content: &str, deps: &mut Vec<String>) {
        let mut in_deps = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "[dependencies]" || trimmed == "[dev-dependencies]" {
                in_deps = true;
                continue;
            }
            if trimmed.starts_with('[') {
                in_deps = false;
                continue;
            }
            if in_deps {
                if let Some((name, _)) = trimmed.split_once('=') {
                    let name = name.trim().to_string();
                    if !name.is_empty() {
                        deps.push(format!("{} (crate)", name));
                    }
                } else if let Some((name, _)) = trimmed.split_once('{') {
                    let name = name.trim().to_string();
                    if !name.is_empty() {
                        deps.push(format!("{} (crate)", name));
                    }
                }
            }
        }
    }

    fn extract_npm_deps(&self, content: &str, deps: &mut Vec<String>) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(content) {
            for section in &["dependencies", "devDependencies"] {
                if let Some(obj) = json.get(section).and_then(|v| v.as_object()) {
                    for name in obj.keys() {
                        deps.push(format!("{} (npm)", name));
                    }
                }
            }
        }
    }

    fn extract_go_deps(&self, content: &str, deps: &mut Vec<String>) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("require (") {
                continue;
            }
            if trimmed.starts_with("require ") {
                // Single-line require
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 3 && parts[0] == "require" {
                    deps.push(format!("{} (go)", parts[1]));
                }
            } else if !trimmed.starts_with("//")
                && !trimmed.starts_with(')')
                && !trimmed.starts_with("module ")
                && !trimmed.starts_with("go ")
                && !trimmed.is_empty()
            {
                // Inside require block: "github.com/foo/bar v1.2.3"
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 && parts[0].contains('/') {
                    deps.push(format!("{} (go)", parts[0]));
                }
            }
        }
    }

    fn extract_python_deps(&self, content: &str, deps: &mut Vec<String>) {
        let mut in_deps = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "[project]" || trimmed == "[tool.poetry]" {
                in_deps = false;
            }
            // Poetry-style: [tool.poetry.dependencies]
            if trimmed.starts_with('[') && trimmed.contains("dependencies") {
                in_deps = true;
                continue;
            }
            if trimmed.starts_with('[') && !trimmed.contains("dependencies") {
                in_deps = false;
                continue;
            }
            if in_deps {
                // PEP 621: dependencies = ["requests", "flask"]
                if let Some((key, value)) = trimmed.split_once('=') {
                    let key = key.trim();
                    if key == "dependencies" {
                        let cleaned = value
                            .trim()
                            .trim_start_matches('[')
                            .trim_end_matches(']');
                        for dep in cleaned.split(',') {
                            let dep = dep.trim().trim_matches('"').trim_matches('\'');
                            if !dep.is_empty() {
                                deps.push(format!("{} (pypi)", dep));
                            }
                        }
                    } else if key != "python"
                        && !key.contains("version")
                        && !key.contains("requires")
                    {
                        // Poetry-style: key = "version"
                        let name = key.to_string();
                        if !name.is_empty() {
                            deps.push(format!("{} (pypi)", name));
                        }
                    }
                }
            }
        }
    }

    // ── Internal: pattern detection ──────────────────────────────

    fn detect_patterns(
        &self,
        files: &[FileEntry],
        _key_files: &[KeyFile],
        deps: &[String],
        root: &Path,
    ) -> Vec<Pattern> {
        let mut patterns: Vec<Pattern> = Vec::new();

        // Build quick lookup maps
        let file_names: BTreeSet<&str> = files.iter().map(|f| f.name.as_str()).collect();
        let has_ext = |ext: &str| -> bool { files.iter().any(|f| f.ext == ext) };
        let has_dir = |dir_name: &str| -> bool {
            root.join(dir_name).is_dir()
                || files
                    .iter()
                    .any(|f| f.rel_path.starts_with(&format!("{}/", dir_name)))
        };
        let dep_contains = |substr: &str| -> bool {
            deps.iter()
                .any(|d| d.to_lowercase().contains(&substr.to_lowercase()))
        };

        // ── Language detection ───────────────────────────────

        if has_ext(".rs") {
            patterns.push(Pattern {
                name: "Rust".to_string(),
                category: "language".to_string(),
                confidence: 98,
                evidence: "Found .rs files".to_string(),
            });
        }

        if has_ext(".ts") || has_ext(".tsx") {
            patterns.push(Pattern {
                name: "TypeScript".to_string(),
                category: "language".to_string(),
                confidence: 97,
                evidence: "Found .ts/.tsx files".to_string(),
            });
        } else if has_ext(".js") || has_ext(".jsx") {
            patterns.push(Pattern {
                name: "JavaScript".to_string(),
                category: "language".to_string(),
                confidence: 95,
                evidence: "Found .js/.jsx files (no .ts)".to_string(),
            });
        }

        if has_ext(".py") {
            patterns.push(Pattern {
                name: "Python".to_string(),
                category: "language".to_string(),
                confidence: 97,
                evidence: "Found .py files".to_string(),
            });
        }

        if has_ext(".go") {
            patterns.push(Pattern {
                name: "Go".to_string(),
                category: "language".to_string(),
                confidence: 98,
                evidence: "Found .go files".to_string(),
            });
        }

        if has_ext(".java") {
            patterns.push(Pattern {
                name: "Java".to_string(),
                category: "language".to_string(),
                confidence: 98,
                evidence: "Found .java files".to_string(),
            });
        }

        if has_ext(".swift") {
            patterns.push(Pattern {
                name: "Swift".to_string(),
                category: "language".to_string(),
                confidence: 98,
                evidence: "Found .swift files".to_string(),
            });
        }

        // ── Framework detection ──────────────────────────────

        if file_names.contains("Cargo.toml") && has_ext(".rs") {
            if dep_contains("tokio") {
                patterns.push(Pattern {
                    name: "Async Rust (Tokio)".to_string(),
                    category: "framework".to_string(),
                    confidence: 90,
                    evidence: "tokio dependency in Cargo.toml".to_string(),
                });
            }
            if dep_contains("actix") {
                patterns.push(Pattern {
                    name: "Actix Web".to_string(),
                    category: "framework".to_string(),
                    confidence: 92,
                    evidence: "actix dependency".to_string(),
                });
            }
            if dep_contains("axum") {
                patterns.push(Pattern {
                    name: "Axum".to_string(),
                    category: "framework".to_string(),
                    confidence: 92,
                    evidence: "axum dependency".to_string(),
                });
            }
            if dep_contains("wasm") || dep_contains("leptos") {
                patterns.push(Pattern {
                    name: "WASM/Leptos".to_string(),
                    category: "framework".to_string(),
                    confidence: 85,
                    evidence: "wasm-related dependency".to_string(),
                });
            }
        }

        if file_names.contains("package.json") {
            if dep_contains("react") {
                patterns.push(Pattern {
                    name: "React".to_string(),
                    category: "framework".to_string(),
                    confidence: 95,
                    evidence: "react dependency".to_string(),
                });
            }
            if dep_contains("vue") {
                patterns.push(Pattern {
                    name: "Vue".to_string(),
                    category: "framework".to_string(),
                    confidence: 95,
                    evidence: "vue dependency".to_string(),
                });
            }
            if dep_contains("svelte") {
                patterns.push(Pattern {
                    name: "Svelte".to_string(),
                    category: "framework".to_string(),
                    confidence: 95,
                    evidence: "svelte dependency".to_string(),
                });
            }
            if dep_contains("next") {
                patterns.push(Pattern {
                    name: "Next.js".to_string(),
                    category: "framework".to_string(),
                    confidence: 95,
                    evidence: "next dependency".to_string(),
                });
            }
            if dep_contains("express") {
                patterns.push(Pattern {
                    name: "Express".to_string(),
                    category: "framework".to_string(),
                    confidence: 92,
                    evidence: "express dependency".to_string(),
                });
            }
            if dep_contains("fastify") {
                patterns.push(Pattern {
                    name: "Fastify".to_string(),
                    category: "framework".to_string(),
                    confidence: 92,
                    evidence: "fastify dependency".to_string(),
                });
            }
        }

        if has_ext(".py") {
            if dep_contains("django") {
                patterns.push(Pattern {
                    name: "Django".to_string(),
                    category: "framework".to_string(),
                    confidence: 93,
                    evidence: "django dependency".to_string(),
                });
            }
            if dep_contains("flask") {
                patterns.push(Pattern {
                    name: "Flask".to_string(),
                    category: "framework".to_string(),
                    confidence: 93,
                    evidence: "flask dependency".to_string(),
                });
            }
            if dep_contains("fastapi") {
                patterns.push(Pattern {
                    name: "FastAPI".to_string(),
                    category: "framework".to_string(),
                    confidence: 93,
                    evidence: "fastapi dependency".to_string(),
                });
            }
        }

        // ── Architecture patterns ────────────────────────────

        // Monorepo / workspace
        if file_names.contains("Cargo.toml") {
            let cargo_content =
                fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
            if cargo_content.contains("[workspace]") {
                patterns.push(Pattern {
                    name: "Rust workspace (monorepo)".to_string(),
                    category: "architecture".to_string(),
                    confidence: 95,
                    evidence: "[workspace] in Cargo.toml".to_string(),
                });
            }
        }

        // src/ layout
        if has_dir("src") {
            patterns.push(Pattern {
                name: "Standard src/ layout".to_string(),
                category: "architecture".to_string(),
                confidence: 90,
                evidence: "src/ directory present".to_string(),
            });
        }

        // lib + binary separation
        if root.join("src/lib.rs").exists() && root.join("src/main.rs").exists() {
            patterns.push(Pattern {
                name: "Lib+Binary Rust crate".to_string(),
                category: "architecture".to_string(),
                confidence: 90,
                evidence: "Both lib.rs and main.rs in src/".to_string(),
            });
        }

        // Feature modules
        let mod_dirs: Vec<&FileEntry> = files
            .iter()
            .filter(|f| {
                f.ext == ".rs"
                    && f.rel_path.starts_with("src/")
                    && f.rel_path.ends_with("/mod.rs")
            })
            .collect();
        if mod_dirs.len() >= 3 {
            patterns.push(Pattern {
                name: "Multi-module Rust project".to_string(),
                category: "architecture".to_string(),
                confidence: 85,
                evidence: format!("{} mod.rs modules found", mod_dirs.len()),
            });
        }

        // MVC / layered
        if has_dir("controllers") && has_dir("models") && has_dir("views") {
            patterns.push(Pattern {
                name: "MVC architecture".to_string(),
                category: "architecture".to_string(),
                confidence: 88,
                evidence: "Has controllers/, models/, views/ directories".to_string(),
            });
        }

        // ── Tooling patterns ─────────────────────────────────

        if has_dir(".github") {
            patterns.push(Pattern {
                name: "GitHub Actions CI".to_string(),
                category: "tooling".to_string(),
                confidence: 95,
                evidence: ".github/ directory present".to_string(),
            });
        }

        if root.join("Dockerfile").exists() {
            patterns.push(Pattern {
                name: "Dockerized".to_string(),
                category: "tooling".to_string(),
                confidence: 95,
                evidence: "Dockerfile found".to_string(),
            });
        }

        if file_names.contains("Makefile") {
            patterns.push(Pattern {
                name: "Make-based build".to_string(),
                category: "tooling".to_string(),
                confidence: 90,
                evidence: "Makefile found".to_string(),
            });
        }

        // ── Convention patterns ──────────────────────────────

        if has_dir("tests") || has_dir("test") {
            patterns.push(Pattern {
                name: "Has dedicated test directory".to_string(),
                category: "convention".to_string(),
                confidence: 95,
                evidence: "tests/ or test/ directory present".to_string(),
            });
        }

        if has_dir("docs") {
            patterns.push(Pattern {
                name: "Has docs/ directory".to_string(),
                category: "convention".to_string(),
                confidence: 90,
                evidence: "docs/ directory present".to_string(),
            });
        }

        if file_names.contains("CLIP.md")
            || file_names.contains("AGENTS.md")
            || file_names.contains("CLAUDE.md")
        {
            patterns.push(Pattern {
                name: "AI agent conventions".to_string(),
                category: "convention".to_string(),
                confidence: 92,
                evidence: "Agent config file (AGENTS.md/CLAUDE.md/CLIP.md)".to_string(),
            });
        }

        // ── Anti-pattern detection ───────────────────────────

        // Very large files
        let large_files: Vec<&FileEntry> = files.iter().filter(|f| f.size > 100_000).collect();
        if large_files.len() > 5 {
            patterns.push(Pattern {
                name: "Large files (>100KB)".to_string(),
                category: "anti-pattern".to_string(),
                confidence: 80,
                evidence: format!(
                    "{} files exceed 100KB — largest: {}",
                    large_files.len(),
                    large_files
                        .iter()
                        .max_by_key(|f| f.size)
                        .map(|f| format!("{} ({}KB)", f.rel_path, f.size / 1024))
                        .unwrap_or_default()
                ),
            });
        }

        // Mixed tab/space indentation (sample a few files)
        let mixed_indent = self.detect_mixed_indentation(root, files);
        if mixed_indent > 0 {
            patterns.push(Pattern {
                name: "Mixed indentation".to_string(),
                category: "anti-pattern".to_string(),
                confidence: 70,
                evidence: format!(
                    "{} file(s) mix tabs and spaces for indentation",
                    mixed_indent
                ),
            });
        }

        // Sort by category, then confidence descending
        patterns.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then_with(|| b.confidence.cmp(&a.confidence))
        });

        patterns
    }

    /// Check a sample of source files for mixed tabs/spaces.
    fn detect_mixed_indentation(&self, root: &Path, files: &[FileEntry]) -> usize {
        let source_exts = [".rs", ".ts", ".js", ".py", ".go", ".java", ".tsx", ".jsx"];
        let mut count = 0usize;
        let mut sampled = 0usize;
        let max_sample = 20;

        for file in files {
            if sampled >= max_sample {
                break;
            }
            if !source_exts.contains(&file.ext.as_str()) {
                continue;
            }

            let path = root.join(&file.rel_path);
            if let Ok(content) = fs::read_to_string(&path) {
                sampled += 1;
                let has_tabs = content.lines().any(|l| l.starts_with('\t'));
                let has_spaces = content
                    .lines()
                    .any(|l| l.starts_with("    ") || l.starts_with("  "));
                if has_tabs && has_spaces {
                    count += 1;
                }
            }
        }

        count
    }

    // ── Rendering ───────────────────────────────────────────────

    /// Render a compact, human-readable summary of the codebase snapshot.
    /// Designed to be used as handoff context between agents.
    pub fn render_compact(snapshot: &CodebaseSnapshot) -> String {
        let mut out = String::with_capacity(4096);

        out.push_str("╔══ Codebase Snapshot ══════════════════════╗\n");
        out.push_str(&format!("║ Root: {}\n", snapshot.root));
        out.push_str(&format!(
            "║ Files: {} | Size: {} | Scan: {}ms\n",
            snapshot.total_files,
            format_bytes(snapshot.total_bytes),
            snapshot.scan_ms
        ));
        out.push_str("╚═══════════════════════════════════════════╝\n\n");

        // Languages
        if !snapshot.languages.is_empty() {
            out.push_str("## Languages\n\n");
            out.push_str("| Language | Files | Size |\n");
            out.push_str("|----------|-------|------|\n");
            for lang in &snapshot.languages {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    lang.language,
                    lang.file_count,
                    format_bytes(lang.total_bytes)
                ));
            }
            out.push('\n');
        }

        // Patterns
        if !snapshot.patterns.is_empty() {
            out.push_str("## Detected Patterns\n\n");
            for pattern in &snapshot.patterns {
                let conf = if pattern.confidence >= 90 {
                    "●"
                } else if pattern.confidence >= 70 {
                    "◐"
                } else {
                    "○"
                };
                out.push_str(&format!(
                    "- {} **{}** [{}] — {}\n",
                    conf, pattern.name, pattern.category, pattern.evidence
                ));
            }
            out.push('\n');
        }

        // Key files
        if !snapshot.key_files.is_empty() {
            out.push_str("## Key Files\n\n");
            for kf in &snapshot.key_files {
                if let Some(ref summary) = kf.summary {
                    out.push_str(&format!(
                        "- `{}` [{}] — {}\n",
                        kf.path, kf.role, summary
                    ));
                } else {
                    out.push_str(&format!("- `{}` [{}]\n", kf.path, kf.role));
                }
            }
            out.push('\n');
        }

        // Dependencies (abbreviated)
        if !snapshot.dependencies.is_empty() {
            let display_count = 15;
            out.push_str(&format!(
                "## Dependencies ({} total)\n\n",
                snapshot.dependencies.len()
            ));
            for dep in snapshot.dependencies.iter().take(display_count) {
                out.push_str(&format!("- {}\n", dep));
            }
            if snapshot.dependencies.len() > display_count {
                out.push_str(&format!(
                    "- … and {} more\n",
                    snapshot.dependencies.len() - display_count
                ));
            }
            out.push('\n');
        }

        // Directory tree (compact)
        if !snapshot.tree.is_empty() {
            out.push_str("## Directory Tree (top)\n\n");
            out.push_str("```\n");
            for node in &snapshot.tree {
                let indent = node.path.matches('/').count();
                let prefix = "  ".repeat(indent);
                let name = node.path.rsplit('/').next().unwrap_or(&node.path);
                if node.is_dir {
                    out.push_str(&format!("{}{}/\n", prefix, name));
                } else {
                    out.push_str(&format!(
                        "{}{} {}\n",
                        prefix,
                        name,
                        if node.size > 0 {
                            format!("({})", format_bytes(node.size))
                        } else {
                            String::new()
                        }
                    ));
                }
            }
            out.push_str("```\n");
        }

        out
    }

    /// Render a JSON string of the snapshot (for programmatic handoff).
    pub fn render_json(snapshot: &CodebaseSnapshot) -> Result<String> {
        Ok(serde_json::to_string_pretty(snapshot)?)
    }

    /// Render a markdown report of the snapshot.
    pub fn render_markdown(snapshot: &CodebaseSnapshot) -> String {
        let mut md = String::with_capacity(4096);

        md.push_str("# Codebase Scout Report\n\n");
        md.push_str(&format!(
            "> Scanned: {} | Files: {} | Size: {} | Duration: {}ms\n\n",
            snapshot.scanned_at,
            snapshot.total_files,
            format_bytes(snapshot.total_bytes),
            snapshot.scan_ms,
        ));

        // Overview
        md.push_str("## Overview\n\n");
        md.push_str(&format!("- **Root:** `{}`\n", snapshot.root));
        md.push_str(&format!("- **Total files:** {}\n", snapshot.total_files));
        md.push_str(&format!(
            "- **Total size:** {}\n",
            format_bytes(snapshot.total_bytes)
        ));

        if let Some(primary_lang) = snapshot.languages.first() {
            md.push_str(&format!(
                "- **Primary language:** {} ({} files)\n",
                primary_lang.language, primary_lang.file_count
            ));
        }
        md.push('\n');

        // Language breakdown
        if !snapshot.languages.is_empty() {
            md.push_str("## Language Breakdown\n\n");
            md.push_str("| Language | Files | Size | Extensions |\n");
            md.push_str("|----------|-------|------|------------|\n");
            for lang in &snapshot.languages {
                let exts = lang
                    .extensions
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                md.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    lang.language,
                    lang.file_count,
                    format_bytes(lang.total_bytes),
                    exts
                ));
            }
            md.push('\n');
        }

        // Patterns
        if !snapshot.patterns.is_empty() {
            md.push_str("## Detected Patterns\n\n");
            let mut current_category = String::new();
            for pattern in &snapshot.patterns {
                if pattern.category != current_category {
                    current_category = pattern.category.clone();
                    md.push_str(&format!("### {}s\n\n", capitalize(&current_category)));
                }
                md.push_str(&format!(
                    "- **{}** ({}% confidence) — {}\n",
                    pattern.name, pattern.confidence, pattern.evidence
                ));
            }
            md.push('\n');
        }

        // Key files
        if !snapshot.key_files.is_empty() {
            md.push_str("## Key Files\n\n");
            md.push_str("| Path | Role | Summary |\n");
            md.push_str("|------|------|--------|\n");
            for kf in &snapshot.key_files {
                let summary = kf.summary.as_deref().unwrap_or("—");
                md.push_str(&format!(
                    "| `{}` | {} | {} |\n",
                    kf.path, kf.role, summary
                ));
            }
            md.push('\n');
        }

        // Dependencies
        if !snapshot.dependencies.is_empty() {
            md.push_str(&format!(
                "## Dependencies ({})\n\n",
                snapshot.dependencies.len()
            ));
            for dep in &snapshot.dependencies {
                md.push_str(&format!("- {}\n", dep));
            }
            md.push('\n');
        }

        // Directory tree
        if !snapshot.tree.is_empty() {
            md.push_str("## Directory Structure\n\n");
            md.push_str("```\n");
            for node in &snapshot.tree {
                let depth = node.path.matches('/').count();
                let indent = "  ".repeat(depth);
                let name = node.path.rsplit('/').next().unwrap_or(&node.path);
                if node.is_dir {
                    md.push_str(&format!("{}{}/\n", indent, name));
                } else {
                    md.push_str(&format!(
                        "{}{} {}\n",
                        indent,
                        name,
                        if node.size > 0 {
                            format!("({})", format_bytes(node.size))
                        } else {
                            String::new()
                        }
                    ));
                }
            }
            md.push_str("```\n");
        }

        md
    }
}

impl Default for Scout {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_default())
    }
}

impl fmt::Debug for Scout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scout")
            .field("root", &self.config.root)
            .finish()
    }
}

// ── Internal types ───────────────────────────────────────────────────

#[derive(Debug)]
struct FileEntry {
    rel_path: String,
    name: String,
    ext: String,
    size: u64,
}

// ── Helpers ──────────────────────────────────────────────────────────

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().chain(c).collect(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_scan_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert_eq!(snapshot.total_files, 0);
        assert_eq!(snapshot.total_bytes, 0);
        assert!(snapshot.languages.is_empty());
        assert!(snapshot.patterns.is_empty());
    }

    #[test]
    fn test_scan_rust_project() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();

        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "test-project"
version = "0.1.0"

[dependencies]
serde = { version = "1", features = ["derive"] }
tokio = "1"
anyhow = "1"
"#,
        )
        .unwrap();
        fs::write(src.join("main.rs"), "fn main() { println!(\"hello\"); }").unwrap();
        fs::write(
            src.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }",
        )
        .unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot.total_files >= 3);
        assert!(snapshot.languages.iter().any(|l| l.language == "Rust"));
        assert!(snapshot
            .dependencies
            .iter()
            .any(|d| d.contains("serde")));
        assert!(snapshot.patterns.iter().any(|p| p.name == "Rust"));
        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "Async Rust (Tokio)"));
        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "Standard src/ layout"));
    }

    #[test]
    fn test_scan_ts_project() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();

        fs::write(
            tmp.path().join("package.json"),
            r#"{"dependencies": {"react": "^18.0.0", "next": "^14.0.0"}}"#,
        )
        .unwrap();
        fs::write(
            src.join("index.tsx"),
            "export default function App() { return <div/> }",
        )
        .unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot
            .languages
            .iter()
            .any(|l| l.language == "TypeScript"));
        assert!(snapshot.patterns.iter().any(|p| p.name == "React"));
        assert!(snapshot.patterns.iter().any(|p| p.name == "Next.js"));
    }

    #[test]
    fn test_scan_python_project() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"[tool.poetry]
name = "test-project"

[tool.poetry.dependencies]
python = "^3.11"
flask = "^3.0"
requests = "^2.31"
"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("main.py"),
            "from flask import Flask\napp = Flask(__name__)\n",
        )
        .unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot.languages.iter().any(|l| l.language == "Python"));
        assert!(snapshot.patterns.iter().any(|p| p.name == "Flask"));
        assert!(snapshot
            .dependencies
            .iter()
            .any(|d| d.contains("flask")));
    }

    #[test]
    fn test_scan_go_project() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("go.mod"),
            "module example.com/test\n\ngo 1.22\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.9.1\n)\n",
        )
        .unwrap();
        fs::write(tmp.path().join("main.go"), "package main\n\nfunc main() {}\n").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot.languages.iter().any(|l| l.language == "Go"));
        assert!(snapshot.dependencies.iter().any(|d| d.contains("gin")));
    }

    #[test]
    fn test_scan_ignores_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git/objects")).unwrap();
        fs::create_dir_all(tmp.path().join("target/debug")).unwrap();
        fs::create_dir_all(tmp.path().join("node_modules/react")).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        for node in &snapshot.tree {
            assert!(
                !node.path.starts_with(".git/"),
                "Should skip .git: {}",
                node.path
            );
            assert!(
                !node.path.starts_with("target/"),
                "Should skip target: {}",
                node.path
            );
            assert!(
                !node.path.starts_with("node_modules/"),
                "Should skip node_modules: {}",
                node.path
            );
        }
    }

    #[test]
    fn test_scan_respects_depth_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a/b/c/d/e/f");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep.txt"), "content").unwrap();
        fs::write(tmp.path().join("shallow.txt"), "content").unwrap();

        let config = ScoutConfig {
            root: tmp.path().to_path_buf(),
            max_depth: 3,
            ..Default::default()
        };
        let scout = Scout::with_config(config);
        let snapshot = scout.scan().unwrap();

        assert!(snapshot.tree.iter().any(|n| n.path == "shallow.txt"));
        assert!(!snapshot.tree.iter().any(|n| n.path.contains("deep.txt")));
    }

    #[test]
    fn test_scan_nonexistent_dir() {
        let scout = Scout::new("/nonexistent/path/that/does/not/exist");
        assert!(scout.scan().is_err());
    }

    #[test]
    fn test_scan_file_as_root() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("not_a_dir.txt");
        fs::write(&file_path, "content").unwrap();

        let scout = Scout::new(&file_path);
        assert!(scout.scan().is_err());
    }

    #[test]
    fn test_render_compact_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();
        let compact = Scout::render_compact(&snapshot);

        assert!(compact.contains("Codebase Snapshot"));
        assert!(compact.contains("Rust"));
        assert!(compact.contains("Cargo.toml"));
    }

    #[test]
    fn test_render_markdown_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();
        let md = Scout::render_markdown(&snapshot);

        assert!(md.contains("# Codebase Scout Report"));
        assert!(md.contains("## Language Breakdown"));
        assert!(md.contains("Rust"));
    }

    #[test]
    fn test_render_json_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();
        let json = Scout::render_json(&snapshot).unwrap();

        let parsed: CodebaseSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.root, snapshot.root);
        assert_eq!(parsed.total_files, snapshot.total_files);
    }

    #[test]
    fn test_key_file_summary_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"my-cool-project\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        let cargo = snapshot
            .key_files
            .iter()
            .find(|kf| kf.path == "Cargo.toml")
            .unwrap();
        assert_eq!(cargo.role, "config");
        assert!(cargo.summary.is_some());
    }

    #[test]
    fn test_workspace_detection() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "Rust workspace (monorepo)"));
    }

    #[test]
    fn test_lib_binary_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(src.join("lib.rs"), "pub fn foo() {}").unwrap();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "Lib+Binary Rust crate"));
    }

    #[test]
    fn test_ci_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let workflows = tmp.path().join(".github/workflows");
        fs::create_dir_all(&workflows).unwrap();
        fs::write(workflows.join("ci.yml"), "name: CI\non: push\n").unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "GitHub Actions CI"));
        assert!(snapshot
            .key_files
            .iter()
            .any(|kf| kf.role == "ci" && kf.path.contains("workflows")));
    }

    #[test]
    fn test_docker_detection() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("Dockerfile"), "FROM rust:1.75\n").unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot.patterns.iter().any(|p| p.name == "Dockerized"));
    }

    #[test]
    fn test_mvc_detection() {
        let tmp = tempfile::tempdir().unwrap();
        for dir in &["controllers", "models", "views", "src"] {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        fs::write(
            tmp.path().join("package.json"),
            r#"{"dependencies": {"express": "^4.0.0"}}"#,
        )
        .unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "MVC architecture"));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
        assert_eq!(format_bytes(1073741824), "1.0 GB");
    }

    #[test]
    fn test_capitalize() {
        assert_eq!(capitalize("language"), "Language");
        assert_eq!(capitalize("framework"), "Framework");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn test_config_default() {
        let config = ScoutConfig::default();
        assert_eq!(config.max_depth, 6);
        assert!(config.ignore.contains(&".git".to_string()));
        assert!(config.ignore.contains(&"node_modules".to_string()));
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = ScoutConfig {
            root: PathBuf::from("/tmp/project"),
            max_depth: 4,
            max_sample_bytes: 256 * 1024,
            max_tree_files: 100,
            ignore: vec![".git".into()],
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: ScoutConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.root, config.root);
        assert_eq!(parsed.max_depth, 4);
        assert_eq!(parsed.max_tree_files, 100);
    }

    #[test]
    fn test_snapshot_serde_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        let json = serde_json::to_string_pretty(&snapshot).unwrap();
        let parsed: CodebaseSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.root, snapshot.root);
        assert_eq!(parsed.total_files, snapshot.total_files);
        assert_eq!(parsed.languages.len(), snapshot.languages.len());
        assert_eq!(parsed.patterns.len(), snapshot.patterns.len());
    }

    #[test]
    fn test_scan_cargo_workspace_with_members() {
        let tmp = tempfile::tempdir().unwrap();
        let crates_dir = tmp.path().join("crates");
        let crate_a = crates_dir.join("crate-a");
        let crate_b = crates_dir.join("crate-b");
        fs::create_dir_all(crate_a.join("src")).unwrap();
        fs::create_dir_all(crate_b.join("src")).unwrap();

        fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        fs::write(
            crate_a.join("Cargo.toml"),
            "[package]\nname = \"crate-a\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        fs::write(crate_a.join("src/lib.rs"), "pub fn a() {}").unwrap();
        fs::write(
            crate_b.join("Cargo.toml"),
            "[package]\nname = \"crate-b\"\nversion = \"0.1.0\"\n\n[dependencies]\ntokio = \"1\"\n",
        )
        .unwrap();
        fs::write(crate_b.join("src/lib.rs"), "pub fn b() {}").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot.total_files >= 5);
        assert!(snapshot
            .dependencies
            .iter()
            .any(|d| d.contains("serde")));
        assert!(snapshot
            .dependencies
            .iter()
            .any(|d| d.contains("tokio")));
        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "Rust workspace (monorepo)"));
        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "Async Rust (Tokio)"));
    }

    #[test]
    fn test_agent_conventions_detection() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("AGENTS.md"),
            "# Agent Conventions\nUse Rust.\n",
        )
        .unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        assert!(snapshot
            .patterns
            .iter()
            .any(|p| p.name == "AI agent conventions"));
    }

    #[test]
    fn test_multi_language_project() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let frontend = tmp.path().join("frontend/src");
        let scripts = tmp.path().join("scripts");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&frontend).unwrap();
        fs::create_dir_all(&scripts).unwrap();

        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        fs::write(
            frontend.join("App.tsx"),
            "export default function App() {}",
        )
        .unwrap();
        fs::write(
            scripts.join("build.py"),
            "#!/usr/bin/env python3\nprint('hi')\n",
        )
        .unwrap();

        let scout = Scout::new(tmp.path());
        let snapshot = scout.scan().unwrap();

        let lang_names: Vec<&str> = snapshot
            .languages
            .iter()
            .map(|l| l.language.as_str())
            .collect();
        assert!(lang_names.contains(&"Rust"));
        assert!(lang_names.contains(&"TypeScript"));
        assert!(lang_names.contains(&"Python"));
    }
}
