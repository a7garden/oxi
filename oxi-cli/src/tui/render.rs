//! Overlay rendering for the TUI.

use super::app::AppState;
use oxi_tui::theme::Theme;
use ratatui::{Frame, layout::Rect};

// ── Main draw function ──────────────────────────────────────────────────

/// Overlay draw function — renders only modal overlays in alternate screen.
pub fn draw(f: &mut Frame, state: &mut AppState, theme: &Theme) {
    let size = f.area();
    if state.overlay.is_some() || state.overlay_state.is_some() {
        render_overlay(f, size, state, theme);
    }
}

fn render_overlay(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    // ── Component-based overlay ──
    // All overlays have been migrated to `Box<dyn OverlayComponent>`.
    if let Some(ref mut overlay) = state.overlay_state {
        overlay.render(f, area, theme);
    }
}
