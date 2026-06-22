//! Anthropic provider implementation

use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::openai::split_complete_lines;
use super::openai_responses_shared::parse_streaming_json;
use super::shared_client;
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Model, Provider, ProviderEvent, StopReason,
    StreamOptions, StreamResult, TextContent, ThinkingContent, ToolCall, Usage,
    error::ProviderError,
};

/// Anthropic provider
///
/// Supports the Anthropic Messages API (v1/messages) and Anthropic-compatible
/// providers (MiniMax, etc.) via custom base URLs and extra headers.
#[derive(Clone)]
pub struct AnthropicProvider {
    client: &'static Client,
    api_key: Option<String>,
    /// Override base URL. When `None`, falls back to `model.base_url`.
    base_url: Option<String>,
    /// Extra HTTP headers to include in every request (e.g. anthropic-version,
    /// anthropic-beta).
    extra_headers: Vec<(String, String)>,
    /// Whether this is the native Anthropic API endpoint.
    /// When `false` (compatible providers like MiniMax), Anthropic-specific
    /// features such as the `thinking` parameter and beta headers are
    /// suppressed because they may not be supported.
    native: bool,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider without an API key.
    ///
    /// API keys are resolved at request time via auth.json or StreamOptions.
    /// Use `with_api_key()` for explicit key injection.
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_key: None,
            base_url: None,
            extra_headers: vec![
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
                // Enable interleaved thinking and fine-grained tool streaming
                // (matches opencode's anthropic custom loader defaults)
                (
                    "anthropic-beta".to_string(),
                    "interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14"
                        .to_string(),
                ),
            ],
            native: true,
        }
    }

    /// Create with an explicit API key.
    #[allow(dead_code)]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
            base_url: None,
            extra_headers: vec![
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
                (
                    "anthropic-beta".to_string(),
                    "interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14"
                        .to_string(),
                ),
            ],
            native: true,
        }
    }

    /// Create with a custom base URL (for Anthropic-compatible providers like
    /// MiniMax that expose the Messages API at a different host).
    ///
    /// Note: Anthropic-specific beta headers are NOT included because
    /// third-party compatible providers may not support them.
    #[allow(dead_code)]
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: shared_client(),
            api_key: None,
            base_url: Some(base_url.to_string()),
            extra_headers: vec![("anthropic-version".to_string(), "2023-06-01".to_string())],
            native: false,
        }
    }

    /// Create with a custom base URL, API key, and extra headers.
    ///
    /// Used for registering Anthropic-compatible providers (MiniMax, etc.).
    ///
    /// Note: Anthropic-specific beta headers (`interleaved-thinking`,
    /// `fine-grained-tool-streaming`) are NOT included here because
    /// third-party compatible providers may not support them, leading
    /// to stream hangs or protocol errors. Use [`Self::new`] or
    /// [`Self::with_api_key`] for the native Anthropic endpoint which includes
    /// beta headers.
    pub fn with_config(
        base_url: &str,
        api_key: Option<String>,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        let mut headers = vec![("anthropic-version".to_string(), "2023-06-01".to_string())];
        headers.extend(extra_headers);
        Self {
            client: shared_client(),
            api_key,
            base_url: Some(base_url.to_string()),
            extra_headers: headers,
            native: false,
        }
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for AnthropicProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let options = options.unwrap_or_default();

            // Build the request – use provider base_url override, fall back to model.base_url
            let effective_base_url = self.base_url.as_deref().unwrap_or(&model.base_url);
            let url = format!("{}/v1/messages", effective_base_url);

            // Get API key
            let api_key = options
                .api_key
                .as_ref()
                .or(self.api_key.as_ref())
                .ok_or_else(|| ProviderError::MissingApiKey)?;

            // Build messages (apply provider-specific normalization for
            // Anthropic: filter empty content, reorder tool_use blocks)
            let normalized = crate::providers::openai::normalize_messages(
                &context.messages,
                &model.provider,
                &model.id,
            );
            let messages =
                build_anthropic_messages_from_normalized(&context.system_prompt, &normalized)?;

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
            // Temperature is incompatible with extended thinking (adaptive or
            // budget-based). Only send when thinking is not active.
            // Matches pi: `if (options?.temperature !== undefined && !options?.thinkingEnabled)`
            let thinking_active = body.get("thinking").is_some();
            if let Some(temp) = options.temperature
                && !thinking_active
            {
                body["temperature"] = serde_json::json!(temp);
            }

            if let Some(max) = options.max_tokens {
                body["max_tokens"] = serde_json::json!(max);
            }

            // Add tools if present
            if !context.tools.is_empty() {
                body["tools"] = build_anthropic_tools(&context.tools)?;
            }

            // ── Thinking / Extended Reasoning ──────────────────────────────
            // Supports two modes via provider_options.anthropic:
            //   1. "enabled" with explicit budget_tokens (from thinking_level or custom)
            //   2. "adaptive" with effort level (Anthropic dynamically allocates budget)
            //
            // Falls back to thinking_level-based budget when provider_options is absent.
            //
            // IMPORTANT: Only send the `thinking` parameter when connected to the
            // native Anthropic API (`self.native == true`). Third-party compatible
            // providers (MiniMax, etc.) may generate thinking content natively but
            // don't support the explicit `thinking` request parameter — sending it
            // can cause incomplete responses or prevent tool calls.
            if model.reasoning && self.native {
                // Check provider_options first for fine-grained control
                let anthropic_opts = options
                    .provider_options
                    .as_ref()
                    .and_then(|po| po.anthropic.as_ref());

                if let Some(opts) = anthropic_opts {
                    // Provider-level override
                    match opts.thinking_type.as_deref() {
                        Some("adaptive") => {
                            // Adaptive thinking — Anthropic chooses budget
                            body["thinking"] = serde_json::json!({
                                "type": "adaptive",
                            });
                            if let Some(ref effort) = opts.effort {
                                // effort is not a body param but could influence
                                // max_tokens allocation
                                let budget = match effort.as_str() {
                                    "max" => model.max_tokens.min(31999),
                                    "xhigh" => (model.max_tokens * 4 / 5).min(31999),
                                    "high" => (model.max_tokens / 2).min(31999),
                                    "medium" => (model.max_tokens / 4).min(16000),
                                    "low" => (model.max_tokens / 8).min(8000),
                                    _ => (model.max_tokens / 4).min(16000),
                                };
                                if body.get("max_tokens").is_none() {
                                    body["max_tokens"] =
                                        serde_json::json!((budget + 1024).min(model.max_tokens));
                                }
                            }
                        }
                        Some("enabled") => {
                            // Explicit budget from provider_options or thinking_level
                            let budget = opts.thinking_budget.unwrap_or_else(|| {
                                compute_thinking_budget(&options.thinking_level, model.max_tokens)
                            });
                            if budget > 0 {
                                if body.get("max_tokens").is_none() {
                                    body["max_tokens"] =
                                        serde_json::json!((budget + 1024).min(model.max_tokens));
                                }
                                body["thinking"] = serde_json::json!({
                                    "type": "enabled",
                                    "budget_tokens": budget,
                                });
                            }
                        }
                        _ => {
                            // No explicit thinking_type — use thinking_level fallback
                            let budget =
                                compute_thinking_budget(&options.thinking_level, model.max_tokens);
                            if budget > 0 {
                                if body.get("max_tokens").is_none() {
                                    body["max_tokens"] =
                                        serde_json::json!((budget + 1024).min(model.max_tokens));
                                }
                                body["thinking"] = serde_json::json!({
                                    "type": "enabled",
                                    "budget_tokens": budget,
                                });
                            }
                        }
                    }
                } else if let Some(ref level) = options.thinking_level {
                    // No provider_options — use thinking_level directly
                    let budget = compute_thinking_budget(&Some(*level), model.max_tokens);
                    if budget > 0 {
                        if body.get("max_tokens").is_none() {
                            body["max_tokens"] =
                                serde_json::json!((budget + 1024).min(model.max_tokens));
                        }
                        body["thinking"] = serde_json::json!({
                            "type": "enabled",
                            "budget_tokens": budget,
                        });
                    }
                }
            }

            // Ensure max_tokens is always set (Anthropic requires it)
            // For reasoning models, ensure max_tokens is large enough for
            // the model to think AND respond. opencode uses OUTPUT_TOKEN_MAX = 32_000
            // for MiniMax and other reasoning models.
            //
            // When the caller sets a small max_tokens (e.g., 4096 default),
            // reasoning models like MiniMax may think for thousands of tokens
            // and then stop before generating tool calls because they calculate
            // they won't have enough room. Bumping to a minimum of 16_384 for
            // reasoning models ensures the model has space to think + call tools.
            if model.reasoning
                && let Some(current) = body.get("max_tokens").and_then(|v| v.as_u64())
                && current < 16_384
            {
                body["max_tokens"] = serde_json::json!(model.max_tokens.min(32_768));
            }
            if body.get("max_tokens").is_none() {
                body["max_tokens"] = serde_json::json!(model.max_tokens.min(16384));
            }

            // ── Cache Control ─────────────────────────────────────────────
            // When cache_retention is set, add cache_control breakpoints to
            // the system prompt and last few messages.
            //
            // Anthropic allows at most 4 cache_control breakpoints per request.
            // We use a counter to enforce this limit, allocating in priority
            // order: system prompt → last message → second-to-last message.
            // Mirrors opencode's Cache.Breakpoints with ANTHROPIC_BREAKPOINT_CAP.
            const ANTHROPIC_BREAKPOINT_CAP: usize = 4;
            let want_cache = options.cache_retention == Some(crate::CacheRetention::Short)
                || options.cache_retention == Some(crate::CacheRetention::Long);

            if want_cache {
                let mut remaining = ANTHROPIC_BREAKPOINT_CAP;
                let cache_marker = serde_json::json!({ "type": "ephemeral" });

                // 1. System prompt (highest priority)
                if remaining > 0
                    && let Some(system) = body.get_mut("system")
                    && system.is_string()
                {
                    *system = serde_json::json!([{
                        "type": "text",
                        "text": system,
                        "cache_control": cache_marker.clone(),
                    }]);
                    remaining -= 1;
                }

                // 2. Last message (high priority)
                if remaining > 0
                    && let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut())
                {
                    if let Some(last_msg) = messages.last_mut()
                        && let Some(content) = last_msg.get_mut("content")
                    {
                        if let Some(parts) = content.as_array_mut() {
                            if let Some(last_part) = parts.last_mut() {
                                last_part["cache_control"] = cache_marker.clone();
                                remaining -= 1;
                            }
                        } else if content.is_string() {
                            let text = content.take();
                            *content = serde_json::json!([{
                                "type": "text",
                                "text": text,
                                "cache_control": cache_marker.clone(),
                            }]);
                            remaining -= 1;
                        }
                    }

                    // 3. Second-to-last message (tool results)
                    if remaining > 0 {
                        let msg_count = messages.len();
                        if msg_count >= 3
                            && let Some(msg) = messages.get_mut(msg_count - 3)
                            && let Some(content) = msg.get_mut("content")
                            && let Some(parts) = content.as_array_mut()
                            && let Some(last_part) = parts.last_mut()
                        {
                            last_part["cache_control"] = cache_marker;
                            remaining -= 1;
                        }
                    }
                }

                if remaining < ANTHROPIC_BREAKPOINT_CAP {
                    tracing::debug!(
                        used = ANTHROPIC_BREAKPOINT_CAP - remaining,
                        cap = ANTHROPIC_BREAKPOINT_CAP,
                        "Anthropic cache breakpoints applied"
                    );
                }
            }

            // Build headers
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("x-api-key", api_key.parse().expect("valid header value"));
            headers.insert(
                "content-type",
                "application/json".parse().expect("valid header value"),
            );

            // Provider-level default headers (e.g. anthropic-version, anthropic-beta)
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
            let model_name = model.id.clone();

            // Stateful scan: persists partial_message and usage ACROSS chunks.
            //
            // Previous implementation created a fresh partial_message per chunk,
            // which caused content loss at chunk boundaries. When an HTTP response
            // is split into multiple chunks, content_block_delta events from
            // earlier chunks would be lost because the new chunk's partial_message
            // started empty. This particularly affected compatible providers
            // (MiniMax, etc.) that split responses across many small chunks.
            //
            // pi (TypeScript) avoids this by keeping a single `output` object
            // across the entire stream. We replicate that pattern here by
            // including partial_message in the scan state.
            //
            // Tool calls are tracked in `pending_tool_calls` so that partial JSON
            // arguments are accumulated across `input_json_delta` events and
            // finalized when `content_block_stop` fires.

            struct AnthropicScanState {
                pending_bytes: Vec<u8>,
                partial: AssistantMessage,
                usage: Usage,
                /// In-flight tool calls keyed by content block index.
                pending_tool_calls: std::collections::HashMap<usize, AnthropicPendingToolCall>,
            }

            let initial_state = AnthropicScanState {
                pending_bytes: Vec::new(),
                partial: AssistantMessage::new(Api::AnthropicMessages, "anthropic", &model_name),
                usage: Usage::default(),
                pending_tool_calls: std::collections::HashMap::new(),
            };

            let stream = response
                .bytes_stream()
                .scan(
                    initial_state,
                    move |state, chunk: Result<bytes::Bytes, reqwest::Error>| {
                        let events = match chunk {
                            Ok(bytes) => {
                                let mut combined =
                                    Vec::with_capacity(state.pending_bytes.len() + bytes.len());
                                combined.extend_from_slice(&state.pending_bytes);
                                combined.extend_from_slice(&bytes);
                                let (text, trailing) = split_complete_lines(&combined);
                                state.pending_bytes = trailing;
                                parse_anthropic_events_stateful(
                                    &text,
                                    &mut state.partial,
                                    &mut state.usage,
                                    &mut state.pending_tool_calls,
                                )
                            }
                            Err(e) => vec![ProviderEvent::Error {
                                reason: StopReason::Error,
                                error: create_error_message(&e.to_string()),
                            }],
                        };
                        async move { Some(futures::stream::iter(events)) }
                    },
                )
                .flatten();

            Ok(Box::pin(stream) as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
        })
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

/// Build messages in Anthropic format from normalized Message structs.
///
/// Key behavior for Anthropic compatibility:
/// - Consecutive `ToolResult` messages are merged into a single `user` message
///   with multiple `tool_result` content blocks (Anthropic API requirement).
///   Matches pi's look-ahead pattern.
fn build_anthropic_messages_from_normalized(
    _system_prompt: &Option<String>,
    messages_in: &[crate::Message],
) -> Result<Vec<JsonValue>, ProviderError> {
    let mut messages: Vec<JsonValue> = Vec::new();
    let mut i = 0;

    while i < messages_in.len() {
        let msg = &messages_in[i];
        match msg {
            crate::Message::User(u) => {
                let content = match &u.content {
                    crate::MessageContent::Text(s) => vec![serde_json::json!({
                        "type": "text",
                        "text": s,
                    })],
                    crate::MessageContent::Blocks(blocks) => blocks_to_anthropic_content(blocks)?,
                };
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
                i += 1;
            }
            crate::Message::Assistant(a) => {
                let content = blocks_to_anthropic_content(&a.content)?;
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }));
                i += 1;
            }
            crate::Message::ToolResult(t) => {
                // Anthropic requires consecutive tool_results to be grouped
                // into a single user message (pi's look-ahead pattern).
                let mut tool_results: Vec<JsonValue> = Vec::new();
                let content = blocks_to_anthropic_content(&t.content)?;
                tool_results.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": t.tool_call_id,
                    "content": content,
                }));

                // Look ahead for consecutive ToolResult messages
                let mut j = i + 1;
                while j < messages_in.len() {
                    if let crate::Message::ToolResult(next_t) = &messages_in[j] {
                        let next_content = blocks_to_anthropic_content(&next_t.content)?;
                        tool_results.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": next_t.tool_call_id,
                            "content": next_content,
                        }));
                        j += 1;
                    } else {
                        break;
                    }
                }

                messages.push(serde_json::json!({
                    "role": "user",
                    "content": tool_results,
                }));
                i = j;
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

/// Compute thinking budget from ThinkingLevel enum + model max_tokens.
fn compute_thinking_budget(level: &Option<crate::ThinkingLevel>, max_tokens: usize) -> usize {
    match level {
        Some(crate::ThinkingLevel::High) | Some(crate::ThinkingLevel::XHigh) => {
            (max_tokens / 2).min(31999)
        }
        Some(crate::ThinkingLevel::Medium) => (max_tokens / 4).min(16000),
        Some(crate::ThinkingLevel::Low) => (max_tokens / 8).min(8000),
        Some(crate::ThinkingLevel::Minimal) => (max_tokens / 16).min(4000),
        _ => 0,
    }
}

fn build_anthropic_tools(tools: &[crate::Tool]) -> Result<JsonValue, ProviderError> {
    let items: Vec<_> = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.parameters,
            })
        })
        .collect();

    Ok(serde_json::json!(items))
}

/// Parse Anthropic SSE event stream (stateful across chunks).
///
/// Unlike the old `parse_anthropic_events` which created a fresh `partial_message`
/// per invocation, this version takes `&mut AssistantMessage` and `&mut Usage`
/// so content accumulates correctly across HTTP chunk boundaries.
///
/// Tool calls are tracked in `pending_tool_calls` so that partial JSON arguments
/// are accumulated across `input_json_delta` events and finalized when
/// Tracks a pending tool call being assembled from Anthropic streaming events.
///
/// Anthropic streams tool calls as three SSE events:
///   1. `content_block_start` → id, name (arguments empty)
///   2. `content_block_delta` (input_json_delta) → partial JSON fragments
///   3. `content_block_stop` → complete
///
/// We accumulate the partial JSON in `partial_json` and build the final
/// `ToolCall` when the block completes.
struct AnthropicPendingToolCall {
    /// Tool call ID from `content_block_start`.
    id: String,
    /// Tool name from `content_block_start`.
    name: String,
    /// Accumulated partial JSON arguments from `input_json_delta` events.
    partial_json: String,
}

/// Parse Anthropic SSE event stream (stateful across chunks).
///
/// Unlike the old `parse_anthropic_events` which created a fresh `partial_message`
/// per invocation, this version takes `&mut AssistantMessage` and `&mut Usage`
/// so content accumulates correctly across HTTP chunk boundaries.
///
/// Tool calls are tracked in `pending_tool_calls` so that partial JSON arguments
/// are accumulated across `input_json_delta` events and finalized when
/// `content_block_stop` fires (matching pi's `content_block_stop → toolcall_end`
/// pattern). On `content_block_start` (tool_use), a placeholder `ToolCall` is
/// added to `partial_message.content` so that downstream consumers (streaming.rs)
/// can see tool calls in the partial snapshot immediately.
fn parse_anthropic_events_stateful(
    text: &str,
    partial_message: &mut AssistantMessage,
    accumulated_usage: &mut Usage,
    pending_tool_calls: &mut std::collections::HashMap<usize, AnthropicPendingToolCall>,
) -> Vec<ProviderEvent> {
    // F-6 (audit 2026-06-21): length-based estimate replaces 2-pass
    // count-then-parse scan. See oxi-ai/src/providers/openai.rs for the
    // rationale (same heuristic applied uniformly across providers).
    let mut events = Vec::with_capacity(text.len() / 80);

    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        if !line.starts_with("data: ") {
            continue;
        }

        let data = &line[6..];

        if data == "[DONE]" || data.is_empty() {
            continue;
        }

        let event = match serde_json::from_str::<AnthropicEvent>(data) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let event_type = event.type_.as_deref();

        // ── Accumulate usage BEFORE the match statement ──────────────────
        // Use max() to avoid overwriting values from earlier events with 0
        // when a later event only includes a subset of fields.
        if let Some(usage) = &event.usage {
            accumulated_usage.input = usage.input_tokens.max(accumulated_usage.input);
            accumulated_usage.output = usage.output_tokens.max(accumulated_usage.output);
            accumulated_usage.cache_read = usage.cache_read.max(accumulated_usage.cache_read);
            accumulated_usage.cache_write = usage.cache_creation.max(accumulated_usage.cache_write);
            accumulated_usage.total_tokens = accumulated_usage.input + accumulated_usage.output;
        } else if let Some(msg) = &event.message
            && let Some(usage) = &msg.usage
        {
            accumulated_usage.input = usage.input_tokens.max(accumulated_usage.input);
            accumulated_usage.output = usage.output_tokens.max(accumulated_usage.output);
            accumulated_usage.cache_read = usage.cache_read.max(accumulated_usage.cache_read);
            accumulated_usage.cache_write = usage.cache_creation.max(accumulated_usage.cache_write);
            accumulated_usage.total_tokens = accumulated_usage.input + accumulated_usage.output;
        }

        match event_type {
            Some("message_start") => {
                events.push(ProviderEvent::Start {
                    partial: Arc::new(partial_message.clone()),
                });
            }
            Some("content_block_start") => {
                if let Some(block) = &event.content_block {
                    let idx = block.index.or(event.index).unwrap_or(0);
                    match block.type_.as_deref() {
                        Some("text") => {
                            events.push(ProviderEvent::TextStart {
                                content_index: idx,
                                partial: Arc::new(partial_message.clone()),
                            });
                        }
                        Some("thinking") => {
                            events.push(ProviderEvent::ThinkingStart {
                                content_index: idx,
                                partial: Arc::new(partial_message.clone()),
                            });
                        }
                        Some("tool_use") | Some("server_tool_use") => {
                            // Register the tool call in pending_tool_calls
                            // so that input_json_delta events can accumulate
                            // the partial JSON arguments.
                            let tc_id = block.id.clone().unwrap_or_default();
                            let tc_name = block.name.clone().unwrap_or_default();
                            pending_tool_calls.insert(
                                idx,
                                AnthropicPendingToolCall {
                                    id: tc_id.clone(),
                                    name: tc_name.clone(),
                                    partial_json: String::new(),
                                },
                            );
                            events.push(ProviderEvent::ToolCallStart {
                                content_index: idx,
                                tool_call_id: Some(tc_id),
                                tool_name: Some(tc_name),
                                partial: Arc::new(partial_message.clone()),
                            });
                        }
                        Some(t) if t.ends_with("_tool_result") => {
                            let name = match t {
                                "web_search_tool_result" => Some("web_search".to_string()),
                                "code_execution_tool_result" => Some("code_execution".to_string()),
                                "web_fetch_tool_result" => Some("web_fetch".to_string()),
                                _ => None,
                            };
                            if let Some(tool_name) = name {
                                let tc = ToolCall::new(
                                    block.tool_use_id.clone().unwrap_or_default(),
                                    tool_name,
                                    serde_json::json!({}),
                                );
                                partial_message
                                    .content
                                    .push(ContentBlock::ToolCall(tc.clone()));
                                events.push(ProviderEvent::ToolCallEnd {
                                    content_index: idx,
                                    tool_call: tc,
                                    partial: Arc::new(partial_message.clone()),
                                });
                            }
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
                                // Accumulate into partial_message so the TUI
                                // can diff against its snapshot tracker.
                                let last_text_idx = partial_message
                                    .content
                                    .iter()
                                    .rposition(|b| matches!(b, ContentBlock::Text(_)));
                                if let Some(idx) = last_text_idx
                                    && let ContentBlock::Text(t) = &mut partial_message.content[idx]
                                {
                                    t.text.push_str(text);
                                } else {
                                    partial_message
                                        .content
                                        .push(ContentBlock::Text(TextContent::new(text.clone())));
                                }
                                events.push(ProviderEvent::TextDelta {
                                    content_index: event.index.unwrap_or(0),
                                    delta: text.clone(),
                                    partial: Arc::new(partial_message.clone()),
                                });
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(text) = &delta.thinking {
                                let last_think_idx = partial_message
                                    .content
                                    .iter()
                                    .rposition(|b| matches!(b, ContentBlock::Thinking(_)));
                                if let Some(idx) = last_think_idx
                                    && let ContentBlock::Thinking(t) =
                                        &mut partial_message.content[idx]
                                {
                                    t.thinking.push_str(text);
                                } else {
                                    partial_message.content.push(ContentBlock::Thinking(
                                        ThinkingContent::new(text.clone()),
                                    ));
                                }
                                events.push(ProviderEvent::ThinkingDelta {
                                    content_index: event.index.unwrap_or(0),
                                    delta: text.clone(),
                                    partial: Arc::new(partial_message.clone()),
                                });
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(args) = &delta.partial_json {
                                // Accumulate partial JSON into the pending tool call.
                                let block_idx = event.index.unwrap_or(0);
                                if let Some(ptc) = pending_tool_calls.get_mut(&block_idx) {
                                    ptc.partial_json.push_str(args);
                                }
                                events.push(ProviderEvent::ToolCallDelta {
                                    content_index: block_idx,
                                    delta: args.clone(),
                                    partial: Arc::new(partial_message.clone()),
                                });
                            }
                        }
                        Some("signature_delta") => {
                            if let Some(_sig) = &delta.signature {
                                // Signature for session continuity
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("content_block_stop") => {
                // When a tool_use block completes, finalize the accumulated
                // partial JSON into a proper ToolCall and emit ToolCallEnd.
                // This matches pi's `content_block_stop → toolcall_end` pattern.
                let block_idx = event.index.unwrap_or(0);
                if let Some(ptc) = pending_tool_calls.remove(&block_idx) {
                    let args_value = parse_streaming_json(&ptc.partial_json);
                    let tc = ToolCall::new(ptc.id, ptc.name, args_value);

                    // Add the finalized tool call to partial_message so
                    // downstream consumers (streaming.rs Done handler) can
                    // see it in the accumulated message.
                    partial_message
                        .content
                        .push(ContentBlock::ToolCall(tc.clone()));

                    tracing::debug!(
                        block_idx,
                        tool_id = %tc.id,
                        tool_name = %tc.name,
                        "content_block_stop: finalized tool call"
                    );

                    events.push(ProviderEvent::ToolCallEnd {
                        content_index: block_idx,
                        tool_call: tc,
                        partial: Arc::new(partial_message.clone()),
                    });
                }
            }
            Some("message_delta") => {
                if let Some(delta) = &event.delta {
                    let reason = match delta.stop_reason.as_deref() {
                        Some("end_turn") | Some("stop_sequence") | Some("pause_turn") => {
                            StopReason::Stop
                        }
                        Some("max_tokens") => StopReason::Length,
                        Some("tool_use") => StopReason::ToolUse,
                        Some("refusal") | Some("sensitive") => StopReason::Error,
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
            Some("message_stop") => {
                // Message complete – no event needed
            }
            _ => {}
        }
    }

    events
}

/// Test-friendly wrapper that creates fresh state for a single-chunk parse.
/// Used by unit tests that pass complete SSE streams in one go.
#[cfg(test)]
fn parse_anthropic_events(text: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut partial = AssistantMessage::new(Api::AnthropicMessages, "anthropic", model_id);
    let mut usage = Usage::default();
    let mut pending_tool_calls = std::collections::HashMap::new();
    parse_anthropic_events_stateful(text, &mut partial, &mut usage, &mut pending_tool_calls)
}

/// Create error assistant message
fn create_error_message(msg: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(Api::AnthropicMessages, "anthropic", "unknown");
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
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
    /// Nested message from `message_start` event — contains initial usage.
    message: Option<AnthropicMessageStart>,
}

/// Nested message object from Anthropic `message_start` events.
/// Carries initial token usage (input_tokens, output_tokens) before streaming begins.
#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct ContentBlockStart {
    #[serde(rename = "type")]
    type_: Option<String>,
    index: Option<usize>,
    /// Tool call ID present for tool_use blocks (Anthropic sends this in content_block_start)
    id: Option<String>,
    /// Tool name for tool_use blocks.
    name: Option<String>,
    /// Initial thinking content (may be empty string).
    #[allow(dead_code)]
    thinking: Option<String>,
    /// Signature for extended thinking session continuity.
    #[allow(dead_code)]
    signature: Option<String>,
    /// Initial text content (may be non-empty for pre-filled blocks).
    #[allow(dead_code)]
    text: Option<String>,
    /// Tool use ID for server tool result blocks.
    tool_use_id: Option<String>,
    /// Structured content for server tool result blocks.
    #[allow(dead_code)]
    content: Option<JsonValue>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    type_: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    partial_json: Option<String>,
    /// Signature for extended thinking session continuity.
    /// Sent in `signature_delta` events after a thinking block.
    #[allow(dead_code)]
    signature: Option<String>,
    #[serde(rename = "stop_reason")]
    stop_reason: Option<String>,
    #[serde(rename = "stop_sequence")]
    #[allow(dead_code)]
    stop_sequence: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(rename = "input_tokens", default)]
    input_tokens: usize,
    #[serde(rename = "output_tokens", default)]
    output_tokens: usize,
    /// Cache read tokens (Anthropic field: `cache_read_input_tokens`)
    #[serde(rename = "cache_read_input_tokens", alias = "cache_read", default)]
    cache_read: usize,
    /// Cache write tokens (Anthropic field: `cache_creation_input_tokens`)
    #[serde(
        rename = "cache_creation_input_tokens",
        alias = "cache_creation",
        default
    )]
    cache_creation: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "claude-3-5-sonnet-20241022";

    // ── message_start ──────────────────────────────────────────────────

    #[test]
    fn parse_message_start() {
        let sse = "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ProviderEvent::Start { .. }));
    }

    // ── content_block_start ────────────────────────────────────────────

    #[test]
    fn parse_text_block_start() {
        let sse = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextStart { content_index, .. } => assert_eq!(*content_index, 0),
            other => panic!("expected TextStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_block_start() {
        let sse = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ProviderEvent::ThinkingStart { .. }));
    }

    #[test]
    fn parse_tool_use_block_start() {
        let sse = "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"search\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::ToolCallStart { content_index, .. } => assert_eq!(*content_index, 1),
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
    }

    // ── content_block_delta ────────────────────────────────────────────

    #[test]
    fn parse_text_delta() {
        let sse = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
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
    fn parse_thinking_delta() {
        let sse = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me reason...\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::ThinkingDelta { delta, .. } => assert_eq!(delta, "Let me reason..."),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_input_json_delta() {
        let sse = "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"SF\\\"}\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::ToolCallDelta {
                delta,
                content_index,
                ..
            } => {
                assert_eq!(delta, "{\"city\":\"SF\"}");
                assert_eq!(*content_index, 1);
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
    }

    // ── content_block_stop ────────────────────────────────────────────

    #[test]
    fn parse_content_block_stop_finalizes_tool_call() {
        // Full tool call flow: start → delta → stop
        let sse = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_abc\",\"name\":\"bash\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"ls\\\"}\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // ToolCallStart + ToolCallDelta + ToolCallEnd
        assert_eq!(events.len(), 3);

        // Verify ToolCallEnd has the finalized tool call
        let tc_end = events.iter().find_map(|e| match e {
            ProviderEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
            _ => None,
        });
        let tc = tc_end.expect("Should have ToolCallEnd");
        assert_eq!(tc.id, "tool_abc");
        assert_eq!(tc.name, "bash");
        assert_eq!(tc.arguments, serde_json::json!({"command": "ls"}));
    }

    #[test]
    fn parse_content_block_stop_ignores_non_tool() {
        // content_block_stop for a text block should not emit anything
        let sse = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // Only TextStart — content_block_stop for text doesn't emit
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ProviderEvent::TextStart { .. }));
    }

    #[test]
    fn parse_tool_call_accumulates_across_deltas() {
        // Tool arguments split across multiple deltas
        let sse = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"edit\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"tes\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"t.rs\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // ToolCallStart + 2×ToolCallDelta + ToolCallEnd
        assert_eq!(events.len(), 4);

        let tc_end = events.iter().find_map(|e| match e {
            ProviderEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
            _ => None,
        });
        let tc = tc_end.expect("Should have ToolCallEnd");
        assert_eq!(tc.id, "tool_1");
        assert_eq!(tc.name, "edit");
        // Streaming JSON parser should handle partial "test.rs"
        assert_eq!(tc.arguments["path"].as_str(), Some("test.rs"));
    }

    #[test]
    fn parse_tool_call_in_done_message() {
        // Full flow with thinking + tool_use + Done
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"I need to search.\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_search\",\"name\":\"web_search\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"rust async\\\"}\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n"
        );
        let events = parse_anthropic_events(sse, MODEL);

        // Start + ThinkingStart + ThinkingDelta + ToolCallStart + ToolCallDelta
        // + content_block_stop(thinking, no emit) + ToolCallEnd + Done
        // content_block_stop for thinking index 0 has no pending tool call → no emit
        assert!(events.len() >= 6);

        // Verify ToolCallEnd is present
        let tc_end = events.iter().find_map(|e| match e {
            ProviderEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
            _ => None,
        });
        let tc = tc_end.expect("Should have ToolCallEnd");
        assert_eq!(tc.id, "tool_search");
        assert_eq!(tc.name, "web_search");
        assert_eq!(tc.arguments, serde_json::json!({"query": "rust async"}));

        // Verify Done has ToolUse stop reason
        let done = events.iter().find_map(|e| match e {
            ProviderEvent::Done { reason, .. } => Some(*reason),
            _ => None,
        });
        assert_eq!(done, Some(StopReason::ToolUse));

        // Verify Done message contains tool call
        let done_msg = events.iter().find_map(|e| match e {
            ProviderEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        });
        let msg = done_msg.expect("Should have Done event");
        let tool_calls: Vec<_> = msg
            .content
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolCall(_)))
            .collect();
        assert_eq!(
            tool_calls.len(),
            1,
            "Done message should contain exactly 1 tool call"
        );
    }

    // ── message_delta (completion) ─────────────────────────────────────

    #[test]
    fn parse_message_delta_end_turn() {
        let sse = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Stop)),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_message_delta_max_tokens() {
        let sse = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Length)),
            other => panic!("expected Done with Length, got {other:?}"),
        }
    }

    #[test]
    fn parse_message_delta_stop_sequence() {
        let sse =
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"stop_sequence\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        match &events[0] {
            ProviderEvent::Done { reason, .. } => assert!(matches!(reason, StopReason::Stop)),
            other => panic!("expected Done with Stop, got {other:?}"),
        }
    }

    // ── message_stop ───────────────────────────────────────────────────

    #[test]
    fn parse_message_stop_no_event_emitted() {
        let sse = "data: {\"type\":\"message_stop\"}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert!(events.is_empty());
    }

    // ── Thinking block full flow ───────────────────────────────────────

    #[test]
    fn parse_thinking_block_flow() {
        let sse = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"I should\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" check this.\"}}\n",
            "\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], ProviderEvent::ThinkingStart { .. }));
        let thinking: Vec<&str> = events[1..]
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(thinking, vec!["I should", " check this."]);
    }

    // ── Usage accumulation ─────────────────────────────────────────────

    #[test]
    fn parse_usage_from_message_start() {
        // Usage accumulates from earlier events; Done captures what was accumulated
        // *before* the message_delta chunk (since usage updates happen after event emission).
        // The message_start carries initial usage, which gets captured in the Done event.
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"},\"usage\":{\"input_tokens\":100,\"output_tokens\":0,\"cache_read\":80,\"cache_creation\":20}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // Start + TextDelta + Done
        assert_eq!(events.len(), 3);
        match &events[2] {
            ProviderEvent::Done { message, .. } => {
                // Captures usage from message_start (output_tokens was 0 there)
                assert_eq!(message.usage.input, 100);
                assert_eq!(message.usage.output, 0);
                assert_eq!(message.usage.total_tokens, 100);
                assert_eq!(message.usage.cache_read, 80);
                assert_eq!(message.usage.cache_write, 20);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ── Cache metrics ──────────────────────────────────────────────────

    #[test]
    fn parse_cache_metrics() {
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"usage\":{\"input_tokens\":50,\"output_tokens\":0,\"cache_read\":40,\"cache_creation\":10}}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":50,\"output_tokens\":20,\"cache_read\":40,\"cache_creation\":10}}\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // Start + Done
        assert_eq!(events.len(), 2);
        match &events[1] {
            ProviderEvent::Done { message, .. } => {
                assert_eq!(message.usage.cache_read, 40);
                assert_eq!(message.usage.cache_write, 10);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ── Empty / malformed handling ─────────────────────────────────────

    #[test]
    fn parse_empty_input() {
        let events = parse_anthropic_events("", MODEL);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_done_marker_is_ignored() {
        // Anthropic uses event-type based termination, but [DONE] should be silently skipped
        let sse = "data: [DONE]\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_malformed_json_is_skipped() {
        let sse = "data: {broken\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "ok"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_non_data_lines_ignored() {
        let sse = "event: ping\nid: 42\ndata: {\"type\":\"message_start\"}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_empty_data_line_skipped() {
        let sse = "data: \ndata: {\"type\":\"message_start\"}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_unknown_event_type_ignored() {
        let sse = "data: {\"type\":\"ping\"}\ndata: {\"type\":\"message_start\"}\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_carriage_return_line_endings() {
        let sse = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"CR\"}}\r\n\r\n";
        let events = parse_anthropic_events(sse, MODEL);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderEvent::TextDelta { delta, .. } => assert_eq!(delta, "CR"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    // ── Full stream ────────────────────────────────────────────────────

    #[test]
    fn parse_full_anthropic_stream() {
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n",
            "\n",
            "data: {\"type\":\"message_stop\"}\n"
        );
        let events = parse_anthropic_events(sse, MODEL);
        // Start + TextStart + 2×TextDelta + Done
        assert_eq!(events.len(), 5);

        assert!(matches!(&events[0], ProviderEvent::Start { .. }));
        assert!(matches!(&events[1], ProviderEvent::TextStart { .. }));

        let texts: Vec<&str> = events[2..4]
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Hello", " world"]);

        assert!(matches!(
            &events[4],
            ProviderEvent::Done {
                reason: StopReason::Stop,
                ..
            }
        ));
    }

    // ── Stateful multi-chunk parsing ────────────────────────────────────

    /// Simulate how bytes_stream + scan works: parse two chunks with a
    /// shared partial_message and verify content survives across chunks.
    #[test]
    fn parse_stateful_across_two_chunks() {
        let chunk1 = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me\"}}\n",
            "\n"
        );
        let chunk2 = concat!(
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" think.\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n",
            "\n"
        );

        // State persists across chunks (mimics scan() state)
        let mut partial = AssistantMessage::new(Api::AnthropicMessages, "anthropic", MODEL);
        let mut usage = Usage::default();
        let mut pending_tc: std::collections::HashMap<usize, AnthropicPendingToolCall> =
            std::collections::HashMap::new();

        let events1 =
            parse_anthropic_events_stateful(chunk1, &mut partial, &mut usage, &mut pending_tc);
        // Start + ThinkingStart + ThinkingDelta("Let me")
        assert_eq!(events1.len(), 3);

        // CRITICAL: partial_message should have accumulated thinking content
        assert_eq!(partial.content.len(), 1);
        match &partial.content[0] {
            ContentBlock::Thinking(t) => assert_eq!(t.thinking, "Let me"),
            other => panic!("Expected Thinking block, got {:?}", other),
        }

        let events2 =
            parse_anthropic_events_stateful(chunk2, &mut partial, &mut usage, &mut pending_tc);
        // ThinkingDelta(" think.") + TextStart + 2×TextDelta + Done
        assert_eq!(events2.len(), 5);

        // After both chunks, partial_message has both thinking AND text
        assert_eq!(partial.content.len(), 2);
        match &partial.content[0] {
            ContentBlock::Thinking(t) => assert_eq!(t.thinking, "Let me think."),
            other => panic!("Expected Thinking block, got {:?}", other),
        }
        match &partial.content[1] {
            ContentBlock::Text(t) => assert_eq!(t.text, "Hello world"),
            other => panic!("Expected Text block, got {:?}", other),
        }

        // Done event should carry the accumulated content
        let done = events2.iter().find_map(|e| match e {
            ProviderEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        });
        let done_msg = done.expect("Should have Done event");
        assert_eq!(done_msg.content.len(), 2);
    }

    #[test]
    fn parse_stateful_tool_use_across_chunks() {
        let chunk1 = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"I should search.\"}}\n",
            "\n"
        );
        let chunk2 = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"search\"}}\n",
            "\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"q\\\":\\\"rust\\\"}\"}}\n",
            "\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n",
            "\n"
        );

        let mut partial = AssistantMessage::new(Api::AnthropicMessages, "anthropic", MODEL);
        let mut usage = Usage::default();
        let mut pending_tc: std::collections::HashMap<usize, AnthropicPendingToolCall> =
            std::collections::HashMap::new();

        let events1 =
            parse_anthropic_events_stateful(chunk1, &mut partial, &mut usage, &mut pending_tc);
        assert_eq!(events1.len(), 3); // Start + ThinkingStart + ThinkingDelta

        // Thinking persists into chunk2
        assert_eq!(partial.content.len(), 1);

        let events2 =
            parse_anthropic_events_stateful(chunk2, &mut partial, &mut usage, &mut pending_tc);
        // ToolCallStart + ToolCallDelta + Done
        // Note: if content_block_stop were included, we'd also get ToolCallEnd
        assert!(events2.len() >= 2); // At least ToolCallStart + Done

        // Done should have ToolUse stop reason
        let done = events2.iter().find_map(|e| match e {
            ProviderEvent::Done { reason, .. } => Some(*reason),
            _ => None,
        });
        assert_eq!(done, Some(StopReason::ToolUse));
    }
}
