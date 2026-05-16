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
pub mod structured_output;
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
/// MCP (Model Context Protocol) integration.
pub mod mcp;

pub use agent::Agent;
pub use agent::ProviderResolver;
pub use agent_loop::{AgentLoop, AgentLoopConfig};

/// Agent configuration, hooks, and tool execution mode.
pub use config::{AgentConfig, AgentHooks, ToolExecutionMode, BeforeToolCallContext, BeforeToolCallResult, AfterToolCallContext, AfterToolCallResult, ShouldStopAfterTurnContext};
pub use error::AgentError;
pub use events::AgentEvent;

/// Fault-recovery primitives for resilient agent execution.
pub use recovery::{
    CircuitBreaker, CircuitBreakerConfig, CircuitOpenError, FallbackChain, PartialResponse,
};
pub use state::{AgentState, SharedState};
pub use compaction::{CompactedContext, CompactionEvent};
pub use oxi_ai::{CompactionManager, CompactionStrategy};
pub use structured_output::{StructuredOutput, OutputMode, StructuredOutputError};

/// Built-in tool implementations and registry.
pub use tools::{
    AgentTool, AgentToolResult, BashTool, EditTool, FindTool, GrepTool,
    LsTool, ReadTool, ToolContext, ToolRegistry, WriteTool, ToolError,
};
pub use tools::search_cache::{SearchCache, GetSearchResultsTool};
pub use tools::web_search::WebSearchTool;
pub use tools::github::GitHubTool;
pub use tools::github_search::GitHubSearchTool;
pub use tools::subagent::SubagentTool;
pub use mcp::{McpTool, McpManager, McpConfig};
pub use tools::context7::{Context7ResolveLibraryIdTool, Context7QueryDocsTool};
pub use tools::questionnaire::{QuestionnaireBridge, QuestionnaireTool};

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
        LsTool, ReadTool, ToolContext, ToolRegistry, WriteTool,
    };
    pub use crate::tools::search_cache::{SearchCache, GetSearchResultsTool};
    pub use crate::tools::web_search::WebSearchTool;
    pub use crate::tools::github::GitHubTool;
    pub use crate::tools::github_search::GitHubSearchTool;
    pub use crate::tools::subagent::SubagentTool;
    pub use crate::mcp::{McpTool, McpManager, McpConfig};
    pub use crate::tools::context7::{Context7ResolveLibraryIdTool, Context7QueryDocsTool};
    pub use crate::tools::questionnaire::{QuestionnaireBridge, QuestionnaireTool};
}

#[cfg(test)]
mod tests;
