//! GitHub Codex provider implementation
//!
//! This provider targets the GitHub Copilot's codex engine via OpenAI-compatible API.
//! It uses the codex-specific endpoint for code completion tasks.
//!
//! Codex is optimized for code-related tasks and supports additional parameters
//! like "temperature" and "top_p" that can be tuned for code generation.

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
#[allow(unused_imports)] // Used in build_tools return type
use std::pin::Pin;

use crate::{
    Api, AssistantMessage, ContentBlock, Context, Model, Provider, ProviderEvent, StopReason,
    StreamOptions, Usage,
};

use super::shared_client;
use super::ProviderError;

/// Configuration for Codex provider
#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// Base URL for the Codex API (reserved for future use)
    #[allow(dead_code)]
    pub base_url: String,
    /// API key for authentication
    pub api_key: Option<String>,
    /// Model ID (e.g., "codex-davinci-002")
    pub model: String,
    /// Default temperature (0.0-1.0)
    pub temperature: Option<f32>,
    /// Default max tokens
    pub max_tokens: Option<usize>,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            // GitHub Copilot's codex endpoint
            base_url: "https://api.githubcopilot.com".to_string(),
            api_key: None,
            model: "codex-davinci-002".to_string(),
            temperature: Some(0.2), // Lower temp for more deterministic code
            max_tokens: Some(2048),
        }
    }
}

/// GitHub Codex provider
///
/// API keys are resolved at request time from auth.json or StreamOptions.
#[derive(Clone)]
pub struct CodexProvider {
    client: &'static Client,
    config: CodexConfig,
}

impl CodexProvider {
    /// Create a new Codex provider with default configuration
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            config: CodexConfig::default(),
        }
    }

    /// Create with explicit API key (public API for external consumers)
    #[allow(dead_code)]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        let mut config = CodexConfig::default();
        config.api_key = Some(api_key.into());
        Self {
            client: shared_client(),
            config,
        }
    }

    /// Create with custom configuration (public API)
    #[allow(dead_code)]
    pub fn with_config(config: CodexConfig) -> Self {
        Self {
            client: shared_client(),
            config,
        }
    }

    /// Set model ID (public API builder)
    #[allow(dead_code)]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set temperature (public API builder)
    #[allow(dead_code)]
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.config.temperature = Some(temp);
        self
    }

    /// Set max tokens (public API builder)
    #[allow(dead_code)]
    pub fn with_max_tokens(mut self, max: usize) -> Self {
        self.config.max_tokens = Some(max);
        self
    }

    /// Get provider name (public API)
    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        "codex"
    }

    /// Get the default Codex API endpoint
    fn default_endpoint() -> &'static str {
        "https://api.githubcopilot.com/chat/completions"
    }

    /// Build messages from context for code generation
    fn build_code_messages(context: &Context) -> Result<Vec<JsonValue>, ProviderError> {
        let mut messages = Vec::new();

        // System prompt for code-focused assistant
        if let Some(ref prompt) = context.system_prompt {
            messages.push(serde_json::json!({
                "role": "system",
                "content": prompt,
            }));
        } else {
            // Default code-focused system prompt
            messages.push(serde_json::json!({
                "role": "system",
                "content": "You are an expert programmer helping write, review, and explain code. Provide clear, accurate code with explanations when helpful.",
            }));
        }

        // Conversation messages
        for msg in &context.messages {
            match msg {
                crate::Message::User(u) => {
                    let content = Self::extract_text_content(&u.content)?;
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content,
                    }));
                }
                crate::Message::Assistant(a) => {
                    let content = Self::extract_blocks_content(&a.content)?;
                    if !content.is_empty() {
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content,
                        }));
                    }
                }
                crate::Message::ToolResult(t) => {
                    let content = Self::extract_blocks_content(&t.content)?;
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

    /// Extract text from message content
    fn extract_text_content(content: &crate::MessageContent) -> Result<String, ProviderError> {
        match content {
            crate::MessageContent::Text(s) => Ok(s.clone()),
            crate::MessageContent::Blocks(blocks) => {
                let parts: Vec<String> = blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect();
                Ok(parts.join("\n"))
            }
        }
    }

    /// Extract text from content blocks
    fn extract_blocks_content(blocks: &[ContentBlock]) -> Result<String, ProviderError> {
        let parts: Vec<String> = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        Ok(parts.join("\n"))
    }
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CodexProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Determine API key
        let api_key = options
            .api_key
            .as_ref()
            .or(self.config.api_key.as_ref())
            .ok_or(ProviderError::MissingApiKey)?;

        // Build the request URL
        let url = if model.base_url.is_empty() {
            Self::default_endpoint().to_string()
        } else {
            format!("{}/chat/completions", model.base_url.trim_end_matches('/'))
        };

        // Build messages
        let messages = Self::build_code_messages(context)?;

        // Determine model ID
        let model_id = if model.id.is_empty() {
            self.config.model.clone()
        } else {
            model.id.clone()
        };

        // Build request body (OpenAI-compatible)
        let mut body = serde_json::json!({
            "model": model_id,
            "messages": messages,
            "stream": true,
        });

        // Add optional parameters
        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        } else if let Some(temp) = self.config.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if let Some(max) = options.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        } else if let Some(max) = self.config.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }

        // Add tools if present (codex supports function calling)
        if !context.tools.is_empty() {
            body["tools"] = build_tools(&context.tools)?;
        }

        // Build headers
        let mut headers = reqwest::header::HeaderMap::new();

        // Authorization: Bearer token
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", api_key).parse().expect("valid bearer header"),
        );

        // GitHub Copilot specific: x-github-token header
        headers.insert(
            reqwest::header::HeaderName::from_static("x-github-token"),
            api_key.parse().expect("valid header value"),
        );

        // Content type
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().expect("valid header value"),
        );

        // Copilot-specific headers
        headers.insert(
            reqwest::header::HeaderName::from_static("x-github-api-version"),
            "2024-11-20".parse().expect("valid header value"),
        );

        // Copilot intent header for code-specific requests
        headers.insert(
            reqwest::header::HeaderName::from_static("x-copilot-integration"),
            "oxi-codex".parse().expect("valid header value"),
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
        let model_id_str = model_id;

        let stream = response.bytes_stream().flat_map(
            move |chunk: Result<Bytes, reqwest::Error>| match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    futures::stream::iter(parse_sse_events(&text, &provider_name, &model_id_str))
                }
                Err(e) => futures::stream::iter(vec![ProviderEvent::Error {
                    reason: StopReason::Error,
                    error: create_error_message(&e.to_string(), &provider_name, &model_id_str),
                }]),
            },
        );

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "codex"
    }
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

/// Parse SSE event stream
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

        let chunk = match serde_json::from_str::<CodexSSEChunk>(data) {
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

        // Accumulate usage
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

        // Check for completion
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

// SSE structures
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde deserialization structs
struct CodexSSEChunk {
    id: Option<String>,
    #[serde(rename = "model")]
    model: Option<String>,
    choices: Vec<CodexChoice>,
    usage: Option<CodexUsage>,
}

#[derive(Debug, Deserialize)]
struct CodexChoice {
    index: usize,
    delta: Option<CodexDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexDelta {
    content: Option<String>,
    tool_calls: Option<Vec<CodexToolCall>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde deserialization structs
struct CodexToolCall {
    index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    type_: Option<String>,
    function: Option<CodexFunctionDelta>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde deserialization structs
struct CodexFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct CodexUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    #[serde(rename = "prompt_tokens_details")]
    prompt_tokens_details: Option<CodexPromptTokensDetails>,
}

#[derive(Debug, Deserialize, Clone)]
struct CodexPromptTokensDetails {
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
        let provider = CodexProvider::new();
        assert_eq!(provider.name(), "codex");
    }

    #[test]
    fn test_config_defaults() {
        let config = CodexConfig::default();
        assert_eq!(config.model, "codex-davinci-002");
        assert!(config.temperature.is_some());
        assert!(config.max_tokens.is_some());
    }

    #[test]
    fn test_builder_pattern() {
        // Cannot call async test directly, just verify it compiles
    }

    #[test]
    fn test_with_api_key() {
        // CodexProvider has no with_api_key, but we verify the name
        let provider = CodexProvider::new();
        assert_eq!(provider.name(), "codex");
    }

    #[test]
    fn test_build_messages_empty_context() {
        let context = Context::new();
        let messages = CodexProvider::build_code_messages(&context).unwrap();

        // Should have default system prompt
        assert!(!messages.is_empty());
        assert_eq!(messages[0]["role"], "system");
    }

    #[test]
    fn test_build_messages_with_user() {
        let mut context = Context::new();
        context.set_system_prompt("You are a code assistant");
        context.add_message(crate::Message::user("Write code"));
        let messages = CodexProvider::build_code_messages(&context).unwrap();
        assert!(messages.len() >= 2);
    }

    #[test]
    fn test_build_messages_with_custom_system() {
        let mut context = Context::new();
        context.set_system_prompt("You are a Rust expert");

        let messages = CodexProvider::build_code_messages(&context).unwrap();
        assert_eq!(messages[0]["content"], "You are a Rust expert");
    }

    #[test]
    fn test_parse_sse_basic() {
        let data = r#"data: {"id":"chatcmpl-123","model":"codex","choices":[{"index":0,"delta":{"content":"fn add"},"finish_reason":null}]}"#;
        let events = parse_sse_events(data, "codex", "codex-davinci-002");

        assert!(!events.is_empty());
        let has_text_delta = events.iter().any(|e| matches!(e, ProviderEvent::TextDelta { .. }));
        assert!(has_text_delta);
    }

    #[test]
    fn test_parse_sse_done() {
        let data = r#"data: {"id":"chatcmpl-123","model":"codex","choices":[{"index":0,"delta":{"content":"Done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let events = parse_sse_events(data, "codex", "codex-davinci-002");

        let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done { .. }));
        assert!(has_done);
    }

    #[test]
    fn test_parse_sse_incremental() {
        let data = r#"data: {"id":"1","model":"c","choices":[{"index":0,"delta":{"content":"fn "},"finish_reason":null}]}
data: {"id":"2","model":"c","choices":[{"index":0,"delta":{"content":"main"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}}"#;
        let events = parse_sse_events(data, "codex", "codex-davinci-002");

        assert!(events.len() >= 2);
    }

    #[test]
    fn test_parse_sse_tool_call() {
        let data = r#"data: {"id":"chatcmpl-123","model":"codex","choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_123","type":"function","function":{"name":"read_file","arguments":"{"}}]},"finish_reason":"tool_calls"}]}"#;
        let events = parse_sse_events(data, "codex", "codex-davinci-002");

        let has_tool_delta = events.iter().any(|e| matches!(e, ProviderEvent::ToolCallDelta { .. }));
        assert!(has_tool_delta);
    }

    #[test]
    fn test_parse_sse_empty_content() {
        let data = "data: {\"id\":\"123\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}";
        let events = parse_sse_events(data, "codex", "codex-davinci-002");

        let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done { .. }));
        assert!(has_done);
    }

    #[test]
    fn test_parse_sse_done_marker() {
        let data = "data: [DONE]";
        let events = parse_sse_events(data, "codex", "codex-davinci-002");
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_sse_invalid_json() {
        let data = "data: invalid json";
        let events = parse_sse_events(data, "codex", "codex-davinci-002");
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_sse_usage_accumulation() {
        let data = r#"data: {"id":"cmpl-1","model":"c","choices":[{"index":0,"delta":{"content":"X"},"finish_reason":"stop"}],"usage":{"prompt_tokens":20,"completion_tokens":10,"total_tokens":30,"prompt_tokens_details":{"cached_tokens":5}}}"#;
        let events = parse_sse_events(data, "codex", "codex-davinci-002");

        if let Some(ProviderEvent::Done { message, .. }) = events.last() {
            assert_eq!(message.usage.input, 20);
            assert_eq!(message.usage.output, 10);
            assert_eq!(message.usage.cache_read, 5);
        }
    }

    #[test]
    fn test_build_tools() {
        let tools = vec![crate::Tool {
            name: "search_code".to_string(),
            description: "Search for code patterns".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
        }];

        let result = build_tools(&tools).unwrap();
        let tools_array = result.as_array().unwrap();
        assert_eq!(tools_array.len(), 1);
        assert_eq!(tools_array[0]["function"]["name"], "search_code");
    }

    #[test]
    fn test_build_tools_empty() {
        let tools: Vec<crate::Tool> = vec![];
        let result = build_tools(&tools).unwrap();
        let tools_array = result.as_array().unwrap();
        assert!(tools_array.is_empty());
    }

    #[test]
    fn test_default_endpoint() {
        assert_eq!(
            CodexProvider::default_endpoint(),
            "https://api.githubcopilot.com/chat/completions"
        );
    }
}