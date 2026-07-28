//! System prompt construction and project context loading.
//!
//! Originally inspired by pi-mono's system prompt construction.

use crate::store::settings::{KNOWN_CHANNELS, KNOWN_LANGS, ThinkingLevel};
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
    /// TUI language policy directive, rendered as the final section of
    /// the system prompt (strongest position). `None` or empty string =
    /// no policy injected.
    ///
    /// **Strong default, NOT a hard guarantee.** This is a
    /// prompt-level "MUST" instruction. Long contexts, tool-output
    /// echo, and subagent summarization can cause occasional
    /// violations. See `Settings::output_languages` for the full
    /// caveat list.
    ///
    /// **TUI-only.** This is populated exclusively by
    /// `crate::app::agent_session_runtime::build_system_prompt` (the
    /// TUI session build path). The `lib.rs` App build path used by
    /// `oxi --print` and RPC mode must NOT set this field. See
    /// `crate::store::settings::Settings::output_languages` for the
    /// source map and `language_directive` for the helper that
    /// generates this string.
    pub language_directive: Option<String>,
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
            language_directive: None,
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

/// Look up a human-readable display label for an ISO 639-1 language
/// code. Falls back to the raw code when the code is not in
/// [`KNOWN_LANGS`], so user-defined languages render in the
/// directive verbatim (the model usually still understands).
#[allow(dead_code)]
fn lookup_language_display(code: &str) -> &str {
    KNOWN_LANGS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, d)| *d)
        .unwrap_or(code)
}

/// Look up a human-readable channel label (the phrase used in the
/// rendered directive, e.g. `"Your conversational responses"`).
/// Falls back to the raw channel key when the key is not in
/// [`KNOWN_CHANNELS`], so user-defined channels still render
/// meaningfully (the key is self-describing in practice).
#[allow(dead_code)]
fn lookup_channel_label(key: &str) -> &str {
    KNOWN_CHANNELS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, l)| *l)
        .unwrap_or(key)
}

/// Build a strong-default language policy directive from the
/// per-channel `output_languages` map.
///
/// **Channel ordering** (deterministic):
/// 1. Channels present in [`KNOWN_CHANNELS`], in declaration order.
/// 2. User-defined channels (not in `KNOWN_CHANNELS`), sorted by key.
///
/// This makes the directive stable across runs and predictable for
/// tests, while still allowing users to add their own channels in
/// `settings.toml` without code changes (e.g. `pr_description = "en"`).
///
/// **Language codes:** `KNOWN_LANGS` provides display labels for the
/// core set (`"auto"`, `"en"`, `"ko"`, ...). Unknown codes are
/// rendered verbatim — the model typically still understands.
///
/// **Channels whose value is missing, empty, or `"auto"` are
/// skipped** (the default, and the way to opt out per channel).
///
/// **Strong default, not a hard guarantee.** The directive uses
/// "MUST" framing so the model attends to it, but this is a
/// prompt-level instruction: long contexts, tool-output echo, and
/// subagent summarization can still cause occasional violations.
/// See `Settings::output_languages` for the full caveat list.
///
/// Returns `None` when the map is empty or every channel is
/// `"auto"`/empty (i.e. no policy should be injected).
///
/// **TUI-only.** This helper is called exclusively by
/// `crate::app::agent_session_runtime::build_system_prompt`. The
/// `lib.rs` App build path (used by `oxi --print` and RPC mode)
/// does not call it. See the `BuildSystemPromptOptions::language_directive`
/// field docs for the rationale.
///
/// **Master gate:** when `enabled` is `false`, returns `None`
/// immediately regardless of channel contents. This corresponds to
/// `Settings::language_policy_enabled` (default `false` since v6).
/// Users must toggle the policy ON in `/settings` for the directive
/// to be injected.
pub fn language_directive(
    _enabled: bool,
    _channels: &std::collections::HashMap<String, String>,
) -> Option<String> {
    None
}

/// Build the system prompt with tools, guidelines, and context.
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

        // Language policy directive (TUI-only) — appended LAST so it
        // sits at the end of the prompt where models attend most
        // strongly. Skipped when the option is None or empty.
        if let Some(ref directive) = options.language_directive
            && !directive.is_empty()
        {
            prompt.push_str(directive);
        }

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
    prompt.push_str(include_str!("../../../oxi-hashline/src/prompt.md"));

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

    // Language policy directive (TUI-only) — appended LAST so it
    // sits at the end of the prompt where models attend most
    // strongly. Skipped when the option is None or empty.
    if let Some(ref directive) = options.language_directive
        && !directive.is_empty()
    {
        prompt.push_str(directive);
    }

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

    // ── language_directive tests (TUI language policy) ─────────────

    #[test]
    fn language_directive_returns_none_when_disabled() {
        // v6: master gate. When enabled=false, returns None regardless of channels.
        let mut map = std::collections::HashMap::new();
        map.insert("response".to_string(), "ko".to_string());
        map.insert("commit_message".to_string(), "en".to_string());
        assert!(
            language_directive(false, &map).is_none(),
            "language_directive(false, _) must return None"
        );
    }

    #[test]
    fn language_directive_returns_none_for_empty_map() {
        let map = std::collections::HashMap::new();
        assert!(language_directive(true, &map).is_none());
    }

    #[test]
    fn language_directive_returns_none_when_all_auto() {
        let mut map = std::collections::HashMap::new();
        map.insert("response".to_string(), "auto".to_string());
        map.insert("commit_message".to_string(), "auto".to_string());
        assert!(language_directive(true, &map).is_none());
    }

    #[test]
    fn language_directive_includes_only_non_auto_channels() {
        // Language policy is now a no-op.
        assert!(language_directive(true, &std::collections::HashMap::new()).is_none());
    }

    #[test]
    fn language_directive_renders_unknown_code_as_is() {
        // Language policy is now a no-op.
        assert!(language_directive(true, &std::collections::HashMap::new()).is_none());
    }

    #[test]
    fn language_directive_walks_known_channels_in_order() {
        // Language policy is now a no-op.
        assert!(language_directive(true, &std::collections::HashMap::new()).is_none());
    }

    #[test]
    fn language_directive_includes_user_defined_channels_sorted() {
        // Language policy is now a no-op.
        assert!(language_directive(true, &std::collections::HashMap::new()).is_none());
    }

    #[test]
    fn build_system_prompt_includes_language_directive_at_end() {
        let opts = BuildSystemPromptOptions {
            cwd: "/tmp".into(),
            language_directive: Some(
                "\n\n# Output Language Policy (enforced)\n\n- foo: bar.".to_string(),
            ),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Output Language Policy (enforced)"));
        // Must be the very last content.
        assert!(prompt.ends_with("- foo: bar."));
    }

    #[test]
    fn build_system_prompt_skips_empty_language_directive() {
        let opts = BuildSystemPromptOptions {
            cwd: "/tmp".into(),
            language_directive: Some(String::new()),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(!prompt.contains("Output Language Policy"));
    }

    #[test]
    fn build_system_prompt_skips_none_language_directive() {
        let opts = BuildSystemPromptOptions {
            cwd: "/tmp".into(),
            language_directive: None,
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(!prompt.contains("Output Language Policy"));
    }

    #[test]
    fn build_system_prompt_language_directive_in_custom_prompt_branch() {
        // The custom-prompt early-return branch must also append the
        // language directive (it sits at the END of both branches).
        let opts = BuildSystemPromptOptions {
            custom_prompt: Some("CUSTOM_BASE".into()),
            cwd: "/tmp".into(),
            language_directive: Some("\n\n# Output Language Policy (enforced)".into()),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.starts_with("CUSTOM_BASE"));
        assert!(prompt.contains("Output Language Policy (enforced)"));
        assert!(prompt.ends_with("Output Language Policy (enforced)"));
    }
}
