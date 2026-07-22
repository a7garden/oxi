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
    /// Placeholder until theme module lands (Task 11). For now, () — widgets
    /// use hardcoded styles or skip theme-dependent rendering.
    _theme: (),
    /// Placeholder for terminal capabilities. Task 11 adds real `TerminalCaps`.
    _caps: (),
    pub focus: FocusTarget,
    pub time: Instant,
    links: LinkCollector,
    cursor: CursorSlot,
}

impl<'a, 'f> RenderCtx<'a, 'f> {
    pub fn new(frame: &'a mut Frame<'f>) -> Self {
        Self {
            frame,
            _theme: (),
            _caps: (),
            focus: FocusTarget::default(),
            time: Instant::now(),
            links: LinkCollector::new(),
            cursor: CursorSlot::NotSet,
        }
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
    #[expect(dead_code, reason = "used by RetainedTree in Task 9")]
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn make_ctx<'a, 'f>(frame: &'a mut Frame<'f>) -> RenderCtx<'a, 'f> {
        RenderCtx::new(frame)
    }

    #[test]
    fn cursor_starts_notset() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let mut ctx = make_ctx(f);
            let slot = ctx.take_cursor_slot();
            assert_eq!(slot, CursorSlot::NotSet);
        })
        .unwrap();
    }

    #[test]
    fn set_cursor_makes_slot_show() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let mut ctx = make_ctx(f);
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
        term.draw(|f| {
            let mut ctx = make_ctx(f);
            ctx.hide_cursor();
            let slot = ctx.take_cursor_slot();
            assert_eq!(slot, CursorSlot::Hide);
        })
        .unwrap();
    }

    #[test]
    fn take_resets_to_notset() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let mut ctx = make_ctx(f);
            ctx.set_cursor(Position { x: 0, y: 0 });
            let _ = ctx.take_cursor_slot();
            let slot2 = ctx.take_cursor_slot();
            assert_eq!(slot2, CursorSlot::NotSet);
        })
        .unwrap();
    }
}
