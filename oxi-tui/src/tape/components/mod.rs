//! Tape component implementations — chat message types as `Component`s.
//!
//! These are building blocks for the tape engine. They are NOT wired into
//! the default render path (which uses ratatui Frame rendering). Future
//! integration will be gated behind `OXI_TAPE_RENDER=1`.

pub mod streaming;
pub mod text;
pub mod tool_call;

pub use streaming::StreamingMessage;
pub use text::TextMessage;
pub use tool_call::ToolCallBlock;
