//! Amazon Bedrock provider implementation
//!
//! This provider uses AWS SigV4 authentication with the Bedrock ConverseStream API.
//! Supports Claude, Mistral, and other Bedrock models.

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    error::ProviderError, Api, AssistantMessage, ContentBlock, Context, Model, Provider,
    ProviderEvent, StopReason, StreamOptions, Usage,
};

use super::shared_client;

// Import Digest trait and Sha256 type for SHA256 hashing
use sha2::{Digest, Sha256};

/// HMAC-SHA256 type for SigV4 signing
type HmacSha256 = Hmac<Sha256>;

/// Amazon Bedrock provider
#[derive(Clone)]
pub struct BedrockProvider {
    client: &'static Client,
    default_region: String,
}

impl BedrockProvider {
    /// Create a new Bedrock provider with default region (us-east-1)
    ///
    /// Region is resolved from:
    /// 1. `~/.aws/config` (via AWS CLI profile)
    /// 2. Environment variable `AWS_REGION` (CI/CD fallback)
    /// 3. Default "us-east-1"
    pub fn new() -> Self {
        let region = Self::resolve_region();
        Self {
            client: shared_client(),
            default_region: region,
        }
    }

    /// Resolve AWS region from file config, then env, then default.
    fn resolve_region() -> String {
        // 1. Try ~/.aws/config
        if let Some(region) = Self::region_from_aws_config() {
            return region;
        }
        // 2. Fallback to env (CI/CD)
        if let Ok(region) = std::env::var("AWS_REGION") {
            return region;
        }
        // 3. Default
        "us-east-1".to_string()
    }

    /// Read region from ~/.aws/config [default] region=...
    fn region_from_aws_config() -> Option<String> {
        let home = dirs::home_dir()?;
        let config_path = home.join(".aws").join("config");
        let content = std::fs::read_to_string(&config_path).ok()?;
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(value) = trimmed.strip_prefix("region") {
                let value = value.trim_start_matches([' ', '=']).trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    }

    /// Get AWS credentials from auth.json, ~/.aws/credentials, or env.
    ///
    /// Priority:
    /// 1. Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
    /// 2. ~/.aws/credentials file
    fn get_credentials(&self) -> Result<(String, String, String), ProviderError> {
        // 1. Try environment variables (CI/CD)
        if let (Ok(access_key), Ok(secret_key)) = (
            std::env::var("AWS_ACCESS_KEY_ID"),
            std::env::var("AWS_SECRET_ACCESS_KEY"),
        ) {
            let region =
                std::env::var("AWS_REGION").unwrap_or_else(|_| self.default_region.clone());
            return Ok((access_key, secret_key, region));
        }

        // 2. Try ~/.aws/credentials
        if let Some((access_key, secret_key)) = Self::creds_from_aws_file() {
            return Ok((access_key, secret_key, self.default_region.clone()));
        }

        Err(ProviderError::MissingApiKey)
    }

    /// Read credentials from ~/.aws/credentials [default]
    fn creds_from_aws_file() -> Option<(String, String)> {
        let home = dirs::home_dir()?;
        let creds_path = home.join(".aws").join("credentials");
        let content = std::fs::read_to_string(&creds_path).ok()?;

        let mut access_key = None;
        let mut secret_key = None;
        let mut in_default = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_default = trimmed == "[default]";
                continue;
            }
            if !in_default {
                continue;
            }
            if let Some(value) = trimmed.strip_prefix("aws_access_key_id") {
                let value = value.trim_start_matches([' ', '=']).trim();
                access_key = Some(value.to_string());
            } else if let Some(value) = trimmed.strip_prefix("aws_secret_access_key") {
                let value = value.trim_start_matches([' ', '=']).trim();
                secret_key = Some(value.to_string());
            }
        }

        match (access_key, secret_key) {
            (Some(a), Some(s)) => Some((a, s)),
            _ => None,
        }
    }

    /// Get optional session token (for temporary credentials)
    fn get_session_token(&self) -> Option<String> {
        std::env::var("AWS_SESSION_TOKEN").ok()
    }

    /// Get the endpoint URL for a model
    fn get_endpoint(&self, model: &Model, region: &str) -> String {
        // Use model's base_url if available, otherwise construct from region
        if !model.base_url.is_empty() {
            format!("{}/converse-stream", model.base_url)
        } else {
            let region = if region.is_empty() {
                &self.default_region
            } else {
                region
            };
            format!(
                "https://bedrock-runtime.{}.amazonaws.com/model/{}/converse-stream",
                region, model.id
            )
        }
    }

    /// Sign a request using AWS SigV4
    #[allow(clippy::too_many_arguments)]
    fn sign_request(
        &self,
        method: &str,
        url: &str,
        headers: &mut reqwest::header::HeaderMap,
        body: &[u8],
        access_key: &str,
        secret_key: &str,
        region: &str,
        service: &str,
    ) -> Result<(), ProviderError> {
        // Parse the URL to get host and path
        let parsed_url =
            url::Url::parse(url).map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;

        let host = parsed_url.host_str().unwrap_or("");
        let path = parsed_url.path();
        let query = parsed_url.query().unwrap_or("");

        // Get current time for signing
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ProviderError::InvalidResponse("Invalid system time".into()))?;
        let timestamp = now.as_secs();
        let datetime = format_timestamp(timestamp);

        // Content hash
        let content_hash = hex_encode(hash_sha256(body));

        // Set required headers
        headers.insert(
            "content-type",
            "application/json".parse().expect("valid header value"),
        );
        headers.insert("host", host.parse().expect("valid header value"));
        headers.insert("x-amz-date", datetime.parse().expect("valid header value"));
        headers.insert(
            "x-amz-content-sha256",
            content_hash.parse().expect("valid header value"),
        );

        // Build canonical request
        let canonical_request =
            build_canonical_request(method, path, query, headers, &content_hash);

        // Build string to sign
        let credential_scope = format!("{}/{}/*", datetime, service);
        let hashed_canonical = hex_encode(hash_sha256(canonical_request.as_bytes()));
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            datetime, credential_scope, hashed_canonical
        );

        // Calculate signature
        let signature =
            self.calculate_signature(secret_key, region, service, timestamp, &string_to_sign);

        // Build authorization header
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            access_key,
            credential_scope,
            "content-type;host;x-amz-content-sha256;x-amz-date",
            signature
        );

        headers.insert(
            "authorization",
            authorization.parse().expect("valid header value"),
        );

        Ok(())
    }

    /// Calculate AWS SigV4 signature
    fn calculate_signature(
        &self,
        secret_key: &str,
        region: &str,
        service: &str,
        timestamp: u64,
        string_to_sign: &str,
    ) -> String {
        let datetime = format_timestamp(timestamp);

        // AWS4 secret key
        let k_secret = format!("AWS4{}", secret_key);
        let k_date = hmac_sign(&datetime[..8], k_secret.as_bytes());
        let k_region = hmac_sign(region, &k_date);
        let k_service = hmac_sign(service, &k_region);
        let k_signing = hmac_sign("aws4_request", &k_service);

        // Final signature
        hex_encode(hmac_sign_n(string_to_sign.as_bytes(), &k_signing))
    }
}

impl Default for BedrockProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// AWS timestamp format: YYYYMMDDTHHMMSSZ using chrono
fn format_timestamp(timestamp: u64) -> String {
    use chrono::TimeZone;
    let datetime = chrono::Utc
        .timestamp_opt(timestamp as i64, 0)
        .single()
        .expect("invalid timestamp");
    datetime.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Build canonical request for SigV4
fn build_canonical_request(
    method: &str,
    path: &str,
    query: &str,
    headers: &reqwest::header::HeaderMap,
    content_hash: &str,
) -> String {
    // Canonical query string
    let canonical_query = if query.is_empty() {
        String::new()
    } else {
        let mut parts: Vec<(String, String)> = query
            .split('&')
            .map(|part| {
                let mut split = part.split('=');
                let key = split.next().unwrap_or("");
                let val = split.next().unwrap_or("");
                (key.to_string(), val.to_string())
            })
            .collect();
        parts.sort_by(|a, b| a.0.cmp(&b.0));
        parts
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding_encode(k), urlencoding_encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    };

    // Canonical headers (sorted)
    let mut header_vec: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_lowercase(),
                String::from_utf8_lossy(v.as_bytes()).trim().to_string(),
            )
        })
        .collect();
    header_vec.sort_by(|a, b| a.0.cmp(&b.0));

    let canonical_headers: Vec<String> = header_vec
        .iter()
        .map(|(k, v)| format!("{}:{}", k, v))
        .collect();
    let canonical_headers_str = canonical_headers.join("\n");

    let signed_headers: Vec<&str> = header_vec.iter().map(|(k, _)| k.as_str()).collect();
    let signed_headers_str = signed_headers.join(";");

    format!(
        "{}\n{}\n{}\n{}\n\n{}\n{}",
        method, path, canonical_query, canonical_headers_str, signed_headers_str, content_hash
    )
}

/// HMAC-SHA256 sign
fn hmac_sign(msg: &str, key: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(msg.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// HMAC-SHA256 sign with pre-computed key
fn hmac_sign_n(msg: &[u8], key: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

/// SHA256 hash using Digest trait
fn hash_sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Hex encode bytes
fn hex_encode(data: Vec<u8>) -> String {
    data.iter().map(|b| format!("{:02x}", b)).collect()
}

/// URL encoding for SigV4 (RFC 3986)
fn urlencoding_encode(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
            result.push(c);
        } else {
            for b in c.to_string().as_bytes() {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
}

#[async_trait]
impl Provider for BedrockProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Get credentials
        let (access_key, secret_key, region) = self.get_credentials()?;
        let session_token = self.get_session_token();

        // Get endpoint
        let url = self.get_endpoint(model, &region);

        // Build messages
        let messages = build_bedrock_messages(context)?;

        // Build request body
        let mut body = serde_json::json!({
            "messages": messages,
        });

        // Add system prompt
        if let Some(ref prompt) = context.system_prompt {
            body["system"] = serde_json::json!([{
                "text": prompt,
            }]);
        }

        // Add inference config
        let mut inference_config = serde_json::json!({});
        if let Some(temp) = options.temperature {
            inference_config["temperature"] = serde_json::json!(temp);
        }
        if let Some(max) = options.max_tokens {
            inference_config["maxTokens"] = serde_json::json!(max);
        }
        body["inferenceConfig"] = inference_config;

        // Add tool config if tools are present
        if !context.tools.is_empty() {
            body["toolConfig"] = build_bedrock_tool_config(&context.tools)?;
        }

        let body_bytes = serde_json::to_vec(&body)?;

        // Build headers
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().expect("valid header value"),
        );

        // Add session token if present (for temporary credentials)
        if let Some(token) = session_token {
            headers.insert(
                "x-amz-security-token",
                token.parse().expect("valid header value"),
            );
        }

        // Sign the request
        self.sign_request(
            "POST",
            &url,
            &mut headers,
            &body_bytes,
            &access_key,
            &secret_key,
            &region,
            "bedrock",
        )?;

        // Make request
        let response = self
            .client
            .post(&url)
            .headers(headers)
            .body(body_bytes)
            .send()
            .await
            .map_err(ProviderError::RequestFailed)?;

        if !response.status().is_success() {
            let status = response.status();
            let body: String = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpError(status.as_u16(), body));
        }

        // Create event stream
        let provider_name = "bedrock".to_string();
        let model_id = model.id.clone();

        let stream = response.bytes_stream().flat_map(
            move |chunk: Result<Bytes, reqwest::Error>| match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    futures::stream::iter(parse_bedrock_events(&text, &provider_name, &model_id))
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
        "bedrock"
    }
}

/// Build messages in Bedrock Converse format
fn build_bedrock_messages(context: &Context) -> Result<Vec<JsonValue>, ProviderError> {
    let mut messages = Vec::new();

    for msg in &context.messages {
        match msg {
            crate::Message::User(u) => {
                let content = match &u.content {
                    crate::MessageContent::Text(s) => {
                        vec![serde_json::json!({
                            "text": s,
                        })]
                    }
                    crate::MessageContent::Blocks(blocks) => blocks_to_bedrock_content(blocks)?,
                };
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
            }
            crate::Message::Assistant(a) => {
                let content = blocks_to_bedrock_content(&a.content)?;
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }));
            }
            crate::Message::ToolResult(t) => {
                let content = blocks_to_bedrock_content(&t.content)?;
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "toolResult": {
                            "toolUseId": t.tool_call_id,
                            "toolName": t.tool_name,
                            "content": [{
                                "json": content,
                            }],
                        }
                    }],
                }));
            }
        }
    }

    Ok(messages)
}

/// Convert content blocks to Bedrock format
fn blocks_to_bedrock_content(blocks: &[ContentBlock]) -> Result<Vec<JsonValue>, ProviderError> {
    let mut items = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text(t) => {
                items.push(serde_json::json!({
                    "text": t.text,
                }));
            }
            ContentBlock::ToolCall(tc) => {
                items.push(serde_json::json!({
                    "toolUse": {
                        "toolUseId": tc.id,
                        "name": tc.name,
                        "input": tc.arguments,
                    },
                }));
            }
            ContentBlock::Thinking(th) => {
                // Bedrock doesn't have native thinking, but Claude models support it
                items.push(serde_json::json!({
                    "thinking": {
                        "thinking": th.thinking,
                    },
                }));
            }
            ContentBlock::Image(img) => {
                items.push(serde_json::json!({
                    "image": {
                        "format": img.mime_type.split('/').next_back().unwrap_or("jpeg"),
                        "source": {
                            "bytes": img.data,
                        },
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

/// Build tool config in Bedrock format
fn build_bedrock_tool_config(tools: &[crate::Tool]) -> Result<JsonValue, ProviderError> {
    let items: Vec<_> = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "toolSpec": {
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": {
                        "json": tool.parameters,
                    },
                },
            })
        })
        .collect();

    Ok(serde_json::json!({
        "tools": items,
    }))
}

/// Parse Bedrock ConverseStream SSE events
fn parse_bedrock_events(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let mut partial_message = AssistantMessage::new(Api::BedrockConverseStream, provider, model_id);

    let estimated_events = text.split('\n').filter(|l| l.starts_with("data: ")).count();
    events.reserve(estimated_events);

    let mut accumulated_usage = Usage::default();
    let mut stop_reason: Option<StopReason> = None;
    let mut seen_start = false;

    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        if !line.starts_with("data: ") {
            continue;
        }

        let data = &line[6..];

        if data.is_empty() {
            continue;
        }

        let event = match serde_json::from_str::<BedrockEvent>(data) {
            Ok(e) => e,
            Err(_) => continue,
        };

        match event.type_.as_deref() {
            Some("messageStart") => {
                seen_start = true;
                events.push(ProviderEvent::Start {
                    partial: partial_message.clone(),
                });
            }
            Some("contentBlockStart") => {
                if let Some(block) = &event.content_block {
                    let block_type = block.get_type();

                    match block_type {
                        Some("text") => {
                            events.push(ProviderEvent::TextStart {
                                content_index: event.index.unwrap_or(0),
                                partial: partial_message.clone(),
                            });
                        }
                        Some("toolUse") => {
                            events.push(ProviderEvent::ToolCallStart {
                                content_index: event.index.unwrap_or(0),
                                tool_call_id: block.id.clone(),
                                tool_name: None,
                                partial: partial_message.clone(),
                            });
                        }
                        Some("thinking") => {
                            events.push(ProviderEvent::ThinkingStart {
                                content_index: event.index.unwrap_or(0),
                                partial: partial_message.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
            Some("contentBlockDelta") => {
                if let Some(delta) = &event.delta {
                    match delta.type_.as_deref() {
                        Some("textDelta") => {
                            if let Some(text) = &delta.text {
                                // pi-mono: accumulate into partial_message so the TUI can
                                // diff against its snapshot tracker.
                                let last_text_idx = partial_message
                                    .content
                                    .iter()
                                    .rposition(|b| matches!(b, ContentBlock::Text(_)));
                                if let Some(idx) = last_text_idx {
                                    if let ContentBlock::Text(t) = &mut partial_message.content[idx]
                                    {
                                        t.text.push_str(text);
                                    }
                                } else {
                                    partial_message.content.push(ContentBlock::Text(
                                        crate::TextContent::new(text.clone()),
                                    ));
                                }
                                events.push(ProviderEvent::TextDelta {
                                    content_index: event.index.unwrap_or(0),
                                    delta: text.clone(),
                                    partial: partial_message.clone(),
                                });
                            }
                        }
                        Some("toolUseDelta") => {
                            if let Some(tool_use) = &delta.tool_use {
                                // Emit tool call name if present
                                if let Some(name) = &tool_use.name {
                                    events.push(ProviderEvent::ToolCallDelta {
                                        content_index: event.index.unwrap_or(0),
                                        delta: format!("name:{}:DELIMITER", name),
                                        partial: partial_message.clone(),
                                    });
                                }
                                // Emit arguments
                                if let Some(input) = &tool_use.input {
                                    events.push(ProviderEvent::ToolCallDelta {
                                        content_index: event.index.unwrap_or(0),
                                        delta: input.clone(),
                                        partial: partial_message.clone(),
                                    });
                                }
                            }
                        }
                        Some("thinkingDelta") => {
                            if let Some(thinking) = &delta.thinking {
                                // pi-mono: accumulate into partial_message
                                let last_think_idx = partial_message
                                    .content
                                    .iter()
                                    .rposition(|b| matches!(b, ContentBlock::Thinking(_)));
                                if let Some(idx) = last_think_idx {
                                    if let ContentBlock::Thinking(t) =
                                        &mut partial_message.content[idx]
                                    {
                                        t.thinking.push_str(thinking);
                                    }
                                } else {
                                    partial_message.content.push(ContentBlock::Thinking(
                                        crate::ThinkingContent::new(thinking.clone()),
                                    ));
                                }
                                events.push(ProviderEvent::ThinkingDelta {
                                    content_index: event.index.unwrap_or(0),
                                    delta: thinking.clone(),
                                    partial: partial_message.clone(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("contentBlockStop") => {
                // Content block ended
            }
            Some("messageStop") => {
                // Check for stop reason in metadata
                if let Some(metadata) = &event.metadata {
                    if let Some(reason) = &metadata.stop_reason {
                        stop_reason = Some(match reason.as_str() {
                            "end_turn" => StopReason::Stop,
                            "max_tokens" => StopReason::Length,
                            "tool_use" => StopReason::ToolUse,
                            "content_filtered" => StopReason::Error,
                            _ => StopReason::Stop,
                        });
                    }
                    if let Some(usage) = &metadata.usage {
                        accumulated_usage.input = usage.input_tokens.unwrap_or(0);
                        accumulated_usage.output = usage.output_tokens.unwrap_or(0);
                        accumulated_usage.total_tokens =
                            usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0);
                    }
                }
            }
            _ => {}
        }
    }

    // Emit done event if we saw a start
    if seen_start {
        let mut done_msg = partial_message.clone();
        done_msg.usage = accumulated_usage.clone();
        events.push(ProviderEvent::Done {
            reason: stop_reason.unwrap_or(StopReason::Stop),
            message: done_msg,
        });
    }

    events
}

/// Create error assistant message
fn create_error_message(msg: &str, provider: &str, model_id: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(Api::BedrockConverseStream, provider, model_id);
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// Bedrock ConverseStream event structure
#[derive(Debug, Deserialize)]
// serde deserialization structs
struct BedrockEvent {
    #[serde(rename = "type")]
    type_: Option<String>,
    #[serde(rename = "index")]
    index: Option<usize>,
    #[serde(rename = "contentBlock")]
    content_block: Option<ContentBlockRef>,
    delta: Option<BedrockDelta>,
    metadata: Option<BedrockMetadata>,
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
struct ContentBlockRef {
    #[serde(rename = "type")]
    block_type: Option<String>,
    #[serde(rename = "index")]
    _index: Option<usize>,
    /// Tool call ID present for toolUse blocks
    #[serde(default)]
    id: Option<String>,
}

impl ContentBlockRef {
    fn get_type(&self) -> Option<&str> {
        self.block_type.as_deref()
    }
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
struct BedrockDelta {
    #[serde(rename = "type")]
    type_: Option<String>,
    text: Option<String>,
    #[serde(rename = "toolUse")]
    tool_use: Option<ToolUseDelta>,
    thinking: Option<String>,
    #[serde(rename = "partialJson")]
    _partial_json: Option<String>,
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
struct ToolUseDelta {
    #[serde(rename = "toolUseId")]
    _tool_use_id: Option<String>,
    name: Option<String>,
    input: Option<String>,
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
struct BedrockMetadata {
    #[serde(rename = "stopReason")]
    stop_reason: Option<String>,
    #[serde(rename = "usage")]
    usage: Option<BedrockUsage>,
    #[serde(rename = "trace")]
    _trace: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
// serde deserialization structs
struct BedrockUsage {
    #[serde(rename = "inputTokens")]
    input_tokens: Option<usize>,
    #[serde(rename = "outputTokens")]
    output_tokens: Option<usize>,
    #[serde(rename = "totalTokens")]
    _total_tokens: Option<usize>,
    #[serde(rename = "cacheReadInputTokens")]
    _cache_read_input_tokens: Option<usize>,
    #[serde(rename = "cacheCreationInputTokens")]
    _cache_creation_input_tokens: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    #[test]
    fn test_timestamp_format() {
        // Test timestamp for known date: 2024-01-15 13:50:45 UTC
        let timestamp = 1705326645u64;
        let formatted = format_timestamp(timestamp);
        // UTC timestamp 1705326645 = 2024-01-15T13:50:45Z
        assert!(formatted.starts_with("20240115T1350"));
        assert!(formatted.ends_with("Z"));
    }

    #[test]
    fn test_hmac_sign() {
        let key = b"secret";
        let msg = "test message";
        let result = hmac_sign(msg, key);
        assert_eq!(result.len(), 32); // SHA256 output length
    }

    #[test]
    fn test_hash_sha256() {
        let data = b"hello world";
        let result = hash_sha256(data);
        // Known SHA256 hash of "hello world"
        assert_eq!(
            hex_encode(result),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_urlencoding() {
        // RFC 3986 percent-encoding - = is a reserved character and should be encoded
        assert_eq!(urlencoding_encode("hello world"), "hello%20world");
        assert_eq!(urlencoding_encode("test-file.png"), "test-file.png");
        assert_eq!(
            urlencoding_encode("key=value&other=1"),
            "key%3Dvalue%26other%3D1"
        );
    }

    #[test]
    fn test_build_bedrock_messages() {
        let mut context = Context::default();
        context.add_message(Message::user("Hello, world!"));

        let messages = build_bedrock_messages(&context).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn test_parse_bedrock_events_usage() {
        let json = r#"{"type":"messageStart","message":{}}"#;
        let json2 = r#"{"type":"messageStop","metadata":{"stopReason":"end_turn","usage":{"inputTokens":100,"outputTokens":50}}}"#;

        let events = parse_bedrock_events(
            &format!("data: {}\ndata: {}", json, json2),
            "bedrock",
            "anthropic.claude-3-sonnet",
        );

        let done_event = events
            .iter()
            .find(|e| matches!(e, ProviderEvent::Done { .. }));
        assert!(done_event.is_some());
        if let ProviderEvent::Done { message, .. } = done_event.unwrap() {
            assert_eq!(message.usage.input, 100);
            assert_eq!(message.usage.output, 50);
        }
    }

    #[test]
    fn test_blocks_to_bedrock_content_text() {
        let blocks = vec![ContentBlock::Text(crate::TextContent::new("Hello"))];
        let result = blocks_to_bedrock_content(&blocks).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["text"], "Hello");
    }

    #[test]
    fn test_blocks_to_bedrock_content_tool_call() {
        let blocks = vec![ContentBlock::ToolCall(crate::ToolCall::new(
            "call-123",
            "get_weather",
            serde_json::json!({"city": "NYC"}),
        ))];
        let result = blocks_to_bedrock_content(&blocks).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["toolUse"]["toolUseId"], "call-123");
        assert_eq!(result[0]["toolUse"]["name"], "get_weather");
    }

    #[test]
    fn test_build_bedrock_tool_config() {
        let tools = vec![crate::Tool {
            name: "get_weather".to_string(),
            description: "Get weather for a city".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string"}
                }
            }),
        }];

        let config = build_bedrock_tool_config(&tools).unwrap();
        assert_eq!(config["tools"].as_array().unwrap().len(), 1);
        assert_eq!(config["tools"][0]["toolSpec"]["name"], "get_weather");
    }

    #[test]
    fn test_hex_encode() {
        let data = vec![0x48, 0x65, 0x6c, 0x6c, 0x6f]; // "Hello"
        assert_eq!(hex_encode(data), "48656c6c6f");
    }

    #[test]
    fn test_provider_name() {
        let provider = BedrockProvider::new();
        assert_eq!(provider.name(), "bedrock");
    }

    #[test]
    fn test_parse_bedrock_events_message_start() {
        let json = r#"{"type":"messageStart"}"#;
        let events = parse_bedrock_events(
            &format!("data: {}", json),
            "bedrock",
            "anthropic.claude-3-sonnet",
        );
        assert!(!events.is_empty());
        assert!(matches!(events[0], ProviderEvent::Start { .. }));
    }

    #[test]
    fn test_parse_bedrock_events_content_blocks() {
        let j1 = r#"{"type":"messageStart"}"#;
        let j2 = r#"{"type":"contentBlockStart","contentBlock":{"type":"text","index":0}}"#;
        let j3 =
            r#"{"type":"contentBlockDelta","index":0,"delta":{"type":"textDelta","text":"Hello"}}"#;
        let j4 = r#"{"type":"contentBlockStop","index":0}"#;
        let j5 = r#"{"type":"messageStop","metadata":{"stopReason":"end_turn"}}"#;
        let text = format!(
            "data: {}\ndata: {}\ndata: {}\ndata: {}\ndata: {}",
            j1, j2, j3, j4, j5
        );
        let events = parse_bedrock_events(&text, "bedrock", "model");
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::Start { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::TextStart { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::TextDelta { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::Done { .. })));
    }
    #[test]
    fn test_parse_bedrock_events_thinking() {
        let j1 = r#"{"type":"messageStart"}"#;
        let j2 = r#"{"type":"contentBlockStart","contentBlock":{"type":"thinking","index":0}}"#;
        let j3 = r#"{"type":"contentBlockDelta","index":0,"delta":{"type":"thinkingDelta","thinking":"test"}}"#;
        let j4 = r#"{"type":"contentBlockStop","index":0}"#;
        let j5 = r#"{"type":"messageStop","metadata":{"stopReason":"end_turn"}}"#;
        let text = format!(
            "data: {}\ndata: {}\ndata: {}\ndata: {}\ndata: {}",
            j1, j2, j3, j4, j5
        );
        let events = parse_bedrock_events(&text, "bedrock", "model");
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ThinkingStart { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ThinkingDelta { .. })));
    }
    #[test]
    fn test_parse_bedrock_events_tool_call() {
        let j1 = r#"{"type":"messageStart"}"#;
        let j2 = r#"{"type":"contentBlockStart","contentBlock":{"type":"toolUse","index":0}}"#;
        let j3 = r#"{"type":"contentBlockDelta","index":0,"delta":{"type":"toolUseDelta","toolUse":{"name":"test"}}}"#;
        let j4 = r#"{"type":"contentBlockStop","index":0}"#;
        let j5 = r#"{"type":"messageStop","metadata":{"stopReason":"tool_use"}}"#;
        let text = format!(
            "data: {}\ndata: {}\ndata: {}\ndata: {}\ndata: {}",
            j1, j2, j3, j4, j5
        );
        let events = parse_bedrock_events(&text, "bedrock", "model");
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ToolCallStart { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::Done {
                reason: StopReason::ToolUse,
                ..
            }
        )));
    }
}
