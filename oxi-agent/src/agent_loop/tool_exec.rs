/// Tool execution logic for agent loop
use crate::events::{ToolCallContext, VisitReason};
use crate::{AgentEvent, AgentToolResult};
use anyhow::Result;
use futures::{FutureExt, StreamExt};
use oxi_ai::{progress_callback, AssistantMessage, Message, ToolCall, ToolResultMessage};
use std::pin::Pin;
use std::sync::Arc;

use super::config::{AfterToolCallHook, ToolExecutionMode};
use super::helpers::{create_tool_result_message, should_terminate_batch, FinalizedToolCall};
use crate::tools::ToolContext as ToolExecContext;

// ── Context inference ─────────────────────────────────────────────────────

/// Infer semantic context from tool name and arguments.
///
/// This is the **single point** where the agent loop assigns meaning
/// to tool calls. Tools themselves remain unaware of semantics — they
/// just do their work and emit facts.
fn infer_context(tool_name: &str, args: &serde_json::Value) -> Option<ToolCallContext> {
    match tool_name {
        "web_search" => args["query"].as_str().map(|q| ToolCallContext::WebSearch {
            query: q.into(),
            engine: args["engines"].as_str().map(String::from),
        }),

        "browse" => args["url"].as_str().map(|u| ToolCallContext::PageVisit {
            url: u.into(),
            reason: Some(VisitReason::DirectNavigation),
            page_title: None,
            page_status: None,
            page_bytes: None,
            page_duration_ms: None,
        }),

        "browse_extract" => Some(ToolCallContext::DataExtraction {
            target: args["selector"]
                .as_str()
                .unwrap_or("data")
                .to_string(),
            url: args["url"].as_str().map(String::from),
            result_count: None,
            page_status: None,
            page_duration_ms: None,
        }),

        "browse_session" => {
            let action = args["action"].as_str().unwrap_or("unknown");
            // "goto" is semantically a page visit, not a generic action.
            if action == "goto" {
                args["url"].as_str().map(|u| ToolCallContext::PageVisit {
                    url: u.into(),
                    reason: Some(VisitReason::DirectNavigation),
                    page_title: None,
                    page_status: None,
                    page_bytes: None,
                    page_duration_ms: None,
                })
            } else {
                Some(ToolCallContext::SessionAction {
                    action: action.to_string(),
                    url: args["url"].as_str().map(String::from),
                })
            }
        }

        "browse_script" => {
            let total = args["steps"].as_array().map(|a| a.len()).unwrap_or(0);
            if total > 0 {
                Some(ToolCallContext::ScriptStep {
                    current: 0,
                    total,
                    step: "starting".into(),
                })
            } else {
                // No structured steps array — script is YAML text.
                // Parsing requires serde_yaml (native-browser feature).
                #[cfg(feature = "native-browser")]
                {
                    args["script"]
                        .as_str()
                        .and_then(|s| serde_yaml::from_str::<serde_yaml::Value>(s).ok())
                        .as_ref()
                        .and_then(|v| v.get("steps").and_then(|s| s.as_sequence()))
                        .map(|steps| ToolCallContext::ScriptStep {
                            current: 0,
                            total: steps.len(),
                            step: "starting".into(),
                        })
                }
                #[cfg(not(feature = "native-browser"))]
                { None }
            }
        }

        _ => None,
    }
}

/// Enrich `context_cell` from `AgentToolResult` metadata after execute.
/// Handles `result_count` for `DataExtraction` contexts.
fn enrich_context_from_metadata(
    context_cell: &Arc<parking_lot::Mutex<Option<ToolCallContext>>>,
    result: &AgentToolResult,
) {
    if let Some(ref meta) = result.metadata {
        if let Some(count) = meta.get("result_count").and_then(|v| v.as_u64()) {
            let mut guard = context_cell.lock();
            if let Some(ToolCallContext::DataExtraction { result_count, .. }) = &mut *guard {
                *result_count = Some(count as usize);
            }
        }
    }
}

/// Build a `BrowseProgressCallback` that enriches `context_cell` with
/// structured data from each `BrowseProgress` event.
///
/// Mapping:
/// - `DocumentReady` + `PageVisit` → fills `page_title`, `page_status`,
///   `page_bytes`, `page_duration_ms`; updates `url` if redirected.
/// - `DocumentReady` + `DataExtraction` → fills `page_status`, `page_duration_ms`.
/// - All other combinations → no-op.
fn make_browse_enrichment_cb(
    context_cell: Arc<parking_lot::Mutex<Option<ToolCallContext>>>,
) -> crate::tools::browse::BrowseProgressCallback {
    Arc::new(move |progress: crate::tools::browse::BrowseProgress| {
        let mut guard = context_cell.lock();
        match (&mut *guard, &progress) {
            (
                Some(ToolCallContext::PageVisit {
                    url,
                    page_title,
                    page_status,
                    page_bytes,
                    page_duration_ms,
                    ..
                }),
                crate::tools::browse::BrowseProgress::DocumentReady {
                    url: ready_url,
                    title,
                    status,
                    bytes,
                    duration_ms,
                },
            ) => {
                // Update URL if redirected.
                if url != ready_url {
                    *url = ready_url.clone();
                }
                *page_title = Some(title.clone());
                *page_status = Some(*status);
                *page_bytes = Some(*bytes);
                *page_duration_ms = Some(*duration_ms);
            }
            (
                Some(ToolCallContext::DataExtraction {
                    page_status,
                    page_duration_ms,
                    ..
                }),
                crate::tools::browse::BrowseProgress::DocumentReady {
                    status,
                    duration_ms,
                    ..
                },
            ) => {
                *page_status = Some(*status);
                *page_duration_ms = Some(*duration_ms);
            }
            _ => {}
        }
    })
}

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
    ctx: &ToolExecContext,
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
    ctx: &ToolExecContext,
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
            args: tc_args.clone(),
            context: infer_context(&tc_name, &tc_args),
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
    ctx: &ToolExecContext,
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
            args: tc_args.clone(),
            context: infer_context(&tc_name, &tc_args),
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
    ctx: &ToolExecContext,
) -> ExecutedToolCallOutcome {
    let tool_call_id = tool_call.id.clone();
    let tool_name = tool_call.name.clone();

    let mut result = AgentToolResult::success("");
    let mut is_error = false;

    if let Some(ref tool) = tool {
        // Infer semantic context — same as sequential path.
        let context = infer_context(&tool_name, &args);

        // Shared context cell for progressive enrichment.
        let context_cell: Arc<parking_lot::Mutex<Option<ToolCallContext>>> =
            Arc::new(parking_lot::Mutex::new(context));

        // Tab ID slot.
        let tab_id_slot: Arc<parking_lot::Mutex<Option<uuid::Uuid>>> =
            Arc::new(parking_lot::Mutex::new(None));
        tool.set_tab_id_slot(Arc::clone(&tab_id_slot));

        // String progress callback.
        let emit_for_cb = emit.clone();
        let tcid = tool_call_id.clone();
        let tn = tool_name.clone();
        let cc = Arc::clone(&context_cell);
        let progress_cb: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |msg: String| {
            let tab_id = *tab_id_slot.lock();
            let ctx = cc.lock().clone();
            emit_for_cb(AgentEvent::ToolExecutionUpdate {
                tool_call_id: tcid.clone(),
                tool_name: tn.clone(),
                partial_result: msg,
                tab_id,
                context: ctx,
            });
        });
        tool.on_progress(progress_callback(move |msg: String| {
            progress_cb(msg);
        }));

        // Browse progress callback — enriches context cell.
        tool.on_browse_progress(make_browse_enrichment_cb(Arc::clone(&context_cell)));

        match tool.execute(&tool_call_id, args, None, ctx).await {
            Ok(r) => result = r,
            Err(e) => {
                result = AgentToolResult::error(e);
                is_error = true;
            }
        }

        enrich_context_from_metadata(&context_cell, &result);
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
    ctx: &ToolExecContext,
) -> ExecutedToolCallOutcome {
    let tool_call_id = prepared.tool_call.id.clone();
    let tool_name = prepared.tool_call.name.clone();

    let mut result = AgentToolResult::success("");
    let mut is_error = false;

    if let Some(ref tool) = prepared.tool {
        let tool_call_id_clone = tool_call_id.clone();
        let tool_name_clone = tool_name.clone();
        let emit_clone = emit.clone();

        // Infer semantic context from tool name + args.
        let context = infer_context(&tool_name, &prepared.args);

        // Shared mutable context cell. The String progress callback reads
        // from here; the browse progress callback writes enriched fields.
        let context_cell: Arc<parking_lot::Mutex<Option<ToolCallContext>>> =
            Arc::new(parking_lot::Mutex::new(context));

        // Shared slot for the active tab ID. Tools that manage browser tabs
        // (BrowseTool) populate this when they open a tab; the progress
        // callback reads it to include `tab_id` in `ToolExecutionUpdate`.
        let tab_id_slot: Arc<parking_lot::Mutex<Option<uuid::Uuid>>> =
            Arc::new(parking_lot::Mutex::new(None));

        // Pass the slot to the tool so it can write the tab_id.
        tool.set_tab_id_slot(Arc::clone(&tab_id_slot));

        // String progress callback — reads context from shared cell.
        let tab_id_slot_cb = Arc::clone(&tab_id_slot);
        let cc = Arc::clone(&context_cell);
        let progress_cb: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |msg: String| {
            let tab_id = *tab_id_slot_cb.lock();
            let ctx = cc.lock().clone();
            emit_clone(AgentEvent::ToolExecutionUpdate {
                tool_call_id: tool_call_id_clone.clone(),
                tool_name: tool_name_clone.clone(),
                partial_result: msg,
                tab_id,
                context: ctx,
            });
        });

        // Wire up progress callback BEFORE execute — pi-mono: tool's onUpdate
        tool.on_progress(progress_callback(move |msg: String| {
            progress_cb(msg);
        }));

        // Browse progress callback — enriches context cell with structured data.
        // Wire up browse progress callback.
        tool.on_browse_progress(make_browse_enrichment_cb(Arc::clone(&context_cell)));

        match tool.execute(&tool_call_id, prepared.args.clone(), None, ctx).await {
            Ok(r) => result = r,
            Err(e) => {
                result = AgentToolResult::error(e);
                is_error = true;
            }
        }

        enrich_context_from_metadata(&context_cell, &result);
    }

    ExecutedToolCallOutcome { result, is_error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn infer_context_web_search() {
        let ctx = infer_context("web_search", &json!({ "query": "rust headless browser" }));
        assert!(matches!(
            ctx,
            Some(ToolCallContext::WebSearch { query, .. }) if query == "rust headless browser"
        ));
    }

    #[test]
    fn infer_context_web_search_with_engine() {
        let ctx = infer_context(
            "web_search",
            &json!({ "query": "rust", "engines": "brave" }),
        );
        assert!(matches!(
            ctx,
            Some(ToolCallContext::WebSearch { engine: Some(e), .. }) if e == "brave"
        ));
    }

    #[test]
    fn infer_context_browse() {
        let ctx = infer_context(
            "browse",
            &json!({ "url": "https://github.com/example/repo" }),
        );
        match ctx {
            Some(ToolCallContext::PageVisit { url, reason, .. }) => {
                assert_eq!(url, "https://github.com/example/repo");
                assert!(matches!(reason, Some(VisitReason::DirectNavigation)));
            }
            other => panic!("expected PageVisit, got {:?}", other),
        }
    }

    #[test]
    fn infer_context_browse_extract() {
        let ctx = infer_context(
            "browse_extract",
            &json!({ "url": "https://example.com", "selector": ".title" }),
        );
        match ctx {
            Some(ToolCallContext::DataExtraction { target, url, .. }) => {
                assert_eq!(target, ".title");
                assert_eq!(url.as_deref(), Some("https://example.com"));
            }
            other => panic!("expected DataExtraction, got {:?}", other),
        }
    }

    #[test]
    fn infer_context_browse_session_goto() {
        let ctx = infer_context(
            "browse_session",
            &json!({ "action": "goto", "url": "https://example.com" }),
        );
        match ctx {
            Some(ToolCallContext::PageVisit { url, reason, .. }) => {
                assert_eq!(url, "https://example.com");
                assert!(matches!(reason, Some(VisitReason::DirectNavigation)));
            }
            other => panic!("expected PageVisit, got {:?}", other),
        }
    }

    #[test]
    fn infer_context_browse_session_click() {
        let ctx = infer_context(
            "browse_session",
            &json!({ "action": "click", "selector": "#btn" }),
        );
        match ctx {
            Some(ToolCallContext::SessionAction { action, url }) => {
                assert_eq!(action, "click");
                assert!(url.is_none());
            }
            other => panic!("expected SessionAction, got {:?}", other),
        }
    }

    #[test]
    fn infer_context_browse_script_with_steps_array() {
        let ctx = infer_context(
            "browse_script",
            &json!({ "steps": [{"goto": "https://example.com"}, {"click": "#btn"}] }),
        );
        match ctx {
            Some(ToolCallContext::ScriptStep { current, total, step }) => {
                assert_eq!(current, 0);
                assert_eq!(total, 2);
                assert_eq!(step, "starting");
            }
            other => panic!("expected ScriptStep, got {:?}", other),
        }
    }

    #[test]
    fn infer_context_browse_script_empty() {
        let ctx = infer_context("browse_script", &json!({ "script": "" }));
        #[cfg(feature = "native-browser")]
        assert!(ctx.is_none());
        #[cfg(not(feature = "native-browser"))]
        assert!(ctx.is_none());
    }

    #[test]
    fn infer_context_unknown_tool() {
        let ctx = infer_context("bash", &json!({ "command": "ls" }));
        assert!(ctx.is_none());
    }

    #[test]
    fn infer_context_missing_args() {
        // browse without url → None
        let ctx = infer_context("browse", &json!({}));
        assert!(ctx.is_none());

        // web_search without query → None
        let ctx = infer_context("web_search", &json!({}));
        assert!(ctx.is_none());
    }

    #[test]
    fn tool_context_serde_roundtrip() {
        let contexts = vec![
            ToolCallContext::WebSearch {
                query: "test".into(),
                engine: Some("ddg".into()),
            },
            ToolCallContext::PageVisit {
                url: "https://example.com".into(),
                reason: Some(VisitReason::DirectNavigation),
                page_title: None,
                page_status: None,
                page_bytes: None,
                page_duration_ms: None,
            },
            ToolCallContext::PageVisit {
                url: "https://example.com".into(),
                reason: Some(VisitReason::SearchResult { position: 3 }),
                page_title: None,
                page_status: None,
                page_bytes: None,
                page_duration_ms: None,
            },
            ToolCallContext::PageVisit {
                url: "https://example.com".into(),
                reason: None,
                page_title: Some("Example Page".into()),
                page_status: Some(200),
                page_bytes: Some(12400),
                page_duration_ms: Some(245),
            },
            ToolCallContext::DataExtraction {
                target: ".title".into(),
                url: Some("https://example.com".into()),
                result_count: None,
                page_status: None,
                page_duration_ms: None,
            },
            ToolCallContext::DataExtraction {
                target: ".items".into(),
                url: Some("https://shop.example.com/products".into()),
                result_count: Some(42),
                page_status: Some(200),
                page_duration_ms: Some(180),
            },
            ToolCallContext::SessionAction {
                action: "goto".into(),
                url: Some("https://example.com".into()),
            },
            ToolCallContext::ScriptStep {
                current: 3,
                total: 10,
                step: "clicking".into(),
            },
        ];

        for ctx in &contexts {
            let json = serde_json::to_string(ctx).unwrap();
            let restored: ToolCallContext = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&restored).unwrap();
            assert_eq!(json, json2, "roundtrip failed for {:?}", ctx);
        }
    }

    #[test]
    fn tool_execution_update_backward_compat() {
        // Old JSON without context field → deserializes with context: None
        let old_json = json!({
            "type": "toolExecutionUpdate",
            "tool_call_id": "call_123",
            "tool_name": "browse",
            "partial_result": "Loading...",
            "tab_id": null
        });
        let event: crate::events::AgentEvent =
            serde_json::from_value(old_json).unwrap();
        match event {
            crate::events::AgentEvent::ToolExecutionUpdate { context, .. } => {
                assert!(context.is_none());
            }
            other => panic!("expected ToolExecutionUpdate, got {:?}", other),
        }
    }

    #[test]
    fn tool_execution_start_backward_compat() {
        // Old JSON without context field
        let old_json = json!({
            "type": "toolExecutionStart",
            "tool_call_id": "call_123",
            "tool_name": "browse",
            "args": { "url": "https://example.com" }
        });
        let event: crate::events::AgentEvent =
            serde_json::from_value(old_json).unwrap();
        match event {
            crate::events::AgentEvent::ToolExecutionStart { context, .. } => {
                assert!(context.is_none());
            }
            other => panic!("expected ToolExecutionStart, got {:?}", other),
        }
    }

    #[test]
    fn browse_enrichment_callback_fills_page_visit() {
        use crate::tools::browse::BrowseProgress;
        use std::sync::Arc;

        let cell: Arc<parking_lot::Mutex<Option<ToolCallContext>>> =
            Arc::new(parking_lot::Mutex::new(Some(ToolCallContext::PageVisit {
                url: "https://example.com".into(),
                reason: Some(VisitReason::DirectNavigation),
                page_title: None,
                page_status: None,
                page_bytes: None,
                page_duration_ms: None,
            })));
        let cb = make_browse_enrichment_cb(Arc::clone(&cell));
        cb(BrowseProgress::DocumentReady {
            url: "https://example.com/final".into(),
            title: "Example".into(),
            status: 200,
            bytes: 4096,
            duration_ms: 245,
        });
        let snapshot = cell.lock().clone();
        match snapshot {
            Some(ToolCallContext::PageVisit {
                url,
                page_title,
                page_status,
                page_bytes,
                page_duration_ms,
                ..
            }) => {
                assert_eq!(url, "https://example.com/final");
                assert_eq!(page_title.as_deref(), Some("Example"));
                assert_eq!(page_status, Some(200));
                assert_eq!(page_bytes, Some(4096));
                assert_eq!(page_duration_ms, Some(245));
            }
            other => panic!("expected PageVisit, got {:?}", other),
        }
    }

    #[test]
    fn browse_enrichment_callback_fills_data_extraction() {
        use crate::tools::browse::BrowseProgress;
        use std::sync::Arc;

        let cell: Arc<parking_lot::Mutex<Option<ToolCallContext>>> =
            Arc::new(parking_lot::Mutex::new(Some(ToolCallContext::DataExtraction {
                target: ".item".into(),
                url: Some("https://shop.example.com".into()),
                result_count: None,
                page_status: None,
                page_duration_ms: None,
            })));
        let cb = make_browse_enrichment_cb(Arc::clone(&cell));
        cb(BrowseProgress::DocumentReady {
            url: "https://shop.example.com".into(),
            title: "Shop".into(),
            status: 200,
            bytes: 8192,
            duration_ms: 180,
        });
        let snapshot = cell.lock().clone();
        match snapshot {
            Some(ToolCallContext::DataExtraction {
                page_status,
                page_duration_ms,
                ..
            }) => {
                assert_eq!(page_status, Some(200));
                assert_eq!(page_duration_ms, Some(180));
            }
            other => panic!("expected DataExtraction, got {:?}", other),
        }
    }

    #[test]
    fn browse_enrichment_callback_no_op_for_mismatched() {
        use crate::tools::browse::BrowseProgress;
        use std::sync::Arc;

        // DocumentReady + ScriptStep → no-op
        let cell: Arc<parking_lot::Mutex<Option<ToolCallContext>>> =
            Arc::new(parking_lot::Mutex::new(Some(ToolCallContext::ScriptStep {
                current: 1,
                total: 5,
                step: "click".into(),
            })));
        let cb = make_browse_enrichment_cb(Arc::clone(&cell));
        cb(BrowseProgress::DocumentReady {
            url: "x".into(),
            title: "t".into(),
            status: 200,
            bytes: 0,
            duration_ms: 0,
        });
        // ScriptStep should be untouched
        assert!(matches!(cell.lock().as_ref(), Some(ToolCallContext::ScriptStep { .. })));
    }
}
