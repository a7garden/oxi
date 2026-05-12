/// Core agent implementation

use crate::config::AgentConfig;
use crate::config::ShouldStopAfterTurnContext;
use crate::error::AgentError;
use crate::events::AgentEvent;
use crate::state::{AgentState, SharedState};
use crate::tools::{AgentTool, AgentToolResult, ToolRegistry};
use crate::types::{Response, StopReason};
use anyhow::{Error, Result};
use futures::StreamExt;
use oxi_ai::{
    progress_callback, transform_for_provider, CompactionManager, CompactionStrategy,
    ContentBlock, Context, LlmCompactor, Message, Provider, ProviderEvent, StreamOptions,
    TextContent,
};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

use crate::stream_retry::{self, RetryCallback};

/// Default fallback model used when the primary model fails.
const DEFAULT_FALLBACK_MODEL: &str = "openai/gpt-4o-mini";

/// [`RetryCallback`] that emits [`AgentEvent::Retry`] through an mpsc channel.
struct MpscRetryCallback {
    tx: mpsc::Sender<AgentEvent>,
}

impl RetryCallback for MpscRetryCallback {
    fn on_retry(&self, attempt: usize, max_retries: usize, delay_secs: u64, reason: String) {
        let tx = self.tx.clone();
        // Fire-and-forget: send from a spawned task so we don't need &self to be 'static.
        tokio::spawn(async move {
            let _ = tx
                .send(AgentEvent::Retry {
                    session_id: None,
                    attempt,
                    max_retries,
                    retry_after_secs: delay_secs,
                    reason,
                })
                .await;
        });
    }
}

/// Mutable agent internals protected by a read-write lock.
struct AgentInner {
    config: AgentConfig,
    provider: Arc<dyn Provider>,
}

/// Agent runtime.
///
/// Manages provider, tool registry, state, and compaction, providing an
/// agentic loop for prompt execution, model switching, tool calls, and fallback.
pub struct Agent {
    inner: RwLock<AgentInner>,
    tools: Arc<ToolRegistry>,
    state: SharedState,
    compaction_manager: CompactionManager,
    hooks: parking_lot::RwLock<crate::config::AgentHooks>,
    /// Guard: true while a run is in progress. Prevents concurrent runs.
    is_running: AtomicBool,
}

/// Result of executing a batch of tool calls.
struct ToolBatchResult {
    messages: Vec<oxi_ai::ToolResultMessage>,
    terminate: bool,
}

impl Agent {
    /// Create a new agent with the given provider and config
    pub fn new(provider: Arc<dyn Provider>, config: AgentConfig) -> Self {
        let mut compaction_manager =
            CompactionManager::new(config.compaction_strategy.clone(), config.context_window);

        // Pre-initialize the LLM compactor if compaction is enabled
        if config.compaction_strategy != CompactionStrategy::Disabled {
            let model = crate::model_id::resolve_model_from_id(&config.model_id);

            if let Some(model) = model {
                let llm_compactor =
                    Arc::new(LlmCompactor::new(model.clone(), Arc::clone(&provider)));
                compaction_manager.set_compactor(llm_compactor);
            }
        }

        Self {
            inner: RwLock::new(AgentInner { config, provider }),
            tools: Arc::new(ToolRegistry::new()),
            state: SharedState::new(),
            compaction_manager,
            hooks: parking_lot::RwLock::new(crate::config::AgentHooks::default()),
            is_running: AtomicBool::new(false),
        }
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
        let new_model = crate::model_id::resolve_model_from_id(model_id)
            .ok_or_else(|| Error::msg(format!("Model '{}' not found", model_id)))?;

        // Create the new provider
        let new_provider = oxi_ai::get_provider(&new_model.provider)
            .ok_or_else(|| Error::msg(format!("Provider '{}' not found", new_model.provider)))?;

        // Detect API change and transform messages if needed
        {
            let inner = self.config();
            let old_model_id = &inner.config.model_id;
            let old_api = crate::model_id::resolve_model_from_id(old_model_id)
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
        inner.provider = Arc::from(new_provider);

        Ok(())
    }

    /// Switch the model using a pre-resolved `Model` object.
    ///
    /// This is useful when the caller has already looked up the model
    /// and optionally created the provider.
    pub fn switch_to_model(&self, model: &oxi_ai::Model) -> Result<()> {
        let model_id = format!("{}/{}", model.provider, model.id);
        let new_provider = oxi_ai::get_provider(&model.provider)
            .ok_or_else(|| Error::msg(format!("Provider '{}' not found", model.provider)))?;

        // Detect API change and transform messages if needed
        {
            let inner = self.config();
            let old_api = crate::model_id::resolve_model_from_id(&inner.config.model_id)
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
        inner.provider = Arc::from(new_provider);

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
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(100);
        let result = self.run_with_channel(prompt, tx).await;
        while let Some(event) = rx.recv().await {
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
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        // pi-mono: Agent.prompt() throws if activeRun exists.
        // Prevent concurrent runs that would corrupt shared state.
        if self.is_running.compare_exchange(
            false, true,
            Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
            return Err(Error::msg("Agent is already running"));
        }

        let result = self.run_with_channel_inner(prompt, tx).await;

        // Always clear the running flag
        self.is_running.store(false, Ordering::SeqCst);
        result
    }

    /// Inner implementation of run_with_channel, called after the running guard is set.
    async fn run_with_channel_inner(
        &self,
        prompt: String,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        use crate::agent_loop::AgentLoop;

        let inner = self.inner.read();
        let provider: Arc<dyn Provider> = Arc::clone(&inner.provider);
        let max_iterations = inner.config.max_iterations;
        let system_prompt = inner.config.system_prompt.clone();
        let temperature = inner.config.temperature;
        let max_tokens = inner.config.max_tokens;
        let compaction_strategy = inner.config.compaction_strategy.clone();
        let context_window = inner.config.context_window;
        drop(inner); // release read lock

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
            auto_retry_enabled: false,
            auto_retry_max_attempts: 3,
            auto_retry_base_delay_ms: 1000,
            api_key: None,
        };

        // Create AgentLoop. We give it a NEW SharedState and sync back after.
        // (SharedState is not Clone, so we create a fresh one from current state)
        let mut fresh_state = crate::state::SharedState::new();
        let current = self.state.get_state();
        fresh_state.update(|s| {
            *s = current;
        });

        let agent_loop = AgentLoop::new(
            provider,
            loop_config,
            Arc::clone(&self.tools),
            fresh_state,
        );

        // Pre-populate steering/follow-up from hooks
        let hooks = self.hooks.read();
        let mut al = agent_loop;

        if let Some(ref get_steering) = hooks.get_steering_messages {
            for msg_text in get_steering() {
                al.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new(msg_text)));
            }
        }
        if let Some(ref get_follow_up) = hooks.get_follow_up_messages {
            for msg_text in get_follow_up() {
                al.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(msg_text)));
            }
        }

        // Wire should_stop_after_turn hook: share AgentLoop's external_stop
        // Arc with the emit callback. When the hook fires (Ctrl+C detected),
        // it sets ext_stop. AgentLoop checks this in should_stop_after_turn().
        //
        // Note: Box<dyn Fn> can't be cloned. We take it from hooks.
        let maybe_hook = {
            drop(hooks);
            let mut hooks_w = self.hooks.write();
            hooks_w.should_stop_after_turn.take()
        };
        let ext_stop = al.external_stop().clone();

        // Create emit callback that sends through the channel.
        // AgentLoop calls this synchronously. We use try_send (non-blocking)
        // because blocking_send panics inside a tokio runtime.
        // If the channel is full, the event is dropped — the channel should
        // be sized generously (256+) to avoid this.
        let tx_emit = tx.clone();

        // Run the agent loop
        let result = al.run(prompt.clone(), move |event: AgentEvent| {
            // Forward event to channel (non-blocking)
            let _ = tx_emit.try_send(event.clone());

            // On TurnEnd, poll the should_stop_after_turn hook to detect Ctrl+C.
            // The hook wraps an AtomicBool (should_stop_flag from AgentSession).
            // We can't pass real context here, but the TUI hook only checks
            // the AtomicBool anyway: |ctx| should_stop_flag.load(SeqCst).
            if let Some(ref hook) = maybe_hook {
                if let AgentEvent::TurnEnd { ref assistant_message, ref tool_results, .. } = event {
                    // Build real context from actual turn data
                    let asst = match assistant_message {
                        oxi_ai::Message::Assistant(a) => a.clone(),
                        _ => {
                            // Can't extract assistant message, just check the hook with empty ctx
                            let ctx = ShouldStopAfterTurnContext {
                                message: oxi_ai::AssistantMessage::new(
                                    oxi_ai::Api::OpenAiCompletions, "agent", "agent-model",
                                ),
                                tool_results: Vec::new(),
                                iteration: 0,
                            };
                            if hook(&ctx) {
                                ext_stop.store(true, Ordering::SeqCst);
                            }
                            return;
                        }
                    };
                    let ctx = ShouldStopAfterTurnContext {
                        message: asst,
                        tool_results: tool_results.clone(),
                        iteration: 0,
                    };
                    if hook(&ctx) {
                        ext_stop.store(true, Ordering::SeqCst);
                    }
                }
            }
        }).await;

        match result {
            Ok(_events) => {
                // Sync state back from AgentLoop
                let loop_state = al.state().get_state();
                self.state.update(|s| {
                    *s = loop_state;
                });

                // Extract final response text from state
                let state = self.state.get_state();
                let final_text = state.messages.iter().rev()
                    .find_map(|m| match m {
                        oxi_ai::Message::Assistant(a) => {
                            a.content.iter().find_map(|b| match b {
                                oxi_ai::ContentBlock::Text(t) => Some(t.text.clone()),
                                _ => None,
                            })
                        }
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

    /// Check and run compaction if needed.
    async fn run_compaction_check(&self, tx: &mpsc::Sender<AgentEvent>) {
        let state_msgs = self.state.get_state().messages.clone();
        let context_text = serde_json::to_string(&state_msgs).unwrap_or_default();
        let context_tokens = oxi_ai::estimate_tokens(&context_text);
        let iteration = self.state.get_state().iteration;

        if self.compaction_manager.should_compact(context_tokens, iteration) {
            let _ = tx.send(AgentEvent::Compaction {
                event: crate::compaction::CompactionEvent::Triggered {
                    context_tokens,
                    iteration,
                },
            }).await;

            match self.compaction_manager.compact_if_needed(
                &state_msgs,
                None,
                context_tokens,
                iteration,
            ).await {
                Ok(Some(compacted)) => {
                    let _ = tx.send(AgentEvent::Compaction {
                        event: crate::compaction::CompactionEvent::Started {
                            message_count: compacted.compacted_count,
                        },
                    }).await;
                    self.state.update(|s| {
                        s.messages = compacted.kept_messages.clone();
                    });
                    let _ = tx.send(AgentEvent::Compaction {
                        event: crate::compaction::CompactionEvent::Completed {
                            result: crate::compaction::CompactedContext::from(compacted),
                            duration_ms: 0,
                        },
                    }).await;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("Compaction failed: {}", e);
                }
            }
        }
    }

    /// Drain steering messages from hooks or session queue.
    fn drain_steering_messages(&self) -> Vec<String> {
        let hooks = self.hooks.read();
        if let Some(ref get_steering) = hooks.get_steering_messages {
            return get_steering();
        }
        Vec::new()
    }

    /// Drain follow-up messages from hooks or session queue.
    fn drain_follow_up_messages(&self) -> Vec<String> {
        let hooks = self.hooks.read();
        if let Some(ref get_follow_up) = hooks.get_follow_up_messages {
            return get_follow_up();
        }
        Vec::new()
    }

    /// Check shouldStopAfterTurn hook.
    fn should_stop_after_turn(&self) -> bool {
        let hooks = self.hooks.read();
        if let Some(ref hook) = hooks.should_stop_after_turn {
            let ctx = crate::config::ShouldStopAfterTurnContext {
                message: oxi_ai::AssistantMessage::new(
                    oxi_ai::Api::OpenAiCompletions, "agent", "agent-model",
                ),
                tool_results: Vec::new(),
                iteration: self.state.get_state().iteration,
            };
            return hook(&ctx);
        }
        false
    }

    /// Execute a batch of tool calls, returning results and termination flag.
    async fn execute_tool_batch(
        &self,
        tools: &Arc<ToolRegistry>,
        tool_calls: &[oxi_ai::ToolCall],
        tx: &mpsc::Sender<AgentEvent>,
    ) -> ToolBatchResult {
        let mode = {
            let hooks = self.hooks.read();
            hooks.tool_execution
        };

        match mode {
            crate::config::ToolExecutionMode::Parallel => {
                self.execute_tools_parallel(tools, tool_calls, tx).await
            }
            crate::config::ToolExecutionMode::Sequential => {
                self.execute_tools_sequential(tools, tool_calls, tx).await
            }
        }
    }

    /// Execute tool calls sequentially.
    async fn execute_tools_sequential(
        &self,
        tools: &Arc<ToolRegistry>,
        tool_calls: &[oxi_ai::ToolCall],
        tx: &mpsc::Sender<AgentEvent>,
    ) -> ToolBatchResult {
        let mut messages = Vec::new();
        let mut all_terminate = true;

        for tool_call in tool_calls {
            let tool_call_id = tool_call.id.clone();
            let tool_name = tool_call.name.clone();

            // beforeToolCall hook
            if self.before_tool_call(&tool_call_id, &tool_name, &tool_call.arguments) {
                let error_msg = format!("Tool '{}' execution blocked by beforeToolCall hook", tool_name);
                let result_msg = oxi_ai::ToolResultMessage::new(
                    tool_call_id,
                    tool_name,
                    vec![ContentBlock::Text(TextContent::new(error_msg.clone()))],
                );
                messages.push(result_msg);
                continue;
            }

            let _ = tx.send(AgentEvent::ToolStart {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                arguments: tool_call.arguments.clone(),
            }).await;

            let tool_result = self.execute_tool_single(tools, tool_call, tx.clone()).await;

            // afterToolCall hook
            let finalized = self.after_tool_call(
                &tool_call_id,
                &tool_name,
                &tool_result.content,
                tool_result.status == "error",
            );

            let _ = tx.send(AgentEvent::ToolComplete {
                result: tool_result.clone(),
            }).await;

            let result_msg = oxi_ai::ToolResultMessage::new(
                tool_call_id,
                tool_name,
                vec![ContentBlock::Text(TextContent::new(
                    finalized.content.unwrap_or(tool_result.content.clone())
                ))],
            );
            messages.push(result_msg);

            if !finalized.terminate.unwrap_or(false) {
                all_terminate = false;
            }
        }

        ToolBatchResult {
            messages,
            terminate: all_terminate && !tool_calls.is_empty(),
        }
    }

    /// Execute tool calls in parallel (fallback to sequential for simplicity).
    /// Full parallel execution requires tools to be Send + 'static safe.
    async fn execute_tools_parallel(
        &self,
        tools: &Arc<ToolRegistry>,
        tool_calls: &[oxi_ai::ToolCall],
        tx: &mpsc::Sender<AgentEvent>,
    ) -> ToolBatchResult {
        // For now, use sequential execution under the parallel mode.
        // True parallel execution requires restructuring tools to be
        // spawn-safe. This matches pi-mono's "prepare sequentially,
        // execute concurrently" pattern in spirit.
        self.execute_tools_sequential(tools, tool_calls, tx).await
    }

    /// Execute a single tool call (shared between sequential and parallel).
    async fn execute_tool_single(
        &self,
        tools: &Arc<ToolRegistry>,
        tool_call: &oxi_ai::ToolCall,
        tx: mpsc::Sender<AgentEvent>,
    ) -> oxi_ai::ToolResult {
        let tool_call_id = tool_call.id.clone();
        let tool_name = tool_call.name.clone();

        let tool = match tools.get(&tool_name) {
            Some(t) => t,
            None => {
                return oxi_ai::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    content: format!("Error: Unknown tool '{}'", tool_name),
                    status: "error".to_string(),
                };
            }
        };

        // Set up progress callback
        let tool_call_id_clone = tool_call_id.clone();
        let tx_clone = tx.clone();
        let progress_cb = progress_callback(move |msg: String| {
            let tx = tx_clone.clone();
            let tool_call_id = tool_call_id_clone.clone();
            tokio::spawn(async move {
                let _ = tx.send(AgentEvent::ToolProgress {
                    tool_call_id,
                    message: msg,
                }).await;
            });
        });
        tool.on_progress(progress_cb);

        let params = tool_call.arguments.clone();

        match tool.execute(&tool_call_id, params, None).await {
            Ok(AgentToolResult { success, output, .. }) => oxi_ai::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: output,
                status: if success { "success".to_string() } else { "error".to_string() },
            },
            Err(e) => oxi_ai::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: e,
                status: "error".to_string(),
            },
        }
    }

    /// Call beforeToolCall hook. Returns true if the call should be blocked.
    fn before_tool_call(&self, tool_call_id: &str, tool_name: &str, args: &serde_json::Value) -> bool {
        let hooks = self.hooks.read();
        if let Some(ref hook) = hooks.before_tool_call {
            let ctx = crate::config::BeforeToolCallContext {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                args: args.clone(),
            };
            let result = hook(&ctx);
            return result.block;
        }
        false
    }

    /// Call afterToolCall hook and return finalized result.
    fn after_tool_call(&self, tool_call_id: &str, tool_name: &str, result: &str, is_error: bool) -> crate::config::AfterToolCallResult {
        let hooks = self.hooks.read();
        if let Some(ref hook) = hooks.after_tool_call {
            let ctx = crate::config::AfterToolCallContext {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                result: result.to_string(),
                is_error,
            };
            return hook(&ctx);
        }
        crate::config::AfterToolCallResult::default()
    }

    /// Set hooks for the agent loop.
    pub fn set_hooks(&self, hooks: crate::config::AgentHooks) {
        let mut h = self.hooks.write();
        *h = hooks;
    }

    /// Run the agent, invoking `on_event` for each [`AgentEvent`] produced.
    ///
    /// Blocking convenience wrapper suitable for callers that prefer a
    /// callback-based API over a channel.
    pub async fn run_streaming<F>(&self, prompt: String, mut on_event: F) -> Result<Response>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(100);
        let tx_clone = tx;
        let result = self.run_with_channel(prompt, tx_clone).await;
        while let Some(event) = rx.recv().await {
            on_event(event);
        }
        result
    }

    // -----------------------------------------------------------------------
    // Retry & fallback helpers
    // -----------------------------------------------------------------------

    /// Attempt to stream from the provider with retry + exponential back-off.
    ///
    /// Delegates to [`stream_retry::stream_with_retry_core`] and emits
    /// [`AgentEvent::Retry`] events through the channel.
    async fn stream_with_retry(
        provider: &dyn Provider,
        model: &oxi_ai::Model,
        context: &Context,
        options: Option<StreamOptions>,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> std::result::Result<futures::stream::BoxStream<'static, ProviderEvent>, AgentError> {
        let cb = MpscRetryCallback { tx: tx.clone() };
        stream_retry::stream_with_retry_core(
            provider,
            model,
            context,
            options,
            &cb,
            None,  // no max_delay cap for Agent
            || {},   // no circuit-breaker tracking for Agent
            || {},
        )
        .await
    }

    /// Try a fallback model when the primary model fails.
    ///
    /// Returns the streaming response from the fallback, or the combined
    /// [`AgentError::FallbackFailed`] if both models fail.
    async fn try_fallback(
        &self,
        model: &oxi_ai::Model,
        context: &Context,
        options: Option<StreamOptions>,
        tx: &mpsc::Sender<AgentEvent>,
        primary_error: String,
    ) -> std::result::Result<futures::stream::BoxStream<'static, ProviderEvent>, AgentError> {
        // Resolve fallback model
        let fallback_id = DEFAULT_FALLBACK_MODEL;
        let fallback_model = crate::model_id::resolve_model_from_id(fallback_id);

        let fallback_model = match fallback_model {
            Some(m) => m,
            None => {
                return Err(AgentError::FallbackFailed {
                    primary_model: format!("{}/{}", model.provider, model.id),
                    primary_error,
                    fallback_model: fallback_id.to_string(),
                    fallback_error: "Model not found in registry".into(),
                });
            }
        };

        let fallback_provider = match oxi_ai::get_provider(&fallback_model.provider) {
            Some(p) => p,
            None => {
                return Err(AgentError::FallbackFailed {
                    primary_model: format!("{}/{}", model.provider, model.id),
                    primary_error,
                    fallback_model: fallback_id.to_string(),
                    fallback_error: "Provider not available".into(),
                });
            }
        };

        let _ = tx
            .send(AgentEvent::Fallback {
                from_model: format!("{}/{}", model.provider, model.id),
                to_model: fallback_id.to_string(),
            })
            .await;

        // Try streaming with the fallback provider
        match Self::stream_with_retry(
            fallback_provider.as_ref(),
            &fallback_model,
            context,
            options,
            tx,
        )
        .await
        {
            Ok(stream) => Ok(stream),
            Err(fallback_err) => Err(AgentError::FallbackFailed {
                primary_model: format!("{}/{}", model.provider, model.id),
                primary_error,
                fallback_model: fallback_id.to_string(),
                fallback_error: fallback_err.to_string(),
            }),
        }
    }
}
