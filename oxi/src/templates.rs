//! Prompt template system for oxi
//!
//! Reusable prompt templates stored as Markdown files with `{{variable}}`
//! placeholders.  Templates live under `~/.oxi/templates/` and each `.md`
//! file becomes a named template (filename sans extension).

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// A single prompt template loaded from disk.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    /// Template name (derived from filename without extension).
    pub name: String,
    /// Raw template content containing `{{variable}}` placeholders.
    pub content: String,
    /// Variable names extracted from the template.
    pub variables: Vec<String>,
}

impl PromptTemplate {
    /// Parse a template from raw text, extracting `{{variable}}` names.
    pub fn parse(name: String, content: String) -> Self {
        let variables = extract_variables(&content);
        Self {
            name,
            content,
            variables,
        }
    }

    /// Render the template, replacing every `{{key}}` with the provided value.
    pub fn render(&self, vars: &HashMap<&str, &str>) -> Result<String> {
        // Check for missing variables
        let missing: Vec<&str> = self
            .variables
            .iter()
            .filter(|v| !vars.contains_key(v.as_str()))
            .map(|v| v.as_str())
            .collect();

        if !missing.is_empty() {
            bail!(
                "Missing variables for template '{}': {}",
                self.name,
                missing.join(", ")
            );
        }

        Ok(render_template(&self.content, vars))
    }
}

/// Extract all `{{variable}}` names from a template string.
fn extract_variables(template: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let chars: Vec<char> = template.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '{' && i + 1 < len && chars[i + 1] == '{' {
            i += 2; // skip '{{'
            let mut name = String::new();
            let mut found_end = false;
            while i < len {
                if chars[i] == '}' && i + 1 < len && chars[i + 1] == '}' {
                    i += 2; // skip '}}'
                    found_end = true;
                    break;
                }
                name.push(chars[i]);
                i += 1;
            }
            if found_end {
                let trimmed = name.trim().to_string();
                if !trimmed.is_empty() && seen.insert(trimmed.clone()) {
                    vars.push(trimmed);
                }
            }
        } else {
            i += 1;
        }
    }

    vars
}

/// Replace all `{{key}}` placeholders in `template` with values from `vars`.
fn render_template(template: &str, vars: &HashMap<&str, &str>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{{{}}}}}", key), value);
    }
    result
}

/// Manages a collection of named prompt templates.
#[derive(Debug, Clone, Default)]
pub struct TemplateManager {
    templates: HashMap<String, PromptTemplate>,
}

impl TemplateManager {
    /// Create an empty template manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load all `.md` files from a directory as templates.
    ///
    /// Each file becomes a template named after the file (without extension).
    /// Returns an error only if the directory cannot be read.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut manager = Self::new();

        if !dir.exists() {
            // No templates directory is fine — start empty.
            return Ok(manager);
        }

        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("Failed to read templates directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Only load .md files
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            if name.is_empty() {
                continue;
            }

            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read template: {}", path.display()))?;

            let template = PromptTemplate::parse(name.clone(), content);
            tracing::debug!(name = %name, vars = ?template.variables, "loaded template");
            manager.templates.insert(name, template);
        }

        Ok(manager)
    }

    /// Render a template by name with the given variables.
    pub fn render(&self, name: &str, vars: HashMap<&str, &str>) -> Result<String> {
        let template = self
            .templates
            .get(name)
            .with_context(|| format!("Template '{}' not found", name))?;
        template.render(&vars)
    }

    /// Get a template by name.
    pub fn get(&self, name: &str) -> Option<&PromptTemplate> {
        self.templates.get(name)
    }

    /// List all loaded template names.
    pub fn template_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.templates.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Number of loaded templates.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Whether any templates are loaded.
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_variables_simple() {
        let vars = extract_variables("Hello {{name}}, welcome to {{place}}!");
        assert_eq!(vars, vec!["name", "place"]);
    }

    #[test]
    fn test_extract_variables_dedup() {
        let vars = extract_variables("{{x}} + {{x}} = {{y}}");
        assert_eq!(vars, vec!["x", "y"]);
    }

    #[test]
    fn test_extract_variables_none() {
        let vars = extract_variables("No variables here.");
        assert!(vars.is_empty());
    }

    #[test]
    fn test_extract_variables_whitespace() {
        let vars = extract_variables("{{ name }} and {{  place  }}");
        assert_eq!(vars, vec!["name", "place"]);
    }

    #[test]
    fn test_render_template_basic() {
        let mut vars = HashMap::new();
        vars.insert("name", "world");
        vars.insert("lang", "Rust");

        let result = render_template("Hello {{name}}, write {{lang}}!", &vars);
        assert_eq!(result, "Hello world, write Rust!");
    }

    #[test]
    fn test_render_template_no_match() {
        let vars = HashMap::new();
        let result = render_template("No {{vars}} to replace", &vars);
        assert_eq!(result, "No {{vars}} to replace");
    }

    #[test]
    fn test_prompt_template_parse_and_render() {
        let tmpl = PromptTemplate::parse(
            "greet".to_string(),
            "Hello {{name}}, your role is {{role}}.".to_string(),
        );
        assert_eq!(tmpl.name, "greet");
        assert_eq!(tmpl.variables, vec!["name", "role"]);

        let mut vars = HashMap::new();
        vars.insert("name", "Alice");
        vars.insert("role", "admin");
        let result = tmpl.render(&vars).unwrap();
        assert_eq!(result, "Hello Alice, your role is admin.");
    }

    #[test]
    fn test_prompt_template_missing_vars() {
        let tmpl = PromptTemplate::parse(
            "greet".to_string(),
            "Hello {{name}}, role: {{role}}.".to_string(),
        );

        let mut vars = HashMap::new();
        vars.insert("name", "Alice");
        // missing "role"
        let err = tmpl.render(&vars).unwrap_err();
        assert!(err.to_string().contains("Missing variables"));
        assert!(err.to_string().contains("role"));
    }

    #[test]
    fn test_template_manager_new() {
        let mgr = TemplateManager::new();
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
    }

    #[test]
    fn test_template_manager_get_and_render() {
        let mut mgr = TemplateManager::new();
        let tmpl = PromptTemplate::parse(
            "review".to_string(),
            "Review this {{type}}: {{content}}".to_string(),
        );
        mgr.templates.insert("review".to_string(), tmpl);

        assert_eq!(mgr.template_names(), vec!["review"]);

        let mut vars = HashMap::new();
        vars.insert("type", "PR");
        vars.insert("content", "my changes");
        let result = mgr.render("review", vars).unwrap();
        assert_eq!(result, "Review this PR: my changes");
    }

    #[test]
    fn test_template_manager_not_found() {
        let mgr = TemplateManager::new();
        let err = mgr.render("nonexistent", HashMap::new()).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_load_from_dir_missing() {
        let mgr = TemplateManager::load_from_dir(Path::new("/nonexistent/templates")).unwrap();
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_load_from_dir_with_files() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path();

        std::fs::write(templates_dir.join("greet.md"), "Hello {{name}}!").unwrap();
        std::fs::write(templates_dir.join("review.md"), "Review {{lang}} code.").unwrap();
        // Non-md file should be skipped
        std::fs::write(templates_dir.join("notes.txt"), "Not a template").unwrap();

        let mgr = TemplateManager::load_from_dir(templates_dir).unwrap();
        assert_eq!(mgr.len(), 2);

        let mut names = mgr.template_names();
        names.sort();
        assert_eq!(names, vec!["greet", "review"]);

        let mut vars = HashMap::new();
        vars.insert("name", "world");
        assert_eq!(mgr.render("greet", vars).unwrap(), "Hello world!");
    }
}
