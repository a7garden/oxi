//! oxi-agent: Agent runtime for oxi
//!
//! This crate provides an agent runtime that integrates with oxi-ai providers.

pub mod types;
pub mod events;
pub mod tools;
pub mod state;
pub mod config;
pub mod agent;
pub mod compaction;

pub use agent::Agent;
pub use config::AgentConfig;
pub use events::AgentEvent;
pub use state::{AgentState, SharedState};
pub use tools::{ToolRegistry, AgentTool, AgentToolResult, ReadTool, WriteTool, EditTool, BashTool};

pub mod prelude {
    pub use crate::agent::Agent;
    pub use crate::config::AgentConfig;
    pub use crate::events::AgentEvent;
    pub use crate::state::{AgentState, SharedState};
    pub use crate::tools::{ToolRegistry, AgentTool, AgentToolResult, ReadTool, WriteTool, EditTool, BashTool};
    pub use crate::compaction::{CompactedContext, CompactionEvent};
}

#[cfg(test)]
mod tests;
