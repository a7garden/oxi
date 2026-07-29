//! TUI interactive mode — module structure.
//!
//! Provides a flicker-free terminal chat interface using ratatui.
mod app;
mod completion;
mod handlers;
mod overlay;
mod render;
pub(crate) mod slash;
mod tape_render;
mod terminal_host;
mod welcome;

pub use app::run_tui_interactive;
pub use app::run_tui_interactive_with_continue;
