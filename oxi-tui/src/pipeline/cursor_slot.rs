//! Tri-state cursor slot — distinguishes "widget did not touch cursor"
//! from "widget explicitly showed/hid cursor".
//!
//! Without tri-state, `Option<Position>` cannot tell apart:
//! - hash-skipped widget that never called `set_cursor` (should fall back to last cursor)
//! - widget that explicitly called `hide_cursor` (authoritative — should propagate)
//!
//! See spec §5.2 "Cursor 폴백의 필요성".

use ratatui::layout::Position;

/// What a widget did to the cursor during this frame's `render()`.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorSlot {
    /// Widget did not touch the cursor (hash-skipped or doesn't manage cursor).
    /// `RetainedTree::render` falls back to `last_cursor`.
    #[default]
    NotSet,
    /// Widget explicitly showed cursor at `position`. Authoritative.
    Show(Position),
    /// Widget explicitly hid cursor. Authoritative — overrides `last_cursor` fallback.
    Hide,
}

impl CursorSlot {
    /// Resolve to `Option<Position>` given the previous cursor. `NotSet` falls back.
    #[must_use]
    pub fn resolve(self, last_cursor: Option<Position>) -> Option<Position> {
        match self {
            CursorSlot::NotSet => last_cursor,
            CursorSlot::Show(p) => Some(p),
            CursorSlot::Hide => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P1: Position = Position { x: 1, y: 1 };
    const P2: Position = Position { x: 2, y: 2 };

    #[test]
    fn notset_falls_back_to_last_cursor() {
        assert_eq!(CursorSlot::NotSet.resolve(Some(P1)), Some(P1));
        assert_eq!(CursorSlot::NotSet.resolve(None), None);
    }

    #[test]
    fn show_is_authoritative_over_last_cursor() {
        assert_eq!(CursorSlot::Show(P2).resolve(Some(P1)), Some(P2));
        assert_eq!(CursorSlot::Show(P2).resolve(None), Some(P2));
    }

    #[test]
    fn hide_is_authoritative_over_last_cursor() {
        // ★ critical: Hide must NOT be clobbered by fallback (the bug class we're fixing)
        assert_eq!(CursorSlot::Hide.resolve(Some(P1)), None);
        assert_eq!(CursorSlot::Hide.resolve(None), None);
    }

    #[test]
    fn default_is_notset() {
        assert_eq!(CursorSlot::default(), CursorSlot::NotSet);
    }
}
