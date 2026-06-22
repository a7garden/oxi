//! OpenAI-compatible provider implementation

use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::openai_responses_shared::parse_streaming_json;
use super::shared_client;
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Message, MessageContent, Model, Provider,
    ProviderEvent, StopReason, StreamOptions, StreamResult, TextContent, ThinkingContent, ToolCall,
    ToolResultMessage, Usage, error::ProviderError,
};

/// Detect whether a model targets the ZAI provider.
fn is_zai(model: &Model) -> bool {
    model.provider.eq_ignore_ascii_case("zai") || model.base_url.contains("api.z.ai")
}

/// OpenAI-compatible provider
#[derive(Clone)]
pub struct OpenAiProvider {
    client: &'static Client,
    api_key: Option<String>,
    base_url: Option<String>,
    /// Extra HTTP headers to include in every request (e.g. OpenRouter Referer).
    extra_headers: Vec<(String, String)>,
}

impl OpenAiProvider {
    /// Create a new OpenAI provider without an API key.
    ///
    /// API keys are resolved at request time via auth.json or StreamOptions.
    /// Use `with_api_key()` for explicit key injection.
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_key: None,
            base_url: None,
            extra_headers: Vec::new(),
        }
    }

    /// Create with explicit API key (public API for external consumers)
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
            base_url: None,
            extra_headers: Vec::new(),
        }
    }

    /// Create with a custom base URL (API key resolved from auth storage).
    ///
    /// Used for built-in OpenAI-compatible providers like Groq, Cerebras, etc.
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: shared_client(),
            api_key: None,
            base_url: Some(base_url.to_string()),
            extra_headers: Vec::new(),
        }
    }

    /// Create with a custom base URL and optional API key.
    ///
    /// Used for registering custom OpenAI-compatible providers (Minimax, ZAI, etc.).
    pub fn with_base_url_and_key(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            client: shared_client(),
            api_key,
            base_url: Some(base_url.to_string()),
            extra_headers: Vec::new(),
        }
    }

    /// Create with a custom base URL, optional API key, and extra headers.
    ///
    /// Used for providers that require specific HTTP headers (OpenRouter, etc.).
    pub fn with_config(
        base_url: &str,
        api_key: Option<String>,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            client: shared_client(),
            api_key,
            base_url: Some(base_url.to_string()),
            extra_headers,
        }
    }
}

impl Default for OpenAiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for OpenAiProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let options = options.unwrap_or_default();

            // Build the request
            let effective_base_url = self.base_url.as_deref().unwrap_or(&model.base_url);
            let url = format!("{}/chat/completions", effective_base_url);

            // Get API key
            let api_key = options
                .api_key
                .as_ref()
                .or(self.api_key.as_ref())
                .ok_or_else(|| ProviderError::MissingApiKey)?;

            // Build messages (apply provider-specific normalization)
            let normalized = normalize_messages(&context.messages, &model.provider, &model.id);
            let messages = build_messages_from_normalized(&context.system_prompt, &normalized)?;

            // Build request body
            let mut body = serde_json::json!({
                "model": model.id,
                "messages": messages,
                "stream": true,
                "stream_options": { "include_usage": true },
            });

            // Add optional parameters
            if let Some(temp) = options.temperature {
                body["temperature"] = serde_json::json!(temp);
            }

            if let Some(max) = options.max_tokens {
                // Use max_completion_tokens for OpenAI (newer API field).
                // Some providers still use max_tokens; we keep both for compat.
                body["max_completion_tokens"] = serde_json::json!(max);
                body["max_tokens"] = serde_json::json!(max);
            }

            // Add tools if present
            if !context.tools.is_empty() {
                body["tools"] = build_tools(&context.tools)?;
            }

            // ── Reasoning effort (o1/o3/o4 models) ──────────────────────────
            // When thinking_level is set and the model supports reasoning,
            // include `reasoning_effort` in the request body.
            // Also checks provider_options.openai for fine-grained control.
            if model.reasoning {
                let openai_opts = options
                    .provider_options
                    .as_ref()
                    .and_then(|po| po.openai.as_ref());

                let effort = openai_opts
                    .and_then(|o| o.reasoning_effort.clone())
                    .or_else(|| {
                        options
                            .thinking_level
                            .as_ref()
                            .and_then(|l| l.as_str().map(String::from))
                    });

                if let Some(effort_str) = effort {
                    body["reasoning_effort"] = serde_json::json!(effort_str);
                }
            }

            // ── ZAI-specific parameters ──────────────────────────────────
            // Mirror pi's detectCompat: when provider is ZAI (or base_url contains
            // api.z.ai), send enable_thinking and tool_stream.
            if is_zai(model) {
                if model.reasoning {
                    body["enable_thinking"] = serde_json::json!(true);
                }
                if !context.tools.is_empty() {
                    body["tool_stream"] = serde_json::json!(true);
                }
            }

            tracing::info!(
                "Sending request to {} model={} body_len={} enable_thinking={} tool_stream={}",
                url,
                model.id,
                body.to_string().len(),
                body.get("enable_thinking").is_some(),
                body.get("tool_stream").is_some()
            );
            tracing::debug!("Request body: {}", body.to_string());

            // Build headers
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", api_key)
                    .parse()
                    .expect("valid bearer header"),
            );
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                "application/json".parse().expect("valid header value"),
            );

            // Provider-level default headers (e.g. OpenRouter HTTP-Referer)
            for (k, v) in &self.extra_headers {
                if let (Ok(name), Ok(value)) = (
                    k.parse::<reqwest::header::HeaderName>(),
                    v.parse::<reqwest::header::HeaderValue>(),
                ) {
                    headers.insert(name, value);
                }
            }

            // Per-request headers (from StreamOptions)
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

            // Emit Start event once at the beginning of the stream (matches pi's behavior)
            let start_event = ProviderEvent::Start {
                partial: Arc::new(AssistantMessage::new(
                    Api::OpenAiCompletions,
                    &provider_name,
                    &model_id,
                )),
            };

            // Stateful stream parser that accumulates tool calls across chunks.
            // OpenAI sends tool calls as multiple deltas (id, name, arguments fragments)
            // that must be reassembled before emitting ToolCallEnd.
            //
            // State:
            //   pending_bytes     – incomplete UTF-8 bytes from the previous HTTP chunk
            //   pending_tc_index  – accumulated tool calls keyed by streaming index
            //   pending_tc_id     – secondary lookup by tool-call ID (ZAI et al. may
            //                       omit the index on continuation deltas)
            //   thinking_started  – whether ThinkingStart has been emitted
            let stream = response
                .bytes_stream()
                .scan(
                    (
                        Vec::new(),
                        std::collections::HashMap::<usize, (String, String, String)>::new(),
                        std::collections::HashMap::<String, usize>::new(), // id → index
                        false,
                        AssistantMessage::new(Api::OpenAiCompletions, &provider_name, &model_id),
                    ),
                    move |(
                        pending_bytes,
                        pending_tc,
                        tc_id_to_index,
                        thinking_started,
                        accumulated_output,
                    ),
                          chunk: Result<Bytes, reqwest::Error>| {
                        let events = match chunk {
                            Ok(bytes) => {
                                // Prepend any incomplete bytes from previous chunk
                                let mut combined =
                                    Vec::with_capacity(pending_bytes.len() + bytes.len());
                                combined.extend_from_slice(pending_bytes);
                                combined.extend_from_slice(&bytes);

                                // Split into complete lines (ending with \n) and trailing incomplete data.
                                // This prevents JSON parse failures from partial SSE lines
                                // that were split across HTTP chunks.
                                let (text, trailing) = split_complete_lines(&combined);
                                *pending_bytes = trailing;

                                tracing::debug!(
                                    "parse_sse_events input: {} bytes, {} lines",
                                    text.len(),
                                    text.lines().count()
                                );
                                let raw_events = parse_sse_events(
                                    &text,
                                    &provider_name,
                                    &model_id,
                                    accumulated_output,
                                );
                                tracing::debug!("parse_sse_events output: {} events", raw_events.len());

                                // Post-process: accumulate tool call deltas, inject ThinkingStart once
                                let mut processed = Vec::new();
                                for event in raw_events {
                                    match &event {
                                        ProviderEvent::ThinkingDelta { content_index, .. } => {
                                            // Inject ThinkingStart before the first ThinkingDelta
                                            if !*thinking_started {
                                                *thinking_started = true;
                                                processed.push(ProviderEvent::ThinkingStart {
                                                    content_index: *content_index,
                                                    partial: Arc::new(AssistantMessage::new(
                                                        Api::OpenAiCompletions,
                                                        &provider_name,
                                                        &model_id,
                                                    )),
                                                });
                                            }
                                            processed.push(event);
                                        }
                                        ProviderEvent::ToolCallStart {
                                            content_index,
                                            tool_call_id,
                                            tool_name,
                                            ..
                                        } => {
                                            let entry =
                                                pending_tc.entry(*content_index).or_insert_with(|| {
                                                    (String::new(), String::new(), String::new())
                                                });
                                            if let Some(id) = tool_call_id
                                                && !id.is_empty()
                                            {
                                                entry.0 = id.clone();
                                                tc_id_to_index.insert(id.clone(), *content_index);
                                            }
                                            if let Some(name) = tool_name
                                                && !name.is_empty()
                                            {
                                                entry.1 = name.clone();
                                            }
                                            processed.push(event);
                                        }
                                        ProviderEvent::ToolCallDelta {
                                            content_index,
                                            delta,
                                            ..
                                        } => {
                                            // Dual-map lookup: prefer index, fall back to ID
                                            let idx = if pending_tc.contains_key(content_index) {
                                                *content_index
                                            } else {
                                                // Scan id→index map for a match
                                                tc_id_to_index
                                                    .values()
                                                    .copied()
                                                    .find(|i| *i == *content_index)
                                                    .unwrap_or(*content_index)
                                            };
                                            let entry = pending_tc.entry(idx).or_insert_with(|| {
                                                (String::new(), String::new(), String::new())
                                            });
                                            tracing::debug!(
                                                "[TC-DELTA] idx={}, delta_len={}, accumulated_len={}",
                                                idx,
                                                delta.len(),
                                                entry.2.len() + delta.len()
                                            );
                                            entry.2.push_str(delta);
                                            processed.push(event);
                                        }
                                        ProviderEvent::ToolCallEnd { .. } => {
                                            // Already a ToolCallEnd from parse_sse_events
                                            processed.push(event);
                                        }
                                        ProviderEvent::Done { reason, .. } => {
                                            // Before Done, emit ToolCallEnd for all accumulated tool calls
                                            if matches!(reason, StopReason::ToolUse) {
                                                let mut indices: Vec<usize> =
                                                    pending_tc.keys().copied().collect();
                                                indices.sort();
                                                for idx in indices {
                                                    let (id, name, arguments) = &pending_tc[&idx];
                                                    tracing::debug!(
                                                        "[TC-END] idx={}, id={}, name={}, args_len={}",
                                                        idx,
                                                        id.len(),
                                                        name.len(),
                                                        arguments.len()
                                                    );
                                                    let args_value = parse_streaming_json(arguments);
                                                    processed.push(ProviderEvent::ToolCallEnd {
                                                        content_index: idx,
                                                        tool_call: crate::ToolCall {
                                                            content_type:
                                                                crate::messages::ToolCallType::ToolCall,
                                                            id: id.clone(),
                                                            name: name.clone(),
                                                            arguments: args_value,
                                                            thought_signature: None,
                                                        },
                                                        partial: Arc::new(AssistantMessage::new(
                                                            Api::OpenAiCompletions,
                                                            &provider_name,
                                                            &model_id,
                                                        )),
                                                    });
                                                }
                                            }
                                            // Clear pending_tc for the next stream/turn.
                                            // Without this, tool call arguments from the previous
                                            // turn leak into the next turn's accumulation.
                                            pending_tc.clear();
                                            tc_id_to_index.clear();
                                            processed.push(event);
                                        }
                                        _ => {
                                            processed.push(event);
                                        }
                                    }
                                }
                                processed
                            }
                            Err(e) => {
                                vec![ProviderEvent::Error {
                                    reason: StopReason::Error,
                                    error: create_error_message(
                                        &e.to_string(),
                                        &provider_name,
                                        &model_id,
                                    ),
                                }]
                            }
                        };
                        // Return Some to continue, wrap events in an iterator
                        async move { Some(futures::stream::iter(events)) }
                    },
                )
                .flatten();

            // Prepend Start event to the stream
            let stream_with_start = futures::stream::once(async move { start_event }).chain(stream);
            Ok(Box::pin(stream_with_start) as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
        })
    }

    fn name(&self) -> &str {
        "openai"
    }
}

/// Build messages array from normalized Message structs (without Context dependency).
///
/// This function is called after `normalize_messages` has already applied
/// provider-specific transforms. It converts oxi Message types to the
/// JSON format required by the OpenAI Chat API.
fn build_messages_from_normalized(
    system_prompt: &Option<String>,
    messages: &[Message],
) -> Result<Vec<JsonValue>, ProviderError> {
    let mut result = Vec::new();

    // System prompt
    if let Some(prompt) = system_prompt {
        result.push(serde_json::json!({
            "role": "system",
            "content": prompt,
        }));
    }

    // Conversation messages
    for msg in messages {
        match msg {
            Message::User(u) => {
                let content: String = match &u.content {
                    MessageContent::Text(s) => s.clone(),
                    MessageContent::Blocks(blocks) => blocks_to_content(blocks)?.to_string(),
                };
                result.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
            }
            Message::Assistant(a) => {
                // OpenAI format: separate content (text) and tool_calls
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for block in &a.content {
                    match block {
                        ContentBlock::Text(t) => {
                            text_parts.push(t.text.clone());
                        }
                        ContentBlock::Thinking(_) => {
                            // Skip thinking blocks in message history
                        }
                        ContentBlock::ToolCall(tc) => {
                            tool_calls.push(serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments.to_string(),
                                },
                            }));
                        }
                        ContentBlock::Image(_) | ContentBlock::Unknown(_) => {}
                    }
                }

                let mut msg_obj = serde_json::json!({
                    "role": "assistant",
                    "content": text_parts.join(""),
                });
                if !tool_calls.is_empty() {
                    msg_obj["tool_calls"] = serde_json::json!(tool_calls);
                }
                result.push(msg_obj);
            }
            Message::ToolResult(t) => {
                let result_text: String = t
                    .content
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect::<Vec<_>>()
                    .join("");
                result.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": t.tool_call_id,
                    "content": result_text,
                }));
            }
        }
    }

    Ok(result)
}

/// Convert content blocks to a string representation
fn blocks_to_content(blocks: &[ContentBlock]) -> Result<JsonValue, ProviderError> {
    if blocks.len() == 1
        && let Some(text) = blocks[0].as_text()
    {
        return Ok(JsonValue::String(text.to_string()));
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

/// Extract the longest valid UTF-8 prefix from a byte slice.
///
/// Returns the valid string and the trailing bytes that form an incomplete UTF-8
/// sequence. These trailing bytes should be prepended to the next chunk to
/// ensure no characters are lost at HTTP chunk boundaries.
fn find_valid_utf8_prefix(bytes: &[u8]) -> (String, Vec<u8>) {
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), Vec::new()),
        Err(e) => {
            let valid = &bytes[..e.valid_up_to()];
            let trailing = bytes[e.valid_up_to()..].to_vec();
            (String::from_utf8_lossy(valid).to_string(), trailing)
        }
    }
}

/// Split bytes into complete lines (ending with \n) and trailing incomplete data.
/// This ensures `parse_sse_events` only receives complete SSE `data:` lines,
/// preventing JSON parse failures from lines split across HTTP chunks.
pub fn split_complete_lines(bytes: &[u8]) -> (String, Vec<u8>) {
    // Find the last newline — everything up to and including it is complete.
    match bytes.iter().rposition(|&b| b == b'\n') {
        Some(last_nl) => {
            let split_at = last_nl + 1;
            let complete = match std::str::from_utf8(&bytes[..split_at]) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    let (s, _) = find_valid_utf8_prefix(&bytes[..split_at]);
                    s
                }
            };
            let trailing = bytes[split_at..].to_vec();
            (complete, trailing)
        }
        None => {
            // No newline at all — the entire buffer is incomplete.
            // Check if it's valid UTF-8; if not, save as pending.
            (String::new(), bytes.to_vec())
        }
    }
}

/// Parse SSE event stream from a byte buffer.
///
/// Optimizations over a naïve implementation:
/// - **Fast-line splitting** – iterates over `\n` boundaries via `split`
///   instead of allocating an intermediate `String` per line.
/// - **Early `DONE` exit** – breaks immediately when `data: [DONE]` is
///   encountered.
/// - **Pre-allocated events** – reserves capacity based on data-line count.
/// - **Accumulated usage** – tracks usage separately, only cloning into
///   the Done message at stream end, not on every chunk.
fn parse_sse_events(
    text: &str,
    _provider: &str,
    _model_id: &str,
    output: &mut AssistantMessage,
) -> Vec<ProviderEvent> {
    // F-6 (audit 2026-06-21): replace the 2-pass scan (count + parse) with a
    // single length-based estimate. Most SSE `data:` lines are 60–120 bytes,
    // so `text.len() / 80` is a tighter upper bound than the old
    // `count(filter("data: "))` pass and avoids scanning the chunk twice.
    // For very small chunks the estimate is 0 and `Vec::with_capacity(0)`
    // is a no-op. For 64KB chunks this saves one full scan (≈64µs per
    // chunk on a 2024-era CPU at ~1GB/s scan).
    let mut events = Vec::with_capacity(text.len() / 80);

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

        // ── Accumulate usage BEFORE processing choices ────────────────
        // OpenAI with include_usage sends usage in a final chunk with
        // empty choices. By accumulating before the choice loop, the
        // Done event (triggered by finish_reason in an earlier chunk)
        // and any subsequent rendering sees the latest usage.
        if let Some(chunk_usage) = &chunk.usage {
            accumulated_usage.input = chunk_usage.prompt_tokens.max(accumulated_usage.input);
            accumulated_usage.output = chunk_usage.completion_tokens.max(accumulated_usage.output);
            accumulated_usage.cache_read = chunk_usage
                .prompt_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .unwrap_or(0)
                .max(accumulated_usage.cache_read);
            accumulated_usage.total_tokens =
                chunk_usage.total_tokens.max(accumulated_usage.total_tokens);
        }

        for choice in &chunk.choices {
            if let Some(delta) = &choice.delta {
                if let Some(content) = &delta.content {
                    // pi-mono: append to the output's text block
                    let last_text_idx = output
                        .content
                        .iter()
                        .rposition(|b| matches!(b, ContentBlock::Text(_)));
                    if let Some(idx) = last_text_idx
                        && let ContentBlock::Text(t) = &mut output.content[idx]
                    {
                        t.text.push_str(content);
                    } else {
                        output
                            .content
                            .push(ContentBlock::Text(TextContent::new(content.clone())));
                    }
                    events.push(ProviderEvent::TextDelta {
                        content_index: choice.index,
                        delta: content.clone(),
                        partial: Arc::new(output.clone()),
                    });
                }

                // Handle GLM's reasoning_content field (thinking/thought chain)
                if let Some(ref reasoning) = delta.reasoning_content
                    && !reasoning.is_empty()
                {
                    // pi-mono: append to the output's thinking block
                    let last_think_idx = output
                        .content
                        .iter()
                        .rposition(|b| matches!(b, ContentBlock::Thinking(_)));
                    if let Some(idx) = last_think_idx
                        && let ContentBlock::Thinking(t) = &mut output.content[idx]
                    {
                        t.thinking.push_str(reasoning);
                    } else {
                        output
                            .content
                            .push(ContentBlock::Thinking(ThinkingContent::new(
                                reasoning.clone(),
                            )));
                    }
                    events.push(ProviderEvent::ThinkingDelta {
                        content_index: choice.index,
                        delta: reasoning.clone(),
                        partial: Arc::new(output.clone()),
                    });
                }

                if let Some(tool_calls) = &delta.tool_calls {
                    for tc in tool_calls {
                        let tc_index = tc.index.unwrap_or(choice.index);

                        // Emit ToolCallStart when id or name is present (first delta)
                        if tc.id.is_some()
                            || tc.function.as_ref().and_then(|f| f.name.as_ref()).is_some()
                        {
                            events.push(ProviderEvent::ToolCallStart {
                                content_index: tc_index,
                                tool_call_id: tc.id.clone(),
                                tool_name: tc.function.as_ref().and_then(|f| f.name.clone()),
                                partial: Arc::new(output.clone()),
                            });
                        }

                        // Emit ToolCallDelta for arguments
                        if let Some(func) = &tc.function {
                            events.push(ProviderEvent::ToolCallDelta {
                                content_index: tc_index,
                                delta: func.arguments.clone().unwrap_or_default(),
                                partial: Arc::new(output.clone()),
                            });
                        }
                    }
                }
            }

            if choice.finish_reason.is_some() {
                let reason = match choice.finish_reason.as_deref() {
                    Some("stop") | Some("end") => StopReason::Stop,
                    Some("length") => StopReason::Length,
                    Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
                    Some("content_filter") => StopReason::Error,
                    Some(unknown) => {
                        tracing::warn!("Unknown finish_reason: '{}', treating as Error", unknown);
                        StopReason::Error
                    }
                    None => StopReason::Stop,
                };
                tracing::info!("finish_reason={:?} → {:?}", choice.finish_reason, reason);

                let mut done_msg = output.clone();
                done_msg.stop_reason = reason;
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

// SSE chunk structure
#[derive(Debug, Deserialize)]
// serde deserialization structs
struct SSEChunk {
    _id: Option<String>,
    #[serde(rename = "model")]
    _model: Option<String>,
    choices: Vec<Choice>,
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
struct Choice {
    index: usize,
    delta: Option<Delta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
struct ToolCallDelta {
    index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    _type_: Option<String>,
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
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

// ============================================================================
// Provider-agnostic Message Normalization
//
// Mirrors opencode's transform.ts normalizeMessages() — these transforms
// sanitize message content before sending to the API:
//   - Empty content filtering (Anthropic rejects empty messages/parts)
//   - Tool ID scrubbing (Mistral requires 9-char, Claude needs alphanum)
//   - DeepSeek reasoning injection (empty reasoning parts required)
//   - Anthropic tool-use ordering (tool_use must not precede non-tool content)
// ============================================================================

/// Normalize messages for a specific provider.
///
/// This applies provider-specific transforms that the API requires:
///   - Anthropic: filter empty text/reasoning parts, add cache_control
///   - Claude (via Anthropic): scrub tool IDs to alphanumeric + underscore
///   - Mistral: truncate tool IDs to 9 alphanumeric chars
///   - DeepSeek: inject empty reasoning parts into assistant messages
///   - Anthropic/Vertex: reorder tool_use before text parts
pub fn normalize_messages(messages: &[Message], provider: &str, model_id: &str) -> Vec<Message> {
    let provider_lower = provider.to_lowercase();
    let model_lower = model_id.to_lowercase();

    let is_anthropic = provider_lower == "anthropic"
        || provider_lower.contains("vertex-anthropic")
        || provider_lower.contains("google-vertex") && model_lower.contains("claude");

    let is_mistral = provider_lower == "mistral"
        || model_lower.contains("mistral")
        || model_lower.contains("devstral");

    let is_deepseek = model_lower.contains("deepseek");

    let is_claude = model_lower.contains("claude");

    let needs_tool_id_scrub = is_mistral || is_claude;

    let messages: Vec<Message> = messages.to_vec();

    // 1. DeepSeek: inject empty reasoning parts into assistant messages
    let messages = if is_deepseek {
        messages
            .iter()
            .map(|msg| match msg {
                Message::Assistant(a) => {
                    let has_reasoning = a
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::Thinking(_)));
                    if has_reasoning {
                        Message::Assistant(a.clone())
                    } else {
                        let mut new_content = a.content.clone();
                        new_content
                            .push(ContentBlock::Thinking(ThinkingContent::new(String::new())));
                        let mut new_msg = AssistantMessage::new(a.api, &a.provider, &a.model);
                        new_msg.content = new_content;
                        new_msg.usage = a.usage.clone();
                        new_msg.stop_reason = a.stop_reason;
                        new_msg.response_id = a.response_id.clone();
                        new_msg.timestamp = a.timestamp;
                        Message::Assistant(new_msg)
                    }
                }
                _ => msg.clone(),
            })
            .collect()
    } else {
        messages
    };

    // 2. Anthropic: filter empty text/reasoning parts, remove empty messages
    let messages = if is_anthropic {
        messages
            .iter()
            .filter_map(|msg| match msg {
                Message::User(u) => {
                    let filtered = filter_empty_content(&u.content);
                    filtered.as_ref()?;
                    Some(Message::User(crate::UserMessage {
                        role: u.role,
                        content: filtered.expect("checked above"),
                        timestamp: u.timestamp,
                    }))
                }
                Message::Assistant(a) => {
                    let filtered: Vec<ContentBlock> = a
                        .content
                        .iter()
                        .filter(|b| !is_empty_content_block(b))
                        .cloned()
                        .collect();
                    if filtered.is_empty() {
                        return None; // Entire message is empty
                    }
                    let mut new_msg = AssistantMessage::new(a.api, &a.provider, &a.model);
                    new_msg.content = filtered;
                    new_msg.usage = a.usage.clone();
                    new_msg.stop_reason = a.stop_reason;
                    new_msg.response_id = a.response_id.clone();
                    new_msg.timestamp = a.timestamp;
                    Some(Message::Assistant(new_msg))
                }
                Message::ToolResult(t) => Some(Message::ToolResult(t.clone())),
            })
            .collect()
    } else {
        messages
    };

    // 3. Anthropic/Vertex: reorder tool_use before non-tool content
    let messages = if is_anthropic {
        messages
            .iter()
            .flat_map(|msg| match msg {
                Message::Assistant(a) => {
                    let parts = &a.content;
                    let has_tool = parts.iter().any(|b| matches!(b, ContentBlock::ToolCall(_)));
                    let has_non_tool = parts
                        .iter()
                        .any(|b| !matches!(b, ContentBlock::ToolCall(_)));

                    if has_tool && has_non_tool {
                        // Split into [non-tool parts] + [tool parts]
                        let non_tools: Vec<ContentBlock> = parts
                            .iter()
                            .filter(|b| !matches!(b, ContentBlock::ToolCall(_)))
                            .cloned()
                            .collect();
                        let tools: Vec<ContentBlock> = parts
                            .iter()
                            .filter(|b| matches!(b, ContentBlock::ToolCall(_)))
                            .cloned()
                            .collect();

                        let mut msg1 = AssistantMessage::new(a.api, &a.provider, &a.model);
                        msg1.content = non_tools;
                        msg1.usage = a.usage.clone();
                        msg1.stop_reason = a.stop_reason;
                        msg1.response_id = a.response_id.clone();
                        msg1.timestamp = a.timestamp;

                        let mut msg2 = AssistantMessage::new(a.api, &a.provider, &a.model);
                        msg2.content = tools;
                        msg2.usage = a.usage.clone();
                        msg2.stop_reason = a.stop_reason;
                        msg2.response_id = a.response_id.clone();
                        msg2.timestamp = a.timestamp;

                        vec![Message::Assistant(msg1), Message::Assistant(msg2)]
                    } else {
                        vec![Message::Assistant(a.clone())]
                    }
                }
                _ => vec![msg.clone()],
            })
            .collect()
    } else {
        messages
    };

    // 4. Tool ID scrubbing (Mistral: 9 chars, Claude: alphanumeric + underscore)

    if needs_tool_id_scrub {
        messages
            .iter()
            .map(|msg| match msg {
                Message::Assistant(a) => {
                    let new_content: Vec<ContentBlock> = a
                        .content
                        .iter()
                        .map(|block| match block {
                            ContentBlock::ToolCall(tc) => ContentBlock::ToolCall(ToolCall::new(
                                scrub_tool_id(&tc.id, is_mistral, is_anthropic),
                                tc.name.clone(),
                                tc.arguments.clone(),
                            )),
                            _ => block.clone(),
                        })
                        .collect();
                    let mut new_msg = AssistantMessage::new(a.api, &a.provider, &a.model);
                    new_msg.content = new_content;
                    new_msg.usage = a.usage.clone();
                    new_msg.stop_reason = a.stop_reason;
                    new_msg.response_id = a.response_id.clone();
                    new_msg.timestamp = a.timestamp;
                    Message::Assistant(new_msg)
                }
                Message::ToolResult(t) => Message::ToolResult(ToolResultMessage::new(
                    scrub_tool_id(&t.tool_call_id, is_mistral, is_anthropic),
                    &t.tool_name,
                    t.content.clone(),
                )),
                _ => msg.clone(),
            })
            .collect()
    } else {
        messages
    }
}

/// Check if a content block is effectively empty.
fn is_empty_content_block(block: &ContentBlock) -> bool {
    match block {
        ContentBlock::Text(t) => t.text.trim().is_empty(),
        ContentBlock::Thinking(t) => t.thinking.trim().is_empty(),
        _ => false,
    }
}

/// Filter empty parts from MessageContent.
/// Returns None if the entire content becomes empty.
fn filter_empty_content(content: &MessageContent) -> Option<MessageContent> {
    match content {
        MessageContent::Text(s) => {
            if s.trim().is_empty() {
                None
            } else {
                Some(MessageContent::Text(s.clone()))
            }
        }
        MessageContent::Blocks(blocks) => {
            let filtered: Vec<ContentBlock> = blocks
                .iter()
                .filter(|b| !is_empty_content_block(b))
                .cloned()
                .collect();
            if filtered.is_empty() {
                None
            } else {
                Some(MessageContent::Blocks(filtered))
            }
        }
    }
}

/// Scrub a tool call ID for provider compatibility.
/// Mistral: keep first 9 alphanumeric chars, pad with zeros
/// Claude: keep only alphanumeric + underscore
fn scrub_tool_id(id: &str, is_mistral: bool, is_anthropic: bool) -> String {
    if is_mistral {
        let alphanumeric: String = id.chars().filter(|c| c.is_alphanumeric()).take(9).collect();
        if alphanumeric.len() < 9 {
            format!("{}{}", alphanumeric, "0".repeat(9 - alphanumeric.len()))
        } else {
            alphanumeric
        }
    } else if is_anthropic {
        // Anthropic requires tool call IDs matching [a-zA-Z0-9_-]{1,64}.
        // pi: `id.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64)`
        id.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(64)
            .collect()
    } else {
        id.chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: &str = "openai";
    const MODEL: &str = "gpt-4o";

    fn parse_sse(sse: &str) -> Vec<ProviderEvent> {
        let mut output = AssistantMessage::new(Api::OpenAiCompletions, PROVIDER, MODEL);
        parse_sse_events(sse, PROVIDER, MODEL, &mut output)
    }

    // ── SSE event parsing ──────────────────────────────────────────────

    #[test]
    fn parse_single_text_event() {
        let sse = "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n";
        let events = parse_sse(sse);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta {
                delta,
                content_index,
                ..
            } => {
                assert_eq!(delta, "Hello");
                assert_eq!(*content_index, 0);
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_multiple_text_events() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo!\"}}]}\n",
            "\n"
        );
        let events = parse_sse(sse);
        assert_eq!(events.len(), 2);
        let texts: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Hel", "lo!"]);
    }

    #[test]
    fn parse_done_terminator() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"X\"}}]}\n",
            "\n",
            "data: [DONE]\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"NEVER\"}}]}\n"
        );
        let events = parse_sse(sse);
        // Should stop at [DONE]; the final data line is never parsed
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "X"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    // ── Content extraction ─────────────────────────────────────────────

    #[test]
    fn parse_finish_reason_stop() {
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"stop\"}]}\n\n";
        let events = parse_sse(sse);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Stop)),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_finish_reason_length() {
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"length\"}]}\n\n";
        let events = parse_sse(sse);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Length)),
            other => panic!("expected Done with Length, got {other:?}"),
        }
    }

    #[test]
    fn parse_finish_reason_tool_calls() {
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"tool_calls\"}]}\n\n";
        let events = parse_sse(sse);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::ToolUse)),
            other => panic!("expected Done with ToolUse, got {other:?}"),
        }
    }

    // ── Tool call delta accumulation ───────────────────────────────────

    #[test]
    fn parse_tool_call_deltas() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\\\"SF\\\"}\"}}]}}]}\n",
            "\n"
        );
        let events = parse_sse(sse);
        // First chunk: ToolCallStart (id+name present) + ToolCallDelta (function present)
        // Second chunk: ToolCallDelta only
        assert_eq!(events.len(), 3);
        let starts: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::ToolCallStart { tool_name, .. } => tool_name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec!["get_weather"]);
        let deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["", "{\"city\":\"SF\"}"]);
    }

    #[test]
    fn parse_tool_call_with_no_arguments_field() {
        // function field present but arguments is null → emits ToolCallStart + ToolCallDelta
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"run\"}}]}}]}\n\n";
        let events = parse_sse(sse);
        assert_eq!(events.len(), 2);
        match &events[0] {
            ProviderEvent::ToolCallStart { tool_name, .. } => {
                assert_eq!(tool_name.as_deref(), Some("run"));
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
        match &events[1] {
            ProviderEvent::ToolCallDelta { delta, .. } => assert_eq!(delta, ""),
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
    }

    // ── Usage accumulation ─────────────────────────────────────────────

    #[test]
    fn parse_usage_in_chunk() {
        // Usage is accumulated from earlier chunks; the Done event captures
        // usage that was accumulated *before* the finish_reason chunk.
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":8,\"total_tokens\":18,\"prompt_tokens_details\":{\"cached_tokens\":3}}}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"stop\"}]}\n"
        );
        let events = parse_sse(sse);
        // TextDelta + Done
        assert_eq!(events.len(), 2);
        match &events[1] {
            ProviderEvent::Done { message, .. } => {
                assert_eq!(message.usage.input, 10);
                assert_eq!(message.usage.output, 8);
                assert_eq!(message.usage.total_tokens, 18);
                assert_eq!(message.usage.cache_read, 3);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_usage_without_cache_details() {
        // Usage from an earlier chunk; Done event on a separate chunk without usage.
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"stop\"}]}\n"
        );
        let events = parse_sse(sse);
        match &events[0] {
            ProviderEvent::Done { message, .. } => {
                assert_eq!(message.usage.input, 5);
                assert_eq!(message.usage.output, 2);
                assert_eq!(message.usage.cache_read, 0);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ── Empty / malformed handling ─────────────────────────────────────

    #[test]
    fn parse_empty_input() {
        let events = parse_sse("");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_only_empty_lines() {
        let events = parse_sse("\n\n\n");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_malformed_json_after_data() {
        let sse = "data: {not json at all}\ndata: also bad\ndata: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"}}]}\n";
        let events = parse_sse(sse);
        // Malformed lines are skipped, only the valid one emits
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "ok"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_data_line() {
        let sse = "data: \ndata: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"X\"}}]}\n";
        let events = parse_sse(sse);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_non_data_lines_ignored() {
        let sse = "event: ping\nid: 42\nretry: 5000\ndata: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Y\"}}]}\n";
        let events = parse_sse(sse);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_carriage_return_line_endings() {
        let sse = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"CR\"}}]}\r\n\r\n";
        let events = parse_sse(sse);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "CR"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    // ── Mixed content + tool + done ────────────────────────────────────

    #[test]
    fn parse_full_stream_with_text_tool_and_done() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Let me\"}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" check\"}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"search\",\"arguments\":\"{\\\"q\\\":\\\"rust\\\"}\"}}]}}]}\n",
            "\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"tool_calls\"}]}\n",
            "\n",
            "data: [DONE]\n"
        );
        let events = parse_sse(sse);
        assert_eq!(events.len(), 5); // 2 TextDelta + ToolCallStart + ToolCallDelta + Done

        let mut text_count = 0;
        let mut tc_start_count = 0;
        let mut tc_delta_count = 0;
        let mut done_count = 0;
        for e in &events {
            match e {
                ProviderEvent::TextDelta { .. } => text_count += 1,
                ProviderEvent::ToolCallStart { .. } => tc_start_count += 1,
                ProviderEvent::ToolCallDelta { .. } => tc_delta_count += 1,
                ProviderEvent::Done { reason, .. } => {
                    done_count += 1;
                    assert!(matches!(reason, StopReason::ToolUse));
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(text_count, 2);
        assert_eq!(tc_start_count, 1);
        assert_eq!(tc_delta_count, 1);
        assert_eq!(done_count, 1);
    }
}
