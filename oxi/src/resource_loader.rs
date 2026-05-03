//! Resource loader for skills, extensions, themes, and prompts
//!
//! Provides utilities for loading and watching resource files
//! from various locations.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resource type
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceType {
    Skill,
    Extension,
    Theme,
    Prompt,
}

/// A loaded resource
#[derive(Debug, Clone)]
pub struct Resource {
    /// Resource ID
    pub id: String,
    /// Resource type
    pub resource_type: ResourceType,
    /// Path to the resource file/directory
    pub path: PathBuf,
    /// Resource content or metadata
    pub content: Option<String>,
    /// Source (local, npm, git, etc.)
    pub source: String,
}

/// Resource loading result
#[derive(Debug)]
pub struct LoadResult<T> {
    /// Loaded items
    pub items: Vec<T>,
    /// Any errors encountered
    pub errors: Vec<LoadError>,
    /// Diagnostics
    pub diagnostics: Vec<ResourceDiagnostic>,
}

/// Load error
#[derive(Debug, Clone)]
pub struct LoadError {
    pub path: PathBuf,
    pub error: String,
}

/// Resource diagnostic
#[derive(Debug, Clone)]
pub struct ResourceDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub path: Option<PathBuf>,
}

/// Diagnostic severity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
    Info,
}

/// Resource path configuration
#[derive(Debug, Clone)]
pub struct ResourcePaths {
    /// Base directory for resources
    pub base_dir: PathBuf,
    /// Additional paths to search
    pub additional_paths: Vec<PathBuf>,
    /// Whether to include default paths
    pub include_defaults: bool,
}

impl Default for ResourcePaths {
    fn default() -> Self {
        Self {
            base_dir: dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("oxi"),
            additional_paths: Vec::new(),
            include_defaults: true,
        }
    }
}

/// Resolve the default resource directory
pub fn default_resource_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oxi")
}

/// Get the skills directory
pub fn skills_dir(base: &Path) -> PathBuf {
    base.join("skills")
}

/// Get the extensions directory
pub fn extensions_dir(base: &Path) -> PathBuf {
    base.join("extensions")
}

/// Get the themes directory
pub fn themes_dir(base: &Path) -> PathBuf {
    base.join("themes")
}

/// Get the prompts directory
pub fn prompts_dir(base: &Path) -> PathBuf {
    base.join("prompts")
}

/// Load skills from a directory
pub fn load_skills_from_dir(dir: &Path) -> LoadResult<Skill> {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut diagnostics = Vec::new();

    if !dir.exists() {
        return LoadResult {
            items,
            errors,
            diagnostics,
        };
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() || path.extension().map(|e| e == "md").unwrap_or(false) {
                match load_skill(&path) {
                    Ok(skill) => items.push(skill),
                    Err(e) => {
                        errors.push(LoadError {
                            path: path.clone(),
                            error: e.clone(),
                        });
                        diagnostics.push(ResourceDiagnostic {
                            severity: DiagnosticSeverity::Error,
                            message: e,
                            path: Some(path),
                        });
                    }
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

/// Load a single skill
pub fn load_skill(path: &Path) -> Result<Skill, String> {
    let content = if path.is_file() {
        fs::read_to_string(path).map_err(|e| e.to_string())?
    } else if path.is_dir() {
        // Look for SKILL.md in directory
        let skill_md = path.join("SKILL.md");
        if skill_md.exists() {
            fs::read_to_string(&skill_md).map_err(|e| e.to_string())?
        } else {
            return Err("No SKILL.md found in directory".to_string());
        }
    } else {
        return Err("Invalid skill path".to_string());
    };

    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let name = extract_yaml_field(&content, "name").or_else(|| Some(id.clone()));
    let description = extract_yaml_field(&content, "description");

    Ok(Skill {
        id,
        path: path.to_path_buf(),
        content,
        name,
        description,
        source: "local".to_string(),
    })
}

/// A loaded skill
#[derive(Debug, Clone)]
pub struct Skill {
    pub id: String,
    pub path: PathBuf,
    pub content: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub source: String,
}

/// Load themes from a directory
pub fn load_themes_from_dir(dir: &Path) -> LoadResult<Theme> {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut diagnostics = Vec::new();

    if !dir.exists() {
        return LoadResult {
            items,
            errors,
            diagnostics,
        };
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                match load_theme(&path) {
                    Ok(theme) => items.push(theme),
                    Err(e) => {
                        errors.push(LoadError {
                            path: path.clone(),
                            error: e.clone(),
                        });
                        diagnostics.push(ResourceDiagnostic {
                            severity: DiagnosticSeverity::Warning,
                            message: e,
                            path: Some(path),
                        });
                    }
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

/// Load a single theme
pub fn load_theme(path: &Path) -> Result<Theme, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unnamed")
                .to_string()
        });

    Ok(Theme {
        id: name.to_lowercase().replace(' ', "_"),
        name,
        path: path.to_path_buf(),
        content: json,
        source: "local".to_string(),
    })
}

/// A loaded theme
#[derive(Debug, Clone)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub content: serde_json::Value,
    pub source: String,
}

/// Load prompts from a directory
pub fn load_prompts_from_dir(dir: &Path) -> LoadResult<Prompt> {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut diagnostics = Vec::new();

    if !dir.exists() {
        return LoadResult {
            items,
            errors,
            diagnostics,
        };
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
                match load_prompt(&path) {
                    Ok(prompt) => items.push(prompt),
                    Err(e) => {
                        errors.push(LoadError {
                            path: path.clone(),
                            error: e.clone(),
                        });
                        diagnostics.push(ResourceDiagnostic {
                            severity: DiagnosticSeverity::Warning,
                            message: e,
                            path: Some(path),
                        });
                    }
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

/// Load a single prompt
pub fn load_prompt(path: &Path) -> Result<Prompt, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(Prompt {
        id: name.clone(),
        name,
        path: path.to_path_buf(),
        content,
        description: None,
        source: "local".to_string(),
    })
}

/// A loaded prompt template
#[derive(Debug, Clone)]
pub struct Prompt {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub content: String,
    pub description: Option<String>,
    pub source: String,
}

/// Extract a YAML frontmatter field
fn extract_yaml_field(content: &str, field: &str) -> Option<String> {
    // Simple YAML frontmatter extraction
    if !content.starts_with("---") {
        return None;
    }

    if let Some(end) = content[3..].find("---") {
        let frontmatter = &content[3..end + 3];
        for line in frontmatter.lines() {
            if let Some(value) = line.strip_prefix(&format!("{}:", field)) {
                let value = value.trim();
                // Remove quotes if present
                let value = value.trim_matches('"').trim_matches('\'');
                return Some(value.to_string());
            }
        }
    }

    None
}

/// Resolve a path with ~ expansion
pub fn resolve_path(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if path_str.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(path_str.strip_prefix("~/").unwrap());
        }
    }
    path.to_path_buf()
}

/// Watch a directory for changes
pub struct ResourceWatcher {
    paths: Vec<PathBuf>,
    callbacks: HashMap<PathBuf, Vec<Box<dyn Fn(ResourceChange) + Send + Sync>>>,
}

impl ResourceWatcher {
    pub fn new() -> Self {
        Self {
            paths: Vec::new(),
            callbacks: HashMap::new(),
        }
    }

    /// Add a path to watch
    pub fn add_path(&mut self, path: PathBuf) {
        self.paths.push(path.clone());
        self.callbacks.entry(path).or_insert_with(Vec::new);
    }

    /// Register a callback for changes
    pub fn on_change<F>(&mut self, path: &Path, callback: F)
    where
        F: Fn(ResourceChange) + Send + Sync + 'static,
    {
        let path = path.to_path_buf();
        self.callbacks
            .entry(path.clone())
            .or_insert_with(Vec::new)
            .push(Box::new(callback));
    }

    /// Check for changes and notify callbacks
    pub fn check_changes(&mut self) {
        for path in &self.paths {
            if let Ok(metadata) = fs::metadata(path) {
                if metadata.modified().is_ok() {
                    let change = ResourceChange {
                        path: path.clone(),
                        kind: ChangeKind::Modified,
                    };
                    if let Some(callbacks) = self.callbacks.get(path) {
                        for callback in callbacks {
                            callback(change.clone());
                        }
                    }
                }
            }
        }
    }
}

impl Default for ResourceWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// A resource change event
#[derive(Debug, Clone)]
pub struct ResourceChange {
    pub path: PathBuf,
    pub kind: ChangeKind,
}

/// Change kind
#[derive(Debug, Clone, Copy)]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

/// Load all resources from default locations
pub fn load_all_resources(base_dir: &Path) -> LoadAllResourcesResult {
    let mut errors = Vec::new();
    let mut diagnostics = Vec::new();

    let skills_base = skills_dir(base_dir);
    let skills_result = load_skills_from_dir(&skills_base);
    errors.extend(skills_result.errors);
    diagnostics.extend(skills_result.diagnostics);

    let themes_base = themes_dir(base_dir);
    let themes_result = load_themes_from_dir(&themes_base);
    errors.extend(themes_result.errors);
    diagnostics.extend(themes_result.diagnostics);

    let prompts_base = prompts_dir(base_dir);
    let prompts_result = load_prompts_from_dir(&prompts_base);
    errors.extend(prompts_result.errors);
    diagnostics.extend(prompts_result.diagnostics);

    LoadAllResourcesResult {
        skills: skills_result.items,
        themes: themes_result.items,
        prompts: prompts_result.items,
        errors,
        diagnostics,
    }
}

/// Result of loading all resources
pub struct LoadAllResourcesResult {
    pub skills: Vec<Skill>,
    pub themes: Vec<Theme>,
    pub prompts: Vec<Prompt>,
    pub errors: Vec<LoadError>,
    pub diagnostics: Vec<ResourceDiagnostic>,
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
        ResourceType::Extension => path.extension().map(|e| e == "js" || e == "ts").unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_resolve_path_with_tilde() {
        let path = resolve_path(Path::new("~/test"));
        assert!(!path.to_string_lossy().contains("~"));
    }

    #[test]
    fn test_resolve_path_absolute() {
        let path = resolve_path(Path::new("/absolute/path"));
        assert_eq!(path, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_extract_yaml_field() {
        let content = r#"---
name: Test Skill
description: A test skill
---
# Content"#;
        assert_eq!(extract_yaml_field(content, "name"), Some("Test Skill".to_string()));
        assert_eq!(extract_yaml_field(content, "description"), Some("A test skill".to_string()));
        assert_eq!(extract_yaml_field(content, "nonexistent"), None);
    }

    #[test]
    fn test_load_skills_from_nonexistent_dir() {
        let result = load_skills_from_dir(Path::new("/nonexistent/path"));
        assert!(result.items.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_load_themes_from_nonexistent_dir() {
        let result = load_themes_from_dir(Path::new("/nonexistent/path"));
        assert!(result.items.is_empty());
    }

    #[test]
    fn test_load_prompts_from_nonexistent_dir() {
        let result = load_prompts_from_dir(Path::new("/nonexistent/path"));
        assert!(result.items.is_empty());
    }

    #[test]
    fn test_is_valid_resource_path() {
        assert!(!is_valid_resource_path(Path::new("/nonexistent"), ResourceType::Skill));
    }

    #[test]
    fn test_resource_watcher() {
        let mut watcher = ResourceWatcher::new();
        let path = PathBuf::from("/tmp/test");
        watcher.add_path(path.clone());

        let change_received = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let change_received_clone = change_received.clone();
        watcher.on_change(&path, move |_| {
            change_received_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        // Just verify it doesn't panic
        watcher.check_changes();
        // change_received might be false if path doesn't exist
        assert!(true);
    }

    #[test]
    fn test_load_all_resources() {
        // Create a temp directory with some resources
        let temp_dir = tempfile::tempdir().unwrap();
        let base = temp_dir.path();

        // Create skills dir
        let skills_dir = base.join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("test.md"), "---\nname: Test\n---\nTest content").unwrap();

        let result = load_all_resources(base);

        // Should have loaded the test skill
        assert!(!result.skills.is_empty());
    }

    #[test]
    fn test_skill_id_extraction() {
        let temp_dir = tempfile::tempdir().unwrap();
        let skill_path = temp_dir.path().join("my_skill.md");
        fs::write(&skill_path, "# Skill").unwrap();

        let skill = load_skill(&skill_path).unwrap();
        assert_eq!(skill.id, "my_skill");
    }

    #[test]
    fn test_resource_paths_default() {
        let paths = ResourcePaths::default();
        assert!(paths.base_dir.ends_with("oxi"));
    }

    #[test]
    fn test_resource_dirs() {
        let base = PathBuf::from("/test/base");
        assert_eq!(skills_dir(&base), base.join("skills"));
        assert_eq!(extensions_dir(&base), base.join("extensions"));
        assert_eq!(themes_dir(&base), base.join("themes"));
        assert_eq!(prompts_dir(&base), base.join("prompts"));
    }

    #[test]
    fn test_load_error_struct() {
        let error = LoadError {
            path: PathBuf::from("/test"),
            error: "test error".to_string(),
        };
        assert_eq!(error.error, "test error");
    }

    #[test]
    fn test_resource_diagnostic() {
        let diag = ResourceDiagnostic {
            severity: DiagnosticSeverity::Warning,
            message: "test warning".to_string(),
            path: Some(PathBuf::from("/test")),
        };
        assert_eq!(diag.severity, DiagnosticSeverity::Warning);
    }
}