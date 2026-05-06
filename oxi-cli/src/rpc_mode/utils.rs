//! Utility types: paste handling and RPC client.

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use tokio::sync::oneshot;

use super::protocol::*;

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
    /// Current paste state.
    pub state: PasteState,
    /// Buffer for collecting paste data.
    pub buffer: Vec<u8>,
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
    pending_requests: HashMap<String, oneshot::Sender<RpcResponse>>,
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
            pending_requests: HashMap::new(),
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
