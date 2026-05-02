//! OpenAI-compatible provider implementation

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;

use crate::{
    Api, AssistantMessage, ContentBlock, Context, Model, Provider, 
    error::ProviderError, ProviderEvent, StopReason, StreamOptions, Usage,
};

/// OpenAI-compatible provider
#[derive(Clone)]
pub struct OpenAiProvider {
    client: Client,
    api_key: Option<String>,
}

impl OpenAiProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            api_key: std::env::var("OPENAI_API_KEY").ok(),
        }
    }
    
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: Some(api_key.into()),
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
        let url = format!("{}/chat/completions", model.base_url);
        
        // Get API key
        let api_key = options.api_key.as_ref()
            .or(self.api_key.as_ref())
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
        let response = self.client
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
        
        let stream = response.bytes_stream()
            .flat_map(move |chunk: Result<Bytes, reqwest::Error>| {
                match chunk {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        futures::stream::iter(parse_sse_events(&text, &provider_name, &model_id))
                    }
                    Err(e) => futures::stream::iter(vec![ProviderEvent::Error {
                        reason: StopReason::Error,
                        error: create_error_message(&e.to_string(), &provider_name, &model_id),
                    }]),
                }
            });
        
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
                    crate::MessageContent::Blocks(blocks) => {
                        blocks_to_content(blocks)?.to_string()
                    }
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
    
    let items: Result<Vec<_>, _> = blocks.iter().map(|block| {
        match block {
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
            ContentBlock::Unknown(_) => {
                Err(ProviderError::InvalidResponse("Unknown content block type".into()))
            }
        }
    }).collect();
    
    Ok(serde_json::json!(items?))
}

/// Build tools array
fn build_tools(tools: &[crate::Tool]) -> Result<JsonValue, ProviderError> {
    let items: Vec<_> = tools.iter().map(|tool| {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            },
        })
    }).collect();
    
    Ok(serde_json::json!(items))
}

/// Parse SSE event stream
fn parse_sse_events(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let mut partial_message = AssistantMessage::new(
        Api::OpenAiCompletions,
        provider,
        model_id,
    );
    
    for line in text.lines() {
        if line.is_empty() || line == "data: [DONE]" {
            continue;
        }
        
        if let Some(data) = line.strip_prefix("data: ") {
            // Parse JSON chunk
            if let Ok(chunk) = serde_json::from_str::<SSEChunk>(data) {
                // Process choices
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
                    
                    // Check for completion
                    if choice.finish_reason.is_some() {
                        let reason = match choice.finish_reason.as_deref() {
                            Some("stop") => StopReason::Stop,
                            Some("length") => StopReason::Length,
                            Some("tool_calls") => StopReason::ToolUse,
                            _ => StopReason::Stop,
                        };
                        
                        events.push(ProviderEvent::Done {
                            reason,
                            message: partial_message.clone(),
                        });
                    }
                }
                
                // Update usage if present
                if let Some(usage) = &chunk.usage {
                    partial_message.usage = Usage {
                        input: usage.prompt_tokens,
                        output: usage.completion_tokens,
                        cache_read: usage.prompt_tokens_details.as_ref().map(|d| d.cached_tokens).unwrap_or(0),
                        cache_write: 0,
                        total_tokens: usage.total_tokens,
                        cost: Default::default(),
                    };
                }
            }
        }
    }
    
    events
}

/// Create error assistant message
fn create_error_message(msg: &str, provider: &str, model_id: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(
        Api::OpenAiCompletions,
        provider,
        model_id,
    );
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// SSE chunk structure
#[derive(Debug, Deserialize)]
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
struct ToolCallDelta {
    index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    type_: Option<String>,
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
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