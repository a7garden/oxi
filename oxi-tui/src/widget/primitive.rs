//! Minimal primitive widgets. The full set (Border, List, Scrollbar) comes
//! in Plan B. This module just has `Text` for integration testing of the
//! pipeline + retained tree.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::widget::{RenderCtx, Renderable, hash_str};

#[derive(Debug, Clone)]
pub struct Text {
    content: String,
    style: Style,
    cached_hash: u64,
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self {
        let content = content.into();
        let cached_hash = hash_str(&content);
        Self {
            content,
            style: Style::default(),
            cached_hash,
        }
    }

    #[must_use]
    pub fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    #[must_use]
    pub fn fg(mut self, color: Color) -> Self {
        self.style = self.style.fg(color);
        self
    }

    #[must_use]
    pub fn bold(mut self) -> Self {
        self.style = self.style.add_modifier(Modifier::BOLD);
        self
    }

    pub fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        self.cached_hash = hash_str(&content);
        self.content = content;
    }
}

impl Renderable for Text {
    fn content_hash(&self) -> u64 {
        // Combine content hash with style bits for correctness
        let style_bits = u64::from(self.style.fg.is_some())
            | (u64::from(self.style.bg.is_some()) << 1)
            | (u64::from(self.style.add_modifier.bits()) << 2);
        crate::widget::hash_combine(self.cached_hash, style_bits)
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        u16::try_from(self.content.lines().count().max(1)).unwrap_or(u16::MAX)
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        let line = Line::from(vec![Span::styled(self.content.clone(), self.style)]);
        ctx.buffer_mut().set_line(area.x, area.y, &line, area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_hash_changes_on_set_content() {
        let mut t = Text::new("hello");
        let h1 = t.content_hash();
        t.set_content("world");
        let h2 = t.content_hash();
        assert_ne!(h1, h2);
    }

    #[test]
    fn text_hash_stable_on_same_content() {
        let t1 = Text::new("hello");
        let t2 = Text::new("hello");
        assert_eq!(t1.content_hash(), t2.content_hash());
    }

    #[test]
    fn text_hash_differs_with_style() {
        let plain = Text::new("hi");
        let bold = Text::new("hi").bold();
        assert_ne!(plain.content_hash(), bold.content_hash());
    }
}
