//! SelectList component for displaying selectable list items with filtering.

use std::string::String;
use std::vec::Vec;

/// A selectable list component with filter support.
pub struct SelectList {
    /// The list of items.
    items: Vec<SelectItem>,
    /// Currently selected index.
    selected: usize,
    /// Filter string for searching items.
    filter: String,
    /// Number of visible items to display.
    visible_count: usize,
    /// Scroll offset for large lists.
    scroll_offset: usize,
}

impl SelectList {
    /// Creates a new SelectList with the given items.
    pub fn new(items: Vec<SelectItem>) -> Self {
        Self {
            items,
            selected: 0,
            filter: String::new(),
            visible_count: 10,
            scroll_offset: 0,
        }
    }

    /// Sets the visible count (number of items shown at once).
    pub fn visible_count(mut self, count: usize) -> Self {
        self.visible_count = count;
        self
    }

    /// Sets the filter string.
    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = filter.into();
        self
    }

    /// Gets the current filter.
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Gets the currently selected index.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Gets the currently selected item.
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_items().get(self.selected).map(|v| &**v)
    }

    /// Gets all items.
    pub fn items(&self) -> &[SelectItem] {
        &self.items
    }

    /// Returns the filtered list of items based on the filter string.
    pub fn filtered_items(&self) -> Vec<&SelectItem> {
        if self.filter.is_empty() {
            return self.items.iter().collect();
        }
        self.items
            .iter()
            .filter(|item| {
                item.label.to_lowercase().contains(&self.filter.to_lowercase())
                    || item.value.to_lowercase().contains(&self.filter.to_lowercase())
            })
            .collect()
    }

    /// Adds a character to the filter.
    pub fn add_to_filter(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Removes the last character from the filter.
    pub fn remove_from_filter(&mut self) {
        self.filter.pop();
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Clears the filter.
    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Moves selection up by one.
    pub fn select_previous(&mut self) {
        let _filtered = self.filtered_items();
        if self.selected > 0 {
            self.selected -= 1;
            self.ensure_visible();
        }
    }

    /// Moves selection down by one.
    pub fn select_next(&mut self) {
        let _filtered = self.filtered_items();
        if self.selected < _filtered.len().saturating_sub(1) {
            self.selected += 1;
            self.ensure_visible();
        }
    }

    /// Moves selection to the first item.
    pub fn select_first(&mut self) {
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Moves selection to the last item.
    pub fn select_last(&mut self) {
        let filtered = self.filtered_items();
        self.selected = filtered.len().saturating_sub(1);
        self.ensure_visible();
    }

    /// Ensures the selected item is visible (adjusts scroll offset).
    fn ensure_visible(&mut self) {
        if self.selected >= self.scroll_offset + self.visible_count {
            self.scroll_offset = self.selected - self.visible_count + 1;
        } else if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
    }

    /// Handles a key event.
    /// Returns Some(String) if Enter was pressed with the selected value.
    pub fn handle_key(&mut self, key: super::input::KeyEvent) -> Option<String> {
        use super::input::KeyEvent;

        match key {
            KeyEvent::Up => {
                self.select_previous();
                None
            }
            KeyEvent::Down => {
                self.select_next();
                None
            }
            KeyEvent::Home => {
                self.select_first();
                None
            }
            KeyEvent::End => {
                self.select_last();
                None
            }
            KeyEvent::Backspace => {
                self.remove_from_filter();
                None
            }
            KeyEvent::Escape => {
                self.clear_filter();
                None
            }
            KeyEvent::Enter => {
                self.selected_item().map(|item| item.value.clone())
            }
            KeyEvent::Char(c) => {
                self.add_to_filter(c);
                None
            }
            _ => None,
        }
    }
}

impl Default for SelectList {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// A selectable item in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    /// The value returned when selected.
    pub value: String,
    /// The display label.
    pub label: String,
    /// Optional description shown below the label.
    pub description: Option<String>,
}

impl SelectItem {
    /// Creates a new SelectItem.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }

    /// Sets the description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Renders the select list component.
pub trait Render {
    fn render(&self, width: usize) -> Vec<String>;
}

impl Render for SelectList {
    fn render(&self, width: usize) -> Vec<String> {
        use unicode_width::UnicodeWidthStr;

        let filtered = self.filtered_items();
        let mut lines = Vec::new();

        // Render filter bar
        let filter_display = if self.filter.is_empty() {
            String::from("[filter...] ")
        } else {
            format!("[{}] ", self.filter)
        };

        let filter_line = format!(
            "{}{}",
            filter_display,
            " ".repeat(width.saturating_sub(UnicodeWidthStr::width(filter_display.as_str())))
        );
        lines.push(filter_line);

        // Render items
        let visible_items: Vec<_> = filtered
            .iter()
            .skip(self.scroll_offset)
            .take(self.visible_count)
            .collect();

        for (idx, item) in visible_items.iter().enumerate() {
            let actual_idx = self.scroll_offset + idx;
            let is_selected = actual_idx == self.selected;
            let marker = if is_selected { "> " } else { "  " };

            // Truncate label to fit
            let max_label_width = width.saturating_sub(2 + 2); // marker + padding
            let label = truncate(&item.label, max_label_width);

            let line = if is_selected {
                // Highlight selected item
                format!("{}{}", marker, label)
            } else {
                format!("{}{}", marker, label)
            };

            let padded_line = format!(
                "{}{}",
                line,
                " ".repeat(width.saturating_sub(UnicodeWidthStr::width(line.as_str())))
            );
            lines.push(padded_line);

            // Render description if present and selected
            if is_selected {
                if let Some(ref desc) = item.description {
                    let max_desc_width = width.saturating_sub(4);
                    let desc_display = format!("    {}", truncate(desc, max_desc_width));
                    lines.push(desc_display);
                }
            }
        }

        // Fill remaining lines with spaces
        let items_rendered = 1 + visible_items.len();
        for _ in items_rendered..self.visible_count {
            lines.push(" ".repeat(width));
        }

        // Show count
        let count_line = format!(
            "{}/{} items",
            self.selected + 1,
            filtered.len()
        );
        lines.push(format!(
            "{}{}",
            count_line,
            " ".repeat(width.saturating_sub(UnicodeWidthStr::width(count_line.as_str())))
        ));

        lines
    }
}

/// Truncates a string to fit within the given width, appending "..." if truncated.
fn truncate(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthStr;

    let width = UnicodeWidthStr::width(s);
    if width <= max_width {
        return s.to_string();
    }

    let mut result = String::new();
    let mut current_width = 0;
    let ellipsis = "...";
    let ellipsis_width = UnicodeWidthStr::width(ellipsis);
    let target_width = max_width.saturating_sub(ellipsis_width);

    for c in s.chars() {
        let char_width = UnicodeWidthStr::width(c.to_string().as_str());
        if current_width + char_width > target_width {
            break;
        }
        result.push(c);
        current_width += char_width;
    }

    result.push_str(ellipsis);
    result
}
