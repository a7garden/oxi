//! Undo/redo stack for the editor.
//!
//! Provides a generic undo-redo mechanism using a pair of stacks. Pushing a new
//! state clears the redo history (standard behaviour), and undo/redo move
//! states between the two stacks.

/// Maximum number of undo states retained by default.
const DEFAULT_CAPACITY: usize = 100;

/// A generic undo/redo stack.
///
/// Stores snapshots of type `T` so the user can step backwards (undo) and
/// forwards (redo) through the history. Pushing a new state discards any
/// redo entries, matching the convention used by most editors.
pub struct UndoStack<T> {
    /// Past states, newest at the top (end).
    undo: Vec<T>,
    /// Future states (available for redo), newest at the top (end).
    redo: Vec<T>,
    /// Maximum number of states kept in each direction.
    capacity: usize,
}

impl<T: Clone> UndoStack<T> {
    /// Create a new undo stack with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            undo: Vec::with_capacity(capacity),
            redo: Vec::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    /// Create a new undo stack with the default capacity (100).
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Push a new state onto the undo stack, clearing the redo stack.
    ///
    /// If the undo stack is at capacity, the oldest entry is dropped.
    pub fn push(&mut self, state: T) {
        // Clear redo on new push
        self.redo.clear();

        if self.undo.len() >= self.capacity {
            self.undo.remove(0);
        }
        self.undo.push(state);
    }

    /// Undo: pop the most recent state from the undo stack and push it onto
    /// the redo stack.
    ///
    /// Returns a reference to the new current state (the new top of the undo
    /// stack), or `None` if there is nothing to undo.
    pub fn undo(&mut self) -> Option<&T> {
        let state = self.undo.pop()?;
        self.redo.push(state);
        self.undo.last()
    }

    /// Redo: pop the most recent state from the redo stack and push it onto
    /// the undo stack.
    ///
    /// Returns a reference to the restored state, or `None` if there is
    /// nothing to redo.
    pub fn redo(&mut self) -> Option<&T> {
        let state = self.redo.pop()?;
        self.undo.push(state);
        self.undo.last()
    }

    /// Returns `true` if there are states available to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Returns `true` if there are states available to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Get a reference to the current state (top of the undo stack).
    pub fn current(&self) -> Option<&T> {
        self.undo.last()
    }

    /// Get the number of undo entries.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Get the number of redo entries.
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Clear all undo and redo history.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

impl<T: Clone> Default for UndoStack<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_current() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        assert!(stack.current().is_none());

        stack.push(1);
        stack.push(2);
        stack.push(3);

        assert_eq!(stack.current(), Some(&3));
        assert_eq!(stack.undo_len(), 3);
    }

    #[test]
    fn test_undo_redo_basic() {
        let mut stack: UndoStack<&str> = UndoStack::new();
        stack.push("a");
        stack.push("b");
        stack.push("c");

        // Undo: c -> redo, current becomes b
        assert_eq!(stack.undo(), Some(&"b"));
        assert!(stack.can_redo());

        // Redo: restore c
        assert_eq!(stack.redo(), Some(&"c"));
        assert!(!stack.can_redo());
    }

    #[test]
    fn test_can_undo_can_redo() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());

        stack.push(1);
        assert!(stack.can_undo());

        stack.undo();
        assert!(!stack.can_undo());
        assert!(stack.can_redo());
    }

    #[test]
    fn test_push_clears_redo() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        stack.push(1);
        stack.push(2);
        stack.push(3);

        // Undo twice
        stack.undo();
        stack.undo();
        assert_eq!(stack.redo_len(), 2);

        // New push clears redo
        stack.push(99);
        assert!(!stack.can_redo());
        assert_eq!(stack.redo_len(), 0);
    }

    #[test]
    fn test_capacity_limit() {
        let mut stack: UndoStack<i32> = UndoStack::with_capacity(3);
        stack.push(1);
        stack.push(2);
        stack.push(3);
        stack.push(4); // evicts 1

        assert_eq!(stack.undo_len(), 3);
        assert_eq!(stack.current(), Some(&4));

        // Undo all
        assert_eq!(stack.undo(), Some(&3));
        assert_eq!(stack.undo(), Some(&2));
        // One more undo pops 4; now undo stack is empty
        assert_eq!(stack.undo(), None);
    }

    #[test]
    fn test_undo_on_empty_returns_none() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        assert_eq!(stack.undo(), None);
    }

    #[test]
    fn test_redo_on_empty_returns_none() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        assert_eq!(stack.redo(), None);
    }

    #[test]
    fn test_clear() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        stack.push(1);
        stack.push(2);
        stack.undo();
        stack.clear();
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert!(stack.current().is_none());
    }

    #[test]
    fn test_multiple_undo_redo_cycles() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        for i in 0..5 {
            stack.push(i);
        }

        // Undo all the way: each undo pops current and returns new top
        assert_eq!(stack.undo(), Some(&3)); // popped 4
        assert_eq!(stack.undo(), Some(&2)); // popped 3
        assert_eq!(stack.undo(), Some(&1)); // popped 2
        assert_eq!(stack.undo(), Some(&0)); // popped 1
        assert_eq!(stack.undo(), None);     // popped 0, undo stack empty

        // Redo forward: each redo restores from redo stack
        assert_eq!(stack.redo(), Some(&0));
        assert_eq!(stack.redo(), Some(&1));
        assert_eq!(stack.redo(), Some(&2));
        assert_eq!(stack.redo(), Some(&3));
        assert_eq!(stack.redo(), Some(&4));
        assert_eq!(stack.redo(), None);
    }
}
