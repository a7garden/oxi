//! WASM extension event hooks — types and hook manager.
//!
//! Provides bridge types for communication between WASM extensions and oxi's
//! TUI/agent. Extensions call `oxi_notify` and `oxi_send_message` host functions
//! that push to these queues; the TUI/agent loop drains them.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A pending notification from a WASM extension to be shown in TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingNotification {
    /// Extension name that sent the notification.
    pub extension: String,
    /// Notification message text.
    pub message: String,
    /// Notification level: "info", "warning", "error".
    #[serde(default = "default_info")]
    pub level: String,
}
fn default_info() -> String {
    "info".to_string()
}

/// A pending message injection from a WASM extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMessage {
    /// Extension name that sent the message.
    pub extension: String,
    /// Message content.
    pub content: String,
    /// Role for the message ("user", "system").
    #[serde(default = "default_user")]
    pub role: String,
}
fn default_user() -> String {
    "user".to_string()
}

/// Result from a tool_call hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallHookResult {
    /// Whether to block this tool call.
    #[serde(default)]
    pub block: bool,
    /// Reason for blocking (shown to LLM).
    #[serde(default)]
    pub reason: Option<String>,
}

/// Result from a tool_result hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultHookResult {
    /// Optional replacement content.
    #[serde(default)]
    pub content: Option<String>,
    /// Whether to override the error flag.
    #[serde(default)]
    pub is_error: Option<bool>,
}

/// Manages hook dispatching to WASM extensions.
pub struct WasmHookManager {
    extensions: Arc<super::wasm::WasmExtensionManager>,
    notifications: Arc<RwLock<Vec<PendingNotification>>>,
    messages: Arc<RwLock<Vec<PendingMessage>>>,
}

impl WasmHookManager {
    /// Create a new hook manager.
    pub fn new(extensions: Arc<super::wasm::WasmExtensionManager>) -> Self {
        Self {
            extensions,
            notifications: Arc::new(RwLock::new(Vec::new())),
            messages: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Fire `on_tool_call` for all extensions. Returns first block result.
    pub fn fire_tool_call(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        input: &serde_json::Value,
    ) -> Option<ToolCallHookResult> {
        let mut plugins = self.extensions.plugins.lock();
        for (ext_name, plugin) in plugins.iter_mut() {
            let event = serde_json::json!({
                "event": "tool_call", "tool_name": tool_name,
                "tool_call_id": tool_call_id, "input": input,
            });
            let Ok(s) = serde_json::to_string(&event) else {
                continue;
            };
            if let Ok(output) = plugin.call::<&str, &str>("on_tool_call", &s)
                && let Ok(result) = serde_json::from_str::<ToolCallHookResult>(output)
                && result.block
            {
                tracing::info!("Extension '{}' blocked tool_call '{}'", ext_name, tool_name);
                return Some(result);
            }
        }
        None
    }

    /// Fire `on_tool_result` for all extensions. Returns first modification.
    pub fn fire_tool_result(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        content: &str,
        is_error: bool,
    ) -> Option<ToolResultHookResult> {
        let mut plugins = self.extensions.plugins.lock();
        for (_, plugin) in plugins.iter_mut() {
            let event = serde_json::json!({
                "event": "tool_result", "tool_name": tool_name,
                "tool_call_id": tool_call_id, "content": content, "is_error": is_error,
            });
            let Ok(s) = serde_json::to_string(&event) else {
                continue;
            };
            if let Ok(output) = plugin.call::<&str, &str>("on_tool_result", &s)
                && let Ok(result) = serde_json::from_str::<ToolResultHookResult>(output)
                && (result.content.is_some() || result.is_error.is_some())
            {
                return Some(result);
            }
        }
        None
    }

    /// Fire `on_session_shutdown` for all extensions.
    pub fn fire_session_shutdown(&self, reason: &str) {
        let mut plugins = self.extensions.plugins.lock();
        for (_, plugin) in plugins.iter_mut() {
            let event = serde_json::json!({ "event": "session_shutdown", "reason": reason });
            let Ok(s) = serde_json::to_string(&event) else {
                continue;
            };
            let _ = plugin.call::<&str, &str>("on_session_shutdown", &s);
        }
    }

    /// Fire `on_agent_event` for all extensions (fire-and-forget).
    pub fn fire_agent_event(&self, event_name: &str, event_data: &serde_json::Value) {
        let mut plugins = self.extensions.plugins.lock();
        for (_, plugin) in plugins.iter_mut() {
            let payload = serde_json::json!({ "event": event_name, "data": event_data });
            let Ok(s) = serde_json::to_string(&payload) else {
                continue;
            };
            let _ = plugin.call::<&str, &str>("on_agent_event", &s);
        }
    }

    /// Drain all pending notifications. Called by TUI.
    pub fn drain_notifications(&self) -> Vec<PendingNotification> {
        std::mem::take(&mut *self.notifications.write())
    }

    /// Drain all pending messages. Called by agent loop.
    pub fn drain_messages(&self) -> Vec<PendingMessage> {
        std::mem::take(&mut *self.messages.write())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_results() {
        let r: ToolCallHookResult =
            serde_json::from_str(r#"{"block":true,"reason":"Dangerous"}"#).unwrap();
        assert!(r.block);
        assert_eq!(r.reason.as_deref(), Some("Dangerous"));

        let r: ToolResultHookResult = serde_json::from_str(r#"{"content":"Modified"}"#).unwrap();
        assert_eq!(r.content.as_deref(), Some("Modified"));
    }

    #[test]
    fn test_notification_serde() {
        let n = PendingNotification {
            extension: "ext".into(),
            message: "Hello".into(),
            level: "warning".into(),
        };
        let json = serde_json::to_string(&n).unwrap();
        let back: PendingNotification = serde_json::from_str(&json).unwrap();
        assert_eq!(back.extension, "ext");
    }
}
