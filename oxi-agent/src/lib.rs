#![allow(unused_doc_comments)]
#![warn(missing_docs)]
// Relax two test-idiom lints under `cfg(test)` so `cargo clippy --all-targets`
// stays clean without weakening the shipped library:
//   - `clippy::unwrap_used` — `unwrap()`/`unwrap_err()` are idiomatic in tests;
//     shipped (non-test) code still `warn`s on it (see the line below).
//   - `clippy::field_reassign_with_default` — the `let mut x = X::default();
//     x.f = ..;` test-setup pattern.
#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::field_reassign_with_default))]
#![allow(unknown_lints)]

//! oxi-agent: Agent runtime for oxi
//!
//! Provides the core agent loop, tool execution, state management,
//! and streaming event pipeline for the oxi coding agent.

/// Advisor subsystem — read-only reviewer that shadows the primary agent.
pub mod advisor;
/// Core agent implementation.
pub mod agent;
/// Agent definition file parsing and discovery.
pub mod agent_definition;
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
/// MCP (Model Context Protocol) integration.
pub mod mcp;
/// Model identifier constants and helpers.
pub mod model_id;
/// Fault-recovery primitives (circuit breaker, fallback chains).
pub mod recovery;
/// Agent state machine and shared mutable state.
pub mod state;
/// Shared streaming retry logic.
pub mod stream_retry;
pub mod structured_output;
/// Built-in tool implementations and registry.
pub mod tools;
/// Shared type aliases and helpers.
pub mod types;

pub use agent::Agent;
pub use agent::ProviderResolver;
pub use agent_definition::{
    AgentDefinition, AgentDiscovery, AgentScope, DefaultContext, current_subagent_depth,
    max_subagent_depth, validate_agent_name,
};
pub use agent_loop::{AgentLoop, AgentLoopConfig};

pub use advisor::{
    ADVISOR_GUIDANCE, ADVISOR_READONLY_TOOL_NAMES, ADVISOR_SYSTEM_PROMPT, AdviseTool, AdvisorAgent,
    AdvisorDeliveryChannel, AdvisorEmissionGuard, AdvisorNote, AdvisorRuntime, AdvisorRuntimeHost,
    AdvisorSeverity, AgentAdvisor, DeliveryOpts, EnqueueAdviceFn, format_advisory_batch,
    is_immune_turn_active, is_interrupting_severity, normalize_advisor_note,
    resolve_delivery_channel,
};
/// Agent configuration, hooks, and tool execution mode.
pub use config::{
    AfterToolCallContext, AfterToolCallResult, AgentConfig, AgentHooks, BeforeToolCallContext,
    BeforeToolCallResult, ShouldStopAfterTurnContext, ToolExecutionMode,
};
pub use error::AgentError;
pub use events::{AgentEvent, ToolCallContext, VisitReason};
pub use tools::browse::{BrowseProgress, BrowseProgressCallback};

pub use agent_loop::config::CompactionHook;
pub use compaction::{CompactedContext, CompactionEvent};
pub use oxi_ai::{CompactionManager, CompactionStrategy};
/// Fault-recovery primitives for resilient agent execution.
pub use recovery::PartialResponse;
pub use state::{AgentState, SharedState};
pub use structured_output::{OutputMode, StructuredOutput, StructuredOutputError};

pub use mcp::{McpConfig, McpManager, McpTool};
pub use tools::ask::{AskBridge, AskTool};
pub use tools::commit::{
    CommitGroup, CommitTool, CommitType, ConventionalAnalysis, ConventionalDetail, NumstatEntry,
    ScopeCandidate,
};
pub use tools::context7::{Context7QueryDocsTool, Context7ResolveLibraryIdTool};
pub use tools::github::GitHubTool;
pub use tools::github_search::GitHubSearchTool;
pub use tools::memory_edit::MemoryEditTool;
pub use tools::memory_recall::MemoryRecallTool;
pub use tools::memory_reflect::MemoryReflectTool;
pub use tools::memory_retain::MemoryRetainTool;
pub use tools::search_cache::{GetSearchResultsTool, SearchCache};
pub use tools::subagent::SubagentTool;
pub use tools::web_search::WebSearchTool;
/// Built-in tool implementations and registry.
pub use tools::{
    AgentTool, AgentToolResult, BashTool, EditTool, FindTool, ForkResult, GrepTool, LsTool,
    ReadTool, SubagentRunner, ToolContext, ToolError, ToolRegistry, WriteTool,
};

pub use tools::TodoStateProvider;
pub use tools::todo::{
    InitListEntry, TodoCompletionTransition, TodoItem, TodoOp, TodoPhase, TodoStatus, TodoTool,
    TodoUpdateResult,
};

pub use tools::{AgentHubStatus, AgentInfo, AgentKind, AgentPoolProvider};
pub use tools::{LspAction, LspProvider};

/// Standard imports for oxi-agent usage.
pub mod prelude {
    pub use crate::agent::Agent;
    pub use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    pub use crate::compaction::{CompactedContext, CompactionEvent};
    pub use crate::config::AgentConfig;
    pub use crate::events::AgentEvent;
    pub use crate::mcp::{McpConfig, McpManager, McpTool};
    pub use crate::state::{AgentState, SharedState};
    pub use crate::tools::ask::{AskBridge, AskTool};
    pub use crate::tools::context7::{Context7QueryDocsTool, Context7ResolveLibraryIdTool};
    pub use crate::tools::github::GitHubTool;
    pub use crate::tools::github_search::GitHubSearchTool;
    pub use crate::tools::search_cache::{GetSearchResultsTool, SearchCache};
    pub use crate::tools::subagent::SubagentTool;
    pub use crate::tools::web_search::WebSearchTool;
    pub use crate::tools::{
        AgentTool, AgentToolResult, BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool,
        ToolContext, ToolRegistry, WriteTool,
    };
}

#[cfg(test)]
mod tests;
