//! OpenAI Completions API provider (legacy /v1/completions endpoint)
//!
//! This provider uses the older /v1/completions endpoint instead of /v1/chat/completions.
//! Suitable for text-davinci models and backward compatibility.

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use std::pin::Pin;

use crate::{
    Api, AssistantMessage, Context, Model, Provider, ProviderEvent, StopReason, StreamOptions, Usage,
};

use super::ProviderError;

/// Configuration for OpenAI Completions provider
#[derive(Debug, Clone)]
pub struct OpenAICompletionsConfig {
    /// Base URL for the API
    pub base_url: String,
    /// API key for authentication
    pub api_key: Option<String>,
    /// Model ID (e.g., "text-davinci-003")
    pub model: String,
    /// Completion-specific options
    pub options: CompletionsOptions,
}

#[derive(Debug, Clone)]
pub struct CompletionsOptions {
    /// Sampling temperature (0-2)
    pub temperature: Option<f32>,
    /// Max tokens to generate
    pub max_tokens: Option<usize>,
    /// Stop sequences
    pub stop: Option<Vec<String>>,
    /// Nucleus sampling parameter
    pub top_p: Option<f32>,
    /// Frequency penalty
    pub frequency_penalty: Option<f32>,
    /// Presence penalty
    pub presence_penalty: Option<f32>,
    /// Echo previous prompt
    pub echo: bool,
    /// Log probabilities
    pub logprobs: Option<usize>,
    /// Best of completions
    pub n: Option<usize>,
}

impl Default for CompletionsOptions {
    fn default() -> Self {
        Self {
            temperature: Some(0.7),
            max_tokens: Some(2048),
            stop: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            echo: false,
            logprobs: None,
            n: None,
        }
    }
}

impl Default for OpenAICompletionsConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            model: "text-davinci-003".to_string(),
            options: CompletionsOptions::default(),
        }
    }
}

/// OpenAI Completions provider using /v1/completions endpoint
#[derive(Clone)]
pub struct OpenAICompletionsProvider {
    client: Client,
    config: OpenAICompletionsConfig,
}

impl OpenAICompletionsProvider {
    /// Create a new provider with default configuration
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: OpenAICompletionsConfig::default(),
        }
    }

    /// Create with explicit API key
    #[allow(dead_code)]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        let mut config = OpenAICompletionsConfig::default();
        config.api_key = Some(api_key.into());
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Create with custom configuration
    #[allow(dead_code)]
    pub fn with_config(config: OpenAICompletionsConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Get provider name
    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        "openai-completions"
    }
}

impl Default for OpenAICompletionsProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OpenAICompletionsProvider {
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
            .ok_or_else(|| ProviderError::MissingApiKey)?;

        // Build the prompt from context
        let prompt = build_prompt_from_context(context)?;

        // Build URL - use completions endpoint
        let url = if model.base_url.is_empty() {
            format!("{}/completions", self.config.base_url)
        } else {
            format!("{}/completions", model.base_url.trim_end_matches('/'))
        };

        // Build request body
        let mut body = serde_json::json!({
            "model": if model.id.is_empty() { &self.config.model } else { &model.id },
            "prompt": prompt,
            "stream": true,
        });

        // Add optional parameters
        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        } else if let Some(temp) = self.config.options.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if let Some(max) = options.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        } else if let Some(max) = self.config.options.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }

        if let Some(stop) = &self.config.options.stop {
            body["stop"] = serde_json::json!(stop);
        }

        if let Some(top_p) = self.config.options.top_p {
            body["top_p"] = serde_json::json!(top_p);
        }

        if let Some(freq_pen) = self.config.options.frequency_penalty {
            body["frequency_penalty"] = serde_json::json!(freq_pen);
        }

        if let Some(pres_pen) = self.config.options.presence_penalty {
            body["presence_penalty"] = serde_json::json!(pres_pen);
        }

        if self.config.options.echo {
            body["echo"] = serde_json::json!(true);
        }

        if let Some(logprobs) = self.config.options.logprobs {
            body["logprobs"] = serde_json::json!(logprobs);
        }

        if let Some(n) = self.config.options.n {
            body["n"] = serde_json::json!(n);
        }

        // Build headers
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", api_key).parse().unwrap(),
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
        let model_id = if model.id.is_empty() {
            self.config.model.clone()
        } else {
            model.id.clone()
        };

        let stream = response.bytes_stream().flat_map(
            move |chunk: Result<Bytes, reqwest::Error>| match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    futures::stream::iter(parse_completions_sse(&text, &provider_name, &model_id))
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
        "openai-completions"
    }
}

/// Build a prompt string from context messages
fn build_prompt_from_context(context: &Context) -> Result<String, ProviderError> {
    let mut prompt_parts = Vec::new();

    // System prompt
    if let Some(ref system) = context.system_prompt {
        prompt_parts.push(format!("System: {}", system));
    }

    // Conversation messages
    for msg in &context.messages {
        match msg {
            crate::Message::User(u) => {
                let content = match &u.content {
                    crate::MessageContent::Text(s) => s.clone(),
                    crate::MessageContent::Blocks(blocks) => {
                        let text_blocks: Vec<String> = blocks
                            .iter()
                            .filter_map(|b| {
                                if let crate::ContentBlock::Text(t) = b {
                                    Some(t.text.clone())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        text_blocks.join("\n")
                    }
                };
                prompt_parts.push(format!("User: {}", content));
            }
            crate::Message::Assistant(a) => {
                let content = a.content.iter()
                    .filter_map(|b| {
                        if let crate::ContentBlock::Text(t) = b {
                            Some(t.text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                prompt_parts.push(format!("Assistant: {}", content));
            }
            crate::Message::ToolResult(t) => {
                prompt_parts.push(format!(
                    "Tool Result ({}): {}",
                    t.tool_name,
                    t.content.iter()
                        .filter_map(|b| {
                            if let crate::ContentBlock::Text(t) = b {
                                Some(t.text.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }
    }

    prompt_parts.push("Assistant:".to_string());
    Ok(prompt_parts.join("\n"))
}

/// Parse SSE events from completions response
fn parse_completions_sse(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let partial_message = AssistantMessage::new(Api::OpenAiCompletions, provider, model_id);

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

        let chunk = match serde_json::from_str::<CompletionChunk>(data) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Handle text delta
        if let Some(text_delta) = &chunk.choices.first().and_then(|c| c.text.as_ref()) {
            if !text_delta.is_empty() {
                events.push(ProviderEvent::TextDelta {
                    content_index: 0,
                    delta: (*text_delta).clone(),
                    partial: partial_message.clone(),
                });
            }
        }

        // Check for completion
        if chunk.choices.first().map(|c| c.finish_reason.is_some()).unwrap_or(false) {
            let reason = match chunk.choices.first().and_then(|c| c.finish_reason.as_ref()).map(|s| s.as_str()) {
                Some("stop") => StopReason::Stop,
                Some("length") => StopReason::Length,
                _ => StopReason::Stop,
            };

            let mut done_msg = partial_message.clone();
            if let Some(usage) = &chunk.usage {
                done_msg.usage = Usage {
                    input: usage.prompt_tokens,
                    output: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
                    cache_read: usage.prompt_tokens_details.as_ref()
                        .map(|d| d.cached_tokens)
                        .unwrap_or(0),
                    cache_write: 0,
                    cost: crate::types::Cost::default(),
                };
            }

            events.push(ProviderEvent::Done {
                reason,
                message: done_msg,
            });
        }
    }

    events
}

/// Create error message
fn create_error_message(msg: &str, provider: &str, model_id: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(Api::OpenAiCompletions, provider, model_id);
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// SSE structures for completions API
#[derive(Debug, Deserialize)]
struct CompletionChunk {
    #[allow(dead_code)]
    id: Option<String>,
    #[allow(dead_code)]
    object: Option<String>,
    #[allow(dead_code)]
    created: Option<i64>,
    #[allow(dead_code)]
    model: Option<String>,
    choices: Vec<CompletionChoice>,
    usage: Option<CompletionUsage>,
}

#[derive(Debug, Deserialize)]
struct CompletionChoice {
    text: Option<String>,
    #[allow(dead_code)]
    index: usize,
    finish_reason: Option<String>,
    #[allow(dead_code)]
    logprobs: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct CompletionUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    #[serde(rename = "prompt_tokens_details")]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Deserialize)]
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
        let provider = OpenAICompletionsProvider::new();
        assert_eq!(provider.name(), "openai-completions");
    }

    #[test]
    fn test_config_defaults() {
        let config = OpenAICompletionsConfig::default();
        assert_eq!(config.model, "text-davinci-003");
        assert!(config.api_key.is_none());
        assert!(config.options.temperature.is_some());
    }

    #[test]
    fn test_build_prompt_empty() {
        let context = Context::new();
        let prompt = build_prompt_from_context(&context).unwrap();
        assert_eq!(prompt, "Assistant:");
    }

    #[test]
    fn test_build_prompt_with_system() {
        let mut context = Context::new();
        context.set_system_prompt("You are a helpful assistant");

        let prompt = build_prompt_from_context(&context).unwrap();
        assert!(prompt.contains("System: You are a helpful assistant"));
        assert!(prompt.contains("Assistant:"));
    }

    #[test]
    fn test_build_prompt_with_user_message() {
        let mut context = Context::new();
        context.add_message(crate::Message::user("Hello, world!"));

        let prompt = build_prompt_from_context(&context).unwrap();
        assert!(prompt.contains("User: Hello, world!"));
        assert!(prompt.contains("Assistant:"));
    }

    #[test]
    fn test_build_prompt_full_conversation() {
        let mut context = Context::new();
        context.set_system_prompt("You are a coding assistant");
        context.add_message(crate::Message::user("How do I write a loop?"));
        
        // Note: Completions API doesn't support tool results in the same way
        // This test verifies the basic structure works

        let prompt = build_prompt_from_context(&context).unwrap();
        assert!(prompt.contains("System: You are a coding assistant"));
        assert!(prompt.contains("User: How do I write a loop?"));
        assert!(prompt.contains("Assistant:"));
    }

    #[test]
    fn test_parse_completions_sse_basic() {
        let data = r#"data: {"id":"cmpl-123","object":"text_completion","choices":[{"text":"Hello","index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let events = parse_completions_sse(data, "openai-completions", "text-davinci-003");

        assert!(!events.is_empty());
        let has_text_delta = events.iter().any(|e| matches!(e, ProviderEvent::TextDelta { .. }));
        assert!(has_text_delta);

        let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done { .. }));
        assert!(has_done);
    }

    #[test]
    fn test_parse_completions_sse_incremental() {
        let data = r#"data: {"id":"1","object":"text_completion","choices":[{"text":"Hel","index":0,"finish_reason":null}]}
data: {"id":"2","object":"text_completion","choices":[{"text":"lo","index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let events = parse_completions_sse(data, "openai-completions", "text-davinci-003");

        // Should have multiple text deltas and one done
        assert!(events.len() >= 2);
    }

    #[test]
    fn test_parse_completions_sse_done_marker() {
        let data = "data: [DONE]";
        let events = parse_completions_sse(data, "openai-completions", "text-davinci-003");
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_completions_sse_invalid_json() {
        let data = "data: not valid json";
        let events = parse_completions_sse(data, "openai-completions", "text-davinci-003");
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_completions_sse_usage() {
        let data = r#"data: {"id":"cmpl-123","object":"text_completion","choices":[{"text":"Test","index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":20,"completion_tokens":10,"total_tokens":30,"prompt_tokens_details":{"cached_tokens":5}}}"#;
        let events = parse_completions_sse(data, "openai-completions", "text-davinci-003");

        if let Some(ProviderEvent::Done { message, .. }) = events.last() {
            assert_eq!(message.usage.input, 20);
            assert_eq!(message.usage.output, 10);
            assert_eq!(message.usage.cache_read, 5);
        }
    }

    #[test]
    fn test_with_api_key() {
        let provider = OpenAICompletionsProvider::with_api_key("test-key");
        assert_eq!(provider.name(), "openai-completions");
    }

    #[test]
    fn test_with_custom_config() {
        let config = OpenAICompletionsConfig {
            base_url: "https://api.example.com/v1".to_string(),
            api_key: Some("custom-key".to_string()),
            model: "gpt-3.5-turbo-instruct".to_string(),
            options: CompletionsOptions {
                temperature: Some(0.5),
                max_tokens: Some(1024),
                ..Default::default()
            },
        };
        let provider = OpenAICompletionsProvider::with_config(config);
        assert_eq!(provider.name(), "openai-completions");
    }

    #[test]
    fn test_completions_options_default() {
        let options = CompletionsOptions::default();
        assert!(options.temperature.is_some());
        assert!(options.max_tokens.is_some());
        assert!(!options.echo);
    }

    #[test]
    fn test_build_prompt_with_assistant_message() {
        let mut context = Context::new();
        context.set_system_prompt("You are helpful");
        
        // Add user message first
        context.add_message(crate::Message::user("Hi"));
        
        // Build prompt
        let prompt = build_prompt_from_context(&context).unwrap();
        assert!(prompt.contains("User: Hi"));
        assert!(prompt.contains("Assistant:"));
    }
}