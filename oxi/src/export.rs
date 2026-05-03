//! Export conversation sessions to standalone HTML files.
//!
//! Produces a self-contained HTML document with embedded CSS and JS that renders
//! a conversation session with:
//! - Color-coded user / assistant / system messages
//! - Markdown → HTML rendering (basic, no external deps)
//! - Tool calls and results in styled blocks
//! - Collapsible thinking blocks
//! - Metadata header (model, provider, date, token counts)
//! - Dark theme (default) with light-theme toggle
//! - Syntax highlighting for code blocks (JS-only, via highlight.js CDN)
//! - Session tree navigation for branched sessions

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Write as FmtWrite;
use uuid::Uuid;

use crate::session::{AgentMessage, SessionEntry, SessionMeta};

// ── Public types ─────────────────────────────────────────────────────

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

// ── Core export function ─────────────────────────────────────────────

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
    let mut html = String::with_capacity(64 * 1024);

    // ── Head ──────────────────────────────────────────────────────
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");

    let title = session_meta
        .and_then(|m| m.name.clone())
        .unwrap_or_else(|| "oxi session export".to_string());
    write!(html, "<title>{}</title>\n", html_escape(&title))?;

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
    html.push_str("<body class=\"dark\">\n");

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
        render_entry(&mut html, entry)?;
    }

    html.push_str("</main>\n");

    // Embedded JS
    html.push_str("<script>\n");
    html.push_str(JS);
    html.push_str("\n</script>\n");

    html.push_str("</body>\n</html>\n");
    Ok(html)
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

fn render_entry(html: &mut String, entry: &SessionEntry) -> Result<()> {
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
            html.push_str(&render_markdown(content));
            html.push_str("</div>\n</div>\n");
        }
        AgentMessage::Assistant { content } => {
            html.push_str("<div class=\"msg msg-assistant\">\n");
            html.push_str("<div class=\"msg-header\"><span class=\"msg-role\">Assistant</span>");
            write!(html, "<span class=\"msg-time\">{}</span>", html_escape(&ts))?;
            html.push_str("</div>\n");
            html.push_str("<div class=\"msg-body\">");
            html.push_str(&render_markdown(content));
            html.push_str("</div>\n</div>\n");
        }
        AgentMessage::System { content } => {
            html.push_str("<div class=\"msg msg-system\">\n");
            html.push_str("<div class=\"msg-header\"><span class=\"msg-role\">System</span>");
            write!(html, "<span class=\"msg-time\">{}</span>", html_escape(&ts))?;
            html.push_str("</div>\n");
            html.push_str("<div class=\"msg-body\">");
            html.push_str(&render_markdown(content));
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

fn render_markdown(input: &str) -> String {
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

        // ── Tool calls ────────────────────────────────────────────
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
        // Contains both messages
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
}
