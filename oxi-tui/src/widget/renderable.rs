//! The widget trait. Every UI element (chat view, message, footer, scrollbar)
//! implements `Renderable`.
//!
//! ## Memoization contract
//!
//! `content_hash()` MUST change whenever the widget's rendered output would
//! change. The pipeline (spec §4.2) calls `content_hash()` first; if it
//! matches the previous frame's hash, `render()` is not called.
//!
//! For widgets with children, `content_hash` aggregates child hashes (e.g.
//! via `hash_combine`). A child change propagates to parent, which propagates
//! to root, which trips `RetainedTree::any_hash_changed`.
//!
//! ## `height_for` contract
//!
//! Must be cheap (no rendering). Used for scrollback virtualization —
//! off-screen widgets are never `render()`-ed.

use ratatui::layout::Rect;

use crate::widget::RenderCtx;

pub trait Renderable {
    /// Hash of the widget's content. Change this when output would change.
    /// Must be deterministic and cheap.
    fn content_hash(&self) -> u64;

    /// Height this widget will occupy at the given width. Used by parents
    /// to lay out children, and by scrollback virtualization to skip
    /// off-screen widgets entirely.
    fn height_for(&self, width: u16, ctx: &RenderCtx) -> u16;

    /// Paint into `area` of `ctx`'s buffer. Only called when `content_hash`
    /// changed since the last frame (or on first frame, or after resize).
    fn render(&mut self, area: Rect, ctx: &mut RenderCtx);
}

/// Stable hash combine (Fowler–Noll–Vo 1a variant in u64). Use to aggregate
/// child hashes into parent. Same inputs → same output (deterministic).
#[must_use]
pub fn hash_combine(a: u64, b: u64) -> u64 {
    // FNV-1a 64-bit
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = a ^ b;
    h = h.wrapping_mul(FNV_PRIME) ^ (h >> 31);
    h ^ FNV_OFFSET
}

/// Hash a `&str`. Use for widget fields that affect rendering.
#[must_use]
pub fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_combine_is_deterministic() {
        let h1 = hash_combine(12345, 67890);
        let h2 = hash_combine(12345, 67890);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_combine_differs_on_different_inputs() {
        let h1 = hash_combine(12345, 67890);
        let h2 = hash_combine(12345, 67891);
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_str_is_deterministic() {
        assert_eq!(hash_str("hello"), hash_str("hello"));
        assert_ne!(hash_str("hello"), hash_str("world"));
    }
}
