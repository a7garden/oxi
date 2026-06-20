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
use std::collections::HashSet;

use super::helpers::sanitize_orphaned_tool_results;
use super::stream_outcome::StreamOutcome;

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

    if let Some(ref system_prompt) = loop_ref.config.system_prompt {
        context.set_system_prompt(system_prompt.clone());
    }

    for msg in messages.iter() {
        context.add_message(msg.clone());
    }

    let tool_defs = loop_ref.tools.definitions();
    if !tool_defs.is_empty() {
        let mut oxi_tools = Vec::with_capacity(tool_defs.len());
        for def in &tool_defs {
            let schema = serde_json::to_value(&def.input_schema)
                .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
            oxi_tools.push(OxTool::new(&def.name, &def.description, schema));
        }
        context.set_tools(oxi_tools);
    }

    let stream_options = StreamOptions {
        temperature: Some(loop_ref.config.temperature as f64),
        max_tokens: Some(loop_ref.config.max_tokens as usize),
        api_key: loop_ref.config.api_key.clone(),
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

            ProviderEvent::FallbackStart {
                from_model,
                to_model,
                ..
            } => {
                tracing::info!(
                    "Stream event #{}: Fallback from {} to {}",
                    event_count,
                    from_model,
                    to_model
                );
                emit(super::AgentEvent::Fallback {
                    from_model,
                    to_model,
                });
            }

            ProviderEvent::FallbackExhausted {
                models_tried,
                final_error,
            } => {
                tracing::warn!(
                    "Stream event #{}: All fallback models exhausted. Tried: {:?}, error: {}",
                    event_count,
                    models_tried,
                    final_error
                );
                if let Some(last_model) = models_tried.last() {
                    emit(super::AgentEvent::Fallback {
                        from_model: last_model.clone(),
                        to_model: "none".to_string(),
                    });
                }
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
            }

            ProviderEvent::ThinkingDelta { delta, partial, .. } => {
                if added_partial {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = (*partial).clone();
                    }
                }
                let last_msg = messages.last().expect("non-empty").clone();
                emit(super::AgentEvent::MessageUpdate {
                    message: last_msg,
                    delta: Some(delta),
                });
            }

            ProviderEvent::ToolCallStart { partial, .. } if added_partial => {
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    *m = (*partial).clone();
                }
            }

            ProviderEvent::ToolCallDelta { partial, .. } if added_partial => {
                let last_idx = messages.len() - 1;
                if let Message::Assistant(ref mut m) = messages[last_idx] {
                    *m = (*partial).clone();
                }
            }

            ProviderEvent::ToolCallEnd { tool_call, .. } if added_partial => {
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
                loop_ref.circuit_breaker.record_success();

                let (input, output) = (message.usage.input, message.usage.output);
                if input > 0 || output > 0 {
                    loop_ref.state.update(|s| {
                        s.record_usage(input, output);
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
                loop_ref.circuit_breaker.record_failure();

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

                return StreamOutcome::Error { message: error, detail: format!("⚠ {}", friendly) };
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
