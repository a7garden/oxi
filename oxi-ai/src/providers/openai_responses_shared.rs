//! Shared code between OpenAI Responses API and Codex providers.
//!
//! Ported from `openai-responses-shared.ts`. Contains:
//! - Message conversion to the Responses API `input` format
//! - Tool conversion to the Responses API `tools` format
//! - Streaming event processing helpers
//! - Tool call ID normalization for cross-provider compatibility
//! - Text signature encoding/decoding

use serde_json::Value as JsonValue;

use crate::{Api, ContentBlock, Context, Message, MessageContent, Model, Tool};

// ---------------------------------------------------------------------------
// Text signatures
// ---------------------------------------------------------------------------

/// Encode a text signature v1 payload as JSON.
pub fn encode_text_signature_v1(id: &str, phase: Option<&str>) -> String {
    let mut payload = serde_json::json!({ "v": 1, "id": id });
    if let Some(p) = phase {
        payload["phase"] = serde_json::json!(p);
    }
    payload.to_string()
}

/// Parse a text signature, returning `(id, optional_phase)`.
pub fn parse_text_signature(
    signature: Option<&str>,
) -> Option<(String, Option<String>)> {
    let sig = signature?;
    if sig.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(sig) {
            if parsed.get("v").and_then(|v| v.as_u64()) == Some(1) {
                if let Some(id) = parsed.get("id").and_then(|v| v.as_str()) {
                    let phase = parsed
                        .get("phase")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    return Some((id.to_string(), phase));
                }
            }
        }
    }
    // Legacy plain-string handling
    Some((sig.to_string(), None))
}

// ---------------------------------------------------------------------------
// Short hash utility
// ---------------------------------------------------------------------------

/// Produce a short deterministic hash of a string (hex, first 12 chars).
pub fn short_hash(input: &str) -> String {
    use std::fmt::Write;
    let hash = simple_hash(input.as_bytes());
    let mut out = String::with_capacity(12);
    for byte in &hash[..6] {
        write!(&mut out, "{:02x}", byte).unwrap();
    }
    out
}

/// Minimal deterministic hash for ID generation purposes.
/// NOT cryptographic, but sufficient for generating short unique IDs.
fn simple_hash(data: &[u8]) -> [u8; 32] {
    let mut result = [0u8; 32];
    for (i, &byte) in data.iter().enumerate() {
        result[i % 32] ^= byte;
        result[(i + 7) % 32] = result[(i + 7) % 32].wrapping_add(byte);
    }
    // Mix rounds
    for round in 0u8..4 {
        for i in 0..32usize {
            let prev = result[(i + 31) % 32];
            result[i] = result[i]
                .wrapping_add(prev)
                .wrapping_mul(17u8.wrapping_add(round));
        }
    }
    result
}

// ---------------------------------------------------------------------------
// ID normalization helpers
// ---------------------------------------------------------------------------

/// Normalize one part of a tool call ID (alphanumeric + `_-`, max 64 chars, no trailing `_`).
fn normalize_id_part(part: &str) -> String {
    let sanitized: String = part
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let truncated = if sanitized.len() > 64 {
        &sanitized[..64]
    } else {
        &sanitized
    };
    truncated.trim_end_matches('_').to_string()
}

// ---------------------------------------------------------------------------
// Convert messages to Responses API input format
// ---------------------------------------------------------------------------

/// Options for message conversion.
#[derive(Debug, Clone, Default)]
pub struct ConvertResponsesMessagesOptions {
    /// Whether to include the system prompt as a developer/system message.
    pub include_system_prompt: bool,
}

/// Convert messages from a [`Context`] into the OpenAI Responses API `input` format.
///
/// This handles:
/// - System prompt → `developer` or `system` message based on model capabilities
/// - User messages → `input_text` / `input_image` content
/// - Assistant messages → `output_text`, `function_call`, `reasoning` items
/// - Tool results → `function_call_output` items
/// - Tool call ID normalization for cross-provider compatibility
pub fn convert_responses_messages(
    model: &Model,
    context: &Context,
    _allowed_tool_call_providers: &[&str],
    options: Option<ConvertResponsesMessagesOptions>,
) -> Vec<JsonValue> {
    let opts = options.unwrap_or_default();
    let mut messages: Vec<JsonValue> = Vec::new();

    // Transform messages for the target model
    // TODO: implement cross-provider message transformation
    let transformed: Vec<crate::Message> = context.messages.clone();

    // System prompt
    if opts.include_system_prompt {
        if let Some(ref prompt) = context.system_prompt {
            let role = if model.reasoning { "developer" } else { "system" };
            messages.push(serde_json::json!({
                "role": role,
                "content": sanitize_surrogates(prompt),
            }));
        }
    }

    let mut msg_index: usize = 0;

    for msg in &transformed {
        match msg {
            Message::User(u) => {
                match &u.content {
                    MessageContent::Text(s) => {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": [{ "type": "input_text", "text": sanitize_surrogates(s) }],
                        }));
                    }
                    MessageContent::Blocks(blocks) => {
                        let content: Vec<JsonValue> = blocks
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text(t) => Some(serde_json::json!({
                                    "type": "input_text",
                                    "text": sanitize_surrogates(&t.text),
                                })),
                                ContentBlock::Image(img) => Some(serde_json::json!({
                                    "type": "input_image",
                                    "detail": "auto",
                                    "image_url": format!("data:{};base64,{}", img.mime_type, img.data),
                                })),
                                _ => None,
                            })
                            .collect();
                        if content.is_empty() {
                            msg_index += 1;
                            continue;
                        }
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": content,
                        }));
                    }
                }
            }

            Message::Assistant(a) => {
                let mut output: Vec<JsonValue> = Vec::new();
                let is_different_model = a.model != model.id
                    && a.provider == model.provider
                    && a.api == model.api;

                for block in &a.content {
                    match block {
                        ContentBlock::Thinking(th) => {
                            if let Some(ref sig) = th.thinking_signature {
                                if let Ok(reasoning_item) =
                                    serde_json::from_str::<JsonValue>(sig)
                                {
                                    output.push(reasoning_item);
                                }
                            }
                        }
                        ContentBlock::Text(t) => {
                            let parsed_sig = parse_text_signature(t.text_signature.as_deref());
                            let mut msg_id = parsed_sig
                                .as_ref()
                                .map(|(id, _)| id.clone())
                                .unwrap_or_else(|| format!("msg_{}", msg_index));

                            if msg_id.len() > 64 {
                                msg_id = format!("msg_{}", short_hash(&msg_id));
                            }

                            let phase = parsed_sig.and_then(|(_, p)| p);

                            output.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{
                                    "type": "output_text",
                                    "text": sanitize_surrogates(&t.text),
                                    "annotations": [],
                                }],
                                "status": "completed",
                                "id": msg_id,
                                "phase": phase,
                            }));
                        }
                        ContentBlock::ToolCall(tc) => {
                            let parts: Vec<&str> = tc.id.splitn(2, '|').collect();
                            let call_id = parts[0];
                            let item_id_raw = parts.get(1).copied();

                            let item_id: Option<String> = if is_different_model {
                                item_id_raw.and_then(|id| {
                                    if id.starts_with("fc_") {
                                        None
                                    } else {
                                        Some(normalize_id_part(id))
                                    }
                                })
                            } else {
                                item_id_raw.map(normalize_id_part)
                            };

                            output.push(serde_json::json!({
                                "type": "function_call",
                                "id": item_id,
                                "call_id": normalize_id_part(call_id),
                                "name": tc.name,
                                "arguments": tc.arguments.to_string(),
                            }));
                        }
                        _ => {}
                    }
                }

                if output.is_empty() {
                    msg_index += 1;
                    continue;
                }
                messages.extend(output);
            }

            Message::ToolResult(t) => {
                let text_parts: Vec<&str> = t
                    .content
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect();
                let text_result = text_parts.join("\n");
                let has_images = t.content.iter().any(|b| matches!(b, ContentBlock::Image(_)));
                let has_text = !text_result.is_empty();

                let parts: Vec<&str> = t.tool_call_id.splitn(2, '|').collect();
                let call_id = normalize_id_part(parts[0]);

                let output_val: JsonValue = if has_images
                    && model.input.contains(&crate::InputModality::Image)
                {
                    let mut content_parts: Vec<JsonValue> = Vec::new();
                    if has_text {
                        content_parts.push(serde_json::json!({
                            "type": "input_text",
                            "text": sanitize_surrogates(&text_result),
                        }));
                    }
                    for block in &t.content {
                        if let ContentBlock::Image(img) = block {
                            content_parts.push(serde_json::json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!("data:{};base64,{}", img.mime_type, img.data),
                            }));
                        }
                    }
                    serde_json::json!(content_parts)
                } else {
                    serde_json::json!(sanitize_surrogates(
                        if has_text { &text_result } else { "(see attached image)" }
                    ))
                };

                messages.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output_val,
                }));
            }
        }
        msg_index += 1;
    }

    messages
}

// ---------------------------------------------------------------------------
// Tool conversion
// ---------------------------------------------------------------------------

/// Options for tool conversion.
#[derive(Debug, Clone, Default)]
pub struct ConvertResponsesToolsOptions {
    /// Whether to use strict mode for function tools.
    pub strict: Option<bool>,
}

/// Convert tools to the Responses API format.
pub fn convert_responses_tools(
    tools: &[Tool],
    options: Option<ConvertResponsesToolsOptions>,
) -> Vec<JsonValue> {
    let strict = options.and_then(|o| o.strict).unwrap_or(false);

    tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "strict": strict,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Stream processing helpers
// ---------------------------------------------------------------------------

/// Map a Responses API status to a [`crate::StopReason`].
pub fn map_responses_stop_reason(status: Option<&str>) -> crate::StopReason {
    match status {
        Some("completed") => crate::StopReason::Stop,
        Some("incomplete") => crate::StopReason::Length,
        Some("failed") | Some("cancelled") => crate::StopReason::Error,
        Some("in_progress") | Some("queued") => crate::StopReason::Stop,
        _ => crate::StopReason::Stop,
    }
}

// ---------------------------------------------------------------------------
// Unicode sanitization
// ---------------------------------------------------------------------------

/// Replace lone surrogates and invalid Unicode with the replacement character.
pub fn sanitize_surrogates(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\u{FFFD}' || (c.is_control() && c != '\n' && c != '\r' && c != '\t') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Streaming JSON parser
// ---------------------------------------------------------------------------

/// Parse a partial JSON string, returning the best-effort parsed value.
/// Returns an empty JSON object for completely invalid input.
pub fn parse_streaming_json(input: &str) -> JsonValue {
    if input.is_empty() {
        return serde_json::json!({});
    }

    // Try full parse first
    if let Ok(val) = serde_json::from_str::<JsonValue>(input) {
        return val;
    }

    // Try to complete the JSON by closing open brackets/braces
    let mut open_braces = 0i32;
    let mut open_brackets = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for ch in input.chars() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => open_braces += 1,
            '}' => open_braces -= 1,
            '[' => open_brackets += 1,
            ']' => open_brackets -= 1,
            _ => {}
        }
    }

    let mut completed = input.to_string();
    if in_string {
        completed.push('"');
    }
    for _ in 0..open_brackets {
        completed.push(']');
    }
    for _ in 0..open_braces {
        completed.push('}');
    }

    serde_json::from_str::<JsonValue>(&completed).unwrap_or(serde_json::json!({}))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, TextContent};

    #[test]
    fn test_encode_text_signature_v1() {
        let sig = encode_text_signature_v1("msg_123", None);
        assert!(sig.contains("\"v\":1"));
        assert!(sig.contains("\"id\":\"msg_123\""));
    }

    #[test]
    fn test_encode_text_signature_v1_with_phase() {
        let sig = encode_text_signature_v1("msg_456", Some("commentary"));
        assert!(sig.contains("\"phase\":\"commentary\""));
    }

    #[test]
    fn test_parse_text_signature_v1() {
        let sig = encode_text_signature_v1("msg_abc", Some("final_answer"));
        let result = parse_text_signature(Some(&sig));
        assert_eq!(
            result,
            Some(("msg_abc".to_string(), Some("final_answer".to_string())))
        );
    }

    #[test]
    fn test_parse_text_signature_legacy() {
        let result = parse_text_signature(Some("plain_id"));
        assert_eq!(result, Some(("plain_id".to_string(), None)));
    }

    #[test]
    fn test_parse_text_signature_none() {
        assert_eq!(parse_text_signature(None), None);
    }

    #[test]
    fn test_normalize_id_part() {
        assert_eq!(normalize_id_part("abc123"), "abc123");
        assert_eq!(normalize_id_part("a|b|c"), "a_b_c");
        assert_eq!(normalize_id_part(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn test_short_hash_deterministic() {
        let h1 = short_hash("hello");
        let h2 = short_hash("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
    }

    #[test]
    fn test_short_hash_different_inputs() {
        let h1 = short_hash("hello");
        let h2 = short_hash("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_map_responses_stop_reason() {
        assert_eq!(
            map_responses_stop_reason(Some("completed")),
            crate::StopReason::Stop
        );
        assert_eq!(
            map_responses_stop_reason(Some("incomplete")),
            crate::StopReason::Length
        );
        assert_eq!(
            map_responses_stop_reason(Some("failed")),
            crate::StopReason::Error
        );
        assert_eq!(
            map_responses_stop_reason(Some("cancelled")),
            crate::StopReason::Error
        );
        assert_eq!(map_responses_stop_reason(None), crate::StopReason::Stop);
    }

    #[test]
    fn test_sanitize_surrogates() {
        assert_eq!(sanitize_surrogates("hello"), "hello");
        assert_eq!(sanitize_surrogates("hello\nworld"), "hello\nworld");
    }

    #[test]
    fn test_parse_streaming_json_empty() {
        assert_eq!(parse_streaming_json(""), serde_json::json!({}));
    }

    #[test]
    fn test_parse_streaming_json_complete() {
        let result = parse_streaming_json(r#"{"key": "value"}"#);
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn test_parse_streaming_json_partial() {
        let result = parse_streaming_json(r#"{"key": "val"#);
        assert_eq!(result["key"], "val");
    }

    #[test]
    fn test_convert_responses_tools() {
        let tools = vec![Tool {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let result = convert_responses_tools(&tools, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["name"], "get_weather");
        assert_eq!(result[0]["strict"], false);
    }

    #[test]
    fn test_convert_responses_tools_strict() {
        let tools = vec![Tool {
            name: "test".to_string(),
            description: "Test".to_string(),
            parameters: serde_json::json!({}),
        }];
        let opts = ConvertResponsesToolsOptions { strict: Some(true) };
        let result = convert_responses_tools(&tools, Some(opts));
        assert_eq!(result[0]["strict"], true);
    }

    #[test]
    fn test_convert_responses_messages_basic() {
        let model = crate::Model::new(
            "gpt-4o",
            "GPT-4o",
            Api::OpenAiResponses,
            "openai-responses",
            "https://api.openai.com/v1",
        );
        let mut context = Context::new();
        context.add_message(Message::user("Hello"));

        let result = convert_responses_messages(
            &model,
            &context,
            &[],
            Some(ConvertResponsesMessagesOptions {
                include_system_prompt: false,
            }),
        );

        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");
    }

    #[test]
    fn test_convert_responses_messages_with_system_prompt() {
        let model = crate::Model::new(
            "gpt-4o",
            "GPT-4o",
            Api::OpenAiResponses,
            "openai-responses",
            "https://api.openai.com/v1",
        );
        let mut context = Context::new();
        context.set_system_prompt("You are helpful");

        let result = convert_responses_messages(
            &model,
            &context,
            &[],
            Some(ConvertResponsesMessagesOptions {
                include_system_prompt: true,
            }),
        );

        assert!(result.len() >= 1);
        assert_eq!(result[0]["role"], "system");
    }

    #[test]
    fn test_convert_responses_messages_reasoning_model() {
        let mut model = crate::Model::new(
            "o3",
            "o3",
            Api::OpenAiResponses,
            "openai-responses",
            "https://api.openai.com/v1",
        );
        model.reasoning = true;

        let mut context = Context::new();
        context.set_system_prompt("Think carefully");

        let result = convert_responses_messages(
            &model,
            &context,
            &[],
            Some(ConvertResponsesMessagesOptions {
                include_system_prompt: true,
            }),
        );

        assert_eq!(result[0]["role"], "developer");
    }
}
