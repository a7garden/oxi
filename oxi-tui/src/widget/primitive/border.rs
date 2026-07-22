//! `Border` — bordered box with optional title.
//!
//! Thin wrapper around `ratatui::widgets::Block`. The border is drawn on
//! every render call; the rest of the area is left untouched so parent
//! widgets can render their own content inside.

use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders};

use crate::widget::{RenderCtx, Renderable, hash_combine, hash_str};

#[derive(Debug, Clone, Default)]
pub struct Border {
    title: Option<String>,
    style: Style,
    title_style: Style,
}

impl Border {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    #[must_use]
    pub fn title_styled(mut self, style: Style) -> Self {
        self.title_style = style;
        self
    }
}

impl Renderable for Border {
    fn content_hash(&self) -> u64 {
        let title_hash = self.title.as_deref().map_or(0, hash_str);
        // Style bits: encode fg/bg/add_modifier into u64 the same way `Text` does
        // so the hash changes whenever the visible appearance changes.
        let style_bits = u64::from(self.style.fg.is_some())
            | (u64::from(self.style.bg.is_some()) << 1)
            | (u64::from(self.style.add_modifier.bits()) << 2);
        let title_style_bits = u64::from(self.title_style.fg.is_some())
            | (u64::from(self.title_style.bg.is_some()) << 1)
            | (u64::from(self.title_style.add_modifier.bits()) << 2);
        hash_combine(hash_combine(title_hash, style_bits), title_style_bits)
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        // Border is a single thin wrapper; height is dictated by the
        // area the parent gives it. Returning 1 signals "non-tall";
        // parents render the box at the area they allocate.
        1
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        let mut block = Block::default().borders(Borders::ALL).style(self.style);
        if let Some(title) = self.title.as_deref() {
            block = block.title(title).title_style(self.title_style);
        }
        block.render(area, ctx.buffer_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn render_border(border: &mut Border, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let area = Rect {
                x: 0,
                y: 0,
                width,
                height,
            };
            border.render(area, &mut ctx);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    #[test]
    fn border_renders_box_with_title() {
        let mut border = Border::new().titled("hello");
        let buf = render_border(&mut border, 10, 3);
        // Top-left corner should be the box-drawing character ┌
        assert_eq!(buf[(0, 0)].symbol(), "┌");
        // Top-right corner should be ┐
        assert_eq!(buf[(9, 0)].symbol(), "┐");
        // Bottom-left corner should be └
        assert_eq!(buf[(0, 2)].symbol(), "└");
        // Bottom-right corner should be ┘
        assert_eq!(buf[(9, 2)].symbol(), "┘");
        // Left edge mid cell should be │
        assert_eq!(buf[(0, 1)].symbol(), "│");
        // Right edge mid cell should be │
        assert_eq!(buf[(9, 1)].symbol(), "│");
        // Title text appears somewhere on the top row
        let top_row: String = (0..10).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            top_row.contains("hello"),
            "title should appear in top row, got: {top_row:?}"
        );
    }

    #[test]
    fn border_hash_changes_with_title() {
        let a = Border::new().titled("a");
        let b = Border::new().titled("b");
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn border_hash_stable_on_same_title() {
        let a = Border::new().titled("same");
        let b = Border::new().titled("same");
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn border_hash_changes_with_style() {
        let plain = Border::new();
        let bold =
            Border::new().styled(Style::default().add_modifier(ratatui::style::Modifier::BOLD));
        assert_ne!(plain.content_hash(), bold.content_hash());
    }

    #[test]
    fn border_without_title_renders_box() {
        let mut border = Border::new();
        let buf = render_border(&mut border, 6, 3);
        assert_eq!(buf[(0, 0)].symbol(), "┌");
        assert_eq!(buf[(5, 0)].symbol(), "┐");
    }
}
