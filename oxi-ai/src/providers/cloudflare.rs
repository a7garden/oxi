//! Cloudflare Workers AI provider implementation

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;

use crate::{
    error::ProviderError, Api, AssistantMessage, ContentBlock, Context, Model, Provider,
    ProviderEvent, StopReason, StreamOptions, Usage,
};

use super::shared_client;

/// Cloudflare Workers AI provider
#[derive(Clone)]
#[allow(dead_code)] // account_id used for gateway URLs
pub struct CloudflareProvider {
    client: &'static Client,
    api_token: Option<String>,
    account_id: Option<String>,
}

impl CloudflareProvider {
    /// Create a new Cloudflare provider using environment variables
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_token: std::env::var("CLOUDFLARE_API_TOKEN").ok(),
            account_id: std::env::var("CLOUDFLARE_ACCOUNT_ID").ok(),
        }
    }

    /// Create with explicit credentials (public API for external consumers)
    #[allow(dead_code)]
    pub fn with_credentials(api_token: impl Into<String>, account_id: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_token: Some(api_token.into()),
            account_id: Some(account_id.into()),
        }
    }

    /// Create a model configuration for Cloudflare Workers AI (public API)
    #[allow(dead_code)]
    pub fn model<S: Into<String>>(&self, model_id: S) -> Model {
        let id = model_id.into();
        let base_url = if self.account_id.is_some() {
            format!(
                "https://api.cloudflare.com/client/v4/accounts/{}/workers/ai/v1",
                self.account_id.as_ref().unwrap()
            )
        } else {
            "https://api.cloudflare.com/client/v4/workers/ai/v1".to_string()
        };

        Model::new(&id, &id, Api::OpenAiCompletions, "cloudflare", &base_url)
    }

    /// Create a model with AI Gateway (public API)
    #[allow(dead_code)]
    pub fn model_with_gateway<S: Into<String>>(
        &self,
        model_id: S,
        gateway_id: &str,
    ) -> Option<Model> {
        let account_id = self.account_id.as_ref()?;
        if account_id.is_empty() {
            return None;
        }
        let id = model_id.into();
        let base_url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/ai/gateways/{}/v1",
            account_id, gateway_id
        );

        Some(Model::new(
            &id,
            &id,
            Api::OpenAiCompletions,
            "cloudflare",
            &base_url,
        ))
    }
}

impl Default for CloudflareProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CloudflareProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Build the request URL
        let url = format!("{}/chat/completions", model.base_url);

        // Get API token
        let api_token = options
            .api_key
            .as_ref()
            .or(self.api_token.as_ref())
            .ok_or_else(|| ProviderError::MissingApiKey)?;

        // Build messages
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
            format!("Bearer {}", api_token).parse().unwrap(),
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
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
        "cloudflare"
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
        }

        // Update accumulated usage (before checking finish to ensure usage is available)
        if let Some(chunk_usage) = chunk.usage {
            accumulated_usage.input = chunk_usage.prompt_tokens;
            accumulated_usage.output = chunk_usage.completion_tokens;
            accumulated_usage.cache_read = chunk_usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens)
                .unwrap_or(0);
            accumulated_usage.total_tokens = chunk_usage.total_tokens;
        }

        // Check for finish after updating usage
        for choice in &chunk.choices {
            if choice.finish_reason.is_some() {
                let reason = match choice.finish_reason.as_deref() {
                    Some("stop") => StopReason::Stop,
                    Some("length") => StopReason::Length,
                    Some("tool_calls") => StopReason::ToolUse,
                    Some("end_turn") => StopReason::Stop,
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
    cached_tokens: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test 1: Provider creation with env vars
    #[test]
    fn test_provider_new() {
        let provider = CloudflareProvider::new();
        assert_eq!(provider.name(), "cloudflare");
        // Without env vars, api_token and account_id are None
        assert!(provider.api_token.is_none());
        assert!(provider.account_id.is_none());
    }

    // Test 2: Provider creation with explicit credentials
    #[test]
    fn test_provider_with_credentials() {
        let provider = CloudflareProvider::with_credentials("test-api-token", "test-account-id");
        assert_eq!(provider.api_token.as_deref(), Some("test-api-token"));
        assert_eq!(provider.account_id.as_deref(), Some("test-account-id"));
    }

    // Test 3: Model creation without account ID (direct Workers AI)
    #[test]
    fn test_model_direct_workers_ai() {
        let provider = CloudflareProvider::with_credentials("test-api-token", "");
        let model = provider.model("@cf/meta/llama-3.1-8b-instruct");
        assert_eq!(model.provider, "cloudflare");
        assert_eq!(model.id, "@cf/meta/llama-3.1-8b-instruct");
        assert!(model.base_url.contains("workers/ai/v1"));
    }

    // Test 4: Model creation with account ID
    #[test]
    fn test_model_with_account_id() {
        let provider = CloudflareProvider::with_credentials("test-api-token", "abc123account");
        let model = provider.model("@cf/meta/llama-3.1-8b-instruct");
        assert!(model.base_url.contains("accounts/abc123account"));
    }

    // Test 5: Model creation with AI Gateway
    #[test]
    fn test_model_with_gateway() {
        let provider = CloudflareProvider::with_credentials("test-api-token", "abc123account");
        let model = provider.model_with_gateway("@cf/meta/llama-3.1-8b-instruct", "my-gateway");
        assert!(model.is_some());
        let model = model.unwrap();
        assert!(model.base_url.contains("ai/gateways/my-gateway/v1"));
    }

    // Test 6: Model creation with gateway returns None when no account ID
    #[test]
    fn test_model_with_gateway_no_account_id() {
        let provider = CloudflareProvider::with_credentials("test-api-token", "");
        let model = provider.model_with_gateway("@cf/meta/llama-3.1-8b-instruct", "my-gateway");
        assert!(model.is_none());
    }

    // Test 7: Build messages from context
    #[test]
    fn test_build_messages() {
        use crate::Message;

        let mut context = Context::new();
        context.set_system_prompt("You are a helpful assistant");
        context.add_message(Message::user("Hello"));

        let messages = build_messages(&context).unwrap();
        assert_eq!(messages.len(), 2); // system + user
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello");
    }

    // Test 8: Build messages with tool result
    #[test]
    fn test_build_messages_with_tool_result() {
        let context = Context::new();
        let messages = build_messages(&context).unwrap();
        // Empty context should produce empty messages (no system prompt, no messages)
        assert!(messages.is_empty());
    }

    // Test 9: Build tools array
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

        let tools_json = build_tools(&tools).unwrap();
        assert_eq!(tools_json[0]["type"], "function");
        assert_eq!(tools_json[0]["function"]["name"], "get_weather");
        assert_eq!(
            tools_json[0]["function"]["description"],
            "Get weather for a location"
        );
    }

    // Test 10: Default implementation
    #[test]
    fn test_default_implementation() {
        let provider = CloudflareProvider::default();
        assert_eq!(provider.name(), "cloudflare");
    }

    // Test 11: SSE parsing with text delta
    #[test]
    fn test_parse_sse_text_delta() {
        let data = r#"data: {"id":"1","model":"test","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let events = parse_sse_events(data, "cloudflare", "test-model");

        assert!(!events.is_empty());
        // Check that we get a TextDelta event
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::TextDelta { .. })));
    }

    // Test 12: SSE parsing with done event
    #[test]
    fn test_parse_sse_done_event() {
        let data = r#"data: {"id":"1","model":"test","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let events = parse_sse_events(data, "cloudflare", "test-model");

        let done_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::Done { .. }))
            .collect();
        assert!(!done_events.is_empty());

        if let ProviderEvent::Done { reason, message } = &done_events[0] {
            assert_eq!(*reason, StopReason::Stop);
            assert_eq!(message.usage.total_tokens, 15);
        }
    }

    // Test 13: SSE parsing with tool call delta
    #[test]
    fn test_parse_sse_tool_call_delta() {
        let data = r#"data: {"id":"1","model":"test","choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_1","function":{"name":"get_weather","arguments":"{\"location\":\"NYC\"}"}}]},"finish_reason":null}]}"#;
        let events = parse_sse_events(data, "cloudflare", "test-model");

        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ToolCallDelta { .. })));
    }

    // Test 14: SSE parsing with [DONE] sentinel
    #[test]
    fn test_parse_sse_done_sentinel() {
        let data = r#"data: {"id":"1","model":"test","choices":[{"index":0,"delta":{"content":"Done"},"finish_reason":"stop"}]}
data: [DONE]"#;
        let events = parse_sse_events(data, "cloudflare", "test-model");

        // Should parse the first chunk but stop at [DONE]
        assert!(!events.is_empty());
    }

    // Test 15: SSE parsing handles multiple chunks
    #[test]
    fn test_parse_sse_multiple_chunks() {
        let data = r#"data: {"id":"1","model":"test","choices":[{"index":0,"delta":{"content":"Hello "},"finish_reason":null}]}
data: {"id":"2","model":"test","choices":[{"index":0,"delta":{"content":"world"},"finish_reason":"stop"}]}"#;
        let events = parse_sse_events(data, "cloudflare", "test-model");

        // Should have text delta for "Hello " and another for "world"
        let text_deltas: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::TextDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert!(text_deltas.contains(&"Hello ".to_string()));
        assert!(text_deltas.contains(&"world".to_string()));
    }

    // Test 16: Block content with multiple blocks
    #[test]
    fn test_blocks_to_content_multiple() {
        use crate::TextContent;

        let blocks = vec![
            ContentBlock::Text(TextContent::new("Hello")),
            ContentBlock::Text(TextContent::new(" world")),
        ];

        let content = blocks_to_content(&blocks).unwrap();
        // When multiple blocks, it becomes an array
        assert!(content.is_array());
        assert_eq!(content.as_array().unwrap().len(), 2);
    }

    // Test 17: Block content with single text
    #[test]
    fn test_blocks_to_content_single_text() {
        use crate::TextContent;

        let blocks = vec![ContentBlock::Text(TextContent::new("Just text"))];

        let content = blocks_to_content(&blocks).unwrap();
        // Single text becomes a string
        assert!(content.is_string());
        assert_eq!(content.as_str().unwrap(), "Just text");
    }

    // Test 18: Clone provider
    #[test]
    fn test_provider_clone() {
        let provider = CloudflareProvider::with_credentials("token", "account");
        let cloned = provider.clone();
        assert_eq!(cloned.api_token, provider.api_token);
        assert_eq!(cloned.account_id, provider.account_id);
    }
}
