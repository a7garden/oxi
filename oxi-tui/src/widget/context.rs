//! Per-frame render context passed to every `Renderable::render` call.
//!
//! Widgets read from `ctx` (theme, caps, time, focus) and write to it
//! (cursor slot, link spans). The buffer is accessed via `ctx.buffer_mut()`.
//!
//! ## Cursor slot lifecycle
//!
//! 1. `begin_frame` resets `cursor` to `CursorSlot::NotSet`.
//! 2. During `render()`, widgets call `set_cursor(pos)` or `hide_cursor()`.
//! 3. `RetainedTree::render` calls `take_cursor_slot()` after walking the tree,
//!    resolves via `CursorSlot::resolve(last_cursor)`, and emits to terminal
//!    through `CursorState::reconcile`.

use std::time::Instant;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::pipeline::CursorSlot;
use crate::pipeline::diff_backend::{LinkCollector, LinkTarget};
use crate::theme::{TerminalCaps, Theme};
use crate::widget::CellRange;

/// What has focus this frame. Affects rendering of input cursor, highlights, etc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FocusTarget {
    #[default]
    None,
    Chat,
    Input,
    Overlay,
}

/// Two lifetimes are required: `'a` is the outer `&mut Frame` borrow passed
/// into `RenderCtx::new`, and `'f` is the frame's own buffer lifetime. ratatui
/// 0.30's `Terminal::draw` callback produces a `&mut Frame<'_>` whose buffer
/// outlives the local borrow, so collapsing both into one lifetime does not
/// compile. Downstream code that just uses `&RenderCtx` elides the lifetimes
/// to `RenderCtx<'_, '_>`.
pub struct RenderCtx<'a, 'f> {
    frame: &'a mut Frame<'f>,
    theme: &'a Theme,
    caps: &'a TerminalCaps,
    pub focus: FocusTarget,
    pub time: Instant,
    links: LinkCollector,
    cursor: CursorSlot,
}

impl<'a, 'f> RenderCtx<'a, 'f> {
    pub fn new(frame: &'a mut Frame<'f>, theme: &'a Theme, caps: &'a TerminalCaps) -> Self {
        Self {
            frame,
            theme,
            caps,
            focus: FocusTarget::default(),
            time: Instant::now(),
            links: LinkCollector::new(),
            cursor: CursorSlot::NotSet,
        }
    }

    #[must_use]
    pub fn theme(&self) -> &Theme {
        self.theme
    }

    #[must_use]
    pub fn caps(&self) -> &TerminalCaps {
        self.caps
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.frame.buffer_mut()
    }

    #[must_use]
    pub fn area(&self) -> Rect {
        self.frame.area()
    }

    pub fn set_cursor(&mut self, pos: Position) {
        self.cursor = CursorSlot::Show(pos);
    }

    pub fn hide_cursor(&mut self) {
        self.cursor = CursorSlot::Hide;
    }

    /// Drain the cursor slot, resetting to `NotSet`. Called by `RetainedTree`
    /// after render to inspect what widgets requested.
    pub(crate) fn take_cursor_slot(&mut self) -> CursorSlot {
        std::mem::replace(&mut self.cursor, CursorSlot::NotSet)
    }

    pub fn emit_link(&mut self, range: CellRange, target: LinkTarget) {
        self.links.add(range, target);
    }

    /// Drain collected links. Called by pipeline after render, before flush.
    #[expect(dead_code, reason = "used by pipeline link flush in Task 9")]
    pub(crate) fn take_links(&mut self) -> LinkCollector {
        std::mem::take(&mut self.links)
    }
    /// Access the underlying `Frame` for legacy bridging.
    ///
    /// Used by `LegacyOverlayAdapter` to forward `Renderable::render` calls
    /// to legacy `Overlay` implementations that operate on a raw
    /// `&mut Frame`. NOTE: This is a temporary bridge API. Once all overlays
    /// are migrated to `Renderable`, this accessor should be removed.
    pub fn with_frame<R>(&mut self, f: impl FnOnce(&mut Frame<'f>) -> R) -> R {
        f(self.frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn make_ctx<'a, 'f>(
        frame: &'a mut Frame<'f>,
        theme: &'a Theme,
        caps: &'a TerminalCaps,
    ) -> RenderCtx<'a, 'f> {
        RenderCtx::new(frame, theme, caps)
    }

    #[test]
    fn cursor_starts_notset() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        term.draw(|f| {
            let mut ctx = make_ctx(f, &theme, &caps);
            let slot = ctx.take_cursor_slot();
            assert_eq!(slot, CursorSlot::NotSet);
        })
        .unwrap();
    }

    #[test]
    fn set_cursor_makes_slot_show() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        term.draw(|f| {
            let mut ctx = make_ctx(f, &theme, &caps);
            ctx.set_cursor(Position { x: 3, y: 4 });
            let slot = ctx.take_cursor_slot();
            assert_eq!(slot, CursorSlot::Show(Position { x: 3, y: 4 }));
        })
        .unwrap();
    }

    #[test]
    fn hide_cursor_makes_slot_hide() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        term.draw(|f| {
            let mut ctx = make_ctx(f, &theme, &caps);
            ctx.hide_cursor();
            let slot = ctx.take_cursor_slot();
            assert_eq!(slot, CursorSlot::Hide);
        })
        .unwrap();
    }
    #[test]
    fn with_frame_provides_access_to_frame() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        term.draw(|f| {
            let mut ctx = make_ctx(f, &theme, &caps);
            // Verify the closure receives a `&mut Frame` and can mutate it.
            ctx.with_frame(|frame| {
                let area = frame.area();
                assert_eq!(area.width, 10);
                assert_eq!(area.height, 5);
            });
            // The closure can mutate; surface a sentinel symbol via the buffer
            // and verify it landed in the underlying frame.
            ctx.with_frame(|frame| {
                let cell = &mut frame.buffer_mut()[(0, 0)];
                cell.set_symbol("Z");
            });
            // after closure exits, ctx is still usable: another borrow works.
            assert_eq!(ctx.area().width, 10);
        })
        .unwrap();
    }

    #[test]
    fn take_resets_to_notset() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        term.draw(|f| {
            let mut ctx = make_ctx(f, &theme, &caps);
            ctx.set_cursor(Position { x: 0, y: 0 });
            let _ = ctx.take_cursor_slot();
            let slot2 = ctx.take_cursor_slot();
            assert_eq!(slot2, CursorSlot::NotSet);
        })
        .unwrap();
    }
}
