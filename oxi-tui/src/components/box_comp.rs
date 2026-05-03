//! Box/border component with title and padding support.

use crate::{Cell, Color, Component, Event, Rect, Size, Surface};

/// Border style (character set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    Single,
    Double,
}

/// A box/border component that draws a border around its content area.
pub struct Box {
    title: Option<String>,
    border_style: BorderStyle,
    padding: u16,
    dirty: bool,
    fg_color: Color,
    bg_color: Option<Color>,
}

impl Box {
    pub fn new() -> Self {
        Self {
            title: None,
            border_style: BorderStyle::Single,
            padding: 0,
            dirty: true,
            fg_color: Color::Default,
            bg_color: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    pub fn with_padding(mut self, padding: u16) -> Self {
        self.padding = padding;
        self
    }

    pub fn with_fg(mut self, color: Color) -> Self {
        self.fg_color = color;
        self
    }

    pub fn with_bg(mut self, color: Color) -> Self {
        self.bg_color = Some(color);
        self
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = Some(title.into());
        self.dirty = true;
    }

    /// Get the inner content area (inside border + padding).
    pub fn inner_area(&self, outer: Rect) -> Rect {
        let inset = 1 + self.padding; // border + padding on each side
        outer.inner(inset)
    }

    fn border_chars(&self) -> (char, char, char, char, char, char) {
        // top-left, top-right, bottom-left, bottom-right, horizontal, vertical
        match self.border_style {
            BorderStyle::Single => ('┌', '┐', '└', '┘', '─', '│'),
            BorderStyle::Double => ('╔', '╗', '╚', '╝', '═', '║'),
        }
    }
}

impl Default for Box {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Box {
    fn name(&self) -> &str {
        "Box"
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
        if area.width < 2 || area.height < 2 {
            return;
        }

        let (tl, tr, bl, br, h, v) = self.border_chars();
        let bg = self.bg_color;

        // Fill background
        if let Some(bg_color) = bg {
            for r in area.y..area.y + area.height {
                for c in area.x..area.x + area.width {
                    surface.set(r, c, Cell::new(' ').with_bg(bg_color));
                }
            }
        }

        // Top border
        surface.set(area.y, area.x, Cell::new(tl).with_fg(self.fg_color));
        surface.set(area.y, area.x + area.width - 1, Cell::new(tr).with_fg(self.fg_color));
        for c in area.x + 1..area.x + area.width - 1 {
            surface.set(area.y, c, Cell::new(h).with_fg(self.fg_color));
        }

        // Bottom border
        let bottom = area.y + area.height - 1;
        surface.set(bottom, area.x, Cell::new(bl).with_fg(self.fg_color));
        surface.set(bottom, area.x + area.width - 1, Cell::new(br).with_fg(self.fg_color));
        for c in area.x + 1..area.x + area.width - 1 {
            surface.set(bottom, c, Cell::new(h).with_fg(self.fg_color));
        }

        // Side borders
        for r in area.y + 1..bottom {
            surface.set(r, area.x, Cell::new(v).with_fg(self.fg_color));
            surface.set(r, area.x + area.width - 1, Cell::new(v).with_fg(self.fg_color));
        }

        // Title (rendered on top border)
        if let Some(ref title) = self.title {
            let t = format!(" {} ", title);
            let title_start = area.x + 2;
            for (i, c) in t.chars().enumerate() {
                let col = title_start + i as u16;
                if col < area.x + area.width - 2 {
                    surface.set(area.y, col, Cell::new(c).with_fg(self.fg_color).with_bold());
                }
            }
        }
    }

    fn min_size(&self) -> Size {
        let inset = (1 + self.padding) * 2;
        Size {
            width: 2 + inset,
            height: 2 + inset,
        }
    }
}
