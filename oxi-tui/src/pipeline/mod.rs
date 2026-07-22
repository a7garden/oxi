//! Terminal-first frame lifecycle.
//!
//! Decomposes ratatui's `Terminal::draw()` so the application owns the cursor
//! emission decision. See spec §4.

pub mod cursor;
pub mod cursor_slot;
pub mod diff_backend;

pub use cursor::CursorState;
pub use cursor_slot::CursorSlot;
pub use diff_backend::DiffBackend;

use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::theme::{TerminalCaps, Theme};
use crate::widget::{FocusTarget, RenderCtx, RetainedTree};

/// Draws one terminal-first frame, skipping unchanged content when not resized.
pub fn draw_frame<B: Backend>(
    term: &mut Terminal<B>,
    tree: &mut RetainedTree,
    cursor: &mut CursorState,
    focus: FocusTarget,
    theme: &Theme,
    caps: &TerminalCaps,
) -> Result<FrameOutcome, B::Error> {
    let prev_size = term.size()?;
    term.autoresize()?;
    let resized = term.size()? != prev_size;
    if !tree.any_hash_changed() && !resized {
        return Ok(FrameOutcome::Idle);
    }
    let want = {
        let mut frame = term.get_frame();
        let mut ctx = RenderCtx::new(&mut frame, theme, caps);
        ctx.focus = focus;
        tree.render(&mut ctx)
    };
    term.flush()?;
    cursor.reconcile(want, term)?;
    term.swap_buffers();
    term.backend_mut().flush()?;
    Ok(FrameOutcome::Rendered)
}

/// Outcome of a single `draw_frame` call. Lets the caller sleep until the next
/// tick when nothing changed (idle skip — spec §1.4 proactive optimization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameOutcome {
    /// No work was done: `content_hash` unchanged, no resize, no cursor change.
    /// Caller may sleep until the next event/tick.
    #[default]
    Idle,
    /// A frame was rendered. Cell diff may or may not have emitted bytes
    /// (`DiffBackend` knows, but pipeline doesn't need to).
    Rendered,
}

#[cfg(test)]
mod draw_frame_tests {
    use super::*;
    use crate::widget::{RetainedTree, Text};
    use ratatui::{Terminal, backend::TestBackend};

    fn make_term() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(40, 10)).unwrap()
    }

    #[test]
    fn first_frame_is_rendered() {
        let mut term = make_term();
        let mut tree = RetainedTree::new(Box::new(Text::new("hello")));
        let mut cursor = CursorState::new();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        let outcome = draw_frame(
            &mut term,
            &mut tree,
            &mut cursor,
            crate::widget::FocusTarget::None,
            &theme,
            &caps,
        )
        .unwrap();
        assert_eq!(outcome, FrameOutcome::Rendered);
    }

    #[test]
    fn second_frame_with_unchanged_hash_is_idle() {
        let mut term = make_term();
        let mut tree = RetainedTree::new(Box::new(Text::new("hello")));
        let mut cursor = CursorState::new();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        let _ = draw_frame(
            &mut term,
            &mut tree,
            &mut cursor,
            crate::widget::FocusTarget::None,
            &theme,
            &caps,
        )
        .unwrap();
        let outcome = draw_frame(
            &mut term,
            &mut tree,
            &mut cursor,
            crate::widget::FocusTarget::None,
            &theme,
            &caps,
        )
        .unwrap();
        assert_eq!(outcome, FrameOutcome::Idle);
    }
}
