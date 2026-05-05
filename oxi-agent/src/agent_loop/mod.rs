//! Agent loop implementation

pub mod config;
pub mod tool_exec;
pub mod streaming;
pub mod retry;
pub mod queues;
pub mod helpers;

use crate::compaction::{CompactedContext, CompactionEvent};
use crate::events::AgentEvent;
use crate::recovery::{CircuitBreaker, CircuitBreakerConfig};
use crate::{AgentToolResult, error::AgentError, state::SharedState, tools::ToolRegistry};
use anyhow::{Error, Result};
use oxi_ai::{
    ContentBlock, Message, Provider, ProviderEvent, StreamOptions,
    StopReason, TextContent, ToolCall, UserMessage, CompactionStrategy,
    CompactionManager as OxCompactionManager, AssistantMessage,
    estimate_tokens, LlmCompactor,
};
use parking_lot::RwLock;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use config::{AgentLoopConfig, BeforeToolCallHook, AfterToolCallHook, ToolExecutionMode};
use self::queues::{drain_steering_queue, drain_follow_up_queue, clear_steering_queue, clear_follow_up_queue, clear_all_queues};
use self::retry::{stream_with_retry, is_retryable_error, handle_retryable_error, cancel_auto_retry, auto_retry_attempt_method};
use self::streaming::stream_assistant_response;
use self::helpers::should_stop_after_turn;
use self::tool_exec::{execute_tool_calls, should_terminate_batch, create_tool_result_message};

type EmitFn = Arc<dyn Fn(AgentEvent) + Send + Sync>;

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
    auto_retry_cancel: RwLock<bool>,
    circuit_breaker: CircuitBreaker,
}

impl AgentLoop {
    pub fn new(
        provider: Arc<dyn Provider>,
        config: AgentLoopConfig,
        tools: Arc<ToolRegistry>,
        state: SharedState,
    ) -> Self {
        let mut compaction_manager = OxCompactionManager::new(
            config.compaction_strategy.clone(),
            config.context_window,
        );

        if config.compaction_strategy != CompactionStrategy::Disabled {
            let model = crate::model_id::resolve_model_from_id(&config.model_id);
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
            auto_retry_cancel: RwLock::new(false),
            circuit_breaker: CircuitBreaker::new(CircuitBreakerConfig::default()),
        }
    }

    pub fn with_before_tool_call(mut self, hook: BeforeToolCallHook) -> Self {
        self.before_tool_call = Some(hook);
        self
    }

    pub fn with_after_tool_call(mut self, hook: AfterToolCallHook) -> Self {
        self.after_tool_call = Some(hook);
        self
    }

    pub fn steer(&self, message: Message) {
        self.steering_queue.write().push(message);
    }

    pub fn follow_up(&self, message: Message) {
        self.follow_up_queue.write().push(message);
    }

    pub fn clear_steering_queue(&self) {
        clear_steering_queue(self);
    }

    pub fn clear_follow_up_queue(&self) {
        clear_follow_up_queue(self);
    }

    pub fn clear_all_queues(&self) {
        clear_all_queues(self);
    }

    fn drain_steering_queue(&self) -> Vec<Message> {
        drain_steering_queue(self)
    }

    fn drain_follow_up_queue(&self) -> Vec<Message> {
        drain_follow_up_queue(self)
    }

    pub fn cancel_auto_retry(&self) {
        cancel_auto_retry(self);
    }

    pub fn auto_retry_attempt(&self) -> usize {
        auto_retry_attempt_method(self)
    }

    pub async fn run(
        &self,
        prompt: String,
        emit: impl Fn(AgentEvent) + Send + Sync + 'static,
    ) -> Result<Vec<AgentEvent>> {
        let message = Message::User(UserMessage::new(prompt));
        let emit = Arc::new(emit);
        self.run_messages(vec![message], emit).await
    }

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
        emit(AgentEvent::AgentStart { prompts: prompts.clone(), session_id: self.session_id.clone() });
        all_events.push(AgentEvent::AgentStart { prompts: prompts.clone(), session_id: self.session_id.clone() });

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

    pub async fn continue_loop(
        &self,
        emit: impl Fn(AgentEvent) + Send + Sync + 'static,
    ) -> Result<Vec<AgentEvent>> {
        let emit = Arc::new(emit);
        let mut all_events = Vec::new();

        tracing::info!(session_id = ?self.session_id, "AgentLoop continuing");
        emit(AgentEvent::AgentStart { prompts: vec![], session_id: self.session_id.clone() });
        all_events.push(AgentEvent::AgentStart { prompts: vec![], session_id: self.session_id.clone() });

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

    async fn run_loop(
        &self,
        initial_prompts: Vec<Message>,
        emit: EmitFn,
    ) -> Result<(Vec<Message>, Vec<AgentEvent>)> {
        let mut messages = self.state.get_state().messages.clone();
        messages.extend(initial_prompts.clone());

        let mut new_messages: Vec<Message> = initial_prompts;
        let mut events = Vec::new();
        let mut turn_number: u32 = 0;
        let mut first_turn = true;

        let mut pending_messages: Vec<Message> = self.drain_steering_queue();

        loop {
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
                    for message in pending_messages.drain(..) {
                        emit(AgentEvent::SteeringMessage { message: message.clone() });
                        emit(AgentEvent::MessageStart { message: message.clone() });
                        emit(AgentEvent::MessageEnd { message: message.clone() });
                        events.push(AgentEvent::SteeringMessage { message: message.clone() });
                        events.push(AgentEvent::MessageStart { message: message.clone() });
                        events.push(AgentEvent::MessageEnd { message: message.clone() });
                        messages.push(message.clone());
                        new_messages.push(message);
                    }
                    pending_messages = Vec::new();
                }

                self.maybe_compact(&mut messages, turn_number as usize, &emit).await;

                let assistant_message = match stream_assistant_response(self, &mut messages, &emit).await {
                    Ok(msg) => msg,
                    Err(e) => {
                        let err_msg = format!("{:?}", e);
                        emit(AgentEvent::Error { message: err_msg.clone(), session_id: self.session_id.clone() });
                        events.push(AgentEvent::Error { message: err_msg, session_id: self.session_id.clone() });
                        return Err(Error::msg(e));
                    }
                };

                new_messages.push(Message::Assistant(assistant_message.clone()));

                if matches!(assistant_message.stop_reason, StopReason::Error) {
                    if is_retryable_error(&assistant_message) {
                        let did_retry = handle_retryable_error(self, &assistant_message, &mut messages, &emit).await;
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

                let mut tool_results: Vec<oxi_ai::ToolResultMessage> = Vec::new();
                has_more_tool_calls = false;

                if !tool_calls.is_empty() {
                    let executed_batch = execute_tool_calls(
                        self,
                        &mut messages,
                        &assistant_message,
                        tool_calls,
                        &emit,
                    ).await?;

                    tool_results = executed_batch.messages;
                    has_more_tool_calls = !executed_batch.terminate;

                    for result in &tool_results {
                        messages.push(Message::ToolResult(result.clone()));
                        new_messages.push(Message::ToolResult(result.clone()));
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

                if should_stop_after_turn(&messages, &assistant_message, self.config.max_iterations) {
                    return Ok((messages, events));
                }

                pending_messages = self.drain_steering_queue();
            }

            let follow_up_messages = self.drain_follow_up_queue();
            if !follow_up_messages.is_empty() {
                pending_messages = follow_up_messages;
                continue;
            }

            break;
        }

        Ok((messages, events))
    }

    async fn maybe_compact(
        &self,
        messages: &mut Vec<Message>,
        iteration: usize,
        emit: &EmitFn,
    ) {
        let context_text = serde_json::to_string(&*messages).unwrap_or_default();
        let context_tokens = estimate_tokens(&context_text);

        if !self.compaction_manager.should_compact(context_tokens, iteration) {
            return;
        }

        emit(AgentEvent::Compaction {
            event: CompactionEvent::Triggered {
                context_tokens,
                iteration,
            },
        });

        let messages_to_compact: Vec<Message> = messages.iter().cloned().collect();
        let instruction = self.config.compaction_instruction.as_deref();

        match self
            .compaction_manager
            .compact_if_needed(&messages_to_compact, instruction, context_tokens, iteration)
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
                        result: compacted_ctx,
                        duration_ms: start.elapsed().as_millis() as u64,
                    },
                });
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
        crate::model_id::resolve_model_from_id(&self.config.model_id)
            .ok_or_else(|| Error::msg(format!("Model not found: {}", self.config.model_id)))
    }
}