//! Cursor provider — remote-AGENT protocol via HTTP/2 + Connect + Protobuf.
//!
//! Port of omp `packages/ai/src/providers/cursor.ts` (3396 lines).
//!
//! Cursor's Run RPC (`/agent.v1.AgentService/Run`) is a **bidirectional**
//! HTTP/2 stream using the Connect streaming protocol with protobuf-encoded
//! [`AgentClientMessage`] / [`AgentServerMessage`] frames.
//!
//! ## Protocol
//! - HTTP/2 transport (negotiated via TLS ALPN; `reqwest` with `rustls-tls`)
//! - `application/connect+proto` content type, `connect-protocol-version: 1`
//! - Binary framing: 1 flag byte + 4-byte big-endian length + payload
//!   (identical envelope to Devin; flag bit 0x01 = gzip, 0x02 = end-stream)
//! - **Bidirectional**: after the initial [`AgentRunRequest`] frame, the server
//!   sends [`KvServerMessage`] (blob fetch/store) and [`ExecServerMessage`]
//!   (tool execution) requests that the client must answer on the same stream.
//!
//! ## Blob store
//! Conversation state references large payloads (system prompt JSON, history
//! turns) by SHA-256 blob ID. The server resolves these by sending
//! `GetBlobArgs { blob_id }` over the KV channel; the client responds with
//! `GetBlobResult { blob_data }`. Without this handshake the server cannot
//! read the request and the turn never starts.
//!
//! ## Scope (MVP bridge)
//! Text + thinking streaming is fully bridged to [`ProviderEvent`]. The KV
//! channel is fully implemented (mandatory). Cursor's **exec channel** — where
//! the server asks the client to run native tools (bash/read/write/…) and feed
//! results back — is rejected with [`ExecClientControlMessage::Throw`] because
//! oxi-ai's [`Provider`] trait is unidirectional: tool calls surface as
//! [`ProviderEvent::ToolCallEnd`] for the host agent loop to execute via its
//! own registry, with results re-injected as context on the next turn. Native
//! server-side tool execution is therefore not bridged in this provider.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use flate2::read::GzDecoder;
use futures::{Stream, StreamExt};
use prost::Message as ProstMessage;
use sha2::{Digest, Sha256};
use std::io::Read;
use uuid::Uuid;

use super::shared_client;
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Message, MessageContent, Model, Provider,
    ProviderEvent, StopReason, StreamOptions, StreamResult, ToolCall, ToolCallType, Usage,
    error::ProviderError,
};

// Generated proto types (package `agent.v1`, 492 messages) live in OUT_DIR.
// `include!` pulls all top-level items + nested oneof modules into this scope.
#[allow(
    clippy::all,
    clippy::pedantic,
    dead_code,
    unused_imports,
    unused_variables
)]
mod proto {
    include!(concat!(env!("OUT_DIR"), "/agent.v1.rs"));
}

use proto::{
    AgentClientMessage, AgentRunRequest, AgentServerMessage, ConversationAction,
    ConversationStateStructure, ExecClientControlMessage, ExecClientThrow, GetBlobResult,
    KvClientMessage, ModelDetails, RequestedModel, UserMessage as CursorUserMessage,
    UserMessageAction, agent_client_message, agent_server_message, conversation_action,
    interaction_update, kv_server_message,
};

// ── Constants ───────────────────────────────────────────────────────

const CURSOR_API_URL: &str = "https://api2.cursor.sh";
const AGENT_RUN_PATH: &str = "/agent.v1.AgentService/Run";
const CURSOR_CLIENT_VERSION: &str = "cli-2026.01.09-231024f";
const CONNECT_COMPRESSED_FLAG: u8 = 0x01;
const CONNECT_END_STREAM_FLAG: u8 = 0x02;
const MAX_CONNECT_FRAME_PAYLOAD: usize = 32 * 1024 * 1024;
const FRAME_HEADER_SIZE: usize = 5;

// ── Connect protocol framing (same envelope as Devin) ───────────────

struct ConnectFrame {
    flags: u8,
    payload: Bytes,
}

fn build_connect_frame(payload: &[u8]) -> Vec<u8> {
    // Requests are sent uncompressed; gzip inflates request size for small
    // payloads and the server accepts identity. Response frames may still be
    // gzip-compressed (flag 0x01) and are decompressed on read.
    let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE + payload.len());
    frame.push(0); // flags: no compression
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
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
            payload: Bytes::copy_from_slice(
                &buffer[offset + FRAME_HEADER_SIZE..offset + FRAME_HEADER_SIZE + len],
            ),
        });
        offset += FRAME_HEADER_SIZE + len;
    }
    (frames, buffer[offset..].to_vec())
}

fn decompress_frame(frame: &ConnectFrame) -> Option<Bytes> {
    if frame.flags & CONNECT_COMPRESSED_FLAG != 0 {
        let mut decoder = GzDecoder::new(&frame.payload[..]);
        let mut buf = Vec::new();
        decoder.read_to_end(&mut buf).ok()?;
        Some(Bytes::from(buf))
    } else {
        Some(frame.payload.clone())
    }
}

fn parse_connect_trailer(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    let err = parsed.get("error")?;
    let code = err.get("code")?.as_str()?;
    let message = err.get("message")?.as_str()?;
    Some(format!("Cursor stream error {code}: {message}"))
}

// ── Blob store (SHA-256 keyed) ──────────────────────────────────────

/// Content-addressed blob store. Keys are SHA-256 digests of the stored
/// bytes; the same digest is the blob ID referenced by `ConversationState`.
#[derive(Default)]
struct BlobStore(HashMap<Vec<u8>, Vec<u8>>);

impl BlobStore {
    /// Store `data`, returning its SHA-256 blob ID.
    fn store(&mut self, data: Vec<u8>) -> Vec<u8> {
        let id = Sha256::digest(&data).to_vec();
        self.0.entry(id.clone()).or_insert(data);
        id
    }

    /// Look up a blob by ID.
    fn get(&self, id: &[u8]) -> Option<&Vec<u8>> {
        self.0.get(id)
    }
}

// ── Request builder ─────────────────────────────────────────────────

/// Extract text from a user message (string or content-block form).
fn user_text(msg: &crate::UserMessage) -> String {
    match &msg.content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Extract text from an assistant message (text blocks only).
fn assistant_text(msg: &crate::AssistantMessage) -> String {
    msg.content
        .iter()
        .filter_map(|b| b.as_text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract text from a tool-result message.
fn tool_result_text(msg: &crate::ToolResultMessage) -> String {
    msg.content
        .iter()
        .filter_map(|b| b.as_text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the conversation-state JSON blobs + turn blobs from context, returning
/// the encoded [`AgentClientMessage`] bytes plus the populated blob store.
///
/// Mirrors omp `buildGrpcRequest` + `buildRootPromptMessagesJson` +
/// `buildConversationTurns`:
/// - System prompt(s) → one JSON `{role:"system",content}` blob each.
/// - Prior user/assistant/tool-result messages → JSON blobs in Cursor's
///   Vercel-AI-SDK root-prompt format.
/// - History turns → serialized [`ConversationTurnStructure`] blobs.
/// - Active (last) user message → the request action.
fn build_run_request(
    model: &Model,
    context: &Context,
    conversation_id: &str,
) -> (Vec<u8>, BlobStore) {
    let mut blobs = BlobStore::default();

    // 1. System prompt JSON blobs (head of root_prompt_messages_json).
    let mut root_prompt_ids: Vec<Vec<u8>> = Vec::new();
    let system = context
        .system_prompt
        .as_deref()
        .unwrap_or("You are a helpful assistant.");
    for line in system.lines().filter(|l| !l.is_empty()) {
        let json = format!(
            r#"{{"role":"system","content":{}}}"#,
            serde_json::Value::from(line)
        );
        root_prompt_ids.push(blobs.store(json.into_bytes()));
    }
    if root_prompt_ids.is_empty() {
        let json = r#"{"role":"system","content":"You are a helpful assistant."}"#;
        root_prompt_ids.push(blobs.store(json.as_bytes().to_vec()));
    }

    // 2. Locate the active user message (last message if it's a user turn).
    let messages = &context.messages;
    let active_idx = if matches!(messages.last(), Some(Message::User(_))) {
        messages.len().saturating_sub(1)
    } else {
        usize::MAX // no active user message → resume action
    };

    // 3. Root-prompt history JSON blobs (everything before the active message).
    for (i, msg) in messages.iter().enumerate() {
        if i == active_idx {
            break;
        }
        let json = match msg {
            Message::User(u) => {
                let text = user_text(u);
                if text.trim().is_empty() {
                    continue;
                }
                format!(
                    r#"{{"role":"user","content":[{{"type":"text","text":{}}}]}}"#,
                    serde_json::Value::from(text)
                )
            }
            Message::Assistant(a) => {
                let text = assistant_text(a);
                if text.is_empty() {
                    continue;
                }
                format!(
                    r#"{{"role":"assistant","content":[{{"type":"text","text":{}}}]}}"#,
                    serde_json::Value::from(text)
                )
            }
            Message::ToolResult(t) => {
                let text = tool_result_text(t);
                if text.is_empty() {
                    continue;
                }
                let prefix = if t.is_error {
                    "[Tool Error]"
                } else {
                    "[Tool Result]"
                };
                format!(
                    r#"{{"role":"user","content":[{{"type":"text","text":{}}}]}}"#,
                    serde_json::Value::from(format!("{prefix}\n{text}"))
                )
            }
        };
        root_prompt_ids.push(blobs.store(json.into_bytes()));
    }

    // 4. Conversation turns (grouped user+steps → ConversationTurnStructure blobs).
    let mut turns: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let msg = &messages[i];
        let user_msg = match msg {
            Message::User(u) => u,
            _ => {
                i += 1;
                continue;
            }
        };
        if i == active_idx {
            break; // active user message goes in the action, not turns
        }
        let text = user_text(user_msg);
        if text.trim().is_empty() {
            i += 1;
            continue;
        }

        let cursor_user = CursorUserMessage {
            text: text.clone(),
            message_id: Uuid::new_v4().to_string(),
            selected_context: None,
            mode: 0,
            is_simulated_msg: None,
            best_of_n_group_id: None,
            try_use_best_of_n_promotion: None,
            rich_text: None,
        };
        let user_blob = blobs.store(cursor_user.encode_to_vec());

        // Collect steps (assistant + tool results) until the next user message.
        let mut step_ids: Vec<Vec<u8>> = Vec::new();
        i += 1;
        while i < messages.len() && !matches!(messages[i], Message::User(_)) {
            if i == active_idx {
                break;
            }
            match &messages[i] {
                Message::Assistant(a) => {
                    let t = assistant_text(a);
                    if !t.is_empty() {
                        let step = proto::ConversationStep {
                            message: Some(proto::conversation_step::Message::AssistantMessage(
                                proto::AssistantMessage { text: t },
                            )),
                        };
                        step_ids.push(blobs.store(step.encode_to_vec()));
                    }
                }
                Message::ToolResult(tr) => {
                    let t = tool_result_text(tr);
                    if !t.is_empty() {
                        let prefix = if tr.is_error {
                            "[Tool Error]"
                        } else {
                            "[Tool Result]"
                        };
                        let step = proto::ConversationStep {
                            message: Some(proto::conversation_step::Message::AssistantMessage(
                                proto::AssistantMessage {
                                    text: format!("{prefix}\n{t}"),
                                },
                            )),
                        };
                        step_ids.push(blobs.store(step.encode_to_vec()));
                    }
                }
                _ => {}
            }
            i += 1;
        }

        let agent_turn = proto::AgentConversationTurnStructure {
            user_message: user_blob,
            steps: step_ids,
            request_id: None,
        };
        let turn = proto::ConversationTurnStructure {
            turn: Some(proto::conversation_turn_structure::Turn::AgentConversationTurn(agent_turn)),
        };
        turns.push(blobs.store(turn.encode_to_vec()));
    }

    // 5. Build the conversation state.
    let conversation_state = ConversationStateStructure {
        root_prompt_messages_json: root_prompt_ids,
        turns,
        ..Default::default()
    };

    // 6. Build the action (user_message_action or resume_action).
    let action = if active_idx < messages.len() {
        let active_text = match &messages[active_idx] {
            Message::User(u) => user_text(u),
            _ => String::new(),
        };
        ConversationAction {
            action: Some(conversation_action::Action::UserMessageAction(
                UserMessageAction {
                    user_message: Some(CursorUserMessage {
                        text: active_text,
                        message_id: Uuid::new_v4().to_string(),
                        ..Default::default()
                    }),
                    request_context: None,
                    send_to_interaction_listener: None,
                },
            )),
        }
    } else {
        ConversationAction {
            action: Some(conversation_action::Action::ResumeAction(
                proto::ResumeAction {
                    request_context: None,
                },
            )),
        }
    };

    // 7. Model details.
    let model_details = ModelDetails {
        model_id: model.id.clone(),
        display_model_id: model.id.clone(),
        display_name: model.name.clone(),
        ..Default::default()
    };
    let requested_model = RequestedModel {
        model_id: model.id.clone(),
        ..Default::default()
    };

    let run_request = AgentRunRequest {
        conversation_state: Some(conversation_state),
        action: Some(action),
        model_details: Some(model_details),
        requested_model: Some(requested_model),
        conversation_id: Some(conversation_id.to_string()),
        ..Default::default()
    };

    let client_message = AgentClientMessage {
        message: Some(agent_client_message::Message::RunRequest(run_request)),
    };

    (client_message.encode_to_vec(), blobs)
}

// ── Tool call mapping (proto ToolCall → name + JSON args) ───────────

/// Map a Cursor `ToolCall` proto onto a tool name + JSON arguments.
///
/// Handles the common native tools (shell/read/write/edit/grep/ls/delete).
/// Unknown variants fall back to `"unknown"` with the raw args omitted.
fn map_tool_call(tc: &proto::ToolCall) -> (String, serde_json::Value) {
    use proto::tool_call::Tool;
    // Tool-arg messages are `Option<…>` on the wire; map present args to JSON.
    match &tc.tool {
        Some(Tool::ShellToolCall(c)) => {
            let a = c.args.as_ref();
            (
                "bash".into(),
                serde_json::json!({
                    "command": a.map(|a| a.command.clone()).unwrap_or_default(),
                    "working_directory": a.map(|a| a.working_directory.clone()).unwrap_or_default(),
                }),
            )
        }
        Some(Tool::ReadToolCall(c)) => (
            "read".into(),
            serde_json::json!({
                "path": c.args.as_ref().map(|a| a.path.clone()).unwrap_or_default(),
            }),
        ),
        Some(Tool::EditToolCall(c)) => (
            "edit".into(),
            serde_json::json!({
                "path": c.args.as_ref().map(|a| a.path.clone()).unwrap_or_default(),
            }),
        ),
        Some(Tool::GrepToolCall(c)) => {
            let a = c.args.as_ref();
            (
                "grep".into(),
                serde_json::json!({
                    "pattern": a.map(|a| a.pattern.clone()).unwrap_or_default(),
                    "path": a.and_then(|a| a.path.clone()).unwrap_or_default(),
                    "glob": a.and_then(|a| a.glob.clone()).unwrap_or_default(),
                }),
            )
        }
        Some(Tool::LsToolCall(c)) => (
            "ls".into(),
            serde_json::json!({
                "path": c.args.as_ref().map(|a| a.path.clone()).unwrap_or_default(),
            }),
        ),
        Some(Tool::DeleteToolCall(c)) => (
            "delete".into(),
            serde_json::json!({
                "path": c.args.as_ref().map(|a| a.path.clone()).unwrap_or_default(),
            }),
        ),
        Some(Tool::GlobToolCall(_)) => ("glob".into(), serde_json::json!({})),
        Some(Tool::WebSearchToolCall(c)) => (
            "web_search".into(),
            serde_json::json!({
                "query": c.args.as_ref().map(|a| a.search_term.clone()).unwrap_or_default(),
            }),
        ),
        Some(Tool::McpToolCall(_)) => ("mcp".into(), serde_json::json!({})),
        Some(Tool::FetchToolCall(_)) => ("fetch".into(), serde_json::json!({})),
        Some(Tool::TaskToolCall(_)) => ("task".into(), serde_json::json!({})),
        _ => ("unknown".into(), serde_json::json!({})),
    }
}

// ── Cursor Provider ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct CursorProvider {
    client: &'static reqwest::Client,
    base_url: String,
}

impl CursorProvider {
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            base_url: CURSOR_API_URL.to_string(),
        }
    }
    #[allow(dead_code)]
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: shared_client(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

impl Default for CursorProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for CursorProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        let client = self.client;
        let base_url = self.base_url.clone();
        let model_id = model.id.clone();
        let model_name = model.name.clone();
        let context_clone = context.clone();

        Box::pin(async move {
            // 1. Resolve API key.
            let api_key = options
                .as_ref()
                .and_then(|o| o.api_key.clone())
                .or_else(|| std::env::var("CURSOR_API_KEY").ok())
                .ok_or_else(|| {
                    ProviderError::InvalidResponse(
                        "Cursor API key required — set CURSOR_API_KEY".into(),
                    )
                })?;

            // 2. Build the run request + blob store.
            let conversation_id = options
                .as_ref()
                .and_then(|o| o.session_id.clone())
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            let (request_bytes, blob_store) =
                build_run_request(model, &context_clone, &conversation_id);
            let request_frame = build_connect_frame(&request_bytes);

            // 3. Set up the bidirectional HTTP/2 stream.
            //    frame_tx feeds the request body; the response reader clones it
            //    to answer KV/exec requests on the same connection.
            let (frame_tx, frame_rx) =
                tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
            frame_tx
                .send(Ok(Bytes::from(request_frame)))
                .map_err(|_| ProviderError::InvalidResponse("request channel closed".into()))?;

            // Channel yields `Result<Bytes, _>` directly — `Body::wrap_stream`
            // accepts any `TryStream<Ok = Bytes>`, so no `.map()` adapter is
            // needed (which would require a cross-crate Stream trait bridge).
            let body = reqwest::Body::wrap_stream(
                tokio_stream::wrappers::UnboundedReceiverStream::new(frame_rx),
            );

            let url = format!("{base_url}{AGENT_RUN_PATH}");
            let request_id = Uuid::new_v4().to_string();
            let response = client
                .post(&url)
                .header("content-type", "application/connect+proto")
                .header("connect-protocol-version", "1")
                .header("te", "trailers")
                .header("authorization", format!("Bearer {api_key}"))
                .header("x-ghost-mode", "true")
                .header("x-cursor-client-version", CURSOR_CLIENT_VERSION)
                .header("x-cursor-client-type", "cli")
                .header("x-request-id", &request_id)
                .body(body)
                .send()
                .await
                .map_err(|e| ProviderError::NetworkError(format!("Cursor connect failed: {e}")))?;

            if !response.status().is_success() {
                let status = response.status();
                let body_text = response.text().await.unwrap_or_default();
                return Err(ProviderError::HttpError(crate::HttpErrorDetail {
                    status: status.as_u16(),
                    body: format!("Cursor {status}: {body_text}"),
                    provider: Some("cursor".into()),
                    request_id: Some(request_id),
                }));
            }

            // 4. Spawn the response reader. It emits ProviderEvents and answers
            //    KV/exec requests by writing back through frame_tx.
            let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<ProviderEvent>();
            let blob_store = Arc::new(blob_store);

            tokio::spawn(async move {
                let mut byte_stream = response.bytes_stream();
                let mut pending = Vec::new();
                let mut output =
                    AssistantMessage::new(Api::CursorAgent, "cursor".to_string(), model_id.clone());
                output.model = model_name;
                let mut text_buf = String::new();
                let mut thinking_buf = String::new();
                let text_index = 0usize;
                let thinking_index = 1usize;
                let mut saw_turn_ended = false;
                let mut content_index = 0usize; // running index for tool-call blocks

                let _ = event_tx.send(ProviderEvent::Start {
                    partial: Arc::new(output.clone()),
                });

                'read: loop {
                    if pending.len() < FRAME_HEADER_SIZE {
                        match byte_stream.next().await {
                            Some(Ok(chunk)) => pending.extend_from_slice(&chunk),
                            Some(Err(e)) => {
                                output.error_message = Some(format!("Cursor stream error: {e}"));
                                break 'read;
                            }
                            None => break 'read,
                        }
                    }
                    let (frames, rest) = parse_connect_frames(&pending);
                    pending = rest;

                    for frame in frames {
                        // End-stream frame: JSON trailers (possibly an error).
                        if frame.flags & CONNECT_END_STREAM_FLAG != 0 {
                            if let Some(err_msg) = String::from_utf8(frame.payload.to_vec())
                                .ok()
                                .and_then(|t| parse_connect_trailer(&t))
                            {
                                output.error_message = Some(err_msg);
                            }
                            break 'read;
                        }
                        let raw = match decompress_frame(&frame) {
                            Some(r) => r,
                            None => continue,
                        };
                        let msg = match AgentServerMessage::decode(raw) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        let Some(server_msg) = msg.message else {
                            continue;
                        };

                        match server_msg {
                            agent_server_message::Message::InteractionUpdate(update) => {
                                handle_interaction_update(
                                    &update,
                                    &mut output,
                                    &event_tx,
                                    &mut text_buf,
                                    &mut thinking_buf,
                                    text_index,
                                    thinking_index,
                                    &mut content_index,
                                    &mut saw_turn_ended,
                                );
                            }
                            agent_server_message::Message::KvServerMessage(kv) => {
                                handle_kv_message(&kv, &blob_store, &frame_tx);
                            }
                            agent_server_message::Message::ExecServerMessage(exec) => {
                                // Reject native tool execution — the host agent loop
                                let _ = frame_tx.send(Ok(Bytes::from(build_connect_frame(
                                    &AgentClientMessage {
                                        message: Some(
                                            agent_client_message::Message::ExecClientControlMessage(
                                                ExecClientControlMessage {
                                                    message: Some(
                                                        proto::exec_client_control_message::Message::Throw(
                                                            ExecClientThrow {
                                                                id: exec.id,
                                                                error: "native tool execution not bridged".into(),
                                                                stack_trace: None,
                                                            },
                                                        ),
                                                    ),
                                                },
                                            ),
                                        ),
                                    }
                                    .encode_to_vec(),
                                ))));
                            }
                            agent_server_message::Message::ConversationCheckpointUpdate(_) => {
                                // State checkpoint — caching is per-session and out
                                // of scope for the stateless Provider trait.
                            }
                            _ => {} // exec control, interaction query: not bridged
                        }
                    }
                }

                // Finalize: flush any open text/thinking blocks.
                if !thinking_buf.is_empty() {
                    output
                        .content
                        .push(ContentBlock::Thinking(crate::ThinkingContent {
                            content_type: crate::ThinkingContentType::Thinking,
                            thinking: std::mem::take(&mut thinking_buf),
                            thinking_signature: None,
                            redacted: None,
                        }));
                }
                if !text_buf.is_empty() {
                    output.content.push(ContentBlock::Text(crate::TextContent {
                        content_type: crate::TextContentType::Text,
                        text: std::mem::take(&mut text_buf),
                        text_signature: None,
                    }));
                }
                let reason = if saw_turn_ended {
                    StopReason::Stop
                } else if output.error_message.is_some() {
                    StopReason::Error
                } else {
                    StopReason::Stop
                };
                output.stop_reason = reason;
                let _ = event_tx.send(ProviderEvent::Done {
                    message: output,
                    reason,
                });
            });

            Ok(
                Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(
                    event_rx,
                )) as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>,
            )
        })
    }
}

// ── InteractionUpdate → ProviderEvent ───────────────────────────────

#[allow(clippy::too_many_arguments)]
fn handle_interaction_update(
    update: &proto::InteractionUpdate,
    output: &mut AssistantMessage,
    event_tx: &tokio::sync::mpsc::UnboundedSender<ProviderEvent>,
    text_buf: &mut String,
    thinking_buf: &mut String,
    text_index: usize,
    thinking_index: usize,
    content_index: &mut usize,
    saw_turn_ended: &mut bool,
) {
    let Some(msg) = &update.message else { return };
    use interaction_update::Message;
    match msg {
        Message::TextDelta(d) => {
            // Flush thinking if a text block follows.
            if !thinking_buf.is_empty() && text_buf.is_empty() {
                output
                    .content
                    .push(ContentBlock::Thinking(crate::ThinkingContent {
                        content_type: crate::ThinkingContentType::Thinking,
                        thinking: std::mem::take(thinking_buf),
                        thinking_signature: None,
                        redacted: None,
                    }));
            }
            if text_buf.is_empty() {
                let _ = event_tx.send(ProviderEvent::TextStart {
                    content_index: text_index,
                    partial: Arc::new(output.clone()),
                });
            }
            text_buf.push_str(&d.text);
            let _ = event_tx.send(ProviderEvent::TextDelta {
                content_index: text_index,
                delta: d.text.clone(),
                partial: Arc::new(output.clone()),
            });
        }
        Message::ThinkingDelta(d) => {
            if thinking_buf.is_empty() {
                let _ = event_tx.send(ProviderEvent::ThinkingStart {
                    content_index: thinking_index,
                    partial: Arc::new(output.clone()),
                });
            }
            thinking_buf.push_str(&d.text);
            let _ = event_tx.send(ProviderEvent::ThinkingDelta {
                content_index: thinking_index,
                delta: d.text.clone(),
                partial: Arc::new(output.clone()),
            });
        }
        Message::ThinkingCompleted(_) => {
            if !thinking_buf.is_empty() {
                output
                    .content
                    .push(ContentBlock::Thinking(crate::ThinkingContent {
                        content_type: crate::ThinkingContentType::Thinking,
                        thinking: std::mem::take(thinking_buf),
                        thinking_signature: None,
                        redacted: None,
                    }));
                let _ = event_tx.send(ProviderEvent::ThinkingEnd {
                    content_index: thinking_index,
                    content: String::new(),
                    partial: Arc::new(output.clone()),
                });
            }
        }
        Message::ToolCallStarted(tc) => {
            let (name, _args) = tc.tool_call.as_ref().map(map_tool_call).unwrap_or_default();
            let _ = event_tx.send(ProviderEvent::ToolCallStart {
                content_index: *content_index,
                tool_call_id: Some(tc.call_id.clone()),
                tool_name: Some(name),
                partial: Arc::new(output.clone()),
            });
            *content_index += 1;
        }
        Message::ToolCallCompleted(tc) => {
            let (name, args) = tc.tool_call.as_ref().map(map_tool_call).unwrap_or_default();
            // Clone args so both the content block and the event carry them —
            // the host agent loop reads tool arguments from these events.
            output.content.push(ContentBlock::ToolCall(ToolCall {
                content_type: ToolCallType::ToolCall,
                id: tc.call_id.clone(),
                name: name.clone(),
                arguments: args.clone(),
                thought_signature: None,
            }));
            let _ = event_tx.send(ProviderEvent::ToolCallEnd {
                content_index: *content_index,
                tool_call: ToolCall {
                    content_type: ToolCallType::ToolCall,
                    id: tc.call_id.clone(),
                    name,
                    arguments: args,
                    thought_signature: None,
                },
                partial: Arc::new(output.clone()),
            });
        }
        Message::TokenDelta(d) => {
            // Usage signal: Cursor streams cumulative-ish token deltas.
            let tokens = d.tokens.max(0) as usize;
            output.usage = Usage {
                input: 0,
                output: tokens,
                cache_read: 0,
                cache_write: 0,
                total_tokens: tokens,
                cost: crate::Cost::default(),
            };
        }
        Message::TurnEnded(_) => {
            *saw_turn_ended = true;
        }
        // Heartbeat, summary, step, partial-tool-call, shell-output: ignored.
        _ => {}
    }
}

// ── KV channel handler ──────────────────────────────────────────────

fn handle_kv_message(
    kv: &proto::KvServerMessage,
    blob_store: &BlobStore,
    frame_tx: &tokio::sync::mpsc::UnboundedSender<Result<Bytes, std::io::Error>>,
) {
    let Some(msg) = &kv.message else { return };
    let response = match msg {
        kv_server_message::Message::GetBlobArgs(args) => {
            let blob_data = blob_store.get(&args.blob_id).cloned();
            KvClientMessage {
                id: kv.id,
                message: Some(proto::kv_client_message::Message::GetBlobResult(
                    GetBlobResult { blob_data },
                )),
            }
        }
        kv_server_message::Message::SetBlobArgs(args) => {
            // Server pushing a blob to our store — we don't persist across the
            // stateless Provider boundary, just acknowledge.
            let _ = &args.blob_data;
            KvClientMessage {
                id: kv.id,
                message: Some(proto::kv_client_message::Message::SetBlobResult(
                    proto::SetBlobResult { error: None },
                )),
            }
        }
    };
    let client_msg = AgentClientMessage {
        message: Some(agent_client_message::Message::KvClientMessage(response)),
    };
    let _ = frame_tx.send(Ok(Bytes::from(build_connect_frame(
        &client_msg.encode_to_vec(),
    ))));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Message};

    // ── Connect framing ─────────────────────────────────────────────

    #[test]
    fn test_build_and_parse_connect_frame_roundtrip() {
        let payload = b"hello cursor";
        let frame = build_connect_frame(payload);
        assert_eq!(frame.len(), FRAME_HEADER_SIZE + payload.len());
        assert_eq!(frame[0], 0); // no compression flag
        let (frames, rest) = parse_connect_frames(&frame);
        assert!(rest.is_empty());
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].flags, 0);
        assert_eq!(frames[0].payload.as_ref(), payload);
    }

    #[test]
    fn test_parse_multiple_frames() {
        let mut buf = Vec::new();
        buf.extend(build_connect_frame(b"one"));
        buf.extend(build_connect_frame(b"two"));
        buf.extend(build_connect_frame(b"three"));
        let (frames, rest) = parse_connect_frames(&buf);
        assert!(rest.is_empty());
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].payload.as_ref(), b"one");
        assert_eq!(frames[1].payload.as_ref(), b"two");
        assert_eq!(frames[2].payload.as_ref(), b"three");
    }

    #[test]
    fn test_parse_partial_frame_leaves_remainder() {
        let full = build_connect_frame(b"payload");
        // Truncate the last byte → incomplete frame.
        let (frames, rest) = parse_connect_frames(&full[..full.len() - 1]);
        assert!(frames.is_empty());
        assert_eq!(rest.len(), full.len() - 1);
    }

    #[test]
    fn test_end_stream_flag_preserved() {
        let mut frame = build_connect_frame(b"trailer");
        frame[0] |= CONNECT_END_STREAM_FLAG;
        let (frames, _) = parse_connect_frames(&frame);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].flags & CONNECT_END_STREAM_FLAG != 0);
    }

    // ── Blob store ──────────────────────────────────────────────────

    #[test]
    fn test_blob_store_roundtrip() {
        let mut store = BlobStore::default();
        let id = store.store(b"some data".to_vec());
        assert_eq!(id.len(), 32); // SHA-256
        assert_eq!(
            store.get(&id).map(|v| v.as_slice()),
            Some(&b"some data"[..])
        );
    }

    #[test]
    fn test_blob_store_dedup() {
        let mut store = BlobStore::default();
        let id1 = store.store(b"dup".to_vec());
        let id2 = store.store(b"dup".to_vec());
        assert_eq!(id1, id2, "identical content → identical blob id");
    }

    #[test]
    fn test_blob_id_is_sha256() {
        let mut store = BlobStore::default();
        let id = store.store(b"abc".to_vec());
        let expected = Sha256::digest(b"abc").to_vec();
        assert_eq!(id, expected);
    }

    // ── Protobuf message roundtrips ─────────────────────────────────

    #[test]
    fn test_agent_server_message_decode_minimal() {
        // A minimal InteractionUpdate{turn_ended} server message.
        let server_msg = AgentServerMessage {
            message: Some(agent_server_message::Message::InteractionUpdate(
                proto::InteractionUpdate {
                    message: Some(interaction_update::Message::TurnEnded(
                        proto::TurnEndedUpdate {},
                    )),
                },
            )),
        };
        let bytes = server_msg.encode_to_vec();
        let decoded = AgentServerMessage::decode(bytes.as_slice()).unwrap();
        assert!(matches!(
            decoded.message,
            Some(agent_server_message::Message::InteractionUpdate(_))
        ));
    }

    #[test]
    fn test_text_delta_roundtrip() {
        let server_msg = AgentServerMessage {
            message: Some(agent_server_message::Message::InteractionUpdate(
                proto::InteractionUpdate {
                    message: Some(interaction_update::Message::TextDelta(
                        proto::TextDeltaUpdate {
                            text: "hello".into(),
                        },
                    )),
                },
            )),
        };
        let bytes = server_msg.encode_to_vec();
        let frame = build_connect_frame(&bytes);
        let (frames, _) = parse_connect_frames(&frame);
        let raw = decompress_frame(&frames[0]).unwrap();
        let decoded = AgentServerMessage::decode(raw).unwrap();
        match decoded.message {
            Some(agent_server_message::Message::InteractionUpdate(u)) => match u.message {
                Some(interaction_update::Message::TextDelta(d)) => assert_eq!(d.text, "hello"),
                _ => panic!("expected TextDelta"),
            },
            _ => panic!("expected InteractionUpdate"),
        }
    }

    #[test]
    fn test_kv_get_blob_request_response_roundtrip() {
        let kv_req = proto::KvServerMessage {
            id: 42,
            span_context: None,
            message: Some(kv_server_message::Message::GetBlobArgs(
                proto::GetBlobArgs {
                    blob_id: vec![0xAA; 32],
                },
            )),
        };
        let bytes = kv_req.encode_to_vec();
        let decoded: proto::KvServerMessage =
            proto::KvServerMessage::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded.id, 42);
        match decoded.message {
            Some(kv_server_message::Message::GetBlobArgs(a)) => {
                assert_eq!(a.blob_id, vec![0xAA; 32]);
            }
            _ => panic!("expected GetBlobArgs"),
        }
    }

    // ── Request builder ─────────────────────────────────────────────

    #[test]
    fn test_build_run_request_basic() {
        let model = Model::new(
            "cursor-default",
            "Cursor Default",
            Api::CursorAgent,
            "cursor",
            CURSOR_API_URL,
        );
        let mut ctx = Context::new();
        ctx.set_system_prompt("You are helpful.");
        ctx.add_message(Message::user("Hello, Cursor!"));

        let (bytes, blobs) = build_run_request(&model, &ctx, "conv-1");
        assert!(!bytes.is_empty());

        // The request references at least the system-prompt blob + user action.
        assert!(
            !blobs.0.is_empty(),
            "blob store must contain system prompt + history"
        );

        let decoded = AgentClientMessage::decode(bytes.as_slice()).unwrap();
        match decoded.message {
            Some(agent_client_message::Message::RunRequest(req)) => {
                assert_eq!(req.conversation_id.as_deref(), Some("conv-1"));
                let state = req.conversation_state.unwrap();
                assert!(
                    !state.root_prompt_messages_json.is_empty(),
                    "system prompt must produce at least one root-prompt blob"
                );
            }
            _ => panic!("expected RunRequest"),
        }
    }

    #[test]
    fn test_build_run_request_multi_turn_history() {
        let model = Model::new(
            "cursor-default",
            "Cursor",
            Api::CursorAgent,
            "cursor",
            CURSOR_API_URL,
        );
        let mut ctx = Context::new();
        ctx.set_system_prompt("Be concise.");
        ctx.add_message(Message::user("What is 2+2?"));
        ctx.add_message(Message::assistant(vec![ContentBlock::Text(
            crate::TextContent::new("4"),
        )]));
        ctx.add_message(Message::user("Thanks!"));

        let (bytes, blobs) = build_run_request(&model, &ctx, "conv-2");
        let decoded = AgentClientMessage::decode(bytes.as_slice()).unwrap();
        let req = match decoded.message {
            Some(agent_client_message::Message::RunRequest(r)) => r,
            _ => panic!("expected RunRequest"),
        };
        let state = req.conversation_state.unwrap();
        // root_prompt: system + prior user + prior assistant = 3 entries.
        assert!(state.root_prompt_messages_json.len() >= 3);
        // turns: one prior turn (user "What is 2+2?" + assistant "4").
        assert_eq!(state.turns.len(), 1, "one completed prior turn");
        // The active "Thanks!" must NOT appear in turns (it's in the action).
        let turn_blob = blobs.get(&state.turns[0]).unwrap();
        let turn = proto::ConversationTurnStructure::decode(turn_blob.as_slice()).unwrap();
        assert!(turn.turn.is_some());
        // Blob store should have resolved every referenced blob.
        for id in &state.root_prompt_messages_json {
            assert!(blobs.get(id).is_some(), "root-prompt blob must be in store");
        }
        for id in &state.turns {
            assert!(blobs.get(id).is_some(), "turn blob must be in store");
        }
    }

    // ── Provider construction ───────────────────────────────────────

    #[test]
    fn test_cursor_provider_creation() {
        let provider = CursorProvider::new();
        let _ = provider;
    }

    #[test]
    fn test_cursor_provider_with_base_url() {
        let provider = CursorProvider::with_base_url("https://custom.cursor.sh/");
        assert_eq!(provider.base_url, "https://custom.cursor.sh");
    }
}
