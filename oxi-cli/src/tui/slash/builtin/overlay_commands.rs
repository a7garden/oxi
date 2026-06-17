//! Simple overlay-opening commands: `/help`, `/hotkeys`, `/extensions`,
//! `/changelog`. Migrated off the legacy `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::tui::app::NotificationKind;
use crate::tui::overlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};
use std::path::PathBuf;

/// `/help` — show help and available commands. Alias: `?`.
pub(crate) struct HelpCommand;

impl SlashCommand for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }
    fn aliases(&self) -> &[&str] {
        &["?"]
    }
    fn description(&self) -> &str {
        "Show help and available commands"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.overlay = None;
        ctx.state.overlay_state = Some(overlay::help_overlay(build_help_content(ctx.state)));
        SlashOutcome::Handled
    }
}

/// Build the `/help` catalog from the live slash registry — single source of
/// truth (replaces the old hardcoded `HELP_CONTENT`). Lists every command with
/// its aliases, usage, and description.
fn build_help_content(state: &crate::tui::app::AppState) -> String {
    let mut entries = state.slash_registry.complete_command("");
    entries.sort_by(|a, b| a.display.cmp(&b.display));
    let mut out = String::from(" Slash Commands\n\n");
    for e in entries {
        out.push_str(&format!("  {:<16} {}\n", e.display, e.description));
    }
    out.push_str("\n Keys\n  Enter  Send\n  Ctrl+C Interrupt / Quit\n  /  Slash commands");
    out
}

/// `/hotkeys` — show keyboard shortcuts. Alias: `keys`.
pub(crate) struct HotkeysCommand;

impl SlashCommand for HotkeysCommand {
    fn name(&self) -> &str {
        "hotkeys"
    }
    fn aliases(&self) -> &[&str] {
        &["keys"]
    }
    fn description(&self) -> &str {
        "Show all keyboard shortcuts (alias: /keys)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.overlay = None;
        ctx.state.overlay_state = Some(overlay::hotkeys_overlay());
        SlashOutcome::Handled
    }
}

/// `/extensions` — list extensions & WASM tools. Alias: `ext`.
pub(crate) struct ExtensionsCommand;

impl SlashCommand for ExtensionsCommand {
    fn name(&self) -> &str {
        "extensions"
    }
    fn aliases(&self) -> &[&str] {
        &["ext"]
    }
    fn description(&self) -> &str {
        "List extensions & WASM tools (alias: /ext)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.overlay = None;
        ctx.state.overlay_state = Some(overlay::extensions_overlay(ctx.session, ctx.state));
        SlashOutcome::Handled
    }
}

/// `/changelog` — show changelog entries.
pub(crate) struct ChangelogCommand;

impl SlashCommand for ChangelogCommand {
    fn name(&self) -> &str {
        "changelog"
    }
    fn description(&self) -> &str {
        "Show changelog entries"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let paths = vec![
            PathBuf::from("CHANGELOG.md"),
            PathBuf::from("../CHANGELOG.md"),
        ];
        let mut entries: Vec<crate::ui::changelog::ChangelogEntry> = Vec::new();
        for path in &paths {
            let parsed = crate::ui::changelog::parse_changelog(path);
            if !parsed.is_empty() {
                entries = parsed;
                break;
            }
        }
        if entries.is_empty() {
            ctx.state
                .add_notification("No changelog found".to_string(), NotificationKind::Info);
        } else {
            let changelog_entries: Vec<(String, String)> = entries
                .iter()
                .take(10)
                .map(|e| (e.version_string(), e.content.clone()))
                .collect();
            ctx.state.overlay = None;
            ctx.state.overlay_state = Some(overlay::changelog_overlay(changelog_entries));
        }
        SlashOutcome::Handled
    }
}
