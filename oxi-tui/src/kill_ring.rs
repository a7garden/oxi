//! Kill ring (clipboard history) for the editor.
//!
//! Provides a circular buffer of killed (cut/copied) text entries that can be
//! yanked (pasted) back, with support for cycling through history via yank-pop.

/// Default maximum number of entries in the kill ring.
const DEFAULT_CAPACITY: usize = 10;

/// A kill ring stores a history of killed (cut) text entries.
///
/// Entries are stored newest-first. `yank` returns the most recent entry,
/// and `yank_pop` cycles through older entries.
pub struct KillRing {
    entries: Vec<String>,
    capacity: usize,
    /// Index for yank-pop cycling; `None` when no yank is in progress.
    yank_index: Option<usize>,
}

impl KillRing {
    /// Create a new kill ring with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity: capacity.max(1),
            yank_index: None,
        }
    }

    /// Create a new kill ring with the default capacity (10).
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Push a new entry onto the kill ring.
    ///
    /// If the ring is at capacity, the oldest entry is dropped.
    /// Duplicate consecutive entries are ignored.
    pub fn push(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        // Skip if identical to the most recent entry
        if self.entries.first() == Some(&text) {
            return;
        }
        if self.entries.len() >= self.capacity {
            self.entries.pop();
        }
        self.entries.insert(0, text);
        // Reset yank state on new push
        self.yank_index = None;
    }

    /// Yank (paste) the most recent entry.
    ///
    /// Returns `None` if the kill ring is empty.
    /// Starts a yank session that can be continued with `yank_pop`.
    pub fn yank(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            self.yank_index = None;
            return None;
        }
        self.yank_index = Some(0);
        Some(&self.entries[0])
    }

    /// Yank-pop: cycle to the next older entry in the kill ring.
    ///
    /// Must be called after `yank()`. Returns `None` if no yank session is
    /// active or the ring is empty. Wraps around to the newest entry after
    /// reaching the oldest.
    pub fn yank_pop(&mut self) -> Option<&str> {
        let idx = self.yank_index?;
        if self.entries.is_empty() {
            self.yank_index = None;
            return None;
        }
        let next = (idx + 1) % self.entries.len();
        self.yank_index = Some(next);
        Some(&self.entries[next])
    }

    /// Get the number of entries currently in the kill ring.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the kill ring is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the current yank index, if a yank session is active.
    pub fn yank_index(&self) -> Option<usize> {
        self.yank_index
    }

    /// Clear all entries and reset yank state.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.yank_index = None;
    }
}

impl Default for KillRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_yank() {
        let mut ring = KillRing::new();
        assert!(ring.yank().is_none());

        ring.push("first".to_string());
        ring.push("second".to_string());

        assert_eq!(ring.yank(), Some("second"));
        assert_eq!(ring.len(), 2);
    }

    #[test]
    fn test_yank_pop_cycles() {
        let mut ring = KillRing::new();
        ring.push("a".to_string());
        ring.push("b".to_string());
        ring.push("c".to_string());

        // yank returns newest
        assert_eq!(ring.yank(), Some("c"));
        // yank_pop cycles through older entries
        assert_eq!(ring.yank_pop(), Some("b"));
        assert_eq!(ring.yank_pop(), Some("a"));
        // wraps around
        assert_eq!(ring.yank_pop(), Some("c"));
    }

    #[test]
    fn test_capacity_limit() {
        let mut ring = KillRing::with_capacity(3);
        ring.push("1".to_string());
        ring.push("2".to_string());
        ring.push("3".to_string());
        ring.push("4".to_string()); // should evict "1"

        assert_eq!(ring.len(), 3);
        assert_eq!(ring.yank(), Some("4"));
        // Verify "1" is gone by cycling
        assert_eq!(ring.yank_pop(), Some("3"));
        assert_eq!(ring.yank_pop(), Some("2"));
        assert_eq!(ring.yank_pop(), Some("4")); // wraps
    }

    #[test]
    fn test_duplicate_consecutive_ignored() {
        let mut ring = KillRing::new();
        ring.push("hello".to_string());
        ring.push("hello".to_string()); // duplicate, ignored

        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn test_empty_push_ignored() {
        let mut ring = KillRing::new();
        ring.push(String::new());
        assert!(ring.is_empty());
    }

    #[test]
    fn test_clear() {
        let mut ring = KillRing::new();
        ring.push("x".to_string());
        ring.yank();
        ring.clear();
        assert!(ring.is_empty());
        assert!(ring.yank_index().is_none());
    }

    #[test]
    fn test_yank_pop_without_yank_returns_none() {
        let mut ring = KillRing::new();
        ring.push("data".to_string());
        assert!(ring.yank_pop().is_none());
    }

    #[test]
    fn test_push_resets_yank_index() {
        let mut ring = KillRing::new();
        ring.push("a".to_string());
        ring.push("b".to_string());
        ring.yank();
        assert!(ring.yank_index().is_some());

        ring.push("c".to_string());
        assert!(ring.yank_index().is_none());
    }
}
