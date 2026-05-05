//! RPC mode for headless operation
//!
//! Implements a JSONL-over-stdio protocol for embedding oxi in other applications.
//! Commands are received as JSON lines on stdin, responses and events are emitted
//! as JSON lines on stdout.
//!
//! # Protocol
//!
//! - **Commands**: JSON objects with a `type` field and optional `id` for correlation
//! - **Responses**: JSON objects with `type: "response"`, `command`, `success`, and optional `data`/`error`
//! - **Events**: `AgentEvent` objects streamed as they occur
//! - **Extension UI**: Extension UI requests emitted to client; client responds with `extension_ui_response`
//! - **JSON-RPC 2.0**: Alternatively supports JSON-RPC 2.0 framing (detected by `jsonrpc` field)
//!
//! # Framing
//!
//! Strict JSONL (JSON Lines): each record is a single JSON object terminated by `\n` (LF).
//! Carriage returns inside payloads are preserved; records are split on `\n` only.

#![allow(dead_code, unused_imports)]

use crate::App;
use anyhow::{Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};

// ============================================================================
// JSONL Framing
// ============================================================================

/// Serialize a value as a strict JSONL record (JSON + LF).
///
/// Framing is LF-only. Payload strings may contain other Unicode separators
/// such as U+2028 and U+2029. Clients must split records on `\n` only.
pub fn serialize_json_line(value: &Value) -> String {
    format!("{}\n", serde_json::to_string(value).unwrap_or_default())
}

/// Serialize a serializable value as a JSONL record.
pub fn serialize_json_line_obj<T: Serialize>(value: &T) -> String {
    match serde_json::to_string(value) {
        Ok(s) => format!("{}\n", s),
        Err(e) => {
            tracing::error!("Failed to serialize JSONL: {}", e);
            format!(
                "{{\"type\":\"response\",\"command\":\"internal\",\"success\":false,\"error\":\"Serialization error\"}}\n"
            )
        }
    }
}

/// Parse a JSONL line, trimming any trailing CR.
pub fn parse_json_line(line: &str) -> Result<Value, serde_json::Error> {
    let trimmed = line.trim_end_matches('\r');
    serde_json::from_str(trimmed)
}

// ============================================================================
// JSON-RPC 2.0 Types
// ============================================================================

/// JSON-RPC 2.0 request
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String, // must be "2.0"
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 success response
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcSuccessResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub result: Value,
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC 2.0 error response
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub error: JsonRpcError,
}

// Standard JSON-RPC error codes
pub const JSONRPC_PARSE_ERROR: i64 = -32700;
pub const JSONRPC_INVALID_REQUEST: i64 = -32600;
pub const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
pub const JSONRPC_INVALID_PARAMS: i64 = -32602;
pub const JSONRPC_INTERNAL_ERROR: i64 = -32603;

// ============================================================================
// RPC Types
// ============================================================================

/// RPC command from client
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcCommand {
    // ── Prompting ───────────────────────────────────────────────────
    Prompt {
        id: Option<String>,
        message: String,
        images: Option<Vec<ImageData>>,
        #[serde(default)]
        streaming_behavior: Option<String>,
    },
    Steer {
        id: Option<String>,
        message: String,
        images: Option<Vec<ImageData>>,
    },
    FollowUp {
        id: Option<String>,
        message: String,
        images: Option<Vec<ImageData>>,
    },
    Abort {
        id: Option<String>,
    },
    NewSession {
        id: Option<String>,
        parent_session: Option<String>,
    },

    // ── State ───────────────────────────────────────────────────────
    GetState {
        id: Option<String>,
    },

    // ── Model ──────────────────────────────────────────────────────
    SetModel {
        id: Option<String>,
        provider: String,
        model_id: String,
    },
    CycleModel {
        id: Option<String>,
    },
    GetAvailableModels {
        id: Option<String>,
    },

    // ── Thinking ────────────────────────────────────────────────────
    SetThinkingLevel {
        id: Option<String>,
        level: String,
    },
    CycleThinkingLevel {
        id: Option<String>,
    },

    // ── Queue modes ─────────────────────────────────────────────────
    SetSteeringMode {
        id: Option<String>,
        mode: String,
    },
    SetFollowUpMode {
        id: Option<String>,
        mode: String,
    },

    // ── Compaction ──────────────────────────────────────────────────
    Compact {
        id: Option<String>,
        custom_instructions: Option<String>,
    },
    SetAutoCompaction {
        id: Option<String>,
        enabled: bool,
    },

    // ── Retry ───────────────────────────────────────────────────────
    SetAutoRetry {
        id: Option<String>,
        enabled: bool,
    },
    AbortRetry {
        id: Option<String>,
    },

    // ── Bash ────────────────────────────────────────────────────────
    Bash {
        id: Option<String>,
        command: String,
    },
    AbortBash {
        id: Option<String>,
    },

    // ── Session ────────────────────────────────────────────────────
    GetSessionStats {
        id: Option<String>,
    },
    ExportHtml {
        id: Option<String>,
        output_path: Option<String>,
    },
    SwitchSession {
        id: Option<String>,
        session_path: String,
    },
    Fork {
        id: Option<String>,
        entry_id: String,
    },
    Clone {
        id: Option<String>,
    },
    GetForkMessages {
        id: Option<String>,
    },
    GetLastAssistantText {
        id: Option<String>,
    },
    SetSessionName {
        id: Option<String>,
        name: String,
    },

    // ── Messages ───────────────────────────────────────────────────
    GetMessages {
        id: Option<String>,
    },

    // ── Commands ────────────────────────────────────────────────────
    GetCommands {
        id: Option<String>,
    },
}

/// Image data for RPC commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    pub source: String,
    #[serde(rename = "type")]
    pub media_type: String,
}

/// Base64-encoded image data
#[derive(Debug, Clone)]
pub struct RpcImageSource {
    pub data: Vec<u8>,
    pub mime_type: String,
}

/// RPC response to client
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcResponse {
    Response {
        id: Option<String>,
        command: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    ExtensionUiRequest(RpcExtensionUiRequest),
}

/// Extension UI request
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum RpcExtensionUiRequest {
    Select {
        id: String,
        title: String,
        options: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    Confirm {
        id: String,
        title: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    Input {
        id: String,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    Editor {
        id: String,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefill: Option<String>,
    },
    Notify {
        id: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        notify_type: Option<String>,
    },
    SetStatus {
        id: String,
        status_key: String,
        status_text: Option<String>,
    },
    SetWidget {
        id: String,
        widget_key: String,
        widget_lines: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        widget_placement: Option<String>,
    },
    SetTitle {
        id: String,
        title: String,
    },
    SetEditorText {
        id: String,
        text: String,
    },
}

/// Extension UI response from client
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcExtensionUiResponse {
    ExtensionUiResponse {
        id: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        confirmed: Option<bool>,
        #[serde(default)]
        cancelled: Option<bool>,
    },
}

/// Session state for get_state response
#[derive(Debug, Clone, Serialize)]
pub struct SessionState {
    /// Current model info
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelInfo>,
    pub thinking_level: String,
    pub is_streaming: bool,
    pub is_compacting: bool,
    pub steering_mode: String,
    pub follow_up_mode: String,
    /// Session file path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub auto_compaction_enabled: bool,
    pub message_count: usize,
    pub pending_message_count: usize,
}

/// Model information for session state
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub provider: String,
    pub id: String,
}

/// Command info for get_commands response
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_info: Option<SourceInfo>,
}

/// Source info for command provenance
#[derive(Debug, Clone, Serialize)]
pub struct SourceInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// Result of a session stat query
#[derive(Debug, Clone, Serialize)]
pub struct SessionStats {
    pub message_count: usize,
    pub token_count: Option<usize>,
    pub last_activity: Option<i64>,
}

/// Result of compaction
#[derive(Debug, Clone, Serialize)]
pub struct CompactionResult {
    pub original_count: usize,
    pub compacted_count: usize,
    pub tokens_saved: Option<usize>,
}

// ============================================================================
// Event Types
// ============================================================================

/// Events emitted by the RPC server to the client during streaming.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcEvent {
    /// Agent started processing
    AgentStart,
    /// Streaming text chunk
    TextChunk { text: String },
    /// Agent is thinking
    Thinking,
    /// Tool execution started
    ToolStart { tool: String },
    /// Tool execution finished
    ToolEnd { tool: String },
    /// Agent finished processing
    AgentEnd,
    /// Error occurred
    Error { message: String },
    /// Extension error
    ExtensionError {
        extension_path: String,
        event: String,
        error: String,
    },
}

// ============================================================================
// Pending Extension UI Request
// ============================================================================

/// Tracks pending extension UI requests awaiting client responses.
pub struct PendingExtensionRequest {
    pub resolve: tokio::sync::oneshot::Sender<RpcExtensionUiResponse>,
}

// ============================================================================
// RPC Server
// ============================================================================

/// RPC server for headless operation
pub struct RpcServer {
    /// Port for TCP mode (0 for stdio mode)
    port: u16,
    /// Shutdown flag
    shutdown: Arc<parking_lot::RwLock<bool>>,
    /// Session state
    session_state: Arc<parking_lot::RwLock<SessionState>>,
    /// Pending extension UI requests
    pending_extension_requests: Arc<Mutex<Vec<(String, oneshot::Sender<RpcExtensionUiResponse>)>>>,
    /// Event subscribers
    event_tx: mpsc::UnboundedSender<RpcEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<RpcEvent>>,
}

impl RpcServer {
    /// Create a new RPC server
    pub fn new(port: u16) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            port,
            shutdown: Arc::new(parking_lot::RwLock::new(false)),
            session_state: Arc::new(parking_lot::RwLock::new(SessionState {
                model: None,
                thinking_level: "default".to_string(),
                is_streaming: false,
                is_compacting: false,
                steering_mode: "all".to_string(),
                follow_up_mode: "all".to_string(),
                session_file: None,
                session_id: uuid::Uuid::new_v4().to_string(),
                session_name: None,
                auto_compaction_enabled: true,
                message_count: 0,
                pending_message_count: 0,
            })),
            pending_extension_requests: Arc::new(Mutex::new(Vec::new())),
            event_tx,
            event_rx: Some(event_rx),
        }
    }

    /// Get the port (0 for stdio mode)
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Check if shutdown has been requested
    pub fn is_shutdown_requested(&self) -> bool {
        *self.shutdown.read()
    }

    /// Request shutdown
    pub fn request_shutdown(&self) {
        *self.shutdown.write() = true;
    }

    /// Update session state
    pub fn update_session_state<F>(&self, f: F)
    where
        F: FnOnce(&mut SessionState),
    {
        let mut state = self.session_state.write();
        f(&mut state);
    }

    /// Get current session state
    pub fn get_session_state(&self) -> SessionState {
        self.session_state.read().clone()
    }

    /// Emit an event to subscribers
    pub fn emit_event(&self, event: RpcEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Take the event receiver (can only be called once)
    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<RpcEvent>> {
        self.event_rx.take()
    }

    /// Register a pending extension UI request and return a oneshot receiver
    pub async fn register_extension_request(
        &self,
        id: String,
    ) -> oneshot::Receiver<RpcExtensionUiResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending_extension_requests
            .lock()
            .await
            .push((id, tx));
        rx
    }

    /// Resolve a pending extension UI request
    pub async fn resolve_extension_request(
        &self,
        id: &str,
        response: RpcExtensionUiResponse,
    ) -> bool {
        let mut pending = self.pending_extension_requests.lock().await;
        if let Some(pos) = pending.iter().position(|(req_id, _)| req_id == id) {
            let (_, sender) = pending.remove(pos);
            let _ = sender.send(response);
            true
        } else {
            false
        }
    }

    /// Parse image data from RPC command
    pub fn parse_images(images: Option<Vec<ImageData>>) -> Vec<RpcImageSource> {
        images
            .unwrap_or_default()
            .into_iter()
            .filter_map(|img| {
                if img.source.starts_with("data:") {
                    let parts: Vec<&str> = img.source.splitn(2, ',').collect();
                    if parts.len() == 2 {
                        let base64_data = parts[1].split(';').next().unwrap_or(parts[1]);
                        if let Ok(decoded) =
                            base64::engine::general_purpose::STANDARD.decode(base64_data)
                        {
                            return Some(RpcImageSource {
                                data: decoded,
                                mime_type: img.media_type,
                            });
                        }
                    }
                }
                None
            })
            .collect()
    }
}

// ============================================================================
// JSON-RPC 2.0 Method Mapping
// ============================================================================

/// Map a JSON-RPC 2.0 method + params to an internal RPC command value.
fn jsonrpc_to_command(method: &str, params: Option<Value>, id: Option<Value>) -> Option<Value> {
    let id_str = id.map(|v| match v {
        Value::String(s) => s,
        Value::Number(n) => n.to_string(),
        _ => v.to_string(),
    });

    let cmd = match method {
        "prompt" => serde_json::json!({
            "type": "prompt",
            "id": id_str,
            "message": params.as_ref().and_then(|p| p.get("message")).and_then(|m| m.as_str()).unwrap_or(""),
            "images": params.as_ref().and_then(|p| p.get("images")),
            "streaming_behavior": params.as_ref().and_then(|p| p.get("streaming_behavior")),
        }),
        "steer" => serde_json::json!({
            "type": "steer",
            "id": id_str,
            "message": params.as_ref().and_then(|p| p.get("message")).and_then(|m| m.as_str()).unwrap_or(""),
        }),
        "follow_up" => serde_json::json!({
            "type": "follow_up",
            "id": id_str,
            "message": params.as_ref().and_then(|p| p.get("message")).and_then(|m| m.as_str()).unwrap_or(""),
        }),
        "abort" => serde_json::json!({ "type": "abort", "id": id_str }),
        "new_session" => serde_json::json!({
            "type": "new_session",
            "id": id_str,
            "parent_session": params.as_ref().and_then(|p| p.get("parent_session")).and_then(|v| v.as_str()),
        }),
        "get_state" => serde_json::json!({ "type": "get_state", "id": id_str }),
        "set_model" => serde_json::json!({
            "type": "set_model",
            "id": id_str,
            "provider": params.as_ref().and_then(|p| p.get("provider")).and_then(|v| v.as_str()).unwrap_or(""),
            "model_id": params.as_ref().and_then(|p| p.get("modelId")).and_then(|v| v.as_str()).unwrap_or(""),
        }),
        "cycle_model" => serde_json::json!({ "type": "cycle_model", "id": id_str }),
        "get_available_models" => serde_json::json!({ "type": "get_available_models", "id": id_str }),
        "set_thinking_level" => serde_json::json!({
            "type": "set_thinking_level",
            "id": id_str,
            "level": params.as_ref().and_then(|p| p.get("level")).and_then(|v| v.as_str()).unwrap_or("default"),
        }),
        "cycle_thinking_level" => serde_json::json!({ "type": "cycle_thinking_level", "id": id_str }),
        "set_steering_mode" => serde_json::json!({
            "type": "set_steering_mode",
            "id": id_str,
            "mode": params.as_ref().and_then(|p| p.get("mode")).and_then(|v| v.as_str()).unwrap_or("all"),
        }),
        "set_follow_up_mode" => serde_json::json!({
            "type": "set_follow_up_mode",
            "id": id_str,
            "mode": params.as_ref().and_then(|p| p.get("mode")).and_then(|v| v.as_str()).unwrap_or("all"),
        }),
        "compact" => serde_json::json!({
            "type": "compact",
            "id": id_str,
            "custom_instructions": params.as_ref().and_then(|p| p.get("customInstructions")).and_then(|v| v.as_str()),
        }),
        "set_auto_compaction" => serde_json::json!({
            "type": "set_auto_compaction",
            "id": id_str,
            "enabled": params.as_ref().and_then(|p| p.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(true),
        }),
        "set_auto_retry" => serde_json::json!({
            "type": "set_auto_retry",
            "id": id_str,
            "enabled": params.as_ref().and_then(|p| p.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(true),
        }),
        "abort_retry" => serde_json::json!({ "type": "abort_retry", "id": id_str }),
        "bash" => serde_json::json!({
            "type": "bash",
            "id": id_str,
            "command": params.as_ref().and_then(|p| p.get("command")).and_then(|v| v.as_str()).unwrap_or(""),
        }),
        "abort_bash" => serde_json::json!({ "type": "abort_bash", "id": id_str }),
        "get_session_stats" => serde_json::json!({ "type": "get_session_stats", "id": id_str }),
        "export_html" => serde_json::json!({
            "type": "export_html",
            "id": id_str,
            "output_path": params.as_ref().and_then(|p| p.get("outputPath")).and_then(|v| v.as_str()),
        }),
        "switch_session" => serde_json::json!({
            "type": "switch_session",
            "id": id_str,
            "session_path": params.as_ref().and_then(|p| p.get("sessionPath")).and_then(|v| v.as_str()).unwrap_or(""),
        }),
        "fork" => serde_json::json!({
            "type": "fork",
            "id": id_str,
            "entry_id": params.as_ref().and_then(|p| p.get("entryId")).and_then(|v| v.as_str()).unwrap_or(""),
        }),
        "clone" => serde_json::json!({ "type": "clone", "id": id_str }),
        "get_fork_messages" => serde_json::json!({ "type": "get_fork_messages", "id": id_str }),
        "get_last_assistant_text" => serde_json::json!({ "type": "get_last_assistant_text", "id": id_str }),
        "set_session_name" => serde_json::json!({
            "type": "set_session_name",
            "id": id_str,
            "name": params.as_ref().and_then(|p| p.get("name")).and_then(|v| v.as_str()).unwrap_or(""),
        }),
        "get_messages" => serde_json::json!({ "type": "get_messages", "id": id_str }),
        "get_commands" => serde_json::json!({ "type": "get_commands", "id": id_str }),
        _ => return None,
    };

    Some(cmd)
}

/// Convert an internal RpcResponse to a JSON-RPC 2.0 response.
fn rpc_response_to_jsonrpc(response: &RpcResponse, rpc_id: Value) -> String {
    match response {
        RpcResponse::Response {
            success: true,
            data,
            ..
        } => {
            let result = data.clone().unwrap_or(Value::Null);
            let resp = JsonRpcSuccessResponse {
                jsonrpc: "2.0".to_string(),
                id: rpc_id,
                result,
            };
            serde_json::to_string(&resp).unwrap_or_default()
        }
        RpcResponse::Response {
            success: false,
            error,
            ..
        } => {
            let resp = JsonRpcErrorResponse {
                jsonrpc: "2.0".to_string(),
                id: rpc_id,
                error: JsonRpcError {
                    code: JSONRPC_INTERNAL_ERROR,
                    message: error.clone().unwrap_or_default(),
                    data: None,
                },
            };
            serde_json::to_string(&resp).unwrap_or_default()
        }
        RpcResponse::ExtensionUiRequest(_) => {
            // Extension UI requests don't map cleanly to JSON-RPC
            // In JSON-RPC mode we skip these or send as notification
            String::new()
        }
    }
}

// ============================================================================
// JSONL Line Reader
// ============================================================================

/// A strict JSONL line reader that splits on LF only (not CR or other separators).
/// This is important because JSON strings may contain U+2028/U+2029 which
/// Node.js readline would incorrectly split on.
pub struct JsonlLineReader {
    buffer: String,
}

impl JsonlLineReader {
    /// Create a new JSONL line reader.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Feed data into the reader, returning complete lines.
    /// Lines are terminated by `\n` only. Trailing `\r` is stripped.
    pub fn feed(&mut self, data: &str) -> Vec<String> {
        self.buffer.push_str(data);
        let mut lines = Vec::new();

        loop {
            match self.buffer.find('\n') {
                Some(pos) => {
                    let line = self.buffer[..pos].to_string();
                    self.buffer = self.buffer[pos + 1..].to_string();
                    // Strip trailing CR
                    let trimmed = line.trim_end_matches('\r').to_string();
                    if !trimmed.is_empty() {
                        lines.push(trimmed);
                    }
                }
                None => break,
            }
        }

        lines
    }

    /// Flush any remaining data as a final line (for EOF handling).
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let line = self.buffer.trim_end_matches('\r').to_string();
        self.buffer.clear();
        if line.is_empty() {
            None
        } else {
            Some(line)
        }
    }

    /// Check if there's buffered data.
    pub fn has_buffered_data(&self) -> bool {
        !self.buffer.is_empty()
    }
}

impl Default for JsonlLineReader {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Output Writer
// ============================================================================

/// Thread-safe output writer that ensures atomic JSONL writes.
pub struct RpcOutput {
    inner: Arc<parking_lot::Mutex<std::io::Stdout>>,
}

impl RpcOutput {
    /// Create a new output writer wrapping stdout.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::Mutex::new(std::io::stdout())),
        }
    }

    /// Write a JSONL record atomically.
    pub fn write_line(&self, value: &Value) {
        let line = serialize_json_line(value);
        let mut out = self.inner.lock();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }

    /// Write a serializable value as a JSONL record.
    pub fn write_obj<T: Serialize>(&self, value: &T) {
        let line = serialize_json_line_obj(value);
        let mut out = self.inner.lock();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }

    /// Write a raw string (must already be a complete JSONL record with trailing LF).
    pub fn write_raw(&self, raw: &str) {
        let mut out = self.inner.lock();
        let _ = out.write_all(raw.as_bytes());
        let _ = out.flush();
    }
}

impl Default for RpcOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for RpcOutput {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// ============================================================================
// Session Handoff Protocol
// ============================================================================

/// Information about a session for handoff between processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHandoff {
    /// Unique session identifier
    pub session_id: String,
    /// Session file path
    pub session_file: Option<String>,
    /// Session display name
    pub session_name: Option<String>,
    /// Parent session ID (for forked sessions)
    pub parent_session_id: Option<String>,
    /// Model being used
    pub model_id: Option<String>,
    /// Thinking level
    pub thinking_level: Option<String>,
    /// Message count at handoff time
    pub message_count: usize,
    /// Timestamp of handoff
    pub timestamp: i64,
}

impl SessionHandoff {
    /// Create a handoff from current session state.
    pub fn from_state(state: &SessionState) -> Self {
        Self {
            session_id: state.session_id.clone(),
            session_file: state.session_file.clone(),
            session_name: state.session_name.clone(),
            parent_session_id: None,
            model_id: state.model.as_ref().map(|m| format!("{}/{}", m.provider, m.id)),
            thinking_level: Some(state.thinking_level.clone()),
            message_count: state.message_count,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Serialize to JSON for transmission.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

// ============================================================================
// RPC Server Entry Point
// ============================================================================

/// Run the RPC server in stdio mode.
///
/// Reads JSONL commands from stdin, processes them, and writes
/// JSONL responses/events to stdout.
pub async fn run_rpc_mode(app: App) -> Result<()> {
    let mut server = Arc::new(RpcServer::new(0)); // 0 = stdio mode
    let output = RpcOutput::new();

    // Spawn event forwarding task
    let output_clone = output.clone();
    // Take the event receiver - server was just created so Arc hasn't been shared
    let rx = Arc::get_mut(&mut server)
        .expect("server Arc must be unique at construction")
        .take_event_receiver();
    let mut event_rx = rx.expect("event receiver must be available at construction");
    let event_handle = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let json = serde_json::to_value(&event).unwrap_or_default();
            output_clone.write_line(&json);
        }
    });

    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut line_reader = JsonlLineReader::new();
    let _raw_buf = [0u8; 4096];

    loop {
        // Check for shutdown
        if server.is_shutdown_requested() {
            break;
        }

        // Read from stdin (blocking)
        let mut read_buf = String::new();
        match input.read_line(&mut read_buf) {
            Ok(0) => {
                // EOF reached — flush any remaining buffered data
                if let Some(final_line) = line_reader.flush() {
                    process_line(&final_line, &server, &app, &output).await;
                }
                break;
            }
            Ok(_) => {
                let lines = line_reader.feed(&read_buf);
                for line in lines {
                    process_line(&line, &server, &app, &output).await;
                }
            }
            Err(e) => {
                eprintln!("Error reading stdin: {}", e);
                break;
            }
        }
    }

    // Cleanup
    event_handle.abort();
    Ok(())
}

/// Process a single JSONL input line.
async fn process_line(
    line: &str,
    server: &Arc<RpcServer>,
    app: &App,
    output: &RpcOutput,
) {
    let value = match parse_json_line(line) {
        Ok(v) => v,
        Err(e) => {
            let error_response = RpcResponse::Response {
                id: None,
                command: "parse".to_string(),
                success: false,
                data: None,
                error: Some(format!("Failed to parse command: {}", e)),
            };
            output.write_obj(&error_response);
            return;
        }
    };

    // Check if this is a JSON-RPC 2.0 request
    if let Some(obj) = value.as_object() {
        if obj.contains_key("jsonrpc") {
            handle_jsonrpc_request(obj, server, app, output);
            return;
        }
    }

    // Handle extension UI responses
    if let Some(obj) = value.as_object() {
        if obj.get("type").and_then(|v| v.as_str()) == Some("extension_ui_response") {
            if let Ok(response) = serde_json::from_value::<RpcExtensionUiResponse>(value.clone()) {
                let id = match &response {
                    RpcExtensionUiResponse::ExtensionUiResponse { id, .. } => id.clone(),
                };
                server.resolve_extension_request(&id, response).await;
            }
            return;
        }
    }

    // Handle as native RPC command
    match serde_json::from_value::<RpcCommand>(value) {
        Ok(command) => {
            let response = execute_command(server, app, command);
            output.write_obj(&response);
        }
        Err(e) => {
            let error_response = RpcResponse::Response {
                id: None,
                command: "parse".to_string(),
                success: false,
                data: None,
                error: Some(format!("Parse error: {}", e)),
            };
            output.write_obj(&error_response);
        }
    }
}

/// Handle a JSON-RPC 2.0 request.
fn handle_jsonrpc_request(
    obj: &serde_json::Map<String, Value>,
    server: &Arc<RpcServer>,
    app: &App,
    output: &RpcOutput,
) {
    let jsonrpc = obj.get("jsonrpc").and_then(|v| v.as_str()).unwrap_or("");
    if jsonrpc != "2.0" {
        let err = JsonRpcErrorResponse {
            jsonrpc: "2.0".to_string(),
            id: Value::Null,
            error: JsonRpcError {
                code: JSONRPC_INVALID_REQUEST,
                message: "Invalid jsonrpc version".to_string(),
                data: None,
            },
        };
        output.write_obj(&err);
        return;
    }

    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    let method = match obj.get("method").and_then(|v| v.as_str()) {
        Some(m) => m,
        None => {
            let err = JsonRpcErrorResponse {
                jsonrpc: "2.0".to_string(),
                id,
                error: JsonRpcError {
                    code: JSONRPC_INVALID_REQUEST,
                    message: "Missing method".to_string(),
                    data: None,
                },
            };
            output.write_obj(&err);
            return;
        }
    };

    let params = obj.get("params").cloned();

    // Map to internal command
    let cmd_value = match jsonrpc_to_command(method, params, Some(id.clone())) {
        Some(v) => v,
        None => {
            let err = JsonRpcErrorResponse {
                jsonrpc: "2.0".to_string(),
                id,
                error: JsonRpcError {
                    code: JSONRPC_METHOD_NOT_FOUND,
                    message: format!("Method not found: {}", method),
                    data: None,
                },
            };
            output.write_obj(&err);
            return;
        }
    };

    // Parse and execute
    match serde_json::from_value::<RpcCommand>(cmd_value) {
        Ok(command) => {
            let response = execute_command(server, app, command);
            let jsonrpc_response = rpc_response_to_jsonrpc(&response, id);
            if !jsonrpc_response.is_empty() {
                output.write_raw(&format!("{}\n", jsonrpc_response));
            }
        }
        Err(e) => {
            let err = JsonRpcErrorResponse {
                jsonrpc: "2.0".to_string(),
                id,
                error: JsonRpcError {
                    code: JSONRPC_INVALID_PARAMS,
                    message: format!("Invalid params: {}", e),
                    data: None,
                },
            };
            output.write_obj(&err);
        }
    }
}

/// Execute an RPC command and return the response.
fn execute_command(server: &Arc<RpcServer>, _app: &App, command: RpcCommand) -> RpcResponse {
    match command {
        // ── Prompting ──────────────────────────────────────────────
        RpcCommand::Prompt {
            id,
            message: _,
            images,
            streaming_behavior: _,
        } => {
            let _image_sources = RpcServer::parse_images(images);

            server.update_session_state(|s| {
                s.is_streaming = true;
                s.pending_message_count += 1;
            });

            server.emit_event(RpcEvent::AgentStart);

            RpcResponse::Response {
                id,
                command: "prompt".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::Steer {
            id,
            message: _,
            images: _,
        } => {
            server.update_session_state(|s| {
                s.steering_mode = "one_at_a_time".to_string();
            });

            RpcResponse::Response {
                id,
                command: "steer".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::FollowUp {
            id,
            message: _,
            images: _,
        } => {
            server.update_session_state(|s| {
                s.follow_up_mode = "one_at_a_time".to_string();
            });

            RpcResponse::Response {
                id,
                command: "follow_up".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::Abort { id } => {
            server.update_session_state(|s| {
                s.is_streaming = false;
            });
            server.emit_event(RpcEvent::AgentEnd);

            RpcResponse::Response {
                id,
                command: "abort".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::NewSession {
            id,
            parent_session: _,
        } => {
            server.update_session_state(|s| {
                s.session_id = uuid::Uuid::new_v4().to_string();
                s.message_count = 0;
                s.pending_message_count = 0;
            });

            RpcResponse::Response {
                id,
                command: "new_session".to_string(),
                success: true,
                data: Some(serde_json::json!({ "cancelled": false })),
                error: None,
            }
        }

        // ── State ──────────────────────────────────────────────────
        RpcCommand::GetState { id } => {
            let state = server.get_session_state();

            RpcResponse::Response {
                id,
                command: "get_state".to_string(),
                success: true,
                data: Some(serde_json::to_value(&state).unwrap()),
                error: None,
            }
        }

        // ── Model ──────────────────────────────────────────────────
        RpcCommand::SetModel {
            id,
            provider,
            model_id,
        } => {
            // In a real implementation, this would switch the model
            server.update_session_state(|s| {
                s.model = Some(ModelInfo {
                    provider: provider.clone(),
                    id: model_id.clone(),
                });
            });

            RpcResponse::Response {
                id,
                command: "set_model".to_string(),
                success: true,
                data: Some(serde_json::json!({
                    "provider": provider,
                    "id": model_id
                })),
                error: None,
            }
        }

        RpcCommand::CycleModel { id } => RpcResponse::Response {
            id,
            command: "cycle_model".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "model": null,
                "thinking_level": "default",
                "is_scoped": false
            })),
            error: None,
        },

        RpcCommand::GetAvailableModels { id } => RpcResponse::Response {
            id,
            command: "get_available_models".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "models": []
            })),
            error: None,
        },

        // ── Thinking ───────────────────────────────────────────────
        RpcCommand::SetThinkingLevel { id, level } => {
            server.update_session_state(|s| {
                s.thinking_level = level;
            });

            RpcResponse::Response {
                id,
                command: "set_thinking_level".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::CycleThinkingLevel { id } => {
            let current = server.get_session_state().thinking_level;
            let next = match current.as_str() {
                "off" => "default",
                "default" => "medium",
                "medium" => "high",
                _ => "off",
            };

            server.update_session_state(|s| {
                s.thinking_level = next.to_string();
            });

            RpcResponse::Response {
                id,
                command: "cycle_thinking_level".to_string(),
                success: true,
                data: Some(serde_json::json!({ "level": next })),
                error: None,
            }
        }

        // ── Queue modes ────────────────────────────────────────────
        RpcCommand::SetSteeringMode { id, mode } => {
            server.update_session_state(|s| {
                s.steering_mode = mode;
            });

            RpcResponse::Response {
                id,
                command: "set_steering_mode".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::SetFollowUpMode { id, mode } => {
            server.update_session_state(|s| {
                s.follow_up_mode = mode;
            });

            RpcResponse::Response {
                id,
                command: "set_follow_up_mode".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        // ── Compaction ─────────────────────────────────────────────
        RpcCommand::Compact {
            id,
            custom_instructions: _,
        } => {
            server.update_session_state(|s| {
                s.is_compacting = true;
            });

            // Simulate compaction
            let state = server.get_session_state();
            let result = CompactionResult {
                original_count: state.message_count,
                compacted_count: (state.message_count as f32 * 0.7) as usize,
                tokens_saved: Some(1000),
            };

            server.update_session_state(|s| {
                s.is_compacting = false;
                s.message_count = result.compacted_count;
            });

            RpcResponse::Response {
                id,
                command: "compact".to_string(),
                success: true,
                data: Some(serde_json::to_value(&result).unwrap()),
                error: None,
            }
        }

        RpcCommand::SetAutoCompaction { id, enabled } => {
            server.update_session_state(|s| {
                s.auto_compaction_enabled = enabled;
            });

            RpcResponse::Response {
                id,
                command: "set_auto_compaction".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        // ── Retry ─────────────────────────────────────────────────
        RpcCommand::SetAutoRetry { id, enabled: _ } => RpcResponse::Response {
            id,
            command: "set_auto_retry".to_string(),
            success: true,
            data: None,
            error: None,
        },

        RpcCommand::AbortRetry { id } => RpcResponse::Response {
            id,
            command: "abort_retry".to_string(),
            success: true,
            data: None,
            error: None,
        },

        // ── Bash ───────────────────────────────────────────────────
        RpcCommand::Bash { id, command } => {
            let output_result = std::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .output();

            match output_result {
                Ok(output) => RpcResponse::Response {
                    id,
                    command: "bash".to_string(),
                    success: true,
                    data: Some(serde_json::json!({
                        "stdout": String::from_utf8_lossy(&output.stdout),
                        "stderr": String::from_utf8_lossy(&output.stderr),
                        "exit_code": output.status.code()
                    })),
                    error: None,
                },
                Err(e) => RpcResponse::Response {
                    id,
                    command: "bash".to_string(),
                    success: false,
                    data: None,
                    error: Some(e.to_string()),
                },
            }
        }

        RpcCommand::AbortBash { id } => RpcResponse::Response {
            id,
            command: "abort_bash".to_string(),
            success: true,
            data: None,
            error: None,
        },

        // ── Session ────────────────────────────────────────────────
        RpcCommand::GetSessionStats { id } => {
            let state = server.get_session_state();
            let stats = SessionStats {
                message_count: state.message_count,
                token_count: None,
                last_activity: None,
            };

            RpcResponse::Response {
                id,
                command: "get_session_stats".to_string(),
                success: true,
                data: Some(serde_json::to_value(&stats).unwrap()),
                error: None,
            }
        }

        RpcCommand::ExportHtml { id, output_path: _ } => RpcResponse::Response {
            id,
            command: "export_html".to_string(),
            success: true,
            data: Some(serde_json::json!({ "path": "session.html" })),
            error: None,
        },

        RpcCommand::SwitchSession {
            id,
            session_path: _,
        } => {
            server.update_session_state(|s| {
                s.session_id = uuid::Uuid::new_v4().to_string();
                s.message_count = 0;
            });

            RpcResponse::Response {
                id,
                command: "switch_session".to_string(),
                success: true,
                data: Some(serde_json::json!({ "cancelled": false })),
                error: None,
            }
        }

        RpcCommand::Fork { id, entry_id: _ } => RpcResponse::Response {
            id,
            command: "fork".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "text": "",
                "cancelled": false
            })),
            error: None,
        },

        RpcCommand::Clone { id } => RpcResponse::Response {
            id,
            command: "clone".to_string(),
            success: true,
            data: Some(serde_json::json!({ "cancelled": false })),
            error: None,
        },

        RpcCommand::GetForkMessages { id } => RpcResponse::Response {
            id,
            command: "get_fork_messages".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "messages": []
            })),
            error: None,
        },

        RpcCommand::GetLastAssistantText { id } => RpcResponse::Response {
            id,
            command: "get_last_assistant_text".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "text": null
            })),
            error: None,
        },

        RpcCommand::SetSessionName { id, name } => {
            server.update_session_state(|s| {
                s.session_name = if name.is_empty() { None } else { Some(name) };
            });

            RpcResponse::Response {
                id,
                command: "set_session_name".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        // ── Messages ───────────────────────────────────────────────
        RpcCommand::GetMessages { id } => RpcResponse::Response {
            id,
            command: "get_messages".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "messages": []
            })),
            error: None,
        },

        // ── Commands ────────────────────────────────────────────────
        RpcCommand::GetCommands { id } => {
            let commands = vec![
                CommandInfo {
                    name: "compact".to_string(),
                    description: Some("Compact context".to_string()),
                    source: "builtin".to_string(),
                    source_info: None,
                },
                CommandInfo {
                    name: "clear".to_string(),
                    description: Some("Clear conversation".to_string()),
                    source: "builtin".to_string(),
                    source_info: None,
                },
            ];

            RpcResponse::Response {
                id,
                command: "get_commands".to_string(),
                success: true,
                data: Some(serde_json::json!({ "commands": commands })),
                error: None,
            }
        }
    }
}

// ============================================================================
// RPC Client (for programmatic access to an oxi RPC server)
// ============================================================================

/// Configuration for the RPC client.
#[derive(Debug, Clone)]
pub struct RpcClientConfig {
    /// Path to the oxi binary
    pub binary_path: String,
    /// Working directory for the agent
    pub cwd: Option<String>,
    /// Environment variables
    pub env: Vec<(String, String)>,
    /// Provider to use
    pub provider: Option<String>,
    /// Model ID to use
    pub model: Option<String>,
    /// Additional CLI arguments
    pub args: Vec<String>,
}

impl Default for RpcClientConfig {
    fn default() -> Self {
        Self {
            binary_path: "oxi".to_string(),
            cwd: None,
            env: Vec::new(),
            provider: None,
            model: None,
            args: Vec::new(),
        }
    }
}

/// Event listener callback type
pub type BoxedEventListener = Box<dyn Fn(RpcEvent) + Send>;

/// RPC Client for programmatic access to an oxi agent.
///
/// Spawns the agent in RPC mode and provides a typed API for all operations.
pub struct RpcClient {
    config: RpcClientConfig,
    child: Option<std::process::Child>,
    line_reader: JsonlLineReader,
    pending_requests: std::collections::HashMap<String, oneshot::Sender<RpcResponse>>,
    request_counter: u64,
    event_listeners: Vec<BoxedEventListener>,
    stderr_buffer: String,
}

impl RpcClient {
    /// Create a new RPC client with the given configuration.
    pub fn new(config: RpcClientConfig) -> Self {
        Self {
            config,
            child: None,
            line_reader: JsonlLineReader::new(),
            pending_requests: std::collections::HashMap::new(),
            request_counter: 0,
            event_listeners: Vec::new(),
            stderr_buffer: String::new(),
        }
    }

    /// Start the RPC agent process.
    pub fn start(&mut self) -> Result<()> {
        if self.child.is_some() {
            anyhow::bail!("Client already started");
        }

        let mut args = vec!["--mode".to_string(), "rpc".to_string()];
        if let Some(ref provider) = self.config.provider {
            args.push("--provider".to_string());
            args.push(provider.clone());
        }
        if let Some(ref model) = self.config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        args.extend(self.config.args.iter().cloned());

        let mut cmd = std::process::Command::new(&self.config.binary_path);
        cmd.args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(ref cwd) = self.config.cwd {
            cmd.current_dir(cwd);
        }

        for (key, value) in &self.config.env {
            cmd.env(key, value);
        }

        let child = cmd.spawn().context("Failed to spawn oxi RPC process")?;
        self.child = Some(child);

        Ok(())
    }

    /// Stop the RPC agent process.
    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.pending_requests.clear();
        Ok(())
    }

    /// Subscribe to agent events.
    pub fn on_event<F>(&mut self, listener: F)
    where
        F: Fn(RpcEvent) + Send + 'static,
    {
        self.event_listeners.push(Box::new(listener));
    }

    /// Get collected stderr output.
    pub fn stderr(&self) -> &str {
        &self.stderr_buffer
    }

    /// Send a prompt to the agent.
    pub fn prompt(&mut self, message: &str) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "prompt",
            "message": message
        }))
    }

    /// Send a steer message.
    pub fn steer(&mut self, message: &str) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "steer",
            "message": message
        }))
    }

    /// Send a follow-up message.
    pub fn follow_up(&mut self, message: &str) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "follow_up",
            "message": message
        }))
    }

    /// Abort current operation.
    pub fn abort(&mut self) -> Result<()> {
        self.send_command(serde_json::json!({ "type": "abort" }))
    }

    /// Start a new session.
    pub fn new_session(&mut self, parent_session: Option<&str>) -> Result<RpcResponse> {
        let mut cmd = serde_json::json!({ "type": "new_session" });
        if let Some(parent) = parent_session {
            cmd["parent_session"] = Value::String(parent.to_string());
        }
        self.send_and_wait(cmd)
    }

    /// Get current session state.
    pub fn get_state(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "get_state" }))
    }

    /// Set model by provider and ID.
    pub fn set_model(&mut self, provider: &str, model_id: &str) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({
            "type": "set_model",
            "provider": provider,
            "model_id": model_id
        }))
    }

    /// Cycle to next model.
    pub fn cycle_model(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "cycle_model" }))
    }

    /// Get available models.
    pub fn get_available_models(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "get_available_models" }))
    }

    /// Set thinking level.
    pub fn set_thinking_level(&mut self, level: &str) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "set_thinking_level",
            "level": level
        }))
    }

    /// Cycle thinking level.
    pub fn cycle_thinking_level(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "cycle_thinking_level" }))
    }

    /// Set steering mode.
    pub fn set_steering_mode(&mut self, mode: &str) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "set_steering_mode",
            "mode": mode
        }))
    }

    /// Set follow-up mode.
    pub fn set_follow_up_mode(&mut self, mode: &str) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "set_follow_up_mode",
            "mode": mode
        }))
    }

    /// Compact session context.
    pub fn compact(&mut self, custom_instructions: Option<&str>) -> Result<RpcResponse> {
        let mut cmd = serde_json::json!({ "type": "compact" });
        if let Some(instructions) = custom_instructions {
            cmd["custom_instructions"] = Value::String(instructions.to_string());
        }
        self.send_and_wait(cmd)
    }

    /// Set auto-compaction.
    pub fn set_auto_compaction(&mut self, enabled: bool) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "set_auto_compaction",
            "enabled": enabled
        }))
    }

    /// Set auto-retry.
    pub fn set_auto_retry(&mut self, enabled: bool) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "set_auto_retry",
            "enabled": enabled
        }))
    }

    /// Abort retry.
    pub fn abort_retry(&mut self) -> Result<()> {
        self.send_command(serde_json::json!({ "type": "abort_retry" }))
    }

    /// Execute a bash command.
    pub fn bash(&mut self, command: &str) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({
            "type": "bash",
            "command": command
        }))
    }

    /// Abort bash.
    pub fn abort_bash(&mut self) -> Result<()> {
        self.send_command(serde_json::json!({ "type": "abort_bash" }))
    }

    /// Get session statistics.
    pub fn get_session_stats(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "get_session_stats" }))
    }

    /// Export session to HTML.
    pub fn export_html(&mut self, output_path: Option<&str>) -> Result<RpcResponse> {
        let mut cmd = serde_json::json!({ "type": "export_html" });
        if let Some(path) = output_path {
            cmd["output_path"] = Value::String(path.to_string());
        }
        self.send_and_wait(cmd)
    }

    /// Switch to a different session.
    pub fn switch_session(&mut self, session_path: &str) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({
            "type": "switch_session",
            "session_path": session_path
        }))
    }

    /// Fork from a specific message.
    pub fn fork(&mut self, entry_id: &str) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({
            "type": "fork",
            "entry_id": entry_id
        }))
    }

    /// Clone the current branch.
    pub fn clone_session(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "clone" }))
    }

    /// Get messages available for forking.
    pub fn get_fork_messages(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "get_fork_messages" }))
    }

    /// Get text of last assistant message.
    pub fn get_last_assistant_text(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "get_last_assistant_text" }))
    }

    /// Set the session display name.
    pub fn set_session_name(&mut self, name: &str) -> Result<()> {
        self.send_command(serde_json::json!({
            "type": "set_session_name",
            "name": name
        }))
    }

    /// Get all messages.
    pub fn get_messages(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "get_messages" }))
    }

    /// Get available commands.
    pub fn get_commands(&mut self) -> Result<RpcResponse> {
        self.send_and_wait(serde_json::json!({ "type": "get_commands" }))
    }

    // ── Internal helpers ─────────────────────────────────────────

    /// Generate the next request ID.
    fn next_request_id(&mut self) -> String {
        self.request_counter += 1;
        format!("req_{}", self.request_counter)
    }

    /// Send a fire-and-forget command (no response expected).
    fn send_command(&mut self, mut command: Value) -> Result<()> {
        let id = self.next_request_id();
        if let Some(obj) = command.as_object_mut() {
            obj.insert("id".to_string(), Value::String(id));
        }

        let line = serialize_json_line(&command);
        let child = self.child.as_mut().context("Client not started")?;
        if let Some(ref mut stdin) = child.stdin {
            stdin
                .write_all(line.as_bytes())
                .context("Failed to write to stdin")?;
            stdin.flush().context("Failed to flush stdin")?;
        }

        Ok(())
    }

    /// Send a command and wait for the response.
    fn send_and_wait(&mut self, mut command: Value) -> Result<RpcResponse> {
        let id = self.next_request_id();
        if let Some(obj) = command.as_object_mut() {
            obj.insert("id".to_string(), Value::String(id.clone()));
        }

        let line = serialize_json_line(&command);

        // Write command
        {
            let child = self.child.as_mut().context("Client not started")?;
            if let Some(ref mut stdin) = child.stdin {
                stdin
                    .write_all(line.as_bytes())
                    .context("Failed to write to stdin")?;
                stdin.flush().context("Failed to flush stdin")?;
            }
        }

        // Read responses until we find ours
        let child = self.child.as_mut().context("Client not started")?;
        if let Some(ref mut stdout) = child.stdout {
            let mut buf_reader = std::io::BufReader::new(std::io::BufReader::new(stdout));
            let mut buf = String::new();
            loop {
                buf.clear();
                match buf_reader.read_line(&mut buf) {
                    Ok(0) => anyhow::bail!("EOF while waiting for response"),
                    Ok(_) => {
                        let trimmed = buf.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match parse_json_line(trimmed) {
                            Ok(value) => {
                                if let Some(obj) = value.as_object() {
                                    // Check if it's our response
                                    if obj.get("type").and_then(|v| v.as_str()) == Some("response")
                                        && obj.get("id").and_then(|v| v.as_str()) == Some(id.as_str())
                                    {
                                        let success = obj
                                            .get("success")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false);
                                        let cmd_name = obj
                                            .get("command")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let data = obj.get("data").cloned();
                                        let error = obj
                                            .get("error")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string());
                                        return Ok(RpcResponse::Response {
                                            id: Some(id.clone()),
                                            command: cmd_name,
                                            success,
                                            data,
                                            error,
                                        });
                                    }

                                    // Otherwise it's an event — parse and notify listeners
                                    let event_type = obj
                                        .get("type")
                                        .and_then(|v: &Value| v.as_str())
                                        .unwrap_or("");
                                    let event = match event_type {
                                        "agent_start" => Some(RpcEvent::AgentStart),
                                        "agent_end" => Some(RpcEvent::AgentEnd),
                                        "thinking" => Some(RpcEvent::Thinking),
                                        "error" => {
                                            let msg = obj
                                                .get("message")
                                                .and_then(|v: &Value| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            Some(RpcEvent::Error { message: msg })
                                        }
                                        "text_chunk" => {
                                            let text = obj
                                                .get("text")
                                                .and_then(|v: &Value| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            Some(RpcEvent::TextChunk { text })
                                        }
                                        "tool_start" => {
                                            let tool = obj
                                                .get("tool")
                                                .and_then(|v: &Value| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            Some(RpcEvent::ToolStart { tool })
                                        }
                                        "tool_end" => {
                                            let tool = obj
                                                .get("tool")
                                                .and_then(|v: &Value| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            Some(RpcEvent::ToolEnd { tool })
                                        }
                                        _ => None,
                                    };
                                    if let Some(event) = event {
                                        for listener in &self.event_listeners {
                                            listener(event.clone());
                                        }
                                    }
                                }
                            }
                            Err(_) => continue,
                        }
                    }
                    Err(e) => anyhow::bail!("Error reading stdout: {}", e),
                }
            }
        } else {
            anyhow::bail!("No stdout available");
        }
    }

    /// Handle an incoming JSON value during send_and_wait.
    /// Returns Ok(response) if it matches our request ID, otherwise processes as event.
    fn handle_incoming_value(
        &mut self,
        expected_id: &str,
        value: Value,
    ) -> Result<RpcResponse> {
        if let Some(obj) = value.as_object() {
            // Check if it's our response
            if obj.get("type").and_then(|v| v.as_str()) == Some("response")
                && obj.get("id").and_then(|v| v.as_str()) == Some(expected_id)
            {
                let success = obj
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let command = obj
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let data = obj.get("data").cloned();
                let error = obj
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                return Ok(RpcResponse::Response {
                    id: Some(expected_id.to_string()),
                    command,
                    success,
                    data,
                    error,
                });
            }

            // Otherwise it's an event — try to parse and notify listeners
            let event_type = obj
                .get("type")
                .and_then(|v: &Value| v.as_str())
                .unwrap_or("");
            let event = match event_type {
                "agent_start" => Some(RpcEvent::AgentStart),
                "agent_end" => Some(RpcEvent::AgentEnd),
                "thinking" => Some(RpcEvent::Thinking),
                "error" => {
                    let msg = obj
                        .get("message")
                        .and_then(|v: &Value| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(RpcEvent::Error { message: msg })
                }
                "text_chunk" => {
                    let text = obj
                        .get("text")
                        .and_then(|v: &Value| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(RpcEvent::TextChunk { text })
                }
                "tool_start" => {
                    let tool = obj
                        .get("tool")
                        .and_then(|v: &Value| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(RpcEvent::ToolStart { tool })
                }
                "tool_end" => {
                    let tool = obj
                        .get("tool")
                        .and_then(|v: &Value| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(RpcEvent::ToolEnd { tool })
                }
                _ => None,
            };
            if let Some(event) = event {
                for listener in &self.event_listeners {
                    listener(event.clone());
                }
            }
        }

        // Not our response — loop continues (caller will read next line)
        // This is a hack: we return an error to signal "continue reading"
        // Better approach: use a loop inside send_and_wait
        anyhow::bail!("Internal: not our response, caller should retry")
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ============================================================================
// Bracketed Paste Detection
// ============================================================================

/// Bracketed paste state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteState {
    /// Normal mode
    Normal,
    /// Bracketed paste mode - collecting paste data
    Pasting,
}

/// Bracketed paste handler
pub struct PasteHandler {
    state: PasteState,
    buffer: Vec<u8>,
    /// Bracketed paste start sequence: ESC [ 2 0 0 ~
    start_sequence: Vec<u8>,
    /// Bracketed paste end sequence: ESC [ 2 0 1 ~
    end_sequence: Vec<u8>,
}

impl PasteHandler {
    /// Create a new paste handler
    pub fn new() -> Self {
        Self {
            state: PasteState::Normal,
            buffer: Vec::new(),
            start_sequence: vec![0x1B, 0x5B, 0x32, 0x30, 0x30, 0x7E], // \x1b[200~
            end_sequence: vec![0x1B, 0x5B, 0x32, 0x30, 0x31, 0x7E],   // \x1b[201~
        }
    }

    /// Reset to normal mode
    pub fn reset(&mut self) {
        self.state = PasteState::Normal;
        self.buffer.clear();
    }

    /// Check current state
    pub fn state(&self) -> PasteState {
        self.state.clone()
    }

    /// Get collected paste buffer
    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }

    /// Process a byte, returning Some(byte) if it should be passed through
    /// or None if it was part of a paste sequence
    pub fn process_byte(&mut self, byte: u8) -> Option<u8> {
        match self.state {
            PasteState::Normal => {
                if self.buffer.is_empty() && byte == 0x1B {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 1 && self.buffer[0] == 0x1B && byte == 0x5B {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 2
                    && self.buffer[0] == 0x1B
                    && self.buffer[1] == 0x5B
                    && byte == 0x32
                {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 3
                    && self.buffer[0] == 0x1B
                    && self.buffer[1] == 0x5B
                    && self.buffer[2] == 0x32
                    && byte == 0x30
                {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 4
                    && self.buffer[0] == 0x1B
                    && self.buffer[1] == 0x5B
                    && self.buffer[2] == 0x32
                    && self.buffer[3] == 0x30
                    && byte == 0x30
                {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 5
                    && self.buffer[0] == 0x1B
                    && self.buffer[1] == 0x5B
                    && self.buffer[2] == 0x32
                    && self.buffer[3] == 0x30
                    && self.buffer[4] == 0x30
                    && byte == 0x7E
                {
                    // Paste start detected
                    self.buffer.clear();
                    self.state = PasteState::Pasting;
                    None
                } else {
                    let first_byte = self.buffer.first().copied();
                    self.buffer.clear();
                    first_byte
                }
            }
            PasteState::Pasting => {
                if self.buffer.is_empty() && byte == 0x1B {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 1 && self.buffer[0] == 0x1B && byte == 0x5B {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 2
                    && self.buffer[0] == 0x1B
                    && self.buffer[1] == 0x5B
                    && byte == 0x32
                {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 3
                    && self.buffer[0] == 0x1B
                    && self.buffer[1] == 0x5B
                    && self.buffer[2] == 0x32
                    && byte == 0x30
                {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 4
                    && self.buffer[0] == 0x1B
                    && self.buffer[1] == 0x5B
                    && self.buffer[2] == 0x32
                    && self.buffer[3] == 0x30
                    && byte == 0x31
                {
                    self.buffer.push(byte);
                    None
                } else if self.buffer.len() >= 5
                    && self.buffer[0] == 0x1B
                    && self.buffer[1] == 0x5B
                    && self.buffer[2] == 0x32
                    && self.buffer[3] == 0x30
                    && self.buffer[4] == 0x31
                    && byte == 0x7E
                {
                    // Paste end detected
                    self.buffer.clear();
                    self.state = PasteState::Normal;
                    None
                } else {
                    self.buffer.push(byte);
                    None
                }
            }
        }
    }

    /// Check if buffer ends with a sequence
    pub fn ends_with(&self, sequence: &[u8]) -> bool {
        if self.buffer.len() < sequence.len() {
            return false;
        }
        let end_pos = self.buffer.len() - sequence.len();
        &self.buffer[end_pos..] == sequence
    }

    /// Extract image data from clipboard paste
    pub fn extract_image_data(&self) -> Option<Vec<u8>> {
        let buffer = self.buffer();
        if buffer.len() < 8 {
            return None;
        }

        // PNG magic bytes
        if buffer.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Some(buffer.to_vec());
        }

        // JPEG magic bytes
        if buffer.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(buffer.to_vec());
        }

        // Binary data heuristic
        if buffer.iter().take(100).filter(|&&b| b == 0).count() > 5 {
            return Some(buffer.to_vec());
        }

        None
    }
}

impl Default for PasteHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── JSONL Tests ────────────────────────────────────────────────

    #[test]
    fn test_serialize_json_line() {
        let value = serde_json::json!({"type": "test", "data": 42});
        let line = serialize_json_line(&value);
        assert!(line.ends_with('\n'));
        assert!(!line.contains("\r\n"));
        let parsed: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["type"], "test");
        assert_eq!(parsed["data"], 42);
    }

    #[test]
    fn test_parse_json_line() {
        let line = r#"{"type":"test"}"#;
        let value = parse_json_line(line).unwrap();
        assert_eq!(value["type"], "test");
    }

    #[test]
    fn test_parse_json_line_trailing_cr() {
        let line = "{\"type\":\"test\"}\r";
        let value = parse_json_line(line).unwrap();
        assert_eq!(value["type"], "test");
    }

    #[test]
    fn test_jsonl_line_reader_basic() {
        let mut reader = JsonlLineReader::new();
        let lines = reader.feed("{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "{\"a\":1}");
        assert_eq!(lines[1], "{\"b\":2}");
    }

    #[test]
    fn test_jsonl_line_reader_partial() {
        let mut reader = JsonlLineReader::new();
        let lines = reader.feed("{\"a\":1");
        assert!(lines.is_empty());
        let lines2 = reader.feed("}\n");
        assert_eq!(lines2.len(), 1);
        assert_eq!(lines2[0], "{\"a\":1}");
    }

    #[test]
    fn test_jsonl_line_reader_flush() {
        let mut reader = JsonlLineReader::new();
        reader.feed("{\"partial\":true");
        assert!(reader.has_buffered_data());
        let final_line = reader.flush();
        assert_eq!(final_line, Some("{\"partial\":true".to_string()));
        assert!(!reader.has_buffered_data());
    }

    #[test]
    fn test_jsonl_line_reader_empty_lines() {
        let mut reader = JsonlLineReader::new();
        let lines = reader.feed("\n\n\n");
        assert!(lines.is_empty());
    }

    // ── JSON-RPC 2.0 Tests ────────────────────────────────────────

    #[test]
    fn test_jsonrpc_method_mapping() {
        let cmd = jsonrpc_to_command(
            "prompt",
            Some(serde_json::json!({"message": "hello"})),
            Some(Value::String("1".to_string())),
        );
        assert!(cmd.is_some());
        let cmd = cmd.unwrap();
        assert_eq!(cmd["type"], "prompt");
        assert_eq!(cmd["message"], "hello");
        assert_eq!(cmd["id"], "1");
    }

    #[test]
    fn test_jsonrpc_method_mapping_unknown() {
        let cmd = jsonrpc_to_command("nonexistent", None, Some(Value::String("1".to_string())));
        assert!(cmd.is_none());
    }

    #[test]
    fn test_jsonrpc_to_command_all_methods() {
        let methods = [
            "prompt", "steer", "follow_up", "abort", "new_session",
            "get_state", "set_model", "cycle_model", "get_available_models",
            "set_thinking_level", "cycle_thinking_level",
            "set_steering_mode", "set_follow_up_mode",
            "compact", "set_auto_compaction",
            "set_auto_retry", "abort_retry",
            "bash", "abort_bash",
            "get_session_stats", "export_html", "switch_session",
            "fork", "clone", "get_fork_messages",
            "get_last_assistant_text", "set_session_name",
            "get_messages", "get_commands",
        ];
        for method in methods {
            let cmd = jsonrpc_to_command(method, None, Some(Value::Number(1.into())));
            assert!(cmd.is_some(), "Method {} should map to a command", method);
        }
    }

    #[test]
    fn test_rpc_response_to_jsonrpc_success() {
        let response = RpcResponse::Response {
            id: Some("1".to_string()),
            command: "get_state".to_string(),
            success: true,
            data: Some(serde_json::json!({"session_id": "abc"})),
            error: None,
        };
        let json = rpc_response_to_jsonrpc(&response, Value::String("1".to_string()));
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], "1");
        assert_eq!(parsed["result"]["session_id"], "abc");
    }

    #[test]
    fn test_rpc_response_to_jsonrpc_error() {
        let response = RpcResponse::Response {
            id: Some("1".to_string()),
            command: "test".to_string(),
            success: false,
            data: None,
            error: Some("Something failed".to_string()),
        };
        let json = rpc_response_to_jsonrpc(&response, Value::String("1".to_string()));
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["error"]["code"], JSONRPC_INTERNAL_ERROR);
        assert_eq!(parsed["error"]["message"], "Something failed");
    }

    // ── RPC Server tests ─────────────────────────────────────────────

    #[test]
    fn test_rpc_server_new() {
        let server = RpcServer::new(8080);
        assert_eq!(server.port(), 8080);
        assert!(!server.is_shutdown_requested());
    }

    #[test]
    fn test_rpc_server_shutdown() {
        let server = Arc::new(RpcServer::new(0));
        assert!(!server.is_shutdown_requested());
        server.request_shutdown();
        assert!(server.is_shutdown_requested());
    }

    #[test]
    fn test_rpc_server_session_state() {
        let server = Arc::new(RpcServer::new(0));
        let state = server.get_session_state();
        assert_eq!(state.message_count, 0);
        assert_eq!(state.thinking_level, "default");
        assert!(state.auto_compaction_enabled);

        server.update_session_state(|s| {
            s.message_count = 10;
            s.thinking_level = "high".to_string();
        });

        let new_state = server.get_session_state();
        assert_eq!(new_state.message_count, 10);
        assert_eq!(new_state.thinking_level, "high");
    }

    #[test]
    fn test_rpc_server_model_state() {
        let server = Arc::new(RpcServer::new(0));
        assert!(server.get_session_state().model.is_none());

        server.update_session_state(|s| {
            s.model = Some(ModelInfo {
                provider: "anthropic".to_string(),
                id: "claude-3-opus".to_string(),
            });
        });

        let state = server.get_session_state();
        assert!(state.model.is_some());
        let model = state.model.unwrap();
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.id, "claude-3-opus");
    }

    #[test]
    fn test_parse_images_data_uri() {
        let images = vec![ImageData {
            source: "data:image/png;base64,iVBORw0KGgo=".to_string(),
            media_type: "image/png".to_string(),
        }];
        let parsed = RpcServer::parse_images(Some(images));
        assert_eq!(parsed.len(), 1);
        assert!(!parsed[0].data.is_empty());
        assert_eq!(parsed[0].mime_type, "image/png");
    }

    #[test]
    fn test_parse_images_empty() {
        let parsed = RpcServer::parse_images(None);
        assert!(parsed.is_empty());
    }

    #[test]
    fn test_parse_images_non_data_uri() {
        let images = vec![ImageData {
            source: "https://example.com/image.png".to_string(),
            media_type: "image/png".to_string(),
        }];
        let parsed = RpcServer::parse_images(Some(images));
        assert!(parsed.is_empty());
    }

    // ── Session Handoff Tests ─────────────────────────────────────────

    #[test]
    fn test_session_handoff_round_trip() {
        let state = SessionState {
            model: Some(ModelInfo {
                provider: "test".to_string(),
                id: "model-1".to_string(),
            }),
            thinking_level: "high".to_string(),
            is_streaming: false,
            is_compacting: false,
            steering_mode: "all".to_string(),
            follow_up_mode: "all".to_string(),
            session_file: Some("/tmp/session.json".to_string()),
            session_id: "abc-123".to_string(),
            session_name: Some("Test Session".to_string()),
            auto_compaction_enabled: true,
            message_count: 42,
            pending_message_count: 0,
        };

        let handoff = SessionHandoff::from_state(&state);
        assert_eq!(handoff.session_id, "abc-123");
        assert_eq!(handoff.message_count, 42);
        assert_eq!(handoff.model_id, Some("test/model-1".to_string()));

        let json = handoff.to_json().unwrap();
        let decoded = SessionHandoff::from_json(&json).unwrap();
        assert_eq!(decoded.session_id, "abc-123");
        assert_eq!(decoded.message_count, 42);
    }

    // ── Event Tests ─────────────────────────────────────────────────

    #[test]
    fn test_rpc_event_serialization() {
        let event = RpcEvent::TextChunk {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"text_chunk\""));
        assert!(json.contains("\"text\":\"hello\""));
    }

    #[test]
    fn test_rpc_event_agent_start() {
        let event = RpcEvent::AgentStart;
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"agent_start\""));
    }

    #[test]
    fn test_rpc_event_agent_end() {
        let event = RpcEvent::AgentEnd;
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"agent_end\""));
    }

    // ── Paste handler tests ──────────────────────────────────────────

    #[test]
    fn test_paste_handler_initial_state() {
        let handler = PasteHandler::new();
        assert_eq!(handler.state(), PasteState::Normal);
        assert!(handler.buffer().is_empty());
    }

    #[test]
    fn test_paste_handler_reset() {
        let mut handler = PasteHandler::new();
        handler.buffer.push(b't');
        handler.state = PasteState::Pasting;
        handler.reset();
        assert_eq!(handler.state(), PasteState::Normal);
        assert!(handler.buffer().is_empty());
    }

    #[test]
    fn test_paste_handler_paste_start_sequence() {
        let mut handler = PasteHandler::new();

        assert!(handler.process_byte(0x1B).is_none());
        assert_eq!(handler.state(), PasteState::Normal);
        assert!(handler.process_byte(0x5B).is_none());
        assert!(handler.process_byte(0x32).is_none());
        assert!(handler.process_byte(0x30).is_none());
        assert!(handler.process_byte(0x30).is_none());
        assert!(handler.process_byte(0x7E).is_none());

        assert_eq!(handler.state(), PasteState::Pasting);
    }

    #[test]
    fn test_paste_handler_extract_image_png() {
        let mut handler = PasteHandler::new();
        handler.buffer = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,
            0x00, 0x00, 0x00, 0x0D,
            0x49, 0x48, 0x44, 0x52,
        ];
        let result = handler.extract_image_data();
        assert!(result.is_some());
        assert!(result.unwrap().starts_with(&[0x89, 0x50, 0x4E, 0x47]));
    }

    #[test]
    fn test_paste_handler_extract_image_jpeg() {
        let mut handler = PasteHandler::new();
        handler.buffer = vec![
            0xFF, 0xD8, 0xFF, 0xE0,
            0x00, 0x10, 0x4A, 0x46,
        ];
        let result = handler.extract_image_data();
        assert!(result.is_some());
        assert!(result.unwrap().starts_with(&[0xFF, 0xD8, 0xFF]));
    }

    #[test]
    fn test_paste_handler_extract_image_none() {
        let mut handler = PasteHandler::new();
        handler.buffer = b"hello world".to_vec();
        let result = handler.extract_image_data();
        assert!(result.is_none());
    }

    #[test]
    fn test_paste_handler_extract_image_short_buffer() {
        let handler = PasteHandler::new();
        let result = handler.extract_image_data();
        assert!(result.is_none());
    }

    #[test]
    fn test_paste_handler_extract_image_with_nulls() {
        let mut handler = PasteHandler::new();
        let mut buffer = vec![0u8; 100];
        for i in 10..20 {
            buffer[i] = 0;
        }
        handler.buffer = buffer;
        let result = handler.extract_image_data();
        assert!(result.is_some());
    }

    // ── Session state serialization tests ───────────────────────────

    #[test]
    fn test_session_state_serialization() {
        let state = SessionState {
            model: Some(ModelInfo {
                provider: "anthropic".to_string(),
                id: "claude-3-opus".to_string(),
            }),
            thinking_level: "high".to_string(),
            is_streaming: true,
            is_compacting: false,
            steering_mode: "all".to_string(),
            follow_up_mode: "one_at_a_time".to_string(),
            session_file: Some("/tmp/session.json".to_string()),
            session_id: "test-123".to_string(),
            session_name: Some("Test Session".to_string()),
            auto_compaction_enabled: true,
            message_count: 42,
            pending_message_count: 1,
        };

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"thinking_level\":\"high\""));
        assert!(json.contains("\"is_streaming\":true"));
        assert!(json.contains("\"message_count\":42"));
        assert!(json.contains("\"session_file\":\"/tmp/session.json\""));
    }

    #[test]
    fn test_session_state_skips_optional_fields() {
        let state = SessionState {
            model: None,
            thinking_level: "default".to_string(),
            is_streaming: false,
            is_compacting: false,
            steering_mode: "all".to_string(),
            follow_up_mode: "all".to_string(),
            session_file: None,
            session_id: "test".to_string(),
            session_name: None,
            auto_compaction_enabled: true,
            message_count: 0,
            pending_message_count: 0,
        };

        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("\"model\""));
        assert!(!json.contains("\"session_file\""));
        assert!(!json.contains("\"session_name\""));
    }

    #[test]
    fn test_command_info_serialization() {
        let cmd = CommandInfo {
            name: "test".to_string(),
            description: Some("Test command".to_string()),
            source: "builtin".to_string(),
            source_info: None,
        };

        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("\"source\":\"builtin\""));
        assert!(!json.contains("\"source_info\""));
    }

    #[test]
    fn test_command_info_with_source_info() {
        let cmd = CommandInfo {
            name: "skill:deploy".to_string(),
            description: Some("Deploy skill".to_string()),
            source: "skill".to_string(),
            source_info: Some(SourceInfo {
                path: Some("~/.oxi/skills/deploy.md".to_string()),
                origin: Some("user".to_string()),
            }),
        };

        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"source_info\""));
        assert!(json.contains("deploy.md"));
    }

    // ── Image data tests ─────────────────────────────────────────────

    #[test]
    fn test_image_data_deserialization() {
        let json = r#"{"source":"data:image/png;base64,ABC123","type":"image/png"}"#;
        let data: ImageData = serde_json::from_str(json).unwrap();
        assert_eq!(data.source, "data:image/png;base64,ABC123");
        assert_eq!(data.media_type, "image/png");
    }

    // ── RPC Response tests ───────────────────────────────────────────

    #[test]
    fn test_rpc_response_success() {
        let response = RpcResponse::Response {
            id: Some("123".to_string()),
            command: "test".to_string(),
            success: true,
            data: Some(serde_json::json!({"result": "ok"})),
            error: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"command\":\"test\""));
    }

    #[test]
    fn test_rpc_response_error() {
        let response = RpcResponse::Response {
            id: Some("123".to_string()),
            command: "test".to_string(),
            success: false,
            data: None,
            error: Some("Something went wrong".to_string()),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"success\":false"));
        assert!(json.contains("\"error\":\"Something went wrong\""));
    }

    #[test]
    fn test_rpc_extension_ui_request_select() {
        let request = RpcExtensionUiRequest::Select {
            id: "req-123".to_string(),
            title: "Select an option".to_string(),
            options: vec!["Option 1".to_string(), "Option 2".to_string()],
            timeout: Some(5000),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"method\":\"select\""));
        assert!(json.contains("\"title\":\"Select an option\""));
    }

    #[test]
    fn test_rpc_extension_ui_request_notify() {
        let request = RpcExtensionUiRequest::Notify {
            id: "req-456".to_string(),
            message: "Hello!".to_string(),
            notify_type: Some("info".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"method\":\"notify\""));
        assert!(json.contains("\"message\":\"Hello!\""));
    }

    // ── Compaction result tests ──────────────────────────────────────

    #[test]
    fn test_compaction_result_serialization() {
        let result = CompactionResult {
            original_count: 100,
            compacted_count: 30,
            tokens_saved: Some(5000),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"original_count\":100"));
        assert!(json.contains("\"compacted_count\":30"));
    }

    #[test]
    fn test_session_stats_serialization() {
        let stats = SessionStats {
            message_count: 50,
            token_count: Some(10000),
            last_activity: Some(1699000000),
        };

        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"message_count\":50"));
        assert!(json.contains("\"token_count\":10000"));
    }

    // ── RpcOutput tests ──────────────────────────────────────────────

    #[test]
    fn test_rpc_output_write_obj() {
        let output = RpcOutput::new();
        // This just tests it doesn't panic
        let response = RpcResponse::Response {
            id: Some("test".to_string()),
            command: "test".to_string(),
            success: true,
            data: None,
            error: None,
        };
        output.write_obj(&response);
    }

    // ── Command deserialization tests ─────────────────────────────────

    #[test]
    fn test_deserialize_prompt_with_streaming_behavior() {
        let json = r#"{
            "type": "prompt",
            "message": "hello",
            "streaming_behavior": "steer"
        }"#;
        let cmd: RpcCommand = serde_json::from_str(json).unwrap();
        match cmd {
            RpcCommand::Prompt {
                streaming_behavior, ..
            } => {
                assert_eq!(streaming_behavior, Some("steer".to_string()));
            }
            _ => panic!("Expected Prompt command"),
        }
    }

    #[test]
    fn test_deserialize_prompt_without_streaming_behavior() {
        let json = r#"{"type": "prompt", "message": "hello"}"#;
        let cmd: RpcCommand = serde_json::from_str(json).unwrap();
        match cmd {
            RpcCommand::Prompt {
                streaming_behavior, ..
            } => {
                assert_eq!(streaming_behavior, None);
            }
            _ => panic!("Expected Prompt command"),
        }
    }

    // ── JSON-RPC error response tests ────────────────────────────────

    #[test]
    fn test_jsonrpc_error_response_serialization() {
        let resp = JsonRpcErrorResponse {
            jsonrpc: "2.0".to_string(),
            id: Value::Number(1.into()),
            error: JsonRpcError {
                code: JSONRPC_METHOD_NOT_FOUND,
                message: "Method not found".to_string(),
                data: None,
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"code\":-32601"));
    }

    #[test]
    fn test_jsonrpc_success_response_serialization() {
        let resp = JsonRpcSuccessResponse {
            jsonrpc: "2.0".to_string(),
            id: Value::String("abc".to_string()),
            result: serde_json::json!({"status": "ok"}),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"result\""));
        assert!(json.contains("\"status\":\"ok\""));
    }

    // ── RPC Client config tests ──────────────────────────────────────

    #[test]
    fn test_rpc_client_config_default() {
        let config = RpcClientConfig::default();
        assert_eq!(config.binary_path, "oxi");
        assert!(config.cwd.is_none());
        assert!(config.provider.is_none());
        assert!(config.model.is_none());
        assert!(config.args.is_empty());
    }

    // ── Source Info tests ────────────────────────────────────────────

    #[test]
    fn test_source_info_serialization() {
        let info = SourceInfo {
            path: Some("/path/to/ext".to_string()),
            origin: Some("user".to_string()),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("/path/to/ext"));
    }

    #[test]
    fn test_source_info_skip_empty() {
        let info = SourceInfo {
            path: None,
            origin: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("path"));
        assert!(!json.contains("origin"));
    }
}
