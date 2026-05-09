//! WASM hooks — stub module

#![allow(dead_code)]
#![allow(unused_imports)]

/// Stub pending notification
#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub message: String,
    pub level: String,
    pub timestamp: String,
}

/// Stub pending message
#[derive(Debug, Clone)]
pub struct PendingMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// Stub hook result
#[derive(Debug, Clone)]
pub struct ToolCallHookResult {
    pub block: bool,
    pub reason: Option<String>,
}

/// Stub tool result hook
#[derive(Debug, Clone)]
pub struct ToolResultHookResult {
    pub content: Option<String>,
    pub is_error: Option<bool>,
}

/// Stub hook manager
pub struct WasmHookManager;

impl WasmHookManager {
    pub fn new(_extensions: std::sync::Arc<super::wasm::WasmExtensionManager>) -> Self { Self }
    pub fn fire_tool_call(&self, _: &str, _: &str, _: &serde_json::Value) -> Option<ToolCallHookResult> { None }
    pub fn fire_tool_result(&self, _: &str, _: &str, _: &str, _: bool) -> Option<ToolResultHookResult> { None }
    pub fn fire_session_shutdown(&self, _: &str) {}
    pub fn fire_agent_event(&self, _: &str, _: &serde_json::Value) {}
    pub fn drain_notifications(&self) -> Vec<PendingNotification> { vec![] }
    pub fn drain_messages(&self) -> Vec<PendingMessage> { vec![] }
}
