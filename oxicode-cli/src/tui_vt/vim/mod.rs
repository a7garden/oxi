//! Vim-style prompt editing engine.
//!
//! App-owned module: relocated from `oxicode-vtui` so the vim key-decision
//! logic lives with the CLI that drives it. The composer's editable text
//! lives in `oxicode_textarea::TextArea`; this engine mutates it via the
//! `Editor` trait (implemented by `InputEditor` in `main_loop.rs`).

mod engine;
mod text;
mod types;

pub use engine::{Editor, HandleKeyOutcome, handle_key};
pub use text::{next_char_boundary, prev_char_boundary};
pub use types::VimState;
