//! Input widgets — the prompt area where the user types messages.
//!
//! Currently exposes [`InputArea`], a thin `Renderable` wrapper around
//! stock `ratatui-textarea` 0.9. The textarea handles IME, paste, undo,
//! and word-wrap natively; we only translate between `Renderable` and
//! `Widget`.

pub mod textarea;

pub use textarea::InputArea;
