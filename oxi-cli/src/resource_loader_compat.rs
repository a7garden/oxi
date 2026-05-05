//! Compatibility module for resource loading functions
//!
//! This module provides the original resource loading functionality
//! that was moved to a separate module to avoid conflicts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Resource type
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceType {
/// skill variant.
    Skill,
/// extension variant.
    Extension,
/// theme variant.
    Theme,
/// prompt variant.
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
/// warning variant.
    Warning,
/// error variant.
    Error,
/// info variant.
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

/// Load skills from a directory (impl version)
pub fn load_skills_from_dir_impl(dir: &Path) -> LoadResult<Skill> {
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
                match load_skill_impl(&path) {
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

/// Load a single skill (impl version)
pub fn load_skill_impl(path: &Path) -> Result<Skill, String> {
    let content = if path.is_file() {
        fs::read_to_string(path).map_err(|e| e.to_string())?
    } else if path.is_dir() {
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

/// Load themes from a directory (impl version)
pub fn load_themes_from_dir_impl(dir: &Path) -> LoadResult<Theme> {
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
                match load_theme_impl(&path) {
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

/// Load a single theme (impl version)
pub fn load_theme_impl(path: &Path) -> Result<Theme, String> {
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

/// Load prompts from a directory (impl version)
pub fn load_prompts_from_dir_impl(dir: &Path) -> LoadResult<Prompt> {
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
                match load_prompt_impl(&path) {
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

/// Load a single prompt (impl version)
pub fn load_prompt_impl(path: &Path) -> Result<Prompt, String> {
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
    if !content.starts_with("---") {
        return None;
    }

    if let Some(end) = content[3..].find("---") {
        let frontmatter = &content[3..end + 3];
        for line in frontmatter.lines() {
            if let Some(value) = line.strip_prefix(&format!("{}:", field)) {
                let value = value.trim();
                let value = value.trim_matches('"').trim_matches('\'');
                return Some(value.to_string());
            }
        }
    }

    None
}

/// Resolve a path with ~ expansion
pub fn resolve_path_impl(path: &Path) -> PathBuf {
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

/// TODO: document this function.
    pub fn add_path(&mut self, path: PathBuf) {
        self.paths.push(path.clone());
        self.callbacks.entry(path).or_insert_with(Vec::new);
    }

/// TODO: document this function.
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

/// TODO: document this function.
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
/// created variant.
    Created,
/// modified variant.
    Modified,
/// deleted variant.
    Deleted,
}

/// Load all resources from default locations
pub fn load_all_resources_impl(base_dir: &Path) -> LoadAllResourcesResult {
    let mut errors = Vec::new();
    let mut diagnostics = Vec::new();

    let skills_base = skills_dir(base_dir);
    let skills_result = load_skills_from_dir_impl(&skills_base);
    errors.extend(skills_result.errors);
    diagnostics.extend(skills_result.diagnostics);

    let themes_base = themes_dir(base_dir);
    let themes_result = load_themes_from_dir_impl(&themes_base);
    errors.extend(themes_result.errors);
    diagnostics.extend(themes_result.diagnostics);

    let prompts_base = prompts_dir(base_dir);
    let prompts_result = load_prompts_from_dir_impl(&prompts_base);
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