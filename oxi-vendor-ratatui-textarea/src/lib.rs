// Vendored from grok-build (Apache-2.0, © 2023-2026 SpaceXAI). See NOTICE-vendored.md.
// Lint relaxations: third-party code copied verbatim. We do NOT chase upstream
// lint churn here; first-party crates keep the workspace `-D warnings` gate.
#![allow(deprecated)]
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_features)]
#![allow(unexpected_cfgs)]
#![allow(clippy::all)]
#![allow(clippy::pedantic)]
#![allow(rustdoc::broken_intra_doc_links)]
#![allow(clippy::new_without_default)]

pub mod editor;
pub mod render;
pub mod textarea;
pub mod wrapping;

pub use editor::{
    ApplyEditPlanError, EditBuffer, EditCommand, EditDelta, EditOutcome, EditPlan,
    PostEditCursorAffinity, SingleLineViewport, WordStyle, classify_key_event,
};
pub use textarea::{
    ClipboardProvider, ElementId, ElementKind, InternalClipboard, MouseAction, TextArea,
    TextAreaState, TextElement, TextElementEvent, TextElementEventKind, is_undo_input,
};

use crossterm::event::KeyModifiers;

// On Windows, AltGr arrives as Ctrl+Alt; on other platforms it's composed before reaching us.
#[cfg(target_os = "windows")]
#[inline]
pub fn is_altgr(modifiers: KeyModifiers) -> bool {
    let without_shift = modifiers & !KeyModifiers::SHIFT;
    without_shift == (KeyModifiers::CONTROL | KeyModifiers::ALT)
}

#[cfg(not(target_os = "windows"))]
#[inline]
pub fn is_altgr(_modifiers: KeyModifiers) -> bool {
    false
}
