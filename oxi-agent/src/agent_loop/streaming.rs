/// Streaming implementation for agent loop.
///
/// pi-mono pattern: the provider accumulates content into a single `output`
/// message. Each event carries a snapshot (`partial`) of this message.
/// Done carries the complete accumulated message.
///
/// TTSR integration: when a [`TtsrEngine`](super::ttsr::TtsrEngine) is
/// provided, every [`ProviderEvent::TextDelta`] is checked against
/// registered rules. A match aborts the stream and returns
/// [`StreamOutcome::RuleInterrupt`].
use futures::StreamExt;
use oxi_ai::{
    ContentBlock, Context, Message, ProviderEvent, StopReason, StreamOptions, Tool as OxTool,
};
use std::collections::{HashMap, HashSet};

use super::helpers::sanitize_orphaned_tool_results;
use super::stream_outcome::StreamOutcome;
use super::ttsr::{MatchSource, TtsrEngine, TtsrMatchContext};

pub(crate) async fn stream_assistant_response(
    loop_ref: &super::AgentLoop,
    messages: &mut Vec<Message>,
    emit: &super::EmitFn,
    ttsr: Option<&TtsrEngine>,
) -> StreamOutcome {
    let model = match loop_ref.resolve_model() {
        Ok(m) => m,
        Err(_) => {
            return StreamOutcome::Error {
                message: oxi_ai::AssistantMessage::new(
                    oxi_ai::Api::OpenAiCompletions,
                    "agent",
                    &loop_ref.config.model_id,
                ),
                detail: "Failed to resolve model".to_string(),
            };
        }
    };

    // Proactively sanitize orphaned tool results to prevent provider
    // errors like "Messages with role 'tool' must be a response to a
    // preceding message with 'tool_calls'".
    let removed = sanitize_orphaned_tool_results(messages);
    if removed > 0 {
        tracing::warn!(
            session_id = ?loop_ref.session_id,
            removed,
            "Sanitized orphaned tool results before streaming"
        );
    }

    let mut context = Context::new();

    // Build the tool definitions once — used both for native tool calling and
    // for the in-band (owned dialect) prompt catalog.
    let tool_defs = loop_ref.tools.definitions();
    let mut oxi_tools: Vec<OxTool> = Vec::with_capacity(tool_defs.len());
    for def in &tool_defs {
        let schema = serde_json::to_value(&def.input_schema)
            .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
        oxi_tools.push(OxTool::new(&def.name, &def.description, schema));
    }

    if let Some(dialect) = loop_ref.config.dialect {
        // Owned (in-band) tool calling: the model has no native tool support, so
        // the tool catalog rides in the system prompt, prior tool calls/results
        // are re-encoded as text, and NO native `tools` are sent. The model's
        // text output is parsed back into tool calls at `Done` (below).
        let base_prompt = loop_ref.config.system_prompt.clone().unwrap_or_default();
        let catalog = oxi_ai::dialect::render_inband_tool_prompt(&oxi_tools, dialect);
        let full_prompt = if base_prompt.trim().is_empty() {
            catalog
        } else {
            format!("{base_prompt}\n\n{catalog}")
        };
        context.set_system_prompt(full_prompt);

        for msg in oxi_ai::dialect::encode_inband_tool_history(messages, dialect, &oxi_tools) {
            context.add_message(msg);
        }
        // Deliberately no `context.set_tools(...)` — owned dialects send no
        // native tools (any `tool_choice` would error on a tools-less request).
    } else {
        if let Some(ref system_prompt) = loop_ref.config.system_prompt {
            context.set_system_prompt(system_prompt.clone());
        }
        for msg in messages.iter() {
            context.add_message(msg.clone());
        }
        if !oxi_tools.is_empty() {
            context.set_tools(oxi_tools);
        }
    }

    let stream_options = StreamOptions {
        temperature: Some(loop_ref.config.temperature as f64),
        max_tokens: Some(loop_ref.config.max_tokens as usize),
        provider_options: loop_ref.config.provider_options.clone(),
        ..Default::default()
    };

    let stream = match super::retry::stream_with_retry(
        loop_ref,
        &model,
        &context,
        Some(stream_options),
        emit,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            return StreamOutcome::Error {
                message: oxi_ai::AssistantMessage::new(
                    oxi_ai::Api::OpenAiCompletions,
                    "agent",
                    &loop_ref.config.model_id,
                ),
                detail: e.to_string(),
            };
        }
    };

    let mut added_partial = false;
    let mut event_count = 0u32;
    // content_index → resolved tool-call id, populated from `ToolCallStart`
    // and reconciled at `ToolCallEnd`. Lets `ToolCallDelta` forward the
    // correct id even though providers (Anthropic, OpenAI) keep the id in a
    // private pending map and do not embed a `ContentBlock::ToolCall` in the
    // streaming partial until the call finalizes.
    let mut tool_call_ids: HashMap<usize, String> = HashMap::new();

    // Reset the thinking-loop detector so each stream attempt is guarded
    // independently. Without this, the detector's `fired` flag stays
    // sticky after the first hit and retries run unguarded (issue
    // flagged in advisory review). The retry layer may resample several
    // times; each attempt deserves a fresh detector.
    if let Some(detector) = loop_ref.thinking_loop_detector.lock().as_mut() {
        detector.reset();
    }
    let mut rx = stream;
    let stream_idle_timeout = std::time::Duration::from_secs(30);
    let cancel_check_interval = std::time::Duration::from_millis(500);
    let mut last_event_at = std::time::Instant::now();

    loop {
        let next_event = tokio::select! {
            event = rx.next() => event,
            _ = tokio::time::sleep(cancel_check_interval) => {
                if loop_ref.is_cancelled() {
                    tracing::info!(
                        "Stream cancelled (detected in periodic check)"
                    );
                    if added_partial {
                        let last_idx = messages.len() - 1;
                        if let Message::Assistant(ref mut m) = messages[last_idx] {
                            m.stop_reason = StopReason::Aborted;
                        }
                        let last_msg = messages.last().expect("non-empty").clone();
                        emit(super::AgentEvent::MessageEnd {
                            message: last_msg.clone(),
                        });
                        if let Message::Assistant(m) = &last_msg {
                            return StreamOutcome::Cancelled(m.clone());
                        }
                    }
                    return StreamOutcome::Cancelled(oxi_ai::AssistantMessage::new(
                        oxi_ai::Api::OpenAiCompletions,
                        "agent",
                        &loop_ref.config.model_id,
                    ));
                }

                if last_event_at.elapsed() >= stream_idle_timeout {
                    tracing::warn!(
                        "Stream idle timeout ({:?}) reached after {} events",
                        stream_idle_timeout, event_count
                    );
                    let mut err_asst = oxi_ai::AssistantMessage::new(
                        oxi_ai::Api::OpenAiCompletions,
                        "agent",
                        &loop_ref.config.model_id,
                    );
                    err_asst.stop_reason = StopReason::Error;
                    err_asst.error_message = Some(format!(
                        "Stream timed out after {:?} of inactivity",
                        stream_idle_timeout
                    ));
                    if added_partial {
                        let last_idx = messages.len() - 1;
                        if let Message::Assistant(ref mut m) = messages[last_idx] {
                            m.stop_reason = StopReason::Error;
                        }
                    }
                    emit(super::AgentEvent::MessageEnd {
                        message: Message::Assistant(err_asst.clone()),
                    });
                    emit(super::AgentEvent::Error {
                        message: format!(
                            "Stream timed out after {:?} of inactivity",
                            stream_idle_timeout
                        ),
                        session_id: loop_ref.session_id.clone(),
                    });
                    return StreamOutcome::Error { message: err_asst, detail: format!("Stream timed out after {:?} of inactivity", stream_idle_timeout) };
                }

                continue;
            }
        };

        let event = match next_event {
            Some(e) => e,
            None => break,
        };

        last_event_at = std::time::Instant::now();

        if loop_ref.is_cancelled() {
            tracing::info!("Stream cancelled after {} events", event_count);
            if added_partial {
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    m.stop_reason = StopReason::Aborted;
                }
                let last_msg = messages.last().expect("non-empty").clone();
                emit(super::AgentEvent::MessageEnd {
                    message: last_msg.clone(),
                });
                if let Message::Assistant(m) = &last_msg {
                    return StreamOutcome::Cancelled(m.clone());
                }
            }
            return StreamOutcome::Cancelled(oxi_ai::AssistantMessage::new(
                oxi_ai::Api::OpenAiCompletions,
                "agent",
                &loop_ref.config.model_id,
            ));
        }

        event_count += 1;
        match event {
            ProviderEvent::Start { partial } => {
                tracing::info!("Stream event #{}: Start", event_count);
                messages.push(Message::Assistant((*partial).clone()));
                added_partial = true;
                emit(super::AgentEvent::MessageStart {
                    message: messages.last().expect("non-empty after push").clone(),
                });
            }

            ProviderEvent::TextDelta { delta, partial, .. } => {
                if added_partial {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = (*partial).clone();
                    }
                }
                let last_msg = messages.last().expect("non-empty").clone();
                let delta_clone = delta.clone();
                emit(super::AgentEvent::MessageUpdate {
                    message: last_msg,
                    delta: Some(delta),
                });

                // ── TTSR check ──
                if let Some(engine) = ttsr {
                    let ctx = TtsrMatchContext {
                        source: MatchSource::Text,
                        file_paths: vec![],
                        tool_name: None,
                    };
                    let violations = engine.check_delta(&delta_clone, &ctx);
                    if !violations.is_empty() {
                        let mut partial_msg = messages
                            .last()
                            .and_then(|m| match m {
                                Message::Assistant(a) => Some(a.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| {
                                oxi_ai::AssistantMessage::new(
                                    oxi_ai::Api::OpenAiCompletions,
                                    "agent",
                                    &loop_ref.config.model_id,
                                )
                            });
                        partial_msg.stop_reason = StopReason::Aborted;
                        return StreamOutcome::RuleInterrupt {
                            partial: partial_msg,
                            rule: violations.into_iter().next().expect("non-empty"),
                        };
                    }
                }
            }

            ProviderEvent::ThinkingStart { partial, .. } if added_partial => {
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    *m = (*partial).clone();
                }
                emit(super::AgentEvent::Thinking);
            }
            ProviderEvent::ThinkingDelta { delta, partial, .. } => {
                // Feed the thinking-loop detector if enabled. On detection
                // we surface an error event so the retry layer resamples;
                // matches omp's "transient stream stall" classification.
                if let Some(detector) = loop_ref.thinking_loop_detector.lock().as_mut()
                    && let Some(reason) = detector.push(&delta)
                {
                    tracing::warn!(
                        session_id = ?loop_ref.session_id,
                        reason = %reason,
                        "thinking-loop detected; aborting stream"
                    );
                    emit(super::AgentEvent::Error {
                        message: reason,
                        session_id: loop_ref.session_id.clone(),
                    });
                    // Break out of the stream loop — the upstream
                    // retry policy treats transient errors as
                    // resample candidates.
                    break;
                }
                if added_partial {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = (*partial).clone();
                    }
                }
                let last_msg = messages.last().expect("non-empty").clone();
                emit(super::AgentEvent::ThinkingDelta {
                    text: delta.clone(),
                });
                emit(super::AgentEvent::MessageUpdate {
                    message: last_msg,
                    delta: Some(delta),
                });
            }
            ProviderEvent::ThinkingEnd { partial, .. } if added_partial => {
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    *m = (*partial).clone();
                }
                emit(super::AgentEvent::ThinkingEnd);
            }

            ProviderEvent::ToolCallStart {
                content_index,
                tool_call_id,
                partial,
                ..
            } if added_partial => {
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    *m = (*partial).clone();
                }
                // Register the provider id so later ToolCallDelta events can
                // forward it. OpenAI re-emits ToolCallStart on id-bearing
                // deltas, so this also fills the map when the first start
                // lacked an id.
                if let Some(id) = tool_call_id
                    && !id.is_empty()
                {
                    tool_call_ids.insert(content_index, id);
                }
            }

            ProviderEvent::ToolCallDelta {
                content_index,
                delta,
                partial,
                ..
            } if added_partial => {
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    *m = (*partial).clone();
                }
                // Forward the streamed argument fragment to downstream
                // consumers (live tool-arg construction UIs, Oxios kernel).
                // Resolve the id from the ToolCallStart registration; fall
                // back to the finalized block in the accumulated partial for
                // any provider that embeds it there. If neither resolves we
                // skip this delta rather than emit an unverified id.
                let resolved_id = tool_call_ids
                    .get(&content_index)
                    .cloned()
                    .or_else(|| extract_tool_call_id(messages, content_index));
                if let Some(id) = resolved_id {
                    emit(super::AgentEvent::ToolCallDelta {
                        tool_call_id: id,
                        args_delta: delta,
                    });
                }
            }

            ProviderEvent::ToolCallEnd {
                content_index,
                tool_call,
                ..
            } if added_partial => {
                // Reconcile the id map with the finalized call so any
                // trailing deltas (and the map itself) stay authoritative.
                tool_call_ids.insert(content_index, tool_call.id.clone());
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    m.content.push(ContentBlock::ToolCall(tool_call));
                }
                let last_msg = messages.last().expect("non-empty").clone();
                emit(super::AgentEvent::MessageUpdate {
                    message: last_msg,
                    delta: None,
                });
            }

            ProviderEvent::Done { message, .. } => {
                let (input, output) = (message.usage.input, message.usage.output);
                if input > 0 || output > 0 {
                    // Snapshot the heuristic estimate of what was *just
                    // sent* so we can compare it to the provider's
                    // reported input_tokens on the same snapshot. This
                    // is the drift metric referenced by issue #28:
                    // `bytes/4` can undercount by 3-4× on token-dense
                    // content (base64, JSON, CJK), and the legacy
                    // compaction path used that heuristic directly.
                    //
                    // The slice we estimate over is the *prompt* the
                    // provider tokenized, NOT the prompt + the
                    // assistant turn we just streamed. At
                    // `ProviderEvent::Done`, `messages` ends with the
                    // just-completed assistant message (pushed on
                    // `Start` at the start of the stream, or — in the
                    // no-partial-Start path — on `Done` after this
                    // branch; we record before that push). We slice
                    // off the trailing assistant message so the
                    // heuristic matches what `usage.input` actually
                    // covers; otherwise the drift metric would
                    // *understate* #28's bytes/4 underestimate.
                    //
                    // The compaction decision itself is unaffected —
                    // it reads `Real(last_input_tokens)` =
                    // `usage.input`, which is correct.
                    let prompt_len = messages.len().saturating_sub(1);
                    let estimate_at_report = estimate_tokens_from_messages(&messages[..prompt_len]);
                    loop_ref.state.update(|s| {
                        s.record_usage(input, output);
                        s.record_provider_turn(input, estimate_at_report);
                    });
                    emit(super::AgentEvent::Usage {
                        input_tokens: input,
                        output_tokens: output,
                    });
                }

                tracing::info!(
                    "Stream event #{}: Done (stop_reason={:?})",
                    event_count,
                    message.stop_reason
                );

                if added_partial {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        let mut seen_ids: HashSet<String> = message
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolCall(tc) => Some(tc.id.clone()),
                                _ => None,
                            })
                            .collect();

                        let extra_tool_calls: Vec<ContentBlock> = m
                            .content
                            .iter()
                            .filter(|b| match b {
                                ContentBlock::ToolCall(tc) => seen_ids.insert(tc.id.clone()),
                                _ => false,
                            })
                            .cloned()
                            .collect();

                        let tc_count = extra_tool_calls.len();
                        *m = message.clone();
                        m.content.extend(extra_tool_calls);

                        tracing::info!(
                            "Done: merged {} extra tool_calls, final has {} content blocks, stop_reason={:?}",
                            tc_count,
                            m.content.len(),
                            m.stop_reason
                        );
                    }
                } else {
                    messages.push(Message::Assistant(message.clone()));
                }
                // Owned dialect: re-materialize in-band tool calls (emitted as
                // text) into native `ToolCall` blocks so the rest of the loop
                // executes them unchanged. Persist into `messages` so the next
                // turn's history encoding sees canonical tool calls.
                if let Some(dialect) = loop_ref.config.dialect {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        let dialect_tools: Vec<OxTool> = tool_defs
                            .iter()
                            .map(|def| {
                                let schema = serde_json::to_value(&def.input_schema)
                                    .unwrap_or_else(
                                        |_| serde_json::json!({"type": "object", "properties": {}}),
                                    );
                                OxTool::new(&def.name, &def.description, schema)
                            })
                            .collect();
                        let parsed = dialect.parse_assistant_message(m, &dialect_tools);
                        let found = parsed
                            .content
                            .iter()
                            .filter(|b| b.as_tool_call().is_some())
                            .count();
                        if found > 0 {
                            *m = parsed;
                            // A clean stop with re-materialized calls must
                            // continue the loop; a length/error stop is left as
                            // is (the call may be truncated).
                            if m.stop_reason == StopReason::Stop {
                                m.stop_reason = StopReason::ToolUse;
                            }
                            tracing::info!(
                                "Owned dialect: re-materialized {} in-band tool call(s)",
                                found
                            );
                        }
                    }
                }

                let last_msg = messages.last().expect("non-empty").clone();
                emit(super::AgentEvent::MessageEnd {
                    message: last_msg.clone(),
                });
                if let Message::Assistant(m) = &last_msg {
                    return StreamOutcome::Complete(m.clone());
                } else {
                    return StreamOutcome::Complete(message);
                }
            }

            ProviderEvent::Error { mut error, .. } => {
                tracing::info!("Stream event #{}: Error", event_count);
                let raw_msg = error.text_content();
                let friendly = if raw_msg.is_empty() {
                    "Unknown provider error".to_string()
                } else {
                    raw_msg
                };
                tracing::error!(
                    session_id = ?loop_ref.session_id,
                    "Provider stream error: {}", friendly
                );

                error.stop_reason = StopReason::Error;

                if added_partial {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = error.clone();
                    }
                } else {
                    messages.push(Message::Assistant(error.clone()));
                }

                emit(super::AgentEvent::MessageEnd {
                    message: Message::Assistant(error.clone()),
                });
                emit(super::AgentEvent::Error {
                    message: format!("⚠ {}", friendly),
                    session_id: loop_ref.session_id.clone(),
                });

                return StreamOutcome::Error {
                    message: error,
                    detail: format!("⚠ {}", friendly),
                };
            }

            _ => {}
        }
    }

    tracing::info!("Stream ended after {} events", event_count);

    let final_message = match messages.last().and_then(|m| match m {
        Message::Assistant(a) => Some(a.clone()),
        _ => None,
    }) {
        Some(m) => m,
        None => {
            return StreamOutcome::Error {
                message: oxi_ai::AssistantMessage::new(
                    oxi_ai::Api::OpenAiCompletions,
                    "agent",
                    &loop_ref.config.model_id,
                ),
                detail: "No final assistant message in stream".to_string(),
            };
        }
    };

    if !added_partial {
        tracing::warn!("Stream ended without Start event, emitting synthetic MessageStart");
        emit(super::AgentEvent::MessageStart {
            message: Message::Assistant(final_message.clone()),
        });
    }

    emit(super::AgentEvent::MessageEnd {
        message: Message::Assistant(final_message.clone()),
    });
    StreamOutcome::Complete(final_message)
}

/// Heuristic token estimate for a messages slice, mirroring
/// `AgentState::estimate_tokens` (serialized JSON length / 4).
///
/// Used in [`stream_assistant_response`] at the moment the provider
/// reports `usage.input_tokens` to record the divergence between
/// the legacy heuristic and the ground-truth provider count (see
/// issue #28 gap 2). The result is cached on
/// `AgentState::last_estimate_at_report` / `last_estimate_divergence`
/// so the operator can see how badly `bytes/4` is undercounting on
/// token-dense content.
///
/// Kept local (not a method on `AgentState`) so we can call it with
/// a borrowed slice of the loop's working `messages` buffer without
/// cloning the whole history.
fn estimate_tokens_from_messages(messages: &[Message]) -> usize {
    let json = serde_json::to_string(messages).unwrap_or_default();
    json.len() / 4
}

/// Best-effort extraction of a tool-call id from the accumulated assistant
/// message's content block at `content_index`.
///
/// This is a fallback for the `tool_call_ids` map used in
/// [`stream_assistant_response`]: most providers (Anthropic, OpenAI) surface
/// the id at `ToolCallStart` and keep it in a private pending map, so the
/// `ToolCall` block is *not* present in the streaming partial during deltas.
/// The lookup only resolves for providers that embed the block early, but it
/// costs nothing and is forward-compatible.
fn extract_tool_call_id(messages: &[Message], content_index: usize) -> Option<String> {
    let last = messages.last()?;
    let Message::Assistant(m) = last else {
        return None;
    };
    m.content.get(content_index).and_then(|b| match b {
        ContentBlock::ToolCall(tc) => Some(tc.id.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod streaming_lifecycle_tests {
    //! Verifies `stream_assistant_response` forwards provider streaming
    //! lifecycle events as `AgentEvent`s:
    //! - `ProviderEvent::ThinkingEnd` → `AgentEvent::ThinkingEnd`
    //! - `ProviderEvent::ToolCallDelta` → `AgentEvent::ToolCallDelta { tool_call_id, args_delta }`
    //!
    //! The `tool_call_id` is resolved from a content_index→id map populated
    //! at `ToolCallStart`/`ToolCallEnd`, because providers keep the id in a
    //! private pending map and never embed a `ContentBlock::ToolCall` in the
    //! streaming partial until the call finalizes.
    use super::stream_assistant_response;
    use crate::ProviderResolver;
    use crate::config::ToolExecutionMode;
    use crate::events::AgentEvent;
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use crate::{AgentLoop, AgentLoopConfig};
    use futures::Stream;
    use oxi_ai::{
        Api, AssistantMessage, CompactionStrategy, ContentBlock, Context, Message, Model, Provider,
        ProviderEvent, StopReason, StreamOptions, StreamResult, ToolCall, UserMessage,
    };
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context as TaskContext, Poll};

    /// Provider that replays a fixed script of `ProviderEvent`s.
    struct ScriptedProvider {
        events: Arc<Vec<ProviderEvent>>,
    }

    impl ScriptedProvider {
        fn new(events: Vec<ProviderEvent>) -> Self {
            Self {
                events: Arc::new(events),
            }
        }
    }

    impl Provider for ScriptedProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a Model,
            _context: &'a Context,
            _options: Option<StreamOptions>,
        ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
            let events = Arc::clone(&self.events);
            Box::pin(async move {
                Ok(Box::pin(ScriptedStream {
                    events: VecDeque::from((*events).clone()),
                })
                    as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
            })
        }
    }

    struct ScriptedStream {
        events: VecDeque<ProviderEvent>,
    }

    impl Stream for ScriptedStream {
        type Item = ProviderEvent;
        fn poll_next(
            mut self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
        ) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.events.pop_front())
        }
    }

    struct DummyResolver;
    impl ProviderResolver for DummyResolver {
        fn resolve_provider(&self, _name: &str) -> Option<Arc<dyn Provider>> {
            None
        }
        fn resolve_model(&self, _model_id: &str) -> Option<Model> {
            Some(Model::new(
                "test/model",
                "Test",
                Api::AnthropicMessages,
                "mock",
                "https://mock.test",
            ))
        }
    }

    fn empty_partial() -> Arc<AssistantMessage> {
        Arc::new(AssistantMessage::new(
            Api::AnthropicMessages,
            "mock",
            "test/model",
        ))
    }

    fn make_loop(provider: Arc<dyn Provider>) -> AgentLoop {
        let config = AgentLoopConfig {
            model_id: "test/model".to_string(),
            system_prompt: None,
            temperature: 1.0,
            max_tokens: 4096,
            tool_execution: ToolExecutionMode::Sequential,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 128_000,
            compact_on_start: false,
            auto_retry_enabled: false,
            auto_retry_max_attempts: 1,
            thinking_loop_detection: false,
            ..Default::default()
        };
        AgentLoop::new_with_resolver(
            provider,
            config,
            Arc::new(ToolRegistry::new()),
            SharedState::new(),
            Arc::new(DummyResolver),
        )
    }

    /// Runs a scripted provider through `stream_assistant_response` and
    /// returns every emitted `AgentEvent` in order.
    async fn run_script(events: Vec<ProviderEvent>) -> Vec<AgentEvent> {
        let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider::new(events));
        let agent_loop = make_loop(provider);
        let collected: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&collected);
        let emit: Arc<dyn Fn(AgentEvent) + Send + Sync> =
            Arc::new(move |e| sink.lock().unwrap().push(e));
        let mut messages: Vec<Message> = vec![Message::User(UserMessage::new("hi".to_string()))];
        let _ = stream_assistant_response(&agent_loop, &mut messages, &emit, None).await;
        collected.lock().unwrap().clone()
    }

    /// Anthropic-style: the tool-call id is known up front at `ToolCallStart`.
    #[tokio::test]
    async fn thinking_end_and_tool_call_delta_forwarded() {
        let finalized = ToolCall::new("tc_abc", "bash", serde_json::json!({"command":"ls"}));
        let mut done_msg = AssistantMessage::new(Api::AnthropicMessages, "mock", "test/model");
        done_msg
            .content
            .push(ContentBlock::ToolCall(finalized.clone()));
        let events = vec![
            ProviderEvent::Start {
                partial: empty_partial(),
            },
            ProviderEvent::ThinkingStart {
                content_index: 0,
                partial: empty_partial(),
            },
            ProviderEvent::ThinkingDelta {
                content_index: 0,
                delta: "reasoning...".to_string(),
                partial: empty_partial(),
            },
            ProviderEvent::ThinkingEnd {
                content_index: 0,
                content: "reasoning...".to_string(),
                partial: empty_partial(),
            },
            ProviderEvent::ToolCallStart {
                content_index: 1,
                tool_call_id: Some("tc_abc".to_string()),
                tool_name: Some("bash".to_string()),
                partial: empty_partial(),
            },
            ProviderEvent::ToolCallDelta {
                content_index: 1,
                delta: "{\"command\":".to_string(),
                partial: empty_partial(),
            },
            ProviderEvent::ToolCallDelta {
                content_index: 1,
                delta: "\"ls\"}".to_string(),
                partial: empty_partial(),
            },
            ProviderEvent::ToolCallEnd {
                content_index: 1,
                tool_call: finalized,
                partial: empty_partial(),
            },
            ProviderEvent::Done {
                reason: StopReason::Stop,
                message: done_msg,
            },
        ];

        let emitted = run_script(events).await;

        let thinking_end_at = emitted
            .iter()
            .position(|e| matches!(e, AgentEvent::ThinkingEnd));
        assert!(
            thinking_end_at.is_some(),
            "AgentEvent::ThinkingEnd must be emitted"
        );

        let deltas: Vec<(&str, &str)> = emitted
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolCallDelta {
                    tool_call_id,
                    args_delta,
                } => Some((tool_call_id.as_str(), args_delta.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(deltas.len(), 2, "expected exactly two ToolCallDelta events");
        assert_eq!(deltas[0], ("tc_abc", "{\"command\":"));
        assert_eq!(deltas[1], ("tc_abc", "\"ls\"}"));

        let first_delta_at = emitted
            .iter()
            .position(|e| matches!(e, AgentEvent::ToolCallDelta { .. }))
            .expect("at least one ToolCallDelta");
        assert!(
            thinking_end_at.unwrap() < first_delta_at,
            "ThinkingEnd must precede ToolCallDelta"
        );
    }

    /// OpenAI-style: the first `ToolCallStart` carries no id (name only),
    /// then a second id-bearing delta re-emits `ToolCallStart`. The map must
    /// pick up the id so later `ToolCallDelta`s resolve.
    #[tokio::test]
    async fn tool_call_delta_resolves_late_id() {
        let finalized = ToolCall::new("tc_late", "grep", serde_json::json!({"pattern":"foo"}));
        let mut done_msg = AssistantMessage::new(Api::AnthropicMessages, "mock", "test/model");
        done_msg
            .content
            .push(ContentBlock::ToolCall(finalized.clone()));
        let events = vec![
            ProviderEvent::Start {
                partial: empty_partial(),
            },
            ProviderEvent::ToolCallStart {
                content_index: 0,
                tool_call_id: None,
                tool_name: Some("grep".to_string()),
                partial: empty_partial(),
            },
            ProviderEvent::ToolCallStart {
                content_index: 0,
                tool_call_id: Some("tc_late".to_string()),
                tool_name: Some("grep".to_string()),
                partial: empty_partial(),
            },
            ProviderEvent::ToolCallDelta {
                content_index: 0,
                delta: "{\"pattern\":".to_string(),
                partial: empty_partial(),
            },
            ProviderEvent::ToolCallEnd {
                content_index: 0,
                tool_call: finalized,
                partial: empty_partial(),
            },
            ProviderEvent::Done {
                reason: StopReason::Stop,
                message: done_msg,
            },
        ];

        let emitted = run_script(events).await;

        let ids: Vec<String> = emitted
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolCallDelta { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec!["tc_late".to_string()],
            "ToolCallDelta must resolve the id from the second ToolCallStart"
        );
    }
}
