//! TUI interactive mode — module structure.
//!
//! Provides a flicker-free terminal chat interface using ratatui.

mod app;
mod completion;
mod handlers;
mod overlay;
mod render;
mod slash;
mod welcome;
pub mod v2_bridge;
mod v2_overlay_adapter;

pub use app::run_tui_interactive;
pub use app::run_tui_interactive_with_continue;
