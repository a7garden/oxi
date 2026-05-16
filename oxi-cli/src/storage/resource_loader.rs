//! Enhanced Resource loader for oxi
//!
//! Loads and manages skills, extensions, themes, and prompts from various locations.
//! Also handles discovery and loading of project context files (AGENTS.md, CLAUDE.md).
//!
//! Features:
//! - Resource discovery from multiple directories (user, project, CLI)
//! - Resource type detection and validation
//! - Caching of loaded resources with invalidation
//! - Deduplication with collision diagnostics
//! - System prompt discovery (SYSTEM.md, APPEND_SYSTEM.md)
//! - Extension conflict detection
//! - Hot-reload on file changes

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use parking_lot::RwLock;

// Re-export types from resource_loader_compat
pub use super::resource_loader_compat::{Prompt, Skill, Theme};

// ============================================================================
// Context Files
// ============================================================================

/// A project context file (AGENTS.md, CLAUDE.md, etc.)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContextFile {
    /// Full path to the file
    pub path: PathBuf,
    /// Filename (e.g., "AGENTS.md", "CLAUDE.md")
    pub name: String,
    /// Priority for inclusion (higher = more important)
    pub priority: u8,
    /// File content
    pub content: String,
}

impl ContextFile {
    /// Create a new context file
    pub fn new(path: PathBuf, name: impl Into<String>, priority: u8, content: String) -> Self {
        Self {
            path,
            name: name.into(),
            priority,
            content,
        }
    }

    /// Get the file extension
    pub fn extension(&self) -> Option<String> {
        self.path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
    }
}

/// Context file candidates in priority order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFileType {
    /// AGENTS.md - highest priority, explicit agent instructions
    Agents,
    /// CLAUDE.md - Claude-specific instructions
    Claude,
}

impl ContextFileType {
    /// Get the filename for this context file type
    pub fn filename(&self) -> &'static str {
        match self {
            ContextFileType::Agents => "AGENTS.md",
            ContextFileType::Claude => "CLAUDE.md",
        }
    }

    /// Get the priority (higher = more important)
    pub fn priority(&self) -> u8 {
        match self {
            ContextFileType::Agents => 100,
            ContextFileType::Claude => 90,
        }
    }

    /// Get all supported variants (case-insensitive)
    pub fn variants(&self) -> Vec<&'static str> {
        match self {
            ContextFileType::Agents => vec!["AGENTS.md", "AGENTS.MD"],
            ContextFileType::Claude => vec!["CLAUDE.md", "CLAUDE.MD"],
        }
    }

    /// Detect file type from filename
    pub fn from_filename(name: &str) -> Option<Self> {
        let upper = name.to_uppercase();
        match upper.as_str() {
            "AGENTS.md" | "AGENTS.MD" => Some(ContextFileType::Agents),
            "CLAUDE.md" | "CLAUDE.MD" => Some(ContextFileType::Claude),
            _ => None,
        }
    }
}

// ============================================================================
// Source Types
// ============================================================================

/// Source of a loaded resource
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
pub enum SourceType {
    /// Default system location (~/.oxi or ~/.config/oxi)
    Default,
    /// Project-level location (.oxi in project root)
    Project,
    /// CLI-specified location
    Cli,
    /// Inline/factory-created resource
    Inline,
    /// Npm package resource
    Package,
    /// Git repository resource
    Git,
}

impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceType::Default => write!(f, "default"),
            SourceType::Project => write!(f, "project"),
            SourceType::Cli => write!(f, "cli"),
            SourceType::Inline => write!(f, "inline"),
            SourceType::Package => write!(f, "package"),
            SourceType::Git => write!(f, "git"),
        }
    }
}

/// A source directory for resources
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Source {
    /// Path to the source directory
    pub path: PathBuf,
    /// Source type
    pub source_type: SourceType,
    /// Whether this source is enabled
    pub enabled: bool,
}

impl Source {
    /// Create a new source
    #[allow(dead_code)]
    pub fn new(path: PathBuf, source_type: SourceType) -> Self {
        Self {
            path,
            source_type,
            enabled: true,
        }
    }

    /// Check if path exists
    #[allow(dead_code)]
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Check if path is a directory
    #[allow(dead_code)]
    pub fn is_dir(&self) -> bool {
        self.path.is_dir()
    }
}

// ============================================================================
// Source Info (for tracking where resources came from)
// ============================================================================

/// Information about where a resource was loaded from
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
pub struct SourceInfo {
    /// Path to the resource
    pub path: PathBuf,
    /// Source type (local, cli, package, etc.)
    pub source: String,
    /// Scope (user, project, temporary)
    pub scope: String,
    /// Origin (top-level, package, etc.)
    pub origin: String,
    /// Base directory (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<PathBuf>,
}

// ============================================================================
// Extension Sources
// ============================================================================

/// Source for extension loading
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ExtensionSource {
    /// Path to the extension
    #[allow(dead_code)]
    pub path: PathBuf,
    /// Metadata about the extension source
    #[allow(dead_code)]
    pub metadata: PathMetadata,
    /// Source info for tracking
    #[allow(dead_code)]
    pub source_info: Option<SourceInfo>,
}

/// Metadata about a resource path
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PathMetadata {
    /// Source (default, cli, package, etc.)
    pub source: String,
    /// Scope (user, project, temporary, etc.)
    pub scope: String,
    /// Origin (top-level, package, etc.)
    pub origin: String,
}

impl Default for PathMetadata {
    fn default() -> Self {
        Self {
            source: "local".to_string(),
            scope: "user".to_string(),
            origin: "top-level".to_string(),
        }
    }
}

impl PathMetadata {
    /// Create metadata for a CLI source
    pub fn cli() -> Self {
        Self {
            source: "cli".to_string(),
            scope: "temporary".to_string(),
            origin: "top-level".to_string(),
        }
    }

    /// Create metadata for a project source
    pub fn project() -> Self {
        Self {
            source: "local".to_string(),
            scope: "project".to_string(),
            origin: "top-level".to_string(),
        }
    }

    /// Create metadata for a default/user source
    pub fn user() -> Self {
        Self {
            source: "local".to_string(),
            scope: "user".to_string(),
            origin: "top-level".to_string(),
        }
    }
}

/// Skill source configuration
#[derive(Debug, Clone)]
pub struct SkillSource {
    /// Path to the skill
    pub path: PathBuf,
    /// Metadata
    #[allow(dead_code)]
    pub metadata: PathMetadata,
    /// Whether enabled
    pub enabled: bool,
}

/// Theme source configuration
#[derive(Debug, Clone)]
pub struct ThemeSource {
    /// Path to the theme
    pub path: PathBuf,
    /// Metadata
    #[allow(dead_code)]
    pub metadata: PathMetadata,
    /// Whether enabled
    pub enabled: bool,
}

/// Prompt source configuration
#[derive(Debug, Clone)]
pub struct PromptSource {
    /// Path to the prompt
    pub path: PathBuf,
    /// Metadata
    #[allow(dead_code)]
    pub metadata: PathMetadata,
    /// Whether enabled
    pub enabled: bool,
}

// ============================================================================
// Resource Collision
// ============================================================================

/// A resource collision (two resources with the same name)
#[derive(Debug, Clone)]
pub struct ResourceCollision {
    /// Resource type
    pub resource_type: String,
    /// Name that collided
    pub name: String,
    /// Path of the winner
    pub winner_path: PathBuf,
    /// Path of the loser
    pub loser_path: PathBuf,
}

impl std::fmt::Display for ResourceCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} '{}' collision: {} vs {}",
            self.resource_type,
            self.name,
            self.winner_path.display(),
            self.loser_path.display()
        )
    }
}

// ============================================================================
// Loaded Resources
// ============================================================================

/// All loaded resources
#[derive(Debug, Clone)]
pub struct LoadedResources {
    /// All loaded skills
    pub skills: Vec<Skill>,
    /// All loaded themes
    pub themes: Vec<Theme>,
    /// All loaded prompts
    pub prompts: Vec<Prompt>,
    /// All loaded context files
    pub context_files: Vec<ContextFile>,
    /// System prompt content
    pub system_prompt: Option<String>,
    /// Append system prompt content
    pub append_system_prompt: Vec<String>,
    /// Errors encountered during loading
    pub errors: Vec<LoadError>,
    /// Diagnostics from loading
    pub diagnostics: Vec<ResourceDiagnostic>,
    /// Resource collisions
    pub collisions: Vec<ResourceCollision>,
    /// Timestamp when resources were loaded
    pub loaded_at: Instant,
}

impl Default for LoadedResources {
    fn default() -> Self {
        Self {
            skills: Vec::new(),
            themes: Vec::new(),
            prompts: Vec::new(),
            context_files: Vec::new(),
            system_prompt: None,
            append_system_prompt: Vec::new(),
            errors: Vec::new(),
            diagnostics: Vec::new(),
            collisions: Vec::new(),
            loaded_at: Instant::now(),
        }
    }
}

// ============================================================================
// Resource Loader Options
// ============================================================================

/// Options for configuring the resource loader
#[derive(Debug, Clone)]
pub struct ResourceLoaderOptions {
    /// Current working directory
    pub cwd: PathBuf,
    /// Base agent directory (e.g., ~/.config/oxi)
    pub agent_dir: PathBuf,
    /// Additional extension paths (from CLI)
    pub additional_extension_paths: Vec<PathBuf>,
    /// Additional skill paths (from CLI)
    pub additional_skill_paths: Vec<PathBuf>,
    /// Additional prompt template paths (from CLI)
    pub additional_prompt_paths: Vec<PathBuf>,
    /// Additional theme paths (from CLI)
    pub additional_theme_paths: Vec<PathBuf>,
    /// Disable extension loading
    pub no_extensions: bool,
    /// Disable skill loading
    pub no_skills: bool,
    /// Disable prompt loading
    pub no_prompts: bool,
    /// Disable theme loading
    pub no_themes: bool,
    /// Disable context file loading
    pub no_context_files: bool,
    /// Explicit system prompt (from CLI)
    pub system_prompt: Option<String>,
    /// Explicit append system prompts
    pub append_system_prompt: Vec<String>,
}

impl ResourceLoaderOptions {
    /// Create options with default directories
    pub fn new() -> Self {
        let agent_dir = default_resource_dir();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            cwd,
            agent_dir,
            additional_extension_paths: Vec::new(),
            additional_skill_paths: Vec::new(),
            additional_prompt_paths: Vec::new(),
            additional_theme_paths: Vec::new(),
            no_extensions: false,
            no_skills: false,
            no_prompts: false,
            no_themes: false,
            no_context_files: false,
            system_prompt: None,
            append_system_prompt: Vec::new(),
        }
    }
}

impl Default for ResourceLoaderOptions {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Resource Loader
// ============================================================================

/// Resource loader for all oxi resources
///
/// Loads skills, prompts, themes, context files, and system prompts from
/// multiple directories (user-level, project-level, CLI-specified).
/// Supports caching, deduplication, and hot-reload.
pub struct ResourceLoader {
    /// Options
    options: ResourceLoaderOptions,
    /// Extension sources
    extensions: Vec<ExtensionSource>,
    /// Skill sources
    skills: Vec<SkillSource>,
    /// Theme sources
    themes: Vec<ThemeSource>,
    /// Prompt sources
    prompts: Vec<PromptSource>,
    /// Loaded resources cache
    cache: RwLock<Option<LoadedResources>>,
    /// Last file modification times for hot-reload
    modification_times: RwLock<HashMap<PathBuf, std::time::SystemTime>>,
}

impl Default for ResourceLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLoader {
    /// Create a new resource loader with defaults
    pub fn new() -> Self {
        Self::with_options(ResourceLoaderOptions::new())
    }

    /// Create a resource loader with options
    pub fn with_options(options: ResourceLoaderOptions) -> Self {
        Self {
            options,
            extensions: Vec::new(),
            skills: Vec::new(),
            themes: Vec::new(),
            prompts: Vec::new(),
            cache: RwLock::new(None),
            modification_times: RwLock::new(HashMap::new()),
        }
    }

    /// Create with custom base and working directory
    pub fn with_paths(base_dir: PathBuf, cwd: PathBuf) -> Self {
        let options = ResourceLoaderOptions {
            cwd,
            agent_dir: base_dir,
            ..ResourceLoaderOptions::default()
        };
        Self::with_options(options)
    }

    // -----------------------------------------------------------------------
    // Builder methods
    // -----------------------------------------------------------------------

    /// Set base directory
    pub fn with_base_dir(&mut self, base_dir: PathBuf) -> &mut Self {
        self.options.agent_dir = base_dir;
        self
    }

    /// Set current working directory
    pub fn with_cwd(&mut self, cwd: PathBuf) -> &mut Self {
        self.options.cwd = cwd;
        self
    }

    /// Add an extension source
    pub fn add_extension(&mut self, path: PathBuf) -> &mut Self {
        self.extensions.push(ExtensionSource {
            path: resolve_path(&path),
            metadata: PathMetadata::cli(),
            source_info: None,
        });
        self
    }

    /// Add a skill source
    pub fn add_skill(&mut self, path: PathBuf) -> &mut Self {
        self.skills.push(SkillSource {
            path: resolve_path(&path),
            metadata: PathMetadata::cli(),
            enabled: true,
        });
        self
    }

    /// Add a theme source
    pub fn add_theme(&mut self, path: PathBuf) -> &mut Self {
        self.themes.push(ThemeSource {
            path: resolve_path(&path),
            metadata: PathMetadata::cli(),
            enabled: true,
        });
        self
    }

    /// Add a prompt source
    pub fn add_prompt(&mut self, path: PathBuf) -> &mut Self {
        self.prompts.push(PromptSource {
            path: resolve_path(&path),
            metadata: PathMetadata::cli(),
            enabled: true,
        });
        self
    }

    /// Extend resources from extension paths
    pub fn extend_resources(
        &mut self,
        skill_paths: Vec<(PathBuf, PathMetadata)>,
        prompt_paths: Vec<(PathBuf, PathMetadata)>,
        theme_paths: Vec<(PathBuf, PathMetadata)>,
    ) {
        for (path, meta) in skill_paths {
            self.skills.push(SkillSource {
                path,
                metadata: meta,
                enabled: true,
            });
        }
        for (path, meta) in prompt_paths {
            self.prompts.push(PromptSource {
                path,
                metadata: meta,
                enabled: true,
            });
        }
        for (path, meta) in theme_paths {
            self.themes.push(ThemeSource {
                path,
                metadata: meta,
                enabled: true,
            });
        }
    }

    // -----------------------------------------------------------------------
    // Loading
    // -----------------------------------------------------------------------

    /// Load all resources
    pub fn load_all(&self) -> Result<LoadedResources, anyhow::Error> {
        let mut result = LoadedResources::default();

        // Load skills
        let skills = self.load_skills_internal();
        result.skills = skills.items;
        result.errors.extend(skills.errors);
        result.diagnostics.extend(skills.diagnostics);

        // Load themes
        let themes = self.load_themes_internal();
        result.themes = themes.items;
        result.errors.extend(themes.errors);
        result.diagnostics.extend(themes.diagnostics);

        // Load prompts
        let prompts = self.load_prompts_internal();
        result.prompts = prompts.items;
        result.errors.extend(prompts.errors);
        result.diagnostics.extend(prompts.diagnostics);

        // Deduplicate and detect collisions
        let (deduped_skills, skill_collisions) = dedupe_skills(result.skills);
        result.skills = deduped_skills;
        result.collisions.extend(skill_collisions);

        let (deduped_themes, theme_collisions) = dedupe_themes(result.themes);
        result.themes = deduped_themes;
        result.collisions.extend(theme_collisions);

        let (deduped_prompts, prompt_collisions) = dedupe_prompts(result.prompts);
        result.prompts = deduped_prompts;
        result.collisions.extend(prompt_collisions);

        // Load context files
        if !self.options.no_context_files {
            result.context_files = self.load_project_context_files(&self.options.cwd)?;
        }

        // Load system prompts
        result.system_prompt = self.load_system_prompt()?;
        result.append_system_prompt = self.load_append_system_prompt()?;

        // Update modification times for hot-reload
        self.update_modification_times(&result);

        // Update cache
        *self.cache.write() = Some(result.clone());

        Ok(result)
    }

    /// Load all resources, returning default on error
    pub fn try_load_all(&self) -> LoadedResources {
        self.load_all().unwrap_or_else(|e| LoadedResources {
            errors: vec![LoadError {
                path: PathBuf::from("."),
                error: e.to_string(),
            }],
            ..LoadedResources::default()
        })
    }

    /// Reload resources (clears cache first)
    pub fn reload(&self) -> Result<LoadedResources, anyhow::Error> {
        self.clear_cache();
        self.load_all()
    }

    // -----------------------------------------------------------------------
    // System Prompts
    // -----------------------------------------------------------------------

    /// Load system prompt from SYSTEM.md files
    pub fn load_system_prompt(&self) -> Result<Option<String>, anyhow::Error> {
        // If explicit system prompt was provided, use it
        if let Some(ref prompt) = self.options.system_prompt {
            return Ok(resolve_prompt_input(prompt, "system prompt"));
        }

        // Discover SYSTEM.md files
        let candidates = vec![
            // Project-level
            self.options.cwd.join(".oxi").join("SYSTEM.md"),
            // Global
            self.options.agent_dir.join("SYSTEM.md"),
        ];

        for path in candidates {
            if path.exists() && path.is_file() {
                match fs::read_to_string(&path) {
                    Ok(content) => return Ok(Some(content)),
                    Err(e) => {
                        tracing::warn!("Failed to read system prompt {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(None)
    }

    /// Load append system prompt from APPEND_SYSTEM.md files
    pub fn load_append_system_prompt(&self) -> Result<Vec<String>, anyhow::Error> {
        // If explicit append prompts were provided, use them
        if !self.options.append_system_prompt.is_empty() {
            return Ok(self
                .options
                .append_system_prompt
                .iter()
                .filter_map(|s| resolve_prompt_input(s, "append system prompt"))
                .collect());
        }

        let mut result = Vec::new();

        let candidates = vec![
            // Project-level
            self.options.cwd.join(".oxi").join("APPEND_SYSTEM.md"),
            // Global
            self.options.agent_dir.join("APPEND_SYSTEM.md"),
        ];

        for path in candidates {
            if path.exists() && path.is_file() {
                match fs::read_to_string(&path) {
                    Ok(content) => result.push(content),
                    Err(e) => {
                        tracing::warn!(
                            "Failed to read append system prompt {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }

        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Context Files
    // -----------------------------------------------------------------------

    /// Load project context files (AGENTS.md, CLAUDE.md, etc.)
    pub fn load_project_context_files(
        &self,
        cwd: &Path,
    ) -> Result<Vec<ContextFile>, anyhow::Error> {
        let mut context_files = Vec::new();
        let mut seen_paths: HashMap<String, bool> = HashMap::new();

        // 1. Check global agent dir for context files
        let global_context = load_context_file_from_dir(&self.options.agent_dir);
        if let Some((path, content)) = global_context {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let file_type = ContextFileType::from_filename(&name);
            let priority = file_type.map(|ft| ft.priority()).unwrap_or(80);
            let path_str = path.to_string_lossy().to_string();
            seen_paths.insert(path_str, true);
            context_files.push(ContextFile::new(path, name, priority, content));
        }

        // 2. Discover context files in project + ancestors
        let discovered = self.discover_context_files(cwd);

        for (path, file_type) in discovered {
            let path_str = path.to_string_lossy().to_string();
            if seen_paths.contains_key(&path_str) {
                continue;
            }

            if let Some(content) = self.read_context_file(&path)? {
                seen_paths.insert(path_str, true);
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                context_files.push(ContextFile::new(path, name, file_type.priority(), content));
            }
        }

        // Sort by priority (descending)
        context_files.sort_by(|a, b| b.priority.cmp(&a.priority));

        Ok(context_files)
    }

    /// Discover context files in project and ancestor directories
    pub fn discover_context_files(&self, dir: &Path) -> Vec<(PathBuf, ContextFileType)> {
        let mut discovered = Vec::new();
        let file_types = [ContextFileType::Agents, ContextFileType::Claude];

        // Try to find git root to limit search
        let git_root = find_git_root(dir);

        let mut current = dir.to_path_buf();
        let root = PathBuf::from("/");

        let max_iterations = 50;
        let mut iterations = 0;

        while current != root && iterations < max_iterations {
            // Check if we've reached or passed the git root
            if let Some(ref git_r) = git_root {
                if current == *git_r || !current.starts_with(git_r) {
                    break;
                }
            }

            for file_type in &file_types {
                for variant in file_type.variants() {
                    let candidate = current.join(variant);
                    if candidate.exists() && candidate.is_file() {
                        discovered.push((candidate, *file_type));
                    }
                }
            }

            // Move to parent
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            } else {
                break;
            }
            iterations += 1;
        }

        // Deduplicate by path
        let mut seen = HashSet::new();
        discovered.retain(|(path, _)| {
            let path_str = path.to_string_lossy().to_string();
            if seen.contains(&path_str) {
                false
            } else {
                seen.insert(path_str);
                true
            }
        });

        discovered
    }

    /// Read a context file, handling potential errors
    fn read_context_file(&self, path: &Path) -> Result<Option<String>, anyhow::Error> {
        match fs::read_to_string(path) {
            Ok(content) => Ok(Some(content)),
            Err(e) => {
                tracing::warn!("Failed to read context file {}: {}", path.display(), e);
                Ok(None)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Resource Loading (internal)
    // -----------------------------------------------------------------------

    /// Load skills from configured sources
    fn load_skills_internal(&self) -> LoadResult<Skill> {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        let mut diagnostics = Vec::new();

        // Load from default directories
        if !self.options.no_skills {
            let skills_base = skills_dir(&self.options.agent_dir);
            let project_skills = self.options.cwd.join(".oxi").join("skills");

            for dir in &[skills_base, project_skills] {
                if dir.exists() {
                    let result = load_skills_from_dir(dir);
                    items.extend(result.items);
                    errors.extend(result.errors);
                    diagnostics.extend(result.diagnostics);
                }
            }
        }

        // Load from custom sources
        for source in &self.skills {
            if !source.enabled {
                continue;
            }
            if source.path.exists() {
                match load_skill(&source.path) {
                    Ok(skill) => items.push(skill),
                    Err(e) => {
                        errors.push(LoadError {
                            path: source.path.clone(),
                            error: e,
                        });
                    }
                }
            }
        }

        LoadResult {
            items,
            errors,
            diagnostics,
        }
    }

    /// Load themes from configured sources
    fn load_themes_internal(&self) -> LoadResult<Theme> {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        let mut diagnostics = Vec::new();

        if !self.options.no_themes {
            let themes_base = themes_dir(&self.options.agent_dir);
            let project_themes = self.options.cwd.join(".oxi").join("themes");

            for dir in &[themes_base, project_themes] {
                if dir.exists() {
                    let result = load_themes_from_dir(dir);
                    items.extend(result.items);
                    errors.extend(result.errors);
                    diagnostics.extend(result.diagnostics);
                }
            }
        }

        for source in &self.themes {
            if !source.enabled {
                continue;
            }
            if source.path.exists() {
                match load_theme(&source.path) {
                    Ok(theme) => items.push(theme),
                    Err(e) => {
                        errors.push(LoadError {
                            path: source.path.clone(),
                            error: e,
                        });
                    }
                }
            }
        }

        LoadResult {
            items,
            errors,
            diagnostics,
        }
    }

    /// Load prompts from configured sources
    fn load_prompts_internal(&self) -> LoadResult<Prompt> {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        let mut diagnostics = Vec::new();

        if !self.options.no_prompts {
            let prompts_base = prompts_dir(&self.options.agent_dir);
            let project_prompts = self.options.cwd.join(".oxi").join("prompts");

            for dir in &[prompts_base, project_prompts] {
                if dir.exists() {
                    let result = load_prompts_from_dir(dir);
                    items.extend(result.items);
                    errors.extend(result.errors);
                    diagnostics.extend(result.diagnostics);
                }
            }
        }

        for source in &self.prompts {
            if !source.enabled {
                continue;
            }
            if source.path.exists() {
                match load_prompt(&source.path) {
                    Ok(prompt) => items.push(prompt),
                    Err(e) => {
                        errors.push(LoadError {
                            path: source.path.clone(),
                            error: e,
                        });
                    }
                }
            }
        }

        LoadResult {
            items,
            errors,
            diagnostics,
        }
    }

    // -----------------------------------------------------------------------
    // Cache & Hot-Reload
    // -----------------------------------------------------------------------

    /// Get cached resources if available
    pub fn cached(&self) -> Option<LoadedResources> {
        self.cache.read().clone()
    }

    /// Clear the cache
    pub fn clear_cache(&self) {
        *self.cache.write() = None;
    }

    /// Check if cache is stale (any source file was modified since last load)
    pub fn is_cache_stale(&self) -> bool {
        let cache = self.cache.read();
        if cache.is_none() {
            return true; // No cache means stale
        }

        let mtimes = self.modification_times.read();
        if mtimes.is_empty() {
            return false; // No files to track
        }

        for (path, last_time) in mtimes.iter() {
            if let Ok(metadata) = fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    if modified > *last_time {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Load if cache is stale, otherwise return cached
    pub fn load_if_stale(&self) -> Result<LoadedResources, anyhow::Error> {
        if self.is_cache_stale() {
            self.reload()
        } else if let Some(cached) = self.cached() {
            Ok(cached)
        } else {
            self.load_all()
        }
    }

    /// Update modification times for all tracked files
    fn update_modification_times(&self, result: &LoadedResources) {
        let mut mtimes = self.modification_times.write();
        mtimes.clear();

        let paths: Vec<PathBuf> = {
            let mut p = Vec::new();
            for s in &result.skills {
                p.push(s.path.clone());
            }
            for t in &result.themes {
                p.push(t.path.clone());
            }
            for pr in &result.prompts {
                p.push(pr.path.clone());
            }
            for cf in &result.context_files {
                p.push(cf.path.clone());
            }
            p
        };

        for path in paths {
            if let Ok(metadata) = fs::metadata(&path) {
                if let Ok(modified) = metadata.modified() {
                    mtimes.insert(path, modified);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Resource Type Detection
    // -----------------------------------------------------------------------

    /// Detect the resource type from a path
    pub fn detect_resource_type(path: &Path) -> Option<ResourceType> {
        if !path.exists() {
            return None;
        }

        if path.is_dir() {
            // Directories can be skills (with SKILL.md) or extensions
            if path.join("SKILL.md").exists() {
                return Some(ResourceType::Skill);
            }
            if path.join("package.json").exists() || path.join("extension.json").exists() {
                return Some(ResourceType::Extension);
            }
            // Default for directories: skill
            return Some(ResourceType::Skill);
        }

        // File-based detection
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "md" => Some(ResourceType::Skill),
            "json" => Some(ResourceType::Theme),
            "js" | "ts" => Some(ResourceType::Extension),
            _ => None,
        }
    }

    /// Check if a path exists and is a valid resource
    pub fn is_valid_resource_path(path: &Path, resource_type: ResourceType) -> bool {
        if !path.exists() {
            return false;
        }
        match resource_type {
            ResourceType::Skill => {
                path.is_dir() || path.extension().map(|e| e == "md").unwrap_or(false)
            }
            ResourceType::Theme => path.extension().map(|e| e == "json").unwrap_or(false),
            ResourceType::Prompt => path.extension().map(|e| e == "md").unwrap_or(false),
            ResourceType::Extension => path
                .extension()
                .map(|e| e == "js" || e == "ts")
                .unwrap_or(false),
        }
    }

    /// Validate that a resource path can be loaded
    pub fn validate_resource_path(path: &Path) -> Result<ResourceType, String> {
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()));
        }

        Self::detect_resource_type(path)
            .ok_or_else(|| format!("Cannot determine resource type for: {}", path.display()))
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Get the current working directory
    pub fn cwd(&self) -> &Path {
        &self.options.cwd
    }

    /// Get the agent directory
    pub fn agent_dir(&self) -> &Path {
        &self.options.agent_dir
    }

    /// Get loaded skills
    pub fn get_skills(&self) -> Vec<Skill> {
        self.cache
            .read()
            .as_ref()
            .map(|c| c.skills.clone())
            .unwrap_or_default()
    }

    /// Get loaded themes
    pub fn get_themes(&self) -> Vec<Theme> {
        self.cache
            .read()
            .as_ref()
            .map(|c| c.themes.clone())
            .unwrap_or_default()
    }

    /// Get loaded prompts
    pub fn get_prompts(&self) -> Vec<Prompt> {
        self.cache
            .read()
            .as_ref()
            .map(|c| c.prompts.clone())
            .unwrap_or_default()
    }

    /// Get loaded context files
    pub fn get_context_files(&self) -> Vec<ContextFile> {
        self.cache
            .read()
            .as_ref()
            .map(|c| c.context_files.clone())
            .unwrap_or_default()
    }

    /// Get system prompt
    pub fn get_system_prompt(&self) -> Option<String> {
        self.cache
            .read()
            .as_ref()
            .and_then(|c| c.system_prompt.clone())
    }

    /// Get append system prompt
    pub fn get_append_system_prompt(&self) -> Vec<String> {
        self.cache
            .read()
            .as_ref()
            .map(|c| c.append_system_prompt.clone())
            .unwrap_or_default()
    }

    /// Get agents files (alias for context files in agent format)
    pub fn get_agents_files(&self) -> Vec<(PathBuf, String)> {
        self.cache
            .read()
            .as_ref()
            .map(|c| {
                c.context_files
                    .iter()
                    .map(|cf| (cf.path.clone(), cf.content.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ============================================================================
// Standalone Functions
// ============================================================================

/// Load a context file from a directory (AGENTS.md, CLAUDE.md, etc.)
pub fn load_context_file_from_dir(dir: &Path) -> Option<(PathBuf, String)> {
    let candidates = ["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];
    for filename in &candidates {
        let file_path = dir.join(filename);
        if file_path.exists() {
            match fs::read_to_string(&file_path) {
                Ok(content) => return Some((file_path, content)),
                Err(e) => {
                    tracing::warn!("Warning: Could not read {}: {}", file_path.display(), e);
                }
            }
        }
    }
    None
}

/// Find the git root for a directory
pub fn find_git_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    let root = PathBuf::from("/");

    let max_iterations = 20;
    let mut iterations = 0;

    while current != root && iterations < max_iterations {
        if current.join(".git").exists() {
            return Some(current);
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
        iterations += 1;
    }

    None
}

/// Resolve prompt input (read from file if path, otherwise return as-is)
pub fn resolve_prompt_input(input: &str, description: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }

    let path = Path::new(input);
    if path.exists() {
        match fs::read_to_string(path) {
            Ok(content) => Some(content),
            Err(e) => {
                tracing::warn!(
                    "Warning: Could not read {} file {}: {}",
                    description,
                    input,
                    e
                );
                Some(input.to_string())
            }
        }
    } else {
        Some(input.to_string())
    }
}

/// Resolve the default resource directory
pub fn default_resource_dir() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("oxi")
}

/// Get the skills directory
pub fn skills_dir(base: &std::path::Path) -> std::path::PathBuf {
    base.join("skills")
}

/// Get the extensions directory
#[allow(dead_code)]
pub fn extensions_dir(base: &std::path::Path) -> std::path::PathBuf {
    base.join("extensions")
}

/// Get the themes directory
pub fn themes_dir(base: &std::path::Path) -> std::path::PathBuf {
    base.join("themes")
}

/// Get the prompts directory
pub fn prompts_dir(base: &std::path::Path) -> std::path::PathBuf {
    base.join("prompts")
}

/// Load skills from a directory
pub fn load_skills_from_dir(dir: &std::path::Path) -> LoadResult<Skill> {
    super::resource_loader_compat::load_skills_from_dir_impl(dir)
}

/// Load a single skill
pub fn load_skill(path: &std::path::Path) -> Result<Skill, String> {
    super::resource_loader_compat::load_skill_impl(path)
}

/// Load themes from a directory
pub fn load_themes_from_dir(dir: &std::path::Path) -> LoadResult<Theme> {
    super::resource_loader_compat::load_themes_from_dir_impl(dir)
}

/// Load a single theme
pub fn load_theme(path: &std::path::Path) -> Result<Theme, String> {
    super::resource_loader_compat::load_theme_impl(path)
}

/// Load prompts from a directory
pub fn load_prompts_from_dir(dir: &std::path::Path) -> LoadResult<Prompt> {
    super::resource_loader_compat::load_prompts_from_dir_impl(dir)
}

/// Load a single prompt
pub fn load_prompt(path: &std::path::Path) -> Result<Prompt, String> {
    super::resource_loader_compat::load_prompt_impl(path)
}

/// Load all resources from default locations
#[allow(dead_code)]
pub fn load_all_resources(base_dir: &std::path::Path) -> LoadAllResourcesResult {
    super::resource_loader_compat::load_all_resources_impl(base_dir)
}

/// Resolve a path with ~ expansion
pub fn resolve_path(path: &std::path::Path) -> std::path::PathBuf {
    let path_str = path.to_string_lossy();
    if path_str.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            // SAFE: strip_prefix guaranteed to return Some because we checked starts_with("~/") above
            return home.join(path_str.strip_prefix("~/").unwrap());
        }
    }
    path.to_path_buf()
}

// ============================================================================
// Deduplication
// ============================================================================

/// Deduplicate skills by ID, keeping first occurrence
fn dedupe_skills(skills: Vec<Skill>) -> (Vec<Skill>, Vec<ResourceCollision>) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<Skill> = Vec::new();
    let mut collisions = Vec::new();

    for skill in skills {
        if let Some(&existing_idx) = seen.get(&skill.id) {
            collisions.push(ResourceCollision {
                resource_type: "skill".to_string(),
                name: skill.id.clone(),
                winner_path: result[existing_idx].path.clone(),
                loser_path: skill.path.clone(),
            });
        } else {
            seen.insert(skill.id.clone(), result.len());
            result.push(skill);
        }
    }

    (result, collisions)
}

/// Deduplicate themes by ID, keeping first occurrence
fn dedupe_themes(themes: Vec<Theme>) -> (Vec<Theme>, Vec<ResourceCollision>) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<Theme> = Vec::new();
    let mut collisions = Vec::new();

    for theme in themes {
        let name = theme.name.clone();
        if let Some(&existing_idx) = seen.get(&name) {
            collisions.push(ResourceCollision {
                resource_type: "theme".to_string(),
                name: name.clone(),
                winner_path: result[existing_idx].path.clone(),
                loser_path: theme.path.clone(),
            });
        } else {
            seen.insert(name, result.len());
            result.push(theme);
        }
    }

    (result, collisions)
}

/// Deduplicate prompts by name, keeping first occurrence
fn dedupe_prompts(prompts: Vec<Prompt>) -> (Vec<Prompt>, Vec<ResourceCollision>) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<Prompt> = Vec::new();
    let mut collisions = Vec::new();

    for prompt in prompts {
        if let Some(&existing_idx) = seen.get(&prompt.name) {
            collisions.push(ResourceCollision {
                resource_type: "prompt".to_string(),
                name: prompt.name.clone(),
                winner_path: result[existing_idx].path.clone(),
                loser_path: prompt.path.clone(),
            });
        } else {
            seen.insert(prompt.name.clone(), result.len());
            result.push(prompt);
        }
    }

    (result, collisions)
}

// ============================================================================
// Re-exports from compat module
// ============================================================================

pub use super::resource_loader_compat::{
    LoadAllResourcesResult, LoadError, LoadResult, ResourceDiagnostic, ResourceType,
};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_context_file_creation() {
        let cf = ContextFile::new(
            PathBuf::from("/project/AGENTS.md"),
            "AGENTS.md",
            100,
            "# Agent Instructions\n".to_string(),
        );
        assert_eq!(cf.name, "AGENTS.md");
        assert_eq!(cf.priority, 100);
        assert_eq!(cf.extension(), Some("md".to_string()));
    }

    #[test]
    fn test_context_file_type_priority() {
        assert!(ContextFileType::Agents.priority() > ContextFileType::Claude.priority());
    }

    #[test]
    fn test_context_file_type_variants() {
        let agents_variants = ContextFileType::Agents.variants();
        assert!(agents_variants.contains(&"AGENTS.md"));
        assert!(agents_variants.contains(&"AGENTS.MD"));
    }

    #[test]
    fn test_context_file_type_from_filename() {
        assert_eq!(
            ContextFileType::from_filename("AGENTS.md"),
            Some(ContextFileType::Agents)
        );
        assert_eq!(
            ContextFileType::from_filename("CLAUDE.md"),
            Some(ContextFileType::Claude)
        );
        assert_eq!(ContextFileType::from_filename("unknown.md"), None);
    }

    #[test]
    fn test_source_type_display() {
        assert_eq!(SourceType::Default.to_string(), "default");
        assert_eq!(SourceType::Project.to_string(), "project");
        assert_eq!(SourceType::Cli.to_string(), "cli");
    }

    #[test]
    fn test_resource_loader_default() {
        let loader = ResourceLoader::new();
        assert!(loader.cached().is_none());
    }

    #[test]
    fn test_resource_loader_with_paths() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(temp.path().join("oxi"), temp.path().to_path_buf());
        assert_eq!(loader.cwd(), temp.path());
    }

    #[test]
    fn test_add_sources() {
        let mut loader = ResourceLoader::new();
        loader.add_extension(PathBuf::from("/extensions/my-ext"));
        loader.add_skill(PathBuf::from("/skills/my-skill"));
        loader.add_theme(PathBuf::from("/themes/my-theme"));
        loader.add_prompt(PathBuf::from("/prompts/my-prompt"));

        assert_eq!(loader.extensions.len(), 1);
        assert_eq!(loader.skills.len(), 1);
        assert_eq!(loader.themes.len(), 1);
        assert_eq!(loader.prompts.len(), 1);
    }

    #[test]
    fn test_load_all_empty() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(temp.path().join("oxi"), temp.path().to_path_buf());

        let result = loader.try_load_all();
        assert!(result.collisions.is_empty());
    }

    #[test]
    fn test_discover_context_files_empty_dir() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::new();

        let discovered = loader.discover_context_files(temp.path());
        assert!(discovered.is_empty());
    }

    #[test]
    fn test_discover_context_files_ancestor() {
        let temp = tempdir().unwrap();
        let subdir = temp.path().join("sub").join("project");
        fs::create_dir_all(&subdir).unwrap();

        // Create AGENTS.md in parent directory
        fs::write(temp.path().join("AGENTS.md"), "# Parent agents").unwrap();

        let loader = ResourceLoader::new();
        let discovered = loader.discover_context_files(&subdir);

        assert!(!discovered.is_empty());
    }

    #[test]
    fn test_load_system_prompt_not_found() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(temp.path().join("oxi"), temp.path().to_path_buf());

        let result = loader.load_system_prompt().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_load_system_prompt_from_file() {
        let temp = tempdir().unwrap();
        let agent_dir = temp.path().join("oxi");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("SYSTEM.md"), "System prompt content").unwrap();

        let loader = ResourceLoader::with_paths(agent_dir.clone(), temp.path().to_path_buf());

        let result = loader.load_system_prompt().unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "System prompt content");
    }

    #[test]
    fn test_load_system_prompt_explicit() {
        let temp = tempdir().unwrap();
        let mut opts = ResourceLoaderOptions::new();
        opts.agent_dir = temp.path().join("oxi");
        opts.cwd = temp.path().to_path_buf();
        opts.system_prompt = Some("Explicit prompt".to_string());

        let loader = ResourceLoader::with_options(opts);
        let result = loader.load_system_prompt().unwrap();
        assert_eq!(result, Some("Explicit prompt".to_string()));
    }

    #[test]
    fn test_load_append_system_prompt() {
        let temp = tempdir().unwrap();
        let agent_dir = temp.path().join("oxi");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("APPEND_SYSTEM.md"), "Append content").unwrap();

        let loader = ResourceLoader::with_paths(agent_dir.clone(), temp.path().to_path_buf());

        let result = loader.load_append_system_prompt().unwrap();
        assert_eq!(result, vec!["Append content".to_string()]);
    }

    #[test]
    fn test_cache_round_trip() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(temp.path().join("oxi"), temp.path().to_path_buf());

        assert!(loader.cached().is_none());

        let _ = loader.try_load_all();
        assert!(loader.cached().is_some());

        loader.clear_cache();
        assert!(loader.cached().is_none());
    }

    #[test]
    fn test_path_metadata_defaults() {
        let meta = PathMetadata::default();
        assert_eq!(meta.source, "local");
        assert_eq!(meta.scope, "user");
        assert_eq!(meta.origin, "top-level");
    }

    #[test]
    fn test_path_metadata_shortcuts() {
        let cli = PathMetadata::cli();
        assert_eq!(cli.source, "cli");
        assert_eq!(cli.scope, "temporary");

        let project = PathMetadata::project();
        assert_eq!(project.scope, "project");

        let user = PathMetadata::user();
        assert_eq!(user.scope, "user");
    }

    #[test]
    fn test_source_helper_methods() {
        let temp = tempdir().unwrap();
        let source = Source::new(temp.path().to_path_buf(), SourceType::Default);

        assert!(source.exists());
        assert!(source.is_dir());
        assert_eq!(source.source_type, SourceType::Default);
    }

    #[test]
    fn test_loader_builder_pattern() {
        let mut loader = ResourceLoader::new();
        loader.with_base_dir(PathBuf::from("/base"));
        loader.with_cwd(PathBuf::from("/cwd"));
        loader.add_extension(PathBuf::from("/ext"));
        loader.add_skill(PathBuf::from("/skill"));

        assert_eq!(loader.extensions.len(), 1);
        assert_eq!(loader.skills.len(), 1);
    }

    #[test]
    fn test_find_git_root_no_git() {
        let temp = tempdir().unwrap();
        let result = find_git_root(temp.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_find_git_root() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("sub").join("deep")).unwrap();
        fs::write(temp.path().join(".git"), "gitdir: somewhere").unwrap();

        let result = find_git_root(&temp.path().join("sub").join("deep"));
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp.path());
    }

    #[test]
    fn test_resolve_prompt_input_text() {
        let result = resolve_prompt_input("hello world", "test");
        assert_eq!(result, Some("hello world".to_string()));
    }

    #[test]
    fn test_resolve_prompt_input_empty() {
        let result = resolve_prompt_input("", "test");
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_prompt_input_from_file() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("prompt.txt");
        fs::write(&file_path, "file content").unwrap();

        let result = resolve_prompt_input(file_path.to_str().unwrap(), "test");
        assert_eq!(result, Some("file content".to_string()));
    }

    #[test]
    fn test_resource_collision_display() {
        let collision = ResourceCollision {
            resource_type: "skill".to_string(),
            name: "my-skill".to_string(),
            winner_path: PathBuf::from("/a/skill.md"),
            loser_path: PathBuf::from("/b/skill.md"),
        };
        let display = collision.to_string();
        assert!(display.contains("skill"));
        assert!(display.contains("my-skill"));
    }

    #[test]
    fn test_load_all_creates_cache() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(temp.path().join("oxi"), temp.path().to_path_buf());

        let result = loader.load_all().unwrap();

        let cached = loader.cached();
        assert!(cached.is_some());

        let cached = cached.unwrap();
        assert_eq!(cached.skills.len(), result.skills.len());
    }

    #[test]
    fn test_deduplication_in_discover() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("AGENTS.md"), "# Agents").unwrap();

        let loader = ResourceLoader::new();
        let discovered = loader.discover_context_files(temp.path());

        let paths: Vec<_> = discovered.iter().map(|(p, _)| p.clone()).collect();
        let unique: HashSet<_> = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert_eq!(paths.len(), unique.len());
    }

    #[test]
    fn test_resource_loader_options_default() {
        let opts = ResourceLoaderOptions::default();
        assert!(!opts.no_extensions);
        assert!(!opts.no_skills);
        assert!(!opts.no_prompts);
        assert!(!opts.no_themes);
        assert!(!opts.no_context_files);
    }

    #[test]
    fn test_extend_resources() {
        let mut loader = ResourceLoader::new();
        loader.extend_resources(
            vec![(PathBuf::from("/skill1"), PathMetadata::cli())],
            vec![(PathBuf::from("/prompt1"), PathMetadata::cli())],
            vec![(PathBuf::from("/theme1"), PathMetadata::cli())],
        );

        assert_eq!(loader.skills.len(), 1);
        assert_eq!(loader.prompts.len(), 1);
        assert_eq!(loader.themes.len(), 1);
    }

    #[test]
    fn test_detect_resource_type() {
        let temp = tempdir().unwrap();

        // Skill directory with SKILL.md
        let skill_dir = temp.path().join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();
        assert_eq!(
            ResourceLoader::detect_resource_type(&skill_dir),
            Some(ResourceType::Skill)
        );

        // Theme JSON file
        let theme_file = temp.path().join("theme.json");
        fs::write(&theme_file, r#"{"name": "test"}"#).unwrap();
        assert_eq!(
            ResourceLoader::detect_resource_type(&theme_file),
            Some(ResourceType::Theme)
        );
    }

    #[test]
    fn test_validate_resource_path() {
        let temp = tempdir().unwrap();

        let skill_file = temp.path().join("skill.md");
        fs::write(&skill_file, "# Skill").unwrap();

        let result = ResourceLoader::validate_resource_path(&skill_file);
        assert!(result.is_ok());

        let nonexistent = temp.path().join("nonexistent");
        let result = ResourceLoader::validate_resource_path(&nonexistent);
        assert!(result.is_err());
    }

    #[test]
    fn test_getters_without_cache() {
        let loader = ResourceLoader::new();
        assert!(loader.get_skills().is_empty());
        assert!(loader.get_themes().is_empty());
        assert!(loader.get_prompts().is_empty());
        assert!(loader.get_context_files().is_empty());
        assert!(loader.get_system_prompt().is_none());
        assert!(loader.get_append_system_prompt().is_empty());
        assert!(loader.get_agents_files().is_empty());
    }

    #[test]
    fn test_load_project_context_files_order() {
        let temp = tempdir().unwrap();

        // Create multiple context files
        fs::write(temp.path().join("CLAUDE.md"), "# Claude").unwrap();
        fs::write(temp.path().join("AGENTS.md"), "# Agents").unwrap();

        let loader = ResourceLoader::with_paths(temp.path().join("oxi"), temp.path().to_path_buf());

        let files = loader.load_project_context_files(temp.path()).unwrap();

        // AGENTS.md should come first (higher priority)
        if files.len() >= 2 {
            assert!(files[0].priority >= files[1].priority);
        }
    }

    #[test]
    fn test_source_info_serialization() {
        let info = SourceInfo {
            path: PathBuf::from("/test"),
            source: "local".to_string(),
            scope: "user".to_string(),
            origin: "top-level".to_string(),
            base_dir: Some(PathBuf::from("/base")),
        };
        let json = serde_json::to_string(&info).unwrap();
        let deserialized: SourceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.source, "local");
        assert_eq!(deserialized.base_dir, Some(PathBuf::from("/base")));
    }
}
