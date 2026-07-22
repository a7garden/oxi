//! Per-child memoization wrapper. Composite widgets (`ChatView`, Footer, etc.)
//! wrap children in `RetainedChild<T>` to get automatic per-subtree skip
//! instead of each reinventing the pattern.
//!
//! ## The problem this solves
//!
//! `RetainedTree` only checks the root hash. During streaming, a token change
//! in `ChatView` trips the root hash → full tree re-render every frame. Without
//! `RetainedChild`, unchanged siblings (Footer, Input) re-render needlessly.
//!
//! ## The fix
//!
//! Composite widgets store children as `RetainedChild<T>` and call
//! `render_if_changed(area, ctx)` instead of `child.render(area, ctx)`. The
//! wrapper tracks `last_hash` and short-circuits when unchanged.

use ratatui::layout::Rect;

use crate::widget::{RenderCtx, Renderable};

#[derive(Debug)]
pub struct RetainedChild<T: Renderable> {
    inner: T,
    last_hash: u64,
    last_height: u16,
}

impl<T: Renderable> RetainedChild<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            last_hash: 0,
            last_height: 0,
        }
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Compute height, caching if hash unchanged.
    pub fn height_for(&mut self, width: u16, ctx: &RenderCtx) -> u16 {
        let h = self.inner.content_hash();
        if h == self.last_hash && self.last_height > 0 {
            return self.last_height;
        }
        let height = self.inner.height_for(width, ctx);
        self.last_hash = h;
        self.last_height = height;
        height
    }

    /// Render only if hash changed since last render. Returns true if rendered.
    pub fn render_if_changed(&mut self, area: Rect, ctx: &mut RenderCtx) -> bool {
        let h = self.inner.content_hash();
        if h == self.last_hash {
            return false;
        }
        self.last_hash = h;
        self.inner.render(area, ctx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::TerminalCaps;
    use crate::theme::Theme;
    use crate::widget::Text;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn first_render_always_renders() {
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let mut child = RetainedChild::new(Text::new("hello"));
            let rendered = child.render_if_changed(ctx.area(), &mut ctx);
            assert!(rendered, "first render must always run");
        })
        .unwrap();
    }

    #[test]
    fn unchanged_hash_skips_render() {
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let mut child = RetainedChild::new(Text::new("hello"));
            let _ = child.render_if_changed(ctx.area(), &mut ctx);
            let rendered2 = child.render_if_changed(ctx.area(), &mut ctx);
            assert!(!rendered2, "second render with same hash must skip");
        })
        .unwrap();
    }

    #[test]
    fn content_change_triggers_rerender() {
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let mut child = RetainedChild::new(Text::new("hello"));
            let _ = child.render_if_changed(ctx.area(), &mut ctx);
            child.inner_mut().set_content("world");
            let rendered2 = child.render_if_changed(ctx.area(), &mut ctx);
            assert!(rendered2, "content change must trigger re-render");
        })
        .unwrap();
    }
}
