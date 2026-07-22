//! Welcome banner formatting with environment info.

use oxi_tui_legacy::widgets::chat::DashboardInfo;
use std::path::Path;

/// Information to display in the welcome banner.
pub(crate) struct WelcomeInfo {
    /// Model identifier (e.g. "anthropic/claude-sonnet-4").
    pub model_id: String,
    /// Thinking level label.
    pub thinking_level: String,
    /// Tool names currently registered.
    #[allow(dead_code)]
    pub tool_names: Vec<String>,
    /// Tool labels: name -> short label.
    pub tool_labels: Vec<(String, String)>,
    /// Skill names loaded from ~/.oxi/skills/.
    pub skill_names: Vec<String>,
    /// Whether AGENTS.md was found and where.
    pub agents_md_path: Option<String>,
    /// Session type: "new" or "resumed".
    #[allow(dead_code)]
    pub session_type: &'static str,
    /// Git branch, if detected.
    pub git_branch: Option<String>,
    /// Project directory basename.
    pub project_name: String,
}

/// Build a `DashboardInfo` from `WelcomeInfo` for the new dashboard panel.
pub(crate) fn build_dashboard_info(info: &WelcomeInfo) -> DashboardInfo {
    DashboardInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        model_id: info.model_id.clone(),
        thinking_level: info.thinking_level.clone(),
        project_name: info.project_name.clone(),
        git_branch: info.git_branch.clone(),
        agents_md_path: info.agents_md_path.clone(),
        tool_names: info.tool_labels.iter().map(|(n, _)| n.clone()).collect(),
        skill_names: info.skill_names.clone(),
    }
}

/// Format a rich startup welcome message (legacy markdown fallback).
///
/// Shows the current environment: model, tools, skills, AGENTS.md, and key shortcuts.
/// Markdown formatting is used so the chat renderer displays it nicely.
#[allow(dead_code)]
pub(crate) fn format_welcome(info: &WelcomeInfo) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut md = String::new();

    // Header
    md.push_str(&format!("## oxi v{}\n", version));
    md.push('\n');

    // Model + thinking
    if !info.model_id.is_empty() {
        let model_display = info
            .model_id
            .split('/')
            .next_back()
            .unwrap_or(&info.model_id);
        let provider = info.model_id.split('/').next().unwrap_or("");
        md.push_str(&format!("**Model**: {} ({})", model_display, provider));
        if !info.thinking_level.is_empty() {
            md.push_str(&format!(" · Thinking: {}", info.thinking_level));
        }
        md.push('\n');
    }

    // Project + Git
    md.push_str(&format!("**Project**: {}", info.project_name));
    if let Some(ref branch) = info.git_branch {
        md.push_str(&format!(" (`{}`)", branch));
    }
    md.push('\n');
    md.push('\n');

    // AGENTS.md status
    if let Some(ref path) = info.agents_md_path {
        md.push_str(&format!("- **AGENTS.md**: {} found\n", shorten(path)));
    } else {
        md.push_str("- **AGENTS.md**: not found\n");
    }

    // Tools
    if !info.tool_labels.is_empty() {
        md.push_str(&format!("- **Tools** ({}): ", info.tool_labels.len()));
        let names: Vec<String> = info
            .tool_labels
            .iter()
            .map(|(name, _label)| format!("`{}`", name))
            .collect();
        md.push_str(&names.join(" "));
        md.push('\n');
    }

    // Skills
    if !info.skill_names.is_empty() {
        md.push_str(&format!("- **Skills** ({}): ", info.skill_names.len()));
        md.push_str(
            &info
                .skill_names
                .iter()
                .map(|s| format!("`{}`", s))
                .collect::<Vec<_>>()
                .join(" "),
        );
        md.push('\n');
    }

    // Shortcuts
    md.push('\n');
    md.push_str("Enter send · /help commands · Ctrl+C interrupt · Shift+Tab thinking\n");

    md
}

/// Shorten a path for display.
fn shorten(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

/// Detect AGENTS.md in the project directory or its parents.
pub(crate) fn detect_agents_md(cwd: &Path) -> Option<String> {
    let candidates = [".oxi/AGENTS.md", "AGENTS.md"];
    let mut dir = cwd;
    loop {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if path.exists() {
                return Some(path.to_string_lossy().into_owned());
            }
        }
        dir = dir.parent()?;
    }
}
