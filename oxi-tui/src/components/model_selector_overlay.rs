//! Model selector overlay with fuzzy search, provider badges, and keyboard navigation.
//!
//! Displays a searchable list of models grouped by provider, with visual
//! indicators for the currently active model. Supports arrow-key navigation,
//! fuzzy filtering, and selection callbacks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::autocomplete::FuzzyMatcher;
use crate::utils::{truncate_to_width, visible_width};
use crate::{Cell, Color, Component, Event, KeyCode, KeyEvent, Rect, Size, Surface};

/// A model item for the model selector overlay.
#[derive(Debug, Clone)]
pub struct ModelItem {
    /// Unique provider identifier (e.g., "anthropic", "openai").
    pub provider: String,
    /// Unique model identifier (e.g., "claude-sonnet-4-20250501").
    pub id: String,
    /// Human-readable model name (optional, falls back to `id`).
    pub name: Option<String>,
    /// Whether this model is the currently active one.
    pub is_current: bool,
}

impl ModelItem {
    /// Create a new model item.
    pub fn new(provider: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            id: id.into(),
            name: None,
            is_current: false,
        }
    }

    /// Set the human-readable name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Mark as the currently active model.
    pub fn with_current(mut self, current: bool) -> Self {
        self.is_current = current;
        self
    }

    /// Get the display name (name field or id).
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// Callback when a model is selected.
pub type OnModelSelectFn = Box<dyn Fn(&ModelItem) + Send>;

/// Callback when the overlay is cancelled.
pub type OnModelCancelFn = Box<dyn Fn() + Send>;

/// Model selector overlay with fuzzy search and keyboard navigation.
///
/// # Example
///
/// ```ignore
/// let selector = ModelSelectorOverlay::new(models, current_model, 10);
/// // In event loop: selector.handle_event(&event);
/// // On select: selector.on_select callback fires
/// ```
pub struct ModelSelectorOverlay {
    /// All available models, sorted.
    models: Vec<ModelItem>,
    /// Filtered subset based on search query.
    filtered_models: Vec<ModelItem>,
    /// Current search query.
    query: String,
    /// Currently selected index in `filtered_models`.
    selected_index: usize,
    /// Maximum number of visible items before scrolling.
    max_visible: usize,
    /// Whether the overlay is visible.
    visible: bool,
    /// Whether the overlay is focused.
    focused: bool,
    /// Dirty flag for rendering.
    dirty: bool,
    /// Callback when a model is selected.
    on_select: Option<OnModelSelectFn>,
    /// Callback when the overlay is cancelled.
    on_cancel: Option<OnModelCancelFn>,
    /// Fuzzy matcher.
    matcher: FuzzyMatcher,
    /// Theme colors.
    theme: ModelSelectorTheme,
}

/// Theme colors for the model selector overlay.
#[derive(Debug, Clone)]
pub struct ModelSelectorTheme {
    /// Border color.
    pub border: Color,
    /// Title text color.
    pub title: Color,
    /// Selected row text color.
    pub selected_text: Color,
    /// Selected row background color.
    pub selected_bg: Color,
    /// Provider badge color.
    pub provider_badge: Color,
    /// Current model indicator color.
    pub current_badge: Color,
    /// Dimmed / muted text color.
    pub muted: Color,
    /// Scroll info text color.
    pub scroll_info: Color,
    /// No-match message color.
    pub no_match: Color,
    /// Hint text color.
    pub hint: Color,
    /// Input text color.
    pub input_text: Color,
    /// Normal text color.
    pub normal: Color,
    /// Overlay background color.
    pub overlay_bg: Color,
}

impl Default for ModelSelectorTheme {
    fn default() -> Self {
        Self {
            border: Color::Indexed(12),
            title: Color::Cyan,
            selected_text: Color::Black,
            selected_bg: Color::Indexed(12),
            provider_badge: Color::Magenta,
            current_badge: Color::Green,
            muted: Color::Indexed(8),
            scroll_info: Color::Indexed(8),
            no_match: Color::Indexed(8),
            hint: Color::Indexed(8),
            input_text: Color::White,
            normal: Color::White,
            overlay_bg: Color::Indexed(236),
        }
    }
}

impl ModelSelectorOverlay {
    /// Create a new model selector overlay.
    pub fn new(
        models: Vec<ModelItem>,
        current_model: Option<&ModelItem>,
        max_visible: usize,
    ) -> Self {
        let sorted = Self::sort_models(models, current_model);
        let filtered = sorted.clone();
        let mut overlay = Self {
            models: sorted,
            filtered_models: filtered,
            query: String::new(),
            selected_index: 0,
            max_visible,
            visible: false,
            focused: false,
            dirty: true,
            on_select: None,
            on_cancel: None,
            matcher: FuzzyMatcher::new(),
            theme: ModelSelectorTheme::default(),
        };

        // Pre-select current model
        if let Some(current) = current_model {
            overlay.pre_select(current);
        }

        overlay
    }

    /// Set theme colors.
    pub fn with_theme(mut self, theme: ModelSelectorTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set the selection callback.
    pub fn on_select(mut self, f: impl Fn(&ModelItem) + Send + 'static) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }

    /// Set the cancel callback.
    pub fn on_cancel(mut self, f: impl Fn() + Send + 'static) -> Self {
        self.on_cancel = Some(Box::new(f));
        self
    }

    /// Show the overlay.
    pub fn show(&mut self) {
        self.visible = true;
        self.query.clear();
        self.apply_filter();
        self.dirty = true;
    }

    /// Hide the overlay.
    pub fn hide(&mut self) {
        self.visible = false;
        self.dirty = true;
    }

    /// Whether the overlay is visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Update the models list.
    pub fn set_models(&mut self, models: Vec<ModelItem>, current: Option<&ModelItem>) {
        self.models = Self::sort_models(models, current);
        self.apply_filter();
        if let Some(cur) = current {
            self.pre_select(cur);
        }
        self.dirty = true;
    }

    /// Get the currently selected model.
    pub fn selected_model(&self) -> Option<&ModelItem> {
        self.filtered_models.get(self.selected_index)
    }

    /// Get the current search query.
    pub fn query(&self) -> &str {
        &self.query
    }

    // ── Private helpers ──────────────────────────────────────────────

    fn sort_models(mut models: Vec<ModelItem>, current: Option<&ModelItem>) -> Vec<ModelItem> {
        models.sort_by(|a, b| {
            // Current model first
            let a_is_current = current.map_or(false, |c| {
                c.provider == a.provider && c.id == a.id
            });
            let b_is_current = current.map_or(false, |c| {
                c.provider == b.provider && c.id == b.id
            });
            if a_is_current && !b_is_current {
                return std::cmp::Ordering::Less;
            }
            if !a_is_current && b_is_current {
                return std::cmp::Ordering::Greater;
            }
            // Then by provider
            match a.provider.cmp(&b.provider) {
                std::cmp::Ordering::Equal => a.id.cmp(&b.id),
                other => other,
            }
        });
        models
    }

    fn pre_select(&mut self, target: &ModelItem) {
        if let Some(idx) = self.filtered_models.iter().position(|m| {
            m.provider == target.provider && m.id == target.id
        }) {
            self.selected_index = idx;
        }
    }

    fn apply_filter(&mut self) {
        if self.query.trim().is_empty() {
            self.filtered_models = self.models.clone();
        } else {
            let mut scored: Vec<(usize, usize)> = self
                .models
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    let search_text = format!("{} {} {}/{}", m.id, m.provider, m.provider, m.id);
                    self.matcher
                        .matches(&self.query, &search_text)
                        .map(|score| (i, score))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_models = scored
                .into_iter()
                .map(|(i, _)| self.models[i].clone())
                .collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered_models.len().saturating_sub(1));
    }

    fn select_prev(&mut self) {
        if self.filtered_models.is_empty() {
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = self.filtered_models.len() - 1;
        } else {
            self.selected_index -= 1;
        }
        self.dirty = true;
    }

    fn select_next(&mut self) {
        if self.filtered_models.is_empty() {
            return;
        }
        if self.selected_index == self.filtered_models.len() - 1 {
            self.selected_index = 0;
        } else {
            self.selected_index += 1;
        }
        self.dirty = true;
    }

    fn confirm(&mut self) {
        if let Some(ref cb) = self.on_select {
            if let Some(model) = self.filtered_models.get(self.selected_index) {
                let model = model.clone();
                cb(&model);
            }
        }
        self.hide();
    }

    fn cancel(&mut self) {
        if let Some(ref cb) = self.on_cancel {
            cb();
        }
        self.hide();
    }

    fn visible_range(&self) -> (usize, usize) {
        let total = self.filtered_models.len();
        if total == 0 {
            return (0, 0);
        }
        let half = self.max_visible / 2;
        let start = if self.selected_index >= half {
            (self.selected_index - half).min(total.saturating_sub(self.max_visible))
        } else {
            0
        };
        let end = (start + self.max_visible).min(total);
        (start, end)
    }

    // ── Rendering helpers ────────────────────────────────────────────

    fn render_border_line(&self, surface: &mut Surface, row: u16, start_x: u16, width: u16) {
        let ch = '─';
        for c in start_x..start_x + width {
            surface.set(row, c, Cell::new(ch).with_fg(self.theme.border).with_bg(self.theme.overlay_bg));
        }
    }

    fn render_text(&self, surface: &mut Surface, row: u16, col: u16, text: &str, fg: Color, bg: Color, max_width: u16) {
        let truncated = truncate_to_width(text, max_width as usize, Some(""), false);
        for (i, c) in truncated.chars().enumerate() {
            let x = col + i as u16;
            if x >= col + max_width {
                break;
            }
            surface.set(row, x, Cell::new(c).with_fg(fg.clone()).with_bg(bg.clone()));
        }
        // Clear remainder
        let text_w = visible_width(&truncated) as u16;
        for x in col + text_w..col + max_width {
            surface.set(row, x, Cell::new(' ').with_fg(fg.clone()).with_bg(bg.clone()));
        }
    }
}

impl Component for ModelSelectorOverlay {
    fn name(&self) -> &str {
        "ModelSelectorOverlay"
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
        if !self.visible {
            return false;
        }

        match event {
            Event::Key(KeyEvent { code: KeyCode::Up, .. }) => {
                self.select_prev();
                true
            }
            Event::Key(KeyEvent { code: KeyCode::Down, .. }) => {
                self.select_next();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('k'),
                ..
            }) => {
                self.select_prev();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('j'),
                ..
            }) => {
                self.select_next();
                true
            }
            Event::Key(KeyEvent { code: KeyCode::Enter, .. }) => {
                self.confirm();
                true
            }
            Event::Key(KeyEvent { code: KeyCode::Escape, .. }) => {
                self.cancel();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(c),
                ..
            }) => {
                self.query.push(*c);
                self.apply_filter();
                self.dirty = true;
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                self.query.pop();
                self.apply_filter();
                self.dirty = true;
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        if !self.visible {
            return;
        }

        let palette_w = area.width.min(70);
        let list_height = self.max_visible.min(10);
        // Layout: border + input + border + list + detail + hint + border
        let total_height = 1 + 1 + 1 + list_height as u16 + 1 + 1 + 1;
        let x = area.x + (area.width.saturating_sub(palette_w)) / 2;
        let y = area.y + 2;

        // Fill overlay background
        for r in y..y + total_height {
            for c in x..x + palette_w {
                if r < area.y + area.height && c < area.x + area.width {
                    surface.set(r, c, Cell::new(' ').with_bg(self.theme.overlay_bg));
                }
            }
        }

        let inner_width = palette_w.saturating_sub(2);

        // Top border
        self.render_border_line(surface, y, x, palette_w);

        // Search input row
        let input_row = y + 1;
        let prompt = "> ";
        self.render_text(surface, input_row, x + 1, prompt, self.theme.muted, self.theme.overlay_bg, inner_width);
        let query_start = x + 1 + prompt.len() as u16;
        let query_max = palette_w.saturating_sub(query_start - x) - 1;
        self.render_text(surface, input_row, query_start, &self.query, self.theme.input_text, self.theme.overlay_bg, query_max);
        // Cursor
        let cursor_col = query_start + self.query.len() as u16;
        if cursor_col < x + palette_w - 1 {
            surface.set(input_row, cursor_col, Cell::new(' ').with_fg(Color::Black).with_bg(Color::White));
        }

        // Separator after input
        self.render_border_line(surface, y + 2, x, palette_w);

        // Model list
        let list_start_y = y + 3;
        let (vis_start, vis_end) = self.visible_range();

        if self.filtered_models.is_empty() {
            self.render_text(
                surface,
                list_start_y,
                x + 1,
                "  No matching models",
                self.theme.no_match,
                self.theme.overlay_bg,
                inner_width,
            );
        } else {
            for (vi, row) in (vis_start..vis_end).zip(list_start_y..) {
                if row >= list_start_y + list_height as u16 {
                    break;
                }
                let model = &self.filtered_models[vi];
                let is_selected = vi == self.selected_index;

                let (fg, bg) = if is_selected {
                    (self.theme.selected_text, self.theme.selected_bg)
                } else {
                    (self.theme.normal, self.theme.overlay_bg)
                };

                // Fill row background
                for c in x..x + palette_w {
                    surface.set(row, c, Cell::new(' ').with_fg(fg).with_bg(bg));
                }

                let mut col = x + 1;

                // Selection indicator
                let prefix = if is_selected { "→ " } else { "  " };
                for c in prefix.chars() {
                    surface.set(row, col, Cell::new(c).with_fg(fg).with_bg(bg));
                    col += 1;
                }

                // Model ID (truncated)
                let badge_text = format!(" [{}]", model.provider);
                let current_text = if model.is_current { " ✓" } else { "" };
                let badge_w = badge_text.len() + current_text.len();
                let max_id_w = inner_width as usize - 2 - badge_w;
                let truncated_id = truncate_to_width(&model.id, max_id_w, Some(""), false);
                for c in truncated_id.chars() {
                    surface.set(row, col, Cell::new(c).with_fg(fg).with_bg(bg));
                    col += 1;
                }

                // Provider badge
                for c in badge_text.chars() {
                    if col < x + palette_w - 1 {
                        surface.set(row, col, Cell::new(c).with_fg(self.theme.provider_badge).with_bg(bg));
                        col += 1;
                    }
                }

                // Current badge
                for c in current_text.chars() {
                    if col < x + palette_w - 1 {
                        surface.set(row, col, Cell::new(c).with_fg(self.theme.current_badge).with_bg(bg));
                        col += 1;
                    }
                }
            }

            // Scroll indicator
            if vis_start > 0 || vis_end < self.filtered_models.len() {
                let scroll_y = list_start_y + (vis_end - vis_start).min(list_height) as u16;
                if scroll_y < list_start_y + list_height as u16 {
                    let scroll_text = format!("  ({}/{})", self.selected_index + 1, self.filtered_models.len());
                    self.render_text(surface, scroll_y, x + 1, &scroll_text, self.theme.scroll_info, self.theme.overlay_bg, inner_width);
                }
            }
        }

        // Detail row for selected model
        let detail_y = list_start_y + list_height as u16;
        if let Some(selected) = self.filtered_models.get(self.selected_index) {
            let detail = selected.display_name();
            let detail_text = format!("  {}", detail);
            self.render_text(surface, detail_y, x + 1, &detail_text, self.theme.muted, self.theme.overlay_bg, inner_width);
        } else {
            for c in x..x + palette_w {
                surface.set(detail_y, c, Cell::new(' ').with_bg(self.theme.overlay_bg));
            }
        }

        // Hint row
        let hint_y = detail_y + 1;
        self.render_text(
            surface,
            hint_y,
            x + 1,
            "  Type to search · Enter to select · Esc to cancel",
            self.theme.hint,
            self.theme.overlay_bg,
            inner_width,
        );

        // Bottom border
        let bottom_y = hint_y + 1;
        self.render_border_line(surface, bottom_y, x, palette_w);

        self.dirty = false;
    }

    fn min_size(&self) -> Size {
        Size {
            width: 40,
            height: 8,
        }
    }

    fn on_focus(&mut self) {
        self.focused = true;
        self.dirty = true;
    }

    fn on_unfocus(&mut self) {
        self.focused = false;
        self.hide();
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_models() -> Vec<ModelItem> {
        vec![
            ModelItem::new("anthropic", "claude-sonnet-4-20250501").with_name("Claude Sonnet 4").with_current(true),
            ModelItem::new("anthropic", "claude-haiku-3-20240307").with_name("Claude Haiku 3"),
            ModelItem::new("openai", "gpt-4o").with_name("GPT-4o"),
            ModelItem::new("openai", "gpt-4o-mini").with_name("GPT-4o Mini"),
            ModelItem::new("google", "gemini-2.5-pro").with_name("Gemini 2.5 Pro"),
        ]
    }

    #[test]
    fn test_new_overlay() {
        let models = make_test_models();
        let current = models[0].clone();
        let overlay = ModelSelectorOverlay::new(models, Some(&current), 10);
        assert_eq!(overlay.models.len(), 5);
    }

    #[test]
    fn test_sort_current_first() {
        let models = make_test_models();
        let current = models[0].clone();
        let overlay = ModelSelectorOverlay::new(models, Some(&current), 10);
        // Current model should be first
        assert!(overlay.models[0].is_current);
        assert_eq!(overlay.models[0].id, "claude-sonnet-4-20250501");
    }

    #[test]
    fn test_filter() {
        let models = make_test_models();
        let current = models[0].clone();
        let mut overlay = ModelSelectorOverlay::new(models, Some(&current), 10);
        overlay.query = "gpt".to_string();
        overlay.apply_filter();
        assert!(overlay.filtered_models.len() >= 2);
        assert!(overlay.filtered_models.iter().all(|m| m.id.contains("gpt") || m.provider == "openai"));
    }

    #[test]
    fn test_navigation() {
        let models = make_test_models();
        let current = models[0].clone();
        let mut overlay = ModelSelectorOverlay::new(models, Some(&current), 10);
        assert_eq!(overlay.selected_index, 0);
        overlay.select_next();
        assert_eq!(overlay.selected_index, 1);
        overlay.select_prev();
        assert_eq!(overlay.selected_index, 0);
    }

    #[test]
    fn test_navigation_wrap() {
        let models = make_test_models();
        let mut overlay = ModelSelectorOverlay::new(models, None, 10);
        overlay.selected_index = 0;
        overlay.select_prev();
        assert_eq!(overlay.selected_index, overlay.filtered_models.len() - 1);

        overlay.selected_index = overlay.filtered_models.len() - 1;
        overlay.select_next();
        assert_eq!(overlay.selected_index, 0);
    }

    #[test]
    fn test_show_hide() {
        let models = make_test_models();
        let mut overlay = ModelSelectorOverlay::new(models, None, 10);
        assert!(!overlay.is_visible());
        overlay.show();
        assert!(overlay.is_visible());
        overlay.hide();
        assert!(!overlay.is_visible());
    }

    #[test]
    fn test_display_name() {
        let model = ModelItem::new("anthropic", "claude-sonnet-4").with_name("Claude Sonnet 4");
        assert_eq!(model.display_name(), "Claude Sonnet 4");

        let model_no_name = ModelItem::new("anthropic", "claude-sonnet-4");
        assert_eq!(model_no_name.display_name(), "claude-sonnet-4");
    }

    #[test]
    fn test_selected_model() {
        let models = make_test_models();
        let mut overlay = ModelSelectorOverlay::new(models, None, 10);
        assert!(overlay.selected_model().is_some());
        overlay.select_next();
        assert!(overlay.selected_model().is_some());
        assert_ne!(overlay.selected_model().unwrap().id, overlay.models[0].id);
    }
}
