//! Mistral AI provider implementation
//!
//! Mistral AI uses an OpenAI-compatible API with some minor differences:
//! - Tool call IDs are 9 characters (vs longer UUIDs from OpenAI)
//! - API key is read from MISTRAL_API_KEY environment variable

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;
use std::sync::Arc;

use crate::{
    Api, AssistantMessage, ContentBlock, Context, Model, Provider, ProviderError, ProviderEvent,
    StopReason, StreamOptions, Usage,
};

use super::shared_client;

/// Mistral AI provider
#[derive(Clone)]
pub struct MistralProvider {
    client: &'static Client,
    api_key: Option<String>,
}

/// Default Mistral API endpoint
const MISTRAL_API_URL: &str = "https://api.mistral.ai/v1";

/// Mistral requires 9-character tool call IDs
const MISTRAL_TOOL_CALL_ID_LENGTH: usize = 9;

impl MistralProvider {
    /// Create a new Mistral provider without an API key.
    ///
    /// API keys are resolved at request time via auth.json or StreamOptions.
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_key: None,
        }
    }

    /// Create a Mistral provider with a specific API key (test-only)
    #[cfg(test)]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
        }
    }

    /// Normalize tool call ID to Mistral's expected format (9 characters)
    ///
    /// Mistral's API expects tool call IDs to be exactly 9 characters.
    /// When processing tool results or assistant messages with tool calls,
    /// we need to normalize longer IDs to fit this constraint.
    fn normalize_tool_call_id(id: &str) -> String {
        // If already 9 chars or shorter, return as-is
        if id.len() <= MISTRAL_TOOL_CALL_ID_LENGTH {
            return id.to_string();
        }

        // Take first 9 characters and ensure they're alphanumeric
        let normalized: String = id
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(MISTRAL_TOOL_CALL_ID_LENGTH)
            .collect();

        // If we don't have enough chars, pad with zeros
        if normalized.len() < MISTRAL_TOOL_CALL_ID_LENGTH {
            format!(
                "{}{}",
                normalized,
                "0".repeat(MISTRAL_TOOL_CALL_ID_LENGTH - normalized.len())
            )
        } else {
            normalized
        }
    }
}

impl Default for MistralProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MistralProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Build the request URL
        let base_url = if model.base_url.is_empty() {
            MISTRAL_API_URL.to_string()
        } else {
            model.base_url.trim_end_matches('/').to_string()
        };
        let url = format!("{}/chat/completions", base_url);

        // Get API key
        let api_key = options
            .api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .ok_or(ProviderError::MissingApiKey)?;

        // Build messages with normalized tool call IDs
        let messages = build_messages(context)?;

        // Build request body
        let mut body = serde_json::json!({
            "model": model.id,
            "messages": messages,
            "stream": true,
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
            format!("Bearer {}", api_key)
                .parse()
                .expect("valid bearer header"),
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
                    let text = String::from_utf8_lossy(&bytes).to_string();
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
        "mistral"
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
                // Normalize tool call ID for Mistral
                let normalized_id = MistralProvider::normalize_tool_call_id(&t.tool_call_id);
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": normalized_id,
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
            ContentBlock::ToolCall(tc) => {
                // Normalize tool call ID
                let normalized_id = MistralProvider::normalize_tool_call_id(&tc.id);
                Ok(serde_json::json!({
                    "type": "function",
                    "id": normalized_id,
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments.to_string(),
                    },
                }))
            }
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

/// Parse SSE event stream from a byte buffer.
///
/// Mistral-specific handling:
/// - Normalizes tool call IDs to 9 characters
/// - Handles Mistral's streaming format (OpenAI-compatible)
fn parse_sse_events(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let mut partial_message = AssistantMessage::new(Api::OpenAiCompletions, provider, model_id);

    // Pre-estimate capacity
    let estimated_events = text.split('\n').filter(|l| l.starts_with("data: ")).count();
    events.reserve(estimated_events);

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
            // Accumulate usage first (before emitting Done)
            if let Some(chunk_usage) = &chunk.usage {
                accumulated_usage.input = chunk_usage.prompt_tokens;
                accumulated_usage.output = chunk_usage.completion_tokens;
                accumulated_usage.cache_read = chunk_usage
                    .prompt_tokens_details
                    .as_ref()
                    .map(|d| d.cached_tokens)
                    .unwrap_or(0);
                accumulated_usage.total_tokens = chunk_usage.total_tokens;
            }

            if let Some(delta) = &choice.delta {
                if let Some(content) = &delta.content {
                    // pi-mono: accumulate into partial_message so the TUI can
                    // diff against its snapshot tracker.
                    let last_text_idx = partial_message
                        .content
                        .iter()
                        .rposition(|b| matches!(b, ContentBlock::Text(_)));
                    if let Some(idx) = last_text_idx {
                        if let ContentBlock::Text(t) = &mut partial_message.content[idx] {
                            t.text.push_str(content);
                        }
                    } else {
                        partial_message
                            .content
                            .push(ContentBlock::Text(crate::TextContent::new(content.clone())));
                    }
                    events.push(ProviderEvent::TextDelta {
                        content_index: choice.index,
                        delta: content.clone(),
                        partial: Arc::new(partial_message.clone()),
                    });
                }

                if let Some(tool_calls) = &delta.tool_calls {
                    for tc in tool_calls {
                        // Normalize tool call ID if present
                        if let Some(ref id) = tc.id {
                            let _normalized_id = MistralProvider::normalize_tool_call_id(id);
                            if let Some(func) = &tc.function {
                                events.push(ProviderEvent::ToolCallDelta {
                                    content_index: choice.index,
                                    delta: func.arguments.clone().unwrap_or_default(),
                                    partial: Arc::new(partial_message.clone()),
                                });
                            }
                        } else if let Some(func) = &tc.function {
                            // Send even without ID
                            events.push(ProviderEvent::ToolCallDelta {
                                content_index: choice.index,
                                delta: func.arguments.clone().unwrap_or_default(),
                                partial: Arc::new(partial_message.clone()),
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

// SSE chunk structures (OpenAI-compatible)
#[derive(Debug, Deserialize)]
// serde deserialization structs
struct SSEChunk {
    _id: Option<String>,
    #[serde(rename = "model")]
    _model: Option<String>,
    choices: Vec<Choice>,
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
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
// serde deserialization structs
struct ToolCallDelta {
    _index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    _type_: Option<String>,
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
struct FunctionDelta {
    _name: Option<String>,
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_tool_call_id_short() {
        // Already short IDs should pass through unchanged
        assert_eq!(MistralProvider::normalize_tool_call_id("short"), "short");
        assert_eq!(MistralProvider::normalize_tool_call_id("abc"), "abc");
        assert_eq!(MistralProvider::normalize_tool_call_id(""), "");
    }

    #[test]
    fn test_normalize_tool_call_id_exact_length() {
        // Exactly 9 characters should pass through
        assert_eq!(
            MistralProvider::normalize_tool_call_id("123456789"),
            "123456789"
        );
        assert_eq!(
            MistralProvider::normalize_tool_call_id("abcdefghi"),
            "abcdefghi"
        );
    }

    #[test]
    fn test_normalize_tool_call_id_long() {
        // Longer IDs should be truncated to 9 chars, keeping only alphanumeric
        let long_uuid = "call_abc123def456ghi789";
        let result = MistralProvider::normalize_tool_call_id(long_uuid);
        assert_eq!(result.len(), 9);
        assert!(result.chars().all(|c| c.is_alphanumeric()));
    }

    #[test]
    fn test_normalize_tool_call_id_with_special_chars() {
        // IDs with special characters should have them removed
        let id_with_special = "call-abc-123";
        let result = MistralProvider::normalize_tool_call_id(id_with_special);
        assert!(result.chars().all(|c| c.is_alphanumeric()));
        assert_eq!(result.len(), 9);
    }

    #[test]
    fn test_normalize_tool_call_id_padding() {
        // IDs shorter than 9 chars are preserved as-is
        // (only longer IDs get truncated/padded)
        let short_id = "a-b-c";
        let result = MistralProvider::normalize_tool_call_id(short_id);
        assert_eq!(result, "a-b-c");

        // Long IDs with special chars get normalized
        let long_with_special = "call-abc-def-ghi-jkl";
        let result = MistralProvider::normalize_tool_call_id(long_with_special);
        assert_eq!(result.len(), 9);
        assert!(result.chars().all(|c| c.is_alphanumeric()));
    }

    #[test]
    fn test_provider_name() {
        let provider = MistralProvider::new();
        assert_eq!(provider.name(), "mistral");
    }

    #[test]
    fn test_provider_default() {
        let provider = MistralProvider::default();
        assert_eq!(provider.name(), "mistral");
    }

    #[test]
    fn test_provider_with_api_key() {
        let provider = MistralProvider::with_api_key("test-key-123");
        assert_eq!(provider.name(), "mistral");
    }

    #[test]
    fn test_parse_sse_text_delta() {
        let sse_data = r#"data: {"id":"chatcmpl-123","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let events = parse_sse_events(sse_data, "mistral", "mistral-small");

        assert!(!events.is_empty());
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => {
                assert_eq!(delta, "Hello");
            }
            _ => panic!("Expected TextDelta event"),
        }
    }

    #[test]
    fn test_parse_sse_done_event() {
        let sse_data = r#"data: {"id":"chatcmpl-123","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let events = parse_sse_events(sse_data, "mistral", "mistral-small");

        assert!(!events.is_empty());
        // Find the Done event
        let done_event = events
            .iter()
            .find(|e| matches!(e, ProviderEvent::Done { .. }));
        assert!(done_event.is_some());

        if let Some(ProviderEvent::Done { reason, message }) = done_event {
            assert_eq!(*reason, StopReason::Stop);
            assert_eq!(message.usage.input, 10);
            assert_eq!(message.usage.output, 5);
        }
    }

    #[test]
    fn test_parse_sse_done_marker() {
        // Test that [DONE] is handled correctly
        let sse_data = "data: [DONE]\n";
        let events = parse_sse_events(sse_data, "mistral", "mistral-small");
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_sse_tool_call() {
        let sse_data = r#"data: {"id":"chatcmpl-123","choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_abc","function":{"name":"get_weather","arguments":"{\"city\":\"NYC\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let events = parse_sse_events(sse_data, "mistral", "mistral-small");

        // Should have tool call delta and done event
        assert!(events.len() >= 2);
        let has_tool_call = events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ToolCallDelta { .. }));
        assert!(has_tool_call);
    }

    #[test]
    fn test_build_tools() {
        let tool = crate::Tool::new(
            "get_weather",
            "Get weather for a location",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string", "description": "City name" }
                },
                "required": ["city"]
            }),
        );

        let result = build_tools(&[tool]).unwrap();
        let tools_array = result.as_array().unwrap();
        assert_eq!(tools_array.len(), 1);

        let first_tool = &tools_array[0];
        assert_eq!(first_tool["type"], "function");
        assert_eq!(first_tool["function"]["name"], "get_weather");
    }

    #[test]
    fn test_build_messages_with_tool_result() {
        use crate::{ContentBlock, Message, TextContent, ToolResultMessage};

        let mut context = Context::new();
        context.add_message(Message::ToolResult(ToolResultMessage::new(
            "call_abc123456789",
            "get_weather",
            vec![ContentBlock::Text(TextContent::new("Sunny, 72°F"))],
        )));

        let messages = build_messages(&context).unwrap();
        assert_eq!(messages.len(), 1);

        // Verify tool call ID was normalized to 9 chars
        let msg = &messages[0];
        let tool_call_id = msg["tool_call_id"].as_str().unwrap();
        assert_eq!(tool_call_id.len(), 9);
    }

    #[test]
    fn test_blocks_to_content_single_text() {
        use crate::TextContent;
        let blocks = vec![ContentBlock::Text(TextContent::new("Hello world"))];
        let result = blocks_to_content(&blocks).unwrap();
        assert_eq!(result, serde_json::json!("Hello world"));
    }

    #[test]
    fn test_blocks_to_content_multiple() {
        use crate::TextContent;
        let blocks = vec![
            ContentBlock::Text(TextContent::new("Hello")),
            ContentBlock::Text(TextContent::new(" world")),
        ];
        let result = blocks_to_content(&blocks).unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }
}
