//! Streaming message component — a message being actively received.
//!
//! The finalized portion (already received text) is FINAL (commits to
//! scrollback). The live portion (currently streaming) is mutable and
//! repaints in-place. This is the omp `NativeScrollbackLiveRegion` pattern:
//! when a paragraph completes, its lines are promoted to finalized, and only
//! the active paragraph stays in the live region.

use super::super::component::{Component, LiveRegion, RenderResult};

/// A message being actively streamed from the LLM.
///
/// Tracks which lines are finalized (byte-stable, commit to scrollback) vs
/// live (mutable, repaint in-place). The boundary advances as paragraphs
/// complete.
pub struct StreamingMessage {
    /// Finalized lines — won't change, commit to scrollback.
    finalized: Vec<String>,
    /// Live lines — may still change, repaint in-place.
    live: Vec<String>,
    /// O(1) cache revision, bumped by every output-affecting mutation.
    revision: u64,
}

impl StreamingMessage {
    /// Create an empty streaming message.
    #[must_use]
    pub fn new() -> Self {
        Self {
            finalized: Vec::new(),
            live: Vec::new(),
            revision: 0,
        }
    }

    /// Start from pre-finalized lines (e.g. system prompt, existing context).
    #[must_use]
    pub fn with_finalized(finalized: Vec<String>) -> Self {
        let mut msg = Self::new();
        msg.finalized = finalized;
        msg.bump_revision();
        msg
    }

    /// Append a line to the finalized portion.
    /// Moves the line past the live region boundary.
    pub fn append_finalized(&mut self, line: impl Into<String>) {
        self.finalized.push(line.into());
        self.bump_revision();
    }

    /// Promote all live lines to finalized. The live region becomes empty.
    pub fn finalize_live(&mut self) {
        if !self.live.is_empty() {
            self.finalized.append(&mut self.live);
            self.bump_revision();
        }
    }

    /// Set the live (mutable) portion. Replaces any existing live lines.
    pub fn set_live(&mut self, lines: Vec<String>) {
        if self.live != lines {
            self.live = lines;
            self.bump_revision();
        }
    }

    /// Append text to the last live line, or create a new live line.
    pub fn append_token(&mut self, token: &str) {
        if self.live.is_empty() {
            self.live.push(token.to_string());
        } else {
            self.live
                .last_mut()
                .expect("checked non-empty")
                .push_str(token);
        }
        self.bump_revision();
    }

    /// Whether the message has a non-empty live region.
    #[must_use]
    pub fn is_streaming(&self) -> bool {
        !self.live.is_empty()
    }

    /// Number of finalized lines.
    #[must_use]
    pub fn finalized_count(&self) -> usize {
        self.finalized.len()
    }

    /// Total line count (finalized + live).
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.finalized.len() + self.live.len()
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

impl Default for StreamingMessage {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for StreamingMessage {
    fn render(&self, _width: u16) -> RenderResult {
        let mut lines = self.finalized.clone();
        lines.extend(self.live.iter().cloned());
        RenderResult::new(lines)
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn invalidate(&mut self) {
        self.bump_revision();
    }

    fn live_region(&self) -> LiveRegion {
        if self.live.is_empty() {
            LiveRegion::None
        } else {
            LiveRegion::Mutable {
                start: self.finalized.len(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_has_no_live_region() {
        let msg = StreamingMessage::new();
        assert_eq!(msg.live_region(), LiveRegion::None);
    }

    #[test]
    fn live_lines_create_mutable_region() {
        let mut msg = StreamingMessage::new();
        msg.append_finalized("line1");
        msg.set_live(vec!["streaming".into()]);
        assert_eq!(msg.live_region(), LiveRegion::Mutable { start: 1 });
    }

    #[test]
    fn render_concatenates_finalized_and_live() {
        let mut msg = StreamingMessage::new();
        msg.append_finalized("done1");
        msg.append_finalized("done2");
        msg.set_live(vec!["live1".into(), "live2".into()]);
        let r = msg.render(80);
        assert_eq!(r.lines, vec!["done1", "done2", "live1", "live2"]);
    }

    #[test]
    fn finalize_live_promotes_all() {
        let mut msg = StreamingMessage::new();
        msg.append_finalized("done");
        msg.set_live(vec!["live".into()]);
        msg.finalize_live();
        assert_eq!(msg.finalized_count(), 2);
        assert!(!msg.is_streaming());
        assert_eq!(msg.live_region(), LiveRegion::None);
    }

    #[test]
    fn append_token_creates_first_live_line() {
        let mut msg = StreamingMessage::new();
        msg.append_token("Hello");
        msg.append_token(" World");
        assert_eq!(msg.live, vec!["Hello World"]);
    }

    #[test]
    fn append_token_appends_to_existing() {
        let mut msg = StreamingMessage::with_finalized(vec!["ctx".into()]);
        msg.set_live(vec!["start".into()]);
        msg.append_token(" end");
        assert_eq!(msg.live, vec!["start end"]);
    }

    #[test]
    fn hash_changes_on_live_update() {
        let mut msg = StreamingMessage::new();
        msg.set_live(vec!["v1".into()]);
        let h1 = msg.render(80).hash;
        msg.set_live(vec!["v2".into()]);
        let h2 = msg.render(80).hash;
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_stable_on_unchanged() {
        let mut msg = StreamingMessage::new();
        msg.append_finalized("a");
        msg.set_live(vec!["b".into()]);
        let h1 = msg.render(80).hash;
        let h2 = msg.render(80).hash;
        assert_eq!(h1, h2);
    }

    #[test]
    fn total_and_finalized_count() {
        let mut msg = StreamingMessage::new();
        msg.append_finalized("a");
        msg.append_finalized("b");
        msg.set_live(vec!["c".into()]);
        assert_eq!(msg.finalized_count(), 2);
        assert_eq!(msg.total_count(), 3);
    }
}
