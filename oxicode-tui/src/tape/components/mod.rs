//! Tape component implementations — chat message types as `Component`s.
//!
//! These are the tape engine's production transcript building blocks; oxicode-cli
//! composes them into main-screen rows while ratatui remains overlay-only.

pub mod streaming;
pub mod text;
pub mod tool_call;

pub use streaming::StreamingMessage;
pub use text::TextMessage;
pub use tool_call::ToolCallBlock;
