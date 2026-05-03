//! Agent loop implementation

use crate::{
    error::AgentError,
    events::AgentEvent,
    state::SharedState,
    tools::{AgentTool, ToolRegistry},
    AgentToolResult,
};
use anyhow::{Error, Result};
use futures::StreamExt;
use oxi_ai::{
    AssistantMessage, CompactionManager as OxCompactionManager, CompactionStrategy, ContentBlock,
    Context, Message, Provider, ProviderEvent, StopReason, StreamOptions, TextContent, ToolCall,
    UserMessage,
};
use parking_lot::RwLock;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;

const MAX_RETRIES: usize = 3;
const BACKOFF_BASE_SECS: u64 = 2;

#[derive(Clone)]
pub struct AgentLoopConfig {
    pub model_id: String,
    pub system_prompt: Option<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_iterations: usize,
    pub tool_execution: ToolExecutionMode,
    pub compaction_strategy: CompactionStrategy,
    pub context_window: usize,
    pub compaction_instruction: Option<String>,
    pub session_id: Option<String>,
    pub transport: Option<String>,
    pub compact_on_start: bool,
    pub max_retry_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Parallel,
    Sequential,
}

impl Default for ToolExecutionMode {
    fn default() -> Self {
        ToolExecutionMode::Parallel
    }
}

pub type BeforeToolCallHook = Arc<
    dyn Fn(
            &str,
            &Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Option<AgentToolResult>, Error>> + Send>,
        > + Send
        + Sync,
>;

pub type AfterToolCallHook = Arc<
    dyn Fn(
            &str,
            &AgentToolResult,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Option<AgentToolResult>, Error>> + Send>,
        > + Send
        + Sync,
>;

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
}

impl AgentLoop {
    pub fn new(
        provider: Arc<dyn Provider>,
        config: AgentLoopConfig,
        tools: Arc<ToolRegistry>,
        state: SharedState,
    ) -> Self {
        let compaction_manager =
            OxCompactionManager::new(config.compaction_strategy.clone(), config.context_window);

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
        self.steering_queue.write().clear();
    }

    pub fn clear_follow_up_queue(&self) {
        self.follow_up_queue.write().clear();
    }

    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    fn drain_steering_queue(&self) -> Vec<Message> {
        let mut queue = self.steering_queue.write();
        queue.drain(..).collect()
    }

    fn drain_follow_up_queue(&self) -> Vec<Message> {
        let mut queue = self.follow_up_queue.write();
        queue.drain(..).collect()
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

        emit(AgentEvent::AgentStart {
            prompts: prompts.clone(),
        });
        all_events.push(AgentEvent::AgentStart { prompts });

        let (result_messages, events) = self.run_loop(prompts, emit.clone()).await?;

        all_events.extend(events);

        let stop_reason = result_messages.last().and_then(|m| {
            if let Message::Assistant(a) = m {
                Some(format!("{:?}", a.stop_reason))
            } else {
                None
            }
        });

        emit(AgentEvent::AgentEnd {
            messages: result_messages.clone(),
            stop_reason: stop_reason.clone(),
        });
        all_events.push(AgentEvent::AgentEnd {
            messages: result_messages.clone(),
            stop_reason,
        });

        Ok(all_events)
    }

    pub async fn continue_loop(
        &self,
        emit: impl Fn(AgentEvent) + Send + Sync + 'static,
    ) -> Result<Vec<AgentEvent>> {
        let emit = Arc::new(emit);
        let mut all_events = Vec::new();

        emit(AgentEvent::AgentStart { prompts: vec![] });
        all_events.push(AgentEvent::AgentStart { prompts: vec![] });

        let (result_messages, events) = self.run_loop(vec![], emit.clone()).await?;

        all_events.extend(events);

        let stop_reason = result_messages.last().and_then(|m| {
            if let Message::Assistant(a) = m {
                Some(format!("{:?}", a.stop_reason))
            } else {
                None
            }
        });

        emit(AgentEvent::AgentEnd {
            messages: result_messages.clone(),
            stop_reason: stop_reason.clone(),
        });
        all_events.push(AgentEvent::AgentEnd {
            messages: result_messages.clone(),
            stop_reason,
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
                    pending_messages = Vec::new();
                }

                let assistant_message =
                    match self.stream_assistant_response(&mut messages, &emit).await {
                        Ok(msg) => msg,
                        Err(e) => {
                            let err_msg = format!("{:?}", e);
                            emit(AgentEvent::Error {
                                message: err_msg.clone(),
                            });
                            events.push(AgentEvent::Error { message: err_msg });
                            return Err(Error::msg(e));
                        }
                    };

                new_messages.push(Message::Assistant(assistant_message.clone()));

                if matches!(
                    assistant_message.stop_reason,
                    StopReason::Error | StopReason::Aborted
                ) {
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

                let tool_calls = self.extract_tool_calls(&assistant_message);

                let mut tool_results: Vec<oxi_ai::ToolResultMessage> = Vec::new();
                has_more_tool_calls = false;

                if !tool_calls.is_empty() {
                    let executed_batch = self
                        .execute_tool_calls(&mut messages, &assistant_message, tool_calls, &emit)
                        .await?;

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

                if self.should_stop_after_turn(&messages, &assistant_message) {
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

    async fn stream_assistant_response(
        &self,
        messages: &mut Vec<Message>,
        emit: &EmitFn,
    ) -> Result<AssistantMessage> {
        let model = self.resolve_model()?;

        let mut context = Context::new();

        if let Some(ref system_prompt) = self.config.system_prompt {
            context.set_system_prompt(system_prompt.clone());
        }

        for msg in messages.iter() {
            context.add_message(msg.clone());
        }

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
            temperature: Some(self.config.temperature as f64),
            max_tokens: Some(self.config.max_tokens as usize),
            ..Default::default()
        };

        let stream = self
            .stream_with_retry(&model, &context, Some(stream_options), emit)
            .await?;

        let mut partial_message: Option<AssistantMessage> = None;
        let mut added_partial = false;

        let mut rx = stream;
        while let Some(event) = rx.next().await {
            match event {
                ProviderEvent::Start { partial } => {
                    partial_message = Some(partial.clone());
                    messages.push(Message::Assistant(partial.clone()));
                    added_partial = true;
                    emit(AgentEvent::MessageStart {
                        message: messages.last().unwrap().clone(),
                    });
                }

                ProviderEvent::TextDelta { delta, partial, .. } => {
                    if let Some(ref mut partial) = partial_message {
                        if let Some(last) = partial.content.last_mut() {
                            if let ContentBlock::Text(t) = last {
                                t.text.push_str(&delta);
                            }
                        } else {
                            partial
                                .content
                                .push(ContentBlock::Text(TextContent::new(delta.clone())));
                        }
                        emit(AgentEvent::MessageUpdate {
                            message: Message::Assistant(partial.clone()),
                            delta: Some(delta.clone()),
                        });
                    }
                    let _ = partial;
                }

                ProviderEvent::ThinkingStart { partial, .. } => {
                    if let Some(ref mut partial) = partial_message {
                        partial
                            .content
                            .push(ContentBlock::Thinking(oxi_ai::ThinkingContent::new("")));
                    }
                    let _ = partial;
                }

                ProviderEvent::ThinkingDelta { delta, partial, .. } => {
                    if let Some(ref mut partial) = partial_message {
                        if let Some(last) = partial.content.last_mut() {
                            if let ContentBlock::Thinking(t) = last {
                                t.thinking.push_str(&delta);
                            }
                        }
                    }
                    let _ = partial;
                }

                ProviderEvent::ToolCallStart { partial, .. } => {
                    // Tool call will be completed in ToolCallEnd
                    let _ = partial;
                }

                ProviderEvent::ToolCallEnd {
                    tool_call, partial, ..
                } => {
                    // Tool call finished
                    if let Some(ref mut partial) = partial_message {
                        partial.content.push(ContentBlock::ToolCall(tool_call));
                    }
                    let _ = partial;
                }

                ProviderEvent::Done { message, .. } => {
                    if added_partial {
                        let last_idx = messages.len() - 1;
                        if let Message::Assistant(ref mut m) = messages[last_idx] {
                            *m = message.clone();
                        }
                    } else {
                        messages.push(Message::Assistant(message.clone()));
                    }
                    emit(AgentEvent::MessageEnd {
                        message: Message::Assistant(message.clone()),
                    });
                    return Ok(message);
                }

                ProviderEvent::Error { error, .. } => {
                    let raw_msg = error.text_content();
                    let friendly = if raw_msg.is_empty() {
                        "Unknown provider error".to_string()
                    } else {
                        raw_msg
                    };
                    emit(AgentEvent::Error {
                        message: format!("⚠ {}", friendly),
                    });
                    return Err(Error::msg(friendly));
                }

                _ => {}
            }

            if let Some(ref partial) = partial_message {
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    *m = partial.clone();
                }
            }
        }

        let final_message = messages
            .last()
            .and_then(|m| match m {
                Message::Assistant(a) => Some(a.clone()),
                _ => None,
            })
            .ok_or_else(|| Error::msg("No assistant message in context"))?;

        emit(AgentEvent::MessageEnd {
            message: Message::Assistant(final_message.clone()),
        });
        Ok(final_message)
    }

    async fn execute_tool_calls(
        &self,
        messages: &mut Vec<Message>,
        assistant_message: &AssistantMessage,
        tool_calls: Vec<ToolCall>,
        emit: &EmitFn,
    ) -> Result<ExecutedToolCallBatch> {
        if self.config.tool_execution == ToolExecutionMode::Sequential {
            self.execute_tool_calls_sequential(messages, assistant_message, tool_calls, emit)
                .await
        } else {
            self.execute_tool_calls_parallel(messages, assistant_message, tool_calls, emit)
                .await
        }
    }

    async fn execute_tool_calls_sequential(
        &self,
        messages: &mut Vec<Message>,
        _assistant_message: &AssistantMessage,
        tool_calls: Vec<ToolCall>,
        emit: &EmitFn,
    ) -> Result<ExecutedToolCallBatch> {
        let mut finalized_calls = Vec::new();
        let mut tool_result_messages = Vec::new();

        for tool_call in tool_calls {
            emit(AgentEvent::ToolExecutionStart {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                args: tool_call.arguments.clone(),
            });

            let prepared = self.prepare_tool_call(&tool_call).await;

            let finalized = if let Some(result) = prepared.immediate_result {
                FinalizedToolCall {
                    tool_call,
                    result,
                    is_error: prepared.is_error,
                }
            } else {
                let executed = self.execute_prepared_tool_call(&prepared, emit).await;

                let mut result = executed.result;
                let mut is_error = executed.is_error;

                if let Some(ref hook) = self.after_tool_call {
                    if let Some(modified) = hook(&tool_call.name, &result).await.ok().flatten() {
                        result = modified;
                        is_error = !result.success;
                    }
                }

                FinalizedToolCall {
                    tool_call,
                    result,
                    is_error,
                }
            };

            emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: finalized.tool_call.id.clone(),
                tool_name: finalized.tool_call.name.clone(),
                result: oxi_ai::ToolResult {
                    tool_call_id: finalized.tool_call.id.clone(),
                    content: finalized.result.output.clone(),
                    status: if finalized.is_error {
                        "error".to_string()
                    } else {
                        "success".to_string()
                    },
                },
                is_error: finalized.is_error,
            });

            let tool_result_message = create_tool_result_message(&finalized);
            emit(AgentEvent::MessageStart {
                message: Message::ToolResult(tool_result_message.clone()),
            });
            emit(AgentEvent::MessageEnd {
                message: Message::ToolResult(tool_result_message.clone()),
            });

            finalized_calls.push(finalized);
            tool_result_messages.push(tool_result_message);
        }

        Ok(ExecutedToolCallBatch {
            messages: tool_result_messages,
            terminate: should_terminate_batch(&finalized_calls),
        })
    }

    async fn execute_tool_calls_parallel(
        &self,
        messages: &mut Vec<Message>,
        _assistant_message: &AssistantMessage,
        tool_calls: Vec<ToolCall>,
        emit: &EmitFn,
    ) -> Result<ExecutedToolCallBatch> {
        let mut finalized_calls: Vec<FinalizedToolCallEntry> = Vec::new();

        for tool_call in tool_calls {
            emit(AgentEvent::ToolExecutionStart {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                args: tool_call.arguments.clone(),
            });

            let prepared = self.prepare_tool_call(&tool_call).await;

            if let Some(result) = prepared.immediate_result {
                let finalized = FinalizedToolCall {
                    tool_call,
                    result,
                    is_error: prepared.is_error,
                };

                emit(AgentEvent::ToolExecutionEnd {
                    tool_call_id: finalized.tool_call.id.clone(),
                    tool_name: finalized.tool_call.name.clone(),
                    result: oxi_ai::ToolResult {
                        tool_call_id: finalized.tool_call.id.clone(),
                        content: finalized.result.output.clone(),
                        status: if finalized.is_error {
                            "error".to_string()
                        } else {
                            "success".to_string()
                        },
                    },
                    is_error: finalized.is_error,
                });

                finalized_calls.push(FinalizedToolCallEntry::Immediate(finalized));
            } else {
                let before_hook = self.before_tool_call.clone();
                let after_hook = self.after_tool_call.clone();
                let emit_clone = emit.clone();

                finalized_calls.push(FinalizedToolCallEntry::Future(Box::pin(async move {
                    let executed = Self::execute_prepared_tool_call_static(
                        tool_call.clone(),
                        before_hook.clone(),
                        after_hook.clone(),
                        emit_clone.clone(),
                    )
                    .await;

                    FinalizedToolCall {
                        tool_call,
                        result: executed.result,
                        is_error: executed.is_error,
                    }
                })));
            }
        }

        let mut ordered_finalized_calls = Vec::new();
        for entry in finalized_calls {
            match entry {
                FinalizedToolCallEntry::Immediate(f) => ordered_finalized_calls.push(f),
                FinalizedToolCallEntry::Future(f) => ordered_finalized_calls.push(f.await),
            }
        }

        let mut tool_result_messages = Vec::new();
        for finalized in &ordered_finalized_calls {
            let tool_result_message = create_tool_result_message(finalized);
            emit(AgentEvent::MessageStart {
                message: Message::ToolResult(tool_result_message.clone()),
            });
            emit(AgentEvent::MessageEnd {
                message: Message::ToolResult(tool_result_message.clone()),
            });
            tool_result_messages.push(tool_result_message);
        }

        Ok(ExecutedToolCallBatch {
            messages: tool_result_messages,
            terminate: should_terminate_batch(&ordered_finalized_calls),
        })
    }

    async fn execute_prepared_tool_call_static(
        tool_call: ToolCall,
        _before_hook: Option<BeforeToolCallHook>,
        _after_hook: Option<AfterToolCallHook>,
        emit: Arc<dyn Fn(AgentEvent) + Send + Sync>,
    ) -> ExecutedToolCallOutcome {
        let tool_call_id = tool_call.id.clone();
        let tool_name = tool_call.name.clone();

        emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.clone(),
            result: oxi_ai::ToolResult {
                tool_call_id,
                content: String::new(),
                status: "success".to_string(),
            },
            is_error: false,
        });

        ExecutedToolCallOutcome {
            result: AgentToolResult::success(""),
            is_error: false,
        }
    }

    async fn prepare_tool_call(&self, tool_call: &ToolCall) -> PreparedToolCallOutcome {
        let tool = match self.tools.get(&tool_call.name) {
            Some(t) => t,
            None => {
                return PreparedToolCallOutcome {
                    kind: PreparedToolCallKind::Immediate,
                    immediate_result: Some(AgentToolResult::error(format!(
                        "Tool '{}' not found",
                        tool_call.name
                    ))),
                    is_error: true,
                    tool: None,
                    tool_call: tool_call.clone(),
                    args: tool_call.arguments.clone(),
                };
            }
        };

        let validated_args = tool_call.arguments.clone();

        if let Some(ref hook) = self.before_tool_call {
            if let Some(blocked) = hook(&tool_call.name, &validated_args).await.ok().flatten() {
                return PreparedToolCallOutcome {
                    kind: PreparedToolCallKind::Immediate,
                    immediate_result: Some(blocked),
                    is_error: true,
                    tool: None,
                    tool_call: tool_call.clone(),
                    args: validated_args,
                };
            }
        }

        PreparedToolCallOutcome {
            kind: PreparedToolCallKind::Prepared,
            immediate_result: None,
            is_error: false,
            tool: Some(Arc::clone(&tool)),
            tool_call: tool_call.clone(),
            args: validated_args,
        }
    }

    async fn execute_prepared_tool_call(
        &self,
        prepared: &PreparedToolCallOutcome,
        emit: &EmitFn,
    ) -> ExecutedToolCallOutcome {
        let tool_call_id = prepared.tool_call.id.clone();
        let tool_name = prepared.tool_call.name.clone();

        let mut result = AgentToolResult::success("");
        let mut is_error = false;

        if let Some(ref tool) = prepared.tool {
            let tool_call_id_clone = tool_call_id.clone();
            let emit_clone = emit.clone();

            let progress_cb: Option<Arc<dyn Fn(String) + Send + Sync>> =
                Some(Arc::new(move |msg: String| {
                    emit_clone(AgentEvent::ToolExecutionUpdate {
                        tool_call_id: tool_call_id_clone.clone(),
                        tool_name: tool_name.clone(),
                        partial_result: msg,
                    });
                }));

            let _ = progress_cb;

            match tool
                .execute(&tool_call_id, prepared.args.clone(), None)
                .await
            {
                Ok(r) => result = r,
                Err(e) => {
                    result = AgentToolResult::error(e);
                    is_error = true;
                }
            }
        }

        ExecutedToolCallOutcome { result, is_error }
    }

    fn resolve_model(&self) -> Result<oxi_ai::Model> {
        let parts: Vec<&str> = self.config.model_id.split('/').collect();
        let model = if parts.len() >= 2 {
            oxi_ai::get_model(parts[0], &parts[1..].join("/"))
        } else {
            oxi_ai::get_model("anthropic", &self.config.model_id)
        };

        model.ok_or_else(|| Error::msg(format!("Model not found: {}", self.config.model_id)))
    }

    fn should_stop_after_turn(
        &self,
        messages: &[Message],
        assistant_message: &AssistantMessage,
    ) -> bool {
        let current_iteration = messages
            .iter()
            .filter(|m| matches!(m, Message::Assistant(_)))
            .count();

        if current_iteration >= self.config.max_iterations {
            return true;
        }

        match assistant_message.stop_reason {
            StopReason::Stop | StopReason::Length => true,
            _ => false,
        }
    }

    fn extract_tool_calls(&self, message: &AssistantMessage) -> Vec<ToolCall> {
        let mut tool_calls = Vec::new();

        for block in &message.content {
            if let ContentBlock::ToolCall(tc) = block {
                tool_calls.push(tc.clone());
            }
        }

        tool_calls
    }

    async fn stream_with_retry(
        &self,
        model: &oxi_ai::Model,
        context: &Context,
        options: Option<StreamOptions>,
        emit: &EmitFn,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>> {
        let mut last_err: Option<String> = None;

        for attempt in 0..=MAX_RETRIES {
            match self.provider.stream(model, context, options.clone()).await {
                Ok(stream) => {
                    return Ok(Box::pin(stream)
                        as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
                }
                Err(e) => {
                    let msg = e.to_string();
                    let is_rate_limit = matches!(e, oxi_ai::ProviderError::HttpError(429, _));

                    if !is_rate_limit && attempt == 0 {
                        return Err(AgentError::Stream(msg).into());
                    }

                    last_err = Some(msg.clone());

                    if attempt < MAX_RETRIES {
                        let delay = BACKOFF_BASE_SECS.pow(attempt as u32 + 1);

                        let final_delay = if let Some(max_delay) = self.config.max_retry_delay_ms {
                            delay.min(max_delay)
                        } else {
                            delay
                        };

                        emit(AgentEvent::Retry {
                            attempt: attempt + 1,
                            max_retries: MAX_RETRIES,
                            retry_after_secs: final_delay,
                            reason: msg.clone(),
                        });
                        tokio::time::sleep(tokio::time::Duration::from_secs(final_delay)).await;
                    }
                }
            }
        }

        Err(AgentError::RetriesExhausted {
            attempts: MAX_RETRIES,
            last_error: last_err.unwrap_or_default(),
        }
        .into())
    }
}

struct ExecutedToolCallBatch {
    messages: Vec<oxi_ai::ToolResultMessage>,
    terminate: bool,
}

struct FinalizedToolCall {
    tool_call: ToolCall,
    result: AgentToolResult,
    is_error: bool,
}

enum FinalizedToolCallEntry {
    Immediate(FinalizedToolCall),
    Future(Pin<Box<dyn futures::Future<Output = FinalizedToolCall>>>),
}

struct ExecutedToolCallOutcome {
    result: AgentToolResult,
    is_error: bool,
}

enum PreparedToolCallKind {
    Immediate,
    Prepared,
}

struct PreparedToolCallOutcome {
    kind: PreparedToolCallKind,
    immediate_result: Option<AgentToolResult>,
    is_error: bool,
    tool: Option<Arc<dyn AgentTool>>,
    tool_call: ToolCall,
    args: Value,
}

fn should_terminate_batch(finalized_calls: &[FinalizedToolCall]) -> bool {
    !finalized_calls.is_empty() && finalized_calls.iter().all(|f| f.result.success)
}

fn create_tool_result_message(finalized: &FinalizedToolCall) -> oxi_ai::ToolResultMessage {
    let content_blocks = if let Some(ref blocks) = finalized.result.content_blocks {
        blocks.clone()
    } else {
        vec![ContentBlock::Text(TextContent::new(
            finalized.result.output.clone(),
        ))]
    };

    oxi_ai::ToolResultMessage::new(
        finalized.tool_call.id.clone(),
        &finalized.tool_call.name,
        content_blocks,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use async_trait::async_trait;
    use futures::Stream;
    use oxi_ai::Provider;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    struct MockProvider {
        response: String,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn stream(
            &self,
            _model: &oxi_ai::Model,
            _context: &Context,
            _options: Option<StreamOptions>,
        ) -> std::result::Result<
            Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>,
            oxi_ai::ProviderError,
        > {
            let mut assistant =
                oxi_ai::AssistantMessage::new(oxi_ai::Api::AnthropicMessages, "mock", "mock-model");
            assistant.content = vec![ContentBlock::Text(TextContent::new(self.response.clone()))];

            let stream = futures::stream::once(async move {
                ProviderEvent::Done {
                    reason: StopReason::Stop,
                    message: assistant,
                }
            });

            Ok(Box::pin(stream))
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    fn create_test_loop() -> AgentLoop {
        let provider = Arc::new(MockProvider {
            response: "Test response".to_string(),
        });

        let config = AgentLoopConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_iterations: 10,
            tool_execution: ToolExecutionMode::Parallel,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 100000,
            compaction_instruction: None,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
        };

        let tools = Arc::new(ToolRegistry::new());
        let state = SharedState::new();

        AgentLoop::new(provider, config, tools, state)
    }

    #[tokio::test]
    async fn test_agent_loop_basic_run() {
        let loop_instance = create_test_loop();
        let mut events = Vec::new();

        let result = loop_instance
            .run("Hello".to_string(), |e| events.push(e))
            .await;

        assert!(result.is_ok());
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentStart { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnStart { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. })));
    }

    #[test]
    fn test_tool_execution_mode_default() {
        assert_eq!(ToolExecutionMode::default(), ToolExecutionMode::Parallel);
    }

    #[test]
    fn test_agent_loop_config_defaults() {
        let config = AgentLoopConfig {
            model_id: "test/model".to_string(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_iterations: 10,
            tool_execution: ToolExecutionMode::Parallel,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 100000,
            compaction_instruction: None,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
        };

        assert_eq!(config.max_iterations, 10);
        assert_eq!(config.tool_execution, ToolExecutionMode::Parallel);
    }

    #[tokio::test]
    async fn test_agent_loop_steering_queue() {
        let loop_instance = create_test_loop();

        loop_instance.steer(Message::User(UserMessage::new("Steering message 1")));
        loop_instance.steer(Message::User(UserMessage::new("Steering message 2")));

        let mut events = Vec::new();
        let result = loop_instance
            .run("Hello".to_string(), |e| events.push(e))
            .await;

        assert!(result.is_ok());

        let steering_count = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
            .count();
        assert_eq!(steering_count, 2);
    }

    #[test]
    fn test_clear_queues() {
        let loop_instance = create_test_loop();

        loop_instance.steer(Message::User(UserMessage::new("steer")));
        loop_instance.follow_up(Message::User(UserMessage::new("follow")));

        loop_instance.clear_steering_queue();
        loop_instance.clear_follow_up_queue();
    }

    #[tokio::test]
    async fn test_agent_loop_message_events() {
        let loop_instance = create_test_loop();
        let mut events = Vec::new();

        let result = loop_instance
            .run("Hello".to_string(), |e| events.push(e))
            .await;

        assert!(result.is_ok());

        let message_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::MessageStart { .. }))
            .count();
        let message_ends = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::MessageEnd { .. }))
            .count();

        assert!(message_starts >= 1);
        assert!(message_ends >= 1);
    }

    #[tokio::test]
    async fn test_agent_loop_sequential_mode() {
        let provider = Arc::new(MockProvider {
            response: "Response".to_string(),
        });

        let config = AgentLoopConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_iterations: 10,
            tool_execution: ToolExecutionMode::Sequential,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 100000,
            compaction_instruction: None,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
        };

        let tools = Arc::new(ToolRegistry::new());
        let state = SharedState::new();

        let loop_instance = AgentLoop::new(provider, config, tools, state);
        let mut events = Vec::new();

        let result = loop_instance
            .run("Hello".to_string(), |e| events.push(e))
            .await;

        assert!(result.is_ok());
    }

    #[test]
    fn test_turn_start_event_type() {
        let event = AgentEvent::TurnStart { turn_number: 1 };
        assert_eq!(event.type_name(), "turn_start");
    }

    #[test]
    fn test_agent_end_event_type() {
        let event = AgentEvent::AgentEnd {
            messages: vec![],
            stop_reason: None,
        };
        assert_eq!(event.type_name(), "agent_end");
        assert!(event.is_terminal());
    }

    #[test]
    fn test_non_terminal_events() {
        assert!(!AgentEvent::TurnStart { turn_number: 1 }.is_terminal());
        let user_msg = Message::User(UserMessage::new("test"));
        assert!(!AgentEvent::TurnEnd {
            turn_number: 1,
            assistant_message: user_msg.clone(),
            tool_results: vec![]
        }
        .is_terminal());
        assert!(!AgentEvent::MessageStart {
            message: user_msg.clone()
        }
        .is_terminal());
        assert!(!AgentEvent::MessageEnd { message: user_msg }.is_terminal());
    }

    #[tokio::test]
    async fn test_agent_loop_error_handling() {
        struct ErrorProvider;

        #[async_trait]
        impl Provider for ErrorProvider {
            async fn stream(
                &self,
                _model: &oxi_ai::Model,
                _context: &Context,
                _options: Option<StreamOptions>,
            ) -> std::result::Result<
                Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>,
                oxi_ai::ProviderError,
            > {
                Err(oxi_ai::ProviderError::Other("Test error".to_string()))
            }

            fn name(&self) -> &str {
                "error"
            }
        }

        let provider = Arc::new(ErrorProvider);

        let config = AgentLoopConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_iterations: 10,
            tool_execution: ToolExecutionMode::Parallel,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 100000,
            compaction_instruction: None,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
        };

        let tools = Arc::new(ToolRegistry::new());
        let state = SharedState::new();

        let loop_instance = AgentLoop::new(provider, config, tools, state);
        let mut events = Vec::new();

        let result = loop_instance
            .run("Hello".to_string(), |e| events.push(e))
            .await;

        assert!(events.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
    }

    #[tokio::test]
    async fn test_agent_loop_max_iterations() {
        struct InfiniteProvider;

        #[async_trait]
        impl Provider for InfiniteProvider {
            async fn stream(
                &self,
                _model: &oxi_ai::Model,
                _context: &Context,
                _options: Option<StreamOptions>,
            ) -> std::result::Result<
                Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>,
                oxi_ai::ProviderError,
            > {
                let mut assistant = oxi_ai::AssistantMessage::new(
                    oxi_ai::Api::AnthropicMessages,
                    "mock",
                    "mock-model",
                );
                assistant.content = vec![ContentBlock::Text(TextContent::new("Response"))];
                assistant.stop_reason = StopReason::Stop;

                let stream = futures::stream::once(async move {
                    ProviderEvent::Done {
                        reason: StopReason::Stop,
                        message: assistant,
                    }
                });

                Ok(Box::pin(stream))
            }

            fn name(&self) -> &str {
                "infinite"
            }
        }

        let provider = Arc::new(InfiniteProvider);

        let config = AgentLoopConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_iterations: 3,
            tool_execution: ToolExecutionMode::Parallel,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 100000,
            compaction_instruction: None,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
        };

        let tools = Arc::new(ToolRegistry::new());
        let state = SharedState::new();

        let loop_instance = AgentLoop::new(provider, config, tools, state);
        let mut events = Vec::new();

        let result = loop_instance
            .run("Hello".to_string(), |e| events.push(e))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_agent_loop_follow_up_queue() {
        let loop_instance = create_test_loop();

        loop_instance.follow_up(Message::User(UserMessage::new("Follow-up message")));

        let mut events = Vec::new();
        let result = loop_instance
            .run("Hello".to_string(), |e| events.push(e))
            .await;

        assert!(result.is_ok());
    }

    // =====================================================================
    // NEW TESTS FOR PROXY, DYNAMIC API KEY, PENDING QUEUE, CONTEXT TRANSFORM
    // =====================================================================

    #[test]
    fn test_pending_message_queue_drain_all() {
        let mut queue = PendingMessageQueue::new(QueueDrainMode::All);
        queue.push(Message::User(UserMessage::new("msg1")));
        queue.push(Message::User(UserMessage::new("msg2")));
        queue.push(Message::User(UserMessage::new("msg3")));

        assert_eq!(queue.len(), 3);

        let drained = queue.drain();
        assert_eq!(drained.len(), 3);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_pending_message_queue_drain_one_at_a_time() {
        let mut queue = PendingMessageQueue::new(QueueDrainMode::OneAtATime);
        queue.push(Message::User(UserMessage::new("msg1")));
        queue.push(Message::User(UserMessage::new("msg2")));
        queue.push(Message::User(UserMessage::new("msg3")));

        assert_eq!(queue.len(), 3);

        let first = queue.drain();
        assert_eq!(first.len(), 1);
        assert_eq!(queue.len(), 2);

        let second = queue.drain();
        assert_eq!(second.len(), 1);
        assert_eq!(queue.len(), 1);

        let third = queue.drain();
        assert_eq!(third.len(), 1);
        assert!(queue.is_empty());

        let empty = queue.drain();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_pending_message_queue_mode_change() {
        let mut queue = PendingMessageQueue::new(QueueDrainMode::All);
        queue.push(Message::User(UserMessage::new("msg1")));
        queue.push(Message::User(UserMessage::new("msg2")));

        // Drain all
        assert_eq!(queue.drain().len(), 2);

        // Change mode
        queue.push(Message::User(UserMessage::new("msg3")));
        queue.set_mode(QueueDrainMode::OneAtATime);
        assert_eq!(queue.mode(), QueueDrainMode::OneAtATime);

        // Drain one
        assert_eq!(queue.drain().len(), 1);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_queue_drain_mode_default() {
        assert_eq!(QueueDrainMode::default(), QueueDrainMode::All);
    }

    #[tokio::test]
    async fn test_agent_loop_pending_queue_integration() {
        let loop_instance = create_test_loop();

        // Add messages to pending queue
        loop_instance.push_pending(Message::User(UserMessage::new("pending1")));
        loop_instance.push_pending(Message::User(UserMessage::new("pending2")));

        let drained = loop_instance.drain_pending_queue();
        assert_eq!(drained.len(), 2);

        assert!(loop_instance.drain_pending_queue().is_empty());
    }

    #[tokio::test]
    async fn test_agent_loop_pending_queue_mode() {
        let loop_instance = create_test_loop();

        loop_instance.push_pending(Message::User(UserMessage::new("msg1")));
        loop_instance.push_pending(Message::User(UserMessage::new("msg2")));

        // Set to OneAtATime mode
        loop_instance.set_pending_queue_mode(QueueDrainMode::OneAtATime);

        let first = loop_instance.drain_pending_queue();
        assert_eq!(first.len(), 1);

        let second = loop_instance.drain_pending_queue();
        assert_eq!(second.len(), 1);

        assert!(loop_instance.drain_pending_queue().is_empty());
    }

    #[test]
    fn test_agent_loop_config_api_key_resolution() {
        let get_key = Arc::new(|_provider: &str| -> Result<Option<String>> {
            Ok(Some("resolved_key".to_string()))
        });

        let config = AgentLoopConfig {
            model_id: "anthropic/claude-3-5-sonnet".to_string(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_iterations: 10,
            tool_execution: ToolExecutionMode::Parallel,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 100000,
            compaction_instruction: None,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
            api_key: Some("fallback_key".to_string()),
            proxy_config: None,
            get_api_key: Some(get_key),
            transform_context: None,
            queue_drain_mode: QueueDrainMode::default(),
        };

        assert!(config.get_api_key.is_some());
        assert!(config.api_key.is_some());
    }

    #[test]
    fn test_agent_loop_config_transform_context() {
        let transform_fn = Arc::new(|messages: Vec<Message>, _signal| {
            Box::pin(async move {
                // Simple transform: limit to last 5 messages
                let transformed: Vec<Message> = messages.into_iter().rev().take(5).rev().collect();
                Ok(transformed)
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Message>>> + Send>>
        });

        let config = AgentLoopConfig {
            model_id: "anthropic/claude-3-5-sonnet".to_string(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_iterations: 10,
            tool_execution: ToolExecutionMode::Parallel,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 100000,
            compaction_instruction: None,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
            api_key: None,
            proxy_config: None,
            get_api_key: None,
            transform_context: Some(transform_fn),
            queue_drain_mode: QueueDrainMode::default(),
        };

        assert!(config.transform_context.is_some());
    }
}
