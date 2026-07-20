#![allow(unused_doc_comments)]

//! Agent loop — the main request/response cycle driver.
//!
//! Coordinates the interaction between the agent, provider, tools, and
//! state management. Handles streaming, tool execution, retry logic,
//! and compaction events.

/// Agent-loop configuration.
pub mod config;
/// Miscellaneous helper functions.
pub mod helpers;
/// Internal message/event queues.
pub mod queues;
/// Compaction reminder system (PR-C1).
pub mod reminder;
/// Retry logic for the agent loop.
pub mod retry;
/// Stream outcome types for TTSR integration.
pub mod stream_outcome;
/// Streaming response handling.
pub mod streaming;
/// Tool execution strategies.
pub mod tool_exec;
/// Time-Traveling Stream Rules engine.
pub mod ttsr;

// Re-export for sibling module access
use crate::agent::ProviderResolver;
use crate::compaction::{CompactedContext, CompactionEvent};
use crate::events::AgentEvent;
use crate::recovery::{CircuitBreaker, CircuitBreakerConfig};
use crate::state::TokenSource;
use crate::{state::SharedState, tools::ToolContext, tools::ToolRegistry};
use anyhow::{Error, Result};
pub use config::{AfterToolCallHook, AgentLoopConfig, BeforeToolCallHook, ToolExecutionMode};
use oxi_ai::{
    CompactionManager as OxCompactionManager, CompactionStrategy, ContentBlock, LlmCompactor,
    Message, Provider, StopReason, TextContent, UserMessage,
};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use self::helpers::{sanitize_orphaned_tool_results, should_stop_after_turn};
use self::queues::{
    clear_all_queues, clear_follow_up_queue, clear_steering_queue, drain_follow_up_queue,
    drain_steering_queue, try_push_follow_up, try_push_steering,
};
use self::retry::{
    auto_retry_attempt_method, cancel_auto_retry, handle_retryable_error, is_retryable_error,
};
use self::streaming::stream_assistant_response;
use self::tool_exec::execute_tool_calls;

pub use self::stream_outcome::StreamOutcome;
type EmitFn = Arc<dyn Fn(AgentEvent) + Send + Sync>;

/// AgentLoop.
pub struct AgentLoop {
    provider: Arc<dyn Provider>,
    config: AgentLoopConfig,
    tools: Arc<ToolRegistry>,
    state: SharedState,
    compaction_manager: OxCompactionManager,
    before_tool_call: Option<BeforeToolCallHook>,
    after_tool_call: Option<AfterToolCallHook>,
    steering_queue: RwLock<Vec<Message>>,
    follow_up_queue: RwLock<Vec<Message>>,
    session_id: Option<String>,
    auto_retry_attempt: AtomicUsize,
    auto_retry_cancel: AtomicBool,
    /// Notify used to wake up the auto-retry sleep immediately when cancelled.
    auto_retry_notify: tokio::sync::Notify,
    circuit_breaker: CircuitBreaker,
    /// External stop flag — when set, should_stop_after_turn returns true.
    /// Used by Agent to forward the should_stop_flag from AgentHooks.
    external_stop: Arc<AtomicBool>,
    /// Direct cancel signal shared with `Agent::cancel_flag`.
    /// Set by `Agent::cancel()` and checked by the streaming loop's periodic
    /// timer so cancellation is detected even when no stream events arrive.
    cancel_signal: Option<Arc<AtomicBool>>,
    /// Provider/model resolver for isolated model lookups.
    resolver: Arc<dyn ProviderResolver>,
    /// Steering hook from AgentHooks — polled each turn to drain new messages
    /// from AgentSession's queue into AgentLoop's internal steering_queue.
    steering_hook: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
    /// Follow-up hook from AgentHooks — same as steering but for follow-ups.
    follow_up_hook: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
    /// TTSR engine for stream rule checking.
    ttsr_engine: Option<Arc<ttsr::TtsrEngine>>,
    /// Thinking-loop detector — fed every thinking delta. When a loop
    /// is recognised the stream is aborted with a transient error so
    /// the retry layer resamples. Gated by
    /// [`AgentLoopConfig::thinking_loop_detection`] (default on).
    thinking_loop_detector:
        parking_lot::Mutex<Option<oxi_ai::utils::thinking_loop::ThinkingLoopDetector>>,
    /// Cross-turn tool-call loop guard. Records each completed assistant
    /// turn; when the same single-tool call repeats past threshold the
    /// agent injects a steering message to break the loop (omp
    /// `TERMINAL_TOOL_RESULT_ABORT_REASON` pattern).
    tool_call_loop_guard: parking_lot::Mutex<oxi_ai::utils::tool_call_loop::ToolCallLoopGuard>,
}

impl AgentLoop {
    /// Creates a new `AgentLoop` with an explicit provider resolver.
    /// Use this when the model ID needs to be resolved to a provider+model pair
    /// using custom logic (e.g., per-session routing).
    pub fn new_with_resolver(
        provider: Arc<dyn Provider>,
        config: AgentLoopConfig,
        tools: Arc<ToolRegistry>,
        state: SharedState,
        resolver: Arc<dyn ProviderResolver>,
    ) -> Self {
        let mut compaction_manager =
            OxCompactionManager::new(config.compaction_strategy.clone(), config.context_window);

        if config.compaction_strategy != CompactionStrategy::Disabled {
            let model = resolver.resolve_model(&config.model_id);
            if let Some(model) = model {
                let llm_compactor =
                    Arc::new(LlmCompactor::new(model.clone(), Arc::clone(&provider)));
                compaction_manager.set_compactor(llm_compactor);
            }
        }

        Self {
            provider,
            config: config.clone(),
            tools,
            state,
            compaction_manager,
            before_tool_call: None,
            after_tool_call: None,
            steering_queue: RwLock::new(Vec::new()),
            follow_up_queue: RwLock::new(Vec::new()),
            session_id: config.session_id.clone(),
            auto_retry_attempt: AtomicUsize::new(0),
            auto_retry_cancel: AtomicBool::new(false),
            auto_retry_notify: tokio::sync::Notify::new(),
            circuit_breaker: CircuitBreaker::new(CircuitBreakerConfig::default()),
            external_stop: Arc::new(AtomicBool::new(false)),
            cancel_signal: None,
            resolver,
            steering_hook: None,
            follow_up_hook: None,
            ttsr_engine: config.ttsr_engine.clone(),
            thinking_loop_detector: parking_lot::Mutex::new(if config.thinking_loop_detection {
                Some(oxi_ai::utils::thinking_loop::ThinkingLoopDetector::new())
            } else {
                None
            }),
            tool_call_loop_guard: parking_lot::Mutex::new(
                oxi_ai::utils::tool_call_loop::ToolCallLoopGuard::new(
                    config.tool_call_loop_guard.clone(),
                ),
            ),
        }
    }

    /// Create a new AgentLoop using the global resolver (backward compat).
    pub fn new(
        provider: Arc<dyn Provider>,
        config: AgentLoopConfig,
        tools: Arc<ToolRegistry>,
        state: SharedState,
    ) -> Self {
        use crate::agent::GlobalProviderResolver;
        Self::new_with_resolver(
            provider,
            config,
            tools,
            state,
            Arc::new(GlobalProviderResolver),
        )
    }

    /// Registers a hook called before every tool execution.
    /// The hook can inspect and modify tool arguments, or reject the call entirely.
    pub fn with_before_tool_call(mut self, hook: BeforeToolCallHook) -> Self {
        self.before_tool_call = Some(hook);
        self
    }

    /// Registers a hook called after every tool execution.
    /// The hook receives the tool name, arguments, and result.
    pub fn with_after_tool_call(mut self, hook: AfterToolCallHook) -> Self {
        self.after_tool_call = Some(hook);
        self
    }

    /// Inject a steering message into the agent loop.
    ///
    /// Steering messages are processed at the start of each turn, before the
    /// next LLM call. If the steering queue is at capacity (256 messages), the
    /// message is dropped and a warning is logged.
    pub fn steer(&self, message: Message) {
        if !try_push_steering(self, message) {
            tracing::warn!("Steering message dropped — queue at capacity");
        }
    }

    /// Enqueue a follow-up message to continue the conversation after all
    /// tool calls in the current batch are complete.
    ///
    /// If the follow-up queue is at capacity (64 messages), the message is
    /// dropped and a warning is logged.
    pub fn follow_up(&self, message: Message) {
        if !try_push_follow_up(self, message) {
            tracing::warn!("Follow-up message dropped — queue at capacity");
        }
    }

    /// Removes all pending steering messages from the queue.
    /// See [`steer()`](Self::steer) for an explanation of steering messages.
    pub fn clear_steering_queue(&self) {
        clear_steering_queue(self);
    }

    /// Removes all pending follow-up messages from the queue.
    /// See [`follow_up()`](Self::follow_up) for an explanation of follow-up messages.
    pub fn clear_follow_up_queue(&self) {
        clear_follow_up_queue(self);
    }

    /// Removes all pending messages from both the steering and follow-up queues.
    pub fn clear_all_queues(&self) {
        clear_all_queues(self);
    }

    fn drain_steering_queue(&self) -> Vec<Message> {
        drain_steering_queue(self)
    }

    /// Build a ToolContext from the agent loop config.
    /// Uses workspace_dir from config if set, otherwise falls back to current directory.
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn build_tool_context(&self) -> ToolContext {
        let workspace = self
            .config
            .workspace_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        ToolContext {
            workspace_dir: workspace,
            root_dir: self.config.workspace_dir.clone(),
            session_id: self.session_id.clone(),
            snapshot_store: self.config.snapshot_store.clone(),
            memory: self.config.memory.clone(),
            url_resolver: self.config.url_resolver.clone(),
            todo: self.config.todo.clone(),
            agent_pool: self.config.agent_pool.clone(),
            lsp: self.config.lsp.clone(),
            subagent_runner: self.config.subagent_runner.clone(),
            subagent_depth: self.config.subagent_depth,
        }
    }

    /// Truncate a tool result's text content if it exceeds
    /// `config.max_tool_result_bytes` (issue #28 gap 1).
    ///
    /// When the limit is `None`, the result is returned unchanged.
    /// When set, any `ContentBlock::Text` block whose `text` field
    /// exceeds the limit is truncated to the limit and a marker is
    /// appended so the model knows content was omitted.
    fn maybe_truncate_tool_result(
        &self,
        mut result: oxi_ai::ToolResultMessage,
    ) -> oxi_ai::ToolResultMessage {
        let Some(max_bytes) = self.config.max_tool_result_bytes else {
            return result;
        };

        for block in &mut result.content {
            if let oxi_ai::ContentBlock::Text(tc) = block
                && tc.text.len() > max_bytes
            {
                let omitted = tc.text.len() - max_bytes;
                tc.text.truncate(max_bytes);
                tc.text.push_str(&format!(
                    "\n\n... [truncated: {omitted} bytes omitted, \
                     use read/grep for full content]"
                ));
            }
        }

        result
    }

    fn drain_follow_up_queue(&self) -> Vec<Message> {
        drain_follow_up_queue(self)
    }

    /// Cancels any in-progress auto-retry countdown.
    /// After calling this, the agent will not automatically retry
    /// on the next turn.
    pub fn cancel_auto_retry(&self) {
        cancel_auto_retry(self);
    }

    /// Returns the current auto-retry attempt number (0-based).
    /// Useful for displaying retry status in the UI.
    pub fn auto_retry_attempt(&self) -> usize {
        auto_retry_attempt_method(self)
    }

    /// Get a reference to the shared state.
    /// Used by Agent to sync state after loop execution.
    pub fn state(&self) -> &SharedState {
        &self.state
    }

    /// Get the external stop flag.
    pub fn external_stop(&self) -> &Arc<AtomicBool> {
        &self.external_stop
    }

    /// Sets a shared cancel signal (typically `Agent::cancel_flag`).
    /// The streaming loop checks this in its periodic wake-up timer,
    /// ensuring cancellation is detected even when the provider stream
    /// produces no events (e.g. waiting for first token).
    pub fn set_cancel_signal(&mut self, flag: Arc<AtomicBool>) {
        self.cancel_signal = Some(flag);
    }

    /// Returns a clone of the loop's cancel-signal flag, if one has been
    /// installed via [`Self::set_cancel_signal`]. Used by tool execution
    /// to bridge the loop's `AtomicBool` cancellation into the per-tool
    /// `oneshot::Receiver` cancellation channel (audit finding F-8).
    pub fn cancel_signal(&self) -> Option<Arc<AtomicBool>> {
        self.cancel_signal.as_ref().map(Arc::clone)
    }

    /// Returns true if cancellation has been requested via either
    /// `external_stop` or the direct `cancel_signal`.
    pub fn is_cancelled(&self) -> bool {
        if self.external_stop.load(Ordering::SeqCst) {
            return true;
        }
        self.cancel_signal
            .as_ref()
            .is_some_and(|f| f.load(Ordering::SeqCst))
    }
    /// Request cancellation from outside the loop (e.g. Ctrl+C).
    /// Sets the `external_stop` flag which causes the streaming loop
    /// to abort on its next periodic check (~500ms) and the agent loop
    /// to exit after the current turn.
    pub fn cancel(&self) {
        self.external_stop.store(true, Ordering::SeqCst);
    }

    /// Set the steering hook — called each turn to drain new messages
    /// from the session's steering queue into the loop's internal queue.
    pub fn set_steering_hook(&mut self, hook: Arc<dyn Fn() -> Vec<String> + Send + Sync>) {
        self.steering_hook = Some(hook);
    }

    /// Set the follow-up hook — called each turn to drain new messages
    /// from the session's follow-up queue into the loop's internal queue.
    pub fn set_follow_up_hook(&mut self, hook: Arc<dyn Fn() -> Vec<String> + Send + Sync>) {
        self.follow_up_hook = Some(hook);
    }

    /// Poll the steering/follow-up hooks and inject new messages
    /// into the internal queues.
    fn poll_external_queues(&self) {
        if let Some(ref hook) = self.steering_hook {
            for msg_text in hook() {
                self.steer(Message::User(UserMessage::new(msg_text)));
            }
        }
        if let Some(ref hook) = self.follow_up_hook {
            for msg_text in hook() {
                self.follow_up(Message::User(UserMessage::new(msg_text)));
            }
        }
    }

    /// Runs the agent loop with a single user prompt.
    /// Convenience wrapper around [`run_messages()`](Self::run_messages).
    pub async fn run(
        &self,
        prompt: String,
        emit: impl Fn(AgentEvent) + Send + Sync + 'static,
    ) -> Result<Vec<AgentEvent>> {
        let message = Message::User(UserMessage::new(prompt));
        let emit = Arc::new(emit);
        self.run_messages(vec![message], emit).await
    }

    /// Run with an `FnMut` callback and mutable state — no `Arc<Mutex<>>` needed.
    ///
    /// Unlike [`run()`](Self::run), which takes `Fn`, this method accepts `FnMut`
    /// and a user-provided state value `S`. The callback receives `&mut S` on each
    /// event, so you can accumulate results without any locking overhead.
    ///
    /// Returns the collected events **and** the final state value.
    ///
    /// # Example
    ///
    /// ```ignore
    /// #[derive(Default)]
    /// struct MyState { steps: usize, output: String }
    ///
    /// let (events, state) = agent_loop.run_mut(
    ///     "do something".into(),
    ///     MyState::default(),
    ///     |event, s| {
    ///         match event {
    ///             AgentEvent::ToolExecutionEnd { is_error: false, .. } => s.steps += 1,
    ///             AgentEvent::AgentEnd { messages, .. } => {
    ///                 if let Some(Message::Assistant(a)) = messages.last() {
    ///                     s.output = a.text_content();
    ///                 }
    ///             }
    ///             _ => {}
    ///         }
    ///     },
    /// ).await?;
    /// ```
    pub async fn run_mut<S: Send + std::fmt::Debug + 'static>(
        &self,
        prompt: String,
        state: S,
        emit: impl FnMut(AgentEvent, &mut S) + Send + 'static,
    ) -> Result<(Vec<AgentEvent>, S)> {
        let emit_fnmut = Arc::new(parking_lot::Mutex::new(emit));
        let state_arc = Arc::new(parking_lot::Mutex::new(state));

        // Clone the Arc for the closure; the original stays for recovery after run.
        let state_for_closure = Arc::clone(&state_arc);

        let emit_fn: EmitFn = Arc::new(move |event: AgentEvent| {
            let mut cb = emit_fnmut.lock();
            let mut s = state_for_closure.lock();
            cb(event, &mut s);
        });

        let events = self.run_inner(prompt, emit_fn).await?;

        // Recover the state. After run_inner completes, emit_fn is dropped,
        // releasing the last Arc clone. Arc::try_unwrap should succeed since
        // only our `state_arc` reference remains.
        let mutex = Arc::try_unwrap(state_arc)
            .expect("run_mut: state Arc still has multiple owners after run");
        Ok((events, mutex.into_inner()))
    }

    /// Internal: create the initial user message and delegate to run_messages.
    async fn run_inner(&self, prompt: String, emit: EmitFn) -> Result<Vec<AgentEvent>> {
        let message = Message::User(UserMessage::new(prompt));
        self.run_messages(vec![message], emit).await
    }

    /// Runs the agent loop with a list of pre-constructed messages.
    /// This is the primary entry point for executing agent turns.
    pub async fn run_messages(
        &self,
        prompts: Vec<Message>,
        emit: EmitFn,
    ) -> Result<Vec<AgentEvent>> {
        let mut all_events = Vec::new();

        let state_messages = self.state.get_state().messages.clone();
        let mut all_messages = state_messages;
        all_messages.extend(prompts.clone());

        tracing::info!(session_id = ?self.session_id, "AgentLoop starting");
        emit(AgentEvent::AgentStart {
            prompts: prompts.clone(),
            session_id: self.session_id.clone(),
        });
        all_events.push(AgentEvent::AgentStart {
            prompts: prompts.clone(),
            session_id: self.session_id.clone(),
        });

        let (result_messages, events) = self.run_loop(prompts, emit.clone()).await?;

        all_events.extend(events);

        let stop_reason = result_messages.last().and_then(|m| {
            if let Message::Assistant(a) = m {
                Some(format!("{:?}", a.stop_reason))
            } else {
                None
            }
        });

        tracing::info!(session_id = ?self.session_id, "AgentLoop run_messages complete");

        // Sync messages back to shared state
        self.state.update(|s| {
            s.replace_messages(result_messages.clone());
        });

        emit(AgentEvent::AgentEnd {
            messages: result_messages.clone(),
            stop_reason: stop_reason.clone(),
            session_id: self.session_id.clone(),
        });
        all_events.push(AgentEvent::AgentEnd {
            messages: result_messages.clone(),
            stop_reason,
            session_id: self.session_id.clone(),
        });

        Ok(all_events)
    }

    /// Resumes the agent loop after a previous turn ended in a paused state
    /// (e.g., waiting for user confirmation). Emits events to the provided callback.
    pub async fn continue_loop(
        &self,
        emit: impl Fn(AgentEvent) + Send + Sync + 'static,
    ) -> Result<Vec<AgentEvent>> {
        let emit = Arc::new(emit);
        let mut all_events = Vec::new();

        tracing::info!(session_id = ?self.session_id, "AgentLoop continuing");
        emit(AgentEvent::AgentStart {
            prompts: vec![],
            session_id: self.session_id.clone(),
        });
        all_events.push(AgentEvent::AgentStart {
            prompts: vec![],
            session_id: self.session_id.clone(),
        });

        let (result_messages, events) = self.run_loop(vec![], emit.clone()).await?;

        all_events.extend(events);

        let stop_reason = result_messages.last().and_then(|m| {
            if let Message::Assistant(a) = m {
                Some(format!("{:?}", a.stop_reason))
            } else {
                None
            }
        });

        tracing::info!(session_id = ?self.session_id, "AgentLoop continue_loop complete");
        emit(AgentEvent::AgentEnd {
            messages: result_messages.clone(),
            stop_reason: stop_reason.clone(),
            session_id: self.session_id.clone(),
        });
        all_events.push(AgentEvent::AgentEnd {
            messages: result_messages.clone(),
            stop_reason,
            session_id: self.session_id.clone(),
        });

        Ok(all_events)
    }

    /// Process pending steering messages, emitting events and appending to message history.
    fn process_steering_messages(
        &self,
        pending_messages: &mut Vec<Message>,
        messages: &mut Vec<Message>,
        new_messages: &mut Vec<Message>,
        events: &mut Vec<AgentEvent>,
        emit: &EmitFn,
    ) {
        if pending_messages.is_empty() {
            return;
        }
        for message in pending_messages.drain(..) {
            emit(AgentEvent::SteeringMessage {
                message: message.clone(),
            });
            emit(AgentEvent::MessageStart {
                message: message.clone(),
            });
            emit(AgentEvent::MessageEnd {
                message: message.clone(),
            });
            events.push(AgentEvent::SteeringMessage {
                message: message.clone(),
            });
            events.push(AgentEvent::MessageStart {
                message: message.clone(),
            });
            events.push(AgentEvent::MessageEnd {
                message: message.clone(),
            });
            messages.push(message.clone());
            new_messages.push(message);
        }
    }

    /// Handle a streaming error by synthesizing an error message and completing the turn.
    async fn handle_streaming_error(
        &self,
        e: anyhow::Error,
        messages: &mut Vec<Message>,
        new_messages: &mut Vec<Message>,
        events: &mut Vec<AgentEvent>,
        emit: &EmitFn,
        turn_number: u32,
    ) -> (Vec<Message>, Vec<AgentEvent>) {
        let err_msg = format!("{}", e);
        tracing::error!(session_id = ?self.session_id, "Unexpected streaming error: {}", err_msg);

        let mut error_asst = oxi_ai::AssistantMessage::new(
            oxi_ai::Api::OpenAiCompletions,
            "agent",
            &self.config.model_id,
        );
        error_asst.stop_reason = StopReason::Error;
        error_asst
            .content
            .push(ContentBlock::Text(TextContent::new(format!(
                "⚠ {}",
                err_msg
            ))));

        new_messages.push(Message::Assistant(error_asst.clone()));
        messages.push(Message::Assistant(error_asst.clone()));

        emit(AgentEvent::MessageStart {
            message: Message::Assistant(error_asst.clone()),
        });
        emit(AgentEvent::MessageEnd {
            message: Message::Assistant(error_asst.clone()),
        });
        emit(AgentEvent::Error {
            message: err_msg.clone(),
            session_id: self.session_id.clone(),
        });

        emit(AgentEvent::TurnEnd {
            turn_number,
            assistant_message: Message::Assistant(error_asst.clone()),
            tool_results: vec![],
        });
        events.push(AgentEvent::TurnEnd {
            turn_number,
            assistant_message: Message::Assistant(error_asst),
            tool_results: vec![],
        });
        // Return Ok — lifecycle is complete
        (messages.clone(), events.clone())
    }

    async fn run_loop(
        &self,
        initial_prompts: Vec<Message>,
        emit: EmitFn,
    ) -> Result<(Vec<Message>, Vec<AgentEvent>)> {
        tracing::info!("[AGENT-LOOP] run_loop started");
        let mut messages = self.state.get_state().messages.clone();
        messages.extend(initial_prompts.clone());

        let mut new_messages: Vec<Message> = initial_prompts;
        let mut events = Vec::new();
        let mut turn_number: u32 = 0;
        let mut first_turn = true;

        let mut pending_messages: Vec<Message> = self.drain_steering_queue();

        loop {
            tracing::info!(
                "[AGENT-LOOP] Top of loop, has_more_tool_calls={}, pending_messages={}",
                true,
                pending_messages.is_empty()
            );
            let mut has_more_tool_calls = true;

            while has_more_tool_calls || !pending_messages.is_empty() {
                if !first_turn {
                    turn_number += 1;
                    emit(AgentEvent::TurnStart { turn_number });
                    events.push(AgentEvent::TurnStart { turn_number });
                } else {
                    first_turn = false;
                    turn_number = 1;
                    emit(AgentEvent::TurnStart { turn_number });
                    events.push(AgentEvent::TurnStart { turn_number });
                }

                if !pending_messages.is_empty() {
                    self.process_steering_messages(
                        &mut pending_messages,
                        &mut messages,
                        &mut new_messages,
                        &mut events,
                        &emit,
                    );
                }

                // Poll external hooks each turn to drain new steering/follow-up
                // messages injected since the last turn.
                self.poll_external_queues();

                self.maybe_compact(&mut messages, turn_number as usize, &emit)
                    .await;

                tracing::info!("[AGENT-LOOP] About to call stream_assistant_response");
                let ttsr = self.ttsr_engine.as_deref();
                let outcome = stream_assistant_response(self, &mut messages, &emit, ttsr).await;

                let assistant_message = match outcome {
                    StreamOutcome::Complete(msg) => msg,
                    StreamOutcome::Error {
                        message: _message,
                        detail,
                    } => {
                        // Check for message-ordering errors that can be recovered
                        // by removing orphaned tool results.
                        let is_tool_ordering_error = detail.contains("tool")
                            && (detail.contains("must be a response")
                                || detail.contains("preceding")
                                || detail.contains("tool_calls"));

                        if is_tool_ordering_error {
                            let removed = sanitize_orphaned_tool_results(&mut messages);
                            tracing::warn!(
                                session_id = ?self.session_id,
                                removed,
                                detail = %detail,
                                "Message-ordering error detected, removed orphaned tool results, retrying"
                            );
                            if removed > 0 {
                                // Don't push the error message to history; retry the turn.
                                emit(AgentEvent::Error {
                                    message: format!(
                                        "⚠ Provider rejected message order: {}. Removed {} orphaned tool results, retrying…",
                                        detail, removed
                                    ),
                                    session_id: self.session_id.clone(),
                                });
                                continue; // Retry the turn with sanitized messages
                            }
                        }

                        // Unrecoverable — fall through to error handler.
                        return Ok(self
                            .handle_streaming_error(
                                anyhow::anyhow!("Provider stream error: {}", detail),
                                &mut messages,
                                &mut new_messages,
                                &mut events,
                                &emit,
                                turn_number,
                            )
                            .await);
                    }
                    StreamOutcome::Cancelled(msg) => {
                        emit(AgentEvent::TurnEnd {
                            turn_number,
                            assistant_message: Message::Assistant(msg.clone()),
                            tool_results: vec![],
                        });
                        return Ok((messages, events));
                    }
                    StreamOutcome::RuleInterrupt { partial, rule } => {
                        tracing::info!("RuleInterrupt: '{}' violated, retrying", rule.name);
                        emit(AgentEvent::TtsrInterrupt {
                            rule_name: rule.name.clone(),
                            session_id: self.session_id.clone(),
                        });
                        messages.push(Message::Assistant(partial));
                        let interrupt_body = format!(
                            "<system-interrupt reason=\"rule_violation\" rule=\"{name}\">\n\
                             Your output was interrupted because it violated a project rule.\n\
                             Comply with: {content}\n</system-interrupt>",
                            name = rule.name,
                            content = rule.content
                        );
                        messages.push(Message::user(interrupt_body));
                        continue;
                    }
                };

                new_messages.push(Message::Assistant(assistant_message.clone()));

                if matches!(assistant_message.stop_reason, StopReason::Error) {
                    if is_retryable_error(&assistant_message) {
                        let did_retry =
                            handle_retryable_error(self, &assistant_message, &mut messages, &emit)
                                .await;
                        if did_retry {
                            emit(AgentEvent::TurnEnd {
                                turn_number,
                                assistant_message: Message::Assistant(assistant_message.clone()),
                                tool_results: vec![],
                            });
                            events.push(AgentEvent::TurnEnd {
                                turn_number,
                                assistant_message: Message::Assistant(assistant_message.clone()),
                                tool_results: vec![],
                            });
                            has_more_tool_calls = true;
                            continue;
                        }
                    }

                    emit(AgentEvent::TurnEnd {
                        turn_number,
                        assistant_message: Message::Assistant(assistant_message.clone()),
                        tool_results: vec![],
                    });
                    events.push(AgentEvent::TurnEnd {
                        turn_number,
                        assistant_message: Message::Assistant(assistant_message.clone()),
                        tool_results: vec![],
                    });
                    return Ok((messages, events));
                }
                if matches!(assistant_message.stop_reason, StopReason::Aborted) {
                    if self.auto_retry_attempt.load(Ordering::Relaxed) > 0 {
                        emit(AgentEvent::AutoRetryEnd {
                            success: true,
                            attempt: self.auto_retry_attempt.load(Ordering::Relaxed),
                            final_error: None,
                        });
                        self.auto_retry_attempt.store(0, Ordering::Relaxed);
                    }

                    emit(AgentEvent::TurnEnd {
                        turn_number,
                        assistant_message: Message::Assistant(assistant_message.clone()),
                        tool_results: vec![],
                    });
                    events.push(AgentEvent::TurnEnd {
                        turn_number,
                        assistant_message: Message::Assistant(assistant_message.clone()),
                        tool_results: vec![],
                    });
                    return Ok((messages, events));
                }

                if self.auto_retry_attempt.load(Ordering::Relaxed) > 0 {
                    emit(AgentEvent::AutoRetryEnd {
                        success: true,
                        attempt: self.auto_retry_attempt.load(Ordering::Relaxed),
                        final_error: None,
                    });
                    self.auto_retry_attempt.store(0, Ordering::Relaxed);
                }

                let tool_calls = helpers::extract_tool_calls(&assistant_message);
                tracing::info!(
                    "[AGENT-LOOP] extract_tool_calls found {} calls, stop_reason={:?}",
                    tool_calls.len(),
                    assistant_message.stop_reason
                );

                let mut tool_results: Vec<oxi_ai::ToolResultMessage> = Vec::new();
                has_more_tool_calls = false;

                if !tool_calls.is_empty() {
                    tracing::info!("[AGENT-LOOP] Executing {} tool calls", tool_calls.len());
                    let ctx = self.build_tool_context();
                    let executed_batch = match execute_tool_calls(
                        self,
                        &mut messages,
                        &assistant_message,
                        tool_calls,
                        &emit,
                        &ctx,
                    )
                    .await
                    {
                        Ok(batch) => batch,
                        Err(e) => {
                            // Tool execution failed — emit TurnEnd and return Ok.
                            // The lifecycle must always complete.
                            tracing::error!(session_id = ?self.session_id, "Tool execution error: {}", e);
                            emit(AgentEvent::Error {
                                message: format!("Tool execution error: {}", e),
                                session_id: self.session_id.clone(),
                            });
                            emit(AgentEvent::TurnEnd {
                                turn_number,
                                assistant_message: Message::Assistant(assistant_message.clone()),
                                tool_results: vec![],
                            });
                            events.push(AgentEvent::TurnEnd {
                                turn_number,
                                assistant_message: Message::Assistant(assistant_message.clone()),
                                tool_results: vec![],
                            });
                            return Ok((messages, events));
                        }
                    };

                    tool_results = executed_batch.messages;
                    has_more_tool_calls = !executed_batch.terminate;

                    if executed_batch.terminate {
                        tracing::warn!(
                            session_id = ?self.session_id,
                            "Tool batch terminated early (terminate flag set by after_tool_call hook). \
                             This halts the tool-calling loop. If this is unexpected, \
                             check after_tool_call hooks for unintended terminate: true."
                        );
                    }

                    for result in &tool_results {
                        let result = self.maybe_truncate_tool_result(result.clone());
                        messages.push(Message::ToolResult(result.clone()));
                        new_messages.push(Message::ToolResult(result));
                    }
                    // Feed completed turn to the tool-call loop guard (omp pattern).
                    if has_more_tool_calls {
                        use oxi_ai::utils::tool_call_loop::{
                            ToolCallLoopTurn, ToolCallRef, ToolResultRef,
                        };
                        let call_refs: Vec<ToolCallRef> = assistant_message
                            .content
                            .iter()
                            .filter_map(|block| match block {
                                oxi_ai::ContentBlock::ToolCall(tc) => Some(ToolCallRef {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    arguments: tc.arguments.clone(),
                                }),
                                _ => None,
                            })
                            .collect();
                        let result_refs: Vec<ToolResultRef> = tool_results
                            .iter()
                            .map(|tr| {
                                let text: String = tr
                                    .content
                                    .iter()
                                    .filter_map(|b| match b {
                                        oxi_ai::ContentBlock::Text(t) => Some(t.text.as_str()),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                ToolResultRef {
                                    tool_call_id: tr.tool_call_id.clone(),
                                    content: text,
                                }
                            })
                            .collect();
                        let turn = ToolCallLoopTurn {
                            tool_calls: &call_refs,
                            tool_results: &result_refs,
                        };
                        // NOTE: assign to a variable first so the MutexGuard
                        // from lock() is dropped at the semicolon. The if-let
                        // pattern would keep the guard alive inside the block,
                        // and calling reset() inside would deadlock on the
                        // non-reentrant parking_lot::Mutex.
                        let detection = self.tool_call_loop_guard.lock().record_turn(turn);
                        if let Some(detection) = detection {
                            let steering = format!(
                                "Tool-call loop detected: '{}' called {} consecutive \
                                 times with identical arguments. Result: '{}'. \
                                 Try a different approach.",
                                detection.tool_name, detection.count, detection.result_summary,
                            );
                            let msg = Message::User(oxi_ai::UserMessage::new(steering));
                            messages.push(msg.clone());
                            new_messages.push(msg);
                            tracing::warn!(
                                session_id = ?self.session_id,
                                tool = %detection.tool_name,
                                count = detection.count,
                                "tool-call loop detected; injecting steering message"
                            );
                            self.tool_call_loop_guard.lock().reset();
                        }
                    }
                }

                emit(AgentEvent::TurnEnd {
                    turn_number,
                    assistant_message: Message::Assistant(assistant_message.clone()),
                    tool_results: tool_results.clone(),
                });
                events.push(AgentEvent::TurnEnd {
                    turn_number,
                    assistant_message: Message::Assistant(assistant_message.clone()),
                    tool_results: tool_results.clone(),
                });

                if should_stop_after_turn(&self.external_stop) {
                    tracing::info!("[AGENT-LOOP] external_stop, ending loop");
                    return Ok((messages, events));
                }

                pending_messages = self.drain_steering_queue();
                tracing::info!(
                    "[AGENT-LOOP] TurnEnd complete, pending_messages={}, has_more_tool_calls={}",
                    !pending_messages.is_empty(),
                    has_more_tool_calls
                );

                // Early stop check: if external_stop was set (e.g. Ctrl+C),
                // don't process steering messages from the next turn.
                if self.external_stop.load(Ordering::SeqCst) {
                    tracing::info!(
                        "[AGENT-LOOP] external_stop set after steering drain, ending loop"
                    );
                    return Ok((messages, events));
                }
            }

            // Re-check steering queue after the inner while loop exits.
            // This closes the race window where steer() is called between the
            // last drain_steering_queue() and the while-exit condition check.
            let late_steering = self.drain_steering_queue();
            if !late_steering.is_empty() {
                tracing::info!(
                    count = late_steering.len(),
                    "[AGENT-LOOP] Caught late steering messages after inner loop exit"
                );
                pending_messages = late_steering;
                continue;
            }

            let follow_up_messages = self.drain_follow_up_queue();
            if !follow_up_messages.is_empty() {
                pending_messages = follow_up_messages;
                continue;
            }

            // Final check: one more steering drain after follow-up to catch
            // messages injected during the follow-up drain window.
            let final_steering = self.drain_steering_queue();
            if !final_steering.is_empty() {
                pending_messages = final_steering;
                continue;
            }

            break;
        }

        Ok((messages, events))
    }

    /// Build the compaction instruction, appending injected TTSR rule
    /// names so that the model remembers rules already enforced.
    fn build_compaction_instruction(&self) -> Option<String> {
        let base = self.config.compaction_instruction.as_deref();
        let injected = self
            .ttsr_engine
            .as_ref()
            .map(|e| e.injected_records())
            .unwrap_or_default();
        if injected.is_empty() {
            return base.map(|s| s.to_string());
        }
        let mut instr = base.map(|s| s.to_string()).unwrap_or_default();
        instr.push_str("\n\nThe following rules have already been enforced in this session and corrections applied. Do NOT violate them again:");
        for (name, _turn) in &injected {
            instr.push_str(&format!("\n- {name}"));
        }
        Some(instr)
    }

    async fn maybe_compact(&self, messages: &mut Vec<Message>, iteration: usize, emit: &EmitFn) {
        // Decide the context-size value to drive compaction with. Prefer
        // the provider-reported `last_input_tokens` (ground truth) over
        // the legacy `bytes/4` heuristic. The heuristic can undercount
        // by 3-4× on token-dense content (base64, JSON, CJK) and is the
        // reason `CompactionStrategy::Threshold` was effectively a no-op
        // in issue #28's failure (35k estimated vs 122k actual).
        //
        // The provider count lags by exactly one turn: `maybe_compact`
        // runs at the **top** of a turn, before streaming. So the
        // `last_input_tokens` we read here reflects the *previous*
        // turn's `Done` event. The drift is at most the size of the
        // tool results the model is about to receive, which is small
        // relative to the failure-mode drift the heuristic suffers.
        // For turn 1 there is no prior count, so we fall back to the
        // heuristic on cold start.
        let snapshot = self.state.get_state();
        let (context_tokens, source_label) = match snapshot.current_token_source() {
            TokenSource::Real(n) => (n, "provider-reported"),
            TokenSource::Heuristic(n) => (n, "bytes/4 heuristic (cold start)"),
            TokenSource::None => (0, "empty"),
        };
        // Surface heuristic drift as a warning when the operator has
        // observed at least one provider count and it diverges from
        // the estimate by more than 2×. This is the diagnostic path
        // from #28's "Proposed fix" option 3.
        if let Some(div) = snapshot.last_estimate_divergence
            && div > 2.0
        {
            tracing::warn!(
                session_id = ?self.session_id,
                divergence = div,
                reported = snapshot.last_input_tokens.unwrap_or(0),
                estimate = snapshot.last_estimate_at_report.unwrap_or(0),
                "Token-count heuristic (bytes/4) diverges from provider-reported usage \
                 by >2x; CompactionStrategy::Threshold decisions are using the \
                 provider-reported count (issue #28 gap 2)."
            );
        }
        drop(snapshot);

        if !self
            .compaction_manager
            .should_compact(context_tokens, iteration)
        {
            return;
        }

        emit(AgentEvent::Compaction {
            event: CompactionEvent::Triggered {
                context_tokens,
                iteration,
                source: source_label.to_string(),
            },
        });

        let messages_to_compact: Vec<Message> = messages.to_vec();
        let instruction = self.build_compaction_instruction();

        match self
            .compaction_manager
            .compact_if_needed(
                &messages_to_compact,
                instruction.as_deref(),
                context_tokens,
                iteration,
            )
            .await
        {
            Ok(Some(compacted)) => {
                let start = Instant::now();
                let message_count = compacted.compacted_count;

                emit(AgentEvent::Compaction {
                    event: CompactionEvent::Started { message_count },
                });

                let kept_messages = compacted.kept_messages;
                let summary = compacted.summary;
                let compacted_count = compacted.compacted_count;

                *messages = kept_messages;

                let state_msgs = messages.clone();
                self.state.update(|s| {
                    s.replace_messages(state_msgs);
                });

                let compacted_ctx = CompactedContext {
                    summary,
                    kept_messages: Vec::new(),
                    compacted_count,
                };
                emit(AgentEvent::Compaction {
                    event: CompactionEvent::Completed {
                        result: compacted_ctx.clone(),
                        duration_ms: start.elapsed().as_millis() as u64,
                    },
                });

                // Async compaction hook — awaited, not fire-and-forget.
                if let Some(ref hook) = self.config.on_compaction {
                    match hook(compacted_ctx).await {
                        Ok(()) => {
                            tracing::debug!("Compaction hook completed successfully");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Compaction hook failed");
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                emit(AgentEvent::Compaction {
                    event: CompactionEvent::Failed {
                        error: e.to_string(),
                    },
                });
            }
        }
    }

    fn resolve_model(&self) -> Result<oxi_ai::Model> {
        self.resolver
            .resolve_model(&self.config.model_id)
            .ok_or_else(|| Error::msg(format!("Model not found: {}", self.config.model_id)))
    }
}

#[cfg(test)]
mod session_id_wiring_tests {
    //! Regression coverage for the #13 fix.
    //! `build_tool_context` is private; testing it here keeps the test in the
    //! same module so it can reach private surface. We never stream — the
    //! nop provider only exists to satisfy `AgentLoop::new_with_resolver`.
    use super::*;
    use crate::ProviderResolver;
    use crate::agent_loop::config::AgentLoopConfig;
    use crate::config::ToolExecutionMode;
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::{
        CompactionStrategy, Context, Model, Provider, ProviderError, StreamOptions, StreamResult,
    };
    use std::future::Future;
    use std::pin::Pin;

    struct NopProvider;
    impl Provider for NopProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a Model,
            _context: &'a Context,
            _options: Option<StreamOptions>,
        ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
            Box::pin(async {
                Err(ProviderError::NotImplemented(
                    "session-id wiring tests never stream".to_string(),
                ))
            })
        }
        fn name(&self) -> &str {
            "nop"
        }
    }

    struct NullResolver;
    impl ProviderResolver for NullResolver {
        fn resolve_provider(&self, _name: &str) -> Option<Arc<dyn Provider>> {
            None
        }
        fn resolve_model(&self, _model_id: &str) -> Option<Model> {
            None
        }
    }

    fn loop_with(session_id: Option<String>) -> AgentLoop {
        let config = AgentLoopConfig {
            model_id: "test/model".to_string(),
            system_prompt: None,
            temperature: 1.0,
            max_tokens: 4096,
            tool_execution: ToolExecutionMode::Sequential,
            compaction_strategy: CompactionStrategy::Disabled,
            compaction_instruction: None,
            context_window: 128_000,
            session_id,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
            auto_retry_enabled: true,
            auto_retry_max_attempts: 3,
            auto_retry_base_delay_ms: 1000,
            workspace_dir: None,
            provider_options: None,
            on_compaction: None,
            snapshot_store: None,
            memory: None,
            url_resolver: None,
            todo: None,
            agent_pool: None,
            lsp: None,
            ttsr_engine: None,
            subagent_runner: None,
            subagent_depth: 0,
            max_tool_result_bytes: None,
            thinking_loop_detection: false, // disable for unit tests
            ..Default::default()
        };
        AgentLoop::new_with_resolver(
            Arc::new(NopProvider),
            config,
            Arc::new(ToolRegistry::new()),
            SharedState::new(),
            Arc::new(NullResolver),
        )
    }

    /// Regression for defect #13: `AgentLoopConfig.session_id` MUST flow into
    /// `ToolContext.session_id`. Before the fix, the field was hardcoded to
    /// `None`, so the `issue` tool received an empty caller id and bypassed
    /// all ownership/liveness checks (two agents could both `start` the same
    /// issue and the last writer silently won).
    #[test]
    fn tool_context_inherits_session_id_when_set() {
        let loop_ = loop_with(Some("proc-test-session-id".to_string()));
        let ctx = loop_.build_tool_context();
        assert_eq!(
            ctx.session_id.as_deref(),
            Some("proc-test-session-id"),
            "ToolContext.session_id must inherit AgentConfig.session_id"
        );
    }

    #[test]
    fn tool_context_session_id_defaults_to_none() {
        let loop_ = loop_with(None);
        let ctx = loop_.build_tool_context();
        assert!(
            ctx.session_id.is_none(),
            "default ToolContext.session_id should be None"
        );
    }
}

// ── Gap 1: tool-result truncation tests (issue #28) ──────────────────

#[cfg(test)]
mod truncation_tests {
    use super::*;
    use crate::agent::ProviderResolver;
    use oxi_ai::{
        ContentBlock, Context, Model, Provider, ProviderError, StreamOptions, StreamResult,
        TextContent, ToolResultMessage,
    };
    use std::future::Future;
    use std::pin::Pin;

    struct NopProvider;
    impl Provider for NopProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a Model,
            _context: &'a Context,
            _options: Option<StreamOptions>,
        ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
            Box::pin(async {
                Err(ProviderError::NotImplemented(
                    "truncation tests never stream".to_string(),
                ))
            })
        }
        fn name(&self) -> &str {
            "nop"
        }
    }

    struct NullResolver;
    impl ProviderResolver for NullResolver {
        fn resolve_provider(&self, _name: &str) -> Option<Arc<dyn Provider>> {
            None
        }
        fn resolve_model(&self, _model_id: &str) -> Option<Model> {
            None
        }
    }

    fn make_result(text: &str) -> ToolResultMessage {
        ToolResultMessage::new(
            "tc_test".to_string(),
            "test_tool",
            vec![ContentBlock::Text(TextContent::new(text.to_string()))],
        )
    }

    fn loop_with_limit(limit: Option<usize>) -> AgentLoop {
        let config = AgentLoopConfig {
            model_id: "test/model".to_string(),
            max_tool_result_bytes: limit,
            ..Default::default()
        };
        AgentLoop::new_with_resolver(
            Arc::new(NopProvider),
            config,
            Arc::new(ToolRegistry::new()),
            SharedState::new(),
            Arc::new(NullResolver),
        )
    }

    #[test]
    fn truncate_passthrough_when_none() {
        let loop_ = loop_with_limit(None);
        let result = make_result(&"x".repeat(10_000));
        let truncated = loop_.maybe_truncate_tool_result(result);
        if let ContentBlock::Text(tc) = &truncated.content[0] {
            assert_eq!(tc.text.len(), 10_000);
            assert!(!tc.text.contains("truncated"));
        }
    }

    #[test]
    fn truncate_passthrough_when_under_limit() {
        let loop_ = loop_with_limit(Some(1000));
        let result = make_result(&"x".repeat(500));
        let truncated = loop_.maybe_truncate_tool_result(result);
        if let ContentBlock::Text(tc) = &truncated.content[0] {
            assert_eq!(tc.text.len(), 500);
            assert!(!tc.text.contains("truncated"));
        }
    }

    #[test]
    fn truncate_applies_when_over_limit() {
        let loop_ = loop_with_limit(Some(100));
        let result = make_result(&"x".repeat(500));
        let truncated = loop_.maybe_truncate_tool_result(result);
        if let ContentBlock::Text(tc) = &truncated.content[0] {
            assert!(
                tc.text.len() < 500,
                "text not truncated: {} bytes",
                tc.text.len()
            );
            assert!(tc.text.contains("truncated"), "missing truncation marker");
            assert!(tc.text.contains("400 bytes omitted"));
        }
    }
}
