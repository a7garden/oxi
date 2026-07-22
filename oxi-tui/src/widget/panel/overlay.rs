//! `Overlay` — centered modal container with a border and optional title.
//!
//! Sized as a percentage of the parent's `Rect` (`width_pct` /
//! `height_pct`, both in `0.0..=1.0`). The popup is centered: any
//! rounding remainder is split with the top/left getting the extra
//! column so the popup's top-left corner is the visual "anchor".
//!
//! Background = `theme.styles.panel_bg` (the most prominent surface,
//! one rung above `surface_bg` per the brightness hierarchy in
//! `theme/palette.rs`). Border = `theme.styles.border`.
//!
//! The panel is intentionally inert — it owns no children. Callers
//! either paint into the inner `Rect` (after measuring via
//! [`Overlay::computed_inner_rect`]) or wrap the modal in another
//! `Renderable` and let the outer tree drive rendering.

#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::widgets::{Block, Borders};

use crate::widget::{RenderCtx, Renderable, hash_combine, hash_str};

/// A popup modal.
#[derive(Debug, Clone)]
pub struct Overlay {
    title: Option<String>,
    width_pct: f32,
    height_pct: f32,
}

impl Overlay {
    /// Creates an overlay with sensible defaults: no title, 60% width,
    /// 40% height of the parent area.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the title shown above the border's top edge. Pass-through to
    /// `ratatui::widgets::Block::title`.
    #[must_use]
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the size as a percentage of the parent area. Values are
    /// clamped to `0.0..=1.0`; passing 0.0 or 1.0 is allowed (the
    /// renderer handles each edge case).
    #[must_use]
    pub fn sized(mut self, width: f32, height: f32) -> Self {
        self.width_pct = width.clamp(0.0, 1.0);
        self.height_pct = height.clamp(0.0, 1.0);
        self
    }

    /// Computes the outer Rect (border inclusive) the overlay would
    /// occupy given the parent `area`. Returns `None` for degenerate
    /// areas (zero width or height).
    #[must_use]
    pub fn computed_rect(&self, area: Rect) -> Option<Rect> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let w_target = f32::from(area.width) * self.width_pct;
        let h_target = f32::from(area.height) * self.height_pct;
        let popup_w = w_target.round().max(1.0) as u16;
        let popup_h = h_target.round().max(1.0) as u16;
        let popup_w = popup_w.min(area.width);
        let popup_h = popup_h.min(area.height);

        // Center, with extra remainder going to the top/left anchor.
        let extra_x = area.width - popup_w;
        let extra_y = area.height - popup_h;
        let x = area.x + extra_x / 2;
        let y = area.y + extra_y / 2;
        Some(Rect {
            x,
            y,
            width: popup_w,
            height: popup_h,
        })
    }

    /// Inner Rect (border-exclusive) computed from the parent area.
    /// Useful for callers that want to embed a child renderer inside
    /// the modal.
    #[must_use]
    pub fn computed_inner_rect(&self, area: Rect) -> Option<Rect> {
        let outer = self.computed_rect(area)?;
        // Borders consume one row top + one bottom, one column left + right.
        if outer.width <= 2 || outer.height <= 2 {
            return None;
        }
        Some(Rect {
            x: outer.x + 1,
            y: outer.y + 1,
            width: outer.width - 2,
            height: outer.height - 2,
        })
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            title: None,
            width_pct: 0.6,
            height_pct: 0.4,
        }
    }
}

impl Renderable for Overlay {
    fn content_hash(&self) -> u64 {
        // `f32` impls `Hash` via `to_bits` in std, but we fold the title
        // hash with size floats explicitly for stability.
        // `f32::to_bits` returns `u32`; widen to `u64` for hash_combine.
        let title_hash = self.title.as_deref().map_or(0, hash_str);
        hash_combine(
            hash_combine(title_hash, u64::from(self.width_pct.to_bits())),
            u64::from(self.height_pct.to_bits()),
        )
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        // Overlays are anchored to the full parent; a separate
        // `height_for` on the parent tree can multiply this by a known
        // viewport height. Return 0 so a naive parent doesn't double-
        // count the overlay.
        0
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        let Some(outer) = self.computed_rect(area) else {
            return;
        };

        // Snapshot the styles we need before taking a `&mut` to the
        // buffer — `RenderCtx` exposes the buffer mutably and the theme
        // immutably, so we cannot hold both at once.
        let bg_style = ctx.theme().styles.panel_bg;
        let border_style = ctx.theme().styles.border;
        let buf = ctx.buffer_mut();

        // Paint background first so the border draws over a fully-filled
        // rectangle (matches the spec's "most prominent surface" intent).
        for dy in 0..outer.height {
            let y = outer.y + dy;
            for dx in 0..outer.width {
                let x = outer.x + dx;
                let cell = &mut buf[(x, y)];
                cell.set_symbol(" ");
                cell.set_style(bg_style);
            }
        }

        // Border with optional title.
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(bg_style);
        if let Some(title) = self.title.as_deref() {
            block = block.title(title);
        }
        block.render(outer, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn render_overlay(overlay: &mut Overlay, width: u16, height: u16) -> Buffer {
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
            overlay.render(area, &mut ctx);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    #[test]
    fn new_equals_default() {
        let a = Overlay::new();
        let b = Overlay::default();
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn overlay_renders_centered_with_title() {
        let mut overlay = Overlay::new().titled("Confirm").sized(0.5, 0.5);
        // Parent 20×10. Popup ≈ 10×5 centered: x=5, y=2, w=10, h=5.
        let buf = render_overlay(&mut overlay, 20, 10);
        // The top-left border char should be at (5, 2).
        assert_eq!(buf[(5, 2)].symbol(), "┌", "top-left corner");
        // Top-right at (14, 2).
        assert_eq!(buf[(14, 2)].symbol(), "┐", "top-right corner");
        // Bottom-left at (5, 6).
        // After "Confirm" (7 chars at x=5..12) the row at x=13 is back on the top border.
        assert_eq!(buf[(13, 2)].symbol(), "─", "top edge mid cell past title");
        // Title should appear on the top row within the popup span.
        let top_row: String = (5..=14).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(
            top_row.contains("Confirm"),
            "title should appear in top border row, got: {top_row:?}",
        );
    }

    #[test]
    fn overlay_clamps_to_area() {
        let overlay = Overlay::new().sized(2.0, 5.0);
        let rect = overlay
            .computed_rect(Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 6,
            })
            .unwrap();
        assert_eq!(rect.width, 10, "pct > 1.0 should clamp to area.width");
        assert_eq!(rect.height, 6, "pct > 1.0 should clamp to area.height");
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
    }

    #[test]
    fn overlay_zero_pct_gives_minimal_box() {
        let overlay = Overlay::new().sized(0.0, 0.0);
        let rect = overlay
            .computed_rect(Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            })
            .unwrap();
        assert_eq!(rect.width, 1, "0% width rounds up to 1 cell");
        assert_eq!(rect.height, 1, "0% height rounds up to 1 cell");
    }

    #[test]
    fn overlay_centered_in_asymmetric_parent() {
        // 7 × 5 area at offset (3, 1); overlay 50% × 50%.
        // popup_w = round(3.5) = 4; popup_h = round(2.5) = 3 (round half away from zero).
        // extra_x = 3, x = 3 + 3/2 = 4; extra_y = 2, y = 1 + 2/2 = 2.
        let overlay = Overlay::new().sized(0.5, 0.5);
        let rect = overlay
            .computed_rect(Rect {
                x: 3,
                y: 1,
                width: 7,
                height: 5,
            })
            .unwrap();
        assert_eq!(rect.x, 4);
        assert_eq!(rect.y, 2);
        assert_eq!(rect.width, 4);
        assert_eq!(rect.height, 3);
    }

    #[test]
    fn overlay_inner_rect_excludes_borders() {
        let overlay = Overlay::new().titled("X").sized(0.5, 0.5);
        let inner = overlay
            .computed_inner_rect(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 10,
            })
            .unwrap();
        // outer 10×5 → inner 8×3 at (outer.x+1, outer.y+1).
        assert_eq!(inner.width, 8);
        assert_eq!(inner.height, 3);
    }

    #[test]
    fn overlay_inner_rect_none_for_too_small_outer() {
        let overlay = Overlay::new();
        // Force a 2×2 outer by setting pct just under max, with a 2×2 parent.
        let inner = overlay.computed_inner_rect(Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        });
        assert!(inner.is_none(), "border takes 2 cells; no inner room");
    }

    #[test]
    fn overlay_hash_changes_with_title() {
        let a = Overlay::new().titled("A");
        let b = Overlay::new().titled("B");
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn overlay_hash_changes_with_size() {
        let a = Overlay::new().sized(0.4, 0.3);
        let b = Overlay::new().sized(0.4, 0.5);
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn overlay_hash_stable_on_same_state() {
        let a = Overlay::new().titled("X").sized(0.5, 0.5);
        let b = Overlay::new().titled("X").sized(0.5, 0.5);
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn overlay_handles_zero_area() {
        let mut overlay = Overlay::new();
        let buf = render_overlay(&mut overlay, 0, 0);
        assert_eq!(*buf.area(), Rect::new(0, 0, 0, 0));
    }

    #[test]
    fn overlay_size_negatives_clamped_to_zero() {
        let overlay = Overlay::new().sized(-1.0, -2.0);
        let rect = overlay
            .computed_rect(Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 6,
            })
            .unwrap();
        assert_eq!(rect.width, 1, "negative pct clamps to 0, rounds to 1");
        assert_eq!(rect.height, 1);
    }

    #[test]
    fn overlay_height_for_returns_zero() {
        let overlay = Overlay::new();
        let backend = TestBackend::new(1, 1);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let ctx = RenderCtx::new(frame, &theme, &caps);
            assert_eq!(overlay.height_for(80, &ctx), 0);
        })
        .unwrap();
    }

    #[test]
    fn overlay_renders_without_title() {
        let mut overlay = Overlay::new().sized(0.4, 0.4);
        let buf = render_overlay(&mut overlay, 20, 10);
        // Just check that the top-left corner still renders.
        let rect = overlay
            .computed_rect(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 10,
            })
            .unwrap();
        assert_eq!(buf[(rect.x, rect.y)].symbol(), "┌");
    }
}
