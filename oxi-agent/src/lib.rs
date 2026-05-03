//! oxi-agent: Agent runtime for oxi
//!
//! This crate provides an agent runtime that integrates with oxi-ai providers.

pub mod agent;
pub mod compaction;
pub mod config;
pub mod error;
pub mod events;
pub mod recovery;
pub mod state;
pub mod tools;
pub mod types;

pub use agent::Agent;
pub use config::AgentConfig;
pub use error::AgentError;
pub use events::AgentEvent;
pub use recovery::{
    CircuitBreaker, CircuitBreakerConfig, CircuitOpenError, FallbackChain, PartialResponse,
};
pub use state::{AgentState, SharedState};
pub use tools::{
    AgentTool, AgentToolResult, BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool,
    ToolRegistry, WriteTool,
};

pub mod prelude {
    pub use crate::agent::Agent;
    pub use crate::compaction::{CompactedContext, CompactionEvent};
    pub use crate::config::AgentConfig;
    pub use crate::events::AgentEvent;
    pub use crate::state::{AgentState, SharedState};
    pub use crate::tools::{
        AgentTool, AgentToolResult, BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool,
        ToolRegistry, WriteTool,
    };
}

#[cfg(test)]
mod tests;
