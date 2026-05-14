//! Overlay component trait and shared types.
//!
//! Each overlay (model selector, logout, resume) implements `OverlayComponent`
//! to encapsulate its own state, event handling, and rendering.
//! This follows ratatui's StatefulWidget philosophy at the overlay level.

use crossterm::event::KeyEvent;
use ratatui::{Frame, layout::Rect};
use oxi_tui::Theme;

pub mod factories;
pub mod questionnaire;
pub use factories::{logout_select, model_select, resume_select};

// ---------------------------------------------------------------------------
// Overlay action
// ---------------------------------------------------------------------------

/// Actions an overlay can request after handling a key event.
#[derive(Debug, Clone)]
pub enum OverlayAction {
    /// No action needed.
    None,
    /// Close the overlay.
    Close,
    /// Switch to a different session.
    SwitchSession(String),
    /// Start a new session.
    NewSession,
    /// Execute a slash command by name.
    ExecuteSlashCommand(String),
    /// Send a user prompt.
    SendPrompt(String),
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait for self-contained overlay components.
///
/// Each overlay owns its state, handles its own key events, and renders itself.
/// The app only needs to dispatch — no match sprawl in handlers/render.
pub trait OverlayComponent: std::fmt::Debug {
    /// Handle a key press. Return an action if the app needs to do something.
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction;

    /// Render the overlay into the given area.
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme);

    /// Footer hint text for this overlay.
    fn hint(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Helper to center a popup in an area with given size ratios.
pub fn centered_popup(area: Rect, width_pct: f32, height_pct: f32) -> Rect {
    let w = (area.width as f32 * width_pct) as u16;
    let h = (area.height as f32 * height_pct) as u16;
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}