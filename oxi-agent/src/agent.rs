/// Core agent implementation

use crate::compaction::{CompactedContext as AgentCompactedContext, CompactionEvent};
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
    TextContent, ToolCall,
};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;
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

/// Agent 런타임.
///
/// 프로바이더, 도구 레지스트리, 상태, 컴팩션 매니저를 통합 관리하며
/// 프롬프트 실행, 모델 전환, 도구 호출, 폴백 등의 에이전트 루프를 제공한다.
pub struct Agent {
    inner: RwLock<AgentInner>,
    tools: Arc<ToolRegistry>,
    state: SharedState,
    compaction_manager: CompactionManager,
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
    /// Handles compaction, streaming, tool execution, retries, and fallback.
    pub async fn run_with_channel(
        &self,
        prompt: String,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        let _ = tx
            .send(AgentEvent::Start {
                prompt: prompt.clone(),
            })
            .await;
        let _ = tx.send(AgentEvent::Thinking).await;

        self.state.update(|s| {
            s.add_user_message(prompt);
        });

        let model = {
            let inner = self.config();
            crate::model_id::resolve_model_from_id(&inner.config.model_id)
        }
        .ok_or_else(|| {
            let inner = self.config();
            Error::msg(format!("Model not found: {}", inner.config.model_id))
        })?;

        // Check for compaction at the start of each iteration
        let messages = &self.state.get_state().messages;
        let iteration = self.state.get_state().iteration;

        // Estimate token count
        let context_text = serde_json::to_string(messages).unwrap_or_default();
        let context_tokens = oxi_ai::estimate_tokens(&context_text);

        // Try to compact if needed
        if self
            .compaction_manager
            .should_compact(context_tokens, iteration)
        {
            let _ = tx
                .send(AgentEvent::Compaction {
                    event: CompactionEvent::Triggered {
                        context_tokens,
                        iteration,
                    },
                })
                .await;

            // Clone messages for compaction since compact_if_needed takes a reference
            let messages_to_compact: Vec<Message> = messages.iter().cloned().collect();

            match self
                .compaction_manager
                .compact_if_needed(
                    &messages_to_compact,
                    {
                        let inner = self.config();
                        inner.config.compaction_instruction.clone().as_deref()
                    },
                    context_tokens,
                    iteration,
                )
                .await
            {
                Ok(Some(compacted)) => {
                    let start = Instant::now();
                    let message_count = compacted.compacted_count;
                    let _ = tx
                        .send(AgentEvent::Compaction {
                            event: CompactionEvent::Started { message_count },
                        })
                        .await;

                    // Extract data before moving
                    let kept_messages = compacted.kept_messages;
                    let summary = compacted.summary;
                    let compacted_count = compacted.compacted_count;

                    // Replace old messages with compacted context
                    self.state.update(|s| {
                        s.replace_messages(kept_messages);
                    });

                    let compacted_ctx = AgentCompactedContext {
                        summary,
                        kept_messages: Vec::new(), // Already moved to state
                        compacted_count,
                    };
                    let _ = tx
                        .send(AgentEvent::Compaction {
                            event: CompactionEvent::Completed {
                                result: compacted_ctx,
                                duration_ms: start.elapsed().as_millis() as u64,
                            },
                        })
                        .await;
                }
                Ok(None) => {
                    // No compaction needed
                }
                Err(e) => {
                    let _ = tx
                        .send(AgentEvent::Compaction {
                            event: CompactionEvent::Failed {
                                error: e.to_string(),
                            },
                        })
                        .await;
                }
            }
        }

        let mut context = Context::new();

        // Add system prompt
        {
            let inner = self.config();
            if let Some(ref system_prompt) = inner.config.system_prompt {
                context.set_system_prompt(system_prompt.clone());
            }
        }

        // Add previous messages
        for msg in &self.state.get_state().messages {
            context.add_message(msg.clone());
        }

        // Add tools to context
        let tool_defs = self.tools.definitions();
        if !tool_defs.is_empty() {
            let mut oxi_tools = Vec::new();
            for def in &tool_defs {
                let schema = serde_json::to_value(&def.input_schema)
                    .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
                oxi_tools.push(oxi_ai::Tool::new(&def.name, &def.description, schema));
            }
            context.set_tools(oxi_tools);
        }

        let stream_options = StreamOptions {
            temperature: {
                let inner = self.config();
                inner.config.temperature
            },
            max_tokens: {
                let inner = self.config();
                inner.config.max_tokens
            },
            api_key: {
                let inner = self.config();
                inner.config.api_key.clone()
            },
            ..Default::default()
        };

        // Clone provider out of the lock *before* any .await so the
        // RwLockReadGuard is dropped immediately and cannot span an await point.
        let provider: Arc<dyn Provider> = {
            let inner = self.config();
            Arc::clone(&inner.provider)
        };

        let mut stream = match Self::stream_with_retry(
            provider.as_ref(),
            &model,
            &context,
            Some(stream_options),
            &tx,
        )
        .await
        {
            Ok(s) => s,
            Err(primary_err) => {
                // Retry exhausted – try fallback model
                let _ = tx
                    .send(AgentEvent::Error {
                        session_id: None,
                        message: format!(
                            "Primary model failed: {}",
                            primary_err.user_friendly()
                        ),
                    })
                    .await;

                let fallback_options = {
                    let inner2 = self.config();
                    StreamOptions {
                        temperature: inner2.config.temperature,
                        max_tokens: inner2.config.max_tokens,
                        api_key: inner2.config.api_key.clone(),
                        ..Default::default()
                    }
                };
                match self
                    .try_fallback(
                        &model,
                        &context,
                        Some(fallback_options),
                        &tx,
                        primary_err.to_string(),
                    )
                    .await
                {
                    Ok(s) => s,
                    Err(fallback_err) => {
                        let msg = fallback_err.user_friendly();
                        let _ = tx
                            .send(AgentEvent::Error {
                        session_id: None,
                                message: msg.clone(),
                            })
                            .await;
                        return Err(Error::msg(msg));
                    }
                }
            }
        };

        let tx_clone = tx.clone();
        let tools = self.tools.clone();
        let max_iterations = {
            let inner = self.config();
            inner.config.max_iterations
        };

        // Agentic loop (based on pi-mono agent-loop.ts):
        // Outer loop: each iteration = one LLM call
        // Inner loop: process events from one stream
        // ToolUse -> execute tools -> add results -> continue
        // Stop/Length -> return response
        let mut pending_tool_calls: Vec<oxi_ai::ToolCall> = Vec::new();

        loop {
            let current_iteration = self.state.get_state().iteration;
            if current_iteration >= max_iterations {
                let _ = tx_clone.send(AgentEvent::Error {
                    session_id: None,
                    message: format!("Max iterations ({}) reached", max_iterations),
                }).await;
                break;
            }

            // Drain steering messages from queue (injected by TUI during busy)
            // TODO: when AgentSession exposes drain, poll from there
            // For now, steering is handled via agent.state.add_user_message in steer()

            // Compaction check at each iteration
            let state_msgs = self.state.get_state().messages.clone();
            let context_text = serde_json::to_string(&state_msgs).unwrap_or_default();
            let context_tokens = oxi_ai::estimate_tokens(&context_text);
            let iteration = self.state.get_state().iteration;
            if self.compaction_manager.should_compact(context_tokens, iteration) {
                let _ = tx_clone.send(AgentEvent::Compaction {
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
                        let _ = tx_clone.send(AgentEvent::Compaction {
                            event: crate::compaction::CompactionEvent::Started {
                                message_count: compacted.compacted_count,
                            },
                        }).await;
                        self.state.update(|s| {
                            s.messages = compacted.kept_messages.clone();
                        });
                        let _ = tx_clone.send(AgentEvent::Compaction {
                            event: crate::compaction::CompactionEvent::Completed {
                                result: crate::compaction::CompactedContext::from(compacted),
                                duration_ms: 0,
                            },
                        }).await;
                    }
                    Ok(None) => {} // No compaction needed
                    Err(e) => {
                        tracing::warn!("Compaction failed: {}", e);
                    }
                }
            }

            // Rebuild context from state messages for each iteration
            let state_messages = self.state.get_state().messages.clone();
            let mut iter_context = Context::new();
            if let Some(ref prompt) = {
                let inner = self.config();
                inner.config.system_prompt.clone()
            } {
                iter_context.set_system_prompt(prompt);
            }
            for msg in &state_messages {
                iter_context.add_message(msg.clone());
            }
            if !tools.names().is_empty() {
                let tool_defs = tools.definitions();
                let mut oxi_tools = Vec::new();
                for def in &tool_defs {
                    let schema = serde_json::to_value(&def.input_schema)
                        .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
                    oxi_tools.push(oxi_ai::Tool::new(&def.name, &def.description, schema));
                }
                iter_context.set_tools(oxi_tools);
            }

            let iter_stream_options = StreamOptions {
                temperature: {
                    let inner = self.config();
                    inner.config.temperature
                },
                max_tokens: {
                    let inner = self.config();
                    inner.config.max_tokens
                },
                api_key: {
                    let inner = self.config();
                    inner.config.api_key.clone()
                },
                ..Default::default()
            };

            let provider: Arc<dyn Provider> = {
                let inner = self.config();
                Arc::clone(&inner.provider)
            };

            let mut stream = match Self::stream_with_retry(
                provider.as_ref(),
                &model,
                &iter_context,
                Some(iter_stream_options),
                &tx_clone,
            ).await {
                Ok(s) => s,
                Err(e) => {
                    let msg = e.user_friendly();
                    let _ = tx_clone.send(AgentEvent::Error {
                        session_id: None,
                        message: format!("Stream error: {}", msg),
                    }).await;
                    break;
                }
            };

            // Inner loop: process events from this stream
            let mut iteration_text = String::new();
            pending_tool_calls.clear();

            while let Some(event) = stream.next().await {
                match event {
                    ProviderEvent::TextDelta { delta, .. } => {
                        iteration_text.push_str(&delta);
                        let _ = tx_clone.send(AgentEvent::TextChunk { text: delta }).await;
                    }
                    ProviderEvent::ToolCallStart { .. } => {}
                    ProviderEvent::ToolCallEnd { tool_call, .. } => {
                        pending_tool_calls.push(tool_call);
                    }
                    ProviderEvent::Done { reason, message: _ } => {
                        // Build assistant message with tool calls if any
                        let mut content_blocks = vec![ContentBlock::Text(TextContent::new(iteration_text.clone()))];
                        for tc in &pending_tool_calls {
                            content_blocks.push(ContentBlock::ToolCall(tc.clone()));
                        }
                        let mut assistant_msg = oxi_ai::AssistantMessage::new(
                            oxi_ai::Api::OpenAiCompletions, "agent", "agent-model"
                        );
                        assistant_msg.content = content_blocks;
                        self.state.update(|s| {
                            s.messages.push(Message::Assistant(assistant_msg));
                        });

                        // Convert provider stop reason to our StopReason
                        let stop_reason = match reason {
                            oxi_ai::StopReason::Stop => StopReason::Stop,
                            oxi_ai::StopReason::Length => StopReason::Length,
                            oxi_ai::StopReason::ToolUse => StopReason::ToolUse,
                            oxi_ai::StopReason::Error => StopReason::Error,
                            _ => StopReason::Stop,
                        };

                        if !pending_tool_calls.is_empty() && matches!(stop_reason, StopReason::ToolUse) {
                            for tool_call in pending_tool_calls.drain(..) {
                                let tool_name = tool_call.name.clone();
                                let tool_call_id = tool_call.id.clone();

                                let _ = tx_clone.send(AgentEvent::ToolStart {
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: tool_name.clone(),
                                }).await;

                                let tool_result = self
                                    .execute_tool(&tools, &tool_call, tx_clone.clone())
                                    .await;

                                let _ = tx_clone.send(AgentEvent::ToolComplete {
                                    result: tool_result.clone(),
                                }).await;

                                self.state.update(|s| {
                                    s.messages.push(Message::tool_result(
                                        tool_call_id,
                                        tool_name,
                                        vec![oxi_ai::ContentBlock::Text(
                                            oxi_ai::TextContent::new(tool_result.content.clone())
                                        )],
                                    ));
                                });
                            }

                            self.state.update(|s| { s.increment_iteration(); });
                            let _ = tx_clone.send(AgentEvent::Iteration {
                                number: self.state.get_state().iteration,
                            }).await;
                            break;
                        }

                        let _ = tx_clone.send(AgentEvent::Complete {
                            content: iteration_text.clone(),
                            stop_reason: format!("{:?}", reason),
                        }).await;
                        self.state.update(|s| { s.increment_iteration(); });
                        return Ok(Response {
                            content: iteration_text.clone(),
                            stop_reason,
                        });
                    }
                    ProviderEvent::Error { error, .. } => {
                        let friendly = error.text_content();
                        let friendly = if friendly.is_empty() { "Unknown provider error".to_string() } else { friendly };
                        let _ = tx_clone.send(AgentEvent::Error {
                            session_id: None,
                            message: friendly.clone(),
                        }).await;
                        return Err(Error::msg(friendly));
                    }
                    _ => {}
                }
            }

            if pending_tool_calls.is_empty() {
                break;
            }
        }

        Ok(Response {
            content: String::new(),
            stop_reason: StopReason::Stop,
        })
    }

    /// Execute a tool with progress streaming
    async fn execute_tool(
        &self,
        tools: &Arc<ToolRegistry>,
        tool_call: &ToolCall,
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

        // Set up progress callback that emits to the channel
        let tool_call_id_clone = tool_call_id.clone();
        let tx_clone = tx.clone();
        let progress_cb = progress_callback(move |msg: String| {
            let tx = tx_clone.clone();
            let tool_call_id = tool_call_id_clone.clone();
            tokio::spawn(async move {
                let _ = tx
                    .send(AgentEvent::ToolProgress {
                        tool_call_id,
                        message: msg,
                    })
                    .await;
            });
        });

        // Set the callback on the tool
        tool.on_progress(progress_cb);

        // tool_call.arguments is already JsonValue, use it directly
        let params = tool_call.arguments.clone();

        // Execute the tool
        match tool.execute(&tool_call_id, params, None).await {
            Ok(AgentToolResult {
                success, output, ..
            }) => oxi_ai::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: output,
                status: if success {
                    "success".to_string()
                } else {
                    "error".to_string()
                },
            },
            Err(e) => oxi_ai::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: e,
                status: "error".to_string(),
            },
        }
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
