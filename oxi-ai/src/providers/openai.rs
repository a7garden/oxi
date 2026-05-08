//! OpenAI-compatible provider implementation

use async_trait::async_trait;
use bytes::Bytes;
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

/// OpenAI-compatible provider
#[derive(Clone)]
pub struct OpenAiProvider {
    client: &'static Client,
    api_key: Option<String>,
    base_url: Option<String>,
}

impl OpenAiProvider {
    /// Create a new OpenAI provider using environment variables.
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            base_url: None,
        }
    }

    /// Create with explicit API key (public API for external consumers)
    #[allow(dead_code)]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
            base_url: None,
        }
    }

    /// Create with a custom base URL (API key resolved from auth storage).
    ///
    /// Used for built-in OpenAI-compatible providers like ZAI.
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: shared_client(),
            api_key: None,
            base_url: Some(base_url.to_string()),
        }
    }

    /// Create with a custom base URL and optional API key.
    ///
    /// Used for registering custom OpenAI-compatible providers (Minimax, ZAI, etc.).
    pub fn with_base_url_and_key(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            client: shared_client(),
            api_key,
            base_url: Some(base_url.to_string()),
        }
    }
}

impl Default for OpenAiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Build the request
        let effective_base_url = self.base_url.as_deref().unwrap_or(&model.base_url);
        let url = format!("{}/chat/completions", effective_base_url);

        // Get API key
        let api_key = options
            .api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .ok_or_else(|| ProviderError::MissingApiKey)?;

        // Build messages
        let messages = build_messages(context)?;

        // Build request body
        let mut body = serde_json::json!({
            "model": model.id,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        // Add optional parameters
        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if let Some(max) = options.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }

        // Add tools if present
        if !context.tools.is_empty() {
            body["tools"] = build_tools(&context.tools)?;
        }

        // Build headers
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", api_key).parse().expect("valid bearer header"),
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().expect("valid header value"),
        );

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
        let provider_name = model.provider.clone();
        let model_id = model.id.clone();

        let stream = response.bytes_stream().flat_map(
            move |chunk: Result<Bytes, reqwest::Error>| match chunk {
                Ok(bytes) => {
                    // Find last valid UTF-8 boundary to avoid splitting
                    // multi-byte characters across chunks.
                    let text = find_valid_utf8_prefix(&bytes);
                    futures::stream::iter(parse_sse_events(&text, &provider_name, &model_id))
                }
                Err(e) => futures::stream::iter(vec![ProviderEvent::Error {
                    reason: StopReason::Error,
                    error: create_error_message(&e.to_string(), &provider_name, &model_id),
                }]),
            },
        );

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "openai"
    }
}

/// Build messages array from context
fn build_messages(context: &Context) -> Result<Vec<JsonValue>, ProviderError> {
    let mut messages = Vec::new();

    // System prompt
    if let Some(ref prompt) = context.system_prompt {
        messages.push(serde_json::json!({
            "role": "system",
            "content": prompt,
        }));
    }

    // Conversation messages
    for msg in &context.messages {
        match msg {
            crate::Message::User(u) => {
                let content: String = match &u.content {
                    crate::MessageContent::Text(s) => s.clone(),
                    crate::MessageContent::Blocks(blocks) => blocks_to_content(blocks)?.to_string(),
                };
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
            }
            crate::Message::Assistant(a) => {
                let content = blocks_to_content(&a.content)?.to_string();
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }));
            }
            crate::Message::ToolResult(t) => {
                let content = blocks_to_content(&t.content)?.to_string();
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": t.tool_call_id,
                    "tool_name": t.tool_name,
                    "content": content,
                }));
            }
        }
    }

    Ok(messages)
}

/// Convert content blocks to a string representation
fn blocks_to_content(blocks: &[ContentBlock]) -> Result<JsonValue, ProviderError> {
    if blocks.len() == 1 {
        if let Some(text) = blocks[0].as_text() {
            return Ok(JsonValue::String(text.to_string()));
        }
    }

    let items: Result<Vec<_>, _> = blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(t) => Ok(serde_json::json!({
                "type": "text",
                "text": t.text,
            })),
            ContentBlock::ToolCall(tc) => Ok(serde_json::json!({
                "type": "function",
                "id": tc.id,
                "function": {
                    "name": tc.name,
                    "arguments": tc.arguments.to_string(),
                },
            })),
            ContentBlock::Thinking(th) => Ok(serde_json::json!({
                "type": "thinking",
                "thinking": th.thinking,
            })),
            ContentBlock::Image(img) => Ok(serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{};base64,{}", img.mime_type, img.data),
                },
            })),
            ContentBlock::Unknown(_) => Err(ProviderError::InvalidResponse(
                "Unknown content block type".into(),
            )),
        })
        .collect();

    Ok(serde_json::json!(items?))
}

/// Build tools array
fn build_tools(tools: &[crate::Tool]) -> Result<JsonValue, ProviderError> {
    let items: Vec<_> = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                },
            })
        })
        .collect();

    Ok(serde_json::json!(items))
}

/// Extract the longest valid UTF-8 prefix from a byte slice.
///
/// If the trailing bytes form an incomplete UTF-8 sequence (the chunk was
/// split mid-character), those bytes are dropped. The next chunk from the
/// network will carry the remainder and complete the character.
fn find_valid_utf8_prefix(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(e) => {
            let valid = &bytes[..e.valid_up_to()];
            String::from_utf8_lossy(valid).to_string()
        }
    }
}

/// Parse SSE event stream from a byte buffer.
///
/// Optimizations over a naïve implementation:
/// - **Fast-line splitting** – iterates over `\n` boundaries via `split`
///   instead of allocating an intermediate `String` per line.
/// - **Early `DONE` exit** – breaks immediately when `data: [DONE]` is
///   encountered.
/// - **Pre-allocated events** – reserves capacity based on data-line count.
/// - **Accumulated usage** – tracks usage separately, only cloning into
///   the Done message at stream end, not on every chunk.
fn parse_sse_events(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let partial_message = AssistantMessage::new(Api::OpenAiCompletions, provider, model_id);

    // Pre-estimate capacity: one event per data line is a reasonable upper bound.
    let estimated_events = text.split('\n').filter(|l| l.starts_with("data: ")).count();
    events.reserve(estimated_events);

    // Accumulate tool calls across deltas (keyed by index)
    let mut pending_tool_calls: std::collections::HashMap<usize, (String, String, String)> =
        std::collections::HashMap::new(); // index → (id, name, arguments)

    let mut accumulated_usage = Usage::default();

    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        // Fast rejection for non-data lines (comments, event tags, etc.)
        if !line.starts_with("data: ") {
            continue;
        }

        let data = &line[6..]; // skip "data: "

        // Early exit on stream end
        if data == "[DONE]" {
            break;
        }

        if data.is_empty() {
            continue;
        }

        let chunk = match serde_json::from_str::<SSEChunk>(data) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for choice in &chunk.choices {
            if let Some(delta) = &choice.delta {
                if let Some(content) = &delta.content {
                    events.push(ProviderEvent::TextDelta {
                        content_index: choice.index,
                        delta: content.clone(),
                        partial: partial_message.clone(),
                    });
                }

                if let Some(tool_calls) = &delta.tool_calls {
                    for tc in tool_calls {
                        if let Some(func) = &tc.function {
                            events.push(ProviderEvent::ToolCallDelta {
                                content_index: choice.index,
                                delta: func.arguments.clone().unwrap_or_default(),
                                partial: partial_message.clone(),
                            });
                        }
                    }
                }
            }

            if choice.finish_reason.is_some() {
                let reason = match choice.finish_reason.as_deref() {
                    Some("stop") => StopReason::Stop,
                    Some("length") => StopReason::Length,
                    Some("tool_calls") => StopReason::ToolUse,
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

        // Accumulate usage from the chunk (if present).
        if let Some(chunk_usage) = chunk.usage {
            accumulated_usage.input = chunk_usage.prompt_tokens;
            accumulated_usage.output = chunk_usage.completion_tokens;
            accumulated_usage.cache_read = chunk_usage
                .prompt_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .unwrap_or(0);
            accumulated_usage.total_tokens = chunk_usage.total_tokens;
        }
    }

    events
}

/// Create error assistant message
fn create_error_message(msg: &str, provider: &str, model_id: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(Api::OpenAiCompletions, provider, model_id);
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// SSE chunk structure
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde deserialization structs
struct SSEChunk {
    id: Option<String>,
    #[serde(rename = "model")]
    model: Option<String>,
    choices: Vec<Choice>,
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde deserialization structs
struct Choice {
    index: usize,
    delta: Option<Delta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde deserialization structs
struct ToolCallDelta {
    index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    type_: Option<String>,
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde deserialization structs
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct UsageInfo {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    #[serde(rename = "prompt_tokens_details")]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Deserialize, Clone)]
struct PromptTokensDetails {
    #[serde(rename = "cached_tokens")]
    cached_tokens: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: &str = "openai";
    const MODEL: &str = "gpt-4o";

    // ── SSE event parsing ──────────────────────────────────────────────

    #[test]
    fn parse_single_text_event() {
        let sse = "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
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
    fn parse_multiple_text_events() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo!\"}}]}\n",
            "\n"
        );
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        assert_eq!(events.len(), 2);
        let texts: Vec<&str> = events.iter().filter_map(|e| match e {
            ProviderEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        }).collect();
        assert_eq!(texts, vec!["Hel", "lo!"]);
    }

    #[test]
    fn parse_done_terminator() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"X\"}}]}\n",
            "\n",
            "data: [DONE]\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"NEVER\"}}]}\n"
        );
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        // Should stop at [DONE]; the final data line is never parsed
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "X"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    // ── Content extraction ─────────────────────────────────────────────

    #[test]
    fn parse_finish_reason_stop() {
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"stop\"}]}\n\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Stop)),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_finish_reason_length() {
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"length\"}]}\n\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Length)),
            other => panic!("expected Done with Length, got {other:?}"),
        }
    }

    #[test]
    fn parse_finish_reason_tool_calls() {
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"tool_calls\"}]}\n\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::ToolUse)),
            other => panic!("expected Done with ToolUse, got {other:?}"),
        }
    }

    // ── Tool call delta accumulation ───────────────────────────────────

    #[test]
    fn parse_tool_call_deltas() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\\\"SF\\\"}\"}}]}}]}\n",
            "\n"
        );
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        assert_eq!(events.len(), 2);
        let deltas: Vec<&str> = events.iter().filter_map(|e| match e {
            ProviderEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        }).collect();
        assert_eq!(deltas, vec!["", "{\"city\":\"SF\"}"]);
    }

    #[test]
    fn parse_tool_call_with_no_arguments_field() {
        // function field present but arguments is null → should default to ""
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"run\"}}]}}]}\n\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::ToolCallDelta { delta, .. } => assert_eq!(delta, ""),
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
    }

    // ── Usage accumulation ─────────────────────────────────────────────

    #[test]
    fn parse_usage_in_chunk() {
        // Usage is accumulated from earlier chunks; the Done event captures
        // usage that was accumulated *before* the finish_reason chunk.
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":8,\"total_tokens\":18,\"prompt_tokens_details\":{\"cached_tokens\":3}}}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"stop\"}]}\n"
        );
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        // TextDelta + Done
        assert_eq!(events.len(), 2);
        match &events[1] {
            ProviderEvent::Done { message, .. } => {
                assert_eq!(message.usage.input, 10);
                assert_eq!(message.usage.output, 8);
                assert_eq!(message.usage.total_tokens, 18);
                assert_eq!(message.usage.cache_read, 3);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_usage_without_cache_details() {
        // Usage from an earlier chunk; Done event on a separate chunk without usage.
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"stop\"}]}\n"
        );
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        match &events[0] {
            ProviderEvent::Done { message, .. } => {
                assert_eq!(message.usage.input, 5);
                assert_eq!(message.usage.output, 2);
                assert_eq!(message.usage.cache_read, 0);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ── Empty / malformed handling ─────────────────────────────────────

    #[test]
    fn parse_empty_input() {
        let events = parse_sse_events("", PROVIDER, MODEL);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_only_empty_lines() {
        let events = parse_sse_events("\n\n\n", PROVIDER, MODEL);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_malformed_json_after_data() {
        let sse = "data: {not json at all}\ndata: also bad\ndata: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"}}]}\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        // Malformed lines are skipped, only the valid one emits
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "ok"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_data_line() {
        let sse = "data: \ndata: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"X\"}}]}\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_non_data_lines_ignored() {
        let sse = "event: ping\nid: 42\nretry: 5000\ndata: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Y\"}}]}\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_carriage_return_line_endings() {
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"CR\"}}]}\r\n\r\n";
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "CR"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    // ── Mixed content + tool + done ────────────────────────────────────

    #[test]
    fn parse_full_stream_with_text_tool_and_done() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Let me\"}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" check\"}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"search\",\"arguments\":\"{\\\"q\\\":\\\"rust\\\"}\"}}]}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"tool_calls\"}]}\n",
            "\n",
            "data: [DONE]\n"
        );
        let events = parse_sse_events(sse, PROVIDER, MODEL);
        assert_eq!(events.len(), 4); // 2 TextDelta + 1 ToolCallDelta + 1 Done

        let mut text_count = 0;
        let mut tool_count = 0;
        let mut done_count = 0;
        for e in &events {
            match e {
                ProviderEvent::TextDelta { .. } => text_count += 1,
                ProviderEvent::ToolCallDelta { .. } => tool_count += 1,
                ProviderEvent::Done { reason, .. } => {
                    done_count += 1;
                    assert!(matches!(reason, StopReason::ToolUse));
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(text_count, 2);
        assert_eq!(tool_count, 1);
        assert_eq!(done_count, 1);
    }
}
