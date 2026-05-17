//! Widgets — ratatui-based UI components.
//!
//! Simplified architecture:
//! - Each widget implements `StatefulWidget` with a separate `FooState`.
//! - All rendering uses simple Paragraph/Line/Block — no manual segment clipping.
//! - Unicode characters are limited to safe, widely-supported glyphs.
//! - ASCII fallbacks are used where needed.

#[allow(missing_docs)]
pub mod chat;
#[allow(missing_docs)]
pub mod footer;
#[allow(missing_docs)]
pub mod input;
#[allow(missing_docs)]
pub mod routing;
#[allow(missing_docs)]
pub mod tool_renderer;
