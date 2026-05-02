//! Anthropic provider implementation

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

use super::{Provider, ProviderEvent, ProviderError, StreamOptions};
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Model, StopReason, TextContent,
    ThinkingContent, ToolCall, Usage,
};

/// Anthropic provider
pub struct AnthropicProvider {
    client: Client,
    api_key: Option<String>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
        }
    }
    
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
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
    ) -> Result<impl Stream<Item = ProviderEvent> + Send, ProviderError> {
        let options = options.unwrap_or_default();
        
        // Build the request
        let url = format!("{}/v1/messages", model.base_url);
        
        // Get API key
        let api_key = options.api_key.as_ref()
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
        let mut headers = HashMap::new();
        headers.insert("x-api-key".to_string(), api_key.clone());
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        
        for (k, v) in &options.headers {
            headers.insert(k.clone(), v.clone());
        }
        
        // Make request
        let response = self.client
            .post(&url)
            .headers(convert_headers(&headers))
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::RequestFailed)?;
        
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpError(status.as_u16(), body));
        }
        
        // Create event stream
        let stream = response.bytes_stream()
            .map(|chunk| {
                match chunk {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        parse_anthropic_events(&text, model)
                    }
                    Err(e) => vec![ProviderEvent::Error {
                        reason: StopReason::Error,
                        error: create_error_message(&e.to_string()),
                    }],
                }
            })
            .flatten();
        
        Ok(stream)
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
                    crate::MessageContent::Blocks(blocks) => {
                        blocks_to_anthropic_content(blocks)?
                    }
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
    let items: Vec<_> = tools.iter().map(|tool| {
        serde_json::json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.parameters,
        })
    }).collect();
    
    Ok(serde_json::json!(items))
}

/// Parse Anthropic SSE event stream
fn parse_anthropic_events(text: &str, model: &Model) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let mut partial_message = AssistantMessage::new(
        Api::AnthropicMessages,
        &model.provider,
        &model.id,
    );
    
    for line in text.lines() {
        if line.is_empty() || line == "data: [DONE]" {
            continue;
        }
        
        if let Some(data) = line.strip_prefix("data: ") {
            if let Ok(event) = serde_json::from_str::<AnthropicEvent>(data) {
                match event.clone_type.as_deref() {
                    Some("message_start") => {
                        events.push(ProviderEvent::Start {
                            partial: partial_message.clone(),
                        });
                    }
                    Some("content_block_start") => {
                        if let Some(block) = &event.content_block {
                            match block.type_.as_deref() {
                                Some("text") => {
                                    events.push(ProviderEvent::TextStart {
                                        content_index: block.index.unwrap_or(0),
                                        partial: partial_message.clone(),
                                    });
                                }
                                Some("thinking") => {
                                    events.push(ProviderEvent::ThinkingStart {
                                        content_index: block.index.unwrap_or(0),
                                        partial: partial_message.clone(),
                                    });
                                }
                                Some("tool_use") => {
                                    events.push(ProviderEvent::ToolCallStart {
                                        content_index: block.index.unwrap_or(0),
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
                            
                            events.push(ProviderEvent::Done {
                                reason,
                                message: partial_message.clone(),
                            });
                        }
                    }
                    Some("message_stop") => {
                        // Message complete
                    }
                    _ => {}
                }
                
                // Update usage
                if let Some(usage) = &event.usage {
                    partial_message.usage = Usage {
                        input: usage.input_tokens,
                        output: usage.output_tokens,
                        cache_read: usage.cache_read,
                        cache_write: usage.cache_creation,
                        total_tokens: usage.input_tokens + usage.output_tokens,
                        cost: Default::default(),
                    };
                }
            }
        }
    }
    
    events
}

/// Create error assistant message
fn create_error_message(msg: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(
        Api::AnthropicMessages,
        "anthropic",
        "unknown",
    );
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

/// Convert headers map to reqwest header type
fn convert_headers(headers: &HashMap<String, String>) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(name), Ok(value)) = (
            k.parse::<reqwest::header::HeaderName>(),
            v.parse::<reqwest::header::HeaderValue>(),
        ) {
            h.insert(name, value);
        }
    }
    h
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
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    type_: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    partial_json: Option<String>,
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

impl AnthropicEvent {
    fn clone_type(&self) -> Option<String> {
        self.type_.clone()
    }
}
