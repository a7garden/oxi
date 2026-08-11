//! Commonly used types
//!
//! Re-exports the most frequently used SDK types for convenient glob imports:
//!
//! ```rust
//! use oxicode_sdk::prelude::*;
//! ```

pub use crate::agent_builder::AgentBuilder;
pub use crate::agent_definition::{AgentDefinition, AgentDiscovery, AgentScope, DefaultContext};
pub use crate::builder::{Oxicode, OxicodeBuilder};
pub use crate::tool_factory::{coding_tools, readonly_tools};

pub use oxicode_agent::{
    Agent, AgentConfig, AgentEvent, AgentLoop, AgentState, AgentTool, AgentToolResult,
    CompactionEvent, SearchCache, SharedState, ToolError, ToolExecutionMode, ToolRegistry,
};

pub use oxicode_ai::{CompactionStrategy, Model, Provider, UserMessage};

// ── Catalog (models.dev-backed: SNAP/LIVE/override/LOCAL) ───────
pub use oxicode_ai::catalog::{
    AuthMethod, BuiltinModelEntry, BuiltinProviderEntry, OverrideFile, discover_all,
    discover_all_authenticated, discover_all_local, discover_models, load_builtin_providers,
    load_overrides,
};
pub use oxicode_ai::model_db::{ModelEntry, builtin_model_count_sentinel};

// ── Concrete provider re-exports (single-dependency pattern) ──────────
pub use oxicode_ai::OpenAiProvider;
pub use oxicode_ai::OpenAiResponsesProvider;

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

// ── Port→agent capability adapters + traits ────────────────────────────
/// Bridge `MemoryStore`+`EmbeddingProvider` ports into the `memory_*` tools.
pub use crate::port_memory_backend::PortMemoryBackend;
pub use oxicode_agent::tools::{MemoryBackend, MemoryItem, ResolvedContent, UrlResolver};
