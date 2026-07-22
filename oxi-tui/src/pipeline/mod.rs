//! Terminal-first frame lifecycle.
//!
//! Decomposes ratatui's `Terminal::draw()` so the application owns the cursor
//! emission decision. See spec §4.

pub mod cursor;
pub mod cursor_slot;

pub use cursor::CursorState;
pub use cursor_slot::CursorSlot;
