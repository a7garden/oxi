#![allow(clippy::new_without_default)]

pub mod editor;
pub mod render;
pub mod textarea;
pub mod wrapping;

// Re-exports restored from grok lib.rs in the port task:
//   editor:: {ApplyEditPlanError, EditBuffer, EditCommand, EditDelta, EditOutcome, EditPlan, PostEditCursorAffinity, SingleLineViewport, WordStyle, classify_key_event}
//   textarea::{ClipboardProvider, ElementId, ElementKind, InternalClipboard, MouseAction, TextArea, TextAreaState, TextElement, TextElementEvent, TextElementEventKind, is_undo_input}
