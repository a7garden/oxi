//! Color primitives for the oxicode theme system.
//!
//! Re-exports `ratatui::style::Color` directly to avoid duplicate type definitions.
//! All theme components use this single color type, eliminating `.to_ratatui()`
//! conversion overhead and preventing mismatches between oxicode and ratatui colors.

pub use ratatui::style::Color;
