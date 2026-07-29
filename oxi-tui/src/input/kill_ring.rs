//! Emacs-style kill ring.
//!
//! When the user deletes text (kills), the deleted text is pushed onto a ring
//! of recent kills. Pasting back (yanking) re-inserts the most recent kill,
//! and "yank-pop" cycles through older entries by rotating the ring.
//!
//! This implementation is intentionally simple — a bounded `VecDeque`-style
//! ring with a configurable capacity and FIFO eviction. It does NOT model the
//! Emacs "consecutive kills merge into one entry" accumulation behavior; if a
//! caller needs that they can build it on top by checking whether the last
//! entry should be merged before calling [`KillRing::kill`].
//!
//! The ring is small (capped at the constructor's `capacity`, typically a few
//! dozen entries) and supports the operations a TUI text editor needs:
//!
//! - [`KillRing::kill`] adds a new entry (oldest evicted if at capacity).
//! - [`KillRing::yank`] returns the most recent entry without changing state.
//! - [`KillRing::yank_pop`] rotates the ring so the next-most-recent entry
//!   becomes the head, then returns it.
//! - [`KillRing::peek_at`] and [`KillRing::len`] expose state for tests/UX.
//!
//! # Examples
//!
//! ```
//! use oxi_tui::input::kill_ring::KillRing;
//!
//! let mut ring = KillRing::new(8);
//! ring.kill("first".to_string());
//! ring.kill("second".to_string());
//!
//! assert_eq!(ring.yank(), Some("second"));
//! assert_eq!(ring.yank_pop(), Some("first"));
//! // Popping wraps the ring: cycle back to "second".
//! assert_eq!(ring.yank_pop(), Some("second"));
//! ```

/// Bounded ring of killed (deleted) text entries.
///
/// Internally a `Vec<String>` whose logical ordering is "head at the end":
/// `yank` returns the last element (most recent kill) and `yank_pop` rotates
/// the last element to the front. Eviction is FIFO when at capacity.
#[derive(Debug, Clone)]
pub struct KillRing {
    /// Entries in chronological order; most-recent at the end.
    entries: Vec<String>,
    /// Maximum number of entries before oldest is evicted on push.
    capacity: usize,
}

impl KillRing {
    /// Create a new empty kill ring with the given capacity.
    ///
    /// `capacity` is clamped to at least 1 (a zero-capacity ring would silently
    /// drop every kill, which is almost certainly a bug). Larger capacities
    /// hold more history for `yank_pop` to cycle through.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
        }
    }

    /// Number of entries currently in the ring.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ring is empty (no kills yet).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add a new entry to the ring.
    ///
    /// If the ring is at capacity, the oldest entry is evicted to make room.
    /// Empty strings are stored as-is — callers that want to skip empty kills
    /// should check themselves before calling this method.
    pub fn kill(&mut self, text: String) {
        if self.entries.len() == self.capacity {
            self.entries.remove(0);
        }
        self.entries.push(text);
    }

    /// Get the most-recently-killed text without changing ring state.
    ///
    /// Returns `None` if the ring is empty.
    #[must_use]
    pub fn yank(&self) -> Option<&str> {
        self.entries.last().map(String::as_str)
    }

    /// Cycle to the next-older entry and return it.
    ///
    /// On each call, the current "most recent" entry is moved to the front of
    /// the ring, so the second call returns the entry that was previously
    /// second-most-recent, and so on. After `len()` calls, the ring returns
    /// to its original ordering (a full rotation).
    ///
    /// Returns `None` if the ring is empty. With a single entry the call is a
    /// no-op (returns `Some` of that one entry).
    pub fn yank_pop(&mut self) -> Option<&str> {
        if self.entries.len() <= 1 {
            return self.entries.last().map(String::as_str);
        }
        let last = self.entries.pop()?;
        self.entries.insert(0, last);
        self.entries.last().map(String::as_str)
    }

    /// Peek at an entry by index from the front (oldest first).
    ///
    /// Index 0 is the oldest kill, index `len()-1` is the most recent. This
    /// is primarily useful for diagnostics / UX ("ring: 3 entries, peek 2 =
    /// 'hello'"); the hot path is `yank`/`yank_pop`.
    #[must_use]
    pub fn peek_at(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ring_yank_returns_none() {
        let ring = KillRing::new(4);
        assert!(ring.is_empty());
        assert_eq!(ring.yank(), None);
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn kill_then_yank_returns_last() {
        let mut ring = KillRing::new(4);
        ring.kill("first".to_string());
        ring.kill("second".to_string());
        ring.kill("third".to_string());
        assert_eq!(ring.yank(), Some("third"));
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn yank_pop_cycles_through_history() {
        let mut ring = KillRing::new(4);
        ring.kill("a".to_string());
        ring.kill("b".to_string());
        ring.kill("c".to_string());

        // Head is "c"; pop moves "c" to front, head becomes "b".
        assert_eq!(ring.yank_pop(), Some("b"));
        // Pop again: "b" to front, head becomes "a".
        assert_eq!(ring.yank_pop(), Some("a"));
        // Pop again: "a" to front, head becomes "c" (full rotation).
        assert_eq!(ring.yank_pop(), Some("c"));
        // Pop again: "c" to front, head becomes "b" again.
        assert_eq!(ring.yank_pop(), Some("b"));
    }

    #[test]
    fn capacity_evicts_oldest() {
        let mut ring = KillRing::new(3);
        ring.kill("a".to_string());
        ring.kill("b".to_string());
        ring.kill("c".to_string());
        ring.kill("d".to_string());
        assert_eq!(ring.len(), 3);
        // "a" was evicted; oldest is "b", newest is "d".
        assert_eq!(ring.peek_at(0), Some("b"));
        assert_eq!(ring.yank(), Some("d"));
    }

    #[test]
    fn capacity_zero_clamps_to_one() {
        let mut ring = KillRing::new(0);
        ring.kill("only".to_string());
        assert_eq!(ring.len(), 1);
        ring.kill("still_only".to_string());
        // Capacity 1 means we evict before pushing the second.
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.yank(), Some("still_only"));
    }

    #[test]
    fn yank_pop_on_single_entry_is_noop() {
        let mut ring = KillRing::new(4);
        ring.kill("only".to_string());
        // First pop returns the only entry.
        assert_eq!(ring.yank_pop(), Some("only"));
        // Second pop also returns it (rotation is a no-op with one entry).
        assert_eq!(ring.yank_pop(), Some("only"));
    }

    #[test]
    fn peek_at_out_of_bounds_returns_none() {
        let mut ring = KillRing::new(4);
        ring.kill("x".to_string());
        assert_eq!(ring.peek_at(0), Some("x"));
        assert_eq!(ring.peek_at(1), None);
    }
}
