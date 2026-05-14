//! Overlay component trait and shared types.
//!
//! Each overlay (settings panel, model selector, etc.) implements
//! `OverlayComponent` to encapsulate its own state, event handling, and rendering.
//! This follows ratatui's StatefulWidget philosophy at the overlay level.

mod factories;

use crossterm::event::KeyEvent;
use ratatui::{Frame, layout::Rect};

use oxi_tui::Theme;

pub use factories::{logout_select, model_select, resume_select};

// ---------------------------------------------------------------------------
// Overlay action
// ---------------------------------------------------------------------------

/// Actions an overlay can request after handling a key event.
#[derive(Debug)]
pub enum OverlayAction {
    /// No action needed.
    None,
    /// Close the overlay.
    Close,
    /// Send a user prompt.
    SendPrompt(String),
    /// Switch to a different session.
    SwitchSession(String),
    /// Start a new session.
    NewSession,
    /// Execute a slash command by name.
    ExecuteSlashCommand(String),
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait for self-contained overlay components.
///
/// Each overlay owns its state, handles its own key events, and renders itself.
/// The app only needs to dispatch — no match sprawl.
pub trait OverlayComponent: std::fmt::Debug {
    /// Handle a key press. Return an action if the app needs to do something.
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction;

    /// Render the overlay into the given area.
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme);

    /// Footer hint text for this overlay.
    fn hint(&self) -> &str;
}