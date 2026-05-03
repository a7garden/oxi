//! Theme selector component for choosing between available themes.
//!
//! Displays a list of themes with color previews and allows selection
//! with keyboard navigation.

use crate::autocomplete::FuzzyMatcher;
use crate::cell::Color;
use crate::{Cell, Component, Event, KeyCode, KeyEvent, Rect, Size, Surface};
use std::path::PathBuf;

/// Information about a theme for the selector
#[derive(Debug, Clone)]
pub struct ThemeInfo {
    /// Theme name
    pub name: String,
    /// Whether it's a dark theme
    pub is_dark: bool,
    /// Path to theme file (None for built-in themes)
    pub source_path: Option<PathBuf>,
    /// Primary background color (for preview)
    pub bg_color: Color,
    /// Primary foreground color (for preview)
    pub fg_color: Color,
    /// Accent color (for preview)
    pub accent_color: Color,
}

impl ThemeInfo {
    /// Create from a theme
    pub fn from_theme(name: &str, is_dark: bool, source_path: Option<PathBuf>) -> Self {
        let (bg_color, fg_color, accent_color) = if is_dark {
            (
                Color::Rgb(30, 30, 36),
                Color::Rgb(220, 223, 228),
                Color::Rgb(137, 180, 250),
            )
        } else {
            (
                Color::Rgb(239, 241, 245),
                Color::Rgb(76, 79, 105),
                Color::Rgb(30, 102, 240),
            )
        };

        Self {
            name: name.to_string(),
            is_dark,
            source_path,
            bg_color,
            fg_color,
            accent_color,
        }
    }
}

/// Callback type for when a theme is selected.
pub type OnThemeSelectFn = Box<dyn Fn(&str) + Send>;

/// Theme selector component.
pub struct ThemeSelector {
    /// Available themes
    themes: Vec<ThemeInfo>,
    /// Filtered indices for search
    filtered_indices: Vec<usize>,
    /// Current filter text
    filter: String,
    /// Currently selected index (in filtered_indices)
    selected: usize,
    /// Scroll offset
    scroll_offset: usize,
    /// Whether component is focused
    focused: bool,
    /// Whether component needs re-render
    dirty: bool,
    /// Callback when theme is selected
    on_select: Option<OnThemeSelectFn>,
    /// Fuzzy matcher for search
    matcher: FuzzyMatcher,
}

impl ThemeSelector {
    /// Create a new theme selector with the given themes.
    pub fn new(themes: Vec<ThemeInfo>) -> Self {
        let filtered_indices = (0..themes.len()).collect();
        Self {
            themes,
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

    /// Set callback for when a theme is selected.
    pub fn on_select(mut self, f: impl Fn(&str) + Send + 'static) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }

    /// Set the available themes.
    pub fn set_themes(&mut self, themes: Vec<ThemeInfo>) {
        self.themes = themes;
        self.apply_filter();
        self.selected = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// Get the currently selected theme info.
    pub fn selected_theme(&self) -> Option<&ThemeInfo> {
        self.filtered_indices
            .get(self.selected)
            .and_then(|&i| self.themes.get(i))
    }

    /// Get the name of the currently selected theme.
    pub fn selected_theme_name(&self) -> Option<String> {
        self.selected_theme().map(|t| t.name.clone())
    }

    /// Set the filter text for searching themes.
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.apply_filter();
        self.selected = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// Set initial selection by theme name.
    pub fn set_selected_by_name(&mut self, name: &str) {
        if let Some(idx) = self.themes.iter().position(|t| t.name == name) {
            // Find the filtered index
            if let Some(filtered_idx) = self.filtered_indices.iter().position(|&i| i == idx) {
                self.selected = filtered_idx;
                self.adjust_scroll();
                self.dirty = true;
            }
        }
    }

    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered_indices = (0..self.themes.len()).collect();
        } else {
            let mut scored: Vec<(usize, usize)> = self
                .themes
                .iter()
                .enumerate()
                .filter_map(|(i, theme)| {
                    self.matcher
                        .matches(&self.filter, &theme.name)
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
        if self.selected < self.filtered_indices.len().saturating_sub(1) {
            self.selected += 1;
            self.adjust_scroll();
            self.dirty = true;
        }
    }

    fn confirm(&mut self) {
        if let Some(ref cb) = self.on_select {
            if let Some(theme) = self.selected_theme() {
                cb(&theme.name);
            }
        }
    }

    fn adjust_scroll(&mut self) {
        let visible_height = 10; // Default visible items
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected - visible_height + 1;
        }
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

impl Component for ThemeSelector {
    fn name(&self) -> &str {
        "ThemeSelector"
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
            let theme_idx = self.filtered_indices[vi];
            let theme = &self.themes[theme_idx];
            let is_selected = vi == self.selected;

            // Selection colors
            let (fg, bg) = if is_selected && self.focused {
                (Color::Default, theme.fg_color.clone())
            } else {
                (theme.fg_color.clone(), Color::Default)
            };

            // Render indicator
            let indicator = if is_selected { ">" } else { " " };
            surface.set(
                row,
                area.x,
                Cell::new(indicator.chars().next().unwrap()).with_fg(fg.clone()).with_bg(bg.clone()),
            );

            // Render color swatches (3 small colored squares)
            let swatch_bg = theme.bg_color.clone();
            let swatch_fg = theme.fg_color.clone();
            let swatch_accent = theme.accent_color.clone();

            for (i, color) in [swatch_bg, swatch_fg, swatch_accent]
                .iter()
                .enumerate()
            {
                let col = area.x + 2 + i as u16;
                surface.set(
                    row,
                    col,
                    Cell::new('█')
                        .with_fg(color.clone())
                        .with_bg(bg.clone()),
                );
            }

            // Render theme name
            let label_start = area.x + 6;
            let max_label_width = (area.width as usize).saturating_sub(8);
            let label_str: String = theme
                .name
                .chars()
                .take(max_label_width)
                .collect();
            for (i, c) in label_str.chars().enumerate() {
                let col = label_start + i as u16;
                if col < area.x + area.width {
                    surface.set(
                        row,
                        col,
                        Cell::new(c).with_fg(fg.clone()).with_bg(bg.clone()),
                    );
                }
            }

            // Render theme type indicator
            let type_indicator = if theme.is_dark { "dark" } else { "light" };
            let type_start = area.x + area.width - 6;
            for (i, c) in type_indicator.chars().enumerate() {
                let col = type_start + i as u16;
                if col < area.x + area.width {
                    surface.set(
                        row,
                        col,
                        Cell::new(c)
                            .with_fg(fg.clone())
                            .with_bg(bg.clone()),
                    );
                }
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
            width: 30,
            height: 5,
        }
    }

    fn desired_size(&self) -> Option<Size> {
        Some(Size {
            width: 40,
            height: (self.filtered_indices.len() as u16).min(15),
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_themes() -> Vec<ThemeInfo> {
        vec![
            ThemeInfo::from_theme("oxi_dark", true, None),
            ThemeInfo::from_theme("oxi_light", false, None),
            ThemeInfo::from_theme("nord", true, None),
            ThemeInfo::from_theme("monokai", true, None),
        ]
    }

    #[test]
    fn test_new_selector_has_themes() {
        let themes = make_test_themes();
        let selector = ThemeSelector::new(themes);
        assert_eq!(selector.themes.len(), 4);
    }

    #[test]
    fn test_set_themes() {
        let mut selector = ThemeSelector::new(vec![]);
        let new_themes = make_test_themes();
        selector.set_themes(new_themes);
        assert_eq!(selector.themes.len(), 4);
    }

    #[test]
    fn test_selected_theme() {
        let themes = make_test_themes();
        let mut selector = ThemeSelector::new(themes);
        assert!(selector.selected_theme().is_some());
        assert_eq!(selector.selected_theme().unwrap().name, "oxi_dark");
    }

    #[test]
    fn test_set_selected_by_name() {
        let themes = make_test_themes();
        let mut selector = ThemeSelector::new(themes);
        selector.set_selected_by_name("nord");
        assert_eq!(selector.selected_theme().unwrap().name, "nord");
    }

    #[test]
    fn test_navigation() {
        let themes = make_test_themes();
        let mut selector = ThemeSelector::new(themes);
        assert_eq!(selector.selected_theme().unwrap().name, "oxi_dark");

        selector.select_next();
        assert_eq!(selector.selected_theme().unwrap().name, "oxi_light");

        selector.select_next();
        assert_eq!(selector.selected_theme().unwrap().name, "nord");

        selector.select_prev();
        assert_eq!(selector.selected_theme().unwrap().name, "oxi_light");
    }

    #[test]
    fn test_filter() {
        let themes = make_test_themes();
        let mut selector = ThemeSelector::new(themes);

        selector.set_filter("dark");
        assert!(selector.selected_theme().is_some());
        assert!(selector
            .selected_theme()
            .map(|t| t.name.contains("dark"))
            .unwrap_or(false));
    }

    #[test]
    fn test_empty_filter_shows_all() {
        let themes = make_test_themes();
        let mut selector = ThemeSelector::new(themes);

        selector.set_filter("dark");
        assert!(selector.filtered_indices.len() <= 4);

        selector.set_filter("");
        assert_eq!(selector.filtered_indices.len(), 4);
    }

    #[test]
    fn test_on_select_callback() {
        let themes = make_test_themes();
        let selected_idx = std::sync::atomic::AtomicUsize::new(0);

        {
            let mut selector = ThemeSelector::new(themes).on_select(|name| {
                let idx = match name {
                    "oxi_dark" => 0,
                    "oxi_light" => 1,
                    "nord" => 2,
                    "monokai" => 3,
                    _ => 0,
                };
                selected_idx.store(idx, std::sync::atomic::Ordering::SeqCst);
            });

            selector.select_next();
            selector.confirm();
        }

        assert_eq!(selected_idx.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
