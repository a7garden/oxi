//! The retained widget tree root. Owns the top-level widget and tracks
//! content hash and resolved cursor state across frames.

use ratatui::layout::Position;

use crate::widget::{RenderCtx, Renderable};

/// Top-level retained widget and its cross-frame state.
pub struct RetainedTree {
    root: Box<dyn Renderable>,
    last_hash: u64,
    last_cursor: Option<Position>,
}

impl RetainedTree {
    #[must_use]
    pub fn new(root: Box<dyn Renderable>) -> Self {
        Self {
            root,
            last_hash: 0,
            last_cursor: None,
        }
    }

    /// Reports whether the root content hash changed since the last call.
    pub fn any_hash_changed(&mut self) -> bool {
        let hash = self.root.content_hash();
        let changed = hash != self.last_hash;
        self.last_hash = hash;
        changed
    }

    /// Renders the tree and resolves its cursor request for this frame.
    pub fn render(&mut self, ctx: &mut RenderCtx) -> Option<Position> {
        let area = ctx.area();
        self.root.render(area, ctx);
        let slot = ctx.take_cursor_slot();
        let cursor = slot.resolve(self.last_cursor);
        self.last_cursor = cursor;
        cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::CursorSlot;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    struct StubWidget {
        hash_value: u64,
        cursor_to_emit: CursorSlot,
        render_count: usize,
    }

    impl Renderable for StubWidget {
        fn content_hash(&self) -> u64 {
            self.hash_value
        }

        fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
            1
        }

        fn render(&mut self, _area: Rect, ctx: &mut RenderCtx) {
            self.render_count += 1;
            match self.cursor_to_emit {
                CursorSlot::Show(position) => ctx.set_cursor(position),
                CursorSlot::Hide => ctx.hide_cursor(),
                CursorSlot::NotSet => {}
            }
        }
    }

    const P1: Position = Position { x: 1, y: 1 };

    fn run_frame(
        tree: &mut RetainedTree,
        terminal: &mut Terminal<TestBackend>,
    ) -> Option<Position> {
        let theme = crate::theme::Theme::dark();
        let caps = crate::theme::TerminalCaps::default();
        let mut cursor_position = None;
        terminal
            .draw(|frame| {
                let mut ctx = RenderCtx::new(frame, &theme, &caps);
                cursor_position = tree.render(&mut ctx);
            })
            .unwrap();
        cursor_position
    }

    #[test]
    fn first_call_emits_cursor_from_show() {
        let mut tree = RetainedTree::new(Box::new(StubWidget {
            hash_value: 100,
            cursor_to_emit: CursorSlot::Show(P1),
            render_count: 0,
        }));
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        assert_eq!(run_frame(&mut tree, &mut terminal), Some(P1));
    }

    #[test]
    fn notset_falls_back_to_last_cursor_across_frames() {
        let mut tree = RetainedTree::new(Box::new(StubWidget {
            hash_value: 100,
            cursor_to_emit: CursorSlot::Show(P1),
            render_count: 0,
        }));
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        assert_eq!(run_frame(&mut tree, &mut terminal), Some(P1));
        assert_eq!(CursorSlot::NotSet.resolve(tree.last_cursor), Some(P1));
    }

    #[test]
    fn hide_overrides_last_cursor_fallback() {
        assert_eq!(CursorSlot::Hide.resolve(Some(P1)), None);
    }

    #[test]
    fn any_hash_changed_first_call_true() {
        let mut tree = RetainedTree::new(Box::new(StubWidget {
            hash_value: 42,
            cursor_to_emit: CursorSlot::NotSet,
            render_count: 0,
        }));
        assert!(tree.any_hash_changed());
    }

    #[test]
    fn any_hash_changed_unchanged_false() {
        let mut tree = RetainedTree::new(Box::new(StubWidget {
            hash_value: 42,
            cursor_to_emit: CursorSlot::NotSet,
            render_count: 0,
        }));
        assert!(tree.any_hash_changed());
        assert!(!tree.any_hash_changed());
    }
}
