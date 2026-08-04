//! AgentBuilder — Fluent API for creating agents

use std::path::PathBuf;
use std::sync::Arc;

use oxicode_agent::{
    Agent, AgentConfig, AgentTool, AgentToolResult, ProviderResolver, ToolContext, ToolRegistry,
    tools::browse::{BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine},
};

use crate::builder::Oxicode;
use crate::middleware::{Middleware, MiddlewarePipeline};
use crate::observability::{AuditLog, CostTracker, Tracer};
use crate::security::{Authorizer, CapabilitySet};

/// Closures owned by the cli (or any product) that participate in the
/// agent hook chain. Passed into [`AgentBuilder::with_session_hooks`] so
/// they are composed into the same `AgentHooks` that the middleware
/// pipeline produces. The single-`set_hooks` invariant (only
/// [`AgentBuilder::build`] calls `set_hooks`) is what keeps the
/// before/after_tool_call slots alive across the cli session boot.
pub struct SessionHookClosures {
    /// Stop signal consulted at the end of every turn.
    pub should_stop_after_turn:
        std::sync::Arc<dyn Fn(&oxicode_agent::ShouldStopAfterTurnContext) -> bool + Send + Sync>,
    /// Drain the steering queue on demand.
    pub get_steering_messages: std::sync::Arc<dyn Fn() -> Vec<oxicode_ai::Message> + Send + Sync>,
    /// Drain the follow-up queue on demand.
    pub get_follow_up_messages: std::sync::Arc<dyn Fn() -> Vec<oxicode_ai::Message> + Send + Sync>,
    /// Tool-execution mode (Sequential is the cli default).
    pub tool_execution: oxicode_agent::ToolExecutionMode,
}

/// Builder for creating an agent with custom configuration.
#[allow(dead_code)]
pub struct AgentBuilder<'a> {
    oxicode: &'a Oxicode,
    config: AgentConfig,
    tools: ToolRegistry,
    workspace_dir: Option<PathBuf>,
    system_prompt: Option<String>,
    // ── Security ──
    capabilities: Option<CapabilitySet>,
    authorizer: Option<Arc<Authorizer>>,
    // ── Observability ──
    tracer: Option<Arc<Tracer>>,
    audit_log: Option<Arc<AuditLog>>,
    cost_tracker: Option<Arc<CostTracker>>,
    // ── Middleware ──
    middlewares: Vec<Arc<dyn Middleware>>,
    // ── Hooks (port 16) ──
    hooks_middleware: Option<crate::middleware::HookMiddleware>,
    // ── Session-level closures (cli-owned stop flag + queues) ──
    session_hooks: Option<SessionHookClosures>,
}

impl<'a> AgentBuilder<'a> {
    /// Create a new builder bound to the given [`Oxicode`] instance with the provided agent config.
    pub fn new(oxicode: &'a Oxicode, config: AgentConfig) -> Self {
        Self {
            oxicode,
            config,
            tools: ToolRegistry::new(),
            workspace_dir: None,
            system_prompt: None,
            capabilities: None,
            authorizer: None,
            tracer: None,
            audit_log: None,
            cost_tracker: None,
            middlewares: Vec::new(),
            hooks_middleware: None,
            session_hooks: None,
        }
    }

    /// Set the working directory for file tools.
    pub fn workspace(mut self, dir: impl Into<PathBuf>) -> Self {
        self.workspace_dir = Some(dir.into());
        self
    }

    /// Set a custom system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
    /// Register a [`TodoStateProvider`](crate::TodoStateProvider) so the agent's `todo` tool works.
    ///
    /// The provider is shared between the agent (writer) and the host
    /// application (reader), so you can observe phase changes in real time
    /// by calling [`TodoStateProvider::get_phases()`](crate::TodoStateProvider::get_phases) periodically.
    ///
    /// Use [`InMemoryTodoState`](crate::inmem::InMemoryTodoState) for a
    /// ready-to-go in-memory implementation:
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use oxicode_sdk::{AgentConfig, OxicodeBuilder, inmem::InMemoryTodoState};
    ///
    /// let todo = Arc::new(InMemoryTodoState::new());
    /// let oxicode = OxicodeBuilder::new().with_builtins().build();
    /// let agent = oxicode.agent(AgentConfig {
    ///     model_id: "anthropic/claude-sonnet-4-20250514".into(),
    ///     ..Default::default()
    /// })
    /// .with_todo(todo.clone())
    /// .build()
    /// .unwrap();
    ///
    /// // Observe later:
    /// let phases = todo.get_phases();
    /// ```
    pub fn with_todo(
        mut self,
        todo: std::sync::Arc<dyn oxicode_agent::tools::TodoStateProvider>,
    ) -> Self {
        self.config.todo = Some(todo);
        self.tools.register(oxicode_agent::tools::todo::TodoTool);
        self
    }

    /// Register a [`MemoryBackend`](oxicode_agent::tools::MemoryBackend) and the
    /// four `memory_*` tools (`memory_recall`, `memory_reflect`,
    /// `memory_retain`, `memory_edit`).
    ///
    /// Generic entry point: pass any `MemoryBackend`. For the common case of
    /// bridging the engine's registered `MemoryStore` port, use
    /// [`Self::with_port_memory`] instead.
    pub fn with_memory_backend(
        mut self,
        backend: std::sync::Arc<dyn oxicode_agent::tools::MemoryBackend>,
    ) -> Self {
        self.config.memory = Some(backend);
        self.tools.register(oxicode_agent::tools::MemoryRecallTool);
        self.tools.register(oxicode_agent::tools::MemoryReflectTool);
        self.tools.register(oxicode_agent::tools::MemoryRetainTool);
        self.tools.register(oxicode_agent::tools::MemoryEditTool);
        self
    }

    /// Bridge the engine's registered `MemoryStore` (+ `EmbeddingProvider`)
    /// ports into this agent's `memory_*` tools via `PortMemoryBackend`.
    ///
    /// This is how a pure-SDK consumer makes memory functional end-to-end:
    /// register the ports on [`OxicodeBuilder`](crate::OxicodeBuilder)
    /// (`with_memory` / `with_embeddings`), then call this on the agent.
    /// Without an `EmbeddingProvider`, `put` / `list` / `delete` work but
    /// semantic `search` returns an error.
    ///
    /// Without this call (or [`Self::with_memory_backend`]), the
    /// `memory_*` tools are absent and `ToolContext.memory` stays `None` —
    /// the registered `MemoryStore` port is unused by the agent loop.
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use oxicode_sdk::{OxicodeBuilder, inmem::InMemoryMemoryStore};
    ///
    /// let oxicode = OxicodeBuilder::new()
    ///     .with_builtins()
    ///     .with_memory(Arc::new(InMemoryMemoryStore::new()))
    ///     .build();
    /// let agent = oxicode.agent(oxicode_agent::AgentConfig {
    ///     model_id: "anthropic/claude-sonnet-4-20250514".into(),
    ///     ..Default::default()
    /// })
    /// .with_port_memory()
    /// .build()
    /// .unwrap();
    /// ```
    pub fn with_port_memory(self) -> Self {
        let ports = self.oxicode.ports().clone();
        let backend = crate::port_memory_backend::PortMemoryBackend::from_ports(
            ports.memory.clone(),
            Some(ports.embeddings.clone()),
        );
        self.with_memory_backend(std::sync::Arc::new(backend))
    }

    /// Set the URL resolver — enables internal-URL dispatch (`issue://`,
    /// `skill://`, `memory://`, …) in the `read`/`grep`/`find` tools.
    pub fn with_url_resolver(
        mut self,
        resolver: std::sync::Arc<dyn oxicode_agent::tools::UrlResolver>,
    ) -> Self {
        self.config.url_resolver = Some(resolver);
        self
    }

    /// Bridge the engine's registered `InternalUrlRouter` port into this
    /// agent's `read`/`grep`/`find` tools via [`SdkUrlResolver`](crate::url_resolver::SdkUrlResolver).
    ///
    /// This is how a pure-SDK consumer enables protocol-scheme URL
    /// resolution: register scheme handlers on the router port, then call
    /// this. Without it (or [`Self::with_url_resolver`]), URL-prefixed
    /// paths are treated as regular file paths.
    pub fn with_port_url_resolver(self) -> Self {
        let resolver = std::sync::Arc::new(crate::url_resolver::SdkUrlResolver::new(
            self.oxicode.ports().url_router.clone(),
        ));
        self.with_url_resolver(resolver)
    }

    /// Set the hashline snapshot store — enables line-anchored edit mode
    /// (`read` emits `[path#TAG]` headers, `edit` validates against them).
    ///
    /// Without this, the `edit` tool falls back to plain text replacement.
    /// Use [`oxicode_hashline::InMemorySnapshotStore`] for an ephemeral store, or
    /// implement [`oxicode_hashline::SnapshotStore`] for persistence.
    pub fn with_snapshot_store(
        mut self,
        store: std::sync::Arc<dyn oxicode_hashline::SnapshotStore>,
    ) -> Self {
        self.config.snapshot_store = Some(store);
        self
    }

    /// Bridge the engine into an in-process subagent runner and register the
    /// `subagent` tool.
    ///
    /// Uses [`SdkSubagentRunner`](crate::delegation::SdkSubagentRunner) so the `subagent`
    /// tool runs isolated agents in-process (no CLI binary). Without this, the
    /// `subagent` tool is absent from the agent's toolset.
    pub fn with_port_subagent(mut self) -> Self {
        let runner = std::sync::Arc::new(crate::delegation::SdkSubagentRunner::new(
            self.oxicode.clone(),
        ));
        self.config.subagent_runner = Some(runner);
        self.tools
            .register(oxicode_agent::tools::SubagentTool::new());
        self
    }

    /// Add the [`HookMiddleware`](crate::middleware::HookMiddleware) backed by the engine's registered
    /// `HookRunner` port (see [`crate::OxicodeBuilder::with_hooks`]).
    ///
    /// When the port is `NoopHookRunner` (the default), this is a no-op.
    /// The middleware composes into the existing pipeline at the
    /// `audit → authorizer → hooks → user` position. `set_hooks` is called
    /// exactly once in `build()` — see the single-`set_hooks` invariant.
    pub fn with_port_hooks(mut self) -> Self {
        let runner = std::sync::Arc::clone(&self.oxicode.ports().hooks);
        self.hooks_middleware = Some(crate::middleware::HookMiddleware::new(runner));
        self
    }

    /// Install session-level closures (stop flag + steering/follow_up
    /// queues). These are composed into the same `AgentHooks` that the
    /// middleware pipeline produces, so `set_hooks` is called exactly once.
    /// This is the **only** way to install session hooks — never call
    /// `agent.set_hooks(...)` elsewhere (it would wipe the middleware
    /// pipeline's before/after_tool_call slots).
    pub fn with_session_hooks(mut self, closures: SessionHookClosures) -> Self {
        self.session_hooks = Some(closures);
        self
    }

    /// Register the standard coding tools (read, write, edit, bash, grep, find, ls, ...).
    pub fn coding_tools(self) -> Self {
        let cwd = self
            .workspace_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let tools = crate::tool_factory::coding_tools(&cwd);
        for name in tools.names() {
            if let Some(tool) = tools.get(&name) {
                self.tools.register_arc(tool);
            }
        }
        self
    }

    /// Register read-only tools (read, ls).
    pub fn readonly_tools(self) -> Self {
        let cwd = self
            .workspace_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let tools = crate::tool_factory::readonly_tools(&cwd);
        for name in tools.names() {
            if let Some(tool) = tools.get(&name) {
                self.tools.register_arc(tool);
            }
        }
        self
    }

    /// Register a tool.
    pub fn tool(self, tool: impl AgentTool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    /// Register a custom tool from a closure (synchronous handler).
    ///
    /// Creates a `ClosureTool` internally.
    ///
    /// # Example
    /// ```rust
    /// use oxicode_sdk::{ClosureTool, AgentToolResult};
    ///
    /// // custom_tool creates a tool from a closure
    /// let tool = ClosureTool::new_sync(
    ///     "memory_recall",
    ///     "Search long-term memory",
    ///     serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
    ///     |params, _ctx| {
    ///         let query = params["query"].as_str().unwrap();
    ///         Ok(AgentToolResult::success(format!("Recalled: {}", query)))
    ///     },
    /// );
    /// ```
    pub fn custom_tool(
        self,
        name: impl Into<String>,
        description: impl Into<String>,
        schema: serde_json::Value,
        handler: impl Fn(
            serde_json::Value,
            &ToolContext,
        ) -> Result<AgentToolResult, oxicode_agent::ToolError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.tool(crate::closure_tool::ClosureTool::new_sync(
            name,
            description,
            schema,
            handler,
        ))
    }

    /// Register multiple tools.
    pub fn tools(self, tools: impl IntoIterator<Item = impl AgentTool + 'static>) -> Self {
        for tool in tools {
            self.tools.register(tool);
        }
        self
    }

    /// Register browser tools (browse, browse_extract) with the given engine.
    ///
    /// This is the primary entry point for SDK consumers that want built-in
    /// web browsing. Pass any [`BrowserEngine`] implementation — when the
    /// `native-browser` feature is enabled on `oxicode-agent`, use
    /// `oxicode_agent::tools::browse::OxicodeBrowserEngine` for
    /// the built-in headless browser.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use oxicode_sdk::prelude::*;
    ///
    /// // Requires a BrowserEngine implementation
    /// let engine: Arc<dyn BrowserEngine> = /* ... */;
    /// let agent = oxicode.agent(config)
    ///     .workspace("/project")
    ///     .coding_tools()
    ///     .browsing(engine)
    ///     .build()?;
    /// ```
    pub fn browsing(self, engine: Arc<dyn BrowserEngine>) -> Self {
        self.tools.register(BrowseTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseExtractTool::new(engine));
        self
    }

    /// Register browser tools with custom configuration.
    ///
    /// Like [`browsing()`](Self::browsing) but allows tuning timeouts,
    /// cache, tab limits, etc. via [`BrowseConfig`].
    pub fn browsing_with_config(
        self,
        engine: Arc<dyn BrowserEngine>,
        config: BrowseConfig,
    ) -> Self {
        self.tools
            .register(BrowseTool::with_config(Arc::clone(&engine), config.clone()));
        self.tools
            .register(BrowseExtractTool::with_config(engine, config));
        self
    }

    /// Register the native browser tools using `oxibrowser-core`.
    ///
    /// Convenience method that creates an `OxicodeBrowserEngine` and registers
    /// all browser tools. Only available when the `native-browser` feature
    /// is enabled.
    #[cfg(feature = "native-browser")]
    #[cfg_attr(docsrs, doc(cfg(feature = "native-browser")))]
    pub async fn native_browser(self) -> anyhow::Result<Self> {
        let engine = oxicode_agent::tools::browse::OxicodeBrowserEngine::new().await?;
        Ok(self.browsing(Arc::new(engine)))
    }

    /// Register all browser tools including persistent session support.
    ///
    /// Like [`browsing()`](Self::browsing) but also registers `browse_script`
    /// and `browse_session` for multi-step interactive sessions with a
    /// persistent tab. Only available when the `native-browser` feature
    /// is enabled.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use oxicode_sdk::prelude::*;
    ///
    /// // Requires the native-browser feature and OxicodeBrowserEngine
    /// let engine: Arc<dyn BrowserEngine> = /* ... */;
    /// let agent = oxicode.agent(config)
    ///     .browsing_with_session(engine)
    ///     .build()?;
    /// ```
    #[cfg(feature = "native-browser")]
    #[cfg_attr(docsrs, doc(cfg(feature = "native-browser")))]
    pub fn browsing_with_session(self, engine: Arc<dyn BrowserEngine>) -> Self {
        use oxicode_agent::tools::browse::{BrowseScriptTool, BrowseSessionTool};

        self.tools.register(BrowseTool::new(Arc::clone(&engine)));
        self.tools
            .register(BrowseExtractTool::new(Arc::clone(&engine)));
        self.tools
            .register(BrowseScriptTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseSessionTool::new(engine));
        self
    }

    /// Register kernel tools from a [`KernelToolProvider`].
    ///
    /// This is the bridge for oxios kernel tools (exec, memory, browser, etc.).
    /// The kernel implements `KernelToolProvider` and registers its tools
    /// into the agent's tool registry.
    ///
    /// [`KernelToolProvider`]: crate::KernelToolProvider
    pub fn kernel_tools(
        self,
        provider: &dyn crate::KernelToolProvider,
        context: &crate::KernelToolContext,
    ) -> Self {
        provider.register_tools(&self.tools, context);
        self
    }

    // ── Security ──────────────────────────────────────────

    /// Set the capability set for this agent.
    pub fn capabilities(mut self, caps: CapabilitySet) -> Self {
        self.capabilities = Some(caps);
        self
    }

    /// Use standard coding capabilities.
    pub fn coding_capabilities(self) -> Self {
        let ws = self
            .workspace_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        self.capabilities(CapabilitySet::coding(ws.to_str().unwrap_or(".")))
    }

    /// Use read-only capabilities.
    pub fn readonly_capabilities(self) -> Self {
        let ws = self
            .workspace_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        self.capabilities(CapabilitySet::read_only(ws.to_str().unwrap_or(".")))
    }

    /// Attach an authorizer for capability enforcement.
    pub fn authorizer(mut self, authorizer: Arc<Authorizer>) -> Self {
        self.authorizer = Some(authorizer);
        self
    }

    // ── Observability ──────────────────────────────────────

    /// Attach a tracer for distributed tracing.
    pub fn tracer(mut self, tracer: Arc<Tracer>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Attach an audit log for security and tool audit trail.
    pub fn audit_log(mut self, audit: Arc<AuditLog>) -> Self {
        self.audit_log = Some(audit);
        self
    }

    /// Attach a cost tracker for token and cost monitoring.
    pub fn cost_tracker(mut self, tracker: Arc<CostTracker>) -> Self {
        self.cost_tracker = Some(tracker);
        self
    }

    // ── Middleware ─────────────────────────────────────────

    /// Add a middleware to the pipeline.
    pub fn middleware(mut self, mw: impl Middleware + 'static) -> Self {
        self.middlewares.push(Arc::new(mw));
        self
    }

    /// Add a rate limit middleware (convenience shortcut).
    pub fn with_rate_limit(self, max_per_minute: usize) -> Self {
        self.middleware(crate::middleware::RateLimitMiddleware::new(max_per_minute))
    }

    /// Add a token budget middleware (convenience shortcut).
    pub fn with_token_budget(self, max_tokens: usize) -> Self {
        self.middleware(crate::middleware::TokenBudgetMiddleware::new(max_tokens))
    }

    /// Add a logging middleware (convenience shortcut).
    pub fn with_logging(self) -> Self {
        self.middleware(crate::middleware::LoggingMiddleware::new(
            tracing::Level::INFO,
        ))
    }

    /// Build the agent.
    ///
    /// Uses the Oxicode engine's `ProviderResolver` for isolated provider/model
    /// lookups, so `switch_model()` and compaction stay within the engine's
    /// registry — no global state pollution.
    pub fn build(mut self) -> anyhow::Result<Agent> {
        // 1. Resolve model from Oxicode's instance registry
        let model = self.oxicode.resolve_model(&self.config.model_id)?;

        // 2. Create provider via Oxicode's engine (custom → built-in fallback)
        let provider: Arc<dyn oxicode_ai::Provider> =
            self.oxicode.create_provider(&model.provider)?;

        // 3. Merge workspace_dir into config
        let mut config = self.config.clone();
        config.workspace_dir = self.workspace_dir.or(config.workspace_dir);
        if let Some(ref prompt) = self.system_prompt {
            config.system_prompt = Some(prompt.clone());
        }

        // 4. Use Oxicode directly as the resolver. Oxicode implements ProviderResolver
        //    with catalog→static model resolution (builder.rs:106-123) and
        //    credential-aware provider creation (builder.rs:131-148).
        //
        //    The previous hand-rolled OxicodeResolver/OxicodeCore closure only
        //    consulted the static ModelRegistry via lookup(), silently
        //    dropping the catalog port — so catalog-known models (e.g.
        //    newer Z.AI models from models.dev) resolved at build() time
        //    via Oxicode::resolve_model but failed inside the agent loop's
        //    own resolve_model(), causing "Failed to resolve model" at
        //    stream time.
        let resolver: Arc<dyn ProviderResolver> = Arc::new(self.oxicode.clone());

        // 4b. Capability gate: drop the `lsp` tool when no `LspProvider`
        //     is configured on the agent config. This avoids the
        //     "LSP not configured" runtime error path entirely — the
        //     tool simply isn't visible to the model when LSP is off.
        //     See docs/designs/2026-07-18-stub-completion.md §4.3.
        if config.lsp.is_none() {
            self.tools.unregister("lsp");
        }

        // 5. Create agent with the isolated resolver
        let agent = Agent::new_with_resolver(provider, config, Arc::new(self.tools), resolver);

        // 6. Authorizer: grant capabilities.
        //
        // The authorizer middleware (`AuthorizerMiddleware`) checks
        // `Capability::ToolUse { tool_name }` against the granted
        // capabilities — type-specific, no cross-variant
        // implication. Without a `ToolUse` grant, every tool
        // call would be denied by the middleware regardless of
        // whether the agent has fine-grained FileRead/Bash caps.
        //
        // Coarse-grant fallback: when the granted capability set
        // contains no `ToolUse` variant, auto-add a wildcard
        // `ToolUse { tool_name: "*" }`. This makes the SDK's
        // authorizer integration usable out of the box with
        // `CapabilitySet::coding()` / `read_only()` / `research()` /
        // `browser()` (none of which contain `ToolUse`).
        //
        // Fine-grained enforcement (command/path restrictions)
        // would require tool-specific arg parsing inside the
        // middleware to derive `Bash`/`FileRead` capabilities
        // from the call's JSON args. That's a follow-up; see
        // design doc at
        // docs/designs/2026-06-30-observability-wiring.md.
        if let Some(authorizer) = &self.authorizer {
            let agent_id = resolved_agent_id(&agent);
            if let Some(mut caps) = self.capabilities.clone() {
                let has_tool_use = caps
                    .capabilities()
                    .iter()
                    .any(|c| matches!(c, crate::security::Capability::ToolUse { .. }));
                if !has_tool_use {
                    caps.add(crate::security::Capability::ToolUse {
                        tool_name: "*".into(),
                    });
                }
                let subject = crate::security::CapabilitySubject::Agent(agent_id);
                authorizer.grant(subject, caps);
            }
        }

        // 7. Build a single unified middleware pipeline that includes
        //    user middlewares, the audit-log adapter, and the
        //    authorizer adapter. Order matters: audit fires FIRST
        //    (records all attempts), authorizer fires SECOND (denies
        //    if needed — short-circuits before user mws run), user
        //    middlewares fire LAST.
        //
        //    The pipeline is wrapped into AgentHooks via
        //    `build_hooks` once, so `set_hooks()` is called exactly
        //    once. This avoids the replace-semantics bug class
        //    documented in docs/audits/2026-06-30-sdk-coverage.md
        //    Gap-0 ("observability silently overwritten when composes
        //    with user middlewares"). HookMiddleware slots and
        //    session-level closures (via with_session_hooks) are composed
        //    into the SAME AgentHooks instance — set_hooks remains the
        //    single call site.
        let has_observability_mws = self.audit_log.is_some() || self.authorizer.is_some();
        let has_user_mws = !self.middlewares.is_empty();
        let has_hooks = self.hooks_middleware.is_some();
        let has_session_hooks = self.session_hooks.is_some();
        if has_user_mws || has_observability_mws || has_hooks || has_session_hooks {
            let agent_id = resolved_agent_id(&agent);
            let mut pipeline = MiddlewarePipeline::new();

            // Audit fires first so every attempt (allowed or denied) is logged.
            if let Some(audit) = &self.audit_log {
                pipeline = pipeline.add_arc(Arc::new(
                    crate::middleware::observability_adapters::AuditLogMiddleware::new(
                        Arc::clone(audit),
                        agent_id.clone(),
                    ),
                ));
            }

            // Authorizer fires second — its denial short-circuits the
            // pipeline via `MiddlewareAction::Block`, which the
            // existing bridge maps to `BeforeToolCallResult { block: true }`.
            if let Some(authorizer) = &self.authorizer {
                let mut mw = crate::middleware::observability_adapters::AuthorizerMiddleware::new(
                    Arc::clone(authorizer),
                    agent_id.clone(),
                );
                if let Some(audit) = &self.audit_log {
                    mw = mw.with_audit(Arc::clone(audit));
                }
                pipeline = pipeline.add_arc(Arc::new(mw));
            }

            // HookMiddleware fires AFTER authorizer (so authorizer denials
            // still short-circuit) and BEFORE user middlewares (so user
            // middlewares observe hook-driven blocks).
            if let Some(hooks_mw) = self.hooks_middleware.take() {
                pipeline = pipeline.add_arc(Arc::new(hooks_mw));
            }

            // User middlewares fire last so audit/auth observe their
            // calls and Authorizer denials short-circuit before them.
            for mw in self.middlewares.into_iter() {
                pipeline = pipeline.add_arc(mw);
            }

            let pipeline = Arc::new(pipeline);
            let terminate_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut hooks = crate::middleware::build_hooks(pipeline, agent_id, terminate_flag);

            // Session-level closures — overwrite the three slots the cli
            // owns (should_stop_after_turn, steering, follow_up) on the
            // SAME AgentHooks the pipeline just produced. before_tool_call
            // and after_tool_call are preserved. This keeps set_hooks as a
            // single call site and avoids the replace-semantics bug class
            // (audit Gap-0).
            if let Some(session) = self.session_hooks.take() {
                hooks.should_stop_after_turn = Some(session.should_stop_after_turn);
                hooks.get_steering_messages = Some(session.get_steering_messages);
                hooks.get_follow_up_messages = Some(session.get_follow_up_messages);
                hooks.tool_execution = session.tool_execution;
            }

            // SINGLE set_hooks call for the entire agent.
            agent.set_hooks(hooks);
        }

        // 8. Tracer and CostTracker → event-tap path (accumulate, not replace).
        if self.tracer.is_some() || self.cost_tracker.is_some() {
            install_observability_dispatch(&agent, self.tracer.clone(), self.cost_tracker.clone());
        }

        Ok(agent)
    }
}

/// Synthesize a stable agent id used as the principal in capability
/// grants, audit-log entries, and observability dispatch. Matches the
/// existing behavior at agent_builder.rs:443-447 (synthesize a UUID
/// only when the config name is empty).
fn resolved_agent_id(agent: &Agent) -> String {
    let cfg = agent.get_config();
    if cfg.name.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        cfg.name
    }
}

/// Build the event-tap closure that records short lifecycle spans and drives
/// `CostTracker` from the agent's emitted events.
fn install_observability_dispatch(
    agent: &Agent,
    tracer: Option<Arc<Tracer>>,
    cost_tracker: Option<Arc<crate::observability::CostTracker>>,
) {
    use crate::observability::{SpanKind, TokenUsage};
    use oxicode_agent::AgentEvent;
    if tracer.is_none() && cost_tracker.is_none() {
        return;
    }
    // Use the same resolved agent_id as the middleware path so
    // AuditLog / Authorizer / CostTracker observations all key
    // by the same principal. Without this, a user-supplied
    // AgentConfig with `name: ""` would create a divergence:
    // `resolved_agent_id` falls back to a UUID for the
    // middleware grants, but `agent.get_config().name`
    // is the empty string — CostTracker would record under
    // `""` while Authorizer grants under the UUID.
    let agent_id = resolved_agent_id(agent);
    let resolver = agent.resolver().clone();
    let model_id = agent.get_config().model_id;
    agent.add_observability_dispatch(move |event: AgentEvent| match event {
        AgentEvent::AgentStart {
            prompts,
            session_id,
        } => {
            if let Some(tracer) = &tracer {
                let mut span = tracer.start("run", SpanKind::Agent);
                span.set_attribute("agent.id", serde_json::json!(agent_id));
                span.set_attribute("model.id", serde_json::json!(model_id));
                span.set_attribute("prompt.count", serde_json::json!(prompts.len()));
                if let Some(session_id) = session_id {
                    span.set_attribute("session.id", serde_json::json!(session_id));
                }
            }
        }
        AgentEvent::TurnStart { turn_number } => {
            if let Some(tracer) = &tracer {
                let mut span = tracer.start("turn_start", SpanKind::Agent);
                span.set_attribute("turn.number", serde_json::json!(turn_number));
            }
        }
        AgentEvent::TurnEnd {
            turn_number,
            tool_results,
            ..
        } => {
            if let Some(tracer) = &tracer {
                let mut span = tracer.start("turn_end", SpanKind::Agent);
                span.set_attribute("turn.number", serde_json::json!(turn_number));
                span.set_attribute("tool.result.count", serde_json::json!(tool_results.len()));
            }
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            ..
        } => {
            if let Some(tracer) = &tracer {
                let mut span = tracer.start("tool_start", SpanKind::Tool);
                span.set_attribute("tool.call.id", serde_json::json!(tool_call_id));
                span.set_attribute("tool.name", serde_json::json!(tool_name));
            }
        }
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            is_error,
            ..
        } => {
            if let Some(tracer) = &tracer {
                let mut span = tracer.start("tool_end", SpanKind::Tool);
                span.set_attribute("tool.call.id", serde_json::json!(tool_call_id));
                span.set_attribute("tool.name", serde_json::json!(tool_name));
                span.set_attribute("error", serde_json::json!(is_error));
                if is_error {
                    span.set_error("tool execution failed");
                }
            }
        }
        AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } => {
            let Some(cost_tracker) = &cost_tracker else {
                return;
            };
            if let Some(model) = resolver.resolve_model(&model_id) {
                cost_tracker.record(
                    &agent_id,
                    &model,
                    TokenUsage {
                        input: input_tokens as u64,
                        output: output_tokens as u64,
                        cache_read: 0,
                        cache_write: 0,
                    },
                );
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::catalog::{
        CatalogEvent, CatalogModelEntry, CatalogProtocol, CatalogSource, ModelCatalog,
    };
    use crate::{OxicodeBuilder, SdkResult};
    use std::future::Future;
    use std::pin::Pin;
    use tokio::sync::broadcast;

    /// Minimal catalog with a single model that exists ONLY in the catalog
    /// port — not in the static ModelRegistry. This reproduces the desync
    /// where `Oxicode::resolve_model()` (catalog→static) finds the model but
    /// the old `OxicodeResolver`'s `lookup()` (static-only) did not.
    struct SingleModelCatalog {
        entry: CatalogModelEntry,
        tx: broadcast::Sender<CatalogEvent>,
    }

    impl std::fmt::Debug for SingleModelCatalog {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SingleModelCatalog").finish_non_exhaustive()
        }
    }

    impl ModelCatalog for SingleModelCatalog {
        fn list_providers(
            &self,
        ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<String>>> + Send + '_>> {
            let p = self.entry.provider.clone();
            Box::pin(async move { Ok(vec![p]) })
        }
        fn get_provider(
            &self,
            _: &str,
        ) -> Pin<
            Box<
                dyn Future<Output = SdkResult<Option<crate::ports::catalog::CatalogProviderEntry>>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(None) })
        }
        fn list_models(
            &self,
            _: &str,
        ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>> {
            let e = self.entry.clone();
            Box::pin(async move { Ok(vec![e]) })
        }
        fn get_model(
            &self,
            provider: &str,
            model_id: &str,
        ) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogModelEntry>>> + Send + '_>>
        {
            let hit = self.get_model_sync(provider, model_id);
            Box::pin(async move { Ok(hit) })
        }
        fn search(
            &self,
            _: &str,
        ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>> {
            let e = self.entry.clone();
            Box::pin(async move { Ok(vec![e]) })
        }
        fn model_count(&self) -> Pin<Box<dyn Future<Output = SdkResult<usize>> + Send + '_>> {
            Box::pin(async { Ok(1) })
        }
        fn refresh(
            &self,
        ) -> Pin<
            Box<dyn Future<Output = SdkResult<crate::ports::catalog::RefreshOutcome>> + Send + '_>,
        > {
            Box::pin(async { Ok(crate::ports::catalog::RefreshOutcome::Unchanged) })
        }
        fn subscribe(&self) -> broadcast::Receiver<CatalogEvent> {
            self.tx.subscribe()
        }

        // ── Sync overrides ──
        fn get_model_sync(&self, provider: &str, model_id: &str) -> Option<CatalogModelEntry> {
            if provider == self.entry.provider && model_id == self.entry.model_id {
                Some(self.entry.clone())
            } else {
                None
            }
        }
    }

    /// Regression: AgentBuilder must wire the catalog-aware `Oxicode` as the
    /// agent loop's resolver, not a static-only closure.
    ///
    /// Pre-fix, the resolver consulted only the static `ModelRegistry`
    /// via `lookup()`, silently dropping the catalog port. Catalog-known
    /// models (e.g. newer Z.AI models from models.dev) resolved at
    /// `build()` time via `Oxicode::resolve_model` but failed inside the
    /// agent loop's `resolve_model()` → "Failed to resolve model".
    #[test]
    fn agent_builder_resolver_consults_catalog() {
        const MODEL_ID: &str = "anthropic/test-catalog-only-model";

        let catalog = Arc::new(SingleModelCatalog {
            entry: CatalogModelEntry {
                provider: "anthropic".into(),
                model_id: "test-catalog-only-model".into(),
                name: "Test Catalog-Only Model".into(),
                protocol: CatalogProtocol::AnthropicMessages,
                source: CatalogSource::Embedded,
                base_url: None,
                reasoning: false,
                supports_vision: false,
                cost_input: 0.0,
                cost_output: 0.0,
                cost_cache_read: 0.0,
                cost_cache_write: 0.0,
                context_window: 200_000,
                max_tokens: 8_192,
                input_modalities: vec!["text".into()],
                release_date: None,
                status: Some("ga".into()),
            },
            tx: broadcast::channel(16).0,
        });

        let oxicode = OxicodeBuilder::new()
            .with_builtins()
            .with_catalog(catalog)
            .build();

        // Sanity: model resolves via Oxicode (catalog→static).
        assert!(oxicode.resolve_model(MODEL_ID).is_ok());

        // Sanity: model is NOT in the static registry (proves the desync).
        assert!(
            oxicode
                .models_arc()
                .lookup("anthropic", "test-catalog-only-model")
                .is_none()
        );

        // Build an agent with the catalog-only model.
        let config = AgentConfig {
            model_id: MODEL_ID.to_string(),
            ..Default::default()
        };
        let agent = oxicode.agent(config).build().unwrap();

        // THE REGRESSION: the agent's loop resolver must also find the
        // catalog-only model.
        //   Pre-fix (OxicodeResolver): lookup() → None.
        //   Post-fix (Oxicode clone): resolve_model() → catalog hit → Some.
        assert!(
            agent.resolver().resolve_model(MODEL_ID).is_some(),
            "AgentBuilder's resolver must consult the catalog port, \
             not just the static registry"
        );
    }
}
