//! `/quit` — exit oxi. Aliases: `/exit`, `/q`.

use super::super::registry::SlashCommand;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// Quit the application.
pub(crate) struct QuitCommand;

impl SlashCommand for QuitCommand {
    fn name(&self) -> &str {
        "quit"
    }
    fn aliases(&self) -> &[&str] {
        &["exit", "q"]
    }
    fn description(&self) -> &str {
        "Quit oxi (aliases: /exit, /q)"
    }
    fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        // The caller maps `SlashOutcome::Quit` to `*running = false`.
        SlashOutcome::Quit
    }
}
