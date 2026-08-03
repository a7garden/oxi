//! Reusable wrapper around ratatui's `List` + `ListState`.
//!
//! All overlay selectors (model picker, theme picker, skill picker, etc.)
//! should use this to share filtering, navigation, and rendering logic.

use ratatui::style::Style;
use ratatui::widgets::ListState;

// ---------------------------------------------------------------------------
// ListStyles
// ---------------------------------------------------------------------------

/// Visual styles for rendering a [`StatefulList`].
pub struct ListStyles {
    /// Style for unselected items.
    pub normal: Style,
    /// Style for the currently-selected item.
    pub selected: Style,
    /// Symbol prepended to the selected row.
    pub highlight_symbol: &'static str,
}

impl Default for ListStyles {
    fn default() -> Self {
        Self {
            normal: Style::default(),
            selected: Style::default(),
            highlight_symbol: crate::symbols::Symbols::default().cursor,
        }
    }
}

// ---------------------------------------------------------------------------
// StatefulList
// ---------------------------------------------------------------------------

/// A filtered, navigable list backed by ratatui's [`ListState`].
///
/// `T` is the item type. When filtering is needed, `T` must implement
/// [`AsRef<str>`] so the filter can perform case-insensitive matching.
pub struct StatefulList<T> {
    /// Original (unfiltered) items in insertion order.
    items: Vec<T>,
    /// ratatui list state — manages offset / selected index.
    state: ListState,
    /// Current filter text.
    filter: String,
    /// Indices into `items` that pass the current filter.
    filtered_indices: Vec<usize>,
}

impl<T> StatefulList<T> {
    // ----- constructors ---------------------------------------------------

    /// Create a new list from the given items.
    ///
    /// The first item is selected by default and no filter is applied.
    pub fn new(items: Vec<T>) -> Self {
        let filtered_indices: Vec<usize> = (0..items.len()).collect();
        let mut state = ListState::default();
        if !filtered_indices.is_empty() {
            state.select(Some(0));
        }
        Self {
            items,
            state,
            filter: String::new(),
            filtered_indices,
        }
    }

    // ----- navigation -----------------------------------------------------

    /// Select the next item (wraps around).
    ///
    /// Delegates to [`ListState::select_next`] which will be clamped during
    /// rendering. Use [`Self::scroll_down_by`] for bounded navigation.
    pub fn select_next(&mut self) {
        self.state.select_next();
    }

    /// Select the previous item (wraps around).
    ///
    /// Delegates to [`ListState::select_previous`] which will be clamped during
    /// rendering. Use [`Self::scroll_up_by`] for bounded navigation.
    pub fn select_previous(&mut self) {
        self.state.select_previous();
    }

    /// Select the first item.
    pub fn select_first(&mut self) {
        self.state.select_first();
    }

    /// Select the last item.
    ///
    /// Sets the selection to the last filtered index directly, because
    /// [`ListState::select_last`] uses `usize::MAX` which is only corrected
    /// during rendering.
    pub fn select_last(&mut self) {
        if self.filtered_indices.is_empty() {
            self.state.select(None);
        } else {
            let last = self.filtered_indices.len() - 1;
            self.state.select(Some(last));
        }
    }

    /// Move the selection down by `n` items, clamped to the last item.
    pub fn scroll_down_by(&mut self, n: u16) {
        let n = n as usize;
        if self.filtered_indices.is_empty() {
            return;
        }
        let max = self.filtered_indices.len().saturating_sub(1);
        let cur = self.state.selected().unwrap_or(0);
        let next = (cur + n).min(max);
        self.state.select(Some(next));
    }

    /// Move the selection up by `n` items, clamped to zero.
    pub fn scroll_up_by(&mut self, n: u16) {
        let n = n as usize;
        let cur = self.state.selected().unwrap_or(0);
        let prev = cur.saturating_sub(n);
        self.state.select(Some(prev));
    }

    // ----- query ----------------------------------------------------------

    /// Return a reference to the currently-selected item (after filtering).
    pub fn selected(&self) -> Option<&T> {
        let idx = *self
            .state
            .selected()
            .and_then(|i| self.filtered_indices.get(i))?;
        self.items.get(idx)
    }

    /// Return a mutable reference to the currently-selected item (after filtering).
    pub fn selected_mut(&mut self) -> Option<&mut T> {
        let idx = *self
            .state
            .selected()
            .and_then(|i| self.filtered_indices.get(i))?;
        self.items.get_mut(idx)
    }

    /// Return the **original** index (i.e. position in `items`) of the
    /// currently-selected item, or `None` if nothing is selected.
    pub fn selected_index(&self) -> Option<usize> {
        let i = self.state.selected()?;
        self.filtered_indices.get(i).copied()
    }

    /// Number of items that pass the current filter.
    pub fn len(&self) -> usize {
        self.filtered_indices.len()
    }

    /// `true` when no items pass the current filter.
    pub fn is_empty(&self) -> bool {
        self.filtered_indices.is_empty()
    }

    /// Direct mutable access to the underlying [`ListState`].
    pub fn state_mut(&mut self) -> &mut ListState {
        &mut self.state
    }

    /// Iterate over items that pass the current filter.
    pub fn items(&self) -> impl Iterator<Item = &T> {
        self.filtered_indices
            .iter()
            .filter_map(|&i| self.items.get(i))
    }

    /// The current filter text.
    pub fn filter_text(&self) -> &str {
        &self.filter
    }

    // ----- filtering ------------------------------------------------------

    /// Clear any active filter, restoring the full item list.
    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.filtered_indices = (0..self.items.len()).collect();
        if !self.filtered_indices.is_empty() {
            self.state.select(Some(0));
        } else {
            self.state.select(None);
        }
    }
}

// Filtering methods that require `T: AsRef<str>`.

impl<T: AsRef<str>> StatefulList<T> {
    /// Set the filter text.
    ///
    /// Items that **case-insensitively contain** the filter string are kept.
    /// After applying, the selection is reset to the first matching item.
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_owned();
        let lower = self.filter.to_lowercase();
        self.filtered_indices = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.as_ref().to_lowercase().contains(&lower))
            .map(|(i, _)| i)
            .collect();
        if !self.filtered_indices.is_empty() {
            self.state.select(Some(0));
        } else {
            self.state.select(None);
        }
    }

    /// Append a character to the current filter and re-apply.
    pub fn filter_input(&mut self, c: char) {
        self.filter.push(c);
        let f = self.filter.clone();
        self.set_filter(&f);
    }

    /// Remove the last character from the filter and re-apply.
    pub fn filter_backspace(&mut self) {
        self.filter.pop();
        let f = self.filter.clone();
        self.set_filter(&f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_selects_first() {
        let list: StatefulList<&str> = StatefulList::new(vec!["a", "b", "c"]);
        assert_eq!(list.selected(), Some(&"a"));
        assert_eq!(list.selected_index(), Some(0));
    }

    #[test]
    fn navigation() {
        let mut list = StatefulList::new(vec!["a", "b", "c"]);
        list.select_next();
        assert_eq!(list.selected(), Some(&"b"));
        list.select_last();
        assert_eq!(list.selected(), Some(&"c"));
        list.select_previous();
        assert_eq!(list.selected(), Some(&"b"));
        list.select_first();
        assert_eq!(list.selected(), Some(&"a"));
    }

    #[test]
    fn scroll_by() {
        let mut list = StatefulList::new(vec!["a", "b", "c", "d", "e"]);
        list.scroll_down_by(2);
        assert_eq!(list.selected(), Some(&"c"));
        list.scroll_down_by(100);
        assert_eq!(list.selected(), Some(&"e"));
        list.scroll_up_by(3);
        assert_eq!(list.selected(), Some(&"b"));
    }

    #[test]
    fn filter() {
        let mut list = StatefulList::new(vec!["apple", "banana", "apricot", "cherry"]);
        list.set_filter("ap");
        assert_eq!(list.len(), 2);
        assert_eq!(list.selected(), Some(&"apple"));
        assert_eq!(list.selected_index(), Some(0));

        list.select_next();
        assert_eq!(list.selected(), Some(&"apricot"));
        assert_eq!(list.selected_index(), Some(2));
    }

    #[test]
    fn filter_input_and_backspace() {
        let mut list = StatefulList::new(vec!["foo", "bar", "baz"]);
        list.filter_input('b');
        assert_eq!(list.len(), 2);
        list.filter_input('a');
        assert_eq!(list.len(), 2); // "bar", "baz"
        list.filter_backspace();
        assert_eq!(list.len(), 2); // back to "b"
        list.filter_backspace();
        assert_eq!(list.len(), 3); // no filter
    }

    #[test]
    fn clear_filter() {
        let mut list = StatefulList::new(vec!["foo", "bar"]);
        list.set_filter("z");
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
        list.clear_filter();
        assert_eq!(list.len(), 2);
        assert_eq!(list.selected(), Some(&"foo"));
    }

    #[test]
    fn empty_list() {
        let list: StatefulList<&str> = StatefulList::new(vec![]);
        assert!(list.is_empty());
        assert_eq!(list.selected(), None);
    }

    #[test]
    fn default_styles() {
        let styles = ListStyles::default();
        // Default glyph set is Unicode → cursor comes from the symbol table.
        assert_eq!(
            styles.highlight_symbol,
            crate::symbols::Symbols::unicode().cursor
        );
    }

    #[test]
    fn items_iterator_respects_filter() {
        let mut list = StatefulList::new(vec!["cat", "dog", "cow"]);
        list.set_filter("c");
        let collected: Vec<&&str> = list.items().collect();
        assert_eq!(collected, vec![&"cat", &"cow"]);
    }
}
