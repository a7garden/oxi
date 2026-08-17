//! Small, host-agnostic presentation primitives for the next oxicode TUI.
//!
//! This module deliberately has no agent, settings, or terminal-lifecycle
//! dependency. It is the seam between oxicode's event/state layer and its
//! ratatui views.

pub mod renderable;
pub mod transcript;

pub use renderable::{Column, Renderable, TextCell};
pub use transcript::{BlockDisplayMode, TranscriptLine, VisibleItem, visible_items};
