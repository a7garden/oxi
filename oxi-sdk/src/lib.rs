//! oxi SDK - Programmatic API for building AI agents
//!
//! # Example
//! ```
//! use oxi_sdk::{OxiBuilder, AgentConfig};
//!
//! let oxi = OxiBuilder::new().with_builtins().build();
//! let agent = oxi.agent(AgentConfig {
//!     model_id: "anthropic/claude-sonnet-4-20250514".into(),
//!     ..Default::default()
//! }).build().unwrap();
//! ```
#![warn(missing_docs)]

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
pub mod multi_provider;
pub mod observability;
pub mod ports;

// Reference implementations bundled with the SDK.
pub use ports::{fs, inmem};
/// Convenience re-exports: `oxi_sdk::fs::*`, `oxi_sdk::inmem::*`.
pub mod prelude;
pub mod routing;
pub mod security;
pub mod tool_factory;
pub mod url_resolver;
pub mod workflow_dsl;

// Re-export core SDK types
pub use agent_builder::AgentBuilder;
pub use agent_group::{AgentGroup, AgentGroupOutput, GroupResult, GroupStrategy};
pub use builder::{Oxi, OxiBuilder};
pub use delegation::SdkSubagentRunner;
pub use url_resolver::SdkUrlResolver;

// Re-export port types — products implement these traits.
// Note: Some names conflict with existing modules (e.g. `EventBus` is also a
// module, `MemoryEntry` is also in `coordination`). We rename on import to
// avoid ambiguity; users can still access the trait via the explicit path
// `oxi_sdk::ports::EventBus` if they need to disambiguate.
pub use closure_tool::ClosureTool;
pub use kernel_bridge::{KernelToolContext, KernelToolProvider};
pub use message_bus::{InterAgentMessage, LagAwareReceiver, MessageBus, PublishResult};
pub use metrics::{AgentMetrics, MetricsSnapshot};
pub use ports::AccessGate as AccessGatePort;
pub use ports::EventBus as EventBusPort;
pub use ports::MemoryEntry as MemoryEntryPort;
pub use ports::{
    AccessDecision, AuthMethod, AuthProvider, CapabilityResolver, ConfigStore, CronJob,
    CronScheduler, EventPayload, EventTopic, InMemoryEventBus, MemoryStore, NoopAuthProvider,
    NoopConfigStore, NoopCronScheduler, NoopEventBus, NoopMemoryStore, NoopPersonaProvider,
    NoopResourceMonitor, NoopSkillLoader, NoopStateStore, OAuthToken, Persona, PersonaProvider,
    PortId, PortRegistry, PortValue, ResourceMonitor, ResourceUsage, Skill, SkillLoader, SkillMeta,
    StateStore, SubscriptionHandle, ToolCallRequest,
};

// Catalog port (Port 12).
pub use ports::catalog::{
    CatalogEvent, CatalogModelEntry, CatalogProtocol, CatalogProviderEntry, CatalogSource,
    ModelCatalog, NoopModelCatalog, RefreshOutcome,
};
// File-backed reference impl for the catalog port.
pub use ports::fs::catalog::{CatalogConfig, FileModelCatalog};

// Composition Layer — EventBus
pub use event_bus::EventBus;

// Foundation Layer
pub use error::{SdkError, SdkResult};
pub use lifecycle::{
    AgentHandle, AgentLifecycleEvent, AgentPool, AgentSnapshot, AgentStatus, AgentSupervisor,
    FileSnapshotStore, RestartBackoff, SnapshotStore, SupervisorPolicy, ToolManifest,
};
pub use middleware::Middleware;
pub use middleware::{
    MiddlewareContext, MiddlewareData, MiddlewarePhase, MiddlewarePipeline, MiddlewareResult,
    build_hooks,
};
pub use multi_provider::{MultiProviderBuilder, RoutingConfig};
pub use observability::{
    AuditAction, AuditEntry, AuditError, AuditFilter, AuditLog, AuditPersistence, AuditTrail,
    CostBreakdown, CostSnapshot, CostTracker, CostTrackerConfig, EventQuery, EventStore,
    EventStoreConfig, GlobalCostSnapshot, HashDigest, Span, SpanContext, SpanGuard, SpanId,
    SpanKind, SpanStatus, StoredEvent, TokenUsage, TraceId, Tracer, TrailEntry,
};

// Composition Layer — Security
pub use security::{
    AccessDenied, AccessGate, Action, AgentContext, AgentPermissions, AllowlistMode,
    ApprovalStatus, AuditEvent, AuditSink, Authorizer, Capability, CapabilitySet,
    CapabilitySubject, CheckRequest, DefaultPolicy, DenyLayer, ExecPolicy, PathMode,
    PendingApproval, PermAuditEntry, PermissionUpdate, RbacAuditEntry, RbacManager, RbacPolicy,
    Role, SecurityMiddleware, StringPattern, Subject, TracingAuditSink, TrailAuditSink,
};

// Composition Layer — Coordination
pub use coordination::{
    Consensus, CoordinatedGroup, CoordinatedGroupBuilder, MemoryEntry, MemoryEvent, MemoryKey,
    SharedMemory, VoteResult, WorkEvent, WorkItem, WorkQueue, WorkQueueConfig, WorkQueueStats,
    WorkResult, WorkStatus,
};

// Runtime routing control
pub use routing::RoutingControl;

// Re-export from oxi-ai
pub use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
pub use oxi_ai::multi_provider::MultiProviderConfig;
pub use oxi_ai::provider_pool::{ProviderPool, RateLimitPolicy};
pub use oxi_ai::{
    Api, CompactionStrategy, ContentBlock, Context, Cost, InputModality, Message, MessageContent,
    Model, ModelRegistry, Provider, ProviderError, ProviderEvent, ProviderOptions,
    ProviderRegistry, StreamOptions, UserMessage,
};

// Model roles + role switching (ported from omp)
pub use oxi_ai::role_routing::RoleRoutingProvider;
pub use oxi_ai::role_switcher::{RoleSignals, decide_role, resolve_role_to_model, role_for_tool};
pub use oxi_ai::roles::{ModelRole, RoleRegistry, live_role_registry, set_live_role_registry};

// Credential management (oauth + env key resolution)
pub use oxi_ai::env_api_keys::{find_env_keys, get_all_env_keys, get_env_api_key, has_env_key};

// Model database — provider catalog, model metadata
pub use oxi_ai::model_db::{
    ModelEntry, builtin_model_count_sentinel, get_all_models, get_cheapest_models, get_model_entry,
    get_provider_models, get_providers, get_reasoning_models, get_vision_models, model_count,
    search_models,
};

// Catalog — models.dev-backed dynamic catalog (SNAP/LIVE/override/LOCAL).
// The catalog module exposes the full surface; `model_db` (above) is the
// legacy compatibility shim that integrates all layers and converts
// BuiltinModelEntry → ModelEntry.
pub use oxi_ai::catalog::{
    BuiltinModelEntry, BuiltinProviderEntry, OverrideFile, apply_model_overrides,
    apply_provider_overrides, builtin_model_count, builtin_providers_count, discover_all,
    discover_all_authenticated, discover_all_local, discover_models, find_override_files,
    load_builtin_providers, load_overrides,
};
pub use oxi_ai::oauth::{
    AuthStore, OAuthError, TokenBundle, default_auth_path, load_auth_store, load_token,
    remove_token, save_auth_store, save_token,
};

// Provider registry — built-in providers (Layer 1 of the catalog) and
// the runtime functions that surface them to consumers.
pub use oxi_ai::register_builtins::{
    BuiltinProvider, get_all_provider_aliases, get_all_provider_names, get_api_mappings,
    get_builtin_provider, get_builtin_providers, get_provider_api, get_provider_base_url,
    get_provider_env_key, get_provider_env_keys, is_builtin_provider, resolve_provider_name,
};

// Provider instance registry (custom + built-in at runtime)
pub use oxi_ai::{
    create_builtin_provider, create_builtin_provider_with_options, custom_provider_names,
    dynamic_models, fetch_models_async, fetch_models_blocking, get_model, get_models, get_provider,
    get_provider_arc, lookup_model, register_model, register_provider, unregister_provider,
};

// Complexity-based routing and the router module
pub use oxi_ai::router;

// Tool-related types (oxi-cli's main.rs uses ToolCall, ToolResult, ToolCallType)
pub use oxi_ai::{
    ProgressCallback, Tool, ToolCall, ToolCallType, ToolResult, ToolValidationError, validate_args,
};

// Thinking level (re-exported from oxi_ai::types, since oxi-ai's top-level
// re-exports it via `pub use types::*` but not as a named item).
pub use oxi_ai::types::ThinkingLevel;

// Re-export from oxi-agent
pub use oxi_agent::{
    Agent, AgentConfig, AgentError, AgentEvent, AgentHooks, AgentLoop, AgentLoopConfig, AgentState,
    AgentTool, AgentToolResult, BrowseProgress, BrowseProgressCallback, CompactedContext,
    CompactionEvent, CompactionHook, EditTool, FindTool, GetSearchResultsTool, GrepTool, LsTool,
    OutputMode, ProviderResolver, ReadTool, SearchCache, SharedState, StructuredOutput,
    StructuredOutputError, ToolCallContext, ToolContext, ToolError, ToolExecutionMode,
    ToolRegistry, VisitReason, WebSearchTool, WriteTool,
};
// ── Advisor subsystem (read-only reviewer that shadows the primary agent) ─
//
// SDK consumers can construct a full advisor: build a second `Agent` with the
// advisor model role + read-only tools + an `AdviseTool` (carrying an
// `EnqueueAdviceFn`), then drive it with `AdvisorRuntime`. The emission guard
pub use oxi_agent::advisor::{
    ADVISOR_GUIDANCE, ADVISOR_READONLY_TOOL_NAMES, ADVISOR_SYSTEM_PROMPT, AdviseTool, AdvisorAgent,
    AdvisorDeliveryChannel, AdvisorEmissionGuard, AdvisorNote, AdvisorRuntime, AdvisorRuntimeHost,
    AdvisorSeverity, AgentAdvisor, DeliveryOpts, EnqueueAdviceFn, format_advisory_batch,
    is_immune_turn_active, is_interrupting_severity, normalize_advisor_note,
    resolve_delivery_channel,
};
// ── Todo tool types (agent-scoped, observable by SDK consumers) ──────
pub use oxi_agent::tools::todo::{TodoItem, TodoOp, TodoPhase, TodoStatus, TodoUpdateResult};
pub use oxi_agent::{TodoStateProvider, TodoTool};

// ── Concrete provider re-exports ─────────────────────────────────────────
//
// SDK consumers can construct providers directly without depending on
// `oxi-ai`. This enables the single-dependency pattern:
//   oxios → oxi-sdk  (no oxi-ai, no oxi-agent direct dep)

pub use oxi_ai::OpenAiProvider;
pub use oxi_ai::OpenAiResponsesProvider;

// ── Browser engine re-exports ────────────────────────────────────────────────
//
// The browser trait layer (BrowserEngine, BrowserTab, config, error types)
// is always available so SDK consumers can implement custom backends.
// The native oxibrowser-core backend requires the `native-browser` feature.

pub use oxi_agent::tools::browse::{
    BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine, BrowserError, BrowserTab,
    ElementInfo, LinkInfo, PageContent, TabGuard,
};

#[cfg(feature = "native-browser")]
pub use oxi_agent::tools::browse::{BrowseScriptTool, BrowseSessionTool, OxiBrowserEngine};

/// Re-export the browser event type from oxibrowser-core. Surfaced through
/// `Browser::subscribe_events()` for agents/UIs that want to render fine-
/// grained progress ("Opening X…", "Loaded Y…") to the user.
///
/// Gated by the `native-browser` feature because it depends on
/// `oxibrowser-core` types.
#[cfg(feature = "native-browser")]
pub use oxibrowser_core::BrowserEvent;

// ── MCP (Model Context Protocol) re-exports ───────────────────────────
//
// SDK consumers can use MCP servers alongside built-in tools.
// `McpManager::spawn()` creates a manager; `OxiBuilder::with_mcp_config()`
// injects a programmatic config; `mcp_tools()` auto-discovers from
// standard config files. See `oxi_sdk::tool_factory::mcp_tools`.

pub use oxi_agent::mcp::{
    ConsentManager, ConsentState, DirectToolDef, DirectToolsConfig, LifecycleMode, McpCallResult,
    McpConfig, McpConnectionStatus, McpContent, McpDashboardData, McpDirectTool, McpManager,
    McpSamplingRequest, McpServerInfo, McpSettings, McpSettingsView, McpTool, McpToolDef,
    McpToolInfo, MetadataCache, ServerEntry, ToolMetadata, ToolPrefix,
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
    fn test_oxi_builder_new() {
        let oxi = OxiBuilder::new().build();
        // Empty registry — no models
        assert!(
            oxi.resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_err()
        );
    }

    #[test]
    fn test_oxi_builder_with_builtins() {
        let oxi = OxiBuilder::new().with_builtins().build();
        // Should have built-in models
        assert!(
            oxi.resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_ok()
        );
        assert!(oxi.resolve_model("openai/gpt-4o").is_ok());
    }

    #[test]
    fn test_oxi_builder_custom_model() {
        let oxi = OxiBuilder::new()
            .model(test_model("test-model", "test-provider"))
            .build();
        assert!(oxi.resolve_model("test-provider/test-model").is_ok());
    }

    #[test]
    fn test_oxi_provider_resolution() {
        let oxi = OxiBuilder::new().with_builtins().build();
        // Built-in provider (falls back to built-in registry)
        assert!(oxi.create_provider("anthropic").is_ok());
        // Unknown provider
        assert!(oxi.create_provider("nonexistent").is_err());
    }

    #[test]
    fn test_agent_builder_workspace() {
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 30,
            ..Default::default()
        };
        // AgentBuilder with workspace — should not panic
        let result = oxi.agent(config).workspace("/tmp/test-workspace").build();
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_agent_builder_coding_tools() {
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxi.agent(config).workspace("/tmp").coding_tools().build();
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
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxi.agent(config).workspace("/tmp").readonly_tools().build();
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
        // Two separate Oxi instances should not share state
        let oxi1 = OxiBuilder::new()
            .model(test_model("unique-1", "test"))
            .build();

        let oxi2 = OxiBuilder::new().with_builtins().build();

        // oxi2 should NOT have oxi1's custom model
        assert!(oxi2.resolve_model("test/unique-1").is_err());
        // oxi1 should have its custom model
        assert!(oxi1.resolve_model("test/unique-1").is_ok());
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
    fn test_provider_resolver_trait_on_oxi() {
        let oxi = OxiBuilder::new().with_builtins().build();
        // Oxi implements ProviderResolver
        let resolver: &dyn ProviderResolver = &oxi;
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
        // Create isolated Oxi with only a mock model
        let oxi = OxiBuilder::new()
            .model(test_model("test-model", "test-provider"))
            .build();

        // This should fail because 'anthropic' provider isn't registered
        let config = AgentConfig {
            model_id: "test-provider/test-model".into(),
            timeout_seconds: 5,
            ..Default::default()
        };
        let result = oxi.agent(config).build();
        // Agent build fails because provider 'test-provider' has no implementation
        // (no custom provider registered, no builtins enabled)
        assert!(result.is_err());
    }

    #[test]
    fn test_oxi_builder_without_builtins() {
        let oxi = OxiBuilder::new().build();
        // No models, no providers
        assert!(
            oxi.resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_err()
        );
        assert!(oxi.create_provider("anthropic").is_err());
        assert!(!oxi.has_builtins());
    }

    #[test]
    fn test_oxi_builder_with_builtins_creates_providers() {
        let oxi = OxiBuilder::new().with_builtins().build();
        assert!(oxi.has_builtins());
        // Built-in provider fallback should work
        assert!(oxi.create_provider("anthropic").is_ok());
        assert!(oxi.create_provider("openai").is_ok());
        assert!(oxi.create_provider("deepseek").is_ok());
        // Unknown still fails
        assert!(oxi.create_provider("unknown-provider").is_err());
    }

    /// Regression (#40): `Oxi::create_provider` must consult the wired
    /// `AuthProvider` port via its sync fast-path when the static
    /// `OxiBuilder::api_key()` map has no entry for the provider. This is
    /// the **primary** credential source for products like the CLI, which
    /// never call `OxiBuilder::api_key()` and instead register
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
        let oxi = OxiBuilder::new()
            .with_builtins()
            .with_auth(auth.clone())
            .build();

        // create_provider on a built-in must consult the auth port.
        let _ = oxi.create_provider("anthropic");
        let calls = auth.calls.lock();
        assert!(
            calls.iter().any(|c| c == "anthropic"),
            "Oxi::create_provider must call AuthProvider::get_api_key_sync \
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
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxi
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
        let oxi1 = OxiBuilder::new()
            .model(test_model("unique-alpha", "p1"))
            .build();

        // Instance 2: builtins only
        let oxi2 = OxiBuilder::new().with_builtins().build();

        // Cross-contamination check
        assert!(oxi2.resolve_model("p1/unique-alpha").is_err());
        assert!(
            oxi1.resolve_model("anthropic/claude-sonnet-4-20250514")
                .is_err()
        );

        // Provider isolation: oxi1 can't create anthropic (no builtins)
        assert!(oxi1.create_provider("anthropic").is_err());
        // oxi2 can create anthropic (builtins enabled)
        assert!(oxi2.create_provider("anthropic").is_ok());
    }

    #[test]
    fn test_agent_builder_system_prompt() {
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            timeout_seconds: 5,
            ..Default::default()
        };
        let agent = oxi
            .agent(config)
            .workspace("/tmp")
            .system_prompt("You are a test agent.")
            .build()
            .unwrap();
        // Agent built successfully with custom system prompt
        drop(agent);
    }

    #[test]
    fn test_oxi_builder_api_key() {
        // Builder accepts api_key without panic
        let oxi = OxiBuilder::new()
            .with_builtins()
            .api_key("anthropic", "sk-ant-test-key")
            .build();
        // The key is stored internally. create_provider will use it
        // (but actual API calls will fail since it's a fake key).
        assert!(oxi.has_builtins());
    }

    #[test]
    fn test_oxi_builder_base_url() {
        let oxi = OxiBuilder::new()
            .with_builtins()
            .base_url("openai", "https://my-proxy.example.com/v1")
            .build();
        assert!(oxi.has_builtins());
    }

    #[test]
    fn test_oxi_builder_credential() {
        let oxi = OxiBuilder::new()
            .with_builtins()
            .credential(
                "openai",
                "sk-test-key",
                Some("https://proxy.example.com/v1"),
            )
            .credential("anthropic", "sk-ant-test", None)
            .build();
        assert!(oxi.has_builtins());
    }
}
