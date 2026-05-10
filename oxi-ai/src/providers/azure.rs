//! Azure OpenAI provider implementation

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

/// Azure OpenAI provider
///
/// Uses Azure-specific endpoint format:
/// https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions
///
/// Supports the following environment variables:
/// - AZURE_OPENAI_API_KEY: API key for authentication
/// Azure OpenAI provider
///
/// Configuration is resolved at runtime from:
/// - auth.json (for api_key)
/// - settings.toml (for resource_name, deployment_name via custom_provider)
/// - StreamOptions (for request-time override)
#[derive(Clone)]
pub struct AzureProvider {
    client: &'static Client,
    api_key: Option<String>,
    resource_name: Option<String>,
    deployment_name: Option<String>,
}

impl AzureProvider {
    /// Create a new Azure provider without configuration.
    ///
    /// Configuration is resolved at request time via auth.json or StreamOptions.
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_key: None,
            resource_name: None,
            deployment_name: None,
        }
    }

    /// Create with explicit configuration (public API for external consumers)
    #[allow(dead_code)]
    pub fn with_config(
        api_key: impl Into<String>,
        resource_name: impl Into<String>,
        deployment_name: impl Into<String>,
    ) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
            resource_name: Some(resource_name.into()),
            deployment_name: Some(deployment_name.into()),
        }
    }

    /// Build the Azure endpoint URL
    fn build_url(&self, model: &Model) -> Result<String, ProviderError> {
        // Priority: model.base_url > resource_name env > fallback
        if !model.base_url.is_empty() && model.base_url != "https://api.openai.com" {
            // Use the provided base URL directly (already includes deployment)
            return Ok(format!(
                "{}/chat/completions?api-version=2024-02-15-preview",
                model.base_url.trim_end_matches('/')
            ));
        }

        // Fallback to constructing from environment variables
        let resource = self.resource_name.as_ref().ok_or_else(|| {
            ProviderError::InvalidResponse("AZURE_OPENAI_RESOURCE_NAME not set".into())
        })?;

        let deployment = self.deployment_name.as_ref().ok_or_else(|| {
            ProviderError::InvalidResponse("AZURE_OPENAI_DEPLOYMENT_NAME not set".into())
        })?;

        let url = format!(
            "https://{}.openai.azure.com/openai/deployments/{}/chat/completions?api-version=2024-02-15-preview",
            resource, deployment
        );

        Ok(url)
    }

    /// Get the API key (from options or self)
    fn get_api_key(&self, options: &Option<StreamOptions>) -> Result<String, ProviderError> {
        options
            .as_ref()
            .and_then(|o| o.api_key.as_ref())
            .or(self.api_key.as_ref())
            .cloned()
            .ok_or_else(|| ProviderError::MissingApiKey)
    }

    /// Build request headers with Azure-specific api-key authentication
    fn build_headers(
        &self,
        api_key: &str,
        options: &Option<StreamOptions>,
    ) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();

        // Azure uses api-key header instead of Bearer token
        headers.insert("api-key", api_key.parse().expect("valid header value"));
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().expect("valid header value"),
        );

        // Add custom headers from options
        if let Some(opts) = options {
            for (k, v) in &opts.headers {
                if let (Ok(name), Ok(value)) = (
                    k.parse::<reqwest::header::HeaderName>(),
                    v.parse::<reqwest::header::HeaderValue>(),
                ) {
                    headers.insert(name, value);
                }
            }
        }

        headers
    }
}

impl Default for AzureProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AzureProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        // Build URL
        let url = self.build_url(model)?;

        // Get API key
        let api_key = self.get_api_key(&options)?;

        // Build messages
        let messages = build_messages(context)?;

        // Build request body
        let mut body = serde_json::json!({
            "messages": messages,
            "stream": true,
        });

        // Add model if not already in URL (some deployments use it)
        if model.id != "default" && model.id != "azure" {
            body["model"] = serde_json::json!(model.id);
        }

        // Add optional parameters
        if let Some(ref opts) = options {
            if let Some(temp) = opts.temperature {
                body["temperature"] = serde_json::json!(temp);
            }

            if let Some(max) = opts.max_tokens {
                body["max_tokens"] = serde_json::json!(max);
            }
        }

        // Add tools if present
        if !context.tools.is_empty() {
            body["tools"] = build_tools(&context.tools)?;
        }

        // Build headers
        let headers = self.build_headers(&api_key, &options);

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
        "azure"
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
///
/// This is identical to the OpenAI provider's SSE parsing logic.
fn parse_sse_events(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let partial_message = AssistantMessage::new(Api::OpenAiCompletions, provider, model_id);

    // Pre-estimate capacity: one event per data line is a reasonable upper bound.
    let estimated_events = text.split('\n').filter(|l| l.starts_with("data: ")).count();
    events.reserve(estimated_events);

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

        // Get this chunk's usage for setting on Done events
        let this_chunk_usage = chunk.usage.as_ref();

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
                // For Done events: prefer current chunk's usage if available,
                // otherwise fall back to accumulated usage
                let reason = match choice.finish_reason.as_deref() {
                    Some("stop") => StopReason::Stop,
                    Some("length") => StopReason::Length,
                    Some("tool_calls") => StopReason::ToolUse,
                    _ => StopReason::Stop,
                };

                let mut done_msg = partial_message.clone();

                // Use current chunk's usage if present, otherwise accumulated
                if let Some(usage) = this_chunk_usage {
                    done_msg.usage.input = usage.prompt_tokens;
                    done_msg.usage.output = usage.completion_tokens;
                    done_msg.usage.cache_read = usage
                        .prompt_tokens_details
                        .as_ref()
                        .map(|d| d.cached_tokens)
                        .unwrap_or(0);
                    done_msg.usage.total_tokens = usage.total_tokens;
                } else {
                    done_msg.usage = accumulated_usage.clone();
                }

                events.push(ProviderEvent::Done {
                    reason,
                    message: done_msg,
                });
            }
        }

        // Update accumulated usage for next chunks
        if let Some(usage) = this_chunk_usage {
            accumulated_usage.input = usage.prompt_tokens;
            accumulated_usage.output = usage.completion_tokens;
            accumulated_usage.cache_read = usage
                .prompt_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .unwrap_or(0);
            accumulated_usage.total_tokens = usage.total_tokens;
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

// SSE chunk structure (same as OpenAI)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_model(id: &str, base_url: &str) -> Model {
        Model::new(id, id, Api::OpenAiCompletions, "azure", base_url)
    }

    #[test]
    fn test_provider_name() {
        let provider = AzureProvider::new();
        assert_eq!(provider.name(), "azure");
    }

    #[test]
    fn test_build_url_from_base_url() {
        let provider = AzureProvider::new();
        let model = make_test_model(
            "gpt-4o",
            "https://my-resource.openai.azure.com/openai/deployments/gpt-4o",
        );

        let url = provider.build_url(&model).unwrap();
        assert!(url.contains("api-version=2024-02-15-preview"));
        assert!(url.contains("my-resource"));
        assert!(url.contains("gpt-4o"));
    }

    #[test]
    fn test_build_url_missing_resource() {
        let provider = AzureProvider {
            client: shared_client(),
            api_key: Some("test-key".to_string()),
            resource_name: None,
            deployment_name: Some("gpt-4o".to_string()),
        };

        let model = make_test_model("default", "");

        let result = provider.build_url(&model);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::InvalidResponse(msg) => {
                assert!(msg.contains("AZURE_OPENAI_RESOURCE_NAME"));
            }
            _ => panic!("Expected InvalidResponse"),
        }
    }

    #[test]
    fn test_build_url_missing_deployment() {
        let provider = AzureProvider {
            client: shared_client(),
            api_key: Some("test-key".to_string()),
            resource_name: Some("my-resource".to_string()),
            deployment_name: None,
        };

        let model = make_test_model("default", "");

        let result = provider.build_url(&model);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::InvalidResponse(msg) => {
                assert!(msg.contains("AZURE_OPENAI_DEPLOYMENT_NAME"));
            }
            _ => panic!("Expected InvalidResponse"),
        }
    }

    #[test]
    fn test_build_url_from_env_vars() {
        let provider = AzureProvider {
            client: shared_client(),
            api_key: Some("test-key".to_string()),
            resource_name: Some("my-resource".to_string()),
            deployment_name: Some("gpt-4o".to_string()),
        };

        let model = make_test_model("default", "");

        let url = provider.build_url(&model).unwrap();
        assert_eq!(url, "https://my-resource.openai.azure.com/openai/deployments/gpt-4o/chat/completions?api-version=2024-02-15-preview");
    }

    #[test]
    fn test_parse_sse_events_text() {
        let sse_data = r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":"stop"}]}

data: [DONE]"#;

        let events = parse_sse_events(sse_data, "azure", "gpt-4o");

        // Should have text delta events and a done event
        assert!(events.len() >= 3);

        // Check first text delta
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hello"),
            _ => panic!("Expected TextDelta event"),
        }

        // Check done event
        match &events[events.len() - 1] {
            ProviderEvent::Done { reason, .. } => assert_eq!(*reason, StopReason::Stop),
            _ => panic!("Expected Done event"),
        }
    }

    #[test]
    fn test_parse_sse_events_with_tool_calls() {
        let sse_data = r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_abc123","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"location\":"}}]},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Boston\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]"#;

        let events = parse_sse_events(sse_data, "azure", "gpt-4o");

        // Should have tool call delta events and a done event
        assert!(events.len() >= 4);

        // Check for tool call delta
        let has_tool_call = events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ToolCallDelta { .. }));
        assert!(
            has_tool_call,
            "Should have at least one ToolCallDelta event"
        );

        // Check done event
        match &events[events.len() - 1] {
            ProviderEvent::Done { reason, .. } => assert_eq!(*reason, StopReason::ToolUse),
            _ => panic!("Expected Done event with ToolUse reason"),
        }
    }

    #[test]
    fn test_parse_sse_events_usage() {
        let sse_data = r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"prompt_tokens_details":{"cached_tokens":0}}}

data: [DONE]"#;

        let events = parse_sse_events(sse_data, "azure", "gpt-4o");

        // Find the done event and check usage
        let done_event = events
            .iter()
            .find(|e| matches!(e, ProviderEvent::Done { .. }));
        assert!(done_event.is_some());

        if let ProviderEvent::Done { message, .. } = done_event.unwrap() {
            assert_eq!(message.usage.input, 10);
            assert_eq!(message.usage.output, 5);
            assert_eq!(message.usage.total_tokens, 15);
        }
    }

    #[test]
    fn test_build_headers_includes_api_key() {
        let provider = AzureProvider::new();
        let api_key = "test-api-key-12345";

        let headers = provider.build_headers(api_key, &None);

        // Check api-key header is present
        let api_key_header = headers.get("api-key");
        assert!(api_key_header.is_some());
        assert_eq!(api_key_header.unwrap().to_str().unwrap(), api_key);

        // Check content-type is present
        let content_type = headers.get(reqwest::header::CONTENT_TYPE);
        assert!(content_type.is_some());
    }

    #[test]
    fn test_build_headers_no_bearer_token() {
        let provider = AzureProvider::new();
        let api_key = "test-api-key-12345";

        let headers = provider.build_headers(api_key, &None);

        // Azure should NOT use Authorization header with Bearer token
        let auth_header = headers.get(reqwest::header::AUTHORIZATION);
        assert!(
            auth_header.is_none(),
            "Azure should not use Bearer token authentication"
        );
    }

    #[test]
    fn test_with_config_constructor() {
        let provider = AzureProvider::with_config("my-api-key", "my-resource", "gpt-4o");

        // Verify the internal state through build_url
        let model = make_test_model("default", "");

        let url = provider.build_url(&model).unwrap();
        assert!(url.contains("my-resource"));
        assert!(url.contains("gpt-4o"));
    }

    #[test]
    fn test_azure_endpoint_format() {
        let provider = AzureProvider {
            client: shared_client(),
            api_key: Some("key".to_string()),
            resource_name: Some("my-resource".to_string()),
            deployment_name: Some("gpt-4-turbo".to_string()),
        };

        let model = make_test_model("default", "");
        let url = provider.build_url(&model).unwrap();

        // Verify the complete Azure endpoint format
        assert!(url.starts_with("https://"));
        assert!(url.contains(".openai.azure.com"));
        assert!(url.contains("/openai/deployments/"));
        assert!(url.contains("gpt-4-turbo"));
        assert!(url.contains("chat/completions"));
        assert!(url.contains("api-version=2024-02-15-preview"));
    }
}
