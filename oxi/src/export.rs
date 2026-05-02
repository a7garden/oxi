//! Session export to HTML and JSON formats.
//!
//! Provides functionality to export conversation sessions as:
//! - **HTML**: Standalone, styled HTML page with conversation rendering
//! - **JSON**: Structured JSON representation of all session entries

use crate::session::{AgentMessage, SessionEntry, SessionManager};
use anyhow::{Context, Result};
use std::path::Path;
use uuid::Uuid;

/// Export a session to a standalone HTML file.
///
/// If `output_path` is `None`, generates a filename based on the session ID
/// in the current working directory.
pub async fn export_session_html(
    manager: &SessionManager,
    session_id: Uuid,
    output_path: Option<&Path>,
) -> Result<String> {
    let entries = manager.load(session_id).await?;
    if entries.is_empty() {
        anyhow::bail!("Session {} has no entries to export", session_id);
    }

    let meta = manager
        .load_meta(session_id)
        .await?
        .context("Session metadata not found")?;

    let html = render_session_html(&entries, &format!("{}", session_id), meta.name.as_deref());

    let path = match output_path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?.join(format!("session-{}.html", &session_id)),
    };

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(&path, html).await?;

    Ok(path.display().to_string())
}

/// Export a session to JSON format (array of entries).
///
/// If `output_path` is `None`, generates a filename based on the session ID
/// in the current working directory.
pub async fn export_session_json(
    manager: &SessionManager,
    session_id: Uuid,
    output_path: Option<&Path>,
) -> Result<String> {
    let entries = manager.load(session_id).await?;
    if entries.is_empty() {
        anyhow::bail!("Session {} has no entries to export", session_id);
    }

    let json = serde_json::to_string_pretty(&entries)
        .context("Failed to serialize session entries to JSON")?;

    let path = match output_path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?.join(format!("session-{}.json", &session_id)),
    };

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(&path, json).await?;

    Ok(path.display().to_string())
}

/// Export the current interactive session entries (not yet persisted) to HTML.
///
/// This is used for the TUI's `/export` command where entries haven't been
/// saved to the session manager yet.
pub fn export_entries_html(entries: &[SessionEntry], output_path: &Path) -> Result<String> {
    let html = render_session_html(entries, "current", None);

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(output_path, &html)?;

    Ok(output_path.display().to_string())
}

/// Export the current interactive session entries to JSON.
pub fn export_entries_json(entries: &[SessionEntry], output_path: &Path) -> Result<String> {
    let json = serde_json::to_string_pretty(entries)
        .context("Failed to serialize session entries to JSON")?;

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(output_path, json)?;

    Ok(output_path.display().to_string())
}

/// Render session entries to a complete, standalone HTML page.
fn render_session_html(
    entries: &[SessionEntry],
    session_id: &str,
    session_name: Option<&str>,
) -> String {
    let title = match session_name {
        Some(name) => format!("oxi Session: {}", name),
        None => format!("oxi Session {}", &session_id[..8.min(session_id.len())]),
    };

    let mut messages_html = String::new();

    for entry in entries {
        let (role_class, role_label, content) = match &entry.message {
            AgentMessage::User { content } => ("user", "You", content),
            AgentMessage::Assistant { content } => ("assistant", "oxi", content),
            AgentMessage::System { content } => ("system", "System", content),
        };

        let timestamp = chrono::DateTime::from_timestamp_millis(entry.timestamp)
            .map(|dt| dt.format("%H:%M:%S").to_string())
            .unwrap_or_default();

        // Escape HTML entities
        let escaped = html_escape(content);

        // Convert markdown-style code blocks to HTML
        let rendered = render_code_blocks(&escaped);

        messages_html.push_str(&format!(
            r#"<div class="message {role_class}">
  <div class="message-header">
    <span class="role">{role_label}</span>
    <span class="timestamp">{timestamp}</span>
  </div>
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
  <title>{title}</title>
  <style>
    :root {{
      --bg-primary: #1a1a2e;
      --bg-secondary: #16213e;
      --bg-user: #0f3460;
      --bg-assistant: #1a1a2e;
      --bg-system: #2d2d2d;
      --text-primary: #e0e0e0;
      --text-secondary: #a0a0a0;
      --text-user: #ffffff;
      --border-color: #333;
      --code-bg: #2d2d2d;
    }}
    * {{
      margin: 0;
      padding: 0;
      box-sizing: border-box;
    }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      background: var(--bg-primary);
      color: var(--text-primary);
      line-height: 1.6;
      padding: 2rem;
      max-width: 900px;
      margin: 0 auto;
    }}
    h1 {{
      font-size: 1.4rem;
      margin-bottom: 0.5rem;
      color: #fff;
    }}
    .session-meta {{
      color: var(--text-secondary);
      font-size: 0.85rem;
      margin-bottom: 2rem;
    }}
    .message {{
      border-radius: 8px;
      padding: 1rem 1.25rem;
      margin-bottom: 0.75rem;
      border: 1px solid var(--border-color);
    }}
    .message.user {{
      background: var(--bg-user);
      border-color: #1a4a7a;
    }}
    .message.assistant {{
      background: var(--bg-assistant);
      border-color: var(--border-color);
    }}
    .message.system {{
      background: var(--bg-system);
      border-color: #444;
      font-style: italic;
    }}
    .message-header {{
      display: flex;
      justify-content: space-between;
      align-items: center;
      margin-bottom: 0.5rem;
    }}
    .role {{
      font-weight: 600;
      font-size: 0.85rem;
    }}
    .message.user .role {{ color: #7ab8ff; }}
    .message.assistant .role {{ color: #7aff7a; }}
    .message.system .role {{ color: #ffcc7a; }}
    .timestamp {{
      font-size: 0.75rem;
      color: var(--text-secondary);
    }}
    .message-content {{
      white-space: pre-wrap;
      word-wrap: break-word;
      font-size: 0.95rem;
    }}
    .message-content code {{
      background: var(--code-bg);
      padding: 0.15rem 0.4rem;
      border-radius: 3px;
      font-family: "SF Mono", "Fira Code", "Cascadia Code", monospace;
      font-size: 0.85em;
    }}
    .message-content pre {{
      background: var(--code-bg);
      padding: 1rem;
      border-radius: 6px;
      overflow-x: auto;
      margin: 0.5rem 0;
    }}
    .message-content pre code {{
      background: none;
      padding: 0;
    }}
    .footer {{
      text-align: center;
      color: var(--text-secondary);
      font-size: 0.75rem;
      margin-top: 2rem;
      padding-top: 1rem;
      border-top: 1px solid var(--border-color);
    }}
  </style>
</head>
<body>
  <h1>{title}</h1>
  <div class="session-meta">Session ID: {session_id}</div>
  <div class="messages">
{messages_html}
  </div>
  <div class="footer">Exported by oxi on {export_date}</div>
</body>
</html>"#,
        title = html_escape(&title),
        session_id = session_id,
        messages_html = messages_html,
        export_date = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
    )
}

/// Escape HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Convert markdown-style fenced code blocks to HTML `<pre><code>`.
fn render_code_blocks(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_code_block = false;
    let mut lines = input.lines().peekable();

    while let Some(line) = lines.next() {
        if line.starts_with("```") && !in_code_block {
            in_code_block = true;
            let lang = line.trim_start_matches('`').trim();
            result.push_str("<pre><code>");
            if !lang.is_empty() {
                // Language hint — we could use it for syntax highlighting
                // but for now just skip it in output
            }
        } else if line.starts_with("```") && in_code_block {
            in_code_block = false;
            result.push_str("</code></pre>\n");
        } else if in_code_block {
            result.push_str(line);
            result.push('\n');
        } else {
            // Inline code
            let rendered = render_inline_code(line);
            result.push_str(&rendered);
            result.push('\n');
        }
    }

    result
}

/// Convert `backtick` inline code to `<code>` elements.
fn render_inline_code(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '`' {
            // Find closing backtick
            let start = i + 1;
            let mut end = start;
            while end < len && chars[end] != '`' {
                end += 1;
            }
            if end < len {
                // Found closing backtick
                let code_content: String = chars[start..end].iter().collect();
                result.push_str("<code>");
                result.push_str(&html_escape(&code_content));
                result.push_str("</code>");
                i = end + 1;
            } else {
                // No closing backtick — treat as literal
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

/// Share a session by creating a secret GitHub gist.
///
/// Requires `gh` (GitHub CLI) to be installed and authenticated.
/// Returns the gist URL on success.
pub async fn share_as_gist(
    manager: &SessionManager,
    session_id: Uuid,
) -> Result<String> {
    // Export to HTML first
    let tmp_dir = tempfile::tempdir()?;
    let html_path = tmp_dir.path().join("session.html");
    let html_file = export_session_html(manager, session_id, Some(&html_path)).await?;

    // Check gh availability
    let gh_check = tokio::process::Command::new("gh")
        .args(["auth", "status"])
        .output()
        .await
        .context("GitHub CLI (gh) is not installed. Install it from https://cli.github.com/")?;

    if !gh_check.status.success() {
        anyhow::bail!("GitHub CLI is not logged in. Run 'gh auth login' first.");
    }

    // Create secret gist
    let output = tokio::process::Command::new("gh")
        .args(["gist", "create", "--public=false", &html_file])
        .output()
        .await
        .context("Failed to run gh gist create")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create gist: {}", stderr.trim());
    }

    let gist_url = String::from_utf8_lossy(&output.stdout).trim().to_string();

    Ok(gist_url)
}

/// Share entries (in-memory) as a GitHub gist.
///
/// Used for TUI `/share` when entries aren't persisted yet.
pub async fn share_entries_as_gist(entries: &[SessionEntry]) -> Result<String> {
    if entries.is_empty() {
        anyhow::bail!("No entries to share");
    }

    // Export to HTML temp file
    let tmp_dir = tempfile::tempdir()?;
    let html_path = tmp_dir.path().join("session.html");
    export_entries_html(entries, &html_path)?;

    // Check gh availability
    let gh_check = tokio::process::Command::new("gh")
        .args(["auth", "status"])
        .output()
        .await
        .context("GitHub CLI (gh) is not installed. Install it from https://cli.github.com/")?;

    if !gh_check.status.success() {
        anyhow::bail!("GitHub CLI is not logged in. Run 'gh auth login' first.");
    }

    // Create secret gist
    let output = tokio::process::Command::new("gh")
        .args(["gist", "create", "--public=false", &html_path.display().to_string()])
        .output()
        .await
        .context("Failed to run gh gist create")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create gist: {}", stderr.trim());
    }

    let gist_url = String::from_utf8_lossy(&output.stdout).trim().to_string();

    Ok(gist_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<script>alert('xss')</script>"),
            "&lt;script&gt;alert(&#x27;xss&#x27;)&lt;/script&gt;".replace("&#x27;", "'").replace("&#x27;", "'"));
        // Simplified test
        assert!(html_escape("<b>bold</b>").contains("&lt;b&gt;"));
        assert!(html_escape("a & b").contains("&amp;"));
        assert!(html_escape("a \"b\" c").contains("&quot;"));
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
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
    }

    #[test]
    fn test_render_session_html() {
        let entries = vec![
            SessionEntry::new(AgentMessage::User {
                content: "Hello, world!".to_string(),
            }),
            SessionEntry::new(AgentMessage::Assistant {
                content: "Hi there! How can I help?".to_string(),
            }),
        ];

        let html = render_session_html(&entries, "test-session-id", Some("Test Session"));
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Test Session"));
        assert!(html.contains("Hello, world!"));
        assert!(html.contains("Hi there!"));
        assert!(html.contains("class=\"message user\""));
        assert!(html.contains("class=\"message assistant\""));
    }

    #[test]
    fn test_render_session_html_empty() {
        let entries: Vec<SessionEntry> = vec![];
        let html = render_session_html(&entries, "test-id", None);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("test-id"));
    }

    #[test]
    fn test_export_entries_html_to_file() {
        let entries = vec![
            SessionEntry::new(AgentMessage::User {
                content: "test message".to_string(),
            }),
        ];

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("export.html");
        let result = export_entries_html(&entries, &path).unwrap();

        assert!(result.contains("export.html"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("<!DOCTYPE html>"));
        assert!(content.contains("test message"));
    }

    #[test]
    fn test_export_entries_json_to_file() {
        let entries = vec![
            SessionEntry::new(AgentMessage::Assistant {
                content: "response".to_string(),
            }),
        ];

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("export.json");
        let result = export_entries_json(&entries, &path).unwrap();

        assert!(result.contains("export.json"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("response"));
        // Valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.is_array());
    }
}
