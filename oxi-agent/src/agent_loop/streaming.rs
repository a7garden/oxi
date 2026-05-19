/// Streaming implementation for agent loop.
///
/// pi-mono pattern: the provider accumulates content into a single `output`
/// message. Each event carries a snapshot (`partial`) of this message.
/// Done carries the complete accumulated message.
///
/// This module simply forwards events to the agent loop emit function.
use anyhow::{Error, Result};
use futures::StreamExt;
use oxi_ai::{
    ContentBlock, Context, Message, ProviderEvent, StopReason, StreamOptions, Tool as OxTool,
};
use std::collections::HashSet;

pub(crate) async fn stream_assistant_response(
    loop_ref: &super::AgentLoop,
    messages: &mut Vec<Message>,
    emit: &super::EmitFn,
) -> Result<oxi_ai::AssistantMessage> {
    let model = loop_ref.resolve_model()?;

    let mut context = Context::new();

    if let Some(ref system_prompt) = loop_ref.config.system_prompt {
        context.set_system_prompt(system_prompt.clone());
    }

    for msg in messages.iter() {
        context.add_message(msg.clone());
    }

    // Cache tool definitions serialization once to avoid repeated serde work.
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
        ..Default::default()
    };

    let stream =
        super::retry::stream_with_retry(loop_ref, &model, &context, Some(stream_options), emit)
            .await?;

    // pi-mono pattern: track whether we've emitted MessageStart.
    // Start event initializes the stream. Subsequent deltas carry
    // accumulated partial messages (content grows in-place at the provider).
    let mut added_partial = false;
    let mut event_count = 0u32;

    let mut rx = stream;
    while let Some(event) = rx.next().await {
        event_count += 1;
        match event {
            ProviderEvent::Start { partial } => {
                tracing::info!("Stream event #{}: Start", event_count);
                messages.push(Message::Assistant(partial));
                added_partial = true;
                emit(super::AgentEvent::MessageStart {
                    message: messages.last().expect("non-empty after push").clone(),
                });
            }

            ProviderEvent::TextDelta { delta, partial, .. } => {
                // Replace the last assistant message with the provider's
                // accumulated snapshot (pi-mono: content grows in partial).
                if added_partial {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = partial;
                    }
                }
                let last_msg = messages.last().expect("non-empty").clone();
                emit(super::AgentEvent::MessageUpdate {
                    message: last_msg,
                    delta: Some(delta),
                });
            }

            ProviderEvent::ThinkingStart { partial, .. }
                // ThinkingStart arrives before ThinkingDelta.
                // Update the snapshot.
                if added_partial => {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = partial;
                    }
                }

            ProviderEvent::ThinkingDelta { delta, partial, .. } => {
                if added_partial {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = partial;
                    }
                }
                let last_msg = messages.last().expect("non-empty").clone();
                emit(super::AgentEvent::MessageUpdate {
                    message: last_msg,
                    delta: Some(delta),
                });
            }

            ProviderEvent::ToolCallStart { partial, .. }
                if added_partial => {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = partial;
                    }
                }

            ProviderEvent::ToolCallDelta { partial, .. }
                if added_partial => {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        *m = partial;
                    }
                }

            ProviderEvent::ToolCallEnd { tool_call, .. }
                // Add the tool call directly to our tracked message.
                if added_partial => {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        m.content.push(ContentBlock::ToolCall(tool_call));
                    }
                    // CRITICAL: emit MessageUpdate so the TUI sees the ToolCall block.
                    // Without this, tool calls are never rendered (matching pi's behavior
                    // where toolcall_end emits message_update).
                    let last_msg = messages.last().expect("non-empty").clone();
                    emit(super::AgentEvent::MessageUpdate {
                        message: last_msg,
                        delta: None,
                    });
                }

            ProviderEvent::Done { message, .. } => {
                // Record success in circuit breaker — the provider returned a
                // complete response without errors.
                loop_ref.circuit_breaker.record_success();

                tracing::info!(
                    "Stream event #{}: Done (stop_reason={:?})",
                    event_count,
                    message.stop_reason
                );
                if added_partial {
                    let last_idx = messages.len() - 1;
                    if let Message::Assistant(ref mut m) = messages[last_idx] {
                        // Preserve tool calls we may have injected via ToolCallEnd.
                        // Some providers also include ToolCall blocks in the final Done message,
                        // so dedupe by tool_call_id to avoid executing the same tool twice.
                        let mut preserved_tool_calls: Vec<ContentBlock> = m
                            .content
                            .drain(..)
                            .filter(|b| matches!(b, ContentBlock::ToolCall(_)))
                            .collect();

                        let mut seen: HashSet<String> = message
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolCall(tc) => Some(tc.id.clone()),
                                _ => None,
                            })
                            .collect();

                        preserved_tool_calls.retain(|b| match b {
                            ContentBlock::ToolCall(tc) => seen.insert(tc.id.clone()),
                            _ => true,
                        });

                        tracing::info!(
                            "Done: preserving {} tool_calls (deduped), Done message has {} content blocks",
                            preserved_tool_calls.len(),
                            message.content.len()
                        );

                        *m = message.clone();
                        m.content.extend(preserved_tool_calls);
                        tracing::info!(
                            "Done: final message has {} content blocks, stop_reason={:?}",
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
                // Return the message we actually stored (with tool calls preserved)
                if let Message::Assistant(m) = &last_msg {
                    return Ok(m.clone());
                } else {
                    return Ok(message);
                }
            }

            ProviderEvent::Error { mut error, .. } => {
                // Record failure in circuit breaker — the provider stream
                // produced an error event after the connection was established.
                // (connection-level failures are recorded in stream_with_retry)
                loop_ref.circuit_breaker.record_failure();

                tracing::info!("Stream event #{}: Error", event_count);
                let raw_msg = error.text_content();
                let friendly = if raw_msg.is_empty() {
                    "Unknown provider error".to_string()
                } else {
                    raw_msg
                };
                tracing::error!(session_id = ?loop_ref.session_id, "Provider stream error: {}", friendly);

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

                return Ok(error);
            }

            _ => {}
        }
    }

    tracing::info!("Stream ended after {} events", event_count);

    let final_message = messages
        .last()
        .and_then(|m| match m {
            Message::Assistant(a) => Some(a.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::msg("No assistant message in context"))?;

    emit(super::AgentEvent::MessageEnd {
        message: Message::Assistant(final_message.clone()),
    });
    Ok(final_message)
}
