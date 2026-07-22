//! Bridge module for the new oxi-tui v2 pipeline.
//! Re-exports key types for incremental migration from legacy.
//! Once migration is complete, this module is removed and oxi-cli
//! uses oxi-tui directly.

use ratatui::Frame;
use ratatui::layout::Rect;

/// Temporary adapter that wraps a legacy render closure as a [`Renderable`].
///
/// The content hash changes every frame because legacy rendering is not
/// memoizable. Remove this adapter once the legacy widgets have migrated to
/// native `Renderable` implementations.
pub struct ClosureRoot {
    render_fn: Box<dyn FnMut(&mut Frame<'_>)>,
    frame_counter: u64,
}

impl ClosureRoot {
    /// Creates an adapter around a legacy frame-rendering closure.
    pub fn new<F>(render_fn: F) -> Self
    where
        F: FnMut(&mut Frame<'_>) + 'static,
    {
        Self {
            render_fn: Box::new(render_fn),
            frame_counter: 0,
        }
    }
}

impl oxi_tui::widget::Renderable for ClosureRoot {
    fn content_hash(&self) -> u64 {
        self.frame_counter.wrapping_add(1)
    }

    fn height_for(&self, _width: u16, _ctx: &oxi_tui::widget::RenderCtx<'_, '_>) -> u16 {
        24
    }

    fn render(&mut self, _area: Rect, ctx: &mut oxi_tui::widget::RenderCtx<'_, '_>) {
        self.frame_counter = self.frame_counter.wrapping_add(1);
        ctx.with_frame(|frame| (self.render_fn)(frame));
    }
}
#[allow(unused_imports)] // re-exported for downstream consumers; not used in this crate yet
pub use oxi_tui::content::{ChatLog, ChatMessage, ContentBlock, MessageRole, StreamingState};
#[allow(unused_imports)] // re-exported for downstream consumers; not used in this crate yet
pub use oxi_tui::pipeline::{CursorState, FrameOutcome, draw_frame};
#[allow(unused_imports)] // re-exported for downstream consumers; not used in this crate yet
pub use oxi_tui::theme::{TerminalCaps, Theme as V2Theme};
#[allow(unused_imports)] // re-exported for downstream consumers; not used in this crate yet
pub use oxi_tui::widget::chat::ChatView;
#[allow(unused_imports)] // re-exported for downstream consumers; not used in this crate yet
pub use oxi_tui::widget::panel::Footer;
#[allow(unused_imports)] // re-exported for downstream consumers; not used in this crate yet
pub use oxi_tui::widget::{FocusTarget, RenderCtx, Renderable, RetainedChild, RetainedTree};