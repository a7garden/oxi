// oxi-pager — pager state machine for the oxi-cli TUI.
//
// See `docs/superpowers/specs/2026-07-20-grok-pager-redesign.md` for the
// full architecture. This crate is a thin layer between `oxi-agent`
// events and the existing `oxi-tui` widget tree. It does not introduce
// new widgets, new agent semantics, or new public types in either
// dependency.

#![forbid(unsafe_code)]

pub mod dispatch;
pub mod emitter;
pub mod keymap;
pub mod main_loop;
pub mod reducer;
pub mod render;
pub mod state;

pub use emitter::{BackgroundEvent, PagerEvent, ResolvedKey};
pub use keymap::{FocusTarget, KeyRouter, ModalInput};
pub use main_loop::run;
pub use reducer::{
    reduce, AgentCmd, ExitReason, ModalCtx, PagerAction, Sound, TermCmd,
};
pub use state::{
    AgentMetaState, ModalKind, PagerState, PromptState, ScrollbackState, SharedState,
    StickyPanelState, StatusState,
};

/// Returns the crate version (matches `Cargo.toml`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
