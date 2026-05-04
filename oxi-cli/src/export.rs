//! Export conversation sessions to standalone HTML files.
//!
//! Produces a self-contained HTML document with embedded CSS and JS that renders
//! a conversation session with:
//! - Color-coded user / assistant / system messages
//! - Markdown → HTML rendering (basic, no external deps)
//! - ANSI escape code → HTML span conversion (full 256-color + true-color support)
//! - Tool calls and results in styled blocks (bash, file ops, search)
//! - Collapsible thinking blocks
//! - Metadata header (model, provider, date, token counts)
//! - Dark theme (default) with light-theme toggle
//! - Syntax highlighting for code blocks (via highlight.js CDN)
//! - Session tree navigation for branched sessions

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Write as FmtWrite;
use uuid::Uuid;

use crate::session::{AgentMessage, SessionEntry, SessionMeta};

// ── Public types ─────────────────────────────────────────────────────

/// Options for HTML export.
#[derive(Debug, Clone)]
pub struct HtmlExportOptions {
    /// Whether to include thinking blocks in the output.
    pub include_thinking: bool,
    /// Whether to include tool call/result blocks in the output.
    pub include_tool_calls: bool,
    /// Whether to use dark theme (default: true).
    pub dark_theme: bool,
    /// Optional custom title for the HTML document.
    pub title: Option<String>,
}

impl Default for HtmlExportOptions {
    fn default() -> Self {
        Self {
            include_thinking: true,
            include_tool_calls: true,
            dark_theme: true,
            title: None,
        }
    }
}

/// Metadata attached to an export (optional but encouraged).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMeta {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub exported_at: i64,
    pub total_user_tokens: Option<u64>,
    pub total_assistant_tokens: Option<u64>,
}

impl Default for ExportMeta {
    fn default() -> Self {
        Self {
            model: None,
            provider: None,
            exported_at: Utc::now().timestamp_millis(),
            total_user_tokens: None,
            total_assistant_tokens: None,
        }
    }
}

/// A single rendered node in the session tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub session_id: Uuid,
    pub name: Option<String>,
    pub is_current: bool,
    pub children: Vec<TreeNode>,
}

// ── ANSI-to-HTML conversion ──────────────────────────────────────────

/// Standard ANSI color palette (0–15).
const ANSI_COLORS: [&str; 16] = [
    "#000000", // 0: black
    "#800000", // 1: red
    "#008000", // 2: green
    "#808000", // 3: yellow
    "#000080", // 4: blue
    "#800080", // 5: magenta
    "#008080", // 6: cyan
    "#c0c0c0", // 7: white
    "#808080", // 8: bright black
    "#ff0000", // 9: bright red
    "#00ff00", // 10: bright green
    "#ffff00", // 11: bright yellow
    "#0000ff", // 12: bright blue
    "#ff00ff", // 13: bright magenta
    "#00ffff", // 14: bright cyan
    "#ffffff", // 15: bright white
];

/// Convert a 256-color index to a hex color string.
fn color_256_to_hex(index: u8) -> String {
    let idx = index as usize;

    // Standard colors (0–15)
    if idx < 16 {
        return ANSI_COLORS[idx].to_string();
    }

    // Color cube (16–231): 6×6×6 = 216 colors
    if idx < 232 {
        let cube = idx - 16;
        let r = cube / 36;
        let g = (cube % 36) / 6;
        let b = cube % 6;
        let component = |n: usize| -> u8 {
            if n == 0 {
                0
            } else {
                (55 + n * 40) as u8
            }
        };
        return format!(
            "#{:02x}{:02x}{:02x}",
            component(r),
            component(g),
            component(b)
        );
    }

    // Grayscale (232–255): 24 shades
    let gray = 8 + (idx - 232) * 10;
    format!("#{gray:02x}{gray:02x}{gray:02x}", gray = gray as u8)
}

/// Tracked text style state during ANSI SGR processing.
#[derive(Clone, Default)]
struct TextStyle {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

impl TextStyle {
    fn to_inline_css(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ref fg) = self.fg {
            parts.push(format!("color:{fg}"));
        }
        if let Some(ref bg) = self.bg {
            parts.push(format!("background-color:{bg}"));
        }
        if self.bold {
            parts.push("font-weight:bold".to_string());
        }
        if self.dim {
            parts.push("opacity:0.6".to_string());
        }
        if self.italic {
            parts.push("font-style:italic".to_string());
        }
        if self.underline {
            parts.push("text-decoration:underline".to_string());
        }
        if self.strikethrough {
            let deco = if self.underline {
                "text-decoration:underline line-through"
            } else {
                "text-decoration:line-through"
            };
            parts.push(deco.to_string());
        }
        parts.join(";")
    }

    fn has_style(&self) -> bool {
        self.fg.is_some()
            || self.bg.is_some()
            || self.bold
            || self.dim
            || self.italic
            || self.underline
            || self.strikethrough
    }

    fn reset(&mut self) {
        self.fg = None;
        self.bg = None;
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.strikethrough = false;
    }
}

/// Apply ANSI SGR (Select Graphic Rendition) parameters to a style.
fn apply_sgr_codes(params: &[u16], style: &mut TextStyle) {
    let mut i = 0;
    while i < params.len() {
        let code = params[i];
        match code {
            0 => {
                style.reset();
            }
            1 => {
                style.bold = true;
            }
            2 => {
                style.dim = true;
            }
            3 => {
                style.italic = true;
            }
            4 => {
                style.underline = true;
            }
            9 => {
                style.strikethrough = true;
            }
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => {
                style.italic = false;
            }
            24 => {
                style.underline = false;
            }
            29 => {
                style.strikethrough = false;
            }
            // Standard foreground colors (30–37)
            30..=37 => {
                style.fg = Some(ANSI_COLORS[(code - 30) as usize].to_string());
            }
            // Extended foreground color (38;5;N or 38;2;R;G;B)
            38 => {
                if i + 1 < params.len() {
                    match params[i + 1] {
                        5 => {
                            // 256-color: 38;5;N
                            if i + 2 < params.len() {
                                style.fg = Some(color_256_to_hex(params[i + 2] as u8));
                                i += 2;
                            }
                        }
                        2 => {
                            // RGB: 38;2;R;G;B
                            if i + 4 < params.len() {
                                let r = params[i + 2];
                                let g = params[i + 3];
                                let b = params[i + 4];
                                style.fg = Some(format!("rgb({r},{g},{b})"));
                                i += 4;
                            }
                        }
                        _ => {}
                    }
                }
            }
            39 => {
                // Default foreground
                style.fg = None;
            }
            // Standard background colors (40–47)
            40..=47 => {
                style.bg = Some(ANSI_COLORS[(code - 40) as usize].to_string());
            }
            // Extended background color (48;5;N or 48;2;R;G;B)
            48 => {
                if i + 1 < params.len() {
                    match params[i + 1] {
                        5 => {
                            if i + 2 < params.len() {
                                style.bg = Some(color_256_to_hex(params[i + 2] as u8));
                                i += 2;
                            }
                        }
                        2 => {
                            if i + 4 < params.len() {
                                let r = params[i + 2];
                                let g = params[i + 3];
                                let b = params[i + 4];
                                style.bg = Some(format!("rgb({r},{g},{b})"));
                                i += 4;
                            }
                        }
                        _ => {}
                    }
                }
            }
            49 => {
                // Default background
                style.bg = None;
            }
            // Bright foreground colors (90–97)
            90..=97 => {
                style.fg = Some(ANSI_COLORS[(code - 90 + 8) as usize].to_string());
            }
            // Bright background colors (100–107)
            100..=107 => {
                style.bg = Some(ANSI_COLORS[(code - 100 + 8) as usize].to_string());
            }
            _ => {
                // Ignore unrecognized SGR codes
            }
        }
        i += 1;
    }
}

/// Convert ANSI-escaped text to HTML with inline styles.
///
/// Supports:
/// - Standard foreground colors (30–37) and bright variants (90–97)
/// - Standard background colors (40–47) and bright variants (100–107)
/// - 256-color palette (38;5;N and 48;5;N)
/// - RGB true color (38;2;R;G;B and 48;2;R;G;B)
/// - Text styles: bold (1), dim (2), italic (3), underline (4), strikethrough (9)
/// - Reset (0)
pub fn ansi_to_html(text: &str) -> String {
    let mut style = TextStyle::default();
    let mut result = String::with_capacity(text.len() * 2);
    let mut last_end = 0;
    let mut in_span = false;

    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    while pos < len {
        // Look for ESC[ ... m
        if bytes[pos] == 0x1b && pos + 1 < len && bytes[pos + 1] == b'[' {
            // Find 'm' terminator
            let seq_start = pos + 2;
            let mut seq_end = seq_start;
            while seq_end < len && bytes[seq_end] != b'm' {
                seq_end += 1;
            }
            if seq_end >= len {
                // No closing 'm', not a valid sequence
                pos += 1;
                continue;
            }

            // Emit text before this sequence
            if pos > last_end {
                result.push_str(&html_escape(&text[last_end..pos]));
            }

            // Close existing span
            if in_span {
                result.push_str("</span>");
                in_span = false;
            }

            // Parse parameters
            let param_str = &text[seq_start..seq_end];
            let params: Vec<u16> = if param_str.is_empty() {
                vec![0]
            } else {
                param_str
                    .split(';')
                    .map(|p| p.parse::<u16>().unwrap_or(0))
                    .collect()
            };

            // Apply SGR codes
            apply_sgr_codes(&params, &mut style);

            // Open new span if styled
            if style.has_style() {
                result.push_str("<span style=\"");
                result.push_str(&style.to_inline_css());
                result.push_str("\">");
                in_span = true;
            }

            last_end = seq_end + 1; // skip past 'm'
            pos = seq_end + 1;
        } else {
            pos += 1;
        }
    }

    // Emit remaining text
    if last_end < len {
        result.push_str(&html_escape(&text[last_end..]));
    }

    // Close any open span
    if in_span {
        result.push_str("</span>");
    }

    result
}

/// Convert an array of ANSI-escaped lines to HTML.
/// Each line is wrapped in a `<div class="ansi-line">` element.
pub fn ansi_lines_to_html(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|line| {
            let rendered = ansi_to_html(line);
            if rendered.is_empty() {
                "<div class=\"ansi-line\">&nbsp;</div>".to_string()
            } else {
                format!("<div class=\"ansi-line\">{rendered}</div>")
            }
        })
        .collect()
}

// ── Tool rendering ───────────────────────────────────────────────────

/// Information about a detected tool operation in message text.
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum ToolOp {
    Bash {
        command: String,
        output: String,
        exit_code: Option<i32>,
    },
    FileRead {
        path: String,
        content: String,
    },
    FileWrite {
        path: String,
        content: String,
    },
    FileEdit {
        path: String,
        old_text: String,
        new_text: String,
    },
    Search {
        query: String,
        results: Vec<String>,
    },
}

/// Detect and render tool operations embedded in assistant message content.
fn render_tool_blocks(content: &str, include_tool_calls: bool) -> Option<String> {
    if !include_tool_calls {
        return None;
    }

    let mut html = String::new();
    let mut found = false;
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        // Detect bash tool call: 🔧 followed by bash command in code block
        if (line.starts_with("🔧 Running bash") || line.starts_with("🔧 bash"))
            && lines.peek().map_or(false, |l| l.starts_with("```"))
        {
            found = true;
            // Consume the ``` line
            let _code_fence = lines.next();
            let mut cmd = String::new();
            let mut output_lines = Vec::new();
            let mut in_output = false;

            // Read command (single line typically)
            if let Some(cmd_line) = lines.next() {
                cmd.push_str(cmd_line);
            }

            // Look for output section
            while let Some(line) = lines.next() {
                if line.starts_with("```") {
                    // End of code block
                    break;
                }
                if line.starts_with("📤") || line.starts_with("result:") {
                    in_output = true;
                    continue;
                }
                if in_output {
                    output_lines.push(line.to_string());
                } else {
                    // Still in command or before result marker
                    cmd.push('\n');
                    cmd.push_str(line);
                }
            }

            html.push_str(&render_bash_tool(&cmd, &output_lines.join("\n")));
            continue;
        }

        // Detect file read: lines with "📄" or "read" + path + code block
        if (line.starts_with("📄 Reading") || line.starts_with("📄 read"))
            && lines.peek().map_or(false, |l| l.starts_with("```"))
        {
            found = true;
            let path = extract_path_from_line(line);
            let _fence = lines.next(); // ```
            let _lang = "";
            let mut content_buf = String::new();
            while let Some(line) = lines.next() {
                if line.starts_with("```") {
                    break;
                }
                content_buf.push_str(line);
                content_buf.push('\n');
            }
            html.push_str(&render_file_read_tool(&path, &content_buf));
            continue;
        }

        // Detect file write: "📝 Writing" or "📝 write" + path + code block
        if (line.starts_with("📝 Writing") || line.starts_with("📝 write"))
            && lines.peek().map_or(false, |l| l.starts_with("```"))
        {
            found = true;
            let path = extract_path_from_line(line);
            let _fence = lines.next();
            let mut content_buf = String::new();
            while let Some(line) = lines.next() {
                if line.starts_with("```") {
                    break;
                }
                content_buf.push_str(line);
                content_buf.push('\n');
            }
            html.push_str(&render_file_write_tool(&path, &content_buf));
            continue;
        }

        // Detect file edit: "✏️ Editing" or "✏️ edit" + path
        if line.starts_with("✏️ Editing") || line.starts_with("✏️ edit") {
            found = true;
            let path = extract_path_from_line(line);
            let mut old_text = String::new();
            let mut new_text = String::new();

            // Consume lines looking for old/new sections
            while let Some(next) = lines.peek() {
                if next.starts_with("🔧") || next.starts_with("📄") || next.starts_with("📝")
                    || next.starts_with("✏️")
                    || next.starts_with("📤")
                {
                    break;
                }
                let l = lines.next().unwrap();
                if l.contains("old:") || l.contains("Old text:") {
                    // Collect old text
                    while let Some(next) = lines.peek() {
                        if next.contains("new:") || next.contains("New text:") {
                            break;
                        }
                        let ol = lines.next().unwrap();
                        if ol.starts_with("```") {
                            continue;
                        }
                        old_text.push_str(ol);
                        old_text.push('\n');
                    }
                } else if l.contains("new:") || l.contains("New text:") {
                    while let Some(next) = lines.peek() {
                        if next.starts_with("🔧")
                            || next.starts_with("📄")
                            || next.starts_with("📝")
                            || next.starts_with("✏️")
                            || next.starts_with("📤")
                        {
                            break;
                        }
                        let nl = lines.next().unwrap();
                        if nl.starts_with("```") {
                            continue;
                        }
                        new_text.push_str(nl);
                        new_text.push('\n');
                    }
                }
            }
            html.push_str(&render_file_edit_tool(&path, &old_text, &new_text));
            continue;
        }

        // Detect search: "🔍" or "grep"/"find"
        if line.starts_with("🔍 Searching") || line.starts_with("🔍 grep")
            || line.starts_with("🔍 find")
        {
            found = true;
            let query = line.trim_start_matches(|c: char| !c.is_alphanumeric())
                .trim()
                .to_string();
            let mut results = Vec::new();

            while let Some(next) = lines.peek() {
                if next.starts_with("🔧")
                    || next.starts_with("📄")
                    || next.starts_with("📝")
                    || next.starts_with("✏️")
                    || next.starts_with("📤")
                    || next.trim().is_empty()
                {
                    break;
                }
                results.push(lines.next().unwrap().to_string());
            }
            html.push_str(&render_search_tool(&query, &results));
            continue;
        }
    }

    if found {
        Some(html)
    } else {
        None
    }
}

/// Extract a file path from a tool operation header line.
fn extract_path_from_line(line: &str) -> String {
    // Try to extract path after common patterns
    let line = line.trim();
    for prefix in &[
        "📄 Reading ",
        "📄 reading ",
        "📄 Read ",
        "📄 read ",
        "📝 Writing ",
        "📝 writing ",
        "📝 Write ",
        "📝 write ",
        "✏️ Editing ",
        "✏️ editing ",
        "✏️ Edit ",
        "✏️ edit ",
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest.trim().trim_end_matches(':').to_string();
        }
    }
    line.to_string()
}

/// Render a bash tool call and its output as styled HTML.
fn render_bash_tool(command: &str, output: &str) -> String {
    let mut html = String::new();
    html.push_str("<div class=\"tool-block tool-bash\">\n");
    html.push_str("<div class=\"tool-label\">⌨ Bash</div>\n");
    html.push_str("<pre class=\"tool-command\"><code>");
    html.push_str(&html_escape(command.trim()));
    html.push_str("</code></pre>\n");
    if !output.trim().is_empty() {
        html.push_str("<details class=\"tool-output-details\">\n");
        html.push_str("<summary>Output</summary>\n");
        html.push_str("<pre class=\"tool-output\"><code>");
        html.push_str(&html_escape(output.trim()));
        html.push_str("</code></pre>\n");
        html.push_str("</details>\n");
    }
    html.push_str("</div>\n");
    html
}

/// Render a file read operation as styled HTML.
fn render_file_read_tool(path: &str, content: &str) -> String {
    let mut html = String::new();
    html.push_str("<div class=\"tool-block tool-file-read\">\n");
    html.push_str("<div class=\"tool-label\">📄 Read: ");
    html.push_str(&html_escape(path));
    html.push_str("</div>\n");
    html.push_str("<details class=\"tool-output-details\" open>\n");
    html.push_str("<summary>Content</summary>\n");
    html.push_str("<pre class=\"tool-output\"><code>");
    html.push_str(&html_escape(content.trim()));
    html.push_str("</code></pre>\n");
    html.push_str("</details>\n");
    html.push_str("</div>\n");
    html
}

/// Render a file write operation as styled HTML.
fn render_file_write_tool(path: &str, content: &str) -> String {
    let mut html = String::new();
    html.push_str("<div class=\"tool-block tool-file-write\">\n");
    html.push_str("<div class=\"tool-label\">📝 Write: ");
    html.push_str(&html_escape(path));
    html.push_str("</div>\n");
    html.push_str("<details class=\"tool-output-details\">\n");
    html.push_str("<summary>Content</summary>\n");
    html.push_str("<pre class=\"tool-output\"><code>");
    html.push_str(&html_escape(content.trim()));
    html.push_str("</code></pre>\n");
    html.push_str("</details>\n");
    html.push_str("</div>\n");
    html
}

/// Render a file edit operation as styled HTML.
fn render_file_edit_tool(path: &str, old_text: &str, new_text: &str) -> String {
    let mut html = String::new();
    html.push_str("<div class=\"tool-block tool-file-edit\">\n");
    html.push_str("<div class=\"tool-label\">✏️ Edit: ");
    html.push_str(&html_escape(path));
    html.push_str("</div>\n");

    if !old_text.trim().is_empty() {
        html.push_str("<div class=\"edit-section edit-old\">\n");
        html.push_str("<div class=\"edit-label\">− Removed</div>\n");
        html.push_str("<pre class=\"tool-output\"><code>");
        html.push_str(&html_escape(old_text.trim()));
        html.push_str("</code></pre>\n");
        html.push_str("</div>\n");
    }

    if !new_text.trim().is_empty() {
        html.push_str("<div class=\"edit-section edit-new\">\n");
        html.push_str("<div class=\"edit-label\">+ Added</div>\n");
        html.push_str("<pre class=\"tool-output\"><code>");
        html.push_str(&html_escape(new_text.trim()));
        html.push_str("</code></pre>\n");
        html.push_str("</div>\n");
    }

    html.push_str("</div>\n");
    html
}

/// Render search results as styled HTML.
fn render_search_tool(query: &str, results: &[String]) -> String {
    let mut html = String::new();
    html.push_str("<div class=\"tool-block tool-search\">\n");
    html.push_str("<div class=\"tool-label\">🔍 Search: ");
    html.push_str(&html_escape(query));
    html.push_str("</div>\n");

    if results.is_empty() {
        html.push_str("<div class=\"tool-no-results\">No results found</div>\n");
    } else {
        html.push_str("<div class=\"search-results\">\n");
        for result in results {
            // Render with ANSI support (grep output often has colors)
            let rendered = ansi_to_html(result);
            html.push_str("<div class=\"search-result-line\">");
            html.push_str(&rendered);
            html.push_str("</div>\n");
        }
        html.push_str("</div>\n");
    }

    html.push_str("</div>\n");
    html
}

// ── Core export functions ────────────────────────────────────────────

/// Render a flat list of session entries into a self-contained HTML string.
///
/// `tree` is optional – when provided, a sidebar with session-tree navigation
/// is included.
pub fn export_html(
    entries: &[SessionEntry],
    meta: &ExportMeta,
    session_meta: Option<&SessionMeta>,
    tree: Option<&TreeNode>,
) -> Result<String> {
    export_html_with_options(entries, meta, session_meta, tree, &HtmlExportOptions::default())
}

/// Render a flat list of session entries into a self-contained HTML string
/// with fine-grained control over the output via [`HtmlExportOptions`].
pub fn export_html_with_options(
    entries: &[SessionEntry],
    meta: &ExportMeta,
    session_meta: Option<&SessionMeta>,
    tree: Option<&TreeNode>,
    options: &HtmlExportOptions,
) -> Result<String> {
    let mut html = String::with_capacity(64 * 1024);

    // ── Head ──────────────────────────────────────────────────────
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");

    let title = options
        .title
        .as_deref()
        .or(session_meta.and_then(|m| m.name.as_deref()))
        .unwrap_or("oxi session export");
    write!(html, "<title>{}</title>\n", html_escape(title))?;

    // highlight.js CDN for syntax highlighting
    html.push_str(
        "<link rel=\"stylesheet\" href=\"https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github-dark.min.css\" id=\"hljs-dark\">\n",
    );
    html.push_str(
        "<link rel=\"stylesheet\" href=\"https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github.min.css\" id=\"hljs-light\" disabled>\n",
    );
    html.push_str(
        "<script src=\"https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/highlight.min.js\"></script>\n",
    );

    // Embedded CSS
    html.push_str("<style>\n");
    html.push_str(CSS);
    html.push_str("\n</style>\n");
    html.push_str("</head>\n");

    // ── Body ──────────────────────────────────────────────────────
    let theme_class = if options.dark_theme {
        "dark"
    } else {
        "light"
    };
    write!(html, "<body class=\"{}\">\n", theme_class)?;

    // Theme toggle button
    html.push_str(
        "<button id=\"theme-toggle\" onclick=\"toggleTheme()\" title=\"Toggle light/dark theme\">",
    );
    html.push_str("🌓</button>\n");

    // Optional tree sidebar
    if let Some(node) = tree {
        html.push_str("<nav class=\"tree-nav\">\n<h3>Session Tree</h3>\n");
        render_tree_node(&mut html, node, 0)?;
        html.push_str("</nav>\n");
    }

    // Main content
    html.push_str("<main class=\"content\">\n");

    // Metadata header
    render_meta_header(&mut html, meta, session_meta)?;

    // Messages
    for entry in entries {
        render_entry(&mut html, entry, options)?;
    }

    html.push_str("</main>\n");

    // Embedded JS
    html.push_str("<script>\n");
    html.push_str(JS);
    html.push_str("\n</script>\n");

    html.push_str("</body>\n</html>\n");
    Ok(html)
}

/// Convenience function: export session entries to an HTML string using the
/// given [`HtmlExportOptions`].
pub fn export_to_html(
    entries: &[SessionEntry],
    meta: &ExportMeta,
    options: &HtmlExportOptions,
) -> Result<String> {
    export_html_with_options(entries, meta, None, None, options)
}

// ── Internal rendering helpers ───────────────────────────────────────

fn render_meta_header(
    html: &mut String,
    meta: &ExportMeta,
    session_meta: Option<&SessionMeta>,
) -> Result<()> {
    html.push_str("<header class=\"meta-header\">\n");
    html.push_str("<h1>oxi Session Export</h1>\n");
    html.push_str("<table class=\"meta-table\">\n");

    let exported_dt = DateTime::from_timestamp_millis(meta.exported_at)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    render_meta_row(html, "Exported", &exported_dt)?;

    if let Some(model) = &meta.model {
        render_meta_row(html, "Model", model)?;
    }
    if let Some(provider) = &meta.provider {
        render_meta_row(html, "Provider", provider)?;
    }
    if let Some(sm) = session_meta {
        let created_dt = DateTime::from_timestamp_millis(sm.created_at)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        render_meta_row(html, "Session ID", &sm.id.to_string())?;
        render_meta_row(html, "Created", &created_dt)?;
        if let Some(name) = &sm.name {
            render_meta_row(html, "Name", name)?;
        }
    }
    if let Some(t) = meta.total_user_tokens {
        render_meta_row(html, "User Tokens", &t.to_string())?;
    }
    if let Some(t) = meta.total_assistant_tokens {
        render_meta_row(html, "Assistant Tokens", &t.to_string())?;
    }

    html.push_str("</table>\n</header>\n");
    Ok(())
}

fn render_meta_row(html: &mut String, label: &str, value: &str) -> Result<()> {
    write!(
        html,
        "<tr><td class=\"meta-label\">{}</td><td class=\"meta-value\">{}</td></tr>\n",
        html_escape(label),
        html_escape(value)
    )?;
    Ok(())
}

fn render_entry(html: &mut String, entry: &SessionEntry, options: &HtmlExportOptions) -> Result<()> {
    let ts = DateTime::from_timestamp_millis(entry.timestamp)
        .map(|dt| dt.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "".to_string());

    match &entry.message {
        AgentMessage::User { content } => {
            html.push_str("<div class=\"msg msg-user\">\n");
            html.push_str("<div class=\"msg-header\"><span class=\"msg-role\">You</span>");
            write!(html, "<span class=\"msg-time\">{}</span>", html_escape(&ts))?;
            html.push_str("</div>\n");
            html.push_str("<div class=\"msg-body\">");
            html.push_str(&render_markdown_with_options(content, options));
            html.push_str("</div>\n</div>\n");
        }
        AgentMessage::Assistant { content } => {
            html.push_str("<div class=\"msg msg-assistant\">\n");
            html.push_str("<div class=\"msg-header\"><span class=\"msg-role\">Assistant</span>");
            write!(html, "<span class=\"msg-time\">{}</span>", html_escape(&ts))?;
            html.push_str("</div>\n");
            html.push_str("<div class=\"msg-body\">");

            // Try tool rendering first for assistant messages
            if let Some(tool_html) = render_tool_blocks(content, options.include_tool_calls) {
                html.push_str(&tool_html);
                // Also render the non-tool content via markdown
                html.push_str(&render_markdown_with_options(content, options));
            } else {
                html.push_str(&render_markdown_with_options(content, options));
            }

            html.push_str("</div>\n</div>\n");
        }
        AgentMessage::System { content } => {
            html.push_str("<div class=\"msg msg-system\">\n");
            html.push_str("<div class=\"msg-header\"><span class=\"msg-role\">System</span>");
            write!(html, "<span class=\"msg-time\">{}</span>", html_escape(&ts))?;
            html.push_str("</div>\n");
            html.push_str("<div class=\"msg-body\">");
            html.push_str(&render_markdown_with_options(content, options));
            html.push_str("</div>\n</div>\n");
        }
    }
    Ok(())
}

/// Recursively render a session-tree sidebar node.
fn render_tree_node(html: &mut String, node: &TreeNode, depth: usize) -> Result<()> {
    let indent = "&nbsp;".repeat(depth * 4);
    let current = if node.is_current { " tree-current" } else { "" };
    let fallback = node.session_id.to_string();
    let short_id = &fallback[..8.min(fallback.len())];
    let name = node.name.as_deref().unwrap_or(short_id);
    write!(
        html,
        "<div class=\"tree-node{}\">{}<a href=\"#\">{}</a></div>\n",
        current,
        indent,
        html_escape(name)
    )?;
    for child in &node.children {
        render_tree_node(html, child, depth + 1)?;
    }
    Ok(())
}

// ── Minimal markdown → HTML renderer ─────────────────────────────────
//
// Handles: code blocks (```), inline code (`), bold (**), italic (*),
// headers (#), unordered lists (- ), links, and paragraphs.
// This is intentionally lightweight to avoid pulling in a heavy crate.

#[allow(dead_code)]
fn render_markdown(input: &str) -> String {
    render_markdown_with_options(input, &HtmlExportOptions::default())
}

#[allow(dead_code)]
fn render_markdown_with_options(input: &str, options: &HtmlExportOptions) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut in_thinking = false;
    let mut think_buf = String::new();
    let mut lines = input.lines().peekable();

    while let Some(line) = lines.next() {
        // ── Fenced code blocks ────────────────────────────────────
        if line.starts_with("```") {
            if in_code_block {
                // close
                out.push_str("<pre><code class=\"language-");
                out.push_str(&html_escape(&code_lang));
                out.push_str("\">");
                out.push_str(&html_escape(&code_buf));
                out.push_str("</code></pre>\n");
                code_buf.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                in_code_block = true;
                code_lang = line.trim_start_matches('`').trim().to_string();
            }
            continue;
        }
        if in_code_block {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }

        // ── Thinking blocks ───────────────────────────────────────
        if line.trim() == "<think/>" {
            // skip empty self-closing
            continue;
        }
        if line.trim() == "<think" || line.trim() == "<thinking>" {
            if !options.include_thinking {
                // Skip thinking blocks if not included
                while let Some(l) = lines.next() {
                    if l.trim() == "</think" || l.trim() == "</thinking>" {
                        break;
                    }
                }
                continue;
            }
            in_thinking = true;
            continue;
        }
        if in_thinking && (line.trim() == "</think" || line.trim() == "</thinking>") {
            // emit collapsible
            out.push_str("<details class=\"thinking-block\"><summary>💭 Thinking</summary><div class=\"think-content\">");
            out.push_str(&render_inline(&think_buf));
            out.push_str("</div></details>\n");
            think_buf.clear();
            in_thinking = false;
            continue;
        }
        if in_thinking {
            think_buf.push_str(line);
            think_buf.push('\n');
            continue;
        }

        // ── Tool calls (emoji-prefixed) ───────────────────────────
        if options.include_tool_calls {
            if line.starts_with("🔧 ") || line.starts_with("tool:") {
                out.push_str("<div class=\"tool-call\">");
                out.push_str(&render_inline(line));
                out.push_str("</div>\n");
                continue;
            }
            if line.starts_with("📤 ") || line.starts_with("result:") {
                out.push_str("<div class=\"tool-result\">");
                out.push_str(&render_inline(line));
                out.push_str("</div>\n");
                continue;
            }
        } else {
            // Skip tool call lines
            if line.starts_with("🔧 ")
                || line.starts_with("tool:")
                || line.starts_with("📤 ")
                || line.starts_with("result:")
            {
                continue;
            }
        }

        // ── Headings ──────────────────────────────────────────────
        if line.starts_with("### ") {
            out.push_str("<h3>");
            out.push_str(&render_inline(&line[4..]));
            out.push_str("</h3>\n");
            continue;
        }
        if line.starts_with("## ") {
            out.push_str("<h2>");
            out.push_str(&render_inline(&line[3..]));
            out.push_str("</h2>\n");
            continue;
        }
        if line.starts_with("# ") {
            out.push_str("<h1>");
            out.push_str(&render_inline(&line[2..]));
            out.push_str("</h1>\n");
            continue;
        }

        // ── Unordered list items ──────────────────────────────────
        if line.starts_with("- ") || line.starts_with("* ") {
            out.push_str("<li>");
            out.push_str(&render_inline(&line[2..]));
            out.push_str("</li>\n");
            continue;
        }

        // ── Empty line = paragraph break ──────────────────────────
        if line.trim().is_empty() {
            out.push_str("<br>\n");
            continue;
        }

        // ── Regular paragraph ─────────────────────────────────────
        out.push_str("<p>");
        out.push_str(&render_inline(line));
        out.push_str("</p>\n");
    }

    // Close any still-open code block
    if in_code_block {
        out.push_str("<pre><code>");
        out.push_str(&html_escape(&code_buf));
        out.push_str("</code></pre>\n");
    }

    out
}

/// Inline formatting: bold, italic, inline code, links.
fn render_inline(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    let mut chars = input.char_indices().peekable();
    let bytes = input.as_bytes();

    while let Some((i, ch)) = chars.next() {
        match ch {
            '`' => {
                // inline code
                let start = i + 1;
                let end = bytes[start..]
                    .iter()
                    .position(|&b| b == b'`')
                    .map(|pos| start + pos)
                    .unwrap_or(input.len());
                let code = &input[start..end];
                out.push_str("<code>");
                out.push_str(&html_escape(code));
                out.push_str("</code>");
                // skip past closing backtick
                if end < input.len() {
                    for _ in input[i..=end].chars() {
                        chars.next();
                    }
                }
            }
            '*' => {
                // Look ahead for bold (**)
                if bytes.get(i + 1) == Some(&b'*') {
                    let rest = &input[i + 2..];
                    if let Some(end_pos) = rest.find("**") {
                        out.push_str("<strong>");
                        out.push_str(&render_inline(&rest[..end_pos]));
                        out.push_str("</strong>");
                        // skip past closing **
                        for _ in input[i..=i + 2 + end_pos + 1].chars() {
                            chars.next();
                        }
                        continue;
                    }
                }
                // Italic (*)
                let rest = &input[i + 1..];
                if let Some(end_pos) = rest.find('*') {
                    out.push_str("<em>");
                    out.push_str(&render_inline(&rest[..end_pos]));
                    out.push_str("</em>");
                    for _ in input[i..=i + 1 + end_pos].chars() {
                        chars.next();
                    }
                    continue;
                }
                out.push('*');
            }
            '[' => {
                // Markdown link [text](url)
                let rest = &input[i..];
                if let Some(link_end) = rest.find(')') {
                    if let Some(mid) = rest.find("](") {
                        let text = &rest[1..mid];
                        let url = &rest[mid + 2..link_end];
                        out.push_str("<a href=\"");
                        out.push_str(&html_escape(url));
                        out.push_str("\">");
                        out.push_str(&html_escape(text));
                        out.push_str("</a>");
                        // skip entire link
                        for _ in rest[..=link_end].chars() {
                            chars.next();
                        }
                        continue;
                    }
                }
                out.push('[');
            }
            '<' => {
                // escape HTML
                out.push_str("&lt;");
            }
            '>' => {
                out.push_str("&gt;");
            }
            '&' => {
                out.push_str("&amp;");
            }
            _ => {
                out.push(ch);
            }
        }
    }
    out
}

fn html_escape(input: &str) -> String {
    let mut s = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '&' => s.push_str("&amp;"),
            '"' => s.push_str("&quot;"),
            '\'' => s.push_str("&#39;"),
            _ => s.push(ch),
        }
    }
    s
}

// ── Embedded CSS ─────────────────────────────────────────────────────

const CSS: &str = r#"
/* ── Reset & base ──────────────────────────────────────────────── */
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  line-height: 1.6;
  padding: 1rem;
  display: flex;
  min-height: 100vh;
}

/* ── Dark theme (default) ─────────────────────────────────────── */
body.dark {
  background: #1a1b26;
  color: #c0caf5;
}

/* ── Light theme ──────────────────────────────────────────────── */
body.light {
  background: #f8f9fc;
  color: #1a1b26;
}

/* ── Theme toggle button ──────────────────────────────────────── */
#theme-toggle {
  position: fixed;
  top: 1rem;
  right: 1rem;
  z-index: 100;
  background: rgba(255,255,255,0.1);
  border: 1px solid rgba(255,255,255,0.2);
  border-radius: 8px;
  padding: 0.4rem 0.7rem;
  cursor: pointer;
  font-size: 1.2rem;
}
body.light #theme-toggle {
  background: rgba(0,0,0,0.05);
  border-color: rgba(0,0,0,0.15);
}

/* ── Tree sidebar ──────────────────────────────────────────────── */
.tree-nav {
  width: 220px;
  min-width: 220px;
  padding: 1rem;
  margin-right: 1rem;
  border-right: 1px solid rgba(255,255,255,0.1);
  font-size: 0.85rem;
  overflow-y: auto;
}
body.light .tree-nav { border-color: rgba(0,0,0,0.12); }
.tree-nav h3 { margin-bottom: 0.5rem; font-size: 0.95rem; }
.tree-node { padding: 0.2rem 0; }
.tree-node a { text-decoration: none; color: inherit; opacity: 0.7; }
.tree-node a:hover { opacity: 1; }
.tree-current a { font-weight: bold; opacity: 1; }
body.dark .tree-current a { color: #7aa2f7; }
body.light .tree-current a { color: #1d4ed8; }

/* ── Main content ──────────────────────────────────────────────── */
.content {
  flex: 1;
  max-width: 900px;
  margin: 0 auto;
}

/* ── Metadata header ───────────────────────────────────────────── */
.meta-header { margin-bottom: 1.5rem; }
.meta-header h1 { font-size: 1.4rem; margin-bottom: 0.5rem; }
.meta-table { border-collapse: collapse; font-size: 0.9rem; }
.meta-table td { padding: 0.15rem 0.75rem 0.15rem 0; }
.meta-label { color: #7982a9; font-weight: 600; }
body.light .meta-label { color: #6b7280; }

/* ── Message bubbles ───────────────────────────────────────────── */
.msg {
  border-radius: 10px;
  padding: 0.75rem 1rem;
  margin-bottom: 0.75rem;
  max-width: 100%;
  word-wrap: break-word;
  overflow-wrap: break-word;
}

.msg-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 0.35rem;
  font-size: 0.82rem;
}

.msg-role { font-weight: 700; text-transform: uppercase; letter-spacing: 0.04em; }
.msg-time { opacity: 0.5; font-size: 0.78rem; }

.msg-body p { margin: 0.25rem 0; }
.msg-body h1, .msg-body h2, .msg-body h3 { margin: 0.6rem 0 0.25rem; }
.msg-body li { margin-left: 1.2rem; }

/* ── User message ──────────────────────────────────────────────── */
body.dark .msg-user  { background: #24283b; border-left: 4px solid #7aa2f7; }
body.light .msg-user { background: #eef2ff; border-left: 4px solid #6366f1; }
.msg-user .msg-role { color: #7aa2f7; }
body.light .msg-user .msg-role { color: #4f46e5; }

/* ── Assistant message ─────────────────────────────────────────── */
body.dark .msg-assistant  { background: #1f2335; border-left: 4px solid #9ece6a; }
body.light .msg-assistant { background: #f0fdf4; border-left: 4px solid #22c55e; }
.msg-assistant .msg-role { color: #9ece6a; }
body.light .msg-assistant .msg-role { color: #16a34a; }

/* ── System message ────────────────────────────────────────────── */
body.dark .msg-system  { background: #292e42; border-left: 4px solid #ff9e64; }
body.light .msg-system { background: #fffbeb; border-left: 4px solid #f59e0b; }
.msg-system .msg-role { color: #ff9e64; }
body.light .msg-system .msg-role { color: #d97706; }

/* ── Code blocks ───────────────────────────────────────────────── */
pre {
  background: #13141c;
  border-radius: 6px;
  padding: 0.75rem 1rem;
  overflow-x: auto;
  margin: 0.5rem 0;
  font-size: 0.88rem;
}
body.light pre { background: #f1f5f9; }
pre code { font-family: "JetBrains Mono", "Fira Code", "Cascadia Code", monospace; }

code {
  background: rgba(255,255,255,0.07);
  padding: 0.1rem 0.3rem;
  border-radius: 3px;
  font-family: "JetBrains Mono", "Fira Code", monospace;
  font-size: 0.88em;
}
body.light code { background: rgba(0,0,0,0.06); }

/* ── Thinking block (collapsible) ──────────────────────────────── */
.thinking-block {
  border: 1px dashed rgba(255,255,255,0.15);
  border-radius: 6px;
  padding: 0.5rem 0.75rem;
  margin: 0.4rem 0;
  font-size: 0.88rem;
}
body.light .thinking-block { border-color: rgba(0,0,0,0.15); }
.thinking-block summary {
  cursor: pointer;
  color: #bb9af7;
  font-weight: 600;
  user-select: none;
}
body.light .thinking-block summary { color: #7c3aed; }
.think-content {
  margin-top: 0.4rem;
  padding-top: 0.4rem;
  border-top: 1px dashed rgba(255,255,255,0.1);
  opacity: 0.8;
}
body.light .think-content { border-color: rgba(0,0,0,0.08); }

/* ── Tool call / result ────────────────────────────────────────── */
.tool-call, .tool-result {
  border-radius: 5px;
  padding: 0.4rem 0.75rem;
  margin: 0.3rem 0;
  font-size: 0.88rem;
  font-family: monospace;
}
body.dark .tool-call  { background: #2d1f3d; border-left: 3px solid #bb9af7; }
body.dark .tool-result { background: #1a2d2d; border-left: 3px solid #73daca; }
body.light .tool-call  { background: #faf5ff; border-left: 3px solid #a78bfa; }
body.light .tool-result { background: #f0fdfa; border-left: 3px solid #14b8a6; }

/* ── Tool blocks (structured) ──────────────────────────────────── */
.tool-block {
  border-radius: 8px;
  margin: 0.5rem 0;
  overflow: hidden;
}
.tool-label {
  padding: 0.35rem 0.75rem;
  font-weight: 600;
  font-size: 0.85rem;
  font-family: monospace;
}

/* Bash tool */
body.dark .tool-bash { border: 1px solid #3b2d5d; }
body.light .tool-bash { border: 1px solid #e0d4f5; }
body.dark .tool-bash .tool-label { background: #2d1f3d; color: #bb9af7; }
body.light .tool-bash .tool-label { background: #faf5ff; color: #7c3aed; }
.tool-bash .tool-command {
  background: #0d1117;
  border-radius: 4px;
  margin: 0.35rem 0.5rem;
  padding: 0.5rem 0.75rem;
  font-size: 0.85rem;
}
body.light .tool-bash .tool-command { background: #f6f8fa; }

/* File read tool */
body.dark .tool-file-read { border: 1px solid #1e3a5f; }
body.light .tool-file-read { border: 1px solid #d0e0f0; }
body.dark .tool-file-read .tool-label { background: #1a2d44; color: #7aa2f7; }
body.light .tool-file-read .tool-label { background: #eff6ff; color: #2563eb; }

/* File write tool */
body.dark .tool-file-write { border: 1px solid #2d4a2d; }
body.light .tool-file-write { border: 1px solid #d0f0d0; }
body.dark .tool-file-write .tool-label { background: #1a2d1a; color: #9ece6a; }
body.light .tool-file-write .tool-label { background: #f0fdf4; color: #16a34a; }

/* File edit tool */
body.dark .tool-file-edit { border: 1px solid #4a3a1a; }
body.light .tool-file-edit { border: 1px solid #f0e0c0; }
body.dark .tool-file-edit .tool-label { background: #2d2a1a; color: #e0af68; }
body.light .tool-file-edit .tool-label { background: #fffbeb; color: #d97706; }
.edit-section { margin: 0.35rem 0.5rem; }
.edit-old { border-left: 3px solid #f7768e; }
.edit-new { border-left: 3px solid #9ece6a; }
body.light .edit-old { border-left-color: #ef4444; }
body.light .edit-new { border-left-color: #22c55e; }
.edit-label {
  font-size: 0.8rem;
  font-weight: 600;
  padding: 0.15rem 0.5rem;
}
.edit-old .edit-label { color: #f7768e; }
.edit-new .edit-label { color: #9ece6a; }
body.light .edit-old .edit-label { color: #ef4444; }
body.light .edit-new .edit-label { color: #22c55e; }

/* Search tool */
body.dark .tool-search { border: 1px solid #1a3a3a; }
body.light .tool-search { border: 1px solid #d0f0f0; }
body.dark .tool-search .tool-label { background: #1a2d2d; color: #73daca; }
body.light .tool-search .tool-label { background: #f0fdfa; color: #14b8a6; }
.search-results {
  margin: 0.35rem 0.5rem;
  font-family: monospace;
  font-size: 0.85rem;
}
.search-result-line {
  padding: 0.15rem 0.5rem;
  border-bottom: 1px solid rgba(255,255,255,0.05);
}
body.light .search-result-line { border-bottom-color: rgba(0,0,0,0.05); }
.search-result-line:last-child { border-bottom: none; }
.tool-no-results {
  padding: 0.5rem 0.75rem;
  opacity: 0.6;
  font-style: italic;
}

/* Tool output details (collapsible) */
.tool-output-details {
  margin: 0.35rem 0.5rem;
}
.tool-output-details summary {
  cursor: pointer;
  font-size: 0.82rem;
  color: #7982a9;
  padding: 0.2rem 0;
  user-select: none;
}
body.light .tool-output-details summary { color: #6b7280; }
.tool-output-details summary:hover { color: inherit; }
.tool-output {
  background: #0d1117;
  border-radius: 4px;
  padding: 0.5rem 0.75rem;
  font-size: 0.85rem;
  max-height: 400px;
  overflow: auto;
}
body.light .tool-output { background: #f6f8fa; }

/* ── ANSI lines ────────────────────────────────────────────────── */
.ansi-line {
  font-family: "JetBrains Mono", "Fira Code", monospace;
  font-size: 0.85rem;
  line-height: 1.5;
  white-space: pre;
}

/* ── Links ─────────────────────────────────────────────────────── */
a { color: #7aa2f7; text-decoration: underline; }
body.light a { color: #2563eb; }
"#;

// ── Embedded JS ──────────────────────────────────────────────────────

const JS: &str = r#"
function toggleTheme() {
  const body = document.body;
  const isDark = body.classList.contains('dark');
  body.classList.toggle('dark', !isDark);
  body.classList.toggle('light', isDark);

  // Swap highlight.js stylesheet
  const darkSheet = document.getElementById('hljs-dark');
  const lightSheet = document.getElementById('hljs-light');
  if (darkSheet && lightSheet) {
    darkSheet.disabled = isDark;
    lightSheet.disabled = !isDark;
  }
}

// Apply syntax highlighting
document.addEventListener('DOMContentLoaded', () => {
  document.querySelectorAll('pre code').forEach((block) => {
    hljs.highlightElement(block);
  });
});
"#;

// ══════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::AgentMessage;

    fn make_entry(msg: AgentMessage) -> SessionEntry {
        SessionEntry {
            id: Uuid::new_v4(),
            parent_id: None,
            message: msg,
            label: None,
            timestamp: 1_700_000_000_000,
        }
    }

    // ── HTML export tests ────────────────────────────────────────

    #[test]
    fn export_produces_valid_html_structure() {
        let entries = vec![
            make_entry(AgentMessage::User {
                content: "Hello".into(),
            }),
            make_entry(AgentMessage::Assistant {
                content: "Hi there!".into(),
            }),
        ];
        let meta = ExportMeta::default();
        let html = export_html(&entries, &meta, None, None).unwrap();

        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<html"));
        assert!(html.contains("</html>"));
        assert!(html.contains("<head>"));
        assert!(html.contains("</head>"));
        assert!(html.contains("<body"));
        assert!(html.contains("</body>"));
        assert!(html.contains("msg-user"));
        assert!(html.contains("msg-assistant"));
        assert!(html.contains("You"));
        assert!(html.contains("Assistant"));
        assert!(html.contains("Hello"));
        assert!(html.contains("Hi there!"));
    }

    #[test]
    fn export_renders_thinking_block_collapsible() {
        let entries = vec![make_entry(AgentMessage::Assistant {
            content: "<think\nLet me reason step by step.\n</think\n\nThe answer is 42.".into(),
        })];
        let meta = ExportMeta::default();
        let html = export_html(&entries, &meta, None, None).unwrap();

        assert!(html.contains("<details class=\"thinking-block\">"));
        assert!(html.contains("<summary>💭 Thinking</summary>"));
        assert!(html.contains("Let me reason step by step."));
        assert!(html.contains("The answer is 42."));
    }

    #[test]
    fn export_includes_metadata_header() {
        let entries = vec![];
        let meta = ExportMeta {
            model: Some("claude-sonnet-4".into()),
            provider: Some("anthropic".into()),
            exported_at: 1_700_000_000_000,
            total_user_tokens: Some(120),
            total_assistant_tokens: Some(350),
        };
        let html = export_html(&entries, &meta, None, None).unwrap();

        assert!(html.contains("claude-sonnet-4"));
        assert!(html.contains("anthropic"));
        assert!(html.contains("120"));
        assert!(html.contains("350"));
        assert!(html.contains("User Tokens"));
        assert!(html.contains("Assistant Tokens"));
    }

    #[test]
    fn export_renders_code_block_with_language_class() {
        let entries = vec![make_entry(AgentMessage::Assistant {
            content:
                "Here is some code:\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\nDone."
                    .into(),
        })];
        let meta = ExportMeta::default();
        let html = export_html(&entries, &meta, None, None).unwrap();

        assert!(html.contains("language-rust"));
        assert!(html.contains("fn main()"));
        assert!(html.contains("println!"));
    }

    #[test]
    fn export_renders_tool_calls_and_results() {
        let entries = vec![make_entry(AgentMessage::Assistant {
            content: "🔧 Running bash\n```\nls -la\n```\n📤 result:\nfile1.txt\nfile2.txt".into(),
        })];
        let meta = ExportMeta::default();
        let html = export_html(&entries, &meta, None, None).unwrap();

        assert!(html.contains("tool-call"));
        assert!(html.contains("tool-result"));
    }

    #[test]
    fn export_renders_session_tree_navigation() {
        let tree = TreeNode {
            session_id: Uuid::new_v4(),
            name: Some("root session".into()),
            is_current: false,
            children: vec![TreeNode {
                session_id: Uuid::new_v4(),
                name: Some("branch-1".into()),
                is_current: true,
                children: vec![],
            }],
        };
        let meta = ExportMeta::default();
        let html = export_html(&[], &meta, None, Some(&tree)).unwrap();

        assert!(html.contains("tree-nav"));
        assert!(html.contains("tree-current"));
        assert!(html.contains("root session"));
        assert!(html.contains("branch-1"));
    }

    #[test]
    fn export_dark_theme_default_with_toggle() {
        let meta = ExportMeta::default();
        let html = export_html(&[], &meta, None, None).unwrap();

        assert!(html.contains("class=\"dark\""));
        assert!(html.contains("toggleTheme"));
        assert!(html.contains("theme-toggle"));
    }

    // ── HtmlExportOptions tests ──────────────────────────────────

    #[test]
    fn export_options_light_theme() {
        let options = HtmlExportOptions {
            dark_theme: false,
            ..Default::default()
        };
        let meta = ExportMeta::default();
        let html =
            export_html_with_options(&[], &meta, None, None, &options).unwrap();
        assert!(html.contains("class=\"light\""));
    }

    #[test]
    fn export_options_custom_title() {
        let options = HtmlExportOptions {
            title: Some("My Session".into()),
            ..Default::default()
        };
        let meta = ExportMeta::default();
        let html =
            export_html_with_options(&[], &meta, None, None, &options).unwrap();
        assert!(html.contains("<title>My Session</title>"));
    }

    #[test]
    fn export_options_skip_thinking() {
        let entries = vec![make_entry(AgentMessage::Assistant {
            content: "<think\nSecret thoughts\n</think\n\nVisible answer.".into(),
        })];
        let options = HtmlExportOptions {
            include_thinking: false,
            ..Default::default()
        };
        let meta = ExportMeta::default();
        let html =
            export_html_with_options(&entries, &meta, None, None, &options).unwrap();
        assert!(!html.contains("thinking-block"));
        assert!(!html.contains("Secret thoughts"));
        assert!(html.contains("Visible answer"));
    }

    #[test]
    fn export_options_skip_tool_calls() {
        let entries = vec![make_entry(AgentMessage::Assistant {
            content: "🔧 Running bash\n📤 Done".into(),
        })];
        let options = HtmlExportOptions {
            include_tool_calls: false,
            ..Default::default()
        };
        let meta = ExportMeta::default();
        let html =
            export_html_with_options(&entries, &meta, None, None, &options).unwrap();
        assert!(!html.contains("tool-call"));
        assert!(!html.contains("tool-result"));
    }

    #[test]
    fn export_to_html_convenience() {
        let entries = vec![make_entry(AgentMessage::User {
            content: "Hello".into(),
        })];
        let meta = ExportMeta::default();
        let options = HtmlExportOptions::default();
        let html = export_to_html(&entries, &meta, &options).unwrap();
        assert!(html.contains("Hello"));
    }

    // ── ANSI-to-HTML tests ───────────────────────────────────────

    #[test]
    fn ansi_to_html_plain_text_unchanged() {
        assert_eq!(ansi_to_html("Hello world"), "Hello world");
    }

    #[test]
    fn ansi_to_html_escapes_html_chars() {
        assert_eq!(
            ansi_to_html("<script>alert('xss')</script>"),
            "&lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt;"
        );
    }

    #[test]
    fn ansi_to_html_standard_foreground_colors() {
        // Red foreground: ESC[31m
        let input = "\x1b[31mError\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("color:#800000"));
        assert!(result.contains("Error"));
        assert!(result.contains("</span>"));
    }

    #[test]
    fn ansi_to_html_bright_foreground_colors() {
        // Bright red: ESC[91m
        let input = "\x1b[91mWarning\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("color:#ff0000"));
        assert!(result.contains("Warning"));
    }

    #[test]
    fn ansi_to_html_standard_background_colors() {
        // Blue background: ESC[44m
        let input = "\x1b[44mBlue bg\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("background-color:#000080"));
        assert!(result.contains("Blue bg"));
    }

    #[test]
    fn ansi_to_html_bright_background_colors() {
        // Bright yellow background: ESC[103m
        let input = "\x1b[103mBright yellow\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("background-color:#ffff00"));
        assert!(result.contains("Bright yellow"));
    }

    #[test]
    fn ansi_to_html_bold() {
        let input = "\x1b[1mBold text\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("font-weight:bold"));
        assert!(result.contains("Bold text"));
    }

    #[test]
    fn ansi_to_html_italic() {
        let input = "\x1b[3mItalic text\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("font-style:italic"));
        assert!(result.contains("Italic text"));
    }

    #[test]
    fn ansi_to_html_underline() {
        let input = "\x1b[4mUnderlined\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("text-decoration:underline"));
        assert!(result.contains("Underlined"));
    }

    #[test]
    fn ansi_to_html_strikethrough() {
        let input = "\x1b[9mStruck\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("text-decoration:line-through"));
        assert!(result.contains("Struck"));
    }

    #[test]
    fn ansi_to_html_dim() {
        let input = "\x1b[2mDimmed\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("opacity:0.6"));
        assert!(result.contains("Dimmed"));
    }

    #[test]
    fn ansi_to_html_256_color() {
        // 256-color: ESC[38;5;196m (bright red from color cube)
        let input = "\x1b[38;5;196mCustom color\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("color:#"));
        assert!(result.contains("Custom color"));
    }

    #[test]
    fn ansi_to_html_true_color_rgb() {
        // True color: ESC[38;2;255;128;0m (orange)
        let input = "\x1b[38;2;255;128;0mOrange\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("color:rgb(255,128,0)"));
        assert!(result.contains("Orange"));
    }

    #[test]
    fn ansi_to_html_background_true_color() {
        let input = "\x1b[48;2;0;128;255mBlue bg\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("background-color:rgb(0,128,255)"));
        assert!(result.contains("Blue bg"));
    }

    #[test]
    fn ansi_to_html_reset_clears_styles() {
        let input = "\x1b[1;31mBold Red\x1b[0m Normal";
        let result = ansi_to_html(input);
        assert!(result.contains("font-weight:bold"));
        assert!(result.contains("color:#800000"));
        assert!(result.contains(" Normal"));
    }

    #[test]
    fn ansi_to_html_multiple_styles_combined() {
        // Bold + Red + Underline
        let input = "\x1b[1;4;31mBold Red Underlined\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("font-weight:bold"));
        assert!(result.contains("text-decoration:underline"));
        assert!(result.contains("color:#800000"));
        assert!(result.contains("Bold Red Underlined"));
    }

    #[test]
    fn ansi_to_html_no_escapes_returns_plain() {
        assert_eq!(ansi_to_html("No escapes here"), "No escapes here");
    }

    #[test]
    fn ansi_to_html_256_color_standard_range() {
        // Color index 2 = green
        let input = "\x1b[38;5;2mGreen\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("color:#008000"));
    }

    #[test]
    fn ansi_to_html_256_color_grayscale() {
        // Grayscale index 232 = very dark gray
        let input = "\x1b[38;5;232mDark gray\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("color:#"));
        assert!(result.contains("Dark gray"));
    }

    #[test]
    fn ansi_to_html_background_256() {
        let input = "\x1b[48;5;4mBlue bg\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("background-color:#000080"));
    }

    #[test]
    fn ansi_lines_to_html_wraps_lines() {
        let lines = vec!["Line 1", "Line 2", ""];
        let result = ansi_lines_to_html(&lines);
        assert!(result.contains("<div class=\"ansi-line\">Line 1</div>"));
        assert!(result.contains("<div class=\"ansi-line\">Line 2</div>"));
        assert!(result.contains("&nbsp;</div>"));
    }

    // ── Color conversion tests ───────────────────────────────────

    #[test]
    fn color_256_standard_colors() {
        assert_eq!(color_256_to_hex(0), "#000000");
        assert_eq!(color_256_to_hex(7), "#c0c0c0");
        assert_eq!(color_256_to_hex(15), "#ffffff");
    }

    #[test]
    fn color_256_cube_colors() {
        // Index 16 = first cube color (black)
        assert_eq!(color_256_to_hex(16), "#000000");
        // Index 231 = last cube color (white)
        assert_eq!(color_256_to_hex(231), "#ffffff");
    }

    #[test]
    fn color_256_grayscale() {
        // Index 232 = darkest gray
        assert_eq!(color_256_to_hex(232), "#080808");
        // Index 255 = lightest gray
        assert_eq!(color_256_to_hex(255), "#eeeeee");
    }

    // ── Markdown tests ───────────────────────────────────────────

    #[test]
    fn markdown_renders_bold_and_italic() {
        let result = render_markdown("This is **bold** and *italic* text.");
        assert!(result.contains("<strong>bold</strong>"));
        assert!(result.contains("<em>italic</em>"));
    }

    #[test]
    fn markdown_renders_inline_code() {
        let result = render_markdown("Use `cargo build` to compile.");
        assert!(result.contains("<code>cargo build</code>"));
    }

    #[test]
    fn markdown_renders_links() {
        let result = render_markdown("See [docs](https://example.com) for info.");
        assert!(result.contains("<a href=\"https://example.com\">docs</a>"));
    }

    #[test]
    fn html_escape_prevents_xss() {
        let escaped = html_escape("<script>alert('xss')</script>");
        assert!(!escaped.contains('<'));
        assert!(escaped.contains("&lt;script"));
    }

    // ── Tool rendering tests ─────────────────────────────────────

    #[test]
    fn bash_tool_renders_command_and_output() {
        let html = render_bash_tool("ls -la", "file1.txt\nfile2.txt");
        assert!(html.contains("tool-bash"));
        assert!(html.contains("ls -la"));
        assert!(html.contains("file1.txt"));
        assert!(html.contains("tool-output-details"));
    }

    #[test]
    fn file_read_tool_renders_path_and_content() {
        let html = render_file_read_tool("/path/to/file.rs", "fn main() {}");
        assert!(html.contains("tool-file-read"));
        assert!(html.contains("/path/to/file.rs"));
        assert!(html.contains("fn main()"));
    }

    #[test]
    fn file_write_tool_renders_path_and_content() {
        let html = render_file_write_tool("/path/to/output.txt", "Hello world");
        assert!(html.contains("tool-file-write"));
        assert!(html.contains("/path/to/output.txt"));
        assert!(html.contains("Hello world"));
    }

    #[test]
    fn file_edit_tool_renders_diff() {
        let html = render_file_edit_tool("/src/main.rs", "old code", "new code");
        assert!(html.contains("tool-file-edit"));
        assert!(html.contains("/src/main.rs"));
        assert!(html.contains("edit-old"));
        assert!(html.contains("edit-new"));
        assert!(html.contains("old code"));
        assert!(html.contains("new code"));
    }

    #[test]
    fn search_tool_renders_results() {
        let results = vec![
            "src/main.rs:10:found match".to_string(),
            "src/lib.rs:42:another match".to_string(),
        ];
        let html = render_search_tool("TODO", &results);
        assert!(html.contains("tool-search"));
        assert!(html.contains("TODO"));
        assert!(html.contains("found match"));
        assert!(html.contains("another match"));
    }

    #[test]
    fn search_tool_shows_no_results() {
        let html = render_search_tool("missing", &[]);
        assert!(html.contains("No results found"));
    }
}
