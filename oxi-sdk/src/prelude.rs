//! Commonly used types
//!
//! Re-exports the most frequently used SDK types for convenient glob imports:
//!
//! ```rust
//! use oxi_sdk::prelude::*;
//! ```

pub use crate::agent_builder::AgentBuilder;
pub use crate::agent_definition::{AgentDefinition, AgentDiscovery, AgentScope, DefaultContext};
pub use crate::builder::{Oxi, OxiBuilder};
pub use crate::multi_provider::{MultiProviderBuilder, RoutingConfig};
pub use crate::tool_factory::{browsing_tools, coding_tools, full_tools, readonly_tools};

#[cfg(feature = "native-browser")]
pub use crate::tool_factory::browsing_tools_with_session;

pub use oxi_agent::{
    Agent, AgentConfig, AgentEvent, AgentLoop, AgentState, AgentTool, AgentToolResult,
    CompactionEvent, SearchCache, SharedState, ToolError, ToolExecutionMode, ToolRegistry,
};

pub use oxi_agent::tools::browse::{
    BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine, BrowserError, BrowserTab,
    ElementInfo, LinkInfo, PageContent, TabGuard,
};

pub use oxi_ai::circuit_breaker::CircuitBreakerConfig;
pub use oxi_ai::{CompactionStrategy, Model, Provider, UserMessage};

// ── Catalog (models.dev-backed: SNAP/LIVE/override/LOCAL) ───────
pub use oxi_ai::catalog::{
    AuthMethod, BuiltinModelEntry, BuiltinProviderEntry, OverrideFile, discover_all,
    discover_all_authenticated, discover_all_local, discover_models, load_builtin_providers,
    load_overrides,
};
pub use oxi_ai::model_db::{ModelEntry, builtin_model_count_sentinel};

// ── Concrete provider re-exports (single-dependency pattern) ──────────
pub use oxi_ai::OpenAiProvider;
pub use oxi_ai::OpenAiResponsesProvider;

// ── Foundation Layer ───────────────────────────────────────────────────
pub use crate::error::{SdkError, SdkResult};
pub use crate::lifecycle::{
    AgentHandle, AgentLifecycleEvent, AgentSnapshot, AgentStatus, AgentSupervisor,
    FileSnapshotStore, RestartBackoff, SnapshotStore, SupervisorPolicy, ToolManifest,
};
pub use crate::middleware::Middleware;
pub use crate::middleware::{
    MiddlewareContext, MiddlewareData, MiddlewarePhase, MiddlewarePipeline, MiddlewareResult,
    build_hooks,
};
pub use crate::observability::{
    AuditEntry, AuditFilter, AuditLog, CostBreakdown, CostSnapshot, CostTracker, CostTrackerConfig,
    EventQuery, EventStore, EventStoreConfig, GlobalCostSnapshot, Span, SpanContext, SpanGuard,
    SpanId, SpanKind, SpanStatus, StoredEvent, TokenUsage, TraceId, Tracer,
};

// ── Composition Layer — Security ────────────────────────────────────────
pub use crate::security::{
    Authorizer, Capability, CapabilitySet, CapabilitySubject, SecurityMiddleware,
};

// ── Composition Layer — Coordination ────────────────────────────────────
pub use crate::coordination::{
    Consensus, CoordinatedGroup, MemoryKey, SharedMemory, VoteResult, WorkQueue, WorkResult,
    WorkStatus,
};

// ── Workflow DSL ────────────────────────────────────────────────────────
pub use crate::workflow_dsl::{WorkflowDefinition, WorkflowStepDef};

// ── Runtime routing control ──────────────────────────────────────────────
pub use crate::routing::RoutingControl;
