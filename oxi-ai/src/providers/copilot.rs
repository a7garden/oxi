//! GitHub Copilot provider implementation
//!
//! Supports GitHub Copilot authentication via GITHUB_TOKEN or GITHUB_COPILOT_TOKEN env vars.
//! Uses the Copilot API endpoint: https://api.githubcopilot.com/chat/completions

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;

use crate::{
    Api, AssistantMessage, ContentBlock, Context, Model, Provider, ProviderEvent, StopReason,
    StreamOptions, Usage,
};

use super::shared_client;
use super::ProviderError;

/// GitHub Copilot provider
///
/// Supports authentication via:
/// - GITHUB_TOKEN environment variable
/// - GITHUB_COPILOT_TOKEN environment variable
/// - Explicit API key via StreamOptions
#[derive(Clone)]
pub struct CopilotProvider {
    client: &'static Client,
    api_key: Option<String>,
}

impl CopilotProvider {
    /// Create a new Copilot provider
    pub fn new() -> Self {
        // Check for GITHUB_COPILOT_TOKEN first, then GITHUB_TOKEN
        let api_key = std::env::var("GITHUB_COPILOT_TOKEN")
            .ok()
            .or_else(|| std::env::var("GITHUB_TOKEN").ok());

        Self {
            client: shared_client(),
            api_key,
        }
    }

    /// Create a provider with an explicit API key (public API for external consumers)
    #[allow(dead_code)]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
        }
    }

    /// Get the default Copilot API endpoint
    fn default_endpoint() -> &'static str {
        "https://api.githubcopilot.com/chat/completions"
    }
}

impl Default for CopilotProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CopilotProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Build the request URL
        let url = if model.base_url.is_empty() {
            Self::default_endpoint().to_string()
        } else {
            format!("{}/chat/completions", model.base_url.trim_end_matches('/'))
        };

        // Get API key - priority: options > self > env vars
        let api_key = options
            .api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .ok_or(ProviderError::MissingApiKey)?;

        // Build messages
        let messages = build_messages(context)?;

        // Build request body (OpenAI-compatible)
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

        // Add tools if present (OpenAI function calling format)
        if !context.tools.is_empty() {
            body["tools"] = build_tools(&context.tools)?;
        }

        // Build headers
        let mut headers = reqwest::header::HeaderMap::new();

        // Authorization: Bearer token
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", api_key).parse().unwrap(),
        );

        // GitHub Copilot specific: x-github-token header
        headers.insert(
            reqwest::header::HeaderName::from_static("x-github-token"),
            api_key.parse().unwrap(),
        );

        // Content type
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );

        // Copilot-specific headers for context
        headers.insert(
            reqwest::header::HeaderName::from_static("x-github-api-version"),
            "2024-11-20".parse().unwrap(),
        );

        // Add custom headers from options
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
        "copilot"
    }
}

// ============================================================================
// Message building
// ============================================================================

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

/// Build tools array (OpenAI function calling format)
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

// ============================================================================
// SSE parsing
// ============================================================================

/// Parse SSE event stream from a byte buffer.
fn parse_sse_events(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let partial_message = AssistantMessage::new(Api::OpenAiCompletions, provider, model_id);

    // Pre-estimate capacity
    let estimated_events = text.split('\n').filter(|l| l.starts_with("data: ")).count();
    events.reserve(estimated_events);

    let mut accumulated_usage = Usage::default();

    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        // Skip non-data lines
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
                // Handle text content
                if let Some(content) = &delta.content {
                    events.push(ProviderEvent::TextDelta {
                        content_index: choice.index,
                        delta: content.clone(),
                        partial: partial_message.clone(),
                    });
                }

                // Handle tool calls
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
        }

        // Accumulate usage from the chunk BEFORE checking completion
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

        // Check for completion (after usage accumulation so usage is included in Done)
        for choice in &chunk.choices {
            if choice.finish_reason.is_some() {
                let reason = match choice.finish_reason.as_deref() {
                    Some("stop") => StopReason::Stop,
                    Some("length") => StopReason::Length,
                    Some("tool_calls") => StopReason::ToolUse,
                    Some("function_call") => StopReason::ToolUse,
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

// SSE chunk structures
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name() {
        let provider = CopilotProvider::new();
        assert_eq!(provider.name(), "copilot");
    }

    #[test]
    fn test_default_endpoint() {
        assert_eq!(
            CopilotProvider::default_endpoint(),
            "https://api.githubcopilot.com/chat/completions"
        );
    }

    #[test]
    fn test_build_tools() {
        let tools = vec![crate::Tool {
            name: "get_weather".to_string(),
            description: "Get weather for a location".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {"type": "string"}
                },
                "required": ["location"]
            }),
        }];

        let result = build_tools(&tools).unwrap();
        let tools_array = result.as_array().unwrap();
        assert_eq!(tools_array.len(), 1);
        assert_eq!(tools_array[0]["type"], "function");
        assert_eq!(tools_array[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn test_build_tools_empty() {
        let tools: Vec<crate::Tool> = vec![];
        let result = build_tools(&tools).unwrap();
        let tools_array = result.as_array().unwrap();
        assert!(tools_array.is_empty());
    }

    #[test]
    fn test_parse_sse_basic() {
        let data = r#"data: {"id":"chatcmpl-123","model":"gpt-4","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let events = parse_sse_events(data, "copilot", "gpt-4");

        assert!(!events.is_empty());
        // Should have TextDelta event
        let has_text_delta = events
            .iter()
            .any(|e| matches!(e, ProviderEvent::TextDelta { .. }));
        assert!(has_text_delta);
    }

    #[test]
    fn test_parse_sse_done() {
        let data = r#"data: {"id":"chatcmpl-123","model":"gpt-4","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"prompt_tokens_details":{"cached_tokens":0}}}"#;
        let events = parse_sse_events(data, "copilot", "gpt-4");

        // Should have both TextDelta and Done
        let has_done = events
            .iter()
            .any(|e| matches!(e, ProviderEvent::Done { .. }));
        assert!(has_done);
    }

    #[test]
    fn test_parse_sse_multiple_chunks() {
        let data = r#"data: {"id":"1","model":"m","choices":[{"index":0,"delta":{"content":"Hel"},"finish_reason":null}]}
data: {"id":"2","model":"m","choices":[{"index":0,"delta":{"content":"lo"},"finish_reason":"stop"}]}"#;
        let events = parse_sse_events(data, "copilot", "gpt-4");
        assert!(events.len() >= 2);
    }

    #[test]
    fn test_parse_sse_tool_call() {
        let data = r#"data: {"id":"chatcmpl-123","model":"gpt-4","choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_123","type":"function","function":{"name":"get_weather","arguments":"{\"loc"}}]},"finish_reason":"tool_calls"}]}"#;
        let events = parse_sse_events(data, "copilot", "gpt-4");

        let has_tool_delta = events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ToolCallDelta { .. }));
        assert!(has_tool_delta);
    }

    #[test]
    fn test_parse_sse_empty_content() {
        let data = "data: {\"id\":\"123\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}";
        let events = parse_sse_events(data, "copilot", "gpt-4");
        // Should still have a Done event
        let has_done = events
            .iter()
            .any(|e| matches!(e, ProviderEvent::Done { .. }));
        assert!(has_done);
    }

    #[test]
    fn test_parse_sse_invalid_json() {
        let data = "data: not valid json";
        let events = parse_sse_events(data, "copilot", "gpt-4");
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_sse_done_marker() {
        let data = "data: [DONE]";
        let events = parse_sse_events(data, "copilot", "gpt-4");
        assert!(events.is_empty());
    }

    #[test]
    fn test_build_messages_with_system() {
        let context = Context::new().with_system_prompt("You are a helpful assistant");

        let messages = build_messages(&context).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a helpful assistant");
    }

    #[test]
    fn test_build_messages_empty() {
        let context = Context::new();
        let messages = build_messages(&context).unwrap();
        assert!(messages.is_empty());
    }

    #[test]
    fn test_usage_accumulation() {
        let data = r#"data: {"id":"chatcmpl-123","model":"gpt-4","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"prompt_tokens_details":{"cached_tokens":3}}}"#;
        let events = parse_sse_events(data, "copilot", "gpt-4");

        if let Some(ProviderEvent::Done { message, .. }) = events.last() {
            assert_eq!(message.usage.input, 10);
            assert_eq!(message.usage.output, 5);
            assert_eq!(message.usage.cache_read, 3);
        }
    }

    #[test]
    fn test_with_api_key() {
        let provider = CopilotProvider::with_api_key("test-token");
        assert_eq!(provider.name(), "copilot");
        // API key is stored but not directly accessible; we verify via behavior
    }
}
