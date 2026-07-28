//! Color primitives for the oxi theme system.
//!
//! Re-exports `ratatui::style::Color` directly to avoid duplicate type definitions.
//! All theme components use this single color type, eliminating `.to_ratatui()`
//! conversion overhead and preventing mismatches between oxi and ratatui colors.

pub use ratatui::style::Color;
