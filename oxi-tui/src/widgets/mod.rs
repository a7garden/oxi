//! Widgets — ratatui-based UI components.
//!
//! Simplified architecture:
//! - Each widget implements `StatefulWidget` with a separate `FooState`.
//! - All rendering uses simple Paragraph/Line/Block — no manual segment clipping.
//! - Unicode characters are limited to safe, widely-supported glyphs.
//! - ASCII fallbacks are used where needed.

pub mod chat;
pub mod footer;
pub mod input;
pub mod tool_renderer;
