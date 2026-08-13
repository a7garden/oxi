//! `oxicode-textarea` — atomic-mutation text editor widget.
//!
//! Derived from `xai-org/grok-build`'s `xai-ratatui-textarea` crate.
//!
//! Public modules:
//! - [`element`] — atomic text units (Plain, Masked, FileRef, Image)
//! - [`command`] — `EditCommand`, `EditPlan`, `EditResult`
//! - [`selection`] — selection state
//! - [`wrap`] — soft-wrap + display width helpers
//! - [`editor`] — `Editor` state with `EditPlan::apply`
//! - [`editor_keys`] — key → `EditCommand` mapping (normal/insert/vim)
//! - [`textarea`] — `TextArea` widget with cursor / position APIs

pub mod command;
pub mod editor;
pub mod editor_keys;
pub mod element;
pub mod selection;
pub mod textarea;
pub mod wrap;

pub use command::{EditCommand, EditPlan, EditResult};
pub use editor::Editor;
pub use element::{ElementRange, TextElement};
pub use selection::{Affinity, Anchor, Selection};
pub use textarea::{TextArea, TextAreaState};
pub use wrap::display_width_of_range;
