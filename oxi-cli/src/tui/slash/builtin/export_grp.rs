//! Export/import commands: `/export`, `/import`, `/share`. Migrated off the
//! legacy `handle_slash_command` match.
//!
//! (Currently: `/export` and `/import` ported. `/share` follows.)

use super::super::registry::SlashCommand;
use crate::storage::export::{self, ExportMeta, HtmlExportOptions};
use crate::tui::app::{NotificationKind, UiEvent};
use crate::tui::slash::{SlashCtx, SlashOutcome};
use oxi_tui::widgets::chat::{ContentBlock, MessageRole};
use std::path::PathBuf;

/// `/export [path]` — export session to HTML.
pub(crate) struct ExportCommand;

impl SlashCommand for ExportCommand {
    fn name(&self) -> &str {
        "export"
    }
    fn description(&self) -> &str {
        "Export session to HTML"
    }
    fn usage(&self) -> &str {
        "/export [path]"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        let arg = args.trim();
        let export_path = if arg.is_empty() {
            None
        } else {
            Some(PathBuf::from(arg))
        };
        let meta = ExportMeta {
            model: Some(session.model_id()),
            provider: None,
            exported_at: chrono::Utc::now().timestamp_millis(),
            total_user_tokens: None,
            total_assistant_tokens: None,
        };
        let entries: Vec<crate::store::session::SessionEntry> = state
            .messages()
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::System => "system",
                };
                let content: String = msg
                    .content_blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { content } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                crate::store::session::SessionEntry::simple_message(role, &content)
            })
            .collect();
        match export::export_to_html(&entries, &meta, &HtmlExportOptions::default()) {
            Ok(html) => {
                if let Some(path) = export_path {
                    match std::fs::write(&path, &html) {
                        Ok(()) => state.add_notification(
                            format!("Exported: {}", path.display()),
                            NotificationKind::Success,
                        ),
                        Err(e) => state.add_notification(
                            format!("Write failed: {}", e),
                            NotificationKind::Error,
                        ),
                    }
                } else {
                    // Auto-save to CWD with session-based filename
                    let sid = session.session_id();
                    let short_sid = &sid[..8.min(sid.len())];
                    let default_name = format!("oxi-export-{}.html", short_sid);
                    match std::fs::write(&default_name, &html) {
                        Ok(()) => state.add_notification(
                            format!("Exported: {} ({} bytes)", default_name, html.len()),
                            NotificationKind::Success,
                        ),
                        Err(e) => state.add_notification(
                            format!("Write failed: {}", e),
                            NotificationKind::Error,
                        ),
                    }
                }
            }
            Err(e) => {
                state.add_notification(format!("Export failed: {}", e), NotificationKind::Error)
            }
        }
        SlashOutcome::Handled
    }
}

/// `/import <path>` — import and resume a session from a JSONL file.
pub(crate) struct ImportCommand;

impl SlashCommand for ImportCommand {
    fn name(&self) -> &str {
        "import"
    }
    fn description(&self) -> &str {
        "Import and resume a session from a JSONL file"
    }
    fn usage(&self) -> &str {
        "/import <path> [path-to-jsonl]"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let path = args.trim();
        if !path.is_empty() {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string());
            match crate::store::session::resolve_session_path(path, &cwd) {
                Ok(resolved) => {
                    if !std::path::Path::new(&resolved).exists() {
                        state.add_notification(
                            format!("File not found: {}", resolved),
                            NotificationKind::Error,
                        );
                    } else {
                        state.next_action = Some(crate::tui::app::TuiNextAction::SwitchSession(
                            resolved.clone(),
                        ));
                        state.add_notification(
                            format!("Importing session from {}...", resolved),
                            NotificationKind::Info,
                        );
                    }
                }
                Err(e) => {
                    state.add_notification(
                        format!("Error resolving path: {}", e),
                        NotificationKind::Error,
                    );
                }
            }
        } else {
            state.add_notification(
                "/import <path> [path-to-jsonl]".to_string(),
                NotificationKind::Info,
            );
        }
        SlashOutcome::Handled
    }
}

/// `/share` — share session as a GitHub Gist (requires gh CLI).
pub(crate) struct ShareCommand;

impl SlashCommand for ShareCommand {
    fn name(&self) -> &str {
        "share"
    }
    fn description(&self) -> &str {
        "Share session as a GitHub Gist (requires gh CLI)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        let ui_tx = ctx.ui_tx;
        // Check if gh CLI is available
        match std::process::Command::new("gh")
            .arg("auth")
            .arg("status")
            .output()
        {
            Ok(output) if output.status.success() => {
                // Export session to HTML first
                let meta = ExportMeta {
                    model: Some(session.model_id()),
                    provider: None,
                    exported_at: chrono::Utc::now().timestamp_millis(),
                    total_user_tokens: None,
                    total_assistant_tokens: None,
                };
                let entries: Vec<crate::store::session::SessionEntry> = state
                    .messages()
                    .iter()
                    .map(|msg| {
                        let role = match msg.role {
                            MessageRole::User => "user",
                            MessageRole::Assistant => "assistant",
                            MessageRole::System => "system",
                        };
                        let content: String = msg
                            .content_blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { content } => Some(content.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        crate::store::session::SessionEntry::simple_message(role, &content)
                    })
                    .collect();
                match export::export_to_html(&entries, &meta, &HtmlExportOptions::default()) {
                    Ok(html) => {
                        // Write to temp file and create gist
                        let temp_path = std::env::temp_dir().join("oxi-share.html");
                        match std::fs::write(&temp_path, &html) {
                            Ok(()) => {
                                state.add_notification(
                                    "Creating Gist... (Esc to cancel)".to_string(),
                                    NotificationKind::Info,
                                );
                                let _sh = session.clone_handle();
                                let tx = ui_tx.clone();
                                tokio::spawn(async move {
                                    let result = tokio::process::Command::new("gh")
                                        .args(["gist", "create", &temp_path.to_string_lossy()])
                                        .output()
                                        .await;
                                    let _ = std::fs::remove_file(&temp_path);
                                    match result {
                                        Ok(output) if output.status.success() => {
                                            let stdout = String::from_utf8_lossy(&output.stdout);
                                            let gist_url = stdout.trim().to_string();
                                            let _ = tx.send(UiEvent::SystemMessage(format!(
                                                "Gist created: {}",
                                                gist_url
                                            )));
                                        }
                                        Ok(output) => {
                                            let stderr = String::from_utf8_lossy(&output.stderr);
                                            let _ = tx.send(UiEvent::SystemMessage(format!(
                                                "Gist failed: {}",
                                                stderr.trim()
                                            )));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(UiEvent::SystemMessage(format!(
                                                "Gist failed: {}",
                                                e
                                            )));
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                state.add_notification(
                                    format!("Error: {}", e),
                                    NotificationKind::Error,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        state.add_notification(
                            format!("Export failed: {}", e),
                            NotificationKind::Error,
                        );
                    }
                }
            }
            Ok(_output) => {
                state.add_notification(
                    "GitHub CLI not authenticated. Run: gh auth login".to_string(),
                    NotificationKind::Warning,
                );
            }
            Err(_e) => {
                state.add_notification(
                    "GitHub CLI (gh) not found".to_string(),
                    NotificationKind::Error,
                );
            }
        }
        SlashOutcome::Handled
    }
}
