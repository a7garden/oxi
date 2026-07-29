//! `/commit` — commit tool slash command.
//!
//! Delegates to the agent's commit tool or the CLI.
//! The full commit tool lives in the agent tool loop (`/tool commit`);
//! this slash command provides a quick entry point.

use super::super::registry::SlashCommand;
use crate::tui::app::NotificationKind;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// Slash command for the commit tool.
pub(crate) struct CommitCommand;

impl SlashCommand for CommitCommand {
    fn name(&self) -> &str {
        "commit"
    }

    fn description(&self) -> &str {
        "Generate a commit message from staged changes"
    }

    fn usage(&self) -> &str {
        "/commit [context]"
    }

    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.add_notification(
            "Commit tool is available via the agent (/tool commit) or CLI (oxi commit --dry-run)"
                .into(),
            NotificationKind::Info,
        );
        SlashOutcome::Handled
    }
}
