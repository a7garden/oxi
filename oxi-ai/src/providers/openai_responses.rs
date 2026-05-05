//! OpenAI Responses API provider implementation
//!
//! This provider implements the newer OpenAI Responses API, which differs from
//! the traditional Completions API in several ways:
//! - Uses `input` instead of `messages`
//! - Returns structured output items with events like `response.output_item.added`
//! - Tool calls use `type: "function_call"` with `call_id`
//! - Supports reasoning/thinking with effort levels

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

/// OpenAI Responses API provider
#[derive(Clone)]
pub struct OpenAiResponsesProvider {
    client: &'static Client,
    api_key: Option<String>,
}

impl OpenAiResponsesProvider {
    /// Create a new provider using the OPENAI_API_KEY environment variable
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_key: std::env::var("OPENAI_API_KEY").ok(),
        }
    }

    /// Create a provider with a specific API key
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
        }
    }
}

impl Default for OpenAiResponsesProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Build the request URL
        let url = format!("{}/responses", model.base_url);

        // Get API key
        let api_key = options
            .api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .ok_or_else(|| ProviderError::MissingApiKey)?;

        // Build input array (replaces messages in Responses API)
        let input = build_input(context)?;

        // Build request body
        let mut body = serde_json::json!({
            "model": model.id,
            "input": input,
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
            body["tools"] = build_tools(&context.tools);
        }

        // Add reasoning if enabled via thinking level
        if let Some(ref thinking_level) = options.thinking_level {
            if thinking_level != &crate::ThinkingLevel::Off {
                if let Some(effort) = thinking_level.as_str() {
                    body["reasoning"] = serde_json::json!({
                        "effort": effort,
                    });
                }
            }
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

        // Add custom headers
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
        "openai-responses"
    }
}

/// Build the input array for the Responses API
///
/// The Responses API uses `input` instead of `messages`. It supports both
/// simple text inputs and structured content with roles.
fn build_input(context: &Context) -> Result<Vec<JsonValue>, ProviderError> {
    let mut input = Vec::new();

    // System prompt becomes a developer message
    if let Some(ref prompt) = context.system_prompt {
        input.push(serde_json::json!({
            "role": "developer",
            "content": prompt,
        }));
    }

    // Convert conversation messages
    for msg in &context.messages {
        match msg {
            crate::Message::User(u) => {
                let content = match &u.content {
                    crate::MessageContent::Text(s) => serde_json::json!(s.clone()),
                    crate::MessageContent::Blocks(blocks) => blocks_to_json(blocks)?,
                };
                input.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
            }
            crate::Message::Assistant(a) => {
                let content = blocks_to_json(&a.content)?;
                input.push(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }));
            }
            crate::Message::ToolResult(t) => {
                let content = blocks_to_json(&t.content)?;
                input.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
            }
        }
    }

    Ok(input)
}

/// Convert content blocks to JSON
fn blocks_to_json(blocks: &[ContentBlock]) -> Result<JsonValue, ProviderError> {
    if blocks.len() == 1 {
        if let Some(text) = blocks[0].as_text() {
            return Ok(JsonValue::String(text.to_string()));
        }
    }

    let items: Result<Vec<_>, _> = blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(t) => Ok(serde_json::json!({
                "type": "output_text",
                "text": t.text,
            })),
            ContentBlock::ToolCall(tc) => Ok(serde_json::json!({
                "type": "function_call",
                "id": tc.id,
                "name": tc.name,
                "arguments": tc.arguments.to_string(),
            })),
            ContentBlock::Thinking(th) => Ok(serde_json::json!({
                "type": "reasoning",
                "summary": [
                    {
                        "type": "summary_text",
                        "text": th.thinking,
                    }
                ]
            })),
            ContentBlock::Image(img) => Ok(serde_json::json!({
                "type": "input_image",
                "data": format!("data:{};base64,{}", img.mime_type, img.data),
                "mime_type": img.mime_type,
            })),
            ContentBlock::Unknown(_) => Err(ProviderError::InvalidResponse(
                "Unknown content block type".into(),
            )),
        })
        .collect();

    Ok(serde_json::json!(items?))
}

/// Build tools array for the Responses API
fn build_tools(tools: &[crate::Tool]) -> JsonValue {
    let items: Vec<_> = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            })
        })
        .collect();

    serde_json::json!(items)
}

/// Parse SSE events from the Responses API stream
///
/// The Responses API emits different events than the Completions API:
/// - `response.created` - Response started
/// - `response.output_item.added` - New output item (message, function_call, reasoning)
/// - `response.content_part.added` - Content part added to item
/// - `response.output_text.delta` - Text delta for output_text
/// - `response.function_call_arguments.delta` - Arguments delta for function_call
/// - `response.completed` - Response completed
fn parse_sse_events(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let mut partial_message = AssistantMessage::new(Api::OpenAiResponses, provider, model_id);
    let mut current_text_index: Option<usize> = None;
    let mut current_tool_call_index: Option<usize> = None;
    let mut accumulated_usage = Usage::default();

    // Pre-estimate capacity
    let estimated_events = text
        .split('\n')
        .filter(|l| l.starts_with("event: ") || l.starts_with("data: "))
        .count();
    events.reserve(estimated_events);

    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        // Parse event line
        if line.starts_with("event: ") {
            let event_name = line[7..].trim();
            // Track current event type for data line processing
            match event_name {
                "response.created"
                | "response.output_item.added"
                | "response.content_part.added"
                | "response.output_text.delta"
                | "response.function_call_arguments.delta"
                | "response.completed"
                | "response.output_text.done"
                | "response.reasoning.done" => {
                    // Event type tracked in data lines
                }
                _ => {}
            }
            continue;
        }

        // Parse data line
        if !line.starts_with("data: ") {
            continue;
        }

        let data = line[6..].trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }

        // Parse the event data
        if let Ok(event) = serde_json::from_str::<ResponsesEvent>(data) {
            match event {
                ResponsesEvent::ResponseCreatedData { response } => {
                    if let Some(id) = response.id {
                        partial_message.response_id = Some(id);
                    }
                    events.push(ProviderEvent::Start {
                        partial: partial_message.clone(),
                    });
                }
                ResponsesEvent::OutputItemAdded { output_item } => {
                    match output_item.r#type.as_str() {
                        "message" => {
                            events.push(ProviderEvent::ToolCallStart {
                                content_index: output_item.index,
                                tool_call_id: output_item.id.clone(),
                                partial: partial_message.clone(),
                            });
                            current_tool_call_index = Some(output_item.index);
                        }
                        "function_call" => {
                            events.push(ProviderEvent::ToolCallStart {
                                content_index: output_item.index,
                                tool_call_id: output_item.id.clone(),
                                partial: partial_message.clone(),
                            });
                            current_tool_call_index = Some(output_item.index);
                        }
                        "reasoning" => {
                            events.push(ProviderEvent::ThinkingStart {
                                content_index: output_item.index,
                                partial: partial_message.clone(),
                            });
                        }
                        _ => {}
                    }
                }
                ResponsesEvent::ContentPartAdded { content_part } => {
                    match content_part.r#type.as_str() {
                        "output_text" => {
                            events.push(ProviderEvent::TextStart {
                                content_index: content_part.index,
                                partial: partial_message.clone(),
                            });
                            current_text_index = Some(content_part.index);
                        }
                        "function_call" => {
                            events.push(ProviderEvent::ToolCallStart {
                                content_index: content_part.index,
                                tool_call_id: None,
                                partial: partial_message.clone(),
                            });
                            current_tool_call_index = Some(content_part.index);
                        }
                        _ => {}
                    }
                }
                ResponsesEvent::OutputTextDelta { output_text: delta } => {
                    // Use the index from the delta if available, otherwise use current tracked index
                    let content_idx = delta.content_index.or(current_text_index).unwrap_or(0);
                    events.push(ProviderEvent::TextDelta {
                        content_index: content_idx,
                        delta: delta.slice.unwrap_or_default(),
                        partial: partial_message.clone(),
                    });
                    // Update the current text index if not already set
                    if current_text_index.is_none() {
                        current_text_index = Some(content_idx);
                    }
                }
                ResponsesEvent::FunctionCallArgumentsDelta {
                    function_call: delta,
                } => {
                    // Use the index from the delta if available
                    let content_idx = delta.content_index.or(current_tool_call_index).unwrap_or(0);
                    events.push(ProviderEvent::ToolCallDelta {
                        content_index: content_idx,
                        delta: delta.arguments.unwrap_or_default(),
                        partial: partial_message.clone(),
                    });
                    // Update the current tool call index if not set
                    if current_tool_call_index.is_none() {
                        current_tool_call_index = Some(content_idx);
                    }
                }
                ResponsesEvent::OutputTextDone { output_text } => {
                    if let Some(idx) = current_text_index {
                        let text_content = output_text
                            .content
                            .map(|c| c.text.unwrap_or_default())
                            .unwrap_or_default();
                        events.push(ProviderEvent::TextEnd {
                            content_index: idx,
                            content: text_content,
                            partial: partial_message.clone(),
                        });
                        current_text_index = None;
                    }
                }
                ResponsesEvent::ReasoningDone { reasoning } => {
                    if let Some(summary) = reasoning.summary {
                        for item in summary {
                            if item.r#type == "summary_text" {
                                events.push(ProviderEvent::ThinkingEnd {
                                    content_index: 0,
                                    content: item.text.unwrap_or_default(),
                                    partial: partial_message.clone(),
                                });
                            }
                        }
                    }
                }
                ResponsesEvent::ResponseWithUsage { response } => {
                    // Check if this is incomplete or completed
                    let is_incomplete = response.incomplete_details.is_some();

                    // Update usage if available
                    if let Some(usage) = response.usage {
                        accumulated_usage.input = usage.input_tokens;
                        accumulated_usage.output = usage.output_tokens;
                        accumulated_usage.total_tokens = usage.total_tokens;
                        if let Some(cached) = usage.input_tokens_details {
                            accumulated_usage.cache_read = cached.cached_tokens;
                        }
                    }

                    // Determine stop reason based on whether response is incomplete
                    let stop_reason = if is_incomplete {
                        if let Some(incomplete) = response.incomplete_details {
                            match incomplete.reason.as_str() {
                                "max_output_tokens" => StopReason::Length,
                                "content_filter" => StopReason::Error,
                                _ => StopReason::Stop,
                            }
                        } else {
                            StopReason::Stop
                        }
                    } else {
                        StopReason::Stop
                    };

                    let mut done_msg = partial_message.clone();
                    done_msg.usage = accumulated_usage.clone();
                    events.push(ProviderEvent::Done {
                        reason: stop_reason,
                        message: done_msg,
                    });
                }
                _ => {}
            }
        }
    }

    events
}

/// Create error assistant message
fn create_error_message(msg: &str, provider: &str, model_id: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(Api::OpenAiResponses, provider, model_id);
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// ============================================================================
// SSE Event Structures
// ============================================================================

/// Root event wrapper that can be any Responses API event
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ResponsesEvent {
    // Response-related events (check for usage field to distinguish)
    ResponseWithUsage {
        response: ResponseWithUsageData,
    },
    // Output item added
    OutputItemAdded {
        output_item: OutputItem,
    },
    // Content part added
    ContentPartAdded {
        content_part: ContentPart,
    },
    // Output text delta
    OutputTextDelta {
        output_text: TextDelta,
    },
    // Function call arguments delta
    FunctionCallArgumentsDelta {
        function_call: FunctionCallDelta,
    },
    // Output text done
    OutputTextDone {
        output_text: OutputTextDone,
    },
    // Reasoning done
    ReasoningDone {
        reasoning: ReasoningDone,
    },
    // General response created (no usage field)
    ResponseCreatedData {
        response: ResponseCreatedData,
    },
    // Fallback for unrecognized formats
    Unknown(JsonValue),
}

#[derive(Debug, Deserialize)]
struct ResponseCreatedData {
    id: Option<String>,
    #[serde(rename = "object")]
    object: Option<String>,
    status: Option<String>,
    #[serde(rename = "model")]
    model: Option<String>,
    created_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct OutputItem {
    index: usize,
    #[serde(rename = "type")]
    r#type: String,
    id: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentPart {
    index: usize,
    #[serde(rename = "type")]
    r#type: String,
}

#[derive(Debug, Deserialize)]
struct TextDelta {
    content_index: Option<usize>,
    output_index: Option<usize>,
    slice: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FunctionCallDelta {
    content_index: Option<usize>,
    output_index: Option<usize>,
    name: Option<String>,
    arguments: Option<String>,
    call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OutputTextDone {
    content_index: Option<usize>,
    output_index: Option<usize>,
    content: Option<TextContent>,
}

#[derive(Debug, Deserialize)]
struct TextContent {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReasoningDone {
    content_index: Option<usize>,
    output_index: Option<usize>,
    summary: Option<Vec<SummaryItem>>,
}

#[derive(Debug, Deserialize)]
struct SummaryItem {
    #[serde(rename = "type")]
    r#type: String,
    text: Option<String>,
}

/// Unified response data that can match both completed and incomplete responses
#[derive(Debug, Deserialize)]
struct ResponseWithUsageData {
    id: Option<String>,
    status: Option<String>,
    usage: Option<UsageData>,
    incomplete_details: Option<IncompleteDetails>,
}

#[derive(Debug, Deserialize)]
struct CompletedResponse {
    id: Option<String>,
    status: Option<String>,
    usage: Option<UsageData>,
}

#[derive(Debug, Deserialize)]
struct IncompleteResponse {
    id: Option<String>,
    status: Option<String>,
    incomplete_details: Option<IncompleteDetails>,
}

#[derive(Debug, Deserialize)]
struct IncompleteDetails {
    reason: String,
}

#[derive(Debug, Deserialize)]
struct UsageData {
    input_tokens: usize,
    output_tokens: usize,
    total_tokens: usize,
    #[serde(rename = "input_tokens_details")]
    input_tokens_details: Option<InputTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct InputTokensDetails {
    #[serde(rename = "cached_tokens")]
    cached_tokens: usize,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Message, Model, TextContent};
    use serde_json::json;

    fn create_test_model() -> Model {
        Model::new(
            "gpt-4o",
            "GPT-4o",
            Api::OpenAiResponses,
            "openai-responses",
            "https://api.openai.com/v1",
        )
    }

    fn create_test_context() -> Context {
        Context::new()
    }

    #[test]
    fn test_provider_name() {
        let provider = OpenAiResponsesProvider::new();
        assert_eq!(provider.name(), "openai-responses");
    }

    #[test]
    fn test_build_input_with_text() {
        let mut context = create_test_context();
        context.add_message(Message::user("Hello, world!"));

        let input = build_input(&context).unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "Hello, world!");
    }

    #[test]
    fn test_build_input_with_system_prompt() {
        let mut context = create_test_context();
        context.set_system_prompt("You are a helpful assistant.");
        context.add_message(Message::user("Hi!"));

        let input = build_input(&context).unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "developer");
        assert_eq!(input[0]["content"], "You are a helpful assistant.");
    }

    #[test]
    fn test_build_input_with_multiple_messages() {
        let mut context = create_test_context();
        context.add_message(Message::user("First message"));
        context.add_message(Message::user("Second message"));

        let input = build_input(&context).unwrap();
        assert_eq!(input.len(), 2);
    }

    #[test]
    fn test_blocks_to_json_text() {
        let blocks = vec![ContentBlock::Text(TextContent::new("Hello"))];
        let result = blocks_to_json(&blocks).unwrap();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_blocks_to_json_multiple_blocks() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("Hello")),
            ContentBlock::Text(TextContent::new("World")),
        ];
        let result = blocks_to_json(&blocks).unwrap();
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_build_tools() {
        let tools = vec![crate::Tool {
            name: "get_weather".to_string(),
            description: "Get weather for a location".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "location": {"type": "string"}
                }
            }),
        }];

        let result = build_tools(&tools);
        assert!(result.is_array());
        let tool = &result[0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["name"], "get_weather");
    }

    #[test]
    fn test_parse_response_created_event() {
        // Data-only format
        let sse_data =
            r#"data: {"response":{"id":"resp_123","status":"in_progress","model":"gpt-4o"}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        assert!(!events.is_empty());
        if let ProviderEvent::Start { partial } = &events[0] {
            assert_eq!(partial.api, Api::OpenAiResponses);
        }
    }

    #[test]
    fn test_parse_output_item_added_event() {
        // Data-only format
        let sse_data = r#"data: {"output_item":{"index":0,"id":"msg_123","type":"message","status":"in_progress"}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        // Should contain a ToolCallStart event
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ToolCallStart { .. })));
    }

    #[test]
    fn test_parse_text_delta_event() {
        // Data-only format (the parser processes data lines, event lines are metadata)
        let sse_data = r#"data: {"output_text":{"content_index":0,"slice":"Hello"}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::TextDelta { .. })));
    }

    #[test]
    fn test_parse_function_call_delta_event() {
        // Data-only format
        let sse_data = r#"data: {"function_call":{"content_index":0,"arguments":"{\"location"}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ToolCallDelta { .. })));
    }

    #[test]
    fn test_parse_completed_event_with_usage() {
        // Data-only format
        let sse_data = r#"data: {"response":{"id":"resp_123","status":"completed","usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::Done {
                reason: StopReason::Stop,
                ..
            }
        )));
    }

    #[test]
    fn test_parse_reasoning_event() {
        // Data-only format
        let sse_data = r#"data: {"reasoning":{"content_index":0,"summary":[{"type":"summary_text","text":"Thinking process..."}]}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ThinkingEnd { .. })));
    }

    #[test]
    fn test_provider_with_api_key() {
        let provider = OpenAiResponsesProvider::with_api_key("sk-test-key");
        // Provider should be created successfully
        assert_eq!(provider.name(), "openai-responses");
    }

    #[test]
    fn test_multiple_events_in_stream() {
        // Multiple data lines
        let sse_data = r#"data: {"response":{"id":"resp_123"}}
data: {"output_item":{"index":0,"type":"message"}}
data: {"output_text":{"slice":"Hello"}}
data: {"response":{"status":"completed"}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        assert!(events.len() >= 4);
    }

    #[test]
    fn test_invalid_json_skipped() {
        let sse_data = r#"event: response.created
data: {invalid json here}
event: response.created
data: {"response":{"id":"resp_123"}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        // Should skip invalid and continue
        assert!(!events.is_empty());
    }

    #[test]
    fn test_done_marker() {
        let sse_data = r#"event: response.created
data: {"response":{"id":"resp_123"}}
data: [DONE]"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        // Should stop at [DONE]
        assert!(events.len() <= 2);
    }

    #[test]
    fn test_incomplete_response() {
        // Data-only format
        let sse_data = r#"data: {"response":{"id":"resp_123","incomplete_details":{"reason":"max_output_tokens"}}}"#;

        let events = parse_sse_events(sse_data, "openai-responses", "gpt-4o");
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::Done {
                reason: StopReason::Length,
                ..
            }
        )));
    }
}
