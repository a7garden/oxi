/// Streaming implementation for agent loop

use anyhow::{Error, Result};
use futures::StreamExt;
use oxi_ai::{
    ContentBlock, Context, Message, ProviderEvent, StreamOptions, TextContent, ThinkingContent,
    Tool as OxTool,
};

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

    let tool_defs = loop_ref.tools.definitions();
    if !tool_defs.is_empty() {
        let mut oxi_tools = Vec::new();
        for def in &tool_defs {
            let schema = serde_json::to_value(&def.input_schema).unwrap_or_else(|_| {
                serde_json::json!({"type": "object", "properties": {}})
            });
            oxi_tools.push(OxTool::new(&def.name, &def.description, schema));
        }
        context.set_tools(oxi_tools);
    }

    let stream_options = StreamOptions {
        temperature: Some(loop_ref.config.temperature as f64),
        max_tokens: Some(loop_ref.config.max_tokens as usize),
        ..Default::default()
    };

    let stream = super::retry::stream_with_retry(loop_ref, &model, &context, Some(stream_options), emit).await?;

    let mut partial_message: Option<oxi_ai::AssistantMessage> = None;
    let mut added_partial = false;

    let mut rx = stream;
    while let Some(event) = rx.next().await {
        match event {
            ProviderEvent::Start { partial } => {
                partial_message = Some(partial.clone());
                messages.push(Message::Assistant(partial.clone()));
                added_partial = true;
                emit(super::AgentEvent::MessageStart { message: messages.last().unwrap().clone() });
            }

            ProviderEvent::TextDelta { delta, partial, .. } => {
                if let Some(ref mut partial) = partial_message {
                    if let Some(last) = partial.content.last_mut() {
                        if let ContentBlock::Text(t) = last {
                            t.text.push_str(&delta);
                        }
                    } else {
                        partial.content.push(ContentBlock::Text(TextContent::new(delta.clone())));
                    }
                    emit(super::AgentEvent::MessageUpdate {
                        message: Message::Assistant(partial.clone()),
                        delta: Some(delta.clone()),
                    });
                }
                let _ = partial;
            }

            ProviderEvent::ThinkingStart { partial, .. } => {
                if let Some(ref mut partial) = partial_message {
                    partial.content.push(ContentBlock::Thinking(ThinkingContent::new("")));
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
                let _ = partial;
            }

            ProviderEvent::ToolCallEnd { tool_call, partial, .. } => {
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
                emit(super::AgentEvent::MessageEnd { message: Message::Assistant(message.clone()) });
                return Ok(message);
            }

            ProviderEvent::Error { error, .. } => {
                let raw_msg = error.text_content();
                let friendly = if raw_msg.is_empty() {
                    "Unknown provider error".to_string()
                } else {
                    raw_msg
                };
                tracing::error!(session_id = ?loop_ref.session_id, "Provider stream error: {}", friendly);
                emit(super::AgentEvent::Error { message: format!("⚠ {}", friendly), session_id: loop_ref.session_id.clone() });
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

    emit(super::AgentEvent::MessageEnd { message: Message::Assistant(final_message.clone()) });
    Ok(final_message)
}