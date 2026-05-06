//! JSONL framing, JSON-RPC 2.0 types, RPC command/response/event types,
//! and serialization helpers.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::sync::Arc;

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
    /// JSON-RPC protocol version.
    pub jsonrpc: String, // must be "2.0"
    #[serde(default)]
    /// Request correlation ID.
    pub id: Option<Value>,
    /// JSON-RPC method name.
    pub method: String,
    #[serde(default)]
    /// Method parameters.
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 success response
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcSuccessResponse {
    /// JSON-RPC protocol version.
    pub jsonrpc: String,
    /// Request correlation ID.
    pub id: Value,
    /// Result value.
    pub result: Value,
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i64,
    /// Error message.
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Response data.
    pub data: Option<Value>,
}

/// JSON-RPC 2.0 error response
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcErrorResponse {
    /// JSON-RPC protocol version.
    pub jsonrpc: String,
    /// Request correlation ID.
    pub id: Value,
    /// Error message.
    pub error: JsonRpcError,
}

// Standard JSON-RPC error codes
/// pub.
pub const JSONRPC_PARSE_ERROR: i64 = -32700;
/// pub.
pub const JSONRPC_INVALID_REQUEST: i64 = -32600;
/// pub.
pub const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
/// pub.
pub const JSONRPC_INVALID_PARAMS: i64 = -32602;
/// pub.
pub const JSONRPC_INTERNAL_ERROR: i64 = -32603;

// ============================================================================
// RPC Types
// ============================================================================

/// RPC command from client
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcCommand {
    // ── Prompting ───────────────────────────────────────────────────
    /// Send a prompt message to the agent.
    Prompt {
        /// TODO: document.
        id: Option<String>,
        /// TODO: document.
        message: String,
        /// TODO: document.
        images: Option<Vec<ImageData>>,
        #[serde(default)]
        /// TODO: document.
        streaming_behavior: Option<String>,
    },
    /// Send a steering message to interrupt the current stream.
    Steer {
        /// TODO: document.
        id: Option<String>,
        /// TODO: document.
        message: String,
        /// TODO: document.
        images: Option<Vec<ImageData>>,
    },
    /// Send a follow-up message to be processed after the current stream.
    FollowUp {
        /// TODO: document.
        id: Option<String>,
        /// TODO: document.
        message: String,
        /// TODO: document.
        images: Option<Vec<ImageData>>,
    },
    /// Abort the current agent response.
    Abort {
        /// TODO: document.
        id: Option<String>,
    },
    /// Create a new session.
    NewSession {
        /// TODO: document.
        id: Option<String>,
        /// TODO: document.
        parent_session: Option<String>,
    },

    // ── State ───────────────────────────────────────────────────────
    /// Get the current agent state.
    GetState {
        /// TODO: document.
        id: Option<String>,
    },

    // ── Model ──────────────────────────────────────────────────────
    /// Set the active model.
    SetModel {
        /// TODO: document.
        id: Option<String>,
        /// TODO: document.
        provider: String,
        /// TODO: document.
        model_id: String,
    },
    /// Cycle to the next available model.
    CycleModel {
        /// TODO: document.
        id: Option<String>,
    },
    /// List all available models.
    GetAvailableModels {
        /// TODO: document.
        id: Option<String>,
    },

    // ── Thinking ────────────────────────────────────────────────────
    /// Set the thinking level.
    SetThinkingLevel {
        /// TODO: document.
        id: Option<String>,
        /// level.
        level: String,
    },
    /// Cycle to the next thinking level.
    CycleThinkingLevel {
        /// TODO: document.
        id: Option<String>,
    },

    // ── Queue modes ─────────────────────────────────────────────────
    /// Set the steering mode.
    SetSteeringMode {
        /// TODO: document.
        id: Option<String>,
        /// mode.
        mode: String,
    },
    /// Set the follow-up mode.
    SetFollowUpMode {
        /// TODO: document.
        id: Option<String>,
        /// mode.
        mode: String,
    },

    // ── Compaction ──────────────────────────────────────────────────
    /// Manually trigger compaction.
    Compact {
        /// TODO: document.
        id: Option<String>,
        /// custom instructions.
        custom_instructions: Option<String>,
    },
    /// Enable or disable auto-compaction.
    SetAutoCompaction {
        /// TODO: document.
        id: Option<String>,
        /// enabled.
        enabled: bool,
    },

    // ── Retry ───────────────────────────────────────────────────────
    /// Enable or disable auto-retry.
    SetAutoRetry {
        /// TODO: document.
        id: Option<String>,
        /// enabled.
        enabled: bool,
    },
    /// Abort the current retry attempt.
    AbortRetry {
        /// TODO: document.
        id: Option<String>,
    },

    // ── Bash ────────────────────────────────────────────────────────
    /// Execute a bash command.
    Bash {
        /// TODO: document.
        id: Option<String>,
        /// command.
        command: String,
    },
    /// Abort the running bash command.
    AbortBash {
        /// TODO: document.
        id: Option<String>,
    },

    // ── Session ────────────────────────────────────────────────────
    /// Get session statistics.
    GetSessionStats {
        /// TODO: document.
        id: Option<String>,
    },
    /// Export the session as HTML.
    ExportHtml {
        /// TODO: document.
        id: Option<String>,
        /// output path.
        output_path: Option<String>,
    },
    /// Switch to a different session.
    SwitchSession {
        /// TODO: document.
        id: Option<String>,
        /// session path.
        session_path: String,
    },
    /// Fork the session at a given entry.
    Fork {
        /// TODO: document.
        id: Option<String>,
        /// entry id.
        entry_id: String,
    },
    /// Clone the current session.
    Clone {
        /// TODO: document.
        id: Option<String>,
    },
    /// Get messages from the forked session.
    GetForkMessages {
        /// TODO: document.
        id: Option<String>,
    },
    /// Get the last assistant text response.
    GetLastAssistantText {
        /// TODO: document.
        id: Option<String>,
    },
    /// Set the session display name.
    SetSessionName {
        /// TODO: document.
        id: Option<String>,
        /// name.
        name: String,
    },

    // ── Messages ───────────────────────────────────────────────────
    /// Get all session messages.
    GetMessages {
        /// TODO: document.
        id: Option<String>,
    },

    // ── Commands ────────────────────────────────────────────────────
    /// get commands.
    GetCommands {
        /// TODO: document.
        id: Option<String>,
    },
}

/// Image data for RPC commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    /// source.
    pub source: String,
    #[serde(rename = "type")]
    /// Image media type.
    pub media_type: String,
}

/// Base64-encoded image data
#[derive(Debug, Clone)]
pub struct RpcImageSource {
    /// Response data.
    pub data: Vec<u8>,
    /// mime type.
    pub mime_type: String,
}

/// RPC response to client
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcResponse {
    /// response.
    Response {
        /// TODO: document.
        id: Option<String>,
        /// TODO: document.
        command: String,
        /// TODO: document.
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        data: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        error: Option<String>,
    },
    /// extension ui request.
    ExtensionUiRequest(RpcExtensionUiRequest),
}

/// Extension UI request
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum RpcExtensionUiRequest {
    /// select.
    Select {
        /// TODO: document.
        id: String,
        /// TODO: document.
        title: String,
        /// TODO: document.
        options: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        timeout: Option<u64>,
    },
    /// confirm.
    Confirm {
        /// TODO: document.
        id: String,
        /// TODO: document.
        title: String,
        /// TODO: document.
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        timeout: Option<u64>,
    },
    /// input.
    Input {
        /// TODO: document.
        id: String,
        /// TODO: document.
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        placeholder: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        timeout: Option<u64>,
    },
    /// editor.
    Editor {
        /// TODO: document.
        id: String,
        /// TODO: document.
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        prefill: Option<String>,
    },
    /// notify.
    Notify {
        /// TODO: document.
        id: String,
        /// TODO: document.
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        notify_type: Option<String>,
    },
    /// set status.
    SetStatus {
        /// TODO: document.
        id: String,
        /// TODO: document.
        status_key: String,
        /// TODO: document.
        status_text: Option<String>,
    },
    /// set widget.
    SetWidget {
        /// TODO: document.
        id: String,
        /// TODO: document.
        widget_key: String,
        /// TODO: document.
        widget_lines: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// TODO: document.
        widget_placement: Option<String>,
    },
    /// set title.
    SetTitle {
        /// TODO: document.
        id: String,
        /// TODO: document.
        title: String,
    },
    /// set editor text.
    SetEditorText {
        /// TODO: document.
        id: String,
        /// TODO: document.
        text: String,
    },
}

/// Extension UI response from client
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcExtensionUiResponse {
    /// Respond to an extension UI request.
    ExtensionUiResponse {
        /// TODO: document.
        id: String,
        #[serde(default)]
        /// TODO: document.
        value: Option<String>,
        #[serde(default)]
        /// TODO: document.
        confirmed: Option<bool>,
        #[serde(default)]
        /// TODO: document.
        cancelled: Option<bool>,
    },
}

// ============================================================================
// State / Info Types
// ============================================================================

/// Session state for get_state response
#[derive(Debug, Clone, Serialize)]
pub struct SessionState {
    /// Current model info
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelInfo>,
    /// thinking level.
    pub thinking_level: String,
    /// is streaming.
    pub is_streaming: bool,
    /// is compacting.
    pub is_compacting: bool,
    /// steering mode.
    pub steering_mode: String,
    /// follow up mode.
    pub follow_up_mode: String,
    /// Session file path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    /// Session identifier.
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// session name.
    pub session_name: Option<String>,
    /// auto compaction enabled.
    pub auto_compaction_enabled: bool,
    /// message count.
    pub message_count: usize,
    /// pending message count.
    pub pending_message_count: usize,
}

/// Model information for session state
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    /// Provider name.
    pub provider: String,
    /// Request correlation ID.
    pub id: String,
}

/// Command info for get_commands response
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    /// Name value.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// description.
    pub description: Option<String>,
    /// source.
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// source info.
    pub source_info: Option<SourceInfo>,
}

/// Source info for command provenance
#[derive(Debug, Clone, Serialize)]
pub struct SourceInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// File path.
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// origin.
    pub origin: Option<String>,
}

/// Result of a session stat query
#[derive(Debug, Clone, Serialize)]
pub struct SessionStats {
    /// message count.
    pub message_count: usize,
    /// token count.
    pub token_count: Option<usize>,
    /// last activity.
    pub last_activity: Option<i64>,
}

/// Result of compaction
#[derive(Debug, Clone, Serialize)]
pub struct CompactionResult {
    /// original count.
    pub original_count: usize,
    /// compacted count.
    pub compacted_count: usize,
    /// tokens saved.
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
    TextChunk {
        /// Streaming text content.
        text: String,
    },
    /// Agent is thinking
    Thinking,
    /// Tool execution started
    ToolStart {
        /// Name of the tool being executed.
        tool: String,
    },
    /// Tool execution finished
    ToolEnd {
        /// Name of the tool that finished.
        tool: String,
    },
    /// Agent finished processing
    AgentEnd,
    /// Error occurred
    Error {
        /// Error message.
        message: String,
    },
    /// Extension error
    ExtensionError {
        /// TODO: document.
        extension_path: String,
        /// TODO: document.
        event: String,
        /// TODO: document.
        error: String,
    },
}

// ============================================================================
// Pending Extension UI Request
// ============================================================================

/// Tracks pending extension UI requests awaiting client responses.
pub struct PendingExtensionRequest {
    /// resolve.
    pub resolve: tokio::sync::oneshot::Sender<RpcExtensionUiResponse>,
}

// ============================================================================
// JSON-RPC 2.0 Method Mapping
// ============================================================================

/// Map a JSON-RPC 2.0 method + params to an internal RPC command value.
pub fn jsonrpc_to_command(method: &str, params: Option<Value>, id: Option<Value>) -> Option<Value> {
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
pub fn rpc_response_to_jsonrpc(response: &RpcResponse, rpc_id: Value) -> String {
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
