//! oxi-agent: Agent runtime layer built on oxi-ai
//!
//! This crate implements agent functionality built on top of oxi-ai,
//! providing tool execution, state management, and event handling.

mod types;
mod events;
mod tools;
mod state;

// Re-exports - be selective to avoid ambiguity
pub use types::{
    AgentConfig, AgentMessage, AgentToolResult, ContentBlock, ToolExecutionMode,
    ImageSource,
};
pub use events::{AgentEvent, AgentEndReason, MessageDelta, ToolUseDelta};
pub use tools::{AgentTool, AgentToolExt, ToolFuture, ToolValidationError};
pub use state::AgentState;

// Re-export prelude for convenience
pub mod prelude {
    pub use crate::types::*;
    pub use crate::events::*;
    pub use crate::tools::*;
    pub use crate::state::*;
}