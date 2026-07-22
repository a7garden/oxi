//! `/copy` — copy code block / last reply to clipboard. Migrated off the
//! legacy `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::media::clipboard_write;
use crate::tui::app::NotificationKind;
use crate::tui::slash::{SlashCtx, SlashOutcome};
use oxi_tui_legacy::widgets::chat::{ContentBlock, MessageRole};

/// `/copy` — copy code block / last reply to clipboard.
pub(crate) struct CopyCommand;

impl SlashCommand for CopyCommand {
    fn name(&self) -> &str {
        "copy"
    }
    fn description(&self) -> &str {
        "Copy last agent message to clipboard"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        if let Some(ref code) = state.chat.last_code_block {
            match clipboard_write::copy_to_clipboard(code) {
                Ok(()) => state.add_notification(
                    "Code block copied to clipboard".to_string(),
                    NotificationKind::Success,
                ),
                Err(e) => {
                    state.add_notification(format!("Copy failed: {}", e), NotificationKind::Error)
                }
            }
        } else {
            let last = state
                .messages()
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::Assistant);
            if let Some(msg) = last {
                let content: String = msg
                    .content_blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { content } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if content.trim().is_empty() {
                    state.add_notification(
                        "No text content to copy".to_string(),
                        NotificationKind::Warning,
                    );
                } else {
                    match clipboard_write::copy_to_clipboard(&content) {
                        Ok(()) => state.add_notification(
                            "Copied to clipboard".to_string(),
                            NotificationKind::Success,
                        ),
                        Err(e) => state.add_notification(
                            format!("Copy failed: {}", e),
                            NotificationKind::Error,
                        ),
                    }
                }
            } else {
                state.add_notification("No assistant message".to_string(), NotificationKind::Info);
            }
        }
        SlashOutcome::Handled
    }
}
