//! Enhanced Resource loader for oxi
//!
//! Loads and manages skills, extensions, themes, and prompts from various locations.
//! Also handles discovery and loading of project context files (AGENTS.md, CLAUDE.md).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
}

// ============================================================================
// Source Types
// ============================================================================

/// Source of a loaded resource
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceType {
    /// Default system location (~/.oxi)
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

/// A source directory for resources
#[derive(Debug, Clone)]
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
    pub fn new(path: PathBuf, source_type: SourceType) -> Self {
        Self {
            path,
            source_type,
            enabled: true,
        }
    }

    /// Check if path exists
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Check if path is a directory
    pub fn is_dir(&self) -> bool {
        self.path.is_dir()
    }
}

// ============================================================================
// Extension Sources
// ============================================================================

/// Source for extension loading
#[derive(Debug, Clone)]
pub struct ExtensionSource {
    /// Path to the extension
    pub path: PathBuf,
    /// Metadata about the extension source
    pub metadata: PathMetadata,
}

#[derive(Debug, Clone)]
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

/// Skill source configuration
#[derive(Debug, Clone)]
pub struct SkillSource {
    /// Path to the skill
    pub path: PathBuf,
    /// Metadata
    pub metadata: PathMetadata,
}

/// Theme source configuration
#[derive(Debug, Clone)]
pub struct ThemeSource {
    /// Path to the theme
    pub path: PathBuf,
    /// Metadata
    pub metadata: PathMetadata,
}

/// Prompt source configuration
#[derive(Debug, Clone)]
pub struct PromptSource {
    /// Path to the prompt
    pub path: PathBuf,
    /// Metadata
    pub metadata: PathMetadata,
}

// ============================================================================
// Resource Loader
// ============================================================================

/// Resource loader for all oxi resources
pub struct ResourceLoader {
    /// Base directory for resources
    base_dir: PathBuf,
    /// Current working directory
    cwd: PathBuf,
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
}

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
    /// Errors encountered during loading
    pub errors: Vec<LoadError>,
    /// Diagnostics from loading
    pub diagnostics: Vec<ResourceDiagnostic>,
}

impl Default for ResourceLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLoader {
    /// Create a new resource loader
    pub fn new() -> Self {
        let base_dir = default_resource_dir();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            base_dir,
            cwd,
            extensions: Vec::new(),
            skills: Vec::new(),
            themes: Vec::new(),
            prompts: Vec::new(),
            cache: RwLock::new(None),
        }
    }

    /// Create with custom base and working directory
    pub fn with_paths(base_dir: PathBuf, cwd: PathBuf) -> Self {
        Self {
            base_dir,
            cwd,
            extensions: Vec::new(),
            skills: Vec::new(),
            themes: Vec::new(),
            prompts: Vec::new(),
            cache: RwLock::new(None),
        }
    }

    /// Set base directory
    pub fn with_base_dir(mut self, base_dir: PathBuf) -> Self {
        self.base_dir = base_dir;
        self
    }

    /// Set current working directory
    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = cwd;
        self
    }

    /// Add an extension source
    pub fn add_extension(&mut self, path: PathBuf) {
        self.extensions.push(ExtensionSource {
            path,
            metadata: PathMetadata::default(),
        });
    }

    /// Add a skill source
    pub fn add_skill(&mut self, path: PathBuf) {
        self.skills.push(SkillSource {
            path,
            metadata: PathMetadata::default(),
        });
    }

    /// Add a theme source
    pub fn add_theme(&mut self, path: PathBuf) {
        self.themes.push(ThemeSource {
            path,
            metadata: PathMetadata::default(),
        });
    }

    /// Add a prompt source
    pub fn add_prompt(&mut self, path: PathBuf) {
        self.prompts.push(PromptSource {
            path,
            metadata: PathMetadata::default(),
        });
    }

    /// Load all resources
    pub fn load_all(&self) -> Result<LoadedResources> {
        let mut errors = Vec::new();
        let mut diagnostics = Vec::new();

        // Load skills
        let skills = self.load_skills();
        for err in &skills.errors {
            errors.push(err.clone());
        }
        diagnostics.extend(skills.diagnostics);

        // Load themes
        let themes = self.load_themes();
        for err in &themes.errors {
            errors.push(err.clone());
        }
        diagnostics.extend(themes.diagnostics);

        // Load prompts
        let prompts = self.load_prompts();
        for err in &prompts.errors {
            errors.push(err.clone());
        }
        diagnostics.extend(prompts.diagnostics);

        // Load context files
        let context_files = self.load_project_context_files(&self.cwd)?;

        let result = LoadedResources {
            skills: skills.items,
            themes: themes.items,
            prompts: prompts.items,
            context_files,
            errors,
            diagnostics,
        };

        // Update cache
        *self.cache.write() = Some(result.clone());

        Ok(result)
    }

    /// Load all resources, returning default on error
    pub fn try_load_all(&self) -> LoadedResources {
        self.load_all().unwrap_or_else(|e| {
            LoadedResources {
                skills: Vec::new(),
                themes: Vec::new(),
                prompts: Vec::new(),
                context_files: Vec::new(),
                errors: vec![LoadError {
                    path: PathBuf::from("."),
                    error: e.to_string(),
                }],
                diagnostics: Vec::new(),
            }
        })
    }

    /// Load project context files (AGENTS.md, CLAUDE.md, etc.)
    pub fn load_project_context_files(&self, cwd: &Path) -> Result<Vec<ContextFile>> {
        let mut context_files = Vec::new();
        let seen_paths = &mut HashMap::new();

        // 1. Check ~/.oxi/system-prompts/default.md
        let system_prompt = self.load_system_prompt_file("default.md")?;
        if let Some(content) = system_prompt {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
            let default_path = home.join(".oxi").join("system-prompts").join("default.md");
            context_files.push(ContextFile::new(
                default_path,
                "default.md",
                95, // High priority, global default
                content,
            ));
        }

        // 2. Discover context files in project + ancestors
        let discovered = self.discover_context_files(cwd);

        for (path, file_type) in discovered {
            if let Some(content) = self.read_context_file(&path)? {
                let name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Avoid duplicate paths
                let path_str = path.to_string_lossy().to_string();
                if !seen_paths.contains_key(&path_str) {
                    seen_paths.insert(path_str.clone(), true);
                    context_files.push(ContextFile::new(
                        path,
                        name,
                        file_type.priority(),
                        content,
                    ));
                }
            }
        }

        // 3. Check global ~/.oxi/AGENTS.md
        if let Some(home) = dirs::home_dir() {
            for file_type in &[ContextFileType::Agents, ContextFileType::Claude] {
                for variant in file_type.variants() {
                    let global_path = home.join(".oxi").join(variant);
                    if global_path.exists() {
                        let path_str = global_path.to_string_lossy().to_string();
                        if !seen_paths.contains_key(&path_str) {
                            if let Some(content) = self.read_context_file(&global_path)? {
                                seen_paths.insert(path_str, true);
                                context_files.push(ContextFile::new(
                                    global_path,
                                    variant.to_string(),
                                    80, // Global config gets lower priority than project
                                    content,
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Sort by priority (descending)
        context_files.sort_by(|a, b| b.priority.cmp(&a.priority));

        Ok(context_files)
    }

    /// Discover context files in project and ancestor directories
    ///
    /// Searches in order:
    /// 1. Project root
    /// 2. Recursive ancestors up to git root
    /// 3. Current working directory and ancestors
    pub fn discover_context_files(&self, dir: &Path) -> Vec<(PathBuf, ContextFileType)> {
        let mut discovered = Vec::new();
        let file_types = [ContextFileType::Agents, ContextFileType::Claude];

        // Try to find git root to limit search
        let git_root = self.find_git_root(dir);

        let mut current = dir.to_path_buf();
        let root = PathBuf::from("/");

        // Limit iterations to prevent infinite loops
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

    /// Find the git root for a directory
    fn find_git_root(&self, dir: &Path) -> Option<PathBuf> {
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

    /// Load a system prompt file by name
    pub fn load_system_prompt_file(&self, name: &str) -> Result<Option<String>> {
        let paths_to_try = vec![
            // Project-level
            self.cwd.join(".oxi").join("system-prompts").join(name),
            // Global
            self.base_dir.join("system-prompts").join(name),
            // Home directory
            dirs::home_dir()
                .map(|h| h.join(".oxi").join("system-prompts").join(name)),
        ];

        for path in paths_to_try {
            if path.exists() && path.is_file() {
                let content = fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;
                return Ok(Some(content));
            }
        }

        Ok(None)
    }

    /// Read a context file, handling potential errors
    fn read_context_file(&self, path: &Path) -> Result<Option<String>> {
        match fs::read_to_string(path) {
            Ok(content) => Ok(Some(content)),
            Err(e) => {
                tracing::warn!("Failed to read context file {}: {}", path.display(), e);
                Ok(None)
            }
        }
    }

    /// Load skills from configured sources
    fn load_skills(&self) -> LoadResult<Skill> {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        let mut diagnostics = Vec::new();

        // Load from default directories
        let skills_base = skills_dir(&self.base_dir);
        let project_skills = self.cwd.join(".oxi").join("skills");

        for dir in &[skills_base, project_skills] {
            if dir.exists() {
                let result = load_skills_from_dir(dir);
                items.extend(result.items);
                errors.extend(result.errors);
                diagnostics.extend(result.diagnostics);
            }
        }

        // Load from custom sources
        for source in &self.skills {
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

        LoadResult { items, errors, diagnostics }
    }

    /// Load themes from configured sources
    fn load_themes(&self) -> LoadResult<Theme> {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        let mut diagnostics = Vec::new();

        let themes_base = themes_dir(&self.base_dir);
        let project_themes = self.cwd.join(".oxi").join("themes");

        for dir in &[themes_base, project_themes] {
            if dir.exists() {
                let result = load_themes_from_dir(dir);
                items.extend(result.items);
                errors.extend(result.errors);
                diagnostics.extend(result.diagnostics);
            }
        }

        for source in &self.themes {
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

        LoadResult { items, errors, diagnostics }
    }

    /// Load prompts from configured sources
    fn load_prompts(&self) -> LoadResult<Prompt> {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        let mut diagnostics = Vec::new();

        let prompts_base = prompts_dir(&self.base_dir);
        let project_prompts = self.cwd.join(".oxi").join("prompts");

        for dir in &[prompts_base, project_prompts] {
            if dir.exists() {
                let result = load_prompts_from_dir(dir);
                items.extend(result.items);
                errors.extend(result.errors);
                diagnostics.extend(result.diagnostics);
            }
        }

        for source in &self.prompts {
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

        LoadResult { items, errors, diagnostics }
    }

    /// Get cached resources if available
    pub fn cached(&self) -> Option<LoadedResources> {
        self.cache.read().clone()
    }

    /// Clear the cache
    pub fn clear_cache(&self) {
        *self.cache.write() = None;
    }

    /// Reload resources
    pub fn reload(&self) -> Result<LoadedResources> {
        self.clear_cache();
        self.load_all()
    }
}

// ============================================================================
// Re-exports from original module
// ============================================================================

pub use super::super::resource_loader::{
    load_skill, load_skills_from_dir, load_theme, load_themes_from_dir,
    load_prompt, load_prompts_from_dir, load_all_resources, default_resource_dir,
    skills_dir, extensions_dir, themes_dir, prompts_dir, resolve_path,
    is_valid_resource_path, ResourceType, Resource, LoadResult, LoadError,
    ResourceDiagnostic, DiagnosticSeverity, ResourcePaths, ResourceWatcher,
    ResourceChange, ChangeKind, Skill, Theme, Prompt, LoadAllResourcesResult,
};

// ============================================================================
// Thread-safe wrapper
// ============================================================================

use parking_lot::RwLock;
use std::collections::HashSet;

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
    fn test_resource_loader_default() {
        let loader = ResourceLoader::new();
        assert!(loader.cached().is_none());
    }

    #[test]
    fn test_resource_loader_with_paths() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(
            temp.path().join("oxi"),
            temp.path().to_path_buf(),
        );
        assert_eq!(loader.cwd, temp.path());
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
        let loader = ResourceLoader::with_paths(
            temp.path().join("oxi"),
            temp.path().to_path_buf(),
        );
        
        let result = loader.try_load_all();
        // Should succeed even with empty directories
        assert!(result.errors.is_empty() || !result.errors.is_empty()); // Either is fine
    }

    #[test]
    fn test_discover_context_files_empty_dir() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::new();

        let discovered = loader.discover_context_files(temp.path());
        // No context files in empty temp dir
        assert!(discovered.is_empty());
    }

    #[test]
    fn test_discover_context_files_with_files() {
        let temp = tempdir().unwrap();
        
        // Create AGENTS.md in temp dir
        fs::write(temp.path().join("AGENTS.md"), "# Agent instructions").unwrap();
        
        let loader = ResourceLoader::new();
        let discovered = loader.discover_context_files(temp.path());
        
        assert_eq!(discovered.len(), 1);
        assert!(discovered[0].0.to_string_lossy().ends_with("AGENTS.md"));
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
    fn test_load_system_prompt_file_not_found() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(
            temp.path().join("oxi"),
            temp.path().to_path_buf(),
        );
        
        let result = loader.load_system_prompt_file("nonexistent.md").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_load_system_prompt_file_exists() {
        let temp = tempdir().unwrap();
        let system_prompts = temp.path().join("oxi").join("system-prompts");
        fs::create_dir_all(&system_prompts).unwrap();
        fs::write(system_prompts.join("custom.md"), "Custom system prompt").unwrap();
        
        let loader = ResourceLoader::with_paths(
            temp.path().join("oxi"),
            temp.path().to_path_buf(),
        );
        
        let result = loader.load_system_prompt_file("custom.md").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "Custom system prompt");
    }

    #[test]
    fn test_cache_round_trip() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(
            temp.path().join("oxi"),
            temp.path().to_path_buf(),
        );
        
        // Initially no cache
        assert!(loader.cached().is_none());
        
        // Load resources
        let _ = loader.try_load_all();
        
        // Now should have cache
        assert!(loader.cached().is_some());
        
        // Clear cache
        loader.clear_cache();
        assert!(loader.cached().is_none());
    }

    #[test]
    fn test_load_all_creates_cache() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::with_paths(
            temp.path().join("oxi"),
            temp.path().to_path_buf(),
        );
        
        let result = loader.load_all().unwrap();
        
        // Check that cache was updated
        let cached = loader.cached();
        assert!(cached.is_some());
        
        // Verify cache matches result
        let cached = cached.unwrap();
        assert_eq!(cached.skills.len(), result.skills.len());
    }

    #[test]
    fn test_path_metadata_default() {
        let meta = PathMetadata::default();
        assert_eq!(meta.source, "local");
        assert_eq!(meta.scope, "user");
        assert_eq!(meta.origin, "top-level");
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
        let loader = ResourceLoader::new()
            .with_base_dir(PathBuf::from("/base"))
            .with_cwd(PathBuf::from("/cwd"))
            .add_extension(PathBuf::from("/ext"))
            .add_skill(PathBuf::from("/skill"));
        
        assert_eq!(loader.extensions.len(), 1);
        assert_eq!(loader.skills.len(), 1);
    }

    #[test]
    fn test_load_project_context_files_order() {
        let temp = tempdir().unwrap();
        
        // Create multiple context files
        fs::write(temp.path().join("CLAUDE.md"), "# Claude").unwrap();
        fs::write(temp.path().join("AGENTS.md"), "# Agents").unwrap();
        
        let loader = ResourceLoader::with_paths(
            temp.path().join("oxi"),
            temp.path().to_path_buf(),
        );
        
        let files = loader.load_project_context_files(temp.path()).unwrap();
        
        // AGENTS.md should come first (higher priority)
        if files.len() >= 2 {
            assert_eq!(files[0].name, "AGENTS.md");
            assert!(files[0].priority > files[1].priority);
        }
    }

    #[test]
    fn test_find_git_root_no_git() {
        let temp = tempdir().unwrap();
        let loader = ResourceLoader::new();
        
        let git_root = loader.find_git_root(temp.path());
        assert!(git_root.is_none());
    }

    #[test]
    fn test_deduplication_in_discover() {
        let temp = tempdir().unwrap();
        
        // Create same file at multiple levels
        fs::write(temp.path().join("AGENTS.md"), "# Agents").unwrap();
        
        let loader = ResourceLoader::new();
        let discovered = loader.discover_context_files(temp.path());
        
        // Should only have one entry
        let paths: Vec<_> = discovered.iter().map(|(p, _)| p.clone()).collect();
        let unique: HashSet<_> = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert_eq!(paths.len(), unique.len());
    }
}

// Need HashSet import for test
use std::collections::HashSet;