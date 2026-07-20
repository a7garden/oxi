// oxi-pager — pager state machine for the oxi-cli TUI.

#![forbid(unsafe_code)]

pub mod dispatch;
pub mod emitter;
pub mod keymap;
pub mod main_loop;
pub mod modal;
pub mod prompt;
pub mod reducer;
pub mod render;
pub mod scrollback;
pub mod slash;
pub mod state;
pub mod status;
pub mod theme_bridge;

pub use emitter::{BackgroundEvent, PagerEvent, ResolvedKey};
pub use keymap::{FocusTarget, KeyRouter, ModalInput};
pub use main_loop::run;
pub use prompt::PromptState;
pub use reducer::{
    reduce, AgentCmd, ExitReason, ModalCtx, PagerAction, Sound, TermCmd,
};
pub use scrollback::ScrollbackState;
pub use state::{ModalKind, PagerState, SharedState, StickyPanelState};
pub use status::StatusState;

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
