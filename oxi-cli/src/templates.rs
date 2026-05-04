//! Prompt template system for oxi
//!
//! Reusable prompt templates stored as Markdown files with `{{variable}}`
//! placeholders, helper functions, and conditional blocks.
//!
//! Templates live under `~/.oxi/templates/` (global) and `.oxi/prompts/` (project),
//! and each `.md` file becomes a named template (filename sans extension).
//!
//! ## Template syntax
//!
//! - `{{variable}}` — simple variable substitution
//! - `{{helper}}` — built-in helper (date, time, cwd, git_branch)
//! - `{{env VAR}}` — environment variable access
//! - `{{#if variable}}...{{/if}}` — conditional blocks
//! - `{{#if variable}}...{{else}}...{{/if}}` — conditional with else

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

/// Config directory name for project-local settings.
const CONFIG_DIR_NAME: &str = ".oxi";

// ---------------------------------------------------------------------------
// PromptTemplate
// ---------------------------------------------------------------------------

/// A single prompt template loaded from disk.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    /// Template name (derived from filename without extension).
    pub name: String,
    /// Human-readable description (from frontmatter or first line).
    pub description: String,
    /// Optional hint about expected arguments.
    pub argument_hint: Option<String>,
    /// Raw template content containing `{{variable}}` placeholders.
    pub content: String,
    /// Variable names extracted from the template.
    pub variables: Vec<String>,
    /// Absolute path to the source file.
    pub file_path: PathBuf,
}

impl PromptTemplate {
    /// Parse a template from raw text, extracting `{{variable}}` names.
    pub fn parse(name: String, content: String) -> Self {
        let variables = extract_variables(&content);
        Self {
            name,
            description: String::new(),
            argument_hint: None,
            content,
            variables,
            file_path: PathBuf::new(),
        }
    }

    /// Parse a template from raw text with full metadata.
    pub fn parse_with_meta(
        name: String,
        content: String,
        description: String,
        argument_hint: Option<String>,
        file_path: PathBuf,
    ) -> Self {
        let variables = extract_variables(&content);
        Self {
            name,
            description,
            argument_hint,
            content,
            variables,
            file_path,
        }
    }

    /// Render the template, replacing every `{{key}}` with the provided value,
    /// evaluating helpers and conditionals.
    pub fn render(&self, vars: &HashMap<&str, &str>) -> Result<String> {
        let context = RenderContext::new(vars);
        context.render(&self.content)
    }

    /// Render the template with argument substitution (bash-style `$1`, `$@`, etc.).
    pub fn render_with_args(&self, args: &[String]) -> Result<String> {
        let substituted = substitute_args(&self.content, args);
        // After argument substitution, still process {{variable}} placeholders
        // (empty vars map — only helpers and conditionals will fire)
        let vars = HashMap::new();
        let context = RenderContext::new(&vars);
        context.render(&substituted)
    }
}

// ---------------------------------------------------------------------------
// Variable extraction
// ---------------------------------------------------------------------------

/// Extract all `{{variable}}` / `{{helper arg}}` names from a template string.
/// Returns a deduplicated, ordered list of variable names (excluding helpers and conditionals).
fn extract_variables(template: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let chars: Vec<char> = template.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Skip conditional blocks {{#if ...}}...{{/if}}
        if chars[i] == '{' && i + 1 < len && chars[i + 1] == '{' {
            let rest_start = i + 2;
            if rest_start < len && chars[rest_start] == '#' {
                // Skip to the closing }}
                i = rest_start + 1;
                while i < len {
                    if chars[i] == '}' && i + 1 < len && chars[i + 1] == '}' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                continue;
            }

            // Regular {{...}} expression
            i += 2;
            let mut name = String::new();
            let mut found_end = false;
            while i < len {
                if chars[i] == '}' && i + 1 < len && chars[i + 1] == '}' {
                    i += 2;
                    found_end = true;
                    break;
                }
                name.push(chars[i]);
                i += 1;
            }
            if found_end {
                let trimmed = name.trim().to_string();
                if !trimmed.is_empty() {
                    // Skip helpers and keywords
                    if !is_helper_or_keyword(&trimmed) && seen.insert(trimmed.clone()) {
                        vars.push(trimmed);
                    }
                }
            }
        } else {
            i += 1;
        }
    }

    vars
}

/// Check if a template expression is a built-in helper or keyword (not a user variable).
fn is_helper_or_keyword(expr: &str) -> bool {
    let lower = expr.to_lowercase();
    // Built-in helpers
    matches!(
        lower.as_str(),
        "date" | "time" | "cwd" | "git_branch" | "hostname" | "username"
    ) ||
        // env:: prefix
        lower.starts_with("env ") ||
        lower.starts_with("env::") ||
        // Keywords
        lower == "else" ||
        lower.starts_with("if ") ||
        lower.starts_with("/if")
}

// ---------------------------------------------------------------------------
// Argument substitution (bash-style)
// ---------------------------------------------------------------------------

/// Substitute argument placeholders in template content.
///
/// Supports:
/// - `$1`, `$2`, ... for positional args
/// - `$@` and `$ARGUMENTS` for all args
/// - `${@:N}` for args from Nth onwards
/// - `${@:N:L}` for L args starting from Nth
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let mut result = content.to_string();

    // Replace $1, $2, etc. with positional args FIRST
    let positional_re = regex::Regex::new(r"\$(\d+)").unwrap();
    result = positional_re
        .replace_all(&result, |caps: &regex::Captures| {
            let num: usize = caps[1].parse().unwrap_or(0);
            if num == 0 {
                String::new()
            } else {
                args.get(num - 1).cloned().unwrap_or_default()
            }
        })
        .into_owned();

    // Replace ${@:start} or ${@:start:length} with sliced args
    let slice_re = regex::Regex::new(r"\$\{@:(\d+)(?::(\d+))?\}").unwrap();
    result = slice_re
        .replace_all(&result, |caps: &regex::Captures| {
            let mut start: usize = caps[1].parse().unwrap_or(1);
            if start == 0 {
                start = 0;
            } else {
                start -= 1; // Convert to 0-indexed
            }
            if let Some(length_str) = caps.get(2) {
                let length: usize = length_str.as_str().parse().unwrap_or(0);
                args.iter()
                    .skip(start)
                    .take(length)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                args.iter().skip(start).cloned().collect::<Vec<_>>().join(" ")
            }
        })
        .into_owned();

    // Pre-compute all args joined
    let all_args = args.join(" ");

    // Replace $ARGUMENTS with all args
    result = result.replace("$ARGUMENTS", &all_args);

    // Replace $@ with all args
    result = result.replace("$@", &all_args);

    result
}

/// Parse command arguments respecting quoted strings (bash-style).
/// Returns array of arguments.
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for ch in args_string.chars() {
        match in_quote {
            Some(qc) => {
                if ch == qc {
                    in_quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    in_quote = Some(ch);
                } else if ch == ' ' || ch == '\t' {
                    if !current.is_empty() {
                        args.push(std::mem::take(&mut current));
                    }
                } else {
                    current.push(ch);
                }
            }
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

// ---------------------------------------------------------------------------
// Render context — helpers + conditionals + variable substitution
// ---------------------------------------------------------------------------

/// Rendering context that resolves helpers, conditionals, and variables.
#[derive(Debug)]
struct RenderContext<'a> {
    vars: &'a HashMap<&'a str, &'a str>,
}

impl<'a> RenderContext<'a> {
    fn new(vars: &'a HashMap<&'a str, &'a str>) -> Self {
        Self { vars }
    }

    /// Render a template string with full expression evaluation.
    fn render(&self, template: &str) -> Result<String> {
        let mut result = String::with_capacity(template.len());
        let chars: Vec<char> = template.chars().collect();
        let len = chars.len();
        let mut i = 0;

        while i < len {
            // Look for {{...}}
            if chars[i] == '{' && i + 1 < len && chars[i + 1] == '{' {
                let expr_start = i + 2;
                let mut j = expr_start;
                let mut found_end = false;
                while j < len {
                    if chars[j] == '}' && j + 1 < len && chars[j + 1] == '}' {
                        found_end = true;
                        break;
                    }
                    j += 1;
                }

                if found_end {
                    let expr: String = chars[expr_start..j].iter().collect();
                    let expr_trimmed = expr.trim();

                    if expr_trimmed.starts_with("#if ") {
                        // Conditional block {{#if var}}...{{/if}}
                        let condition = expr_trimmed[4..].trim();
                        let cond_value = self.resolve_condition_value(condition);
                        let block_end = format!("{{{{/{}}}}}", "if");
                        let else_marker = "{{else}}";

                        let content_start = j + 2; // after }}
                        let rest = &template[content_start..];

                        // Find the matching {{/if}}
                        let end_pos = rest.find(&block_end);
                        match end_pos {
                            Some(ep) => {
                                let block_content = &rest[..ep];
                                let after_end = content_start + ep + block_end.len();

                                if cond_value {
                                    // Render the if-branch, stripping the else if present
                                    let if_content = match block_content.find(else_marker) {
                                        Some(else_pos) => &block_content[..else_pos],
                                        None => block_content,
                                    };
                                    result.push_str(if_content);
                                } else {
                                    // Render the else-branch if present
                                    match block_content.find(else_marker) {
                                        Some(else_pos) => {
                                            result.push_str(&block_content[else_pos + else_marker.len()..]);
                                        }
                                        None => { /* no else branch, render nothing */ }
                                    }
                                }

                                i = chars.len(); // We've jumped ahead in template string space
                                // Recalculate i from string positions
                                // The `result` is built incrementally; we need to sync i with
                                // the string position `after_end`
                                i = template[..after_end].chars().count();
                                continue;
                            }
                            None => {
                                // No matching {{/if}} — treat as literal
                                result.push_str("{{");
                                i += 2;
                                continue;
                            }
                        }
                    } else if expr_trimmed == "else" || expr_trimmed.starts_with("/if") {
                        // Stray else or /if outside a conditional — skip
                        i = j + 2;
                        continue;
                    } else {
                        // Regular expression: variable or helper
                        let resolved = self.resolve_expression(expr_trimmed);
                        result.push_str(&resolved);
                        i = j + 2;
                        continue;
                    }
                } else {
                    // No closing }}, treat as literal
                    result.push_str("{{");
                    i += 2;
                    continue;
                }
            } else {
                result.push(chars[i]);
                i += 1;
            }
        }

        Ok(result)
    }

    /// Resolve a condition value: truthy if the variable is non-empty and not "false".
    fn resolve_condition_value(&self, condition: &str) -> bool {
        let resolved = self.resolve_expression(condition);
        !resolved.is_empty() && resolved.to_lowercase() != "false"
    }

    /// Resolve a single `{{expr}}` — variable, helper, or env access.
    fn resolve_expression(&self, expr: &str) -> String {
        // Check user variables first
        if let Some(val) = self.vars.get(expr) {
            return (*val).to_string();
        }

        // Built-in helpers
        match expr {
            "date" => {
                let now = chrono::Local::now();
                now.format("%Y-%m-%d").to_string()
            }
            "time" => {
                let now = chrono::Local::now();
                now.format("%H:%M:%S").to_string()
            }
            "cwd" => env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            "git_branch" => {
                env::current_dir()
                    .ok()
                    .and_then(|d| crate::git_utils::get_current_branch(&d))
                    .unwrap_or_default()
            }
            "hostname" => env::var("HOSTNAME")
                .or_else(|_| env::var("HOST"))
                .unwrap_or_default(),
            "username" => env::var("USER")
                .or_else(|_| env::var("USERNAME"))
                .unwrap_or_default(),
            _ => {
                // env::VAR or env VAR — access environment variable
                let rest = expr
                    .strip_prefix("env::")
                    .or_else(|| expr.strip_prefix("env "))
                    .map(str::trim);
                if let Some(var_name) = rest {
                    env::var(var_name).unwrap_or_default()
                } else {
                    // Unknown expression — leave as-is
                    format!("{{{{{}}}}}", expr)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TemplateManager
// ---------------------------------------------------------------------------

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
            return Ok(manager);
        }

        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("Failed to read templates directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

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

            let (description, argument_hint, body) = parse_template_metadata(&content);

            let template = PromptTemplate::parse_with_meta(
                name.clone(),
                body,
                description,
                argument_hint,
                path.clone(),
            );

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

    /// Insert a template into the manager.
    pub fn insert(&mut self, template: PromptTemplate) {
        self.templates.insert(template.name.clone(), template);
    }
}

// ---------------------------------------------------------------------------
// Template loading — multi-source (mirrors the TS loadPromptTemplates)
// ---------------------------------------------------------------------------

/// Options for loading prompt templates from multiple sources.
#[derive(Debug, Clone)]
pub struct LoadPromptTemplatesOptions {
    /// Working directory for project-local templates.
    pub cwd: PathBuf,
    /// Agent config directory for global templates.
    pub agent_dir: Option<PathBuf>,
    /// Explicit prompt template paths (files or directories).
    pub prompt_paths: Vec<String>,
    /// Include default prompt directories.
    pub include_defaults: bool,
}

/// Load all prompt templates from:
/// 1. Global: `agent_dir/prompts/`
/// 2. Project: `cwd/.oxi/prompts/`
/// 3. Explicit prompt paths
pub fn load_prompt_templates(options: &LoadPromptTemplatesOptions) -> Vec<PromptTemplate> {
    let mut templates = Vec::new();

    let global_prompts_dir = options
        .agent_dir
        .as_ref()
        .map(|d| d.join("prompts"))
        .unwrap_or_else(|| options.cwd.clone());
    let project_prompts_dir = options.cwd.join(CONFIG_DIR_NAME).join("prompts");

    if options.include_defaults {
        load_templates_from_dir_into(&global_prompts_dir, &mut templates);
        load_templates_from_dir_into(&project_prompts_dir, &mut templates);
    }

    // Load explicit prompt paths
    for raw_path in &options.prompt_paths {
        let resolved = resolve_prompt_path(raw_path, &options.cwd);
        if !resolved.exists() {
            continue;
        }

        if resolved.is_dir() {
            load_templates_from_dir_into(&resolved, &mut templates);
        } else if resolved.is_file() && resolved.extension().and_then(|e| e.to_str()) == Some("md")
        {
            if let Some(tmpl) = load_template_from_file(&resolved) {
                templates.push(tmpl);
            }
        }
    }

    templates
}

/// Load templates from a directory into an existing vector.
fn load_templates_from_dir_into(dir: &Path, templates: &mut Vec<PromptTemplate>) {
    if !dir.exists() {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        // For symlinks, check if they point to a file
        let is_file = match std::fs::metadata(&path) {
            Ok(m) => m.is_file(),
            Err(_) => continue,
        };

        if is_file {
            if let Some(tmpl) = load_template_from_file(&path) {
                templates.push(tmpl);
            }
        }
    }
}

/// Load a single template from a file, parsing frontmatter for metadata.
fn load_template_from_file(file_path: &Path) -> Option<PromptTemplate> {
    let raw_content = std::fs::read_to_string(file_path).ok()?;

    let name = file_path
        .file_stem()?
        .to_str()?
        .to_string();

    let (description, argument_hint, body) = parse_template_metadata(&raw_content);

    Some(PromptTemplate::parse_with_meta(
        name,
        body,
        description,
        argument_hint,
        file_path.to_path_buf(),
    ))
}

/// Parse template metadata from raw content.
///
/// If the content starts with YAML frontmatter (`---`), extracts `description`
/// and `argument-hint` fields. The body is the content after the frontmatter.
/// If no frontmatter, the description is derived from the first non-empty line.
fn parse_template_metadata(raw_content: &str) -> (String, Option<String>, String) {
    let mut description = String::new();
    let mut argument_hint = None;
    let body: String;

    if let Some((fields, body_str)) = crate::frontmatter::parse_frontmatter(raw_content) {
        // Extract description from frontmatter
        if let Some(desc_val) = fields.get("description") {
            if let Some(s) = desc_val.as_str() {
                description = s.to_string();
            }
        }

        // Extract argument-hint
        if let Some(hint_val) = fields.get("argument-hint") {
            if let Some(s) = hint_val.as_str() {
                argument_hint = Some(s.to_string());
            }
        }

        body = body_str.to_string();
    } else {
        body = raw_content.to_string();
    }

    // If no description from frontmatter, use first non-empty line
    if description.is_empty() {
        for line in body.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                description = if trimmed.len() > 60 {
                    format!("{}...", &trimmed[..60])
                } else {
                    trimmed.to_string()
                };
                break;
            }
        }
    }

    (description, argument_hint, body)
}

/// Normalize a path string, expanding `~` to the home directory.
fn normalize_path_string(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(trimmed));
    }
    if let Some(rest) = trimmed.strip_prefix('~') {
        return dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(trimmed));
    }
    PathBuf::from(trimmed)
}

/// Resolve a prompt path relative to `cwd`, normalizing `~`.
fn resolve_prompt_path(p: &str, cwd: &Path) -> PathBuf {
    let normalized = normalize_path_string(p);
    if normalized.is_absolute() {
        normalized
    } else {
        cwd.join(normalized)
    }
}

// ---------------------------------------------------------------------------
// Expand slash-command style template references
// ---------------------------------------------------------------------------

/// Expand a prompt template if the text starts with `/template-name`.
///
/// If `text` starts with `/`, the first word (sans leading `/`) is treated as
/// a template name. The remainder of the text is parsed as arguments.
///
/// Returns the expanded content with argument substitution, or the original
/// text if no matching template is found.
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
    if !text.starts_with('/') {
        return text.to_string();
    }

    let space_index = text.find(' ');
    let template_name = match space_index {
        Some(si) => &text[1..si],
        None => &text[1..],
    };
    let args_string = match space_index {
        Some(si) => &text[si + 1..],
        None => "",
    };

    if let Some(template) = templates.iter().find(|t| t.name == template_name) {
        let args = parse_command_args(args_string);
        substitute_args(&template.content, &args)
    } else {
        text.to_string()
    }
}

// ---------------------------------------------------------------------------
// Built-in templates (system prompts, skill templates)
// ---------------------------------------------------------------------------

lazy_static::lazy_static! {
    /// Built-in templates available without loading from disk.
    pub static ref BUILTIN_TEMPLATES: Vec<PromptTemplate> = {
        let mut templates = Vec::new();

        // System prompt template
        templates.push(PromptTemplate::parse_with_meta(
            "system".to_string(),
            include_str!("templates/system.md").to_string(),
            "Default system prompt".to_string(),
            None,
            PathBuf::new(),
        ));

        templates
    };
}

/// Register built-in templates into a [`TemplateManager`].
pub fn register_builtin_templates(manager: &mut TemplateManager) {
    for tmpl in BUILTIN_TEMPLATES.iter() {
        manager.insert(tmpl.clone());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    fn test_extract_variables_skip_helpers() {
        let vars = extract_variables("{{date}} {{time}} {{cwd}} {{name}}");
        assert_eq!(vars, vec!["name"]);
    }

    #[test]
    fn test_extract_variables_skip_env() {
        let vars = extract_variables("{{env HOME}} {{name}}");
        assert_eq!(vars, vec!["name"]);
    }

    #[test]
    fn test_render_template_basic() {
        let vars = HashMap::new();
        let context = RenderContext::new(&vars);
        let result = context.render("Hello world!").unwrap();
        assert_eq!(result, "Hello world!");
    }

    #[test]
    fn test_render_template_with_vars() {
        let mut vars = HashMap::new();
        vars.insert("name", "world");
        vars.insert("lang", "Rust");

        let context = RenderContext::new(&vars);
        let result = context.render("Hello {{name}}, write {{lang}}!").unwrap();
        assert_eq!(result, "Hello world, write Rust!");
    }

    #[test]
    fn test_render_template_no_match() {
        let vars = HashMap::new();
        let context = RenderContext::new(&vars);
        let result = context.render("No {{vars}} to replace").unwrap();
        assert_eq!(result, "No {{vars}} to replace");
    }

    #[test]
    fn test_render_helpers() {
        let vars = HashMap::new();
        let context = RenderContext::new(&vars);

        // date and time should produce non-empty strings
        let result = context.render("{{date}} {{time}}").unwrap();
        assert!(!result.is_empty());
        assert!(!result.contains("{{"));
    }

    #[test]
    fn test_render_cwd() {
        let vars = HashMap::new();
        let context = RenderContext::new(&vars);
        let result = context.render("{{cwd}}").unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_render_env() {
        let vars = HashMap::new();
        let context = RenderContext::new(&vars);

        // Set a known env var
        env::set_var("OXI_TEST_TEMPLATE_VAR", "hello123");
        let result = context.render("{{env OXI_TEST_TEMPLATE_VAR}}").unwrap();
        assert_eq!(result, "hello123");
        env::remove_var("OXI_TEST_TEMPLATE_VAR");
    }

    #[test]
    fn test_render_env_colon_syntax() {
        let vars = HashMap::new();
        let context = RenderContext::new(&vars);

        env::set_var("OXI_TEST_COLON_VAR", "world456");
        let result = context.render("{{env::OXI_TEST_COLON_VAR}}").unwrap();
        assert_eq!(result, "world456");
        env::remove_var("OXI_TEST_COLON_VAR");
    }

    #[test]
    fn test_render_conditional_true() {
        let mut vars = HashMap::new();
        vars.insert("feature", "enabled");

        let context = RenderContext::new(&vars);
        let result = context
            .render("{{#if feature}}Feature is on{{/if}}")
            .unwrap();
        assert_eq!(result, "Feature is on");
    }

    #[test]
    fn test_render_conditional_false() {
        let mut vars = HashMap::new();
        vars.insert("feature", "false");

        let context = RenderContext::new(&vars);
        let result = context
            .render("{{#if feature}}Feature is on{{/if}}")
            .unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_render_conditional_else() {
        let vars = HashMap::new();
        let context = RenderContext::new(&vars);
        let result = context
            .render("{{#if feature}}yes{{else}}no{{/if}}")
            .unwrap();
        assert_eq!(result, "no");
    }

    #[test]
    fn test_render_conditional_else_true() {
        let mut vars = HashMap::new();
        vars.insert("feature", "yes");

        let context = RenderContext::new(&vars);
        let result = context
            .render("{{#if feature}}yes{{else}}no{{/if}}")
            .unwrap();
        assert_eq!(result, "yes");
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
    fn test_prompt_template_render_with_args() {
        let tmpl = PromptTemplate::parse(
            "greet".to_string(),
            "Hello $1, your role is $2.".to_string(),
        );
        let args = vec!["Alice".to_string(), "admin".to_string()];
        let result = tmpl.render_with_args(&args).unwrap();
        assert_eq!(result, "Hello Alice, your role is admin.");
    }

    #[test]
    fn test_substitute_args_positional() {
        let args = vec!["hello".to_string(), "world".to_string()];
        let result = substitute_args("$1 $2", &args);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_substitute_args_all() {
        let args = vec!["hello".to_string(), "world".to_string()];
        let result = substitute_args("$@", &args);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_substitute_args_arguments_keyword() {
        let args = vec!["hello".to_string(), "world".to_string()];
        let result = substitute_args("$ARGUMENTS", &args);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_substitute_args_slice() {
        let args = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let result = substitute_args("${@:2}", &args);
        assert_eq!(result, "b c d");
    }

    #[test]
    fn test_substitute_args_slice_with_length() {
        let args = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let result = substitute_args("${@:2:2}", &args);
        assert_eq!(result, "b c");
    }

    #[test]
    fn test_parse_command_args_basic() {
        let args = parse_command_args("hello world");
        assert_eq!(args, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_command_args_quoted() {
        let args = parse_command_args("hello \"world foo\" bar");
        assert_eq!(args, vec!["hello", "world foo", "bar"]);
    }

    #[test]
    fn test_parse_command_args_single_quoted() {
        let args = parse_command_args("hello 'world bar' baz");
        assert_eq!(args, vec!["hello", "world bar", "baz"]);
    }

    #[test]
    fn test_parse_command_args_empty() {
        let args = parse_command_args("");
        assert!(args.is_empty());
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
        mgr.insert(tmpl);

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

    #[test]
    fn test_load_from_file_with_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\ndescription: A test template\nargument-hint: <name>\n---\nHello {{name}}!";
        std::fs::write(dir.path().join("test.md"), content).unwrap();

        let tmpl = load_template_from_file(&dir.path().join("test.md")).unwrap();
        assert_eq!(tmpl.name, "test");
        assert_eq!(tmpl.description, "A test template");
        assert_eq!(tmpl.argument_hint, Some("<name>".to_string()));
        assert_eq!(tmpl.variables, vec!["name"]);
    }

    #[test]
    fn test_load_from_file_no_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.md"), "Hello world!").unwrap();

        let tmpl = load_template_from_file(&dir.path().join("test.md")).unwrap();
        assert_eq!(tmpl.name, "test");
        assert_eq!(tmpl.description, "Hello world!");
    }

    #[test]
    fn test_load_from_file_description_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let long_line = "A".repeat(100);
        std::fs::write(dir.path().join("test.md"), &long_line).unwrap();

        let tmpl = load_template_from_file(&dir.path().join("test.md")).unwrap();
        assert!(tmpl.description.len() <= 63); // 60 chars + "..."
        assert!(tmpl.description.ends_with("..."));
    }

    #[test]
    fn test_expand_prompt_template_match() {
        let templates = vec![PromptTemplate::parse(
            "fix".to_string(),
            "Fix the bug in $1".to_string(),
        )];

        let result = expand_prompt_template("/fix auth module", &templates);
        assert_eq!(result, "Fix the bug in auth module");
    }

    #[test]
    fn test_expand_prompt_template_no_match() {
        let templates: Vec<PromptTemplate> = vec![];
        let result = expand_prompt_template("just plain text", &templates);
        assert_eq!(result, "just plain text");
    }

    #[test]
    fn test_expand_prompt_template_no_slash() {
        let templates = vec![PromptTemplate::parse(
            "fix".to_string(),
            "Fix".to_string(),
        )];
        let result = expand_prompt_template("fix something", &templates);
        assert_eq!(result, "fix something");
    }

    #[test]
    fn test_expand_prompt_template_no_args() {
        let templates = vec![PromptTemplate::parse(
            "review".to_string(),
            "Review the code".to_string(),
        )];

        let result = expand_prompt_template("/review", &templates);
        assert_eq!(result, "Review the code");
    }

    #[test]
    fn test_expand_prompt_template_quoted_args() {
        let templates = vec![PromptTemplate::parse(
            "fix".to_string(),
            "Fix $1: $2".to_string(),
        )];

        let result = expand_prompt_template("/fix auth \"login page\"", &templates);
        assert_eq!(result, "Fix auth: login page");
    }

    #[test]
    fn test_normalize_path_string_home() {
        let p = normalize_path_string("~");
        assert!(p.to_string_lossy().len() > 1);
    }

    #[test]
    fn test_normalize_path_string_home_subpath() {
        let p = normalize_path_string("~/foo/bar");
        assert!(p.to_string_lossy().contains("foo"));
    }

    #[test]
    fn test_resolve_prompt_path_absolute() {
        let p = resolve_prompt_path("/absolute/path", Path::new("/cwd"));
        assert_eq!(p, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_resolve_prompt_path_relative() {
        let p = resolve_prompt_path("relative/path", Path::new("/cwd"));
        assert_eq!(p, PathBuf::from("/cwd/relative/path"));
    }

    #[test]
    fn test_load_prompt_templates_from_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.md"), "Hello {{name}}!").unwrap();

        let options = LoadPromptTemplatesOptions {
            cwd: PathBuf::from("/tmp"),
            agent_dir: None,
            prompt_paths: vec![dir.path().to_string_lossy().to_string()],
            include_defaults: false,
        };

        let templates = load_prompt_templates(&options);
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "hello");
    }
}
