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
