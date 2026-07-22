//! Bridge module for the new oxi-tui v2 pipeline.
//! Re-exports key types for incremental migration from legacy.
//! Once migration is complete, this module is removed and oxi-cli
//! uses oxi-tui directly.

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