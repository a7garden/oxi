/// Core agent implementation
use crate::config::AgentConfig;
use crate::config::ShouldStopAfterTurnContext;
use crate::events::AgentEvent;
use crate::state::{AgentState, SharedState};
use crate::tools::{AgentTool, ToolRegistry};
use crate::types::{Response, StopReason};
use anyhow::{Error, Result};
use oxi_ai::{
    transform_for_provider, CompactionManager, CompactionStrategy, LlmCompactor, Model, Provider,
};
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ── ProviderResolver trait ────────────────────────────────────────

/// Trait for resolving providers and models within an Agent.
///
/// This abstracts away global static registries, allowing SDK users
/// to provide isolated provider/model lookups.
///
/// When using the SDK (`oxi-sdk`), the `Oxi` engine implements this trait.
/// When using `Agent::new()` directly, a global fallback is used.
pub trait ProviderResolver: Send + Sync + 'static {
    /// Resolve a provider by name, returning an Arc handle.
    fn resolve_provider(&self, name: &str) -> Option<Arc<dyn Provider>>;

    /// Resolve a model ID ("provider/model" or bare "model") to a Model.
    fn resolve_model(&self, model_id: &str) -> Option<Model>;
}

/// Global provider resolver — uses `oxi_ai` global functions.
///
/// This is the default resolver when using `Agent::new()`, preserving
/// backward compatibility with existing CLI usage.
pub(crate) struct GlobalProviderResolver;

impl ProviderResolver for GlobalProviderResolver {
    fn resolve_provider(&self, name: &str) -> Option<Arc<dyn Provider>> {
        oxi_ai::get_provider(name).map(Arc::from)
    }

    fn resolve_model(&self, model_id: &str) -> Option<Model> {
        crate::model_id::resolve_model_from_id(model_id)
    }
}

// ── AgentInner ────────────────────────────────────────────────────

/// Mutable agent internals protected by a read-write lock.
struct AgentInner {
    config: AgentConfig,
    provider: Arc<dyn Provider>,
}

impl Clone for AgentInner {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            provider: Arc::clone(&self.provider),
        }
    }
}

/// Agent runtime.
///
/// Manages provider, tool registry, state, and compaction, providing an
/// agentic loop for prompt execution, model switching, tool calls, and fallback.
///
/// Supports session continuation via [`continue_with`] and tokio-native
/// event streaming via [`run_tokio_stream`].
///
/// [`continue_with`]: Agent::continue_with
/// [`run_tokio_stream`]: Agent::run_tokio_stream
pub struct Agent {
    inner: RwLock<AgentInner>,
    tools: Arc<ToolRegistry>,
    state: SharedState,
    compaction_manager: CompactionManager,
    hooks: parking_lot::RwLock<crate::config::AgentHooks>,
    /// Guard: true while a run is in progress. Prevents concurrent runs.
    is_running: AtomicBool,
    /// Provider/model resolver. Uses global functions by default,
    /// or a custom resolver when created via `new_with_resolver()`.
    resolver: Arc<dyn ProviderResolver>,
    /// Shared cancellation flag. Set by `cancel()` (e.g. on Ctrl+C),
    /// propagated to AgentLoop's `external_stop` during each run.
    cancel_flag: Arc<AtomicBool>,
}

impl Agent {
    /// Create a new agent with the given provider, config, and tool registry.
    ///
    /// Uses the global `oxi_ai::get_provider()` / `resolve_model_from_id()`
    /// for model switching. For isolated instances, use [`new_with_resolver`].
    ///
    /// [`new_with_resolver`]: Agent::new_with_resolver
    pub fn new(provider: Arc<dyn Provider>, config: AgentConfig, tools: Arc<ToolRegistry>) -> Self {
        let resolver = Arc::new(GlobalProviderResolver);
        Self::build_inner(provider, config, tools, resolver)
    }

    /// Create an agent with a custom provider/model resolver.
    ///
    /// This is the preferred constructor for SDK usage where provider
    /// and model registries must be isolated from global state.
    pub fn new_with_resolver(
        provider: Arc<dyn Provider>,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        resolver: Arc<dyn ProviderResolver>,
    ) -> Self {
        Self::build_inner(provider, config, tools, resolver)
    }

    /// Internal constructor shared by `new()` and `new_with_resolver()`.
    fn build_inner(
        provider: Arc<dyn Provider>,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        resolver: Arc<dyn ProviderResolver>,
    ) -> Self {
        let mut compaction_manager =
            CompactionManager::new(config.compaction_strategy.clone(), config.context_window);

        // Pre-initialize the LLM compactor if compaction is enabled
        if config.compaction_strategy != CompactionStrategy::Disabled {
            let model = resolver.resolve_model(&config.model_id);

            if let Some(model) = model {
                let llm_compactor =
                    Arc::new(LlmCompactor::new(model.clone(), Arc::clone(&provider)));
                compaction_manager.set_compactor(llm_compactor);
            }
        }

        Self {
            inner: RwLock::new(AgentInner { config, provider }),
            tools,
            state: SharedState::new(),
            compaction_manager,
            hooks: parking_lot::RwLock::new(crate::config::AgentHooks::default()),
            is_running: AtomicBool::new(false),
            resolver,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create an agent with an empty tool registry.
    pub fn new_empty(provider: Arc<dyn Provider>, config: AgentConfig) -> Self {
        Self::new(provider, config, Arc::new(ToolRegistry::new()))
    }

    /// Get the agent configuration (read guard)
    fn config(&self) -> parking_lot::RwLockReadGuard<'_, AgentInner> {
        self.inner.read()
    }

    /// Get a write guard for the agent inner state
    fn inner_mut(&self) -> parking_lot::RwLockWriteGuard<'_, AgentInner> {
        self.inner.write()
    }

    /// Get the current model ID
    pub fn model_id(&self) -> String {
        self.config().config.model_id.clone()
    }

    /// Switch the model used for future LLM calls.
    ///
    /// If the new model uses a different provider API, the conversation
    /// history is automatically transformed for cross-provider compatibility
    /// (e.g. thinking blocks are converted to `<thinking>` tags).
    ///
    /// # Arguments
    /// * `model_id` - New model ID in `provider/model` format
    ///
    /// # Returns
    /// `Ok(())` on success, or an error if the model/provider is unknown
    pub fn switch_model(&self, model_id: &str) -> Result<()> {
        let new_model = self
            .resolver
            .resolve_model(model_id)
            .ok_or_else(|| Error::msg(format!("Model '{}' not found", model_id)))?;

        // Create the new provider via resolver
        let new_provider = self
            .resolver
            .resolve_provider(&new_model.provider)
            .ok_or_else(|| Error::msg(format!("Provider '{}' not found", new_model.provider)))?;

        // Detect API change and transform messages if needed
        {
            let inner = self.config();
            let old_model_id = &inner.config.model_id;
            let old_api = self
                .resolver
                .resolve_model(old_model_id)
                .map(|m| m.api)
                .unwrap_or(oxi_ai::Api::AnthropicMessages);

            if old_api != new_model.api {
                // Transform existing messages for the new provider
                let messages = self.state.get_state().messages.clone();
                let transformed = transform_for_provider(&messages, &old_api, &new_model.api);
                self.state.update(|s| {
                    s.replace_messages(transformed);
                });
            }
        }

        // Update config and provider atomically
        let mut inner = self.inner_mut();
        inner.config.model_id = model_id.to_string();
        inner.provider = new_provider;

        Ok(())
    }

    /// Switch the model using a pre-resolved `Model` object.
    ///
    /// This is useful when the caller has already looked up the model
    /// and optionally created the provider.
    pub fn switch_to_model(&self, model: &oxi_ai::Model) -> Result<()> {
        let model_id = format!("{}/{}", model.provider, model.id);
        let new_provider = self
            .resolver
            .resolve_provider(&model.provider)
            .ok_or_else(|| Error::msg(format!("Provider '{}' not found", model.provider)))?;

        // Detect API change and transform messages if needed
        {
            let inner = self.config();
            let old_api = self
                .resolver
                .resolve_model(&inner.config.model_id)
                .map(|m| m.api)
                .unwrap_or(oxi_ai::Api::AnthropicMessages);

            if old_api != model.api {
                let messages = self.state.get_state().messages.clone();
                let transformed = transform_for_provider(&messages, &old_api, &model.api);
                self.state.update(|s| {
                    s.replace_messages(transformed);
                });
            }
        }

        let mut inner = self.inner_mut();
        inner.config.model_id = model_id;
        inner.provider = new_provider;

        Ok(())
    }

    /// Get a handle to the tool registry.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
    }

    /// Get a snapshot of the current agent state.
    pub fn state(&self) -> AgentState {
        self.state.get_state()
    }

    /// Reset agent state for a new conversation
    pub fn reset(&self) {
        self.state.reset();
    }

    /// Register a tool that the agent can invoke during a run.
    pub fn add_tool<T: AgentTool + 'static>(&self, tool: T) {
        self.tools.register(tool);
    }

    /// Update the system prompt for future interactions.
    pub fn set_system_prompt(&self, prompt: String) {
        self.inner_mut().config.system_prompt = Some(prompt);
    }

    /// Get the compaction manager
    pub fn compaction_manager(&self) -> &CompactionManager {
        &self.compaction_manager
    }

    /// Run the agent with a prompt, collecting all events into a vector.
    ///
    /// Convenience wrapper around [`run_with_channel`] that gathers every
    /// [`AgentEvent`] produced during the run.
    pub async fn run(&self, prompt: String) -> Result<(Response, Vec<AgentEvent>)> {
        let mut events = Vec::new();
        let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
        let result = self.run_with_channel(prompt, tx).await;
        while let Ok(event) = rx.recv() {
            events.push(event);
        }
        result.map(|r| (r, events))
    }

    /// Run the agent, delivering events through the provided channel.
    ///
    /// Delegates to [`AgentLoop`] which implements the same 2-level agentic
    /// loop matching pi-mono's architecture:
    ///
    /// ```text
    /// AgentLoop.run_messages()
    ///   Outer loop (follow-up messages):
    ///     Inner loop (tool calls + steering):
    ///       1. Inject pending messages (steering)
    ///       2. Compaction check
    ///       3. Stream LLM response (with accumulated partial messages)
    ///       4. Execute tool calls if any
    ///       5. Emit turn_end
    ///       6. Check shouldStopAfterTurn
    ///       7. Poll steering messages
    ///     Check follow-up messages
    ///     Exit
    /// ```
    pub async fn run_with_channel(
        &self,
        prompt: String,
        tx: std::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        // pi-mono: Agent.prompt() throws if activeRun exists.
        // Prevent concurrent runs that would corrupt shared state.
        if self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(Error::msg("Agent is already running"));
        }

        // Drop guard ensures is_running is cleared even on panic.
        struct RunningGuard<'a>(&'a AtomicBool);
        impl Drop for RunningGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _guard = RunningGuard(&self.is_running);
        self.reset_cancel();

        self.run_with_channel_inner(prompt, tx).await
    }

    /// Inner implementation of run_with_channel, called after the running guard is set.
    async fn run_with_channel_inner(
        &self,
        prompt: String,
        tx: std::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        use crate::agent_loop::AgentLoop;

        let (
            provider,
            max_iterations,
            system_prompt,
            temperature,
            max_tokens,
            compaction_strategy,
            context_window,
            api_key,
            workspace_dir,
        ) = {
            let inner = self.inner.read();
            (
                Arc::clone(&inner.provider) as Arc<dyn Provider>,
                inner.config.max_iterations,
                inner.config.system_prompt.clone(),
                inner.config.temperature,
                inner.config.max_tokens,
                inner.config.compaction_strategy.clone(),
                inner.config.context_window,
                inner.config.api_key.clone(),
                inner.config.workspace_dir.clone(),
            )
        }; // release read lock

        // Build AgentLoopConfig from Agent's config
        let loop_config = crate::agent_loop::config::AgentLoopConfig {
            model_id: self.model_id(),
            system_prompt,
            max_iterations,
            temperature: temperature.unwrap_or(1.0) as f32,
            max_tokens: max_tokens.unwrap_or(4096) as u32,
            tool_execution: crate::config::ToolExecutionMode::Sequential,
            compaction_strategy,
            compaction_instruction: None,
            context_window,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
            auto_retry_enabled: true,
            auto_retry_max_attempts: 3,
            auto_retry_base_delay_ms: 1000,
            api_key,
            workspace_dir,
        };

        // Create AgentLoop. We give it a NEW SharedState and sync back after.
        // (SharedState is not Clone, so we create a fresh one from current state)
        let fresh_state = crate::state::SharedState::new();
        let current = self.state.get_state();
        fresh_state.update(|s| {
            *s = current;
        });

        let mut agent_loop = AgentLoop::new_with_resolver(
            provider,
            loop_config,
            Arc::clone(&self.tools),
            fresh_state,
            Arc::clone(&self.resolver),
        );

        // Pre-populate steering/follow-up from hooks
        {
            let hooks = self.hooks.read();
            if let Some(ref get_steering) = hooks.get_steering_messages {
                for msg_text in get_steering() {
                    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new(msg_text)));
                }
            }
            if let Some(ref get_follow_up) = hooks.get_follow_up_messages {
                for msg_text in get_follow_up() {
                    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(msg_text)));
                }
            }

            // Store hooks on AgentLoop so they can be polled each turn
            // to pick up new messages injected during the run.
            if let Some(ref get_steering) = hooks.get_steering_messages {
                agent_loop.set_steering_hook(Arc::clone(get_steering));
            }
            if let Some(ref get_follow_up) = hooks.get_follow_up_messages {
                agent_loop.set_follow_up_hook(Arc::clone(get_follow_up));
            }
        }
        let mut al = agent_loop;

        // Wire should_stop_after_turn hook: share AgentLoop's external_stop
        // Arc with the emit callback. When the hook fires (Ctrl+C detected),
        // it sets ext_stop. AgentLoop checks this in should_stop_after_turn()
        // AND during streaming (streaming.rs checks external_stop each event).
        //
        // Arc<dyn Fn> can be cloned, so we read it without consuming.
        let maybe_hook = {
            let hooks_r = self.hooks.read();
            hooks_r.should_stop_after_turn.clone()
        };
        let ext_stop = al.external_stop().clone();
        let cancel_flag = self.cancel_flag.clone();

        // Share cancel_flag with AgentLoop so the streaming loop can check
        // it directly in the periodic timer — no emit callback required.
        // This closes the gap where cancel() was ineffective when the
        // provider stream produced no events.
        al.set_cancel_signal(self.cancel_flag.clone());

        // Create emit callback that sends through the channel.
        // AgentLoop calls this synchronously. UnboundedSender::send() is
        // non-blocking and never drops events (unlike try_send on bounded).
        let tx_emit = tx.clone();

        // Run the agent loop
        tracing::info!("[AGENT] Starting agent run with channel");
        let result = al
            .run(prompt.clone(), move |event: AgentEvent| {
                // Forward event to channel (std::sync::mpsc — send from sync context)
                tracing::info!("[AGENT-EMIT] Event: {:?}", std::mem::discriminant(&event));
                if let Err(e) = tx_emit.send(event.clone()) {
                    tracing::error!(
                        "[AGENT-EMIT] Failed to send agent event to channel: {:?}",
                        e
                    );
                } else {
                    tracing::info!("[AGENT-EMIT] Successfully sent event");
                }

                // Propagate cancellation from Agent::cancel() → external_stop.
                // This runs on every event, ensuring the streaming loop detects
                // cancellation promptly.
                if cancel_flag.load(Ordering::SeqCst) {
                    ext_stop.store(true, Ordering::SeqCst);
                }

                // Propagate should_stop → external_stop on every event, not
                // just TurnEnd. The TUI hook only checks should_stop_flag.load(),
                // so the context contents are irrelevant for non-TurnEnd events.
                // This ensures streaming.rs detects cancellation immediately
                // when the user presses Ctrl+C mid-stream.
                if let Some(ref hook) = maybe_hook {
                    let ctx = ShouldStopAfterTurnContext {
                        message: match &event {
                            AgentEvent::TurnEnd {
                                assistant_message: oxi_ai::Message::Assistant(a),
                                ..
                            } => a.clone(),
                            _ => oxi_ai::AssistantMessage::new(
                                oxi_ai::Api::OpenAiCompletions,
                                "agent",
                                "agent-model",
                            ),
                        },
                        tool_results: match &event {
                            AgentEvent::TurnEnd {
                                ref tool_results, ..
                            } => tool_results.clone(),
                            _ => Vec::new(),
                        },
                        iteration: 0,
                    };
                    if hook(&ctx) {
                        ext_stop.store(true, Ordering::SeqCst);
                    }
                }
            })
            .await;

        match result {
            Ok(_events) => {
                // Sync state back from AgentLoop
                let loop_state = al.state().get_state();
                self.state.update(|s| {
                    *s = loop_state;
                });

                // Extract final response text from state
                let state = self.state.get_state();
                let final_text = state
                    .messages
                    .iter()
                    .rev()
                    .find_map(|m| match m {
                        oxi_ai::Message::Assistant(a) => a.content.iter().find_map(|b| match b {
                            oxi_ai::ContentBlock::Text(t) => Some(t.text.clone()),
                            _ => None,
                        }),
                        _ => None,
                    })
                    .unwrap_or_default();

                let stop_reason = state.stop_reason.unwrap_or(StopReason::Stop);

                Ok(Response {
                    content: final_text,
                    stop_reason,
                })
            }
            Err(e) => Err(e),
        }
    }

    // ── Helper methods for the agentic loop ────────────────────────

    /// Set hooks for the agent loop.
    pub fn set_hooks(&self, hooks: crate::config::AgentHooks) {
        let mut h = self.hooks.write();
        *h = hooks;
    }

    /// Request cancellation of the current agent run.
    ///
    /// Sets a shared `cancel_flag` that is propagated to the `AgentLoop`'s
    /// `external_stop` on every event AND polled every ~500ms by the
    /// streaming loop's periodic check. This ensures cancellation is
    /// detected quickly even when the provider stream is completely hung
    /// (no events arriving).
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// Reset the cancellation flag before starting a new run.
    pub fn reset_cancel(&self) {
        self.cancel_flag.store(false, Ordering::SeqCst);
    }

    /// Run the agent, invoking `on_event` for each [`AgentEvent`] produced.
    ///
    /// Blocking convenience wrapper suitable for callers that prefer a
    /// callback-based API over a channel.
    pub async fn run_streaming<F>(&self, prompt: String, mut on_event: F) -> Result<Response>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
        let result = self.run_with_channel(prompt, tx).await;
        while let Ok(event) = rx.recv() {
            on_event(event);
        }
        result
    }

    // ── Session persistence ────────────────────────────────────────

    /// Export the agent state as a JSON value.
    ///
    /// The serialized state includes conversation messages, token counts,
    /// iteration progress, and stop reason. Use [`import_state`] to restore.
    ///
    /// [`import_state`]: Agent::import_state
    pub fn export_state(&self) -> Result<serde_json::Value> {
        let state = self.state.get_state();
        serde_json::to_value(&state).map_err(|e| Error::msg(format!("State export failed: {}", e)))
    }

    /// Import agent state from a JSON value.
    ///
    /// Restores conversation history, token counts, and iteration progress.
    /// Typically used together with [`export_state`] for session persistence.
    ///
    /// [`export_state`]: Agent::export_state
    pub fn import_state(&self, value: serde_json::Value) -> Result<()> {
        let state: AgentState = serde_json::from_value(value)
            .map_err(|e| Error::msg(format!("State import failed: {}", e)))?;
        self.state.update(|s| *s = state);
        Ok(())
    }

    // ── Session continuation ───────────────────────────────────────

    /// Continue the current session with a new prompt.
    ///
    /// Unlike `run()`, which can be used on a fresh agent, `continue_with`
    /// preserves the existing conversation state and appends the new prompt.
    /// This enables multi-turn interactions within the same session.
    pub async fn continue_with(&self, prompt: String) -> Result<(Response, Vec<AgentEvent>)> {
        let mut events = Vec::new();
        let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
        let result = self.run_with_channel(prompt, tx).await;
        while let Ok(event) = rx.recv() {
            events.push(event);
        }
        result.map(|r| (r, events))
    }

    // ── Tokio-native streaming ─────────────────────────────────────

    /// Run the agent with tokio-native event streaming.
    ///
    /// Returns a `tokio::sync::mpsc::Receiver` for events and a
    /// `JoinHandle` for the response. This is the preferred API for
    /// async runtimes (WebSocket/SSE gateways, tokio-based servers).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (rx, handle) = agent.run_tokio_stream("Explain Rust".into()).await?;
    /// while let Some(event) = rx.recv().await {
    ///     println!("Event: {:?}", event.type_name());
    /// }
    /// let response = handle.await??;
    /// ```
    pub async fn run_tokio_stream(
        &self,
        prompt: String,
    ) -> Result<(
        tokio::sync::mpsc::Receiver<AgentEvent>,
        tokio::task::JoinHandle<Result<Response>>,
    )> {
        let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);

        if self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(Error::msg("Agent is already running"));
        }

        let should_stop_hook = self.hooks.read().should_stop_after_turn.clone();

        let state = self.state.clone();
        let inner = self.inner.read().clone();
        let tools = Arc::clone(&self.tools);
        let resolver = Arc::clone(&self.resolver);

        // Build AgentLoopConfig
        let loop_config = crate::agent_loop::config::AgentLoopConfig {
            model_id: inner.config.model_id.clone(),
            system_prompt: inner.config.system_prompt.clone(),
            max_iterations: inner.config.max_iterations,
            temperature: inner.config.temperature.unwrap_or(1.0) as f32,
            max_tokens: inner.config.max_tokens.unwrap_or(4096) as u32,
            tool_execution: crate::config::ToolExecutionMode::Sequential,
            compaction_strategy: inner.config.compaction_strategy.clone(),
            compaction_instruction: None,
            context_window: inner.config.context_window,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
            auto_retry_enabled: true,
            auto_retry_max_attempts: 3,
            auto_retry_base_delay_ms: 1000,
            api_key: inner.config.api_key.clone(),
            workspace_dir: inner.config.workspace_dir.clone(),
        };

        let provider: Arc<dyn Provider> = Arc::clone(&inner.provider);

        // Create fresh state from current
        let fresh_state = SharedState::new();
        let current = state.get_state();
        fresh_state.update(|s| *s = current);

        let agent_loop = crate::agent_loop::AgentLoop::new_with_resolver(
            provider,
            loop_config,
            tools,
            fresh_state,
            resolver,
        );

        let maybe_hook = should_stop_hook;
        let ext_stop = agent_loop.external_stop().clone();

        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_clone = Arc::clone(&is_running);

        let handle = tokio::task::spawn(async move {
            let result = agent_loop
                .run(prompt, move |event: AgentEvent| {
                    // Forward to tokio channel (non-blocking)
                    let _ = tx.try_send(event.clone());

                    // Propagate should_stop → external_stop on every event,
                    // not just TurnEnd. See run_with_channel_inner for rationale.
                    if let Some(ref hook) = maybe_hook {
                        let ctx = ShouldStopAfterTurnContext {
                            message: match &event {
                                AgentEvent::TurnEnd {
                                    assistant_message: oxi_ai::Message::Assistant(a),
                                    ..
                                } => a.clone(),
                                _ => oxi_ai::AssistantMessage::new(
                                    oxi_ai::Api::OpenAiCompletions,
                                    "agent",
                                    "agent-model",
                                ),
                            },
                            tool_results: match &event {
                                AgentEvent::TurnEnd {
                                    ref tool_results, ..
                                } => tool_results.clone(),
                                _ => Vec::new(),
                            },
                            iteration: 0,
                        };
                        if hook(&ctx) {
                            ext_stop.store(true, Ordering::SeqCst);
                        }
                    }
                })
                .await;

            // Clear the running flag
            is_running_clone.store(false, Ordering::SeqCst);

            match result {
                Ok(_events) => {
                    // State is shared via SharedState (Arc<RwLock>),
                    // so the caller can read it from agent.state() after completion.
                    Ok(Response {
                        content: String::new(),
                        stop_reason: StopReason::Stop,
                    })
                }
                Err(e) => Err(e),
            }
        });

        Ok((rx, handle))
    }
}
