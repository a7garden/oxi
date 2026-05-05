//! Tool execution logic for agent loop

use crate::{AgentToolResult, AgentEvent};
use crate::tools::AgentTool;
use anyhow::Result;
use oxi_ai::{AssistantMessage, ContentBlock, Message, TextContent, ToolCall, ToolResultMessage};
use std::pin::Pin;
use std::sync::Arc;

use super::config::{AfterToolCallHook, ToolExecutionMode};
use super::helpers::{FinalizedToolCall, should_terminate_batch, create_tool_result_message};

pub(crate) struct ExecutedToolCallBatch {
    pub messages: Vec<ToolResultMessage>,
    pub terminate: bool,
}

enum FinalizedToolCallEntry {
    Immediate(FinalizedToolCall),
    Future(Pin<Box<dyn futures::Future<Output = FinalizedToolCall>>>),
}

pub(crate) struct ExecutedToolCallOutcome {
    pub result: AgentToolResult,
    pub is_error: bool,
}

enum PreparedToolCallKind {
    Immediate,
    Prepared,
}

struct PreparedToolCallOutcome {
    #[allow(dead_code)]
    kind: PreparedToolCallKind,
    immediate_result: Option<AgentToolResult>,
    is_error: bool,
    tool: Option<Arc<dyn crate::tools::AgentTool>>,
    tool_call: ToolCall,
    args: serde_json::Value,
}

pub(crate) async fn execute_tool_calls(
    loop_ref: &super::AgentLoop,
    messages: &mut Vec<Message>,
    assistant_message: &AssistantMessage,
    tool_calls: Vec<ToolCall>,
    emit: &super::EmitFn,
) -> Result<ExecutedToolCallBatch> {
    if loop_ref.config.tool_execution == super::config::ToolExecutionMode::Sequential {
        execute_tool_calls_sequential(loop_ref, messages, assistant_message, tool_calls, emit).await
    } else {
        execute_tool_calls_parallel(loop_ref, messages, assistant_message, tool_calls, emit).await
    }
}

async fn execute_tool_calls_sequential(
    loop_ref: &super::AgentLoop,
    _messages: &mut Vec<Message>,
    _assistant_message: &AssistantMessage,
    tool_calls: Vec<ToolCall>,
    emit: &super::EmitFn,
) -> Result<ExecutedToolCallBatch> {
    let mut finalized_calls = Vec::new();
    let mut tool_result_messages = Vec::new();

    for tool_call in tool_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: tool_call.arguments.clone(),
        });

        let prepared = prepare_tool_call(loop_ref, &tool_call).await;

        let finalized = if let Some(result) = prepared.immediate_result {
            FinalizedToolCall {
                tool_call,
                result,
                is_error: prepared.is_error,
            }
        } else {
            let executed = execute_prepared_tool_call(loop_ref, &prepared, emit).await;

            let mut result = executed.result;
            let mut is_error = executed.is_error;

            if let Some(ref hook) = loop_ref.after_tool_call {
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
                status: if finalized.is_error { "error".to_string() } else { "success".to_string() },
            },
            is_error: finalized.is_error,
        });

        let tool_result_message = create_tool_result_message(&finalized);
        emit(AgentEvent::MessageStart { message: Message::ToolResult(tool_result_message.clone()) });
        emit(AgentEvent::MessageEnd { message: Message::ToolResult(tool_result_message.clone()) });

        finalized_calls.push(finalized);
        tool_result_messages.push(tool_result_message);
    }

    Ok(ExecutedToolCallBatch {
        messages: tool_result_messages,
        terminate: should_terminate_batch(&finalized_calls),
    })
}

async fn execute_tool_calls_parallel(
    loop_ref: &super::AgentLoop,
    _messages: &mut Vec<Message>,
    _assistant_message: &AssistantMessage,
    tool_calls: Vec<ToolCall>,
    emit: &super::EmitFn,
) -> Result<ExecutedToolCallBatch> {
    let mut finalized_calls: Vec<FinalizedToolCallEntry> = Vec::new();

    for tool_call in tool_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: tool_call.arguments.clone(),
        });

        let prepared = prepare_tool_call(loop_ref, &tool_call).await;

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
                    status: if finalized.is_error { "error".to_string() } else { "success".to_string() },
                },
                is_error: finalized.is_error,
            });

            finalized_calls.push(FinalizedToolCallEntry::Immediate(finalized));
        } else {
            let tool = prepared.tool.clone();
            let args = prepared.args.clone();
            let after_hook = loop_ref.after_tool_call.clone();
            let emit_clone = emit.clone();

            finalized_calls.push(FinalizedToolCallEntry::Future(Box::pin(async move {
                let executed = execute_prepared_tool_call_static(
                    tool_call.clone(),
                    tool,
                    args,
                    after_hook.clone(),
                    emit_clone.clone(),
                ).await;

                FinalizedToolCall {
                    tool_call,
                    result: executed.result,
                    is_error: executed.is_error,
                }
            })));
        }
    }

    let mut slots: Vec<Option<FinalizedToolCall>> = Vec::with_capacity(finalized_calls.len());
    let mut pending_futures: Vec<(usize, Pin<Box<dyn futures::Future<Output = FinalizedToolCall>>>)> = Vec::new();

    for (i, entry) in finalized_calls.into_iter().enumerate() {
        match entry {
            FinalizedToolCallEntry::Immediate(f) => slots.push(Some(f)),
            FinalizedToolCallEntry::Future(f) => {
                slots.push(None);
                pending_futures.push((i, f));
            }
        }
    }

    if !pending_futures.is_empty() {
        let indexed_results: Vec<(usize, FinalizedToolCall)> =
            futures::future::join_all(pending_futures.into_iter().map(|(i, f)| async move {
                (i, f.await)
            }))
            .await;

        for (idx, finalized) in indexed_results {
            slots[idx] = Some(finalized);
        }
    }

    let ordered_finalized_calls: Vec<FinalizedToolCall> =
        slots.into_iter().map(|s| s.expect("all slots should be filled after join_all")).collect();

    let mut tool_result_messages = Vec::new();
    for finalized in &ordered_finalized_calls {
        let tool_result_message = create_tool_result_message(finalized);
        emit(AgentEvent::MessageStart { message: Message::ToolResult(tool_result_message.clone()) });
        emit(AgentEvent::MessageEnd { message: Message::ToolResult(tool_result_message.clone()) });
        tool_result_messages.push(tool_result_message);
    }

    Ok(ExecutedToolCallBatch {
        messages: tool_result_messages,
        terminate: should_terminate_batch(&ordered_finalized_calls),
    })
}

pub(crate) async fn execute_prepared_tool_call_static(
    tool_call: ToolCall,
    tool: Option<Arc<dyn crate::tools::AgentTool>>,
    args: serde_json::Value,
    after_hook: Option<super::config::AfterToolCallHook>,
    emit: Arc<dyn Fn(AgentEvent) + Send + Sync>,
) -> ExecutedToolCallOutcome {
    let tool_call_id = tool_call.id.clone();
    let tool_name = tool_call.name.clone();

    let mut result = AgentToolResult::success("");
    let mut is_error = false;

    if let Some(ref tool) = tool {
        match tool.execute(&tool_call_id, args, None).await {
            Ok(r) => result = r,
            Err(e) => {
                result = AgentToolResult::error(e);
                is_error = true;
            }
        }
    }

    if let Some(ref hook) = after_hook {
        if let Some(modified) = hook(&tool_call.name, &result).await.ok().flatten() {
            result = modified;
            is_error = !result.success;
        }
    }

    emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: tool_call_id.clone(),
        tool_name: tool_name.clone(),
        result: oxi_ai::ToolResult {
            tool_call_id,
            content: result.output.clone(),
            status: if is_error { "error".to_string() } else { "success".to_string() },
        },
        is_error,
    });

    ExecutedToolCallOutcome {
        result,
        is_error,
    }
}

async fn prepare_tool_call(
    loop_ref: &super::AgentLoop,
    tool_call: &ToolCall,
) -> PreparedToolCallOutcome {
    let tool = match loop_ref.tools.get(&tool_call.name) {
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

    if let Some(ref hook) = loop_ref.before_tool_call {
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
    loop_ref: &super::AgentLoop,
    prepared: &PreparedToolCallOutcome,
    emit: &super::EmitFn,
) -> ExecutedToolCallOutcome {
    let tool_call_id = prepared.tool_call.id.clone();
    let tool_name = prepared.tool_call.name.clone();

    let mut result = AgentToolResult::success("");
    let mut is_error = false;

    if let Some(ref tool) = prepared.tool {
        let tool_call_id_clone = tool_call_id.clone();
        let emit_clone = emit.clone();

        let progress_cb: Option<Arc<dyn Fn(String) + Send + Sync>> = Some(Arc::new(move |msg: String| {
            emit_clone(AgentEvent::ToolExecutionUpdate {
                tool_call_id: tool_call_id_clone.clone(),
                tool_name: tool_name.clone(),
                partial_result: msg,
            });
        }));

        let _ = progress_cb;

        match tool.execute(&tool_call_id, prepared.args.clone(), None).await {
            Ok(r) => result = r,
            Err(e) => {
                result = AgentToolResult::error(e);
                is_error = true;
            }
        }
    }

    ExecutedToolCallOutcome { result, is_error }
}