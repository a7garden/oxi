//! Layout system — responsive mode + agent view geometry.
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`agent`] | `AgentViewLayout`, `ActivePane`, `PaneAreas` — pure geometry |
//! | [`config`] | `LayoutConfig`, `ScrollbarConfig` — appearance settings |
//! | [`shortcuts_bar`] | `ShortcutsBar` + `HintItem` — bottom keyboard hints |
//! | [`welcome`] | `WelcomeLayout` — welcome screen geometry |
//!
//! The existing [`LayoutMode`] (ported from `vtcode-ui`) provides responsive
//! breakpoints.  The new `AgentViewLayout` provides the grok-build-style
//! pure-compute layout engine for the agent conversation view.

pub mod agent;
pub mod config;
pub mod shortcuts_bar;
pub mod welcome;

// ───────────────────────────────────────────────────────────────────────────
// Re-exports
// ───────────────────────────────────────────────────────────────────────────

pub use agent::{
    AUTO_COMPACT_MAX_ROWS, ActivePane, AgentViewLayout, LayoutInput, PaneAreas,
    SHORT_TERMINAL_ROWS, effective_compact,
};
pub use config::{LayoutConfig, ScrollbarConfig};
pub use shortcuts_bar::{
    CompactConfig, HintItem, PendingHint, ShortcutBarStyling, ShortcutsBar, compute_effective_hints,
};

pub use welcome::{HERO_BOX_MIN_WIDTH, PROMPT_HEIGHT, WelcomeLayout, WelcomePromptFocus};

// ───────────────────────────────────────────────────────────────────────────
// LayoutMode (preserved from the original vtcode-ui port)
// ───────────────────────────────────────────────────────────────────────────

use ratatui::layout::Rect;

use crate::design::constants::{COMPACT_MAX_COLS, COMPACT_MAX_ROWS, WIDE_MIN_COLS, WIDE_MIN_ROWS};

/// Responsive layout mode based on terminal dimensions.
///
/// This enum provides a single source of truth for layout decisions
/// across the UI, enabling consistent responsive behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    /// Minimal chrome for tiny terminals (< 80 cols or < 20 rows)
    Compact,
    /// Default layout for standard terminals
    Standard,
    /// Enhanced layout with sidebar for wide terminals (>= 120 cols, >= 24 rows)
    Wide,
}

impl LayoutMode {
    /// Determine layout mode from viewport dimensions.
    pub(crate) fn from_area(area: Rect) -> Self {
        if area.width <= COMPACT_MAX_COLS || area.height <= COMPACT_MAX_ROWS {
            LayoutMode::Compact
        } else if area.width >= WIDE_MIN_COLS && area.height >= WIDE_MIN_ROWS {
            LayoutMode::Wide
        } else {
            LayoutMode::Standard
        }
    }

    /// Check if borders should be shown.
    pub(crate) fn show_borders(self) -> bool {
        !matches!(self, LayoutMode::Compact)
    }

    /// Check if panel titles should be shown.
    pub(crate) fn show_titles(self) -> bool {
        !matches!(self, LayoutMode::Compact)
    }

    /// Check if sidebar can be shown.
    pub(crate) fn allow_sidebar(self) -> bool {
        matches!(self, LayoutMode::Wide)
    }

    /// Check if logs panel should be visible.
    pub(crate) fn show_logs_panel(self) -> bool {
        !matches!(self, LayoutMode::Compact)
    }

    /// Get the footer height for this mode.
    pub(crate) fn footer_height(self) -> u16 {
        0
    }

    /// Check if footer should be shown.
    pub(crate) fn show_footer(self) -> bool {
        false
    }

    /// Get the maximum header height as percentage of viewport.
    pub(crate) fn max_header_percent(self) -> f32 {
        match self {
            LayoutMode::Compact => 0.2,
            _ => 0.3,
        }
    }

    /// Get the sidebar width percentage (only meaningful in Wide mode).
    pub(crate) fn sidebar_width_percent(self) -> u16 {
        match self {
            LayoutMode::Wide => 25,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_mode_for_small_terminals() {
        assert_eq!(
            LayoutMode::from_area(Rect::new(0, 0, 60, 20)),
            LayoutMode::Compact
        );
        assert_eq!(
            LayoutMode::from_area(Rect::new(0, 0, 80, 15)),
            LayoutMode::Compact
        );
    }

    #[test]
    fn standard_mode_for_normal_terminals() {
        assert_eq!(
            LayoutMode::from_area(Rect::new(0, 0, 100, 22)),
            LayoutMode::Standard
        );
    }

    #[test]
    fn wide_mode_for_large_terminals() {
        assert_eq!(
            LayoutMode::from_area(Rect::new(0, 0, 140, 30)),
            LayoutMode::Wide
        );
    }
}
