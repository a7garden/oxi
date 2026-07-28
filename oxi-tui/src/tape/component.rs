//! Component trait — omp `Component` interface translated to Rust.
//!
//! omp uses TypeScript reference identity for memoization: an unchanged
//! component returns the same array reference. Rust has no GC reference
//! identity, so we use a `content_hash: u64` field instead. A Container
//! compares child hashes to skip unchanged subtrees.

use std::hash::{Hash, Hasher};

/// The result of rendering a component at a given width.
///
/// `hash` is a content hash of `lines` — two `RenderResult`s with the same
/// hash are guaranteed to have identical `lines`. Containers use this for
/// memoization: if a child's hash is unchanged, its lines are reused without
/// re-concatenation.
#[derive(Clone, Debug)]
pub struct RenderResult {
    /// Rendered ANSI lines (one per terminal row).
    pub lines: Vec<String>,
    /// Content hash of `lines` for memoization.
    pub hash: u64,
}

impl RenderResult {
    /// Create from lines, computing a default hash.
    #[must_use]
    pub fn new(lines: Vec<String>) -> Self {
        let hash = hash_lines(&lines);
        Self { lines, hash }
    }

    /// Create from a single line.
    #[must_use]
    pub fn one(line: impl Into<String>) -> Self {
        Self::new(vec![line.into()])
    }

    /// Empty render result.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}

impl Default for RenderResult {
    fn default() -> Self {
        Self::empty()
    }
}

/// Compute a stable hash over a slice of lines.
fn hash_lines(lines: &[String]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lines.len().hash(&mut hasher);
    for line in lines {
        line.hash(&mut hasher);
    }
    hasher.finish()
}

/// Rendering component — omp `Component` interface translated to Rust.
///
/// Implementors render to a `Vec<String>` of ANSI-escaped terminal lines at
/// the given width. The result includes a content hash for memoization:
/// unchanged components should return the same hash (and ideally the same
/// `Vec` content), so containers can skip re-concatenation.
pub trait Component: Send {
    /// Render to lines at the given width.
    fn render(&self, width: u16) -> RenderResult;

    /// The mutable suffix starts at this line index (within the component's
    /// own rendered lines). Rows above are FINAL — byte-stable at the current
    /// width — and commit to native scrollback. Rows at/after the boundary
    /// repaint in place inside the visible window.
    ///
    /// `None` means the entire component is final (shell semantics).
    /// This is the omp `NativeScrollbackLiveRegion` interface.
    fn live_region(&self) -> LiveRegion {
        LiveRegion::None
    }

    /// Optional: invalidate cached rendering state (theme change, etc.).
    fn invalidate(&mut self) {}
}

/// Live region report — where the mutable suffix begins.
///
/// Translates omp's `NativeScrollbackLiveRegion` interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LiveRegion {
    /// Entire component is final — all rows commit to scrollback.
    #[default]
    None,
    /// Mutable suffix starts at `start` (0-indexed within the component's lines).
    /// Rows `[0, start)` are final; rows `[start, end)` repaint in place.
    Mutable {
        /// 0-indexed line where the mutable suffix begins.
        start: usize,
    },
    /// Viewport-pinned: offscreen mutable rows are virtually clipped until the
    /// boundary advances. Use for fixed-height dashboards whose frames replace
    /// each other rather than append.
    Pinned {
        /// 0-indexed line where the pinned region begins.
        start: usize,
    },
}

impl LiveRegion {
    /// Returns the mutable suffix start index, if any.
    #[must_use]
    pub fn start(&self) -> Option<usize> {
        match self {
            LiveRegion::None => None,
            LiveRegion::Mutable { start } | LiveRegion::Pinned { start } => Some(*start),
        }
    }

    /// Whether this region is viewport-pinned.
    #[must_use]
    pub fn is_pinned(&self) -> bool {
        matches!(self, LiveRegion::Pinned { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple text component for testing.
    struct TextComponent {
        text: String,
    }

    impl Component for TextComponent {
        fn render(&self, _width: u16) -> RenderResult {
            RenderResult::new(vec![self.text.clone()])
        }
    }

    #[test]
    fn render_result_hash_stability() {
        let r1 = RenderResult::new(vec!["hello".into(), "world".into()]);
        let r2 = RenderResult::new(vec!["hello".into(), "world".into()]);
        assert_eq!(r1.hash, r2.hash);
    }

    #[test]
    fn render_result_hash_differs_on_change() {
        let r1 = RenderResult::new(vec!["hello".into()]);
        let r2 = RenderResult::new(vec!["world".into()]);
        assert_ne!(r1.hash, r2.hash);
    }

    #[test]
    fn render_result_hash_differs_on_length() {
        let r1 = RenderResult::new(vec!["hello".into()]);
        let r2 = RenderResult::new(vec!["hello".into(), "world".into()]);
        assert_ne!(r1.hash, r2.hash);
    }

    #[test]
    fn text_component_renders() {
        let c = TextComponent { text: "hi".into() };
        let r = c.render(80);
        assert_eq!(r.lines, vec!["hi"]);
    }

    #[test]
    fn default_live_region_is_none() {
        let c = TextComponent { text: "hi".into() };
        assert_eq!(c.live_region(), LiveRegion::None);
    }

    #[test]
    fn live_region_start() {
        assert_eq!(LiveRegion::None.start(), None);
        assert_eq!(LiveRegion::Mutable { start: 5 }.start(), Some(5));
        assert_eq!(LiveRegion::Pinned { start: 3 }.start(), Some(3));
    }

    #[test]
    fn live_region_is_pinned() {
        assert!(!LiveRegion::None.is_pinned());
        assert!(!LiveRegion::Mutable { start: 0 }.is_pinned());
        assert!(LiveRegion::Pinned { start: 0 }.is_pinned());
    }
}
