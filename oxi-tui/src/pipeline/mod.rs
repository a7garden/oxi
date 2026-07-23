//! Terminal-first frame lifecycle.
//!
//! Decomposes ratatui's `Terminal::draw()` so the application owns the
//! cursor-emission decision. One frame is:
//! `autoresize → hash-skip → render → flush(DiffBackend) → reconcile cursor → swap`.
//!
//! ## Two entry points
//!
//! - [`draw_frame`] — the retained path. Pass a
//!   [`RetainedTree`] whose root hash is
//!   checked each frame; if unchanged (and no resize), rendering and
//!   flushing are skipped entirely. This is the target API.
//! - [`draw_frame_closure`] — the cutover path. Takes a transient render
//!   closure instead of a retained tree, so there is no hash to memoize
//!   (it always renders). Used while widgets are still being migrated to
//!   [`Renderable`][crate::widget::Renderable]; still preserves cursor
//!   dedup, CSI 2026 sync, and DECCARA background fill.
//!
//! Both preserve [`CursorState`] dedup (same cursor position emits zero
//! bytes) and wrap diffed writes in CSI 2026 synchronized output. See spec §4.

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

/// Like `draw_frame` but takes a transient render closure instead of a `RetainedTree`.
///
/// Used for the cutover phase: preserves cursor blink, CSI 2026 sync, and
/// DECCARA background fill but skips hash-memoization (always renders the
/// closure, since the closure has no retained subtree to hash).
///
/// The `render_fn` is `FnOnce(&mut RenderCtx)` and is called synchronously.
/// Callers that need to capture `&mut state` (or `&theme`) can do so safely:
/// the borrow lives only for the duration of this call, with no event
/// handling or other state access in between.
///
/// Once oxi-cli fully migrates to `RetainedTree`, this function is removed.
pub fn draw_frame_closure<B, F>(
    term: &mut Terminal<B>,
    cursor: &mut CursorState,
    focus: FocusTarget,
    theme: &Theme,
    caps: &TerminalCaps,
    render_fn: F,
) -> Result<FrameOutcome, B::Error>
where
    B: Backend,
    F: FnOnce(&mut RenderCtx<'_, '_>),
{
    // 1. Autoresize so `get_frame()` sees the current terminal size.
    term.autoresize()?;

    // 2. Render via the closure — always, no hash skip in cutover mode.
    //    The cursor slot drains inside this block; `None` is passed as the
    //    fallback because cutover mode does not retain a last cursor.
    let want = {
        let mut frame = term.get_frame();
        let mut ctx = RenderCtx::new(&mut frame, theme, caps);
        ctx.focus = focus;
        render_fn(&mut ctx);
        ctx.take_cursor_slot().resolve(None)
    };

    // 3. Flush diffed cells + CSI 2026 sync.
    term.flush()?;

    // 4. Reconcile cursor — emits zero bytes if already in the desired state.
    cursor.reconcile(want, term)?;

    // 5. Swap buffers and write any remaining bytes.
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
    use ratatui::layout::Position;
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

    #[test]
    fn draw_frame_closure_renders_via_fn() {
        let mut term = make_term();
        let mut cursor = CursorState::new();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        let mut rendered_with = None;
        let outcome = draw_frame_closure(
            &mut term,
            &mut cursor,
            crate::widget::FocusTarget::None,
            &theme,
            &caps,
            |ctx| {
                rendered_with = Some(ctx.area().width);
                ctx.set_cursor(Position { x: 0, y: 0 });
            },
        )
        .unwrap();
        assert_eq!(outcome, FrameOutcome::Rendered);
        assert_eq!(rendered_with, Some(40));
    }

    #[test]
    fn draw_frame_closure_keeps_compiling_without_retained_tree() {
        // Second call on the same closure-based flow still renders (no Idle skip).
        let mut term = make_term();
        let mut cursor = CursorState::new();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();
        let outcome = draw_frame_closure(
            &mut term,
            &mut cursor,
            crate::widget::FocusTarget::None,
            &theme,
            &caps,
            |_ctx| {},
        )
        .unwrap();
        assert_eq!(outcome, FrameOutcome::Rendered);
    }
}
