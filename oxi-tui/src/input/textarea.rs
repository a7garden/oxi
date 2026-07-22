//! `InputArea` — `Renderable` wrapper around stock `ratatui-textarea` 0.9.
//!
//! The textarea widget handles all input mechanics:
//! - Insert/delete characters and word-boundary deletion
//! - Cursor movement (arrows, Home/End, Ctrl+Left/Right for words)
//! - Undo/Redo
//! - Soft-wrap via `WrapMode::Word`
//! - Bracketed-paste IME-friendly input
//!
//! `InputArea` only adapts between the `Renderable` contract (content
//! hash, height-for-width, cursor slot) and the `Widget` API. Render uses
//! the textarea's `Widget` impl via a clone (the underlying `Widget::render`
//! takes `self` by value; cloning the textarea is cheap — it's a small
//! `Vec<String>` + index fields).
//!
//! ## Layout
//!
//! `InputArea` paints its whole area with the theme background before
//! delegating to the textarea (see legacy `widgets/input.rs` for the
//! rationale: avoids "stuck cursor-bg" afterimages when Backspace empties
//! a cell). The textarea itself is rendered into the same area — there is
//! NO outer `Block` border and NO horizontal padding here, so the visible
//! text uses the full width. The terminal cursor lands at
//! `(area.x + col, area.y + row)` — `screen_cursor()` returns offsets
//! relative to the textarea's rendered top-left, which is exactly `area`.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui_textarea::{TextArea, WrapMode};

use crate::widget::{RenderCtx, Renderable, hash_combine, hash_str};

/// Multi-line text input rendered into the widget tree.
///
/// `textarea_mut()` exposes the underlying `TextArea` for callers that need
/// to feed keys, set placeholders, or query cursor/scroll state. Every
/// mutation via `textarea_mut()` changes `content_hash()` because the hash
/// is recomputed from the live text + cursor on every call (no cached
/// field would otherwise observe cursor moves done through the mutable
/// reference).
#[derive(Debug, Default)]
pub struct InputArea {
    textarea: TextArea<'static>,
}

impl InputArea {
    /// Build an empty input area with sane defaults (no line-number gutter,
    /// soft word-wrap, no cursor-line highlight).
    #[must_use]
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.remove_line_number();
        textarea.set_cursor_line_style(Style::default());
        textarea.set_wrap_mode(WrapMode::Word);
        Self { textarea }
    }

    /// Borrow the underlying `TextArea` for direct key/paste/selection ops.
    #[must_use]
    pub fn textarea(&self) -> &TextArea<'static> {
        &self.textarea
    }

    /// Mutably borrow the underlying `TextArea`. Mutations made through this
    /// reference are reflected in the next `content_hash()` and `render()`.
    #[must_use]
    pub fn textarea_mut(&mut self) -> &mut TextArea<'static> {
        &mut self.textarea
    }

    /// Joined multi-line text (lines separated by `\n`).
    #[must_use]
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Replace the entire content. Clears the textarea and inserts `text`
    /// at the (new) cursor.
    pub fn set_text(&mut self, text: &str) {
        self.textarea.clear();
        if !text.is_empty() {
            self.textarea.insert_str(text);
        }
    }

    /// Compute the wrapped line count at `width` (the area the parent
    /// will hand us). Uses `Paragraph::line_count` so it matches what
    /// the textarea actually paints after `WrapMode::Word`.
    fn wrapped_height(&self, width: u16) -> u16 {
        if width < 1 {
            return 1;
        }
        let lines = self.textarea.lines();
        if lines.is_empty() || (lines.len() == 1 && lines[0].is_empty()) {
            return 1;
        }
        let text = Text::from(lines.join("\n"));
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
        u16::try_from(paragraph.line_count(width))
            .unwrap_or(u16::MAX)
            .max(1)
    }
}

impl Renderable for InputArea {
    fn content_hash(&self) -> u64 {
        // Hash covers BOTH text and cursor position. Cursor moves via
        // `textarea_mut()` don't touch any cached field, so we recompute
        // the hash from the live state every call. Cheap — `lines()` is
        // a slice ref and `screen_cursor()` is two integer reads.
        let text_hash = hash_str(&self.text());
        let cursor = self.textarea.screen_cursor();
        let cursor_hash = hash_combine(cursor.row as u64, cursor.col as u64);
        hash_combine(text_hash, cursor_hash)
    }

    fn height_for(&self, width: u16, _ctx: &RenderCtx) -> u16 {
        self.wrapped_height(width)
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        if area.height < 1 || area.width < 1 {
            return;
        }
        // Paint the area background first so vacated cells (e.g. after
        // Backspace) don't leave stale cursor-colored afterimages.
        // Copy the bg color out of `ctx` before taking a `&mut Buffer`
        // borrow (Rust 2024 NLL still flags the overlapping borrow).
        let bg = ctx.theme().colors.background;
        let fg = ctx.theme().colors.foreground;
        let buf: &mut Buffer = ctx.buffer_mut();
        let textarea = &mut self.textarea;
        textarea.set_style(Style::default().fg(fg).bg(bg));
        textarea.set_cursor_line_style(Style::default());

        // Render the textarea. `Widget::render` consumes `self`, so clone
        // the (cheap) textarea and render the clone.
        let to_render = textarea.clone();
        to_render.render(area, buf);

        // Hand the cursor position to the pipeline via the cursor slot.
        // `screen_cursor()` returns offsets relative to the textarea's
        // rendered top-left = `area`, so translate by `area.x`/`area.y`.
        let sc = textarea.screen_cursor();
        // Saturating arithmetic: cursor.row/col are usize; clamp to u16
        // then add the area offset.
        let x = area
            .x
            .saturating_add(u16::try_from(sc.col).unwrap_or(u16::MAX));
        let y = area
            .y
            .saturating_add(u16::try_from(sc.row).unwrap_or(u16::MAX));
        ctx.set_cursor(Position { x, y });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn make_ctx<'a, 'f>(
        frame: &'a mut ratatui::Frame<'f>,
        theme: &'a Theme,
        caps: &'a TerminalCaps,
    ) -> RenderCtx<'a, 'f> {
        RenderCtx::new(frame, theme, caps)
    }

    /// Render a single frame at `(width, height)` and run `f(ctx)`.
    /// Returns the terminal buffer for assertions.
    fn render_frame<F>(width: u16, height: u16, mut f: F) -> ratatui::buffer::Buffer
    where
        F: FnMut(&mut RenderCtx<'_, '_>),
    {
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = make_ctx(frame, &theme, &caps);
            f(&mut ctx);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    #[test]
    fn input_area_renders_text() {
        let mut input = InputArea::new();
        input.set_text("hello world");

        let buf = render_frame(20, 3, |ctx| {
            let area = Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 3,
            };
            input.render(area, ctx);
        });

        let row: String = (0..20u16)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            row.starts_with("hello world"),
            "first row should start with the typed text, got: {row:?}",
        );
    }

    #[test]
    fn input_area_hash_changes_on_text_change() {
        let mut input = InputArea::new();
        let h0 = input.content_hash();
        input.set_text("first");
        let h1 = input.content_hash();
        input.set_text("second");
        let h2 = input.content_hash();

        assert_ne!(h0, h1, "adding text should change the hash");
        assert_ne!(h1, h2, "changing text should change the hash");
    }

    #[test]
    fn input_area_hash_changes_on_cursor_move() {
        // Cursor moves happen via `textarea_mut()` — no cached field
        // observes them, so content_hash() MUST recompute against live
        // state. This guards against any future "cache the hash" patch.
        let mut input = InputArea::new();
        input.set_text("abc");
        // `set_text` via insert_str leaves cursor at end. Move home first
        // so we have a guaranteed different start state, then forward to
        // a non-end column.
        input
            .textarea_mut()
            .move_cursor(ratatui_textarea::CursorMove::Head);
        let h_before = input.content_hash();

        input
            .textarea_mut()
            .move_cursor(ratatui_textarea::CursorMove::Forward);
        let h_after = input.content_hash();

        assert_ne!(
            h_before, h_after,
            "cursor move must change content_hash (no field cache)",
        );
    }

    #[test]
    fn input_area_sets_cursor_in_ctx() {
        // Verify `render()` writes a CursorSlot::Show(Position) into the
        // RenderCtx whose (x, y) falls inside the rendered area.
        let mut input = InputArea::new();
        input.set_text("hi");

        let backend = TestBackend::new(20, 3);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = make_ctx(frame, &theme, &caps);
            let area = Rect {
                x: 2,
                y: 1,
                width: 20,
                height: 3,
            };
            input.render(area, &mut ctx);

            let slot = ctx.take_cursor_slot();
            match slot {
                crate::pipeline::CursorSlot::Show(pos) => {
                    assert!(
                        pos.x >= area.x && pos.x < area.x + area.width,
                        "cursor x {} out of rendered area x range [{}..{})",
                        pos.x,
                        area.x,
                        area.x + area.width,
                    );
                    assert!(
                        pos.y >= area.y && pos.y < area.y + area.height,
                        "cursor y {} out of rendered area y range [{}..{})",
                        pos.y,
                        area.y,
                        area.y + area.height,
                    );
                }
                other => panic!("expected CursorSlot::Show, got {other:?}"),
            }
        })
        .unwrap();
    }

    #[test]
    fn height_for_grows_with_wrapped_lines() {
        // Two short lines at width 20 → height 2.
        let mut input_a = InputArea::new();
        input_a.set_text("one\ntwo");
        let h_wide = height_in(&input_a, 20);
        assert!(h_wide >= 2, "two lines should occupy ≥2 rows, got {h_wide}");

        // Long single line at narrow width should wrap.
        let mut input_b = InputArea::new();
        input_b.set_text("aaaaaaaaaa");
        let h_narrow = height_in(&input_b, 5);
        assert!(
            h_narrow >= 2,
            "long line should wrap at narrow width, got {h_narrow}",
        );
    }

    /// Call `height_for` with a throwaway `RenderCtx`. `height_for` doesn't
    /// actually use the ctx, but the signature requires one — build a
    /// minimal frame just to satisfy the type.
    fn height_in(input: &InputArea, width: u16) -> u16 {
        let backend = TestBackend::new(width.max(1), 1);
        let mut term = Terminal::new(backend).unwrap();
        let mut h = 0_u16;
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let ctx = make_ctx(frame, &theme, &caps);
            h = input.height_for(width, &ctx);
        })
        .unwrap();
        h
    }
}
