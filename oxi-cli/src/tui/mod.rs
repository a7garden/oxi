//! TUI interactive mode — module structure.
//!
//! Provides a flicker-free terminal chat interface using ratatui.

mod app;
mod handlers;
mod render;
mod slash;
mod welcome;

pub use app::run_tui_interactive;
