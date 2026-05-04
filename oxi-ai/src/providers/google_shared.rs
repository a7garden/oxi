#![allow(dead_code)]
//! Shared utilities for Google Generative AI and Google Vertex providers.
//!
//! This module contains common types, message conversion, tool conversion,
//! stop reason mapping, and streaming event parsing used by both the
//! `google` (Gemini API) and `vertex` (Vertex AI) providers.

use serde::Deserialize;
use serde_json::Value as JsonValue;

use super::{ProviderEvent, ProviderError};
use crate::{Api, AssistantMessage, ContentBlock, Context, StopReason, Tool, Usage};

// ---------------------------------------------------------------------------
// Google Thinking Level
// ---------------------------------------------------------------------------

/// Thinking level for Gemini 3+ models.
///
/// Mirrors Google's `ThinkingLevel` enum values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GoogleThinkingLevel {
    ThinkingLevelUnspecified,
    Minimal,
    Low,
    Medium,
    High,
}

// ---------------------------------------------------------------------------
// Streaming helpers
// ---------------------------------------------------------------------------

/// Determines whether a streamed Gemini `Part` should be treated as "thinking".
///
/// In the Google API protocol, `thought: true` is the definitive marker for
/// thinking content (thought summaries). `thoughtSignature` can appear on ANY
/// part type and does NOT indicate thinking content.
///
/// See: <https://ai.google.dev/gemini-api/docs/thought-signatures>
pub fn is_thinking_part(part: &GooglePart) -> bool {
    part.thought == Some(true)
}

/// Retain thought signatures during streaming.
///
/// Some backends only send `thoughtSignature` on the first delta for a given
/// part/block; later deltas may omit it. This helper preserves the last
/// non-empty signature for the current block.
pub fn retain_thought_signature(
    existing: Option<&str>,
    incoming: Option<&str>,
) -> Option<String> {
    match incoming {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => existing.map(|s| s.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Stop reason mapping
// ---------------------------------------------------------------------------

/// Map a Google `FinishReason` string to our `StopReason`.
///
/// Google FinishReason values are strings like `"STOP"`, `"MAX_TOKENS"`, etc.
pub fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        // Safety-related reasons → Error
        "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "SAFETY"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION"
        | "IMAGE_OTHER"
        | "RECITATION"
        | "FINISH_REASON_UNSPECIFIED"
        | "OTHER"
        | "LANGUAGE"
        | "MALFORMED_FUNCTION_CALL"
        | "UNEXPECTED_TOOL_CALL"
        | "NO_IMAGE" => StopReason::Error,
        _ => StopReason::Error,
    }
}

// ---------------------------------------------------------------------------
// Tool call ID helpers
// ---------------------------------------------------------------------------

/// Models via Google APIs that require explicit tool call IDs in function calls/responses.
pub fn requires_tool_call_id(model_id: &str) -> bool {
    model_id.starts_with("claude-") || model_id.starts_with("gpt-oss-")
}

/// Normalize a tool call ID for models that require alphanumeric-only IDs.
#[allow(dead_code)]
pub fn normalize_tool_call_id(model_id: &str, id: &str) -> String {
    if !requires_tool_call_id(model_id) {
        return id.to_string();
    }
    id.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect::<String>()
        .chars()
        .take(64)
        .collect()
}

// ---------------------------------------------------------------------------
// Message conversion: Context → Google Content[]
// ---------------------------------------------------------------------------

/// Convert internal messages to Google Gemini `Content[]` format.
///
/// This transforms `Context.messages` into the JSON structure expected by both
/// the Gemini API and Vertex AI endpoints.
pub fn convert_messages(context: &Context) -> Result<Vec<JsonValue>, ProviderError> {
    let mut contents: Vec<JsonValue> = Vec::new();

    for msg in &context.messages {
        match msg {
            crate::Message::User(u) => {
                let parts = match &u.content {
                    crate::MessageContent::Text(s) => {
                        vec![serde_json::json!({ "text": s })]
                    }
                    crate::MessageContent::Blocks(blocks) => {
                        blocks_to_google_parts(blocks)?
                    }
                };
                if parts.is_empty() {
                    continue;
                }
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
            crate::Message::Assistant(a) => {
                let parts = blocks_to_google_parts(&a.content)?;
                if parts.is_empty() {
                    continue;
                }
                contents.push(serde_json::json!({
                    "role": "model",
                    "parts": parts,
                }));
            }
            crate::Message::ToolResult(t) => {
                // Extract text content
                let text_parts: Vec<&str> = t
                    .content
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect();
                let text_result = text_parts.join("\n");

                let has_text = !text_result.is_empty();

                // Build function response value
                let response_value = if has_text {
                    text_result.clone()
                } else {
                    String::new()
                };

                let function_response_part = if t.is_error {
                    serde_json::json!({
                        "functionResponse": {
                            "name": t.tool_name,
                            "response": { "error": response_value }
                        }
                    })
                } else {
                    serde_json::json!({
                        "functionResponse": {
                            "name": t.tool_name,
                            "response": { "output": response_value }
                        }
                    })
                };

                // Merge into existing user turn if the last content is already a user
                // turn with function responses (Cloud Code Assist requirement).
                let last_is_user_with_fn_response = contents
                    .last()
                    .and_then(|c| c.get("role"))
                    .and_then(|r| r.as_str())
                    .map(|r| r == "user")
                    .unwrap_or(false)
                    && contents
                        .last()
                        .and_then(|c| c.get("parts"))
                        .and_then(|p| p.as_array())
                        .map(|arr| arr.iter().any(|p| p.get("functionResponse").is_some()))
                        .unwrap_or(false);

                if last_is_user_with_fn_response {
                    if let Some(last) = contents.last_mut() {
                        if let Some(parts) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                            parts.push(function_response_part);
                        }
                    }
                } else {
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [function_response_part],
                    }));
                }
            }
        }
    }

    Ok(contents)
}

// ---------------------------------------------------------------------------
// Tool conversion: Tool[] → Google functionDeclarations
// ---------------------------------------------------------------------------

/// JSON Schema meta-declarations to strip when using OpenAPI `parameters`.
const JSON_SCHEMA_META_KEYS: &[&str] = &[
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$vocabulary",
    "$comment",
    "$defs",
    "definitions",
];

/// Strip JSON Schema meta-declarations from a schema object (recursive).
fn sanitize_for_openapi(schema: &JsonValue) -> JsonValue {
    match schema {
        JsonValue::Object(map) => {
            let mut result = serde_json::Map::new();
            for (key, value) in map {
                if JSON_SCHEMA_META_KEYS.contains(&key.as_str()) {
                    continue;
                }
                result.insert(key.clone(), sanitize_for_openapi(value));
            }
            JsonValue::Object(result)
        }
        JsonValue::Array(arr) => {
            JsonValue::Array(arr.iter().map(sanitize_for_openapi).collect())
        }
        other => other.clone(),
    }
}

/// Convert tools to Google `functionDeclarations` format.
///
/// Returns `None` if the tools list is empty.
///
/// When `use_parameters` is true, uses the legacy `parameters` field (OpenAPI 3.03)
/// instead of `parametersJsonSchema` (full JSON Schema). This is needed for
/// Cloud Code Assist with Claude models.
pub fn convert_tools(
    tools: &[Tool],
    use_parameters: bool,
) -> Option<JsonValue> {
    if tools.is_empty() {
        return None;
    }

    let declarations: Vec<JsonValue> = tools
        .iter()
        .map(|tool| {
            let params = if use_parameters {
                serde_json::json!({
                    "parameters": sanitize_for_openapi(&tool.parameters)
                })
            } else {
                serde_json::json!({
                    "parametersJsonSchema": tool.parameters
                })
            };

            // Merge name, description with the parameters field
            let mut obj = serde_json::json!({
                "name": tool.name,
                "description": tool.description,
            });
            if let JsonValue::Object(ref mut map) = obj {
                if let JsonValue::Object(param_map) = params {
                    for (k, v) in param_map {
                        map.insert(k, v);
                    }
                }
            }
            obj
        })
        .collect();

    Some(serde_json::json!([{
        "functionDeclarations": declarations
    }]))
}

// ---------------------------------------------------------------------------
// Content block conversion
// ---------------------------------------------------------------------------

/// Convert content blocks to Google parts format.
pub fn blocks_to_google_parts(blocks: &[ContentBlock]) -> Result<Vec<JsonValue>, ProviderError> {
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
                // Google/Vertex supports thinking blocks via `thought: true` marker.
                parts.push(serde_json::json!({
                    "thought": true,
                    "text": th.thinking,
                }));
            }
            ContentBlock::Unknown(_) => {
                // Skip unknown blocks
            }
        }
    }

    Ok(parts)
}

// ---------------------------------------------------------------------------
// Shared request body building
// ---------------------------------------------------------------------------

/// Build the Google/Vertex request body JSON.
pub fn build_request_body(
    contents: &[JsonValue],
    system_prompt: Option<&str>,
    tools: Option<&JsonValue>,
    temperature: Option<f64>,
    max_tokens: Option<usize>,
) -> JsonValue {
    let mut body = serde_json::json!({
        "contents": contents,
    });

    // Generation config
    let mut generation_config = serde_json::json!({});
    if let Some(temp) = temperature {
        generation_config["temperature"] = serde_json::json!(temp);
    }
    if let Some(max) = max_tokens {
        generation_config["maxOutputTokens"] = serde_json::json!(max);
    }
    if let serde_json::Value::Object(ref obj) = generation_config {
        if !obj.is_empty() {
            body["generationConfig"] = generation_config;
        }
    }

    // System instruction
    if let Some(prompt) = system_prompt {
        body["systemInstruction"] = serde_json::json!({
            "parts": [{ "text": prompt }]
        });
    }

    // Tools
    if let Some(tools_json) = tools {
        body["tools"] = tools_json.clone();
    }

    body
}

// ---------------------------------------------------------------------------
// Shared SSE event parsing
// ---------------------------------------------------------------------------

/// Parse Google/Vertex SSE event stream.
///
/// Both providers use the same response format. The `api` and `provider_name`
/// parameters control how the `AssistantMessage` is constructed.
pub fn parse_google_events(
    text: &str,
    api: Api,
    provider_name: &str,
    model_id: &str,
) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let mut partial_message = AssistantMessage::new(api, provider_name, model_id);

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
                                // Check if this is a thinking part
                                if is_thinking_part(part) {
                                    events.push(ProviderEvent::ThinkingDelta {
                                        content_index: index,
                                        delta: text.clone(),
                                        partial: partial_message.clone(),
                                    });
                                } else {
                                    events.push(ProviderEvent::TextDelta {
                                        content_index: index,
                                        delta: text.clone(),
                                        partial: partial_message.clone(),
                                    });
                                }
                            }

                            if let Some(function_call) = &part.function_call {
                                events.push(ProviderEvent::ToolCallDelta {
                                    content_index: index,
                                    delta: serde_json::to_string(&function_call.args)
                                        .unwrap_or_default(),
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
                if let Some(ref finish_reason) = response
                    .candidates
                    .first()
                    .and_then(|c| c.finish_reason.clone())
                {
                    let reason = map_stop_reason(finish_reason);

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

/// Create an error `AssistantMessage` for the given provider.
pub fn create_error_message(api: Api, provider_name: &str, msg: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(api, provider_name, "unknown");
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// ---------------------------------------------------------------------------
// Google Gemini response structures
// ---------------------------------------------------------------------------

/// Google/Vertex AI response (shared between both providers).
#[derive(Debug, Deserialize)]
pub struct GoogleResponse {
    #[serde(default)]
    pub candidates: Vec<GoogleCandidate>,
    #[serde(rename = "usageMetadata", default)]
    pub usage_metadata: Option<GoogleUsageMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct GoogleCandidate {
    pub content: Option<GoogleContent>,
    #[serde(rename = "finishReason")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoogleContent {
    #[serde(default)]
    pub parts: Vec<GooglePart>,
}

#[derive(Debug, Deserialize)]
pub struct GooglePart {
    pub text: Option<String>,
    #[serde(rename = "functionCall")]
    pub function_call: Option<GoogleFunctionCall>,
    /// `true` if this part contains thinking/reasoning content.
    #[serde(default)]
    pub thought: Option<bool>,
    /// Encrypted thought signature for preserving reasoning context.
    #[serde(rename = "thoughtSignature")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoogleFunctionCall {
    pub name: String,
    #[serde(default)]
    pub args: JsonValue,
}

#[derive(Debug, Deserialize)]
pub struct GoogleUsageMetadata {
    #[serde(rename = "promptTokenCount")]
    pub prompt_token_count: Option<usize>,
    #[serde(rename = "candidatesTokenCount")]
    pub candidates_token_count: Option<usize>,
    #[serde(rename = "totalTokenCount")]
    pub total_token_count: Option<usize>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_thinking_part() {
        let mut part = GooglePart {
            text: Some("thinking...".to_string()),
            function_call: None,
            thought: None,
            thought_signature: None,
        };
        assert!(!is_thinking_part(&part));

        part.thought = Some(true);
        assert!(is_thinking_part(&part));

        part.thought = Some(false);
        assert!(!is_thinking_part(&part));
    }

    #[test]
    fn test_retain_thought_signature() {
        assert_eq!(retain_thought_signature(None, None), None);
        assert_eq!(
            retain_thought_signature(None, Some("sig123")),
            Some("sig123".to_string())
        );
        assert_eq!(
            retain_thought_signature(Some("existing"), Some("new")),
            Some("new".to_string())
        );
        assert_eq!(
            retain_thought_signature(Some("existing"), None),
            Some("existing".to_string())
        );
        assert_eq!(
            retain_thought_signature(Some("existing"), Some("")),
            Some("existing".to_string())
        );
    }

    #[test]
    fn test_map_stop_reason() {
        assert_eq!(map_stop_reason("STOP"), StopReason::Stop);
        assert_eq!(map_stop_reason("MAX_TOKENS"), StopReason::Length);
        assert_eq!(map_stop_reason("SAFETY"), StopReason::Error);
        assert_eq!(map_stop_reason("OTHER"), StopReason::Error);
        assert_eq!(map_stop_reason("RECITATION"), StopReason::Error);
        assert_eq!(map_stop_reason("MALFORMED_FUNCTION_CALL"), StopReason::Error);
        assert_eq!(map_stop_reason("UNKNOWN_REASON"), StopReason::Error);
    }

    #[test]
    fn test_requires_tool_call_id() {
        assert!(requires_tool_call_id("claude-3-opus"));
        assert!(requires_tool_call_id("gpt-oss-4o"));
        assert!(!requires_tool_call_id("gemini-2.5-pro"));
        assert!(!requires_tool_call_id("gpt-4o"));
    }

    #[test]
    fn test_normalize_tool_call_id() {
        assert_eq!(
            normalize_tool_call_id("gemini-2.5-pro", "call_abc/123"),
            "call_abc/123"
        );
        assert_eq!(
            normalize_tool_call_id("claude-3-opus", "call_abc/123"),
            "call_abc_123"
        );
        let long_id = "a".repeat(100);
        let result = normalize_tool_call_id("claude-3-opus", &long_id);
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_convert_messages_with_text() {
        let mut ctx = Context::new();
        ctx.add_message(crate::Message::user("Hello, world!"));

        let contents = convert_messages(&ctx).unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello, world!");
    }

    #[test]
    fn test_convert_messages_with_assistant() {
        let mut ctx = Context::new();
        ctx.add_message(crate::Message::user("Hi"));
        ctx.add_message(crate::Message::Assistant(
            AssistantMessage::new(Api::GoogleGenerativeAi, "google", "gemini-1.5-pro"),
        ));

        let contents = convert_messages(&ctx).unwrap();
        assert_eq!(contents.len(), 1);
    }

    #[test]
    fn test_convert_tools_empty() {
        let tools: Vec<Tool> = vec![];
        assert!(convert_tools(&tools, false).is_none());
    }

    #[test]
    fn test_convert_tools_basic() {
        let tools = vec![Tool::new(
            "get_weather",
            "Get weather for a location",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "location": { "type": "string", "description": "City name" }
                },
                "required": ["location"]
            }),
        )];

        let result = convert_tools(&tools, false).unwrap();
        let declarations = result[0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0]["name"], "get_weather");
        assert!(declarations[0].get("parametersJsonSchema").is_some());
    }

    #[test]
    fn test_convert_tools_use_parameters() {
        let tools = vec![Tool::new(
            "test",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "properties": { "x": { "type": "number" } }
            }),
        )];

        let result = convert_tools(&tools, true).unwrap();
        let decl = &result[0]["functionDeclarations"][0];
        assert!(decl.get("parameters").is_some());
        assert!(decl.get("parametersJsonSchema").is_none());
        let params = &decl["parameters"];
        assert!(params.get("$schema").is_none());
        assert!(params.get("properties").is_some());
    }

    #[test]
    fn test_parse_google_events_basic_text() {
        let sse_data = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}"#;
        let events = parse_google_events(sse_data, Api::GoogleGenerativeAi, "google", "gemini-1.5-pro");
        assert!(!events.is_empty());
        if let ProviderEvent::TextDelta { delta, .. } = &events[0] {
            assert_eq!(delta, "Hello");
        } else {
            panic!("Expected TextDelta event");
        }
    }

    #[test]
    fn test_parse_google_events_thinking() {
        let sse_data =
            r#"data: {"candidates":[{"content":{"parts":[{"text":"hmm...","thought":true}]}}]}"#;
        let events = parse_google_events(sse_data, Api::GoogleGenerativeAi, "google", "gemini-2.5-pro");
        assert!(!events.is_empty());
        if let ProviderEvent::ThinkingDelta { delta, .. } = &events[0] {
            assert_eq!(delta, "hmm...");
        } else {
            panic!("Expected ThinkingDelta event");
        }
    }

    #[test]
    fn test_parse_google_events_with_usage() {
        let sse_data = r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":20,"totalTokenCount":30}}"#;
        let events = parse_google_events(sse_data, Api::GoogleGenerativeAi, "google", "gemini-1.5-pro");
        let done_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::Done { .. }))
            .collect();
        assert!(!done_events.is_empty());
    }

    #[test]
    fn test_parse_google_events_with_function_call() {
        let sse_data = r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"location":"Boston"}}}]}}]}"#;
        let events = parse_google_events(sse_data, Api::GoogleGenerativeAi, "google", "gemini-1.5-pro");
        let tool_call_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::ToolCallDelta { .. }))
            .collect();
        assert!(!tool_call_events.is_empty());
    }

    #[test]
    fn test_build_request_body() {
        let contents = vec![serde_json::json!({
            "role": "user",
            "parts": [{ "text": "Hi" }]
        })];
        let body = build_request_body(
            &contents,
            Some("You are helpful"),
            None,
            Some(0.7),
            Some(1024),
        );
        assert_eq!(&body["contents"], &serde_json::json!(contents));
        assert_eq!(body["generationConfig"]["temperature"], 0.7);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 1024);
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "You are helpful"
        );
    }

    #[test]
    fn test_create_error_message() {
        let msg = create_error_message(Api::GoogleGenerativeAi, "google", "Something went wrong");
        assert_eq!(msg.provider, "google");
        assert_eq!(msg.api, Api::GoogleGenerativeAi);
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(msg.error_message, Some("Something went wrong".to_string()));
    }
}
