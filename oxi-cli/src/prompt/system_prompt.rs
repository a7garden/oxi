//! System prompt construction and project context loading.
//!
//! Originally inspired by pi-mono's system prompt construction.

use crate::store::settings::ThinkingLevel;
use chrono::Local;

/// A skill that can be included in the system prompt.
#[derive(Debug, Clone)]
pub struct Skill {
    /// pub.
    pub name: String,
    /// pub.
    pub content: String,
}

/// A pre-loaded context file.
#[derive(Debug, Clone)]
pub struct ContextFile {
    /// pub.
    pub path: String,
    /// pub.
    pub content: String,
}

/// Options for building the system prompt.
#[derive(Debug, Clone)]
pub struct BuildSystemPromptOptions {
    /// Custom system prompt (replaces default).
    pub custom_prompt: Option<String>,
    /// Tools to include in prompt. Default: ["read", "bash", "edit", "write"].
    pub selected_tools: Vec<String>,
    /// Optional one-line tool snippets keyed by tool name.
    pub tool_snippets: std::collections::HashMap<String, String>,
    /// Additional guideline bullets appended to the default system prompt guidelines.
    pub prompt_guidelines: Vec<String>,
    /// Text to append to system prompt.
    pub append_system_prompt: Option<String>,
    /// Working directory.
    pub cwd: String,
    /// Pre-loaded context files.
    pub context_files: Vec<ContextFile>,
    /// Pre-loaded skills.
    pub skills: Vec<Skill>,
    /// Path to README documentation.
    pub readme_path: Option<String>,
    /// Path to additional docs.
    pub docs_path: Option<String>,
    /// Path to examples.
    pub examples_path: Option<String>,
}

/// Convert a [`ThinkingLevel`] to its default custom prompt string.
///
/// This is the single source of truth for the thinking-level-to-prompt mapping.
pub fn thinking_level_prompt(level: ThinkingLevel) -> Option<String> {
    match level {
        ThinkingLevel::Off => {
            Some("You are a helpful AI assistant. Provide direct, concise answers.".into())
        }
        ThinkingLevel::Minimal => {
            Some("You are a helpful AI assistant. Provide clear and helpful answers.".into())
        }
        ThinkingLevel::Low => {
            Some("You are a helpful AI assistant. Provide brief, actionable responses.".into())
        }
        ThinkingLevel::Medium => Some(
            "You are a helpful AI coding assistant. Think through problems \
             step by step when helpful, but keep responses focused and actionable."
                .into(),
        ),
        ThinkingLevel::High => Some(
            "You are an expert AI coding assistant. Take time to thoroughly \
             analyze problems, consider edge cases, and provide comprehensive \
             solutions with explanations. Think deeply before responding."
                .into(),
        ),
        ThinkingLevel::XHigh => Some(
            "You are an expert AI coding assistant. Use maximum reasoning depth. \
             Consider all alternatives, edge cases, and potential implications. \
             Provide the most thorough, comprehensive analysis possible."
                .into(),
        ),
    }
}

pub fn default_tool_snippets() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    m.insert("read".into(), "Read file contents (text or image)".into());
    m.insert("bash".into(), "Execute bash commands".into());
    m.insert(
        "edit".into(),
        "Edit files with line-anchored hashline patches (see format below)".into(),
    );
    m.insert("write".into(), "Write content to files".into());
    m.insert("grep".into(), "Search file contents with regex".into());
    m.insert("find".into(), "Find files by name/pattern".into());
    m.insert("ls".into(), "List directory contents".into());
    m.insert(
        "web_search".into(),
        "Search the web (DuckDuckGo, Wikipedia, Bing)".into(),
    );
    m
}

/// Default tool names used when building prompts for the agent loop.
pub fn default_tool_names() -> Vec<String> {
    vec![
        "read".into(),
        "bash".into(),
        "edit".into(),
        "write".into(),
        "grep".into(),
        "find".into(),
        "ls".into(),
        "web_search".into(),
    ]
}

impl Default for BuildSystemPromptOptions {
    fn default() -> Self {
        Self {
            custom_prompt: None,
            selected_tools: vec!["read".into(), "bash".into(), "edit".into(), "write".into()],
            tool_snippets: std::collections::HashMap::new(),
            prompt_guidelines: Vec::new(),
            append_system_prompt: None,
            cwd: String::new(),
            context_files: Vec::new(),
            skills: Vec::new(),
            readme_path: None,
            docs_path: None,
            examples_path: None,
        }
    }
}

/// Format skills for inclusion in the system prompt.
fn format_skills_for_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n# Skills\n\n");
    for skill in skills {
        out.push_str(&format!("## {}\n\n{}\n\n", skill.name, skill.content));
    }
    out
}

pub fn build_system_prompt(options: &BuildSystemPromptOptions) -> String {
    let prompt_cwd = options.cwd.replace('\\', "/");
    let date = Local::now().format("%Y-%m-%d").to_string();

    let append_section = options
        .append_system_prompt
        .as_deref()
        .map(|s| format!("\n\n{}", s))
        .unwrap_or_default();

    // If a custom prompt is provided, use it as the base
    if let Some(ref custom) = options.custom_prompt {
        let mut prompt = custom.clone();

        prompt.push_str(&append_section);

        // Append project context files
        if !options.context_files.is_empty() {
            prompt.push_str("\n\n# Project Context\n\n");
            prompt.push_str("Project-specific instructions and guidelines:\n\n");
            for cf in &options.context_files {
                prompt.push_str(&format!("## {}\n\n{}\n\n", cf.path, cf.content));
            }
        }

        // Append skills section (only if read tool is available)
        let custom_has_read = options.selected_tools.is_empty()
            || options.selected_tools.contains(&"read".to_string());
        if custom_has_read && !options.skills.is_empty() {
            prompt.push_str(&format_skills_for_prompt(&options.skills));
        }

        // Add date and working directory last
        prompt.push_str(&format!("\nCurrent date: {}", date));
        prompt.push_str(&format!("\nCurrent working directory: {}", prompt_cwd));

        return prompt;
    }

    // Build default prompt
    let readme_path = options
        .readme_path
        .as_deref()
        .unwrap_or("(docs not available)");
    let docs_path = options
        .docs_path
        .as_deref()
        .unwrap_or("(docs not available)");
    let examples_path = options
        .examples_path
        .as_deref()
        .unwrap_or("(examples not available)");

    // Build tools list — a tool appears in Available tools only when a snippet is provided
    let visible_tools: Vec<&str> = options
        .selected_tools
        .iter()
        .filter(|name| options.tool_snippets.contains_key(name.as_str()))
        .map(|s| s.as_str())
        .collect();
    let tools_list = if visible_tools.is_empty() {
        "(none)".to_string()
    } else {
        visible_tools
            .iter()
            .map(|name| {
                let snippet = options
                    .tool_snippets
                    .get(*name)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                format!("- {}: {}", name, snippet)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Build guidelines based on which tools are actually available
    let mut guidelines: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add_guideline = |g: &str| {
        if seen.insert(g.to_string()) {
            guidelines.push(g.to_string());
        }
    };

    let has_bash = options.selected_tools.contains(&"bash".to_string());
    let has_grep = options.selected_tools.contains(&"grep".to_string());
    let has_find = options.selected_tools.contains(&"find".to_string());
    let has_ls = options.selected_tools.contains(&"ls".to_string());
    let has_read = options.selected_tools.contains(&"read".to_string());

    // File exploration guidelines
    if has_bash && !has_grep && !has_find && !has_ls {
        add_guideline("Use bash for file operations like ls, rg, find");
    } else if has_bash && (has_grep || has_find || has_ls) {
        add_guideline(
            "Prefer grep/find/ls tools over bash for file exploration (faster, respects .gitignore)",
        );
    }

    // User-provided guidelines
    for g in &options.prompt_guidelines {
        let trimmed = g.trim();
        if !trimmed.is_empty() {
            add_guideline(trimmed);
        }
    }

    // Always include these
    add_guideline("Be concise in your responses");
    add_guideline("Show file paths clearly when working with files");

    let guidelines_text = guidelines
        .iter()
        .map(|g| format!("- {}", g))
        .collect::<Vec<_>>()
        .join("\n");

    let mut prompt = format!(
        include_str!("../prompts/identity.md"),
        tools_list, guidelines_text, readme_path, docs_path, examples_path,
    );

    prompt.push_str(&append_section);

    // ── Hashline format specification (from oxi-hashline canonical source) ──
    prompt.push_str(include_str!("../prompts/hashline_format.md"));

    // Append project context files
    if !options.context_files.is_empty() {
        prompt.push_str("\n\n# Project Context\n\n");
        prompt.push_str("Project-specific instructions and guidelines:\n\n");
        for cf in &options.context_files {
            prompt.push_str(&format!("## {}\n\n{}\n\n", cf.path, cf.content));
        }
    }

    // Append skills section (only if read tool is available)
    if has_read && !options.skills.is_empty() {
        prompt.push_str(&format_skills_for_prompt(&options.skills));
    }

    // Add date and working directory last
    prompt.push_str(&format!("\nCurrent date: {}", date));
    prompt.push_str(&format!("\nCurrent working directory: {}", prompt_cwd));

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prompt_contains_key_sections() {
        let opts = BuildSystemPromptOptions {
            cwd: "/home/user/project".into(),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("expert coding assistant"));
        assert!(prompt.contains("Available tools:"));
        assert!(prompt.contains("Guidelines:"));
        assert!(prompt.contains("Be concise"));
        assert!(prompt.contains("Current working directory: /home/user/project"));
        assert!(prompt.contains("Current date:"));
    }

    #[test]
    fn custom_prompt_used_as_base() {
        let opts = BuildSystemPromptOptions {
            custom_prompt: Some("Custom prompt here.".into()),
            cwd: "/tmp".into(),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.starts_with("Custom prompt here."));
        assert!(prompt.contains("Current working directory: /tmp"));
    }

    #[test]
    fn context_files_appended() {
        let opts = BuildSystemPromptOptions {
            custom_prompt: Some("Base".into()),
            cwd: "/tmp".into(),
            context_files: vec![ContextFile {
                path: "STYLE.md".into(),
                content: "Use 4-space indent".into(),
            }],
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Project Context"));
        assert!(prompt.contains("STYLE.md"));
        assert!(prompt.contains("Use 4-space indent"));
    }

    #[test]
    fn append_section_included() {
        let opts = BuildSystemPromptOptions {
            append_system_prompt: Some("Extra rules".into()),
            cwd: "/tmp".into(),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Extra rules"));
    }
}
