//! Selectable list component with filtering and keyboard navigation.

use crate::autocomplete::FuzzyMatcher;
use crate::{Cell, Color, Component, Event, KeyCode, KeyEvent, Rect, Size, Surface};

/// A single item in the select list.
#[derive(Debug, Clone)]
pub struct SelectItem {
    /// Display label.
    pub label: String,
    /// Optional description shown alongside.
    pub description: Option<String>,
}

impl SelectItem {
    /// Create a new item with just a label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
        }
    }

    /// Add a description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Callback type for when an item is selected.
pub type OnSelectFn = Box<dyn Fn(&SelectItem) + Send>;

/// A selectable list with filtering support.
pub struct SelectList {
    items: Vec<SelectItem>,
    filtered_indices: Vec<usize>,
    filter: String,
    selected: usize,
    scroll_offset: usize,
    focused: bool,
    dirty: bool,
    on_select: Option<OnSelectFn>,
    matcher: FuzzyMatcher,
}

impl SelectList {
    /// Create a list pre-loaded with items.
    pub fn new(items: Vec<SelectItem>) -> Self {
        let filtered_indices = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            filter: String::new(),
            selected: 0,
            scroll_offset: 0,
            focused: false,
            dirty: true,
            on_select: None,
            matcher: FuzzyMatcher::new(),
        }
    }

    /// Set the callback invoked when an item is selected.
    pub fn on_select(mut self, f: impl Fn(&SelectItem) + Send + 'static) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }

    /// Replace the item list.
    pub fn set_items(&mut self, items: Vec<SelectItem>) {
        self.items = items;
        self.apply_filter();
        self.dirty = true;
    }

    /// Get the currently selected item.
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected)
            .and_then(|&i| self.items.get(i))
    }

    /// Set the filter string and re-apply.
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.apply_filter();
        self.selected = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            let mut scored: Vec<(usize, usize)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    self.matcher
                        .matches(&self.filter, &item.label)
                        .map(|score| (i, score))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_indices = scored.into_iter().map(|(i, _)| i).collect();
        }
    }

    fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.adjust_scroll();
            self.dirty = true;
        }
    }

    fn select_next(&mut self) {
        if !self.filtered_indices.is_empty() && self.selected < self.filtered_indices.len() - 1 {
            self.selected += 1;
            self.adjust_scroll();
            self.dirty = true;
        }
    }

    fn confirm(&mut self) {
        if let Some(ref cb) = self.on_select {
            if let Some(item) = self.selected_item() {
                cb(item);
            }
        }
    }

    fn adjust_scroll(&mut self) {
        // Keep selected item visible
    }

    fn visible_range(&self, area: Rect) -> (usize, usize) {
        let visible_count = area.height as usize;
        let total = self.filtered_indices.len();
        if total == 0 {
            return (0, 0);
        }
        let end = (self.scroll_offset + visible_count).min(total);
        (self.scroll_offset, end)
    }
}

impl Component for SelectList {
    fn name(&self) -> &str {
        "SelectList"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if !self.focused {
            return false;
        }

        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) => {
                self.select_prev();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            }) => {
                self.select_next();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('k'),
                modifiers,
            }) if !modifiers.ctrl && !modifiers.alt => {
                self.select_prev();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('j'),
                modifiers,
            }) if !modifiers.ctrl && !modifiers.alt => {
                self.select_next();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                self.confirm();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(c),
                ..
            }) => {
                self.filter.push(*c);
                self.apply_filter();
                self.selected = 0;
                self.scroll_offset = 0;
                self.dirty = true;
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                self.filter.pop();
                self.apply_filter();
                self.selected = 0;
                self.scroll_offset = 0;
                self.dirty = true;
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Escape,
                ..
            }) => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.apply_filter();
                    self.selected = 0;
                    self.scroll_offset = 0;
                    self.dirty = true;
                }
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let (start, end) = self.visible_range(area);
        let mut row = area.y;

        for vi in start..end {
            if row >= area.y + area.height {
                break;
            }
            let item_idx = self.filtered_indices[vi];
            let item = &self.items[item_idx];
            let is_selected = vi == self.selected;

            let fg = if is_selected && self.focused {
                Color::Black
            } else {
                Color::Default
            };
            let bg = if is_selected && self.focused {
                Color::Indexed(12) // bright cyan highlight
            } else {
                Color::Default
            };

            // Render indicator
            let indicator = if is_selected { ">" } else { " " };
            for (i, c) in indicator.chars().enumerate() {
                let col = area.x + i as u16;
                if col < area.x + area.width {
                    surface.set(row, col, Cell::new(c).with_fg(fg).with_bg(bg));
                }
            }

            // Render label
            let label_start = area.x + 2;
            let max_label_width = (area.width as usize).saturating_sub(4);
            let label_str: String = item.label.chars().take(max_label_width).collect();
            for (i, c) in label_str.chars().enumerate() {
                let col = label_start + i as u16;
                if col < area.x + area.width {
                    surface.set(row, col, Cell::new(c).with_fg(fg).with_bg(bg));
                }
            }

            // Clear rest of line with bg
            let label_end = label_start + label_str.len() as u16;
            for col in label_end..area.x + area.width {
                surface.set(row, col, Cell::new(' ').with_fg(fg).with_bg(bg));
            }

            row += 1;
        }

        // Clear remaining rows
        for r in row..area.y + area.height {
            for col in area.x..area.x + area.width {
                surface.set(r, col, Cell::new(' '));
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 10,
            height: 1,
        }
    }

    fn desired_size(&self) -> Option<Size> {
        Some(Size {
            width: 40,
            height: (self.filtered_indices.len() as u16).min(20),
        })
    }

    fn on_focus(&mut self) {
        self.focused = true;
        self.dirty = true;
    }

    fn on_unfocus(&mut self) {
        self.focused = false;
        self.dirty = true;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}
