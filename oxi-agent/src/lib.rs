#![allow(inner_doc_comments)]
#![warn(missing_docs)]

/// oxi-agent: Agent runtime for oxi

pub mod agent;
//! Module documentation.
pub mod agent_loop;
//! Module documentation.
pub mod compaction;
//! Module documentation.
pub mod compaction_init;
//! Module documentation.
pub mod config;
//! Module documentation.
pub mod context_builder;
//! Module documentation.
pub mod error;
//! Module documentation.
pub mod events;
//! Module documentation.
pub mod model_id;
//! Module documentation.
pub mod recovery;
//! Module documentation.
pub mod retry_constants;
//! Module documentation.
pub mod state;
//! Module documentation.
pub mod tools;
//! Module documentation.
pub mod types;

pub use agent::Agent;
pub use agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
pub use config::AgentConfig;
pub use error::AgentError;
pub use events::AgentEvent;
pub use recovery::{
    CircuitBreaker, CircuitBreakerConfig, CircuitOpenError, FallbackChain, PartialResponse,
};
pub use state::{AgentState, SharedState};
pub use compaction::{CompactedContext, CompactionEvent};
pub use oxi_ai::{CompactionManager, CompactionStrategy};
pub use tools::{
    AgentTool, AgentToolResult, BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool,
    ToolRegistry, WriteTool,
};

/// Standard imports for oxi-agent usage.
pub mod prelude {
    pub use crate::agent::Agent;
    pub use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
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
