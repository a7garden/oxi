//! Ollama provider — local LLM server via NDJSON streaming.
//!
//! Implements the `Provider` trait for Ollama's `/api/chat` endpoint.
//! Ollama streams newline-delimited JSON (NDJSON), not SSE. Tool calls
//! arrive as complete objects per chunk (no delta accumulation needed).
//!
//! Port of omp `packages/ai/src/providers/ollama.ts` (minimal working subset).

use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;
use std::sync::Arc;

use super::shared_client;
use super::sse::split_complete_lines;
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Message, MessageContent, Model, Provider,
    ProviderEvent, StopReason, StreamOptions, StreamResult, TextContent, ThinkingContent, ToolCall,
    Usage, error::ProviderError,
};

/// Default Ollama server URL.
const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Ollama provider for local LLM inference.
#[derive(Clone)]
pub struct OllamaProvider {
    client: &'static Client,
    base_url: String,
    /// Optional API key for Ollama Cloud (Bearer token).
    api_key: Option<String>,
}

impl OllamaProvider {
    /// Create a new Ollama provider pointing at the default local server.
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: None,
        }
    }

    /// Create with a custom base URL (e.g. remote Ollama instance).
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: shared_client(),
            base_url: normalize_base_url(base_url),
            api_key: None,
        }
    }

    /// Create with a custom base URL and optional API key (Ollama Cloud).
    pub fn with_config(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            client: shared_client(),
            base_url: normalize_base_url(base_url),
            api_key,
        }
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip trailing `/api` if present (users often paste the full endpoint).
fn normalize_base_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    url.strip_suffix("/api").unwrap_or(url).to_string()
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// A single NDJSON chunk from Ollama's `/api/chat`.
#[derive(Debug, Deserialize)]
struct OllamaChunk {
    #[serde(default)]
    message: Option<OllamaMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<usize>,
    #[serde(default)]
    eval_count: Option<usize>,
    /// Error field present on error responses.
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCall {
    function: OllamaFunction,
}

#[derive(Debug, Deserialize)]
struct OllamaFunction {
    name: String,
    /// Arguments can be an object or a JSON string.
    arguments: Option<JsonValue>,
}

// ---------------------------------------------------------------------------
// Request construction
// ---------------------------------------------------------------------------

/// Build the JSON request body for `/api/chat`.
fn build_request_body(
    model: &Model,
    context: &Context,
    options: &Option<StreamOptions>,
) -> JsonValue {
    let mut messages = Vec::new();

    // System prompt as a system message.
    if let Some(prompt) = &context.system_prompt {
        messages.push(serde_json::json!({
            "role": "system",
            "content": prompt,
        }));
    }

    // Convert conversation messages.
    for msg in &context.messages {
        match msg {
            Message::User(user_msg) => {
                let text = match &user_msg.content {
                    MessageContent::Text(s) => s.clone(),
                    MessageContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": text,
                }));
            }
            Message::Assistant(asst_msg) => {
                let text = asst_msg.text_content();
                let tool_calls: Vec<JsonValue> = asst_msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::ToolCall(tc) = b {
                            Some(serde_json::json!({
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments,
                                }
                            }))
                        } else {
                            None
                        }
                    })
                    .collect();

                let mut obj = serde_json::json!({
                    "role": "assistant",
                    "content": text,
                });
                if !tool_calls.is_empty() {
                    obj["tool_calls"] = JsonValue::Array(tool_calls);
                }
                messages.push(obj);
            }
            Message::ToolResult(tr) => {
                let text: String = tr
                    .content
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect::<Vec<_>>()
                    .join("\n");
                messages.push(serde_json::json!({
                    "role": "tool",
                    "content": text,
                }));
            }
        }
    }

    let mut body = serde_json::json!({
        "model": model.id,
        "messages": messages,
        "stream": true,
    });

    // Tools.
    if !context.tools.is_empty() {
        let tools: Vec<JsonValue> = context
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": sanitize_schema_for_ollama(&t.parameters),
                    }
                })
            })
            .collect();
        body["tools"] = JsonValue::Array(tools);
    }

    // Thinking level → Ollama `think` parameter.
    if let Some(opts) = options {
        if let Some(level) = opts.thinking_level
            && let Some(s) = level.as_str()
        {
            body["think"] = JsonValue::String(s.to_string());
        }
        if let Some(max_tokens) = opts.max_tokens {
            body["options"] = serde_json::json!({ "num_predict": max_tokens });
        }
    }

    body
}

/// Sanitize JSON Schema for Ollama's llama.cpp grammar backend.
///
/// llama.cpp rejects boolean subschemas (`true`/`false`) and boolean
/// `additionalProperties`. This recursively normalizes them.
fn sanitize_schema_for_ollama(schema: &JsonValue) -> JsonValue {
    match schema {
        JsonValue::Bool(true) => serde_json::json!({}),
        JsonValue::Bool(false) => serde_json::json!({"not": {}}),
        JsonValue::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                match key.as_str() {
                    "additionalProperties" if value.is_boolean() => {
                        // Drop boolean additionalProperties entirely.
                        if value.as_bool() == Some(false) {
                            out.insert(key.clone(), JsonValue::Bool(false));
                        }
                        // `true` → omit (permissive is the default).
                    }
                    _ => {
                        out.insert(key.clone(), sanitize_schema_for_ollama(value));
                    }
                }
            }
            JsonValue::Object(out)
        }
        JsonValue::Array(arr) => {
            JsonValue::Array(arr.iter().map(sanitize_schema_for_ollama).collect())
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

impl Provider for OllamaProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("{}/api/chat", self.base_url);
            let body = build_request_body(model, context, &options);

            let mut req = self.client.post(&url).json(&body);
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            let response = req.send().await?;

            let status = response.status();
            if !status.is_success() {
                let body_text = response.text().await.unwrap_or_default();
                return Err(ProviderError::HttpError(crate::HttpErrorDetail {
                    status: status.as_u16(),
                    body: body_text,
                    provider: Some("ollama".to_string()),
                    request_id: None,
                }));
            }

            let byte_stream = response.bytes_stream();
            let model_id = model.id.clone();
            let provider_name = "ollama".to_string();

            // Accumulator state for the NDJSON stream.
            let partial =
                AssistantMessage::new(Api::OllamaChat, provider_name.clone(), model_id.clone());
            let trailing_bytes: Vec<u8> = Vec::new();

            // Prepend Start event before the scan closure captures model_id.
            let start_event = ProviderEvent::Start {
                partial: Arc::new(AssistantMessage::new(Api::OllamaChat, "ollama", &model_id)),
            };

            let events = byte_stream.scan(
                (partial, trailing_bytes, false, false, 0usize),
                move |(partial, trailing, thinking_started, text_started, tc_counter),
                      chunk_result| {
                    let chunk: Bytes = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            let mut err_msg = AssistantMessage::new(
                                Api::OllamaChat,
                                provider_name.clone(),
                                model_id.clone(),
                            );
                            err_msg.error_message = Some(format!("Stream error: {e}"));
                            err_msg.stop_reason = StopReason::Error;
                            return futures::future::ready(Some(vec![ProviderEvent::Error {
                                reason: StopReason::Error,
                                error: err_msg,
                            }]));
                        }
                    };

                    // Prepend trailing bytes from previous chunk.
                    let mut buf = std::mem::take(trailing);
                    buf.extend_from_slice(&chunk);

                    let (complete, new_trailing) = split_complete_lines(&buf);
                    *trailing = new_trailing;

                    let mut events: Vec<ProviderEvent> = Vec::new();

                    for line in complete.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }

                        let parsed: OllamaChunk = match serde_json::from_str(line) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };

                        // Error response from Ollama.
                        if let Some(err) = &parsed.error {
                            let mut err_msg = AssistantMessage::new(
                                Api::OllamaChat,
                                provider_name.clone(),
                                model_id.clone(),
                            );
                            err_msg.error_message = Some(err.clone());
                            err_msg.stop_reason = StopReason::Error;
                            events.push(ProviderEvent::Error {
                                reason: StopReason::Error,
                                error: err_msg,
                            });
                            return futures::future::ready(Some(events));
                        }

                        if let Some(msg) = &parsed.message {
                            // Thinking delta.
                            if let Some(thinking) = &msg.thinking
                                && !thinking.is_empty()
                            {
                                let idx = if !*thinking_started {
                                    *thinking_started = true;
                                    partial
                                        .content
                                        .push(ContentBlock::Thinking(ThinkingContent::new("")));
                                    let idx = partial.content.len() - 1;
                                    events.push(ProviderEvent::ThinkingStart {
                                        content_index: idx,
                                        partial: Arc::new(partial.clone()),
                                    });
                                    idx
                                } else {
                                    partial.content.len() - 1
                                };
                                if let Some(ContentBlock::Thinking(t)) = partial.content.last_mut()
                                {
                                    t.thinking.push_str(thinking);
                                }
                                events.push(ProviderEvent::ThinkingDelta {
                                    content_index: idx,
                                    delta: thinking.clone(),
                                    partial: Arc::new(partial.clone()),
                                });
                            }

                            // Text content delta.
                            if let Some(content) = &msg.content
                                && !content.is_empty()
                            {
                                let idx = if !*text_started {
                                    *text_started = true;
                                    partial
                                        .content
                                        .push(ContentBlock::Text(TextContent::new("")));
                                    let idx = partial.content.len() - 1;
                                    events.push(ProviderEvent::TextStart {
                                        content_index: idx,
                                        partial: Arc::new(partial.clone()),
                                    });
                                    idx
                                } else {
                                    partial.content.len() - 1
                                };
                                if let Some(ContentBlock::Text(t)) = partial.content.last_mut() {
                                    t.text.push_str(content);
                                }
                                events.push(ProviderEvent::TextDelta {
                                    content_index: idx,
                                    delta: content.clone(),
                                    partial: Arc::new(partial.clone()),
                                });
                            }

                            // Tool calls (complete objects, not deltas).
                            if let Some(tool_calls) = &msg.tool_calls {
                                for tc in tool_calls {
                                    let id = format!("ollama_tc_{}", *tc_counter);
                                    *tc_counter += 1;

                                    let arguments = tc
                                        .function
                                        .arguments
                                        .clone()
                                        .unwrap_or(JsonValue::Object(Default::default()));

                                    let tool_call =
                                        ToolCall::new(&id, &tc.function.name, arguments);
                                    partial
                                        .content
                                        .push(ContentBlock::ToolCall(tool_call.clone()));
                                    let idx = partial.content.len() - 1;

                                    events.push(ProviderEvent::ToolCallStart {
                                        content_index: idx,
                                        tool_call_id: Some(id),
                                        tool_name: Some(tc.function.name.clone()),
                                        partial: Arc::new(partial.clone()),
                                    });
                                    events.push(ProviderEvent::ToolCallEnd {
                                        content_index: idx,
                                        tool_call,
                                        partial: Arc::new(partial.clone()),
                                    });
                                }
                            }
                        }

                        // Terminal chunk.
                        if parsed.done {
                            // Close open thinking block.
                            if *thinking_started {
                                let idx = partial.content.len() - 1;
                                let content = partial
                                    .content
                                    .iter()
                                    .find_map(|b| {
                                        if let ContentBlock::Thinking(t) = b {
                                            Some(t.thinking.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or_default();
                                events.push(ProviderEvent::ThinkingEnd {
                                    content_index: idx,
                                    content,
                                    partial: Arc::new(partial.clone()),
                                });
                            }
                            // Close open text block.
                            if *text_started {
                                let idx = partial.content.len() - 1;
                                let content = partial.text_content();
                                events.push(ProviderEvent::TextEnd {
                                    content_index: idx,
                                    content,
                                    partial: Arc::new(partial.clone()),
                                });
                            }

                            // Usage.
                            partial.usage = Usage {
                                input: parsed.prompt_eval_count.unwrap_or(0),
                                output: parsed.eval_count.unwrap_or(0),
                                ..Default::default()
                            };

                            // Stop reason.
                            partial.stop_reason = match parsed.done_reason.as_deref() {
                                Some("length") => StopReason::Length,
                                Some("tool_calls") => StopReason::ToolUse,
                                _ => {
                                    if *tc_counter > 0 {
                                        StopReason::ToolUse
                                    } else {
                                        StopReason::Stop
                                    }
                                }
                            };

                            events.push(ProviderEvent::Done {
                                reason: partial.stop_reason,
                                message: partial.clone(),
                            });
                            return futures::future::ready(Some(events));
                        }
                    }

                    if events.is_empty() {
                        futures::future::ready(None)
                    } else {
                        futures::future::ready(Some(events))
                    }
                },
            );

            // Flatten Vec<ProviderEvent> into individual events, prepend Start.

            let stream = futures::stream::once(async { start_event })
                .chain(events.flat_map(futures::stream::iter));

            Ok(Box::pin(stream) as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_base_url() {
        assert_eq!(
            normalize_base_url("http://localhost:11434"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("http://localhost:11434/"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("http://localhost:11434/api"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("http://localhost:11434/api/"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn test_sanitize_schema_boolean_subschema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "extra": true,
            },
            "additionalProperties": true,
        });
        let result = sanitize_schema_for_ollama(&schema);
        // `true` in properties → `{}`
        assert_eq!(result["properties"]["extra"], serde_json::json!({}));
        // `additionalProperties: true` → removed
        assert!(result.get("additionalProperties").is_none());
    }

    #[test]
    fn test_sanitize_schema_false_additional_properties() {
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
        });
        let result = sanitize_schema_for_ollama(&schema);
        assert_eq!(result["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn test_build_request_body_basic() {
        let model = Model::new("llama3", "Llama 3", Api::OllamaChat, "ollama", "");
        let mut ctx = Context::new();
        ctx.system_prompt = Some("You are helpful.".to_string());

        let body = build_request_body(&model, &ctx, &None);
        assert_eq!(body["model"], "llama3");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "You are helpful.");
    }

    #[test]
    fn test_ollama_chunk_deserialization() {
        let json = r#"{"message":{"role":"assistant","content":"Hello"},"done":false}"#;
        let chunk: OllamaChunk = serde_json::from_str(json).unwrap();
        assert!(!chunk.done);
        assert_eq!(chunk.message.unwrap().content.unwrap(), "Hello");
    }

    #[test]
    fn test_ollama_chunk_done() {
        let json = r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":20}"#;
        let chunk: OllamaChunk = serde_json::from_str(json).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.done_reason.unwrap(), "stop");
        assert_eq!(chunk.prompt_eval_count.unwrap(), 10);
        assert_eq!(chunk.eval_count.unwrap(), 20);
    }

    #[test]
    fn test_ollama_chunk_tool_calls() {
        let json = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"type":"function","function":{"name":"read_file","arguments":{"path":"src/main.rs"}}}]},"done":false}"#;
        let chunk: OllamaChunk = serde_json::from_str(json).unwrap();
        let msg = chunk.message.unwrap();
        let tcs = msg.tool_calls.unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "read_file");
        assert_eq!(
            tcs[0].function.arguments.as_ref().unwrap()["path"],
            "src/main.rs"
        );
    }

    #[test]
    fn test_ollama_chunk_thinking() {
        let json = r#"{"message":{"role":"assistant","content":"","thinking":"Let me think..."},"done":false}"#;
        let chunk: OllamaChunk = serde_json::from_str(json).unwrap();
        let msg = chunk.message.unwrap();
        assert_eq!(msg.thinking.unwrap(), "Let me think...");
    }

    #[test]
    fn test_ollama_chunk_error() {
        let json = r#"{"error":"model 'foo' not found"}"#;
        let chunk: OllamaChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.error.unwrap(), "model 'foo' not found");
    }
}
