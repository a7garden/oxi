/// Core agent implementation

use crate::config::AgentConfig;
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
    /// Implements a 2-level agentic loop matching pi-mono's architecture:
    ///
    /// ```text
    /// Outer loop (follow-up messages):
    ///   Inner loop (tool calls + steering):
    ///     1. Inject pending messages (steering)
    ///     2. Compaction check
    ///     3. Build context + stream LLM response
    ///     4. If ToolUse → execute tools → continue inner loop
    ///     5. If Stop/Error → emit turn_end
    ///     6. Check shouldStopAfterTurn
    ///     7. Poll steering messages → continue inner loop if any
    ///   Check follow-up messages → continue outer loop if any
    ///   Exit
    /// ```
    pub async fn run_with_channel(
        &self,
        prompt: String,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        let _ = tx.send(AgentEvent::Start { prompt: prompt.clone() }).await;

        // Add user message to state
        self.state.update(|s| {
            s.add_user_message(prompt);
        });

        // ── Pre-flight: resolve model + initial compaction ──────────

        let model = {
            let inner = self.config();
            crate::model_id::resolve_model_from_id(&inner.config.model_id)
        }
        .ok_or_else(|| {
            let inner = self.config();
            Error::msg(format!("Model not found: {}", inner.config.model_id))
        })?;

        // Initial compaction check
        self.run_compaction_check(&tx).await;

        // ── Agentic loop ────────────────────────────────────────────
        let tools = self.tools.clone();
        let max_iterations = {
            let inner = self.config();
            inner.config.max_iterations
        };
        let mut turn_number: u32 = 0;
        let mut final_response_text = String::new();

        // Check for steering messages at start
        let mut pending_messages: Vec<String> = self.drain_steering_messages();

        // Outer loop: continues when follow-up messages arrive
        'outer: loop {
            let mut has_more_tool_calls = true;

            // Inner loop: process tool calls and steering messages
            while has_more_tool_calls || !pending_messages.is_empty() {
                // ── Iteration guard ──────────────────────────────
                let iteration = self.state.get_state().iteration;
                if iteration >= max_iterations {
                    let _ = tx.send(AgentEvent::Error {
                        session_id: None,
                        message: format!("Max iterations ({}) reached", max_iterations),
                    }).await;
                    break 'outer;
                }

                // ── Turn start ──────────────────────────────────
                turn_number += 1;
                let _ = tx.send(AgentEvent::TurnStart { turn_number }).await;

                // ── Inject pending (steering) messages ──────────
                if !pending_messages.is_empty() {
                    for msg_text in pending_messages.drain(..) {
                        let user_msg = oxi_ai::UserMessage::new(msg_text);
                        let message = oxi_ai::Message::User(user_msg);
                        let _ = tx.send(AgentEvent::SteeringMessage {
                            message: message.clone(),
                        }).await;
                        self.state.update(|s| {
                            s.messages.push(message);
                        });
                    }
                }

                // ── Compaction check ────────────────────────────
                self.run_compaction_check(&tx).await;

                // ── Build context ───────────────────────────────
                let state_messages = self.state.get_state().messages.clone();
                let mut context = Context::new();
                if let Some(ref sp) = self.config().config.system_prompt {
                    context.set_system_prompt(sp.clone());
                }
                for msg in &state_messages {
                    context.add_message(msg.clone());
                }
                // Add tools
                let tool_defs = tools.definitions();
                if !tool_defs.is_empty() {
                    let mut oxi_tools = Vec::new();
                    for def in &tool_defs {
                        let schema = serde_json::to_value(&def.input_schema)
                            .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
                        oxi_tools.push(oxi_ai::Tool::new(&def.name, &def.description, schema));
                    }
                    context.set_tools(oxi_tools);
                }

                let stream_options = {
                    let inner = self.config();
                    StreamOptions {
                        temperature: inner.config.temperature,
                        max_tokens: inner.config.max_tokens,
                        api_key: inner.config.api_key.clone(),
                        ..Default::default()
                    }
                };

                let provider: Arc<dyn Provider> = {
                    let inner = self.config();
                    Arc::clone(&inner.provider)
                };

                // ── Stream LLM response ─────────────────────────
                let mut stream = match Self::stream_with_retry(
                    provider.as_ref(),
                    &model,
                    &context,
                    Some(stream_options),
                    &tx,
                ).await {
                    Ok(s) => s,
                    Err(primary_err) => {
                        // Try fallback model before giving up
                        let _ = tx.send(AgentEvent::Error {
                            session_id: None,
                            message: format!("Primary model failed: {}", primary_err.user_friendly()),
                        }).await;

                        match self.try_fallback(
                            &model,
                            &context,
                            None,
                            &tx,
                            primary_err.to_string(),
                        ).await {
                            Ok(s) => s,
                            Err(fallback_err) => {
                                let _ = tx.send(AgentEvent::Error {
                                    session_id: None,
                                    message: format!("Fallback also failed: {}", fallback_err.user_friendly()),
                                }).await;
                                break 'outer;
                            }
                        }
                    }
                };

                // ── Process stream events ───────────────────────
                // Following pi-mono's architecture: use event.partial() for
                // MessageUpdate so content blocks (text + toolCalls) are properly
                // separated by the provider layer.
                let mut iteration_text = String::new();
                let mut pending_tool_calls: Vec<oxi_ai::ToolCall> = Vec::new();
                let mut thinking_text = String::new();
                let mut stream_done = false;
                let mut stop_reason = StopReason::Stop;
                let mut message_started = false;

                while let Some(event) = stream.next().await {
                    match event {
                        ProviderEvent::Start { partial } => {
                            let msg = oxi_ai::Message::Assistant(partial.clone());
                            let _ = tx.send(AgentEvent::MessageStart {
                                message: msg,
                            }).await;
                            message_started = true;
                        }
                        ProviderEvent::TextDelta { delta, partial, .. } => {
                            if !message_started {
                                let msg = oxi_ai::Message::Assistant(partial.clone());
                                let _ = tx.send(AgentEvent::MessageStart {
                                    message: msg,
                                }).await;
                                message_started = true;
                            }
                            iteration_text.push_str(&delta);
                            let _ = tx.send(AgentEvent::TextChunk { text: delta.clone() }).await;
                            // Use provider's partial message for MessageUpdate —
                            // it has properly separated content blocks.
                            let _ = tx.send(AgentEvent::MessageUpdate {
                                message: oxi_ai::Message::Assistant(partial),
                                delta: Some(delta),
                            }).await;
                        }
                        ProviderEvent::ThinkingDelta { delta, partial, .. } => {
                            thinking_text.push_str(&delta);
                            let _ = tx.send(AgentEvent::ThinkingDelta { text: delta }).await;
                            // Also emit MessageUpdate with partial that includes thinking
                            let _ = tx.send(AgentEvent::MessageUpdate {
                                message: oxi_ai::Message::Assistant(partial),
                                delta: None,
                            }).await;
                        }
                        ProviderEvent::ToolCallStart { partial, .. } => {
                            if !message_started {
                                let msg = oxi_ai::Message::Assistant(partial.clone());
                                let _ = tx.send(AgentEvent::MessageStart {
                                    message: msg,
                                }).await;
                                message_started = true;
                            }
                            // Emit MessageUpdate with partial — tool call block now visible
                            let _ = tx.send(AgentEvent::MessageUpdate {
                                message: oxi_ai::Message::Assistant(partial),
                                delta: None,
                            }).await;
                        }
                        ProviderEvent::ToolCallDelta { partial, .. } => {
                            let _ = tx.send(AgentEvent::MessageUpdate {
                                message: oxi_ai::Message::Assistant(partial),
                                delta: None,
                            }).await;
                        }
                        ProviderEvent::ToolCallEnd { tool_call, partial, .. } => {
                            pending_tool_calls.push(tool_call);
                            let _ = tx.send(AgentEvent::MessageUpdate {
                                message: oxi_ai::Message::Assistant(partial),
                                delta: None,
                            }).await;
                        }

                        ProviderEvent::Done { reason, message: ref done_msg } => {
                            if message_started {
                                let _ = tx.send(AgentEvent::MessageEnd {
                                    message: oxi_ai::Message::Assistant(done_msg.clone()),
                                }).await;
                            }

                            stop_reason = match reason {
                                oxi_ai::StopReason::Stop => StopReason::Stop,
                                oxi_ai::StopReason::Length => StopReason::Length,
                                oxi_ai::StopReason::ToolUse => StopReason::ToolUse,
                                oxi_ai::StopReason::Error => StopReason::Error,
                                _ => StopReason::Stop,
                            };

                            // Use done_msg's content blocks directly — provider already
                            // built the correct structure with text/thinking/toolCall blocks
                            let mut assistant_msg = done_msg.clone();

                            // Track text for final_response_text
                            for block in &assistant_msg.content {
                                if let ContentBlock::Text(t) = block {
                                    iteration_text = t.text.clone();
                                }
                            }

                            // Add assistant message to state
                            self.state.update(|s| {
                                s.messages.push(Message::Assistant(assistant_msg.clone()));
                            });

                            final_response_text = iteration_text.clone();
                            stream_done = true;
                        }
                        ProviderEvent::Error { error, .. } => {
                            let friendly = error.text_content();
                            let friendly = if friendly.is_empty() {
                                "Unknown provider error".to_string()
                            } else {
                                friendly
                            };
                            let _ = tx.send(AgentEvent::Error {
                                session_id: None,
                                message: friendly.clone(),
                            }).await;
                            break 'outer;
                        }
                        _ => {}
                    }
                }

                if !stream_done {
                    break 'outer;
                }

                // ── Execute tool calls if any ────────────────────
                let mut tool_result_messages: Vec<oxi_ai::ToolResultMessage> = Vec::new();
                has_more_tool_calls = false;

                if !pending_tool_calls.is_empty() && matches!(stop_reason, StopReason::ToolUse) {
                    let executed = self.execute_tool_batch(
                        &tools,
                        &pending_tool_calls,
                        &tx,
                    ).await;

                    has_more_tool_calls = !executed.terminate;
                    tool_result_messages = executed.messages.clone();

                    // Add tool results to state
                    for msg in &executed.messages {
                        self.state.update(|s| {
                            s.messages.push(Message::ToolResult(msg.clone()));
                        });
                    }
                }

                // ── Turn end ────────────────────────────────────
                // Build the turn's assistant message for TurnEnd event
                let turn_assistant = {
                    let mut content_blocks = vec![];
                    if !iteration_text.is_empty() {
                        content_blocks.push(ContentBlock::Text(TextContent::new(iteration_text.clone())));
                    }
                    let mut msg = oxi_ai::AssistantMessage::new(
                        oxi_ai::Api::OpenAiCompletions, "agent", &model.id,
                    );
                    msg.content = content_blocks;
                    oxi_ai::Message::Assistant(msg)
                };
                let _ = tx.send(AgentEvent::TurnEnd {
                    turn_number,
                    assistant_message: turn_assistant,
                    tool_results: tool_result_messages,
                }).await;

                self.state.update(|s| {
                    s.increment_iteration();
                    s.set_stop_reason(stop_reason);
                });
                let _ = tx.send(AgentEvent::Iteration {
                    number: self.state.get_state().iteration,
                }).await;

                // ── Check shouldStopAfterTurn ────────────────────
                if self.should_stop_after_turn() {
                    break 'outer;
                }

                // ── Error stop → exit ────────────────────────────
                if matches!(stop_reason, StopReason::Error) {
                    break 'outer;
                }

                // ── Poll steering messages ──────────────────────
                pending_messages = self.drain_steering_messages();
            }

            // ── Outer loop: check follow-up messages ──────────────
            let follow_ups = self.drain_follow_up_messages();
            if !follow_ups.is_empty() {
                pending_messages = follow_ups;
                continue 'outer;
            }

            // No more messages, exit
            break 'outer;
        }

        let _ = tx.send(AgentEvent::Complete {
            content: final_response_text.clone(),
            stop_reason: format!("{:?}", self.state.get_state().stop_reason.unwrap_or(StopReason::Stop)),
        }).await;

        Ok(Response {
            content: final_response_text,
            stop_reason: self.state.get_state().stop_reason.unwrap_or(StopReason::Stop),
        })
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
