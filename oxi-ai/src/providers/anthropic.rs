//! Anthropic provider implementation

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;

use super::shared_client;
use crate::{
    error::ProviderError, Api, AssistantMessage, ContentBlock, Context, Model, Provider,
    ProviderEvent, StopReason, StreamOptions, Usage,
};

/// Anthropic provider
#[derive(Clone)]
pub struct AnthropicProvider {
    client: &'static Client,
    api_key: Option<String>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
        }
    }

    /// Create with explicit API key (public API for external consumers)
    #[allow(dead_code)]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
        }
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Build the request
        let url = format!("{}/v1/messages", model.base_url);

        // Get API key
        let api_key = options
            .api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .ok_or_else(|| ProviderError::MissingApiKey)?;

        // Build messages
        let messages = build_anthropic_messages(context)?;

        // Build request body
        let mut body = serde_json::json!({
            "model": model.id,
            "messages": messages,
            "stream": true,
        });

        // Add system prompt
        if let Some(ref prompt) = context.system_prompt {
            body["system"] = serde_json::json!(prompt);
        }

        // Add optional parameters
        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if let Some(max) = options.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }

        // Add tools if present
        if !context.tools.is_empty() {
            body["tools"] = build_anthropic_tools(&context.tools)?;
        }

        // Build headers
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());

        for (k, v) in &options.headers {
            if let (Ok(name), Ok(value)) = (
                k.parse::<reqwest::header::HeaderName>(),
                v.parse::<reqwest::header::HeaderValue>(),
            ) {
                headers.insert(name, value);
            }
        }

        // Make request
        let response = self
            .client
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::RequestFailed)?;

        if !response.status().is_success() {
            let status = response.status();
            let body: String = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpError(status.as_u16(), body));
        }

        // Create event stream
        let model_name = model.id.clone();

        let stream = response.bytes_stream().flat_map(move |chunk| match chunk {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                futures::stream::iter(parse_anthropic_events(&text, &model_name))
            }
            Err(e) => futures::stream::iter(vec![ProviderEvent::Error {
                reason: StopReason::Error,
                error: create_error_message(&e.to_string()),
            }]),
        });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

/// Build messages in Anthropic format
fn build_anthropic_messages(context: &Context) -> Result<Vec<JsonValue>, ProviderError> {
    let mut messages = Vec::new();

    for msg in &context.messages {
        match msg {
            crate::Message::User(u) => {
                let content = match &u.content {
                    crate::MessageContent::Text(s) => vec![serde_json::json!({
                        "type": "text",
                        "text": s,
                    })],
                    crate::MessageContent::Blocks(blocks) => blocks_to_anthropic_content(blocks)?,
                };
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
            }
            crate::Message::Assistant(a) => {
                let content = blocks_to_anthropic_content(&a.content)?;
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }));
            }
            crate::Message::ToolResult(t) => {
                let content = blocks_to_anthropic_content(&t.content)?;
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": t.tool_call_id,
                        "content": content,
                    }],
                }));
            }
        }
    }

    Ok(messages)
}

/// Convert content blocks to Anthropic format
fn blocks_to_anthropic_content(blocks: &[ContentBlock]) -> Result<Vec<JsonValue>, ProviderError> {
    let mut items = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text(t) => {
                items.push(serde_json::json!({
                    "type": "text",
                    "text": t.text,
                }));
            }
            ContentBlock::ToolCall(tc) => {
                items.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.name,
                    "input": tc.arguments,
                }));
            }
            ContentBlock::Thinking(th) => {
                items.push(serde_json::json!({
                    "type": "thinking",
                    "thinking": th.thinking,
                }));
            }
            ContentBlock::Image(img) => {
                items.push(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": img.mime_type,
                        "data": img.data,
                    },
                }));
            }
            ContentBlock::Unknown(_) => {
                // Skip unknown blocks
            }
        }
    }

    Ok(items)
}

/// Build tools in Anthropic format
fn build_anthropic_tools(tools: &[crate::Tool]) -> Result<JsonValue, ProviderError> {
    let items: Vec<_> = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.parameters,
            })
        })
        .collect();

    Ok(serde_json::json!(items))
}

/// Parse Anthropic SSE event stream.
///
/// Optimizations:
/// - Fast-line splitting via `split('\n')`.
/// - Early exit on terminal events.
/// - Pre-allocated events vector.
/// - Accumulated usage tracked separately, only cloned into Done message.
fn parse_anthropic_events(text: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let partial_message = AssistantMessage::new(Api::AnthropicMessages, "anthropic", model_id);

    // Pre-allocate based on data-line count
    let estimated = text.split('\n').filter(|l| l.starts_with("data: ")).count();
    events.reserve(estimated);

    let mut accumulated_usage = Usage::default();

    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        if !line.starts_with("data: ") {
            continue;
        }

        let data = &line[6..];

        if data == "[DONE]" || data.is_empty() {
            continue;
        }

        let event = match serde_json::from_str::<AnthropicEvent>(data) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let event_type = event.type_.as_deref();

        match event_type {
            Some("message_start") => {
                events.push(ProviderEvent::Start {
                    partial: partial_message.clone(),
                });
            }
            Some("content_block_start") => {
                if let Some(block) = &event.content_block {
                    // Use block-level index if present, fall back to event-level index
                    let idx = block.index.or(event.index).unwrap_or(0);
                    match block.type_.as_deref() {
                        Some("text") => {
                            events.push(ProviderEvent::TextStart {
                                content_index: idx,
                                partial: partial_message.clone(),
                            });
                        }
                        Some("thinking") => {
                            events.push(ProviderEvent::ThinkingStart {
                                content_index: idx,
                                partial: partial_message.clone(),
                            });
                        }
                        Some("tool_use") => {
                            events.push(ProviderEvent::ToolCallStart {
                                content_index: idx,
                                tool_call_id: block.id.clone(),
                                partial: partial_message.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
            Some("content_block_delta") => {
                if let Some(delta) = &event.delta {
                    match delta.type_.as_deref() {
                        Some("text_delta") => {
                            if let Some(text) = &delta.text {
                                events.push(ProviderEvent::TextDelta {
                                    content_index: event.index.unwrap_or(0),
                                    delta: text.clone(),
                                    partial: partial_message.clone(),
                                });
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(text) = &delta.thinking {
                                events.push(ProviderEvent::ThinkingDelta {
                                    content_index: event.index.unwrap_or(0),
                                    delta: text.clone(),
                                    partial: partial_message.clone(),
                                });
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(args) = &delta.partial_json {
                                events.push(ProviderEvent::ToolCallDelta {
                                    content_index: event.index.unwrap_or(0),
                                    delta: args.clone(),
                                    partial: partial_message.clone(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("message_delta") => {
                if let Some(delta) = &event.delta {
                    let reason = match delta.stop_reason.as_deref() {
                        Some("end_turn") => StopReason::Stop,
                        Some("max_tokens") => StopReason::Length,
                        Some("stop_sequence") => StopReason::Stop,
                        _ => StopReason::Stop,
                    };

                    let mut done_msg = partial_message.clone();
                    done_msg.usage = accumulated_usage.clone();
                    events.push(ProviderEvent::Done {
                        reason,
                        message: done_msg,
                    });
                }
            }
            Some("message_stop") => {
                // Message complete – no event needed
            }
            _ => {}
        }

        // Accumulate usage
        if let Some(usage) = event.usage {
            accumulated_usage.input = usage.input_tokens;
            accumulated_usage.output = usage.output_tokens;
            accumulated_usage.cache_read = usage.cache_read;
            accumulated_usage.cache_write = usage.cache_creation;
            accumulated_usage.total_tokens = usage.input_tokens + usage.output_tokens;
        }
    }

    events
}

/// Create error assistant message
fn create_error_message(msg: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(Api::AnthropicMessages, "anthropic", "unknown");
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// Anthropic event structure
#[derive(Debug, Deserialize)]
struct AnthropicEvent {
    #[serde(rename = "type")]
    type_: Option<String>,
    #[serde(rename = "index")]
    index: Option<usize>,
    content_block: Option<ContentBlockStart>,
    delta: Option<Delta>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct ContentBlockStart {
    #[serde(rename = "type")]
    type_: Option<String>,
    index: Option<usize>,
    /// Tool call ID present for tool_use blocks (Anthropic sends this in content_block_start)
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    type_: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    partial_json: Option<String>,
    #[serde(rename = "stop_reason")]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(rename = "input_tokens")]
    input_tokens: usize,
    #[serde(rename = "output_tokens")]
    output_tokens: usize,
    #[serde(rename = "cache_read")]
    cache_read: usize,
    #[serde(rename = "cache_creation")]
    cache_creation: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "claude-3-5-sonnet-20241022";

    // ── message_start ──────────────────────────────────────────────────

    #[test]
    fn parse_message_start() {
        let sse = "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ProviderEvent::Start { .. }));
    }

    // ── content_block_start ────────────────────────────────────────────

    #[test]
    fn parse_text_block_start() {
        let sse = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextStart { content_index, .. } => assert_eq!(*content_index, 0),
            other => panic!("expected TextStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_block_start() {
        let sse = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ProviderEvent::ThinkingStart { .. }));
    }

    #[test]
    fn parse_tool_use_block_start() {
        let sse = "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"search\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::ToolCallStart { content_index, .. } => assert_eq!(*content_index, 1),
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
    }

    // ── content_block_delta ────────────────────────────────────────────

    #[test]
    fn parse_text_delta() {
        let sse = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, content_index, .. } => {
                assert_eq!(delta, "Hello");
                assert_eq!(*content_index, 0);
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_delta() {
        let sse = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me reason...\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::ThinkingDelta { delta, .. } => assert_eq!(delta, "Let me reason..."),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_input_json_delta() {
        let sse = "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"SF\\\"}\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::ToolCallDelta { delta, content_index, .. } => {
                assert_eq!(delta, "{\"city\":\"SF\"}");
                assert_eq!(*content_index, 1);
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
    }

    // ── message_delta (completion) ─────────────────────────────────────

    #[test]
    fn parse_message_delta_end_turn() {
        let sse = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Stop)),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_message_delta_max_tokens() {
        let sse = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Length)),
            other => panic!("expected Done with Length, got {other:?}"),
        }
    }

    #[test]
    fn parse_message_delta_stop_sequence() {
        let sse = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"stop_sequence\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Stop)),
            other => panic!("expected Done with Stop, got {other:?}"),
        }
    }

    // ── message_stop ───────────────────────────────────────────────────

    #[test]
    fn parse_message_stop_no_event_emitted() {
        let sse = "data: {\"type\":\"message_stop\"}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert!(events.is_empty());
    }

    // ── Thinking block full flow ───────────────────────────────────────

    #[test]
    fn parse_thinking_block_flow() {
        let sse = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"I should\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" check this.\"}}\n",
            "\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], ProviderEvent::ThinkingStart { .. }));
        let thinking: Vec<&str> = events[1..].iter().filter_map(|e| match e {
            ProviderEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        }).collect();
        assert_eq!(thinking, vec!["I should", " check this."]);
    }

    // ── Usage accumulation ─────────────────────────────────────────────

    #[test]
    fn parse_usage_from_message_start() {
        // Usage accumulates from earlier events; Done captures what was accumulated
        // *before* the message_delta chunk (since usage updates happen after event emission).
        // The message_start carries initial usage, which gets captured in the Done event.
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"},\"usage\":{\"input_tokens\":100,\"output_tokens\":0,\"cache_read\":80,\"cache_creation\":20}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // Start + TextDelta + Done
        assert_eq!(events.len(), 3);
        match &events[2] {
            ProviderEvent::Done { message, .. } => {
                // Captures usage from message_start (output_tokens was 0 there)
                assert_eq!(message.usage.input, 100);
                assert_eq!(message.usage.output, 0);
                assert_eq!(message.usage.total_tokens, 100);
                assert_eq!(message.usage.cache_read, 80);
                assert_eq!(message.usage.cache_write, 20);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ── Cache metrics ──────────────────────────────────────────────────

    #[test]
    fn parse_cache_metrics() {
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"usage\":{\"input_tokens\":50,\"output_tokens\":0,\"cache_read\":40,\"cache_creation\":10}}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":50,\"output_tokens\":20,\"cache_read\":40,\"cache_creation\":10}}\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // Start + Done
        assert_eq!(events.len(), 2);
        match &events[1] {
            ProviderEvent::Done { message, .. } => {
                assert_eq!(message.usage.cache_read, 40);
                assert_eq!(message.usage.cache_write, 10);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ── Empty / malformed handling ─────────────────────────────────────

    #[test]
    fn parse_empty_input() {
        let events = parse_anthropic_events("", MODEL);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_done_marker_is_ignored() {
        // Anthropic uses event-type based termination, but [DONE] should be silently skipped
        let sse = "data: [DONE]\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_malformed_json_is_skipped() {
        let sse = "data: {broken\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "ok"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_non_data_lines_ignored() {
        let sse = "event: ping\nid: 42\ndata: {\"type\":\"message_start\"}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_empty_data_line_skipped() {
        let sse = "data: \ndata: {\"type\":\"message_start\"}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_unknown_event_type_ignored() {
        let sse = "data: {\"type\":\"ping\"}\ndata: {\"type\":\"message_start\"}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_carriage_return_line_endings() {
        let sse = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"CR\"}}\r\n\r\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "CR"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    // ── Full stream ────────────────────────────────────────────────────

    #[test]
    fn parse_full_anthropic_stream() {
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n",
            "\n",
            "data: {\"type\":\"message_stop\"}\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // Start + TextStart + 2×TextDelta + Done
        assert_eq!(events.len(), 5);

        assert!(matches!(&events[0], ProviderEvent::Start { .. }));
        assert!(matches!(&events[1], ProviderEvent::TextStart { .. }));

        let texts: Vec<&str> = events[2..4].iter().filter_map(|e| match e {
            ProviderEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        }).collect();
        assert_eq!(texts, vec!["Hello", " world"]);

        assert!(matches!(&events[4], ProviderEvent::Done { reason: StopReason::Stop, .. }));
    }
}
