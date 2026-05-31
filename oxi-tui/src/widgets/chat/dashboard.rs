//! DashboardInfo and dashboard rendering helpers.

use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::theme::ThemeStyles;

// ── Dashboard ──────────────────────────────────────────────────────────

/// Information displayed in the welcome dashboard panel.
#[derive(Debug, Clone)]
pub struct DashboardInfo {
    /// oxi version string.
    pub version: String,
    /// Full model identifier (e.g. "anthropic/claude-sonnet-4").
    pub model_id: String,
    /// Thinking level label (e.g. "medium").
    pub thinking_level: String,
    /// Project directory basename.
    pub project_name: String,
    /// Git branch name, if detected.
    pub git_branch: Option<String>,
    /// Path to AGENTS.md, if found.
    pub agents_md_path: Option<String>,
    /// Registered tool names.
    pub tool_names: Vec<String>,
    /// Loaded skill names.
    pub skill_names: Vec<String>,
}

/// Count how many lines badges will occupy given the available width.
pub(crate) fn badge_line_count(names: &[String], width: usize) -> usize {
    if names.is_empty() {
        return 0;
    }
    let prefix = 2; // "  " indent
    let mut lines = 1;
    let mut current_width = prefix;

    for name in names {
        // [name] = name display width + 2 brackets + 1 space
        let badge_w = UnicodeWidthStr::width(name.as_str()) + 3;
        if current_width + badge_w > width && current_width > prefix {
            lines += 1;
            current_width = prefix + badge_w;
        } else {
            current_width += badge_w;
        }
    }
    lines
}

/// Build styled badge Lines that wrap at the given width.
pub(crate) fn compute_badge_lines(
    names: &[String],
    width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    if names.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let prefix = 2;
    let mut current_spans: Vec<Span<'static>> = vec![Span::raw("  ")];
    let mut current_width = prefix;

    for name in names {
        let badge_text = format!("[{}] ", name);
        let badge_w = UnicodeWidthStr::width(badge_text.as_str());

        if current_width + badge_w > width && current_width > prefix {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
            current_spans = vec![Span::raw("  ")];
            current_width = prefix;
        }

        current_spans.push(Span::styled(badge_text, styles.primary));
        current_width += badge_w;
    }

    if current_spans.len() > 1 {
        lines.push(Line::from(current_spans));
    }

    lines
}

/// Shorten a path for display (replace home dir with ~).
fn shorten_dashboard_path(path: &str) -> String {
    crate::widgets::tool_renderer::shorten_path(path)
}

/// Measure the dashboard panel height.
pub(crate) fn measure_dashboard(info: &DashboardInfo, width: u16) -> u16 {
    let w = width as usize;
    let mut h: usize = 0;

    h += 1; // version header
    h += 1; // empty line
    h += 1; // model
    h += 1; // project
    h += 1; // agents.md
    h += 1; // empty line

    if !info.tool_names.is_empty() {
        h += 1; // tools header
        h += badge_line_count(&info.tool_names, w);
    }

    h += 1; // empty line

    if !info.skill_names.is_empty() {
        h += 1; // skills header
        h += badge_line_count(&info.skill_names, w);
        h += 1; // empty line
    }

    h += 1; // shortcuts

    h as u16
}

/// Build all dashboard content lines for rendering.
pub(crate) fn dashboard_lines(
    info: &DashboardInfo,
    width: u16,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut lines = Vec::new();

    // Version header
    lines.push(Line::from(vec![
        Span::styled("  \u{25C8} ", styles.accent),
        Span::styled(
            format!("oxi v{}", info.version),
            styles.accent.add_modifier(Modifier::BOLD),
        ),
    ]));

    // Empty line
    lines.push(Line::raw(""));

    // Model info
    let model_display = info
        .model_id
        .split('/')
        .next_back()
        .unwrap_or(&info.model_id);
    let provider = info.model_id.split('/').next().unwrap_or("");

    let mut model_spans = vec![
        Span::raw("  "),
        Span::styled("Model     ", styles.muted),
        Span::styled(format!("{} ({})", model_display, provider), styles.normal),
    ];
    if !info.thinking_level.is_empty() {
        model_spans.push(Span::styled(" \u{00B7} ", styles.muted));
        model_spans.push(Span::styled("Thinking: ", styles.muted));
        model_spans.push(Span::styled(info.thinking_level.clone(), styles.warning));
    }
    lines.push(Line::from(model_spans));

    // Project
    let project = if let Some(ref branch) = info.git_branch {
        format!("{} ({})", info.project_name, branch)
    } else {
        info.project_name.clone()
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Project   ", styles.muted),
        Span::styled(project, styles.normal),
    ]));

    // AGENTS.md
    let agents = match &info.agents_md_path {
        Some(p) => shorten_dashboard_path(p),
        None => "not found".to_string(),
    };
    let agents_style = if info.agents_md_path.is_some() {
        styles.success
    } else {
        styles.muted
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("AGENTS.md ", styles.muted),
        Span::styled(agents, agents_style),
    ]));

    // Empty line
    lines.push(Line::raw(""));

    // Tools section header
    if !info.tool_names.is_empty() {
        let header_text = format!("  \u{2500}\u{2500} Tools ({}) ", info.tool_names.len());
        let header_w = UnicodeWidthStr::width(header_text.as_str());
        let sep_remain = w.saturating_sub(header_w);
        let header_full = format!("{}{}", header_text, "\u{2500}".repeat(sep_remain));
        lines.push(Line::from(Span::styled(header_full, styles.border)));

        let badges = compute_badge_lines(&info.tool_names, w, styles);
        lines.extend(badges);
    }

    // Empty line
    lines.push(Line::raw(""));

    // Skills section
    if !info.skill_names.is_empty() {
        let header_text = format!("  \u{2500}\u{2500} Skills ({}) ", info.skill_names.len());
        let header_w = UnicodeWidthStr::width(header_text.as_str());
        let sep_remain = w.saturating_sub(header_w);
        let header_full = format!("{}{}", header_text, "\u{2500}".repeat(sep_remain));
        lines.push(Line::from(Span::styled(header_full, styles.border)));

        let badges = compute_badge_lines(&info.skill_names, w, styles);
        lines.extend(badges);

        lines.push(Line::raw(""));
    }

    // Shortcuts
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Enter ", styles.primary),
        Span::styled("send  \u{00B7}  ", styles.muted),
        Span::styled("/help ", styles.primary),
        Span::styled("commands  \u{00B7}  ", styles.muted),
        Span::styled("Ctrl+C ", styles.primary),
        Span::styled("interrupt", styles.muted),
    ]));

    lines
}
