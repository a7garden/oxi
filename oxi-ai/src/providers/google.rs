//! Google Generative AI provider (Gemini API)

use async_trait::async_trait;
use futures::Stream;
use futures::stream::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;

use super::{Provider, ProviderEvent, ProviderError, StreamOptions};
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Model, StopReason, Usage,
};

/// Google Generative AI provider
#[derive(Clone)]
pub struct GoogleProvider {
    client: Client,
    api_key: Option<String>,
}

impl GoogleProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            api_key: std::env::var("GOOGLE_API_KEY").ok(),
        }
    }
    
    #[allow(dead_code)]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: Some(api_key.into()),
        }
    }
}

impl Default for GoogleProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for GoogleProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();
        
        // Get API key
        let api_key = options.api_key.as_ref()
            .or(self.api_key.as_ref())
            .ok_or_else(|| ProviderError::MissingApiKey)?;
        
        // Build the request
        let model_id = &model.id;
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
            model_id, api_key
        );
        
        // Build contents
        let contents = build_google_contents(context)?;
        
        // Build request body
        let mut body = serde_json::json!({
            "contents": contents,
            "stream": true,
        });
        
        // Add generation config
        let mut generation_config = serde_json::json!({});
        
        if let Some(temp) = options.temperature {
            generation_config["temperature"] = serde_json::json!(temp);
        }
        
        if let Some(max) = options.max_tokens {
            generation_config["maxOutputTokens"] = serde_json::json!(max);
        }
        
        // Only add generation config if there are actual values
        let has_config = options.temperature.is_some() || options.max_tokens.is_some();
        if has_config {
            body["generationConfig"] = generation_config;
        }
        
        // Add system instruction
        if let Some(ref prompt) = context.system_prompt {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{ "text": prompt }]
            });
        }
        
        // Add tools if present
        if !context.tools.is_empty() {
            body["tools"] = build_google_tools(&context.tools)?;
        }
        
        // Make request
        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
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
        
        let stream = response.bytes_stream()
            .flat_map(move |chunk| {
                match chunk {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        futures::stream::iter(parse_google_events(&text, &model_name))
                    }
                    Err(e) => futures::stream::iter(vec![ProviderEvent::Error {
                        reason: StopReason::Error,
                        error: create_error_message(&e.to_string()),
                    }]),
                }
            });
        
        Ok(Box::pin(stream))
    }
    
    fn name(&self) -> &str {
        "google"
    }
}

/// Build contents in Google Gemini format
fn build_google_contents(context: &Context) -> Result<Vec<JsonValue>, ProviderError> {
    let mut contents = Vec::new();
    
    for msg in &context.messages {
        match msg {
            crate::Message::User(u) => {
                let parts = match &u.content {
                    crate::MessageContent::Text(s) => vec![serde_json::json!({ "text": s })],
                    crate::MessageContent::Blocks(blocks) => blocks_to_google_parts(blocks)?,
                };
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
            crate::Message::Assistant(a) => {
                let parts = blocks_to_google_parts(&a.content)?;
                contents.push(serde_json::json!({
                    "role": "model",
                    "parts": parts,
                }));
            }
            crate::Message::ToolResult(t) => {
                let parts = blocks_to_google_parts(&t.content)?;
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
        }
    }
    
    Ok(contents)
}

/// Convert content blocks to Google parts format
fn blocks_to_google_parts(blocks: &[ContentBlock]) -> Result<Vec<JsonValue>, ProviderError> {
    let mut parts = Vec::new();
    
    for block in blocks {
        match block {
            ContentBlock::Text(t) => {
                parts.push(serde_json::json!({
                    "text": t.text,
                }));
            }
            ContentBlock::ToolCall(tc) => {
                parts.push(serde_json::json!({
                    "functionCall": {
                        "name": tc.name,
                        "args": tc.arguments,
                    },
                }));
            }
            ContentBlock::Image(img) => {
                parts.push(serde_json::json!({
                    "inlineData": {
                        "mimeType": img.mime_type,
                        "data": img.data,
                    },
                }));
            }
            ContentBlock::Thinking(th) => {
                // Google doesn't have native thinking blocks, send as text
                parts.push(serde_json::json!({
                    "text": format!("[Thinking: {}]", th.thinking),
                }));
            }
            ContentBlock::Unknown(_) => {
                // Skip unknown blocks
            }
        }
    }
    
    Ok(parts)
}

/// Build tools in Google format
fn build_google_tools(tools: &[crate::Tool]) -> Result<JsonValue, ProviderError> {
    let declarations: Vec<_> = tools.iter().map(|tool| {
        serde_json::json!({
            "functionDeclarations": [{
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            }]
        })
    }).collect();
    
    Ok(serde_json::json!(declarations))
}

/// Parse Google Gemini SSE event stream
fn parse_google_events(text: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let mut partial_message = AssistantMessage::new(
        Api::GoogleGenerativeAi,
        "google",
        model_id,
    );
    
    for line in text.lines() {
        if line.is_empty() || line == "data: [DONE]" {
            continue;
        }
        
        if let Some(data) = line.strip_prefix("data: ") {
            if let Ok(response) = serde_json::from_str::<GoogleResponse>(data) {
                // Process candidates
                for candidate in &response.candidates {
                    // Process content
                    if let Some(content) = &candidate.content {
                        for (index, part) in content.parts.iter().enumerate() {
                            if let Some(text) = &part.text {
                                events.push(ProviderEvent::TextDelta {
                                    content_index: index,
                                    delta: text.clone(),
                                    partial: partial_message.clone(),
                                });
                            }
                            
                            if let Some(function_call) = &part.function_call {
                                events.push(ProviderEvent::ToolCallDelta {
                                    content_index: index,
                                    delta: serde_json::to_string(&function_call.args).unwrap_or_default(),
                                    partial: partial_message.clone(),
                                });
                            }
                        }
                    }
                }
                
                // Update usage if present
                if let Some(usage) = &response.usage_metadata {
                    partial_message.usage = Usage {
                        input: usage.prompt_token_count.unwrap_or(0),
                        output: usage.candidates_token_count.unwrap_or(0),
                        cache_read: 0,
                        cache_write: 0,
                        total_tokens: usage.total_token_count.unwrap_or(0),
                        cost: Default::default(),
                    };
                }
                
                // Check if done
                if let Some(ref finish_reason) = response.candidates.first().and_then(|c| c.finish_reason.clone()) {
                    let reason = match finish_reason.as_str() {
                        "STOP" => StopReason::Stop,
                        "MAX_TOKENS" => StopReason::Length,
                        "SAFETY" | "OTHER" => StopReason::Error,
                        _ => StopReason::Stop,
                    };
                    
                    // Always emit Done event — even on error, stream has ended
                    events.push(ProviderEvent::Done {
                        reason,
                        message: partial_message.clone(),
                    });
                }
            }
        }
    }
    
    events
}

/// Create error assistant message
fn create_error_message(msg: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(
        Api::GoogleGenerativeAi,
        "google",
        "unknown",
    );
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// Google Gemini response structures
#[derive(Debug, Deserialize)]
struct GoogleResponse {
    candidates: Vec<Candidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    content: Option<Content>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Content {
    parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
struct Part {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<FunctionCall>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FunctionCall {
    name: String,
    args: JsonValue,
}

#[derive(Debug, Deserialize)]
struct UsageMetadata {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<usize>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<usize>,
    #[serde(rename = "totalTokenCount")]
    total_token_count: Option<usize>,
}