//! Devin (Codeium/Windsurf Cascade) provider — remote-AGENT protocol.
//!
//! Port of omp `packages/ai/src/providers/devin.ts` (678 lines).
//!
//! Devin/Cascade uses the **Connect protocol** over HTTP/1.1 with protobuf
//! payloads. The RPC endpoint is
//! `/exa.api_server_pb.ApiServerService/GetChatMessage`.
//!
//! ## Protocol
//! - HTTP/1.1 transport (standard `reqwest::Client`)
//! - `application/connect+proto` content type
//! - Binary framing: 1 flag byte + 4-byte big-endian length + payload
//! - Flag bit 0x01 = gzip compressed, 0x02 = end-of-stream (JSON trailers)
//! - Auth: session token -> GetUserJwt -> Bearer JWT for chat

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures::{Stream, StreamExt};
use prost::Message;
use std::io::Read;
use uuid::Uuid;

use super::shared_client;
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Model, Provider, ProviderEvent, StopReason,
    StreamOptions, StreamResult, TextContent, TextContentType, ThinkingContent,
    ThinkingContentType, ToolCall, ToolCallType, Usage,
    error::ProviderError,
};

// ── Constants ───────────────────────────────────────────────────────

const DEVIN_API_URL: &str = "https://server.codeium.com";
const CHAT_MESSAGE_PATH: &str = "/exa.api_server_pb.ApiServerService/GetChatMessage";
const DEVIN_AUTH_PATH: &str = "/exa.auth_pb.AuthService/GetUserJwt";
const DEVIN_SESSION_TOKEN_PREFIX: &str = "devin-session-token$";
const DEVIN_IDE_VERSION: &str = "3.2.23";
const DEVIN_EXTENSION_VERSION: &str = "1.48.2";
const DEVIN_DEFAULT_STOP_PATTERNS: &[&str] = &[
    "<|user|>", "<|bot|>", "<|context_request|>", "<|endoftext|>", "<|end_of_turn|>",
];
const CONNECT_COMPRESSED_FLAG: u8 = 0x01;
const CONNECT_END_STREAM_FLAG: u8 = 0x02;
const MAX_CONNECT_FRAME_PAYLOAD: usize = 16 * 1024 * 1024;
const FRAME_HEADER_SIZE: usize = 5;

// ── Connect protocol framing ────────────────────────────────────────

struct ConnectFrame {
    flags: u8,
    payload: Bytes,
}

fn parse_connect_frames(buffer: &[u8]) -> (Vec<ConnectFrame>, Vec<u8>) {
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset + FRAME_HEADER_SIZE <= buffer.len() {
        let flags = buffer[offset];
        let len = u32::from_be_bytes([
            buffer[offset + 1],
            buffer[offset + 2],
            buffer[offset + 3],
            buffer[offset + 4],
        ]) as usize;
        if len > MAX_CONNECT_FRAME_PAYLOAD {
            break;
        }
        if offset + FRAME_HEADER_SIZE + len > buffer.len() {
            break;
        }
        frames.push(ConnectFrame {
            flags,
            payload: Bytes::copy_from_slice(&buffer[offset + FRAME_HEADER_SIZE..offset + FRAME_HEADER_SIZE + len]),
        });
        offset += FRAME_HEADER_SIZE + len;
    }
    (frames, buffer[offset..].to_vec())
}

fn build_connect_frame(payload: &[u8], compress: bool) -> Vec<u8> {
    let (data, flags) = if compress {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut encoder, payload)
            .expect("Gzip encoder write failed");
        (encoder.finish().expect("Gzip encoder finish failed"), CONNECT_COMPRESSED_FLAG)
    } else {
        (payload.to_vec(), 0)
    };
    let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE + data.len());
    frame.push(flags);
    frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
    frame.extend_from_slice(&data);
    frame
}

fn parse_connect_trailer(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    let err = parsed.get("error")?;
    let code = err.get("code")?.as_str()?;
    let message = err.get("message")?.as_str()?;
    Some(format!("Devin stream error {code}: {message}"))
}

// ── Protobuf message types ─────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, prost::Enumeration)]
#[repr(i32)]
pub enum ChatMessageSource {
    Unknown = 0,
    User = 1,
    System = 2,
    Tool = 4,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Metadata {
    #[prost(string, tag = "1")]
    pub api_key: String,
    #[prost(string, tag = "2")]
    pub ide_name: String,
    #[prost(string, tag = "3")]
    pub ide_version: String,
    #[prost(string, tag = "4")]
    pub extension_name: String,
    #[prost(string, tag = "5")]
    pub extension_version: String,
    #[prost(string, tag = "6")]
    pub locale: String,
    #[prost(string, tag = "7")]
    pub user_jwt: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ChatToolCall {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub arguments_json: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ChatToolDefinition {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub description: String,
    #[prost(string, tag = "3")]
    pub json_schema_string: String,
    #[prost(bool, tag = "4")]
    pub strict: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ChatMessagePrompt {
    #[prost(string, tag = "1")]
    pub message_id: String,
    #[prost(enumeration = "ChatMessageSource", tag = "2")]
    pub source: i32,
    #[prost(string, tag = "3")]
    pub prompt: String,
    #[prost(string, tag = "4")]
    pub thinking: String,
    #[prost(string, tag = "5")]
    pub signature: String,
    #[prost(message, repeated, tag = "6")]
    pub tool_calls: Vec<ChatToolCall>,
    #[prost(string, tag = "7")]
    pub tool_call_id: String,
    #[prost(bool, tag = "9")]
    pub tool_result_is_error: bool,
    #[prost(string, tag = "11")]
    pub signature_type: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CompletionConfiguration {
    #[prost(uint64, tag = "1")]
    pub num_completions: u64,
    #[prost(uint64, tag = "2")]
    pub max_tokens: u64,
    #[prost(uint64, tag = "3")]
    pub max_newlines: u64,
    #[prost(double, tag = "4")]
    pub temperature: f64,
    #[prost(uint64, tag = "5")]
    pub top_k: u64,
    #[prost(double, tag = "6")]
    pub top_p: f64,
    #[prost(string, repeated, tag = "7")]
    pub stop_patterns: Vec<String>,
    #[prost(double, tag = "8")]
    pub fim_eot_prob_threshold: f64,
    #[prost(double, tag = "9")]
    pub first_temperature: f64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct GetUserJwtRequest {
    #[prost(message, tag = "1")]
    pub metadata: Option<Metadata>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct GetUserJwtResponse {
    #[prost(string, tag = "1")]
    pub user_jwt: String,
    #[prost(string, tag = "2")]
    pub custom_api_server_url: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ModelUsageStats {
    #[prost(int64, tag = "1")]
    pub input_tokens: i64,
    #[prost(int64, tag = "2")]
    pub output_tokens: i64,
    #[prost(int64, tag = "3")]
    pub cache_read_tokens: i64,
    #[prost(int64, tag = "4")]
    pub cache_write_tokens: i64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct GetChatMessageRequest {
    #[prost(message, tag = "1")]
    pub metadata: Option<Metadata>,
    #[prost(string, tag = "2")]
    pub prompt: String,
    #[prost(message, repeated, tag = "3")]
    pub chat_message_prompts: Vec<ChatMessagePrompt>,
    #[prost(string, tag = "21")]
    pub chat_model_uid: String,
    #[prost(int32, tag = "7")]
    pub request_type: i32,
    #[prost(message, tag = "8")]
    pub configuration: Option<CompletionConfiguration>,
    #[prost(message, repeated, tag = "10")]
    pub tools: Vec<ChatToolDefinition>,
    #[prost(bool, tag = "11")]
    pub disable_parallel_tool_calls: bool,
    #[prost(string, tag = "16")]
    pub cascade_id: String,
    #[prost(int32, tag = "20")]
    pub planner_mode: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct GetChatMessageResponse {
    #[prost(string, tag = "1")]
    pub message_id: String,
    #[prost(string, tag = "3")]
    pub delta_text: String,
    #[prost(int32, tag = "5")]
    pub stop_reason: i32,
    #[prost(message, repeated, tag = "6")]
    pub delta_tool_calls: Vec<ChatToolCall>,
    #[prost(message, tag = "7")]
    pub usage: Option<ModelUsageStats>,
    #[prost(string, tag = "9")]
    pub delta_thinking: String,
    #[prost(string, tag = "10")]
    pub delta_signature: String,
}

// ── Devin Provider ──────────────────────────────────────────────────

#[derive(Clone)]
pub struct DevinProvider {
    client: &'static reqwest::Client,
    base_url: String,
}

impl DevinProvider {
    pub fn new() -> Self {
        Self { client: shared_client(), base_url: DEVIN_API_URL.to_string() }
    }
    #[allow(dead_code)]
    pub fn with_base_url(base_url: &str) -> Self {
        Self { client: shared_client(), base_url: base_url.trim_end_matches('/').to_string() }
    }
}

impl Default for DevinProvider {
    fn default() -> Self { Self::new() }
}

impl Provider for DevinProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        let client = self.client;
        let base_url = self.base_url.clone();
        let model_id = model.id.clone();
        let context_clone = context.clone();

        Box::pin(async move {
            // 1. Resolve API key
            let api_key = options.as_ref()
                .and_then(|o| o.api_key.clone())
                .or_else(|| std::env::var("DEVIN_API_KEY").ok())
                .or_else(|| std::env::var("CODEIUM_API_KEY").ok())
                .map(normalize_devin_session_token)
                .ok_or_else(|| ProviderError::InvalidResponse(
                    "Devin API key required — set DEVIN_API_KEY or CODEIUM_API_KEY".into()
                ))?;

            // 2. Auth: GetUserJwt
            let auth_req = GetUserJwtRequest {
                metadata: Some(Metadata {
                    api_key: api_key.clone(), ide_name: "windsurf".into(),
                    ide_version: DEVIN_IDE_VERSION.into(), extension_name: "windsurf".into(),
                    extension_version: DEVIN_EXTENSION_VERSION.into(), locale: "en".into(),
                    user_jwt: String::new(),
                }),
            };
            let auth_body = auth_req.encode_to_vec();
            let auth_resp = client.post(format!("{}{}", base_url, DEVIN_AUTH_PATH))
                .header("content-type", "application/proto")
                .header("connect-protocol-version", "1")
                .header("accept", "*/*")
                .body(auth_body).send().await
                .map_err(|e| ProviderError::NetworkError(format!("Devin auth failed: {e}")))?;
            let auth_status = auth_resp.status();
            let auth_bytes = auth_resp.bytes().await
                .map_err(|e| ProviderError::NetworkError(format!("Devin auth read: {e}")))?;
            if !auth_status.is_success() {
                return Err(ProviderError::HttpError(crate::HttpErrorDetail {
                    status: auth_status.as_u16(),
                    body: format!("Devin auth {}: {}", auth_status, String::from_utf8_lossy(&auth_bytes)),
                    provider: Some("devin".into()), request_id: None,
                }));
            }
            let auth_decoded: GetUserJwtResponse = {
                let data = if auth_bytes.len() >= 2 && auth_bytes[0] == 0x1f && auth_bytes[1] == 0x8b {
                    let mut decoder = GzDecoder::new(&auth_bytes[..]);
                    let mut buf = Vec::new();
                    decoder.read_to_end(&mut buf)
                        .map_err(|e| ProviderError::InvalidResponse(format!("Devin auth decompress: {e}")))?;
                    Bytes::from(buf)
                } else { auth_bytes };
                GetUserJwtResponse::decode(data)
                    .map_err(|e| ProviderError::InvalidResponse(format!("Devin auth decode: {e}")))?
            };
            if auth_decoded.user_jwt.is_empty() {
                return Err(ProviderError::InvalidResponse("Devin auth: empty user JWT".into()));
            }
            let chat_base_url = if auth_decoded.custom_api_server_url.trim().is_empty() {
                base_url.clone()
            } else {
                auth_decoded.custom_api_server_url.trim_end_matches('/').to_string()
            };

            // 3. Build chat request
            let cascade_id = Uuid::new_v4().to_string();
            let system_prompt = context_clone.system_prompt.as_deref().unwrap_or_default();
            let messages = &context_clone.messages;
            let chat_prompts = build_chat_message_prompts(messages, &cascade_id);
            let tools: Vec<ChatToolDefinition> = context_clone.tools.iter().map(|t| {
                ChatToolDefinition {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    json_schema_string: "{}".to_string(),
                    strict: false,
                }
            }).collect();
            let max_tokens = options.as_ref().and_then(|o| o.max_tokens).unwrap_or(64000) as u64;
            let temperature = options.as_ref().and_then(|o| o.temperature).unwrap_or(0.4);
            let chat_request = GetChatMessageRequest {
                metadata: Some(Metadata {
                    api_key: api_key.clone(), ide_name: "windsurf".into(),
                    ide_version: DEVIN_IDE_VERSION.into(), extension_name: "windsurf".into(),
                    extension_version: DEVIN_EXTENSION_VERSION.into(), locale: "en".into(),
                    user_jwt: auth_decoded.user_jwt.clone(),
                }),
                prompt: system_prompt.to_string(),
                chat_message_prompts: chat_prompts,
                chat_model_uid: model_id.clone(),
                request_type: 3,
                configuration: Some(CompletionConfiguration {
                    num_completions: 1, max_tokens, max_newlines: 200,
                    temperature, top_k: 50, top_p: 1.0,
                    stop_patterns: DEVIN_DEFAULT_STOP_PATTERNS.iter().map(|s| s.to_string()).collect(),
                    fim_eot_prob_threshold: 1.0, first_temperature: temperature,
                }),
                tools, disable_parallel_tool_calls: true, cascade_id, planner_mode: 0,
            };
            let chat_body = chat_request.encode_to_vec();
            let framed_body = build_connect_frame(&chat_body, true);

            // 4. Send chat request
            let response = client.post(format!("{}{}", chat_base_url, CHAT_MESSAGE_PATH))
                .header("content-type", "application/connect+proto")
                .header("connect-protocol-version", "1")
                .header("connect-content-encoding", "gzip")
                .header("accept-encoding", "identity")
                .header("user-agent", "connect-go/1.18.1 (go1.26.3)")
                .header("connect-accept-encoding", "gzip")
                .body(framed_body).send().await
                .map_err(|e| ProviderError::NetworkError(format!("Devin chat: {e}")))?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(ProviderError::HttpError(crate::HttpErrorDetail {
                    status: status.as_u16(), body, provider: Some("devin".into()), request_id: None,
                }));
            }

            // 5. Stream response via channel
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ProviderEvent>();

            tokio::spawn(async move {
                let mut byte_stream = response.bytes_stream();
                let mut pending = Vec::new();
                let mut output = AssistantMessage::new(Api::DevinAgent, "devin".to_string(), model_id.clone());
                let mut text_buf = String::new();
                let mut thinking_buf = String::new();
                let mut active_tool_call_id: Option<String> = None;
                let mut tool_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, args_json)
                let mut tool_json_accum: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                let mut latest_stop_reason = 0i32;
                let text_index = 0usize;
                let thinking_index = 0usize;

                let _ = tx.send(ProviderEvent::Start { partial: Arc::new(output.clone()) });

                'read: loop {
                    if pending.len() < FRAME_HEADER_SIZE {
                        match byte_stream.next().await {
                            Some(Ok(chunk)) => pending.extend_from_slice(&chunk),
                            Some(Err(e)) => {
                                output.error_message = Some(format!("Devin stream error: {e}"));
                                break 'read;
                            }
                            None => break 'read,
                        }
                    }
                    let (frames, rest) = parse_connect_frames(&pending);
                    pending = rest;
                    for frame in frames {
                        if frame.flags & CONNECT_END_STREAM_FLAG != 0 {
                            let trailer_text = String::from_utf8(frame.payload.to_vec())
                                .ok()
                                .and_then(|t| parse_connect_trailer(&t));
                            if let Some(e) = trailer_text {
                                output.error_message = Some(e);
                            }
                            break 'read;
                        }
                        let raw = if frame.flags & CONNECT_COMPRESSED_FLAG != 0 {
                            let mut decoder = GzDecoder::new(&frame.payload[..]);
                            let mut buf = Vec::new();
                            if decoder.read_to_end(&mut buf).is_err() { continue; }
                            Bytes::from(buf)
                        } else { frame.payload };
                        let msg = match GetChatMessageResponse::decode(raw) {
                            Ok(m) => m, Err(_) => continue,
                        };

                        if !msg.message_id.is_empty() {
                            output.response_id = Some(msg.message_id);
                        }

                        // Thinking delta
                        if !msg.delta_thinking.is_empty() {
                            if thinking_buf.is_empty() {
                                let _ = tx.send(ProviderEvent::ThinkingStart {
                                    content_index: thinking_index,
                                    partial: Arc::new(output.clone()),
                                });
                            }
                            thinking_buf.push_str(&msg.delta_thinking);
                            let _ = tx.send(ProviderEvent::ThinkingDelta {
                                content_index: thinking_index,
                                delta: msg.delta_thinking.clone(),
                                partial: Arc::new(output.clone()),
                            });
                        }

                        // Text delta — flush thinking first
                        if !msg.delta_text.is_empty() {
                            if !thinking_buf.is_empty() {
                                output.content.push(ContentBlock::Thinking(ThinkingContent {
                                    content_type: ThinkingContentType::Thinking,
                                    thinking: std::mem::take(&mut thinking_buf),
                                    thinking_signature: None,
                                    redacted: None,
                                }));
                                let _ = tx.send(ProviderEvent::ThinkingEnd {
                                    content_index: thinking_index,
                                    content: String::new(),
                                    partial: Arc::new(output.clone()),
                                });
                            }
                            if text_buf.is_empty() {
                                let _ = tx.send(ProviderEvent::TextStart {
                                    content_index: text_index,
                                    partial: Arc::new(output.clone()),
                                });
                            }
                            text_buf.push_str(&msg.delta_text);
                            let _ = tx.send(ProviderEvent::TextDelta {
                                content_index: text_index,
                                delta: msg.delta_text,
                                partial: Arc::new(output.clone()),
                            });
                        }

                        // Tool call deltas
                        for tc in &msg.delta_tool_calls {
                            let tid = if tc.id.is_empty() {
                                active_tool_call_id.get_or_insert_with(|| Uuid::new_v4().to_string()).clone()
                            } else {
                                active_tool_call_id = Some(tc.id.clone());
                                tc.id.clone()
                            };
                            let acc = tool_json_accum.entry(tid.clone()).or_default();
                            if !tc.arguments_json.is_empty() {
                                acc.push_str(&tc.arguments_json);
                            }
                            if !tool_calls.iter().any(|(id, _, _)| id == &tid) {
                                tool_calls.push((tid.clone(), tc.name.clone(), String::new()));
                                let _ = tx.send(ProviderEvent::ToolCallStart {
                                    content_index: output.content.len(),
                                    tool_call_id: Some(tid),
                                    tool_name: if tc.name.is_empty() { None } else { Some(tc.name.clone()) },
                                    partial: Arc::new(output.clone()),
                                });
                            }
                        }

                        if msg.stop_reason != 0 {
                            latest_stop_reason = msg.stop_reason;
                        }
                        if let Some(usage) = &msg.usage {
                            let input = usage.input_tokens as usize;
                            let c_read = usage.cache_read_tokens as usize;
                            let c_write = usage.cache_write_tokens as usize;
                            let output_t = usage.output_tokens as usize;
                            let total = input + output_t + c_read + c_write;
                            output.usage = Usage {
                                input,
                                output: output_t,
                                cache_read: c_read,
                                cache_write: c_write,
                                total_tokens: total,
                                cost: crate::Cost::default(),
                            };
                        }
                    }
                }

                // Finalize output
                if !thinking_buf.is_empty() {
                    output.content.push(ContentBlock::Thinking(ThinkingContent {
                        content_type: ThinkingContentType::Thinking,
                        thinking: std::mem::take(&mut thinking_buf),
                        thinking_signature: None,
                        redacted: None,
                    }));
                }
                if !text_buf.is_empty() {
                    output.content.push(ContentBlock::Text(TextContent {
                        content_type: TextContentType::Text,
                        text: std::mem::take(&mut text_buf),
                        text_signature: None,
                    }));
                }
                for (tid, _name, _args_json) in &tool_calls {
                    let parsed_args = tool_json_accum.get(tid)
                        .and_then(|j| serde_json::from_str(j).ok())
                        .unwrap_or_default();
                    output.content.push(ContentBlock::ToolCall(ToolCall {
                        content_type: ToolCallType::ToolCall,
                        id: tid.clone(),
                        name: String::new(),
                        arguments: parsed_args,
                        thought_signature: None,
                    }));
                }
                let reason = match latest_stop_reason {
                    1 => StopReason::Length,
                    _ if !tool_calls.is_empty() => StopReason::ToolUse,
                    _ => StopReason::Stop,
                };
                output.stop_reason = reason;
                let _ = tx.send(ProviderEvent::Done {
                    message: output, reason,
                });
            });

            Ok(Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
        })
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn normalize_devin_session_token(api_key: String) -> String {
    if api_key.starts_with(DEVIN_SESSION_TOKEN_PREFIX) { api_key }
    else { format!("{}{}", DEVIN_SESSION_TOKEN_PREFIX, api_key) }
}

fn build_chat_message_prompts(messages: &[crate::Message], _cascade_id: &str) -> Vec<ChatMessagePrompt> {
    let mut prompts = Vec::new();
    for msg in messages.iter() {
        let message_id = Uuid::new_v4().to_string();

        match msg {
            crate::Message::User(_m) => {
                let prompt_text = msg.text_content().unwrap_or_default();
                prompts.push(ChatMessagePrompt {
                    message_id, source: ChatMessageSource::User as i32,
                    prompt: prompt_text, thinking: String::new(),
                    signature: String::new(), tool_calls: Vec::new(),
                    tool_call_id: String::new(), tool_result_is_error: false,
                    signature_type: String::new(),
                });
            }
            crate::Message::Assistant(m) => {
                let mut prompt_text = String::new();
                let mut thinking = String::new();
                let mut tool_calls = Vec::new();
                for block in &m.content {
                    match block {
                        ContentBlock::Text(t) => prompt_text.push_str(&t.text),
                        ContentBlock::Thinking(t) => thinking.push_str(&t.thinking),
                        ContentBlock::ToolCall(tc) => {
                            tool_calls.push(ChatToolCall {
                                id: tc.id.clone(), name: tc.name.clone(),
                                arguments_json: serde_json::to_string(&tc.arguments).unwrap_or_default(),
                            });
                        }
                        _ => {}
                    }
                }
                if prompt_text.is_empty() && thinking.is_empty() && tool_calls.is_empty() {
                    continue;
                }
                prompts.push(ChatMessagePrompt {
                    message_id, source: ChatMessageSource::System as i32,
                    prompt: prompt_text, thinking, signature: String::new(),
                    tool_calls, tool_call_id: String::new(),
                    tool_result_is_error: false, signature_type: String::new(),
                });
            }
            crate::Message::ToolResult(m) => {
                let result_text = msg.text_content().unwrap_or_default();
                let tid = m.tool_call_id.clone();
                prompts.push(ChatMessagePrompt {
                    message_id: Uuid::new_v4().to_string(),
                    source: ChatMessageSource::Tool as i32,
                    prompt: result_text, thinking: String::new(),
                    signature: String::new(), tool_calls: Vec::new(),
                    tool_call_id: tid.clone(), tool_result_is_error: m.is_error,
                    signature_type: String::new(),
                });
            }
        }
    }
    prompts
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_provider_creation() {
        let _ = DevinProvider::new();
    }

    // ── Connect framing tests ────────────────────────────────────────

    #[test]
    fn test_frame_roundtrip_uncompressed() {
        let payload = b"hello world";
        let raw = build_connect_frame(payload, false);
        let (frames, rest) = parse_connect_frames(&raw);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].flags, 0);
        assert_eq!(&frames[0].payload[..], payload);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_frame_roundtrip_compressed() {
        let payload = b"this is a longer payload that will compress well";
        let raw = build_connect_frame(payload, true);
        assert_eq!(raw[0], CONNECT_COMPRESSED_FLAG);
        // gzip overhead may exceed payload for small data; verify roundtrip correctness instead
        let (frames, rest) = parse_connect_frames(&raw);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].flags & CONNECT_COMPRESSED_FLAG != 0);
        let mut decoder = GzDecoder::new(&frames[0].payload[..]);
        let mut buf = Vec::new();
        decoder.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, payload);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_frame_parsing_partial_header() {
        let (frames, rest) = parse_connect_frames(&[0x00, 0x00, 0x00]);
        assert_eq!(frames.len(), 0);
        assert_eq!(rest.len(), 3);
    }

    #[test]
    fn test_frame_parsing_partial_payload() {
        let mut raw = vec![0x00, 0x00, 0x00, 0x00, 0x0A];
        raw.extend_from_slice(b"hel");
        let (frames, rest) = parse_connect_frames(&raw);
        assert_eq!(frames.len(), 0);
        assert_eq!(rest.len(), 8);
    }

    #[test]
    fn test_frame_multi_frame() {
        let mut raw = build_connect_frame(b"first", false);
        raw.extend_from_slice(&build_connect_frame(b"second", false));
        let (frames, rest) = parse_connect_frames(&raw);
        assert_eq!(frames.len(), 2);
        assert_eq!(&frames[0].payload[..], b"first");
        assert_eq!(&frames[1].payload[..], b"second");
        assert!(rest.is_empty());
    }

    #[test]
    fn test_connect_trailer_parse() {
        let err = parse_connect_trailer("{\"error\":{\"code\":\"canceled\",\"message\":\"canceled\"}}");
        assert!(err.is_some());
        assert!(err.unwrap().contains("canceled"));
    }

    #[test]
    fn test_connect_trailer_empty() {
        assert!(parse_connect_trailer("").is_none());
    }

    #[test]
    fn test_connect_trailer_no_error() {
        assert!(parse_connect_trailer("{}").is_none());
    }

    // ── Protobuf roundtrip tests ─────────────────────────────────────

    #[test]
    fn test_metadata_roundtrip() {
        let meta = Metadata {
            api_key: "sk-key".into(), ide_name: "windsurf".into(),
            ide_version: "1.0".into(), extension_name: "windsurf".into(),
            extension_version: "1.0".into(), locale: "en".into(),
            user_jwt: "jwt".into(),
        };
        let enc = meta.encode_to_vec();
        let dec = Metadata::decode(Bytes::from(enc)).unwrap();
        assert_eq!(dec.api_key, "sk-key");
    }

    #[test]
    fn test_get_user_jwt_request_roundtrip() {
        let req = GetUserJwtRequest {
            metadata: Some(Metadata {
                api_key: "key".into(), ide_name: "ws".into(),
                ide_version: "1".into(), extension_name: "ws".into(),
                extension_version: "1".into(), locale: "en".into(),
                user_jwt: String::new(),
            }),
        };
        let enc = req.encode_to_vec();
        let dec = GetUserJwtRequest::decode(Bytes::from(enc)).unwrap();
        assert_eq!(dec.metadata.unwrap().api_key, "key");
    }

    #[test]
    fn test_chat_tool_call_roundtrip() {
        let tc = ChatToolCall {
            id: "call_1".into(), name: "bash".into(),
            arguments_json: "{\"command\":\"ls\"}".into(),
        };
        let enc = tc.encode_to_vec();
        let dec = ChatToolCall::decode(Bytes::from(enc)).unwrap();
        assert_eq!(dec.id, "call_1");
        assert_eq!(dec.name, "bash");
        assert_eq!(dec.arguments_json, "{\"command\":\"ls\"}");
    }

    #[test]
    fn test_get_chat_message_request_roundtrip() {
        let request = GetChatMessageRequest {
            metadata: Some(Metadata {
                api_key: "k".into(), ide_name: "ws".into(),
                ide_version: "1".into(), extension_name: "ws".into(),
                extension_version: "1".into(), locale: "en".into(),
                user_jwt: "j".into(),
            }),
            prompt: "system prompt".into(),
            chat_message_prompts: vec![
                ChatMessagePrompt {
                    message_id: "m1".into(), source: ChatMessageSource::User as i32,
                    prompt: "hi".into(), thinking: String::new(),
                    signature: String::new(), tool_calls: Vec::new(),
                    tool_call_id: String::new(), tool_result_is_error: false,
                    signature_type: String::new(),
                },
            ],
            chat_model_uid: "model-1".into(),
            request_type: 3,
            configuration: Some(CompletionConfiguration {
                num_completions: 1, max_tokens: 4096, max_newlines: 200,
                temperature: 0.4, top_k: 50, top_p: 1.0,
                stop_patterns: vec!["<|end|>".into()],
                fim_eot_prob_threshold: 1.0, first_temperature: 0.4,
            }),
            tools: vec![ChatToolDefinition {
                name: "read".into(), description: "Read".into(),
                json_schema_string: "{}".into(), strict: false,
            }],
            disable_parallel_tool_calls: true,
            cascade_id: "c-1".into(),
            planner_mode: 0,
        };
        let enc = request.encode_to_vec();
        assert!(!enc.is_empty());
        let dec = GetChatMessageRequest::decode(Bytes::from(enc)).unwrap();
        assert_eq!(dec.prompt, "system prompt");
        assert_eq!(dec.chat_message_prompts.len(), 1);
        assert_eq!(dec.chat_message_prompts[0].prompt, "hi");
        assert_eq!(dec.tools.len(), 1);
        assert_eq!(dec.cascade_id, "c-1");
    }

    #[test]
    fn test_get_chat_message_response_roundtrip() {
        let response = GetChatMessageResponse {
            message_id: "r1".into(),
            delta_text: "hello".into(),
            stop_reason: 0,
            delta_tool_calls: vec![ChatToolCall {
                id: "tc1".into(), name: "bash".into(),
                arguments_json: "{\"cmd\":\"ls\"}".into(),
            }],
            usage: Some(ModelUsageStats {
                input_tokens: 10, output_tokens: 5,
                cache_read_tokens: 2, cache_write_tokens: 1,
            }),
            delta_thinking: "thinking...".into(),
            delta_signature: "sig".into(),
        };
        let enc = response.encode_to_vec();
        assert!(!enc.is_empty());
        let dec = GetChatMessageResponse::decode(Bytes::from(enc)).unwrap();
        assert_eq!(dec.message_id, "r1");
        assert_eq!(dec.delta_text, "hello");
        assert_eq!(dec.delta_thinking, "thinking...");
        assert_eq!(dec.delta_tool_calls.len(), 1);
        assert_eq!(dec.delta_tool_calls[0].name, "bash");
        let usage = dec.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_frame_with_protobuf_payload() {
        let original = GetChatMessageResponse {
            message_id: "f1".into(), delta_text: "hi".into(),
            stop_reason: 0, delta_tool_calls: Vec::new(),
            usage: Some(ModelUsageStats {
                input_tokens: 5, output_tokens: 3,
                cache_read_tokens: 0, cache_write_tokens: 0,
            }),
            delta_thinking: String::new(), delta_signature: String::new(),
        };
        let protobuf_bytes = original.encode_to_vec();
        let framed = build_connect_frame(&protobuf_bytes, true);
        let (frames, rest) = parse_connect_frames(&framed);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].flags & CONNECT_COMPRESSED_FLAG != 0);
        assert!(rest.is_empty());
        let mut decoder = GzDecoder::new(&frames[0].payload[..]);
        let mut buf = Vec::new();
        decoder.read_to_end(&mut buf).unwrap();
        let decoded = GetChatMessageResponse::decode(Bytes::from(buf)).unwrap();
        assert_eq!(decoded.message_id, "f1");
        assert_eq!(decoded.delta_text, "hi");
    }

    #[test]
    fn test_normalize_token() {
        assert_eq!(
            normalize_devin_session_token("tok".to_string()),
            "devin-session-token$tok"
        );
        assert_eq!(
            normalize_devin_session_token("devin-session-token$existing".to_string()),
            "devin-session-token$existing"
        );
    }
}
