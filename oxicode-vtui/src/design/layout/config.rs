//! Layout configuration — appearance settings that drive the pane geometry.
//!
//! Pure data — the actual geometry computation lives in
//! [`super::agent::AgentViewLayout::compute`].

/// Horizontal + vertical padding configuration for the agent view.
///
/// All values are in terminal cells.  The `eff_*` methods fold in the compact
/// flag so callers pass one boolean instead of replicating the conditional
/// everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutConfig {
    /// Left horizontal padding (normal mode).
    pub hpad_left: u16,
    /// Right horizontal padding (normal mode).
    pub hpad_right: u16,
    /// Left horizontal padding (compact mode).
    pub hpad_left_compact: u16,
    /// Right horizontal padding (compact mode).
    pub hpad_right_compact: u16,
    /// Outer vertical padding (normal mode).
    pub outer_vpad: u16,
    /// Outer vertical padding (compact mode — usually 0).
    pub outer_vpad_compact: u16,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            hpad_left: 2,
            hpad_right: 2,
            hpad_left_compact: 1,
            hpad_right_compact: 1,
            outer_vpad: 1,
            outer_vpad_compact: 0,
        }
    }
}

impl LayoutConfig {
    /// Effective left horizontal padding given the compact flag.
    #[must_use]
    pub fn eff_hpad_left(&self, compact: bool) -> u16 {
        if compact {
            self.hpad_left_compact
        } else {
            self.hpad_left
        }
    }

    /// Effective right horizontal padding given the compact flag.
    #[must_use]
    pub fn eff_hpad_right(&self, compact: bool) -> u16 {
        if compact {
            self.hpad_right_compact
        } else {
            self.hpad_right
        }
    }

    /// Effective outer vertical padding given the compact flag.
    #[must_use]
    pub fn eff_outer_vpad(&self, compact: bool) -> u16 {
        if compact {
            self.outer_vpad_compact
        } else {
            self.outer_vpad
        }
    }
}

/// Scrollbar appearance configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarConfig {
    /// Whether the scrollbar is enabled.
    pub enabled: bool,
    /// Columns of gap between the scrollbar and content (left side).
    pub gap_left: u16,
    /// Columns of gap between the scrollbar and the screen edge (right side).
    pub gap_right: u16,
}

impl Default for ScrollbarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gap_left: 1,
            gap_right: 1,
        }
    }
}
