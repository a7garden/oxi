/// Tool execution logic for agent loop
use crate::{AgentEvent, AgentToolResult};
use anyhow::Result;
use futures::{FutureExt, StreamExt};
use oxi_ai::{progress_callback, AssistantMessage, Message, ToolCall, ToolResultMessage};
use std::pin::Pin;
use std::sync::Arc;

use super::config::{AfterToolCallHook, ToolExecutionMode};
use super::helpers::{create_tool_result_message, should_terminate_batch, FinalizedToolCall};
use crate::tools::ToolContext;

pub(crate) struct ExecutedToolCallBatch {
    pub messages: Vec<ToolResultMessage>,
    pub terminate: bool,
}

enum FinalizedToolCallEntry {
    Immediate(FinalizedToolCall),
    Future(Pin<Box<dyn futures::Future<Output = FinalizedToolCall> + Send>>),
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
    _kind: PreparedToolCallKind,
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
    ctx: &ToolContext,
) -> Result<ExecutedToolCallBatch> {
    if loop_ref.config.tool_execution == ToolExecutionMode::Sequential {
        execute_tool_calls_sequential(loop_ref, messages, assistant_message, tool_calls, emit, ctx)
            .await
    } else {
        execute_tool_calls_parallel(loop_ref, messages, assistant_message, tool_calls, emit, ctx)
            .await
    }
}

async fn execute_tool_calls_sequential(
    loop_ref: &super::AgentLoop,
    _messages: &mut Vec<Message>,
    _assistant_message: &AssistantMessage,
    tool_calls: Vec<ToolCall>,
    emit: &super::EmitFn,
    ctx: &ToolContext,
) -> Result<ExecutedToolCallBatch> {
    let mut finalized_calls = Vec::new();
    let mut tool_result_messages = Vec::new();

    for tool_call in tool_calls {
        // Check cancellation before executing each tool.
        // This allows Ctrl+C to interrupt a batch of tool calls
        // without waiting for all of them to complete.
        if loop_ref.is_cancelled() {
            tracing::info!(
                "[TOOL-EXEC] Cancelled before executing tool {}",
                tool_call.name
            );
            break;
        }
        // Clone tool_call fields once upfront to avoid repeated clones.
        let tc_id = tool_call.id.clone();
        let tc_name = tool_call.name.clone();
        let tc_args = tool_call.arguments.clone();

        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tc_id.clone(),
            tool_name: tc_name.clone(),
            args: tc_args,
        });

        let prepared = prepare_tool_call(loop_ref, &tool_call).await;

        let finalized = if let Some(result) = prepared.immediate_result {
            FinalizedToolCall {
                tool_call,
                result,
                is_error: prepared.is_error,
            }
        } else {
            let executed = execute_prepared_tool_call(loop_ref, &prepared, emit, ctx).await;

            let mut result = executed.result;
            let mut is_error = executed.is_error;

            if let Some(ref hook) = loop_ref.after_tool_call {
                if let Some(modified) = hook(&tc_name, &result).await.ok().flatten() {
                    if let Some(ref details) = modified.metadata {
                        tracing::debug!(
                            tool = %tc_name,
                            details = %details,
                            "after_tool_call hook returned details"
                        );
                    }
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
                    String::from("error")
                } else {
                    String::from("success")
                },
            },
            is_error: finalized.is_error,
        });

        let tool_result_message = create_tool_result_message(&finalized);
        let msg = Message::ToolResult(tool_result_message.clone());
        emit(AgentEvent::MessageStart {
            message: msg.clone(),
        });
        emit(AgentEvent::MessageEnd { message: msg });

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
    ctx: &ToolContext,
) -> Result<ExecutedToolCallBatch> {
    let mut finalized_calls: Vec<FinalizedToolCallEntry> = Vec::new();

    for tool_call in tool_calls {
        // Check cancellation before preparing each tool.
        if loop_ref.is_cancelled() {
            tracing::info!(
                "[TOOL-EXEC-PARALLEL] Cancelled before preparing tool {}",
                tool_call.name
            );
            break;
        }
        // Clone tool_call fields once upfront to avoid repeated clones.
        let tc_id = tool_call.id.clone();
        let tc_name = tool_call.name.clone();
        let tc_args = tool_call.arguments.clone();

        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tc_id.clone(),
            tool_name: tc_name.clone(),
            args: tc_args,
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
                    status: if finalized.is_error {
                        String::from("error")
                    } else {
                        String::from("success")
                    },
                },
                is_error: finalized.is_error,
            });

            finalized_calls.push(FinalizedToolCallEntry::Immediate(finalized));
        } else {
            let tool = prepared.tool.clone();
            let args = prepared.args.clone();
            let after_hook = loop_ref.after_tool_call.clone();
            let emit_clone = emit.clone();
            let ctx_clone = ctx.clone();

            finalized_calls.push(FinalizedToolCallEntry::Future(Box::pin(async move {
                let executed = execute_prepared_tool_call_static(
                    tool_call.clone(),
                    tool,
                    args,
                    after_hook.clone(),
                    emit_clone.clone(),
                    &ctx_clone,
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

    let mut slots: Vec<Option<FinalizedToolCall>> = Vec::with_capacity(finalized_calls.len());
    #[allow(clippy::type_complexity)]
    let mut pending_futures: Vec<(
        usize,
        Pin<Box<dyn futures::Future<Output = FinalizedToolCall> + Send>>,
    )> = Vec::new();

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
        // Poll futures with periodic cancel checks.
        // Uses `FuturesUnordered` so we can drain completed results as they
        // arrive and detect cancellation without waiting for all futures.
        let mut active = futures::stream::FuturesUnordered::new();
        for (i, f) in pending_futures {
            active.push(async move { (i, f.await) });
        }

        // Check cancel every 100ms so Ctrl+C is responsive even when
        // tool calls are slow.
        let mut cancel_interval = tokio::time::interval(tokio::time::Duration::from_millis(100));
        cancel_interval.tick().await; // consume immediate first tick

        loop {
            tokio::select! {
                result = active.next() => {
                    match result {
                        Some((idx, finalized)) => {
                            slots[idx] = Some(finalized);
                        }
                        None => break, // all futures completed
                    }
                }
                _ = cancel_interval.tick() => {
                    if loop_ref.is_cancelled() {
                        tracing::info!(
                            "[TOOL-EXEC-PARALLEL] Cancelled during parallel execution, waiting for {} pending futures",
                            active.len()
                        );
                        // Don't abort futures — let them finish (they may have
                        // side effects). But skip waiting and return what we have.
                        break;
                    }
                }
            }
        }

        // Drain any remaining futures that completed before cancellation.
        while let Some(result) = active.next().now_or_never().flatten() {
            slots[result.0] = Some(result.1);
        }
    }

    // Slots for futures that were still running at cancellation time remain None.
    let ordered_finalized_calls: Vec<FinalizedToolCall> = slots.into_iter().flatten().collect();

    let mut tool_result_messages = Vec::new();
    for finalized in &ordered_finalized_calls {
        let tool_result_message = create_tool_result_message(finalized);
        let msg = Message::ToolResult(tool_result_message.clone());
        emit(AgentEvent::MessageStart {
            message: msg.clone(),
        });
        emit(AgentEvent::MessageEnd { message: msg });
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
    after_hook: Option<AfterToolCallHook>,
    emit: Arc<dyn Fn(AgentEvent) + Send + Sync>,
    ctx: &ToolContext,
) -> ExecutedToolCallOutcome {
    let tool_call_id = tool_call.id.clone();
    let tool_name = tool_call.name.clone();

    let mut result = AgentToolResult::success("");
    let mut is_error = false;

    if let Some(ref tool) = tool {
        match tool.execute(&tool_call_id, args, None, ctx).await {
            Ok(r) => result = r,
            Err(e) => {
                result = AgentToolResult::error(e);
                is_error = true;
            }
        }
    }

    if let Some(ref hook) = after_hook {
        if let Some(modified) = hook(&tool_call.name, &result).await.ok().flatten() {
            if let Some(ref details) = modified.metadata {
                tracing::debug!(
                    tool = %tool_call.name,
                    details = %details,
                    "after_tool_call hook returned details"
                );
            }
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
            status: if is_error {
                String::from("error")
            } else {
                String::from("success")
            },
        },
        is_error,
    });

    ExecutedToolCallOutcome { result, is_error }
}

async fn prepare_tool_call(
    loop_ref: &super::AgentLoop,
    tool_call: &ToolCall,
) -> PreparedToolCallOutcome {
    let tool = match loop_ref.tools.get(&tool_call.name) {
        Some(t) => t,
        None => {
            return PreparedToolCallOutcome {
                _kind: PreparedToolCallKind::Immediate,
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
                _kind: PreparedToolCallKind::Immediate,
                immediate_result: Some(blocked),
                is_error: true,
                tool: None,
                tool_call: tool_call.clone(),
                args: validated_args,
            };
        }
    }

    PreparedToolCallOutcome {
        _kind: PreparedToolCallKind::Prepared,
        immediate_result: None,
        is_error: false,
        tool: Some(Arc::clone(&tool)),
        tool_call: tool_call.clone(),
        args: validated_args,
    }
}

async fn execute_prepared_tool_call(
    _loop_ref: &super::AgentLoop,
    prepared: &PreparedToolCallOutcome,
    emit: &super::EmitFn,
    ctx: &ToolContext,
) -> ExecutedToolCallOutcome {
    let tool_call_id = prepared.tool_call.id.clone();
    let tool_name = prepared.tool_call.name.clone();

    let mut result = AgentToolResult::success("");
    let mut is_error = false;

    if let Some(ref tool) = prepared.tool {
        let tool_call_id_clone = tool_call_id.clone();
        let emit_clone = emit.clone();

        let progress_cb: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |msg: String| {
            emit_clone(AgentEvent::ToolExecutionUpdate {
                tool_call_id: tool_call_id_clone.clone(),
                tool_name: tool_name.clone(),
                partial_result: msg,
                // Per-tab routing is a follow-up; the engine currently
                // hands a String to the callback (no structured event),
                // so we don't know the tab_id here. oxios-kernel treats
                // None as "no tab badge".
                tab_id: None,
            });
        });

        // Wire up progress callback BEFORE execute — pi-mono: tool's onUpdate
        tool.on_progress(progress_callback(move |msg: String| {
            progress_cb(msg);
        }));

        match tool
            .execute(&tool_call_id, prepared.args.clone(), None, ctx)
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
