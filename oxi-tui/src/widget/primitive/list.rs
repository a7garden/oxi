//! `List<T>` — virtualized list of `Renderable` items.
//!
//! Only items in the visible window are rendered. Each item is wrapped in
//! `RetainedChild<T>` so an item whose `content_hash` did not change skips
//! its own render call. The list's own `content_hash` combines the scroll
//! position with the visible items' hashes so a scroll change trips the
//! parent skip even when no item mutated.

use ratatui::layout::Rect;

use crate::widget::{RenderCtx, Renderable, RetainedChild, hash_combine, hash_str};

#[derive(Debug)]
pub struct List<T: Renderable> {
    items: Vec<RetainedChild<T>>,
    scroll: usize,
}

impl<T: Renderable> Default for List<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Renderable> List<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            scroll: 0,
        }
    }

    /// Append a new item to the end of the list.
    pub fn push(&mut self, item: T) {
        self.items.push(RetainedChild::new(item));
    }

    /// Set the scroll offset to `pos`. Items before `pos` are skipped.
    /// Saturates to `self.items.len()` so callers can pass unchecked input.
    pub fn scroll_to(&mut self, pos: usize) {
        self.scroll = pos.min(self.items.len());
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn scroll(&self) -> usize {
        self.scroll
    }
}

impl<T: Renderable> Renderable for List<T> {
    fn content_hash(&self) -> u64 {
        // Combine scroll position + every visible item's content hash.
        // Off-screen items are intentionally excluded — a change in an
        // off-screen item must still bubble up via the scroll change that
        // happens when the user scrolls into it, but for the current
        // frame's skip decision the visible window is what matters.
        //
        // For correctness against the broader contract, we also fold
        // `self.items.len()` in so the hash changes when items are
        // pushed (the visible window may now show different items even
        // at the same scroll offset).
        let mut h = hash_combine(hash_str(&self.scroll.to_string()), self.items.len() as u64);
        for item in self.items.iter().skip(self.scroll) {
            h = hash_combine(h, item.inner().content_hash());
        }
        h
    }

    fn height_for(&self, width: u16, ctx: &RenderCtx) -> u16 {
        // Sum the height of each visible item. `RetainedChild::height_for`
        // takes `&mut self`, but `self` is `&List`, so we go through the
        // `&T` accessor and call the underlying `Renderable::height_for`.
        let mut total: u32 = 0;
        for item in self.items.iter().skip(self.scroll) {
            total = total.saturating_add(u32::from(item.inner().height_for(width, ctx)));
        }
        u16::try_from(total).unwrap_or(u16::MAX)
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        // Layout each visible item in a stacked column starting at `area.y`.
        // Each item's height is determined by its own `height_for`; we
        // advance the y cursor by that height. Visible items past the
        // area bottom are still passed to `render_if_changed` — the
        // ratatui buffer will clip — but this virtualization layer is
        // mostly about memoization, not buffer clipping.
        let mut y = area.y;
        let bottom = area.y.saturating_add(area.height);
        for item in self.items.iter_mut().skip(self.scroll) {
            let h = item.inner().height_for(area.width, ctx);
            if bottom.saturating_sub(y) == 0 {
                break;
            }
            let item_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: h.min(bottom.saturating_sub(y)),
            };
            item.render_if_changed(item_area, ctx);
            y = y.saturating_add(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{TerminalCaps, Theme};
    use crate::widget::Text;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn render_list(list: &mut List<Text>, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let area = ctx.area();
            list.render(area, &mut ctx);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    fn count_renders(list: &mut List<Text>, width: u16, height: u16) -> usize {
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        let mut count = 0;
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let area = ctx.area();
            for item in &mut list.items {
                if item.render_if_changed(
                    Rect {
                        x: area.x,
                        y: area.y,
                        width: area.width,
                        height: 1,
                    },
                    &mut ctx,
                ) {
                    count += 1;
                }
            }
        })
        .unwrap();
        count
    }

    #[test]
    fn renders_only_visible_items() {
        let mut list: List<Text> = List::new();
        for i in 0..5 {
            list.push(Text::new(format!("item{i}")));
        }
        let buf = render_list(&mut list, 10, 3);
        // Item 0, 1, 2 should be in the buffer; items 3, 4 should not.
        let line0: String = (0..10).map(|x| buf[(x, 0)].symbol()).collect();
        let line2: String = (0..10).map(|x| buf[(x, 2)].symbol()).collect();
        assert!(
            line0.starts_with("item0"),
            "row 0 should start with item0: {line0:?}"
        );
        assert!(
            line2.starts_with("item2"),
            "row 2 should start with item2: {line2:?}"
        );
    }

    #[test]
    fn scroll_changes_hash() {
        let mut list: List<Text> = List::new();
        for i in 0..5 {
            list.push(Text::new(format!("item{i}")));
        }
        let h_top = list.content_hash();
        list.scroll_to(2);
        let h_scrolled = list.content_hash();
        assert_ne!(h_top, h_scrolled);
    }
    #[test]
    fn unchanged_items_skip_render() {
        let mut list: List<Text> = List::new();
        list.push(Text::new("a"));
        list.push(Text::new("b"));
        list.push(Text::new("c"));
        // First render: every item renders.
        let count_first = count_renders(&mut list, 10, 3);
        // None of the items changed, so the second render should skip.
        let count_second = count_renders(&mut list, 10, 3);
        assert_eq!(count_first, 3, "first render should render all 3 items");
        assert_eq!(count_second, 0, "unchanged items must skip second render");
    }

    #[test]
    fn push_changes_hash() {
        let mut list: List<Text> = List::new();
        list.push(Text::new("a"));
        let h1 = list.content_hash();
        list.push(Text::new("b"));
        let h2 = list.content_hash();
        assert_ne!(h1, h2);
    }

    #[test]
    fn scroll_to_saturates_at_len() {
        let mut list: List<Text> = List::new();
        list.push(Text::new("a"));
        list.push(Text::new("b"));
        list.scroll_to(99);
        assert_eq!(list.scroll(), 2);
    }
}
