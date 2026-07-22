//! Generic table list widget — filterable, navigable table with column support.
//!
//! Built on ratatui's `Table` + `TableState`. Similar API to `StatefulList` but
//! displays items in multiple columns with headers.
//!
//! Usage:
//! ```ignore
//! use oxi_tui_legacy::widgets::table_list::{TableItem, TableList, TableListStyles};
//!
//! struct ModelEntry { id: String, context: u32 }
//!
//! impl TableItem for ModelEntry {
//!     fn cells(&self) -> Vec<ratatui::widgets::Cell<'static>> {
//!         vec![
//!             Cell::from(self.id.clone()),
//!             Cell::from(format!("{}k", self.context / 1000)),
//!         ]
//!     }
//!     fn constraints() -> Vec<ratatui::layout::Constraint> {
//!         vec![Constraint::Min(25), Constraint::Length(8)]
//!     }
//!     fn header_cells() -> Vec<ratatui::widgets::Cell<'static>> {
//!         vec![Cell::from("Model"), Cell::from("Context")]
//!     }
//! }
//! ```

use ratatui::{
    layout::Constraint,
    style::Style,
    widgets::{Cell, HighlightSpacing, Row, Table, TableState},
};

// ---------------------------------------------------------------------------
// TableItem trait
// ---------------------------------------------------------------------------

/// Trait for items that can be displayed as rows in a `TableList`.
///
/// Implement this for your data type to enable multi-column table rendering
/// with selection, navigation, and filtering.
pub trait TableItem {
    /// Return the cells for this item (one per column).
    fn cells(&self) -> Vec<Cell<'static>>;

    /// Return the column width constraints.
    fn constraints() -> Vec<Constraint>;

    /// Return the header cells (column labels).
    ///
    /// Default implementation returns empty headers. Override to provide
    /// meaningful column labels.
    fn header_cells() -> Vec<Cell<'static>> {
        Self::constraints().iter().map(|_| Cell::from("")).collect()
    }

    /// Return the primary text used for filtering.
    ///
    /// **Must be implemented** — there is no default because `Cell` does not
    /// expose its inner text. Return a string that represents this item for
    /// case-insensitive substring matching.
    fn filter_text(&self) -> String;
}

// ---------------------------------------------------------------------------
// TableListStyles
// ---------------------------------------------------------------------------

/// Visual styles for rendering a [`TableList`].
pub struct TableListStyles {
    /// Style for unselected rows.
    pub normal: Style,
    /// Style for the currently-selected row.
    pub selected: Style,
    /// Style for the header row.
    pub header: Style,
    /// Symbol prepended to the selected row.
    pub highlight_symbol: &'static str,
}

impl Default for TableListStyles {
    fn default() -> Self {
        Self {
            normal: Style::default(),
            selected: Style::default(),
            highlight_symbol: crate::symbols::Symbols::default().cursor,
            header: Style::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// TableList
// ---------------------------------------------------------------------------

/// A filtered, navigable table backed by ratatui's [`TableState`].
///
/// `T` must implement [`TableItem`] to provide cell data and column constraints.
pub struct TableList<T> {
    /// Original (unfiltered) items in insertion order.
    items: Vec<T>,
    /// ratatui table state — manages offset / selected index.
    state: TableState,
    /// Current filter text.
    filter: String,
    /// Indices into `items` that pass the current filter.
    filtered_indices: Vec<usize>,
}

impl<T> TableList<T> {
    /// Create a new table list from the given items.
    ///
    /// The first item is selected by default and no filter is applied.
    pub fn new(items: Vec<T>) -> Self {
        let filtered_indices: Vec<usize> = (0..items.len()).collect();
        let mut state = TableState::default();
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
    pub fn select_next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.state.selected().unwrap_or(0);
        let next = if current + 1 >= self.filtered_indices.len() {
            0
        } else {
            current + 1
        };
        self.state.select(Some(next));
    }

    /// Select the previous item (wraps around).
    pub fn select_previous(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.state.selected().unwrap_or(0);
        let prev = if current == 0 {
            self.filtered_indices.len() - 1
        } else {
            current - 1
        };
        self.state.select(Some(prev));
    }

    /// Select the first item.
    pub fn select_first(&mut self) {
        if self.filtered_indices.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(0));
        }
    }

    /// Select the last item.
    pub fn select_last(&mut self) {
        if self.filtered_indices.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(self.filtered_indices.len() - 1));
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

    /// Direct mutable access to the underlying [`TableState`].
    pub fn state_mut(&mut self) -> &mut TableState {
        &mut self.state
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

impl<T: TableItem> TableList<T> {
    /// Set the filter text.
    ///
    /// Items whose `filter_text()` **case-insensitively contains** the filter
    /// string are kept. After applying, the selection is reset to the first
    /// matching item.
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_owned();
        let lower = self.filter.to_lowercase();
        self.filtered_indices = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.filter_text().to_lowercase().contains(&lower))
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

    /// Build the ratatui `Table` widget for rendering.
    ///
    /// Returns the table widget ready to be rendered with
    /// `frame.render_stateful_widget(table, area, &mut table_list.state_mut())`.
    pub fn build_widget(&self, styles: &TableListStyles) -> Table<'_> {
        let constraints = T::constraints();
        let header = Row::new(
            T::header_cells()
                .into_iter()
                .map(|c| c.style(styles.header))
                .collect::<Vec<_>>(),
        );

        let rows: Vec<Row<'_>> = self
            .filtered_indices
            .iter()
            .filter_map(|&idx| self.items.get(idx))
            .map(|item| Row::new(item.cells().into_iter().map(|c| c.style(styles.normal))))
            .collect();

        Table::new(rows, constraints)
            .header(header)
            .row_highlight_style(styles.selected)
            .highlight_symbol(styles.highlight_symbol)
            .highlight_spacing(HighlightSpacing::Always)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct TestItem {
        name: String,
        value: u32,
    }

    impl TableItem for TestItem {
        fn cells(&self) -> Vec<Cell<'static>> {
            vec![
                Cell::from(self.name.clone()),
                Cell::from(self.value.to_string()),
            ]
        }

        fn constraints() -> Vec<Constraint> {
            vec![Constraint::Min(10), Constraint::Length(6)]
        }

        fn header_cells() -> Vec<Cell<'static>> {
            vec![Cell::from("Name"), Cell::from("Value")]
        }

        fn filter_text(&self) -> String {
            self.name.clone()
        }
    }

    fn sample_items() -> Vec<TestItem> {
        vec![
            TestItem {
                name: "alpha".into(),
                value: 10,
            },
            TestItem {
                name: "beta".into(),
                value: 20,
            },
            TestItem {
                name: "gamma".into(),
                value: 30,
            },
        ]
    }

    #[test]
    fn new_selects_first() {
        let list = TableList::new(sample_items());
        assert_eq!(list.selected().unwrap().name, "alpha");
        assert_eq!(list.selected_index(), Some(0));
    }

    #[test]
    fn navigation() {
        let mut list = TableList::new(sample_items());
        list.select_next();
        assert_eq!(list.selected().unwrap().name, "beta");
        list.select_last();
        assert_eq!(list.selected().unwrap().name, "gamma");
        list.select_previous();
        assert_eq!(list.selected().unwrap().name, "beta");
        list.select_first();
        assert_eq!(list.selected().unwrap().name, "alpha");
    }

    #[test]
    fn scroll_by() {
        let mut list = TableList::new(sample_items());
        list.scroll_down_by(2);
        assert_eq!(list.selected().unwrap().name, "gamma");
        list.scroll_up_by(1);
        assert_eq!(list.selected().unwrap().name, "beta");
    }

    #[test]
    fn filter() {
        let mut list = TableList::new(sample_items());
        list.set_filter("al");
        assert_eq!(list.len(), 1); // only "alpha" matches name filter
        assert_eq!(list.selected().unwrap().name, "alpha");
    }

    #[test]
    fn filter_input_and_backspace() {
        let mut list = TableList::new(sample_items());
        list.filter_input('b');
        assert_eq!(list.len(), 1);
        assert_eq!(list.selected().unwrap().name, "beta");
        list.filter_backspace();
        assert_eq!(list.len(), 3); // no filter
    }

    #[test]
    fn clear_filter() {
        let mut list = TableList::new(sample_items());
        list.set_filter("zzz");
        assert!(list.is_empty());
        list.clear_filter();
        assert_eq!(list.len(), 3);
        assert_eq!(list.selected().unwrap().name, "alpha");
    }

    #[test]
    fn empty_list() {
        let list: TableList<TestItem> = TableList::new(vec![]);
        assert!(list.is_empty());
        assert_eq!(list.selected(), None);
    }

    #[test]
    fn build_widget() {
        let list = TableList::new(sample_items());
        let styles = TableListStyles::default();
        let _table = list.build_widget(&styles);
        // Just verify it builds without panic
    }

    #[test]
    fn wrap_around_navigation() {
        let mut list = TableList::new(sample_items());
        list.select_next();
        list.select_next();
        list.select_next(); // wraps to 0
        assert_eq!(list.selected().unwrap().name, "alpha");

        list.select_previous(); // wraps to last
        assert_eq!(list.selected().unwrap().name, "gamma");
    }
}
