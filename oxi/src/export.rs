//! Session export to HTML and JSON formats.
//!
//! Provides functionality to export conversation sessions as:
//! - **HTML**: Standalone, styled HTML page with conversation rendering
//! - **JSON**: Structured JSON representation of all session entries
//!
//! Also provides sharing via GitHub gist (secret gist via `gh` CLI).

use crate::session::{AgentMessage, SessionFileEntry, SessionHandle, SessionInfo, SessionManager};
use anyhow::{Context, Result};
use std::path::Path;

// ---------------------------------------------------------------------------
// Export from persisted session
// ---------------------------------------------------------------------------

/// Export a session (found by ID prefix) to a standalone HTML file.
pub async fn export_session_html(
    manager: &SessionManager,
    session_id_prefix: &str,
    output_path: Option<&Path>,
) -> Result<String> {
    let sessions = manager.list_sessions()?;
    let info = sessions.iter()
        .find(|s| s.id.starts_with(session_id_prefix))
        .context(format!("Session '{}' not found", session_id_prefix))?;

    let handle = manager.open_session(&info.path).await?;
    let entries: Vec<&SessionFileEntry> = handle.data_entries();

    let html = render_session_html(&entries, &info.id, info.name.as_deref());

    let path = match output_path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?.join(format!("session-{}.html", &info.id[..8.min(info.id.len())])),
    };

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(&path, html).await?;
    Ok(path.display().to_string())
}

/// Export a session (found by ID prefix) to JSON format.
pub async fn export_session_json(
    manager: &SessionManager,
    session_id_prefix: &str,
    output_path: Option<&Path>,
) -> Result<String> {
    let sessions = manager.list_sessions()?;
    let info = sessions.iter()
        .find(|s| s.id.starts_with(session_id_prefix))
        .context(format!("Session '{}' not found", session_id_prefix))?;

    let handle = manager.open_session(&info.path).await?;
    let entries = handle.entries();

    let json = serde_json::to_string_pretty(&entries)
        .context("Failed to serialize session entries to JSON")?;

    let path = match output_path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?.join(format!("session-{}.json", &info.id[..8.min(info.id.len())])),
    };

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(&path, json).await?;
    Ok(path.display().to_string())
}

// ---------------------------------------------------------------------------
// Export from in-memory entries (for TUI /export before save)
// ---------------------------------------------------------------------------

/// Export session entries to a standalone HTML file.
pub fn export_entries_html(entries: &[&SessionFileEntry], output_path: &Path, session_id: &str) -> Result<String> {
    let html = render_session_html(entries, session_id, None);

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(output_path, &html)?;
    Ok(output_path.display().to_string())
}

/// Export session entries to JSON.
pub fn export_entries_json(entries: &[&SessionFileEntry], output_path: &Path) -> Result<String> {
    let json = serde_json::to_string_pretty(&entries)
        .context("Failed to serialize session entries to JSON")?;

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(output_path, json)?;
    Ok(output_path.display().to_string())
}

// ---------------------------------------------------------------------------
// HTML rendering
// ---------------------------------------------------------------------------

/// Render session entries to a complete, standalone HTML page.
fn render_session_html(
    entries: &[&SessionFileEntry],
    session_id: &str,
    session_name: Option<&str>,
) -> String {
    let title = match session_name {
        Some(name) => format!("oxi Session: {}", name),
        None => format!("oxi Session {}", &session_id[..8.min(session_id.len())]),
    };

    let mut messages_html = String::new();

    for entry in entries {
        let (role_class, content) = match entry {
            SessionFileEntry::Message(msg) => match &msg.message {
                AgentMessage::User { content } => ("user", content.clone()),
                AgentMessage::Assistant { content, .. } => ("assistant", content.clone()),
                AgentMessage::System { content } => ("system", content.clone()),
                AgentMessage::ToolResult { content, .. } => ("tool", content.clone()),
            },
            SessionFileEntry::ModelChange(mc) => {
                ("info", format!("Model: {}/{}", mc.provider, mc.model_id))
            }
            _ => continue,
        };

        let escaped = html_escape(&content);
        let rendered = render_code_blocks(&escaped);

        messages_html.push_str(&format!(
            r#"<div class="message {role_class}">
  <div class="message-content">{rendered}</div>
</div>
"#
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{title_escaped}</title>
  <style>
    :root {{
      --bg-primary: #1a1a2e;
      --bg-user: #0f3460;
      --bg-assistant: #1a1a2e;
      --bg-system: #2d2d2d;
      --bg-tool: #1a2e1a;
      --bg-info: #2e2a1a;
      --text-primary: #e0e0e0;
      --border-color: #333;
      --code-bg: #2d2d2d;
    }}
    * {{ margin: 0; padding: 0; box-sizing: border-box; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: var(--bg-primary); color: var(--text-primary);
      line-height: 1.6; padding: 2rem; max-width: 900px; margin: 0 auto;
    }}
    h1 {{ font-size: 1.4rem; margin-bottom: 0.5rem; color: #fff; }}
    .meta {{ color: #a0a0a0; font-size: 0.85rem; margin-bottom: 2rem; }}
    .message {{ border-radius: 8px; padding: 1rem 1.25rem; margin-bottom: 0.75rem; border: 1px solid var(--border-color); }}
    .message.user {{ background: var(--bg-user); border-color: #1a4a7a; }}
    .message.assistant {{ background: var(--bg-assistant); }}
    .message.system {{ background: var(--bg-system); font-style: italic; }}
    .message.tool {{ background: var(--bg-tool); font-family: monospace; font-size: 0.85em; white-space: pre-wrap; }}
    .message.info {{ background: var(--bg-info); font-style: italic; font-size: 0.9em; }}
    .message-content {{ white-space: pre-wrap; word-wrap: break-word; font-size: 0.95rem; }}
    .message-content code {{ background: var(--code-bg); padding: 0.15rem 0.4rem; border-radius: 3px; font-family: "SF Mono", "Fira Code", monospace; font-size: 0.85em; }}
    .message-content pre {{ background: var(--code-bg); padding: 1rem; border-radius: 6px; overflow-x: auto; margin: 0.5rem 0; }}
    .message-content pre code {{ background: none; padding: 0; }}
    .footer {{ text-align: center; color: #a0a0a0; font-size: 0.75rem; margin-top: 2rem; padding-top: 1rem; border-top: 1px solid var(--border-color); }}
  </style>
</head>
<body>
  <h1>{title_escaped}</h1>
  <div class="meta">Session ID: {session_id} &middot; {entry_count} entries</div>
  <div class="messages">
{messages_html}
  </div>
  <div class="footer">Exported by oxi on {export_date}</div>
</body>
</html>"#,
        title_escaped = html_escape(&title),
        session_id = session_id,
        entry_count = entries.len(),
        messages_html = messages_html,
        export_date = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn render_code_blocks(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_code_block = false;
    for line in input.lines() {
        if line.starts_with("```") && !in_code_block {
            in_code_block = true;
            result.push_str("<pre><code>");
        } else if line.starts_with("```") && in_code_block {
            in_code_block = false;
            result.push_str("</code></pre>\n");
        } else if in_code_block {
            result.push_str(line);
            result.push('\n');
        } else {
            result.push_str(&render_inline_code(line));
            result.push('\n');
        }
    }
    result
}

fn render_inline_code(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if chars[i] == '`' {
            let start = i + 1;
            let mut end = start;
            while end < len && chars[end] != '`' { end += 1; }
            if end < len {
                let code: String = chars[start..end].iter().collect();
                result.push_str("<code>");
                result.push_str(&html_escape(&code));
                result.push_str("</code>");
                i = end + 1;
            } else {
                result.push('`');
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Sharing via GitHub gist
// ---------------------------------------------------------------------------

/// Share in-memory entries as a GitHub gist.
pub async fn share_entries_as_gist(entries: &[&SessionFileEntry], session_id: &str) -> Result<String> {
    if entries.is_empty() {
        anyhow::bail!("No entries to share");
    }
    let tmp_dir = tempfile::tempdir()?;
    let html_path = tmp_dir.path().join("session.html");
    export_entries_html(entries, &html_path, session_id)?;
    create_gist_from_file(&html_path.display().to_string()).await
}

async fn create_gist_from_file(file_path: &str) -> Result<String> {
    let gh_check = tokio::process::Command::new("gh")
        .args(["auth", "status"])
        .output()
        .await
        .context("GitHub CLI (gh) is not installed. Install from https://cli.github.com/")?;
    if !gh_check.status.success() {
        anyhow::bail!("GitHub CLI is not logged in. Run 'gh auth login' first.");
    }
    let output = tokio::process::Command::new("gh")
        .args(["gist", "create", "--public=false", file_path])
        .output()
        .await
        .context("Failed to run gh gist create")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create gist: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_escape() {
        assert!(html_escape("<b>bold</b>").contains("&lt;b&gt;"));
        assert!(html_escape("a & b").contains("&amp;"));
    }

    #[test]
    fn test_render_inline_code() {
        let result = render_inline_code("Use `console.log` for debugging");
        assert!(result.contains("<code>console.log</code>"));
    }

    #[test]
    fn test_render_code_blocks() {
        let input = "Before\n```rust\nfn main() {}\n```\nAfter";
        let result = render_code_blocks(input);
        assert!(result.contains("<pre><code>"));
        assert!(result.contains("fn main() {}"));
        assert!(result.contains("</code></pre>"));
    }
}
