#![allow(clippy::new_without_default)]

pub mod editor;
pub mod render;
pub mod textarea;
pub mod wrapping;

// Re-exports restored from grok lib.rs in the port task:
//   editor:: {ApplyEditPlanError, EditBuffer, EditCommand, EditDelta, EditOutcome, EditPlan, PostEditCursorAffinity, SingleLineViewport, WordStyle, classify_key_event}
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
