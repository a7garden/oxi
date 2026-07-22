//! `Footer` — one-line status bar painted along the bottom of the screen.
//!
//! Layout (single row, left → center → right):
//!
//! ```text
//! | <model>          <in> in / <out> out          $<cost> <spinner> |
//! ```
//!
//! - `model` — left-aligned, `theme.styles.accent` foreground.
//! - tokens — center cluster, `theme.styles.muted` foreground.
//! - cost + spinner glyph — right-aligned, `theme.styles.normal`.
//!
//! The row's background uses `theme.styles.surface_bg` (one rung below
//! `panel_bg`, per the brightness hierarchy in `theme/palette.rs`).
//!
//! Each cell is painted individually rather than via `ratatui::Line`, so
//! per-segment foreground colors survive the `surface_bg` background
//! fill.
//!
//! Cell-coordinate math (row width as `usize`, character counts back to
//! `u16` cell offsets, `usize` token/cost bits widened to `u64`)
//! inherently relies on these casts — mirrors the allow set used by
//! `pipeline::diff_backend`.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_lossless
)]

use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::widget::{RenderCtx, Renderable, hash_combine, hash_str};

/// ASCII spinner glyph table. Kept module-private because
/// `Footer::advance_spinner` rotates through it independently of any
/// `Spinner` widget (the spec deliberately separates them).
const SPINNER_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// One-line status bar.
///
/// `content_hash` changes whenever any field affecting the rendered line
/// changes (model, tokens, cost, spinner phase). Callers that mutate via
/// `set_*` get correct memoization out of the box — see `Renderable`.
#[derive(Debug, Clone)]
pub struct Footer {
    model: String,
    tokens_in: u64,
    tokens_out: u64,
    cost: f64,
    spinner_phase: usize,
}

impl Footer {
    /// Creates an empty footer (no model, zero tokens, zero cost, phase 0).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the displayed model name (e.g. `"claude-sonnet-4.5"`).
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    /// Sets the cumulative input/output token counters.
    pub const fn set_tokens(&mut self, tin: u64, tout: u64) {
        self.tokens_in = tin;
        self.tokens_out = tout;
    }

    /// Sets the cumulative cost in USD.
    pub const fn set_cost(&mut self, cost: f64) {
        self.cost = cost;
    }

    /// Advances the spinner by one frame. Wraps modulo the frame count,
    /// so after `SPINNER_FRAMES.len()` advances the hash returns to its
    /// starting value (verifiable by `spinner_phase_wraps` below).
    pub fn advance_spinner(&mut self) {
        self.spinner_phase = self.spinner_phase.wrapping_add(1) % SPINNER_FRAMES.len();
    }

    /// The three text segments, in left-to-right display order.
    fn segments(&self) -> (String, String, String) {
        let model = if self.model.is_empty() {
            "—".to_string()
        } else {
            self.model.clone()
        };
        let tokens = format!("{} in / {} out", self.tokens_in, self.tokens_out);
        let cost = format_cost(self.cost);
        let spinner = SPINNER_FRAMES[self.spinner_phase % SPINNER_FRAMES.len()];
        let right = format!("{cost} {spinner}");
        (model, tokens, right)
    }
}

impl Default for Footer {
    fn default() -> Self {
        Self {
            model: String::new(),
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            spinner_phase: 0,
        }
    }
}

impl Renderable for Footer {
    fn content_hash(&self) -> u64 {
        // `f64` is not `Hash`; `to_bits()` is a stable surrogate that
        // changes iff the displayed value changes.
        hash_combine(
            hash_combine(
                hash_str(&self.model),
                hash_combine(self.tokens_in, self.tokens_out),
            ),
            hash_combine(self.cost.to_bits(), self.spinner_phase as u64),
        )
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        1
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let styles = ctx.theme().styles;
        let bg_style = styles.surface_bg;
        let fg_model = bg_style.patch(styles.accent);
        let fg_tokens = bg_style.patch(styles.muted);
        let fg_right = bg_style.patch(styles.normal);

        let (model, tokens, right) = self.segments();

        // Paint the whole row with bg + space first so any cell not
        // covered by text still shows the surface_bg background.
        let buf = ctx.buffer_mut();
        for dx in 0..area.width {
            let x = area.x + dx;
            let cell = &mut buf[(x, area.y)];
            cell.set_symbol(" ");
            cell.set_style(bg_style);
        }

        let width = area.width as usize;
        let model_len = model.chars().count();
        let tokens_len = tokens.chars().count();
        let right_len = right.chars().count();

        // Required minimum: 1 cell side-pad + 1 sep between model|tokens
        // and tokens|right + the three blocks themselves.
        let total = model_len + tokens_len + right_len + 3;
        if total > width {
            paint_left_to_right(
                buf,
                area,
                width,
                &[
                    (model.as_str(), fg_model),
                    (tokens.as_str(), fg_tokens),
                    (right.as_str(), fg_right),
                ],
            );
            return;
        }

        let slack = width - (model_len + tokens_len + right_len + 3);
        // 1 leading pad + 1 separator + 1 trailing separator + slack.
        // Distribute slack so the right cluster hugs the right edge:
        // put slack/2 in the left gap and remainder + 1 trailing pad.
        let left_gap = slack / 2;

        // Layout: [1 pad][model][1 sep + left_gap][tokens][1 sep + trailing_pad][right]
        let mut col = area.x as usize;
        col = paint_text_clipped(buf, area.y, col, " ", bg_style, width, 1);
        col = paint_text_clipped(buf, area.y, col, &model, fg_model, width, model_len);
        col = paint_text_clipped(buf, area.y, col, " ", bg_style, width, 1 + left_gap);
        col = paint_text_clipped(buf, area.y, col, &tokens, fg_tokens, width, tokens_len);
        let trailing_pad = 1 + (slack - left_gap);
        col = paint_text_clipped(buf, area.y, col, " ", bg_style, width, trailing_pad);
        let remaining = width.saturating_sub(col - area.x as usize);
        let _ = paint_text_clipped(buf, area.y, col, &right, fg_right, width, remaining);
    }
}

fn format_cost(cost: f64) -> String {
    if cost < 0.0 {
        format!("-${:.2}", cost.abs())
    } else {
        format!("${cost:.2}")
    }
}

/// Paint `count` characters of `text` (or spaces if `text == " "`) at
/// column `col`, row `y`. Stops at the row width and returns the new
/// column index.
fn paint_text_clipped(
    buf: &mut ratatui::buffer::Buffer,
    y: u16,
    col: usize,
    text: &str,
    style: Style,
    row_width: usize,
    count: usize,
) -> usize {
    let mut col = col;
    let max = col.saturating_add(count).min(row_width);
    let mut emitted = 0usize;
    for ch in text.chars() {
        if col >= max || emitted >= count {
            break;
        }
        let cell = &mut buf[(col as u16, y)];
        let mut s = String::with_capacity(ch.len_utf8());
        s.push(ch);
        cell.set_symbol(&s);
        cell.set_style(style);
        col = col.saturating_add(1);
        emitted = emitted.saturating_add(1);
    }
    // For a space padding, the `text.chars()` loop emitted `emitted` chars.
    // Reach `count` exactly by filling the remainder with spaces of the
    // same style. (For non-space text, `count` should already equal
    // text.chars().count().)
    if text == " " && emitted < count {
        while col < max && emitted < count {
            let cell = &mut buf[(col as u16, y)];
            cell.set_symbol(" ");
            cell.set_style(style);
            col = col.saturating_add(1);
            emitted = emitted.saturating_add(1);
        }
    }
    col
}

/// Narrow-row fallback: paint each block left-to-right, using only the
/// remaining room for each successive block.
fn paint_left_to_right(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    row_width: usize,
    segments: &[(&str, Style); 3],
) {
    let mut col = area.x as usize;
    for (text, style) in segments {
        let remaining = row_width.saturating_sub(col - area.x as usize);
        if remaining == 0 {
            break;
        }
        col = paint_text_clipped(buf, area.y, col, text, *style, row_width, remaining);
    }
    // Trailing pad in the rightmost style so the visual rhythm matches
    // a wide row (right segment uses fg_normal).
    while col < row_width {
        let cell = &mut buf[(col as u16, area.y)];
        cell.set_symbol(" ");
        cell.set_style(segments[2].1);
        col = col.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn render_footer(footer: &mut Footer, width: u16, height: u16) -> Buffer {
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
            footer.render(area, &mut ctx);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    #[test]
    fn new_equals_default() {
        let a = Footer::new();
        let b = Footer::default();
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn footer_renders_model_and_tokens() {
        let mut footer = Footer::new();
        footer.set_model("claude-sonnet-4.5");
        footer.set_tokens(123, 456);
        footer.set_cost(0.42);
        let buf = render_footer(&mut footer, 80, 1);
        let row: String = (0..80u16)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            row.contains("claude-sonnet-4.5"),
            "row should contain model name, got: {row:?}",
        );
        assert!(row.contains("123"), "row should contain input tokens");
        assert!(row.contains("456"), "row should contain output tokens");
        assert!(row.contains('$'), "row should contain cost");
        assert!(row.contains("in"), "row should contain `in` label");
        assert!(row.contains("out"), "row should contain `out` label");
    }

    #[test]
    fn footer_spinner_advances_hash() {
        let mut footer = Footer::new();
        let h0 = footer.content_hash();
        footer.advance_spinner();
        let h1 = footer.content_hash();
        footer.advance_spinner();
        let h2 = footer.content_hash();
        assert_ne!(h0, h1, "first advance should change hash");
        assert_ne!(h1, h2, "second advance should change hash");
        assert_ne!(h0, h2, "hash should differ across phases");
    }

    #[test]
    fn spinner_phase_wraps_modulo_frame_count() {
        let mut footer = Footer::new();
        let h_at_start = footer.content_hash();
        for _ in 0..(SPINNER_FRAMES.len() * 3) {
            footer.advance_spinner();
        }
        assert_eq!(
            footer.content_hash(),
            h_at_start,
            "spinner phase should wrap after SPINNER_FRAMES.len() advances",
        );
    }

    #[test]
    fn footer_hash_changes_on_data_mutation() {
        let mut footer = Footer::new();
        let h0 = footer.content_hash();
        footer.set_model("gpt-4");
        assert_ne!(footer.content_hash(), h0);
        footer.set_tokens(10, 20);
        let h_after_tokens = footer.content_hash();
        footer.set_cost(0.5);
        assert_ne!(footer.content_hash(), h_after_tokens);
        let _ = h_after_tokens;
    }

    #[test]
    fn footer_hash_stable_on_same_state() {
        let mut a = Footer::new();
        a.set_model("same");
        a.set_tokens(1, 2);
        a.set_cost(0.01);
        let mut b = Footer::new();
        b.set_model("same");
        b.set_tokens(1, 2);
        b.set_cost(0.01);
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn empty_render_paints_no_cells() {
        let mut footer = Footer::new();
        let buf = render_footer(&mut footer, 0, 0);
        assert_eq!(*buf.area(), Rect::new(0, 0, 0, 0));
    }

    #[test]
    fn footer_fits_narrow_terminal() {
        let mut footer = Footer::new();
        footer.set_model("claude");
        footer.set_tokens(1, 2);
        footer.set_cost(0.1);
        let buf = render_footer(&mut footer, 10, 1);
        let joined: String = (0..10u16)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert_eq!(joined.chars().count(), 10);
    }
}
