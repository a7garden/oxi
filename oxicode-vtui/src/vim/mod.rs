//! Vim-style prompt editing engine.
//!
//! **DEPRECATED**: vim mode moved to `oxicode-cli` (host-owned). The
//! `oxicode-textarea` crate owns editable text; vim decision logic now
//! lives with the CLI app that drives it.

#![deprecated(
    since = "0.75.0",
    note = "vim mode relocated to oxicode-cli; oxicode-textarea owns text, CLI owns vim logic"
)]

mod engine;
mod text;
mod types;

pub use engine::{Editor, HandleKeyOutcome, handle_key};
pub use text::{next_char_boundary, prev_char_boundary};
pub use types::VimState;
