//! `Scrollbar` — vertical scroll position indicator.
//!
//! Manually drawn: a vertical track of `│` characters with a single `█`
//! thumb at `position / total` of the way down. The widget is a single
//! column wide and as tall as the area it is given.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::widget::{RenderCtx, Renderable, hash_combine};

/// Vertical scrollbar. Renders a track with a thumb at `position` within
/// `total` content rows, where `visible` is the height of the visible
/// window in rows.
#[derive(Debug, Clone)]
pub struct Scrollbar {
    position: usize,
    total: usize,
    visible: usize,
}

impl Scrollbar {
    #[must_use]
    pub fn new(total: usize, visible: usize) -> Self {
        Self {
            position: 0,
            total,
            visible,
        }
    }

    pub fn set_position(&mut self, pos: usize) {
        self.position = pos;
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.position
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.total
    }

    #[must_use]
    pub fn visible(&self) -> usize {
        self.visible
    }
}

impl Renderable for Scrollbar {
    fn content_hash(&self) -> u64 {
        // Mix position + total + visible; position is the most likely to
        // change frame-to-frame, so XOR it in first.
        let mut h = hash_combine(self.position as u64, self.total as u64);
        h = hash_combine(h, self.visible as u64);
        h
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        // The scrollbar's height is dictated by the area the parent gives
        // it. Returning 1 marks it as a thin vertical control.
        1
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        // Track characters are dim; the thumb is bright.
        let track_style = Style::default().fg(Color::DarkGray);
        let thumb_style = Style::default().fg(Color::White);

        let total = self.total.max(1);
        let height = area.height as usize;

        // Place the thumb at `position / total` of the track. Saturate
        // `position` to `total.saturating_sub(1)` so the thumb never
        // falls off the end when the user has scrolled past the last
        // row.
        let thumb_y = area.y
            + u16::try_from(self.position.min(total.saturating_sub(1)) * height / total)
                .unwrap_or(0);

        let buf = ctx.buffer_mut();
        for dy in 0..height {
            let y = area.y + u16::try_from(dy).unwrap_or(0);
            let cell = &mut buf[(area.x, y)];
            if y == thumb_y {
                cell.set_symbol("█");
                cell.set_style(thumb_style);
            } else {
                cell.set_symbol("│");
                cell.set_style(track_style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn render_scrollbar(sb: &mut Scrollbar, height: u16) -> Buffer {
        let backend = TestBackend::new(1, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let area = ctx.area();
            sb.render(area, &mut ctx);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    #[test]
    fn scrollbar_renders_thumb_at_correct_position() {
        // 10 content rows, 5 visible. Position 5 / total 10 = 50% of the
        // 5-row track → thumb at row 2.
        let mut sb = Scrollbar::new(10, 5);
        sb.set_position(5);
        let buf = render_scrollbar(&mut sb, 5);
        assert_eq!(buf[(0, 0)].symbol(), "│");
        assert_eq!(buf[(0, 1)].symbol(), "│");
        assert_eq!(buf[(0, 2)].symbol(), "█");
        assert_eq!(buf[(0, 3)].symbol(), "│");
        assert_eq!(buf[(0, 4)].symbol(), "│");
    }

    #[test]
    fn scrollbar_position_zero_thumb_at_top() {
        let mut sb = Scrollbar::new(10, 5);
        let buf = render_scrollbar(&mut sb, 5);
        assert_eq!(buf[(0, 0)].symbol(), "█");
        assert_eq!(buf[(0, 4)].symbol(), "│");
    }

    #[test]
    fn scrollbar_position_at_end_thumb_at_bottom() {
        let mut sb = Scrollbar::new(10, 5);
        sb.set_position(9);
        let buf = render_scrollbar(&mut sb, 5);
        assert_eq!(buf[(0, 0)].symbol(), "│");
        // position 9 / total 10 → 9 * 5 / 10 = 4 → thumb at row 4.
        assert_eq!(buf[(0, 4)].symbol(), "█");
    }

    #[test]
    fn scrollbar_hash_changes_with_position() {
        let a = Scrollbar::new(10, 5);
        let mut b = Scrollbar::new(10, 5);
        b.set_position(1);
        assert_ne!(a.content_hash(), b.content_hash());
    }
}
