//! Text message component — renders a finalized user/assistant message.
//!
//! Implements the `Component` trait for a static text message. The entire
//! message is FINAL (LiveRegion::None) — all rows are byte-stable.

use super::super::component::{Component, LiveRegion, RenderResult};

/// A finalized text message (user or assistant).
///
/// The `lines` field holds pre-rendered ANSI lines. The entire message is
/// final — no live region.
pub struct TextMessage {
    /// Pre-rendered ANSI lines for this message.
    lines: Vec<String>,
    /// Cached content hash.
    hash: u64,
}

impl TextMessage {
    /// Create from pre-rendered lines.
    #[must_use]
    pub fn new(lines: Vec<String>) -> Self {
        let hash = super::super::component::RenderResult::new(lines.clone()).hash;
        Self { lines, hash }
    }

    /// Create from a single line of text.
    #[must_use]
    pub fn single(text: impl Into<String>) -> Self {
        Self::new(vec![text.into()])
    }

    /// Create from multi-line text, splitting on newlines.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self::new(text.lines().map(String::from).collect())
    }

    /// Number of rendered lines.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

impl Component for TextMessage {
    fn render(&self, _width: u16) -> RenderResult {
        RenderResult {
            lines: self.lines.clone(),
            hash: self.hash,
        }
    }

    fn revision(&self) -> u64 {
        self.hash
    }

    fn live_region(&self) -> LiveRegion {
        LiveRegion::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_lines() {
        let msg = TextMessage::new(vec!["line1".into(), "line2".into()]);
        let r = msg.render(80);
        assert_eq!(r.lines, vec!["line1", "line2"]);
    }

    #[test]
    fn single_line() {
        let msg = TextMessage::single("hello");
        let r = msg.render(80);
        assert_eq!(r.lines, vec!["hello"]);
    }

    #[test]
    fn from_multiline_text() {
        let msg = TextMessage::from_text("a\nb\nc");
        let r = msg.render(80);
        assert_eq!(r.lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn hash_stable_across_renders() {
        let msg = TextMessage::new(vec!["x".into()]);
        let r1 = msg.render(80);
        let r2 = msg.render(80);
        assert_eq!(r1.hash, r2.hash);
    }

    #[test]
    fn live_region_is_none() {
        let msg = TextMessage::single("hi");
        assert_eq!(msg.live_region(), LiveRegion::None);
    }

    #[test]
    fn line_count() {
        let msg = TextMessage::new(vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(msg.line_count(), 3);
    }

    #[test]
    fn empty_message() {
        let msg = TextMessage::new(vec![]);
        let r = msg.render(80);
        assert!(r.lines.is_empty());
    }
}
