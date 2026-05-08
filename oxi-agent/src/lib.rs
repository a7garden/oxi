#![allow(unused_doc_comments)]
#![warn(missing_docs)]
#![warn(clippy::unwrap_used)]
#![allow(clippy::unwrap_used_in_tests)]

//! oxi-agent: Agent runtime for oxi
//!
//! Provides the core agent loop, tool execution, state management,
//! and streaming event pipeline for the oxi coding agent.

/// Core agent implementation.
pub mod agent;
/// Agent loop – the main request/response cycle driver.
pub mod agent_loop;
/// Context compaction strategies and data types.
pub mod compaction;
/// Agent configuration types.
pub mod config;
/// Error types for agent operations.
pub mod error;
/// Event types emitted during the agent loop.
pub mod events;
/// Model identifier constants and helpers.
pub mod model_id;
/// Fault-recovery primitives (circuit breaker, fallback chains).
pub mod recovery;
/// Agent state machine and shared mutable state.
pub mod state;
/// Built-in tool implementations and registry.
pub mod tools;
/// Shared type aliases and helpers.
pub mod types;
/// Shared streaming retry logic.
pub mod stream_retry;

pub use agent::Agent;
pub use agent_loop::{AgentLoop, AgentLoopConfig};
pub use config::{AgentConfig, AgentHooks, ToolExecutionMode, BeforeToolCallContext, BeforeToolCallResult, AfterToolCallContext, AfterToolCallResult, ShouldStopAfterTurnContext};
pub use error::AgentError;
pub use events::AgentEvent;
pub use recovery::{
    CircuitBreaker, CircuitBreakerConfig, CircuitOpenError, FallbackChain, PartialResponse,
};
pub use state::{AgentState, SharedState};
pub use compaction::{CompactedContext, CompactionEvent};
pub use oxi_ai::{CompactionManager, CompactionStrategy};
pub use tools::{
    AgentTool, AgentToolResult, BashTool, EditTool, FindTool, GrepTool,
    LsTool, ReadTool, ToolRegistry, WriteTool,
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
        AgentTool, AgentToolResult, BashTool, EditTool, FindTool, GrepTool,
        LsTool, ReadTool, ToolRegistry, WriteTool,
    };
}

#[cfg(test)]
mod tests;
