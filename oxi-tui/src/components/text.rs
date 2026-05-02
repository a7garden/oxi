//! Text component - displays static or dynamic text.

use crate::{Cell, Color, Component, Event, Rect, Surface, Size, Theme};

/// Text component configuration.
#[derive(Debug, Clone)]
pub struct TextOptions {
    /// Text color.
    pub fg_color: Option<Color>,
    /// Background color.
    pub bg_color: Option<Color>,
    /// Alignment.
    pub align: TextAlign,
    /// When set, theme colors are used as defaults (overridden by explicit fg/bg).
    pub theme: Option<Theme>,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self {
            fg_color: None,
            bg_color: None,
            align: TextAlign::Left,
            theme: None,
        }
    }
}

impl TextOptions {
    /// Create options pre-filled from a theme.
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            fg_color: Some(theme.colors.foreground),
            bg_color: Some(theme.colors.background),
            align: TextAlign::Left,
            theme: Some(theme.clone()),
        }
    }
}

/// Text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// A simple text component.
pub struct Text {
    content: String,
    options: TextOptions,
    dirty: bool,
}

impl Text {
    pub fn new(content: &str) -> Self {
        Self {
            content: content.to_string(),
            options: TextOptions::default(),
            dirty: true,
        }
    }

    pub fn with_options(content: &str, options: TextOptions) -> Self {
        Self {
            content: content.to_string(),
            options,
            dirty: true,
        }
    }

    /// Set the theme; colors from the theme are used unless explicitly overridden.
    pub fn set_theme(&mut self, theme: &Theme) {
        self.options.theme = Some(theme.clone());
        // Only override if user hasn't set explicit colors
        if self.options.fg_color.is_none() {
            self.options.fg_color = Some(theme.colors.foreground);
        }
        if self.options.bg_color.is_none() {
            self.options.bg_color = Some(theme.colors.background);
        }
        self.dirty = true;
    }

    pub fn set_text(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.dirty = true;
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }
}

impl Component for Text {
    fn name(&self) -> &str {
        "Text"
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

    fn handle_event(&mut self, _event: &Event) -> bool {
        false
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        // Calculate alignment offset
        let text_width = self.content.len() as u16;
        let x_offset = match self.options.align {
            TextAlign::Left => 0,
            TextAlign::Center => (area.width.saturating_sub(text_width)) / 2,
            TextAlign::Right => area.width.saturating_sub(text_width),
        };

        // Write text
        let start_col = area.x + x_offset;
        for (i, c) in self.content.chars().enumerate() {
            let col = start_col + i as u16;
            if col < area.x + area.width {
                let mut cell = Cell::new(c);
                if let Some(fg) = self.options.fg_color {
                    cell.fg = fg;
                }
                if let Some(bg) = self.options.bg_color {
                    cell.bg = bg;
                }
                surface.set(area.y, col, cell);
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: self.content.len() as u16,
            height: 1,
        }
    }
}