//! Slash command handling.
//!
//! Two layers coexist during the refactor:
//! - The legacy `handle_slash_command()` match (being migrated out).
//! - The new `SlashCommand` trait + `SlashRegistry` (see `registry.rs` and
//!   `builtin/`). New commands should be added as `SlashCommand` impls.

// ── New registry layer (migration target) ───────────────────────────────
//
// These types are introduced in step 1 of the slash-command-registry design
// (docs/designs/2026-06-17-slash-command-registry.md). They compile but are
// not yet wired into dispatch/completion until later steps. Suppressed lints
// are removed as each type gains its first real use.

/// Everything a `SlashCommand::execute` call needs, bundled so that adding a
/// dependency does not change every command's signature.
#[allow(dead_code)] // first use in step 2
pub(crate) struct SlashCtx<'a> {
    pub session: &'a AgentSession,
    pub state: &'a mut AppState,
    pub ui_tx: &'a mpsc::UnboundedSender<UiEvent>,
}

/// Result of executing a slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // wired in step 2+
pub(crate) enum SlashOutcome {
    /// Command handled; state already mutated.
    Handled,
    /// Did not match — fall through to unknown.
    NotHandled,
    /// Request application shutdown (`/quit`).
    Quit,
}

/// Completion entry derived from the registry. Replaces `SlashCompletion`.
#[derive(Debug, Clone)]
#[allow(dead_code)] // wired in step 4 (completion unification)
pub(crate) struct CompletionEntry {
    /// Display form, e.g. `/ext` (alias as typed).
    pub display: String,
    /// Canonical name, e.g. `/extensions`.
    pub canonical: String,
    /// Short description shown in the popup.
    pub description: String,
    /// Whether this command comes from a WASM/native extension.
    pub is_extension: bool,
}

mod builtin;
pub(crate) mod registry;

// ── Legacy layer (migration source) ──────────────────────────────────────

use super::app::{AppState, NotificationKind, UiEvent};
use crate::app::agent_session::AgentSession;
use tokio::sync::mpsc;

/// A slash command completion entry.
pub(crate) struct SlashCompletion {
    pub name: String,
    pub description: String,
    /// Whether this is an argument completion (Tab fills the input rather
    /// than executing). False for command-name completions.
    pub is_arg: bool,
}

/// Handle a slash command. Returns `true` if handled.
pub(crate) fn handle_slash_command(
    input: &str,
    session: &AgentSession,
    state: &mut AppState,
    running: &mut bool,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
) -> bool {
    let trimmed = input.trim();
    let cmd = if let Some(space) = trimmed.find(' ') {
        &trimmed[..space]
    } else {
        trimmed
    };

    // ── New registry layer ────────────────────────────────────────────
    // All built-in commands dispatch through the registry. Anything returning
    // `NotHandled` falls through to the unknown-command notice below.
    //
    // The registry is built locally per call: it is immutable and cheap to
    // assemble, and building it locally avoids a borrow conflict between
    // `state.slash_registry` (shared read) and the `&mut AppState` carried in
    // the `SlashCtx`. Completion (step 4) reads the `state.slash_registry`
    // field, which is read-only there.
    {
        let mut registry = registry::SlashRegistry::builtins();
        registry.sync_extensions(state.wasm_ext.as_ref());
        let mut ctx = SlashCtx {
            session,
            state,
            ui_tx,
        };
        match registry.dispatch(input, &mut ctx) {
            SlashOutcome::Handled => return true,
            SlashOutcome::Quit => {
                *running = false;
                return true;
            }
            SlashOutcome::NotHandled => {} // fall through to unknown
        }
    }

    state.add_notification(
        format!("Unknown command: {}", cmd),
        NotificationKind::Warning,
    );
    false
}
