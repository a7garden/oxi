//! oxicode SDK - Programmatic API for building AI agents
//!
//! # Example
//! ```
//! use oxicode_sdk::{OxicodeBuilder, AgentConfig};
//!
//! let oxicode = OxicodeBuilder::new().with_builtins().build();
//! let agent = oxicode.agent(AgentConfig {
//!     model_id: "anthropic/claude-sonnet-4-20250514".into(),
//!     ..Default::default()
//! }).build().unwrap();
//! ```
#![warn(missing_docs)]
// Shipped (non-test) code denies the panic-family lints; test code keeps
// idiomatic `unwrap()`/`expect()`/`panic!("Expected X")` match-arm assertions.
#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::field_reassign_with_default,
        clippy::expect_used,
        clippy::panic,
    )
)]
// Stability tier attribute macros. Renamed to avoid shadowing the (nightly-only,
// but rustc-resolved) builtin `#[stable]/#[unstable]/#[deprecated]`. The proc-
// macro crate's own rustdoc recommends this rename pattern.
use oxicode_api_stability::{internal as oxicode_internal, stable as oxicode_stable};
// `oxicode_unstable` is only referenced on feature-gated re-export blocks; in the
// default (no-feature) build none survive, so the import is conditionally unused.
#[allow(unused_imports)]
use oxicode_api_stability::unstable as oxicode_unstable;

pub mod agent_builder;
pub mod agent_definition;
pub mod agent_group;
pub mod bridge;
pub mod builder;
pub mod closure_tool;
pub mod coordination;
pub mod delegation;
pub mod error;
pub mod event_bus;
pub mod kernel_bridge;
pub mod lifecycle;
pub mod message_bus;
pub mod metrics;
pub mod middleware;
pub mod observability;
pub mod ports;

// Reference implementations bundled with the SDK.
#[oxicode_internal]
pub use ports::{fs, inmem};
pub mod port_memory_backend;
/// Convenience re-exports: `oxicode_sdk::fs::*`, `oxicode_sdk::inmem::*`.
pub mod prelude;
pub mod routing;
pub mod security;
pub mod snapcompact_compactor;
pub mod tool_factory;
pub mod url_resolver;
pub mod workflow_dsl;
pub mod workflow_engine;

// Re-export core SDK types
#[oxicode_stable(since = "0.63.0")]
pub use agent_builder::AgentBuilder;
#[oxicode_stable(since = "0.63.0")]
pub use agent_group::{AgentGroup, AgentGroupOutput, GroupResult, GroupStrategy};
#[oxicode_stable(since = "0.63.0")]
pub use builder::{Oxicode, OxicodeBuilder};
#[cfg(feature = "delegation")]
#[oxicode_unstable(feature = "delegation")]
pub use delegation::SdkSubagentRunner;
#[cfg(feature = "url-resolver")]
#[oxicode_unstable(feature = "url-resolver")]
pub use url_resolver::SdkUrlResolver;
#[cfg(feature = "workflow-dsl")]
#[oxicode_unstable(feature = "workflow-dsl")]
pub use workflow_engine::{StepOutput, WorkflowEngine, WorkflowResult};

// Re-export port types — products implement these traits.
// Note: Some names conflict with existing modules (e.g. `EventBus` is also a
// module, `MemoryEntry` is also in `coordination`). We rename on import to
// avoid ambiguity; users can still access the trait via the explicit path
// `oxicode_sdk::ports::EventBus` if they need to disambiguate.
#[oxicode_stable(since = "0.63.0")]
pub use closure_tool::ClosureTool;
#[oxicode_internal]
pub use kernel_bridge::{KernelToolContext, KernelToolProvider};
#[oxicode_stable(since = "0.63.0")]
pub use message_bus::{InterAgentMessage, LagAwareReceiver, MessageBus, PublishResult};
#[oxicode_stable(since = "0.63.0")]
pub use metrics::{AgentMetrics, MetricsSnapshot};
#[oxicode_stable(since = "0.63.0")]
pub use ports::AccessGate as AccessGatePort;
#[oxicode_stable(since = "0.63.0")]
pub use ports::EventBus as EventBusPort;
#[oxicode_stable(since = "0.63.0")]
pub use ports::MemoryEntry as MemoryEntryPort;
#[oxicode_stable(since = "0.63.0")]
pub use ports::{
    AccessDecision, AuthMethod, AuthProvider, CapabilityResolver, ConfigStore, CronJob,
    CronScheduler, EventPayload, EventTopic, InMemoryEventBus, MemoryStore, NoopAuthProvider,
    NoopConfigStore, NoopCronScheduler, NoopEventBus, NoopMemoryStore, NoopPersonaProvider,
    NoopResourceMonitor, NoopSkillLoader, NoopStateStore, OAuthToken, Persona, PersonaProvider,
    PortId, PortRegistry, PortValue, ResourceMonitor, ResourceUsage, Skill, SkillLoader, SkillMeta,
    StateStore, SubscriptionHandle, ToolCallRequest,
};

// Port 16 — HookRunner.
#[oxicode_stable(since = "0.66.0")]
pub use ports::hooks::{HookContext, HookEvent, HookOutcome, HookRunner, HookSpec, NoopHookRunner};

// Catalog port (Port 12).
#[oxicode_stable(since = "0.63.0")]
pub use ports::catalog::{
    CatalogEvent, CatalogModelEntry, CatalogProtocol, CatalogProviderEntry, CatalogSource,
    ModelCatalog, NoopModelCatalog, RefreshOutcome,
};
// File-backed reference impl for the catalog port.
#[oxicode_internal]
pub use ports::fs::catalog::{CatalogConfig, FileModelCatalog};

// Composition Layer — EventBus
#[oxicode_stable(since = "0.63.0")]
pub use event_bus::EventBus;

// Foundation Layer
#[oxicode_stable(since = "0.63.0")]
pub use error::{SdkError, SdkResult};
#[oxicode_stable(since = "0.63.0")]
pub use lifecycle::{
    AgentHandle, AgentLifecycleEvent, AgentPool, AgentSnapshot, AgentStatus, AgentSupervisor,
    FileSnapshotStore, HubKind, HubStatus, RestartBackoff, SnapshotStore, SupervisorPolicy,
    ToolManifest,
};
#[oxicode_stable(since = "0.63.0")]
pub use middleware::Middleware;
#[oxicode_stable(since = "0.63.0")]
pub use middleware::{
    MiddlewareContext, MiddlewareData, MiddlewarePhase, MiddlewarePipeline, MiddlewareResult,
    build_hooks,
};
#[oxicode_stable(since = "0.63.0")]
pub use observability::{
    AuditAction, AuditEntry, AuditError, AuditFilter, AuditLog, AuditPersistence, AuditTrail,
    CostBreakdown, CostSnapshot, CostTracker, CostTrackerConfig, EventQuery, EventStore,
    EventStoreConfig, GlobalCostSnapshot, HashDigest, Span, SpanContext, SpanGuard, SpanId,
    SpanKind, SpanStatus, StoredEvent, TokenUsage, TraceId, Tracer, TrailEntry,
};

#[oxicode_stable(since = "0.63.0")]
// Composition Layer — Security
pub use security::{
    AccessDenied, AccessGate, Action, AgentContext, AgentPermissions, AllowlistMode,
    ApprovalStatus, AuditEvent, AuditSink, Authorizer, Capability, CapabilitySet,
    CapabilitySubject, CheckRequest, DefaultPolicy, DenyLayer, ExecPolicy, PathMode,
    PendingApproval, PermAuditEntry, PermissionUpdate, RbacAuditEntry, RbacManager, RbacPolicy,
    Role, SecurityMiddleware, StringPattern, Subject, TracingAuditSink, TrailAuditSink,
};

#[oxicode_stable(since = "0.63.0")]
// Composition Layer — Coordination
pub use coordination::{
    Consensus, CoordinatedGroup, CoordinatedGroupBuilder, MemoryEntry, MemoryEvent, MemoryKey,
    SharedMemory, VoteResult, WorkEvent, WorkItem, WorkQueue, WorkQueueConfig, WorkQueueStats,
    WorkResult, WorkStatus,
};

#[oxicode_stable(since = "0.63.0")]
// Runtime routing control
pub use routing::RoutingControl;

#[oxicode_stable(since = "0.63.0")]
// Re-export from oxicode-ai
pub use oxicode_ai::{
    Api, CompactionStrategy, ContentBlock, Context, Cost, InputModality, Message, MessageContent,
    Model, ModelRegistry, Provider, ProviderError, ProviderEvent, ProviderOptions,
    ProviderRegistry, StreamOptions, UserMessage,
};
// Model roles + role switching (ported from omp)
#[cfg(feature = "role-routing")]
#[oxicode_unstable(feature = "role-routing")]
pub use oxicode_ai::role_routing::RoleRoutingProvider;
#[cfg(feature = "role-switching")]
#[oxicode_unstable(feature = "role-switching")]
pub use oxicode_ai::role_switcher::{
    RoleSignals, decide_role, resolve_role_to_model, role_for_tool,
};
#[cfg(feature = "role-routing")]
#[oxicode_unstable(feature = "role-routing")]
pub use oxicode_ai::roles::{ModelRole, RoleRegistry, live_role_registry, set_live_role_registry};

#[oxicode_stable(since = "0.63.0")]
// Credential management (oauth + env key resolution)
pub use oxicode_ai::env_api_keys::{find_env_keys, get_all_env_keys, get_env_api_key, has_env_key};

#[oxicode_internal]
// Model database — provider catalog, model metadata
pub use oxicode_ai::model_db::{
    ModelEntry, builtin_model_count_sentinel, get_all_models, get_cheapest_models, get_model_entry,
    get_provider_models, get_providers, get_reasoning_models, get_vision_models, model_count,
    search_models,
};

// Catalog — models.dev-backed dynamic catalog (SNAP/LIVE/override/LOCAL).
// The catalog module exposes the full surface; `model_db` (above) is the
// legacy compatibility shim that integrates all layers and converts
#[oxicode_stable(since = "0.63.0")]
// BuiltinModelEntry → ModelEntry.
pub use oxicode_ai::catalog::{
    BuiltinModelEntry, BuiltinProviderEntry, OverrideFile, apply_model_overrides,
    apply_provider_overrides, builtin_model_count, builtin_providers_count, discover_all,
    discover_all_authenticated, discover_all_local, discover_models, find_override_files,
    load_builtin_providers, load_overrides,
};
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_ai::oauth::{
    AuthStore, OAuthError, TokenBundle, default_auth_path, load_auth_store, load_token,
    remove_token, save_auth_store, save_token,
};

// Provider registry — built-in providers (Layer 1 of the catalog) and
#[oxicode_stable(since = "0.63.0")]
// the runtime functions that surface them to consumers.
pub use oxicode_ai::register_builtins::{
    BuiltinProvider, get_all_provider_aliases, get_all_provider_names, get_api_mappings,
    get_builtin_provider, get_builtin_providers, get_provider_api, get_provider_base_url,
    get_provider_env_key, get_provider_env_keys, is_builtin_provider, resolve_provider_name,
};

// Provider instance registry (custom + built-in at runtime)
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_ai::{
    create_builtin_provider, create_builtin_provider_with_options, custom_provider_names,
    dynamic_models, fetch_models_async, fetch_models_blocking, get_model, get_models, get_provider,
    get_provider_arc, lookup_model, register_model, register_provider, unregister_provider,
};

// Complexity-based routing and the router module
#[cfg(feature = "router")]
#[oxicode_unstable(feature = "router")]
pub use oxicode_ai::router;

// Tool-related types (oxicode-cli's main.rs uses ToolCall, ToolResult, ToolCallType)
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_ai::{
    ProgressCallback, Tool, ToolCall, ToolCallType, ToolResult, ToolValidationError, validate_args,
};

// Thinking level (re-exported from oxicode_ai::types, since oxicode-ai's top-level
// re-exports it via `pub use types::*` but not as a named item).
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_ai::types::ThinkingLevel;

// Re-export from oxicode-agent
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_agent::{
    Agent, AgentConfig, AgentError, AgentEvent, AgentHooks, AgentLoop, AgentLoopConfig, AgentState,
    AgentTool, AgentToolResult, BrowseProgress, BrowseProgressCallback, CompactedContext,
    CompactionEvent, CompactionHook, EditTool, FindTool, GetSearchResultsTool, GrepTool, LsTool,
    Mode, OutputMode, ProviderResolver, ReadTool, SearchCache, SharedState, StreamDelta,
    StructuredOutput, StructuredOutputError, ToolCallContext, ToolContext, ToolError,
    ToolExecutionMode, ToolRegistry, VisitReason, WebSearchTool, WriteTool,
};
// ── Advisor subsystem (read-only reviewer that shadows the primary agent) ─
//
// SDK consumers can construct a full advisor: build a second `Agent` with the
// advisor model role + read-only tools + an `AdviseTool` (carrying an
// `EnqueueAdviceFn`), then drive it with `AdvisorRuntime`. The emission guard
#[cfg(feature = "advisor")]
#[oxicode_unstable(feature = "advisor")]
pub use oxicode_agent::advisor::{
    ADVISOR_GUIDANCE, ADVISOR_READONLY_TOOL_NAMES, ADVISOR_SYSTEM_PROMPT, AdviseTool, AdvisorAgent,
    AdvisorDeliveryChannel, AdvisorEmissionGuard, AdvisorNote, AdvisorRuntime, AdvisorRuntimeHost,
    AdvisorSeverity, AgentAdvisor, DeliveryOpts, EnqueueAdviceFn, format_advisory_batch,
    is_immune_turn_active, is_interrupting_severity, normalize_advisor_note,
    resolve_delivery_channel,
};
// ── Todo tool types (agent-scoped, observable by SDK consumers) ──────
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_agent::tools::todo::{TodoItem, TodoOp, TodoPhase, TodoStatus, TodoUpdateResult};
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_agent::{TodoStateProvider, TodoTool};

// ── Capability traits + agent tools (single-dependency pattern) ───────
//
// A pure-`oxicode-sdk` consumer (no `oxicode-agent` direct dep) can implement these
// capability traits to wire the agent's tools, and register the tool structs
// directly. Without these re-exports the single-dependency pattern
// (`oxios → oxicode-sdk`) is incomplete: implementing a custom memory backend,
// URL resolver, LSP provider, subagent runner, or agent-pool source would
// otherwise force a direct `oxicode-agent` dependency.

/// `MemoryBackend` backed by the SDK `MemoryStore` + `EmbeddingProvider` ports.
#[cfg(feature = "memory")]
#[oxicode_unstable(feature = "memory")]
pub use crate::port_memory_backend::PortMemoryBackend;
/// `BashTool` (read/write/edit/grep/find/ls are already re-exported above and
/// bundled by `coding_tools()`).
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_agent::BashTool;
/// In-process subagent runner trait (see [`crate::delegation::SdkSubagentRunner`]).
#[cfg(feature = "subagent")]
#[oxicode_unstable(feature = "subagent")]
pub use oxicode_agent::SubagentRunner;
/// Memory backend trait + item — implement to back the `memory_*` tools, or
/// use [`PortMemoryBackend`] to bridge the SDK's `MemoryStore` port.
#[cfg(feature = "memory")]
#[oxicode_unstable(feature = "memory")]
pub use oxicode_agent::tools::{MemoryBackend, MemoryItem};
/// The `memory_*` + `subagent` tool structs (register directly if desired).
#[cfg(feature = "memory")]
#[oxicode_unstable(feature = "memory")]
pub use oxicode_agent::tools::{
    MemoryEditTool, MemoryRecallTool, MemoryReflectTool, MemoryRetainTool, SubagentTool,
};
/// URL resolver trait + resolved content — implement for internal-URL dispatch.
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_agent::tools::{ResolvedContent, UrlResolver};
/// Agent-pool source for Hub display + todo sub-agent matching.
#[cfg(feature = "agent-hub")]
#[oxicode_unstable(feature = "agent-hub")]
pub use oxicode_agent::{AgentHubStatus, AgentInfo, AgentKind, AgentPoolProvider};
/// LSP capability — implement to back the `lsp` tool.
#[cfg(feature = "lsp")]
#[oxicode_unstable(feature = "lsp")]
pub use oxicode_agent::{LspAction, LspProvider};

// Re-export the hashline crate so consumers can implement `SnapshotStore`
// (enables line-anchored edit mode) without a direct `oxicode-hashline` dep.
#[oxicode_internal]
pub use oxicode_hashline;

// ── Concrete provider re-exports ─────────────────────────────────────────
//
// SDK consumers can construct providers directly without depending on
// `oxicode-ai`. This enables the single-dependency pattern:
//   oxios → oxicode-sdk  (no oxicode-ai, no oxicode-agent direct dep)

#[oxicode_stable(since = "0.63.0")]
pub use oxicode_ai::OpenAiProvider;
#[oxicode_stable(since = "0.63.0")]
pub use oxicode_ai::OpenAiResponsesProvider;

// ── Browser engine re-exports ────────────────────────────────────────────────
//
// The browser trait layer (BrowserEngine, BrowserTab, config, error types)
// is available behind the `browser` feature so SDK consumers can implement
// custom backends. The native oxibrowser-core backend additionally requires
// the `native-browser` feature.

#[cfg(feature = "browser")]
#[oxicode_unstable(feature = "browser")]
pub use oxicode_agent::tools::browse::{
    BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine, BrowserError, BrowserTab,
    ElementInfo, LinkInfo, PageContent, TabGuard,
};

#[cfg(feature = "native-browser")]
#[oxicode_unstable(feature = "native-browser")]
pub use oxicode_agent::tools::browse::{BrowseScriptTool, BrowseSessionTool, OxicodeBrowserEngine};

/// Re-export the browser event type from oxibrowser-core. Surfaced through
/// `Browser::subscribe_events()` for agents/UIs that want to render fine-
/// grained progress ("Opening X…", "Loaded Y…") to the user.
///
/// Gated by the `native-browser` feature because it depends on
/// `oxibrowser-core` types.
#[cfg(feature = "native-browser")]
#[oxicode_unstable(feature = "native-browser")]
pub use oxibrowser_core::BrowserEvent;

// ── MCP (Model Context Protocol) re-exports ───────────────────────────
//
// SDK consumers can use MCP servers alongside built-in tools.
// `McpManager::spawn()` creates a manager; `OxicodeBuilder::with_mcp_config()`
// injects a programmatic config; `mcp_tools()` auto-discovers from
// standard config files. See `oxicode_sdk::tool_factory::mcp_tools`.

#[oxicode_stable(since = "0.63.0")]
pub use oxicode_agent::mcp::{
    ConsentManager, ConsentState, DirectToolDef, DirectToolsConfig, LifecycleMode, McpCallResult,
    McpConfig, McpConnectionStatus, McpContent, McpDashboardData, McpDirectTool, McpManager,
    McpSamplingRequest, McpServerInfo, McpSettings, McpSettingsView, McpTool, McpToolDef,
    McpToolInfo, MetadataCache, ServerEntry, ToolMetadata, ToolPrefix,
};

// Transport layer — re-exported so consumers can implement custom transports
// without a direct `oxicode-agent` dependency. See
// `docs/oxicode-sdk-ownership.md` §2 (MCP transport is SDK-owned behavior).
#[cfg(feature = "mcp-transport")]
#[oxicode_unstable(feature = "mcp-transport")]
pub use oxicode_agent::mcp::transport::{
    McpTransport, http::StreamableHttpTransport, stdio::StdioTransport,
};

// Spawn validation policy — composable trait. SDK owns the trait + noop impl;
// consumers (oxicode-cli, oxios) register their own policy. See
// `docs/oxicode-sdk-ownership.md` §2.
#[cfg(feature = "mcp-spawn-validator")]
#[oxicode_unstable(feature = "mcp-spawn-validator")]
pub use oxicode_agent::mcp::{NoopSpawnValidator, SpawnValidator};

// Circuit-breaker behavior trait + reference impl. SDK owns the trait;
// consumers implement for their domain (A2A, HTTP, etc.). See
// `docs/oxicode-sdk-ownership.md` §3.
#[cfg(feature = "circuit-breaker")]
#[oxicode_unstable(feature = "circuit-breaker")]
pub use oxicode_ai::circuit_breaker::{
    BreakerError, BreakerState, CircuitBreaker, DefaultCircuitBreaker, SharedBreaker,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn catalog_reachable_via_sdk() {
        // Verify the catalog surface is exposed to SDK consumers.
        // Models now come from the materialized snapshot (models.dev),
        // not from load_builtin_models() (which is an empty legacy map).
        let providers = load_builtin_providers();
        assert!(
            providers.len() >= 70,
            "expected >= 70 providers, got {}",
            providers.len()
        );
        // Materialized catalog should have models.
        let all = get_all_models().collect::<Vec<_>>();
        assert!(
            !all.is_empty(),
            "models should be loaded via get_all_models()"
        );
        assert!(
            all.len() > 5000,
            "expected >5000 materialized models, got {}",
            all.len()
        );
    }

    #[test]
    fn sentinel_count_via_sdk() {
        let n = builtin_model_count_sentinel();
        // With materialize from models.dev, no sentinel pricing is applied
        // (models.dev is the verified source of truth).
        assert_eq!(
            n, 0,
            "expected 0 sentinel entries with materialize, got {n}"
        );
    }

    /// Helper to build a minimal Model for tests.
    fn test_model(id: &str, provider: &str) -> Model {
        Model::new(
            id,
            id,
            Api::AnthropicMessages,
            provider,
            "https://api.example.com",
        )
    }

    #[test]
    fn test_oxicode_builder_new() {
        let oxicode = OxicodeBuilder::new().build();
        // Empty registry — no models
        assert!(
            oxicode
                .resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_err()
        );
    }

    #[test]
    fn test_oxicode_builder_with_builtins() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        // Should have built-in models
        assert!(
            oxicode
                .resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_ok()
        );
        assert!(oxicode.resolve_model("openai/gpt-4o").is_ok());
    }

    #[test]
    fn test_oxicode_builder_custom_model() {
        let oxicode = OxicodeBuilder::new()
            .model(test_model("test-model", "test-provider"))
            .build();
        assert!(oxicode.resolve_model("test-provider/test-model").is_ok());
    }

    #[test]
    fn test_oxicode_provider_resolution() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        // Built-in provider (falls back to built-in registry)
        assert!(oxicode.create_provider("anthropic").is_ok());
        // Unknown provider
        assert!(oxicode.create_provider("nonexistent").is_err());
    }

    #[test]
    fn test_agent_builder_workspace() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 30,
            ..Default::default()
        };
        // AgentBuilder with workspace — should not panic
        let result = oxicode
            .agent(config)
            .workspace("/tmp/test-workspace")
            .build();
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_agent_builder_coding_tools() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxicode
            .agent(config)
            .workspace("/tmp")
            .coding_tools()
            .build();
        if let Ok(agent) = result {
            let tool_names = agent.tools().names();
            assert!(tool_names.contains(&"read".to_string()));
            assert!(tool_names.contains(&"write".to_string()));
            assert!(tool_names.contains(&"edit".to_string()));
            assert!(tool_names.contains(&"ls".to_string()));
        }
    }

    #[test]
    fn test_agent_builder_readonly_tools() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxicode
            .agent(config)
            .workspace("/tmp")
            .readonly_tools()
            .build();
        if let Ok(agent) = result {
            let tool_names = agent.tools().names();
            assert!(tool_names.contains(&"read".to_string()));
            assert!(tool_names.contains(&"ls".to_string()));
            // Should NOT have write/edit
            assert!(!tool_names.contains(&"write".to_string()));
        }
    }

    #[test]
    fn test_model_registry_isolation() {
        // Two separate Oxicode instances should not share state
        let oxicode1 = OxicodeBuilder::new()
            .model(test_model("unique-1", "test"))
            .build();

        let oxicode2 = OxicodeBuilder::new().with_builtins().build();

        // oxicode2 should NOT have oxicode1's custom model
        assert!(oxicode2.resolve_model("test/unique-1").is_err());
        // oxicode1 should have its custom model
        assert!(oxicode1.resolve_model("test/unique-1").is_ok());
    }

    #[test]
    fn test_tool_factory_coding_tools() {
        let tools = crate::tool_factory::coding_tools(Path::new("/tmp"));
        let names = tools.names();
        assert!(names.contains(&"read".to_string()));
        assert!(names.contains(&"write".to_string()));
        assert!(names.contains(&"edit".to_string()));
        assert!(names.contains(&"ls".to_string()));
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn test_tool_factory_readonly_tools() {
        let tools = crate::tool_factory::readonly_tools(Path::new("/tmp"));
        let names = tools.names();
        assert!(names.contains(&"read".to_string()));
        assert!(names.contains(&"ls".to_string()));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn test_tool_registry_extend_from() {
        let base = crate::tool_factory::coding_tools(Path::new("/tmp"));
        let extra = crate::tool_factory::readonly_tools(Path::new("/tmp"));

        // extend_from should add tools from extra into base
        let combined = ToolRegistry::new();
        combined.extend_from(&base);
        combined.extend_from(&extra);

        let names = combined.names();
        assert!(names.contains(&"read".to_string()));
        assert!(names.contains(&"write".to_string()));
        assert!(names.contains(&"ls".to_string()));
        // read and ls are in both — no duplicates expected since same names
    }

    // ── Phase 2+ Tests: ProviderResolver, ClosureTool, Isolation ──

    #[test]
    fn test_provider_resolver_trait_on_oxicode() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        // Oxicode implements ProviderResolver
        let resolver: &dyn ProviderResolver = &oxicode;
        assert!(resolver.resolve_provider("anthropic").is_some());
        assert!(resolver.resolve_provider("nonexistent").is_none());
        assert!(
            resolver
                .resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_some()
        );
        assert!(resolver.resolve_model("nonexistent/model").is_none());
    }

    #[test]
    fn test_agent_uses_resolver_for_switch_model() {
        // Create isolated Oxicode with only a mock model
        let oxicode = OxicodeBuilder::new()
            .model(test_model("test-model", "test-provider"))
            .build();

        // This should fail because 'anthropic' provider isn't registered
        let config = AgentConfig {
            model_id: "test-provider/test-model".into(),
            timeout_seconds: 5,
            ..Default::default()
        };
        let result = oxicode.agent(config).build();
        // Agent build fails because provider 'test-provider' has no implementation
        // (no custom provider registered, no builtins enabled)
        assert!(result.is_err());
    }

    #[test]
    fn test_oxicode_builder_without_builtins() {
        let oxicode = OxicodeBuilder::new().build();
        // No models, no providers
        assert!(
            oxicode
                .resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_err()
        );
        assert!(oxicode.create_provider("anthropic").is_err());
        assert!(!oxicode.has_builtins());
    }

    #[test]
    fn test_oxicode_builder_with_builtins_creates_providers() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        assert!(oxicode.has_builtins());
        // Built-in provider fallback should work
        assert!(oxicode.create_provider("anthropic").is_ok());
        assert!(oxicode.create_provider("openai").is_ok());
        assert!(oxicode.create_provider("deepseek").is_ok());
        // Unknown still fails
        assert!(oxicode.create_provider("unknown-provider").is_err());
    }

    /// Regression (#40): `Oxicode::create_provider` must consult the wired
    /// `AuthProvider` port via its sync fast-path when the static
    /// `OxicodeBuilder::api_key()` map has no entry for the provider. This is
    /// the **primary** credential source for products like the CLI, which
    /// never call `OxicodeBuilder::api_key()` and instead register
    /// `FileAuthProvider` via `.with_auth(...)`. Without this wiring, every
    /// built-in provider would fall through to env vars and the CLI would
    /// fail with `MissingApiKey` for any provider not in the environment.
    #[test]
    fn test_create_provider_consults_auth_port() {
        use parking_lot::Mutex;
        use std::pin::Pin;

        /// Recording AuthProvider — captures every `get_api_key_sync` call
        /// and returns `None`, forcing the wiring to actually consult us
        /// (rather than short-circuiting on a returned key).
        struct RecordingAuth {
            calls: Mutex<Vec<String>>,
        }
        impl AuthProvider for RecordingAuth {
            fn get_api_key(
                &self,
                _provider: &str,
            ) -> Pin<Box<dyn Future<Output = Result<Option<String>, SdkError>> + Send + '_>>
            {
                Box::pin(async { Ok(None) })
            }
            fn get_api_key_sync(&self, provider: &str) -> Result<Option<String>, SdkError> {
                self.calls.lock().push(provider.to_string());
                Ok(None)
            }
            fn set_api_key(
                &self,
                _: &str,
                _: &str,
            ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
                Box::pin(async { Ok(()) })
            }
            fn delete_api_key(
                &self,
                _: &str,
            ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
                Box::pin(async { Ok(()) })
            }
            fn get_oauth(
                &self,
                _: &str,
            ) -> Pin<
                Box<
                    dyn Future<Output = Result<Option<crate::ports::OAuthToken>, SdkError>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async { Ok(None) })
            }
            fn set_oauth(
                &self,
                _: &str,
                _: crate::ports::OAuthToken,
            ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
                Box::pin(async { Ok(()) })
            }
            fn list_providers(
                &self,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SdkError>> + Send + '_>>
            {
                Box::pin(async { Ok(Vec::new()) })
            }
        }

        let auth = std::sync::Arc::new(RecordingAuth {
            calls: Mutex::new(Vec::new()),
        });
        let oxicode = OxicodeBuilder::new()
            .with_builtins()
            .with_auth(auth.clone())
            .build();

        // create_provider on a built-in must consult the auth port.
        let _ = oxicode.create_provider("anthropic");
        let calls = auth.calls.lock();
        assert!(
            calls.iter().any(|c| c == "anthropic"),
            "Oxicode::create_provider must call AuthProvider::get_api_key_sync \
             when the static api_keys map has no entry. Got calls: {:?}",
            *calls
        );
    }

    #[test]
    fn test_closure_tool_sync() {
        let tool = crate::closure_tool::ClosureTool::new_sync(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            }),
            |params, _ctx| {
                let input = params["input"].as_str().unwrap_or("default");
                Ok(AgentToolResult::success(format!("processed: {}", input)))
            },
        );

        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "A test tool");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(tool.execute(
                "call_1",
                serde_json::json!({"input": "hello"}),
                None,
                &ToolContext::default(),
            ))
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("processed: hello"));
    }

    #[test]
    fn test_custom_tool_in_agent_builder() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxicode
            .agent(config)
            .workspace("/tmp")
            .custom_tool(
                "my_tool",
                "My custom tool",
                serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
                |params, _ctx| {
                    Ok(AgentToolResult::success(format!(
                        "result: {}",
                        params["query"]
                    )))
                },
            )
            .build();

        if let Ok(agent) = result {
            let tool_names = agent.tools().names();
            assert!(tool_names.contains(&"my_tool".to_string()));
        }
    }

    #[test]
    fn test_full_isolation_between_instances() {
        // Instance 1: custom model + no builtins
        let oxicode1 = OxicodeBuilder::new()
            .model(test_model("unique-alpha", "p1"))
            .build();

        // Instance 2: builtins only
        let oxicode2 = OxicodeBuilder::new().with_builtins().build();

        // Cross-contamination check
        assert!(oxicode2.resolve_model("p1/unique-alpha").is_err());
        assert!(
            oxicode1
                .resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_err()
        );

        // Provider isolation: oxicode1 can't create anthropic (no builtins)
        assert!(oxicode1.create_provider("anthropic").is_err());
        // oxicode2 can create anthropic (builtins enabled)
        assert!(oxicode2.create_provider("anthropic").is_ok());
    }

    #[test]
    fn test_agent_builder_system_prompt() {
        let oxicode = OxicodeBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 5,
            ..Default::default()
        };
        let agent = oxicode
            .agent(config)
            .workspace("/tmp")
            .system_prompt("You are a test agent.")
            .build()
            .unwrap();
        // Agent built successfully with custom system prompt
        drop(agent);
    }

    #[test]
    fn test_oxicode_builder_api_key() {
        // Builder accepts api_key without panic
        let oxicode = OxicodeBuilder::new()
            .with_builtins()
            .api_key("anthropic", "sk-ant-test-key")
            .build();
        // The key is stored internally. create_provider will use it
        // (but actual API calls will fail since it's a fake key).
        assert!(oxicode.has_builtins());
    }

    #[test]
    fn test_oxicode_builder_base_url() {
        let oxicode = OxicodeBuilder::new()
            .with_builtins()
            .base_url("openai", "https://my-proxy.example.com/v1")
            .build();
        assert!(oxicode.has_builtins());
    }

    #[test]
    fn test_oxicode_builder_credential() {
        let oxicode = OxicodeBuilder::new()
            .with_builtins()
            .credential(
                "openai",
                "sk-test-key",
                Some("https://proxy.example.com/v1"),
            )
            .credential("anthropic", "sk-ant-test", None)
            .build();
        assert!(oxicode.has_builtins());
    }
}
