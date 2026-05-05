//! Extension types: enums, structs, events, and emit results.
//!
//! This module contains all the data types used across the extension system.

use oxi_ai::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

// Re-export from oxi-agent
pub use oxi_agent::{AgentEvent, AgentTool, AgentToolResult};

// ═══════════════════════════════════════════════════════════════════════════
// Extension Permissions
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPermission {
    FileRead,
    FileWrite,
    Bash,
    Network,
}

impl fmt::Display for ExtensionPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtensionPermission::FileRead => write!(f, "file_read"),
            ExtensionPermission::FileWrite => write!(f, "file_write"),
            ExtensionPermission::Bash => write!(f, "bash"),
            ExtensionPermission::Network => write!(f, "network"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Manifest
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub permissions: Vec<ExtensionPermission>,
    #[serde(default)]
    pub config_schema: Option<Value>,
}

impl ExtensionManifest {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            description: String::new(),
            author: String::new(),
            permissions: Vec::new(),
            config_schema: None,
        }
    }
    pub fn with_description(mut self, desc: impl Into<String>) -> Self { self.description = desc.into(); self }
    pub fn with_author(mut self, author: impl Into<String>) -> Self { self.author = author.into(); self }
    pub fn with_permission(mut self, perm: ExtensionPermission) -> Self {
        if !self.permissions.contains(&perm) { self.permissions.push(perm); }
        self
    }
    pub fn with_config_schema(mut self, schema: Value) -> Self { self.config_schema = Some(schema); self }
    pub fn has_permission(&self, perm: ExtensionPermission) -> bool { self.permissions.contains(&perm) }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Error Handling
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("Extension '{name}' not found")]
    NotFound { name: String },
    #[error("Failed to load extension '{name}': {reason}")]
    LoadFailed { name: String, reason: String },
    #[error("Extension '{name}' hook '{hook}' failed: {error}")]
    HookFailed { name: String, hook: String, error: String },
    #[error("Extension '{name}' requires permission '{permission}'")]
    PermissionDenied { name: String, permission: ExtensionPermission },
    #[error("Extension '{name}' is disabled")]
    Disabled { name: String },
    #[error("Hot-reload of extension '{name}' failed: {reason}")]
    HotReloadFailed { name: String, reason: String },
    #[error("Invalid configuration for extension '{name}': {reason}")]
    InvalidConfig { name: String, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionErrorRecord {
    pub extension_name: String,
    pub event: String,
    pub error: String,
    #[serde(default)]
    pub stack: Option<String>,
    pub timestamp: i64,
}

impl ExtensionErrorRecord {
    pub fn new(extension_name: impl Into<String>, event: impl Into<String>, error: impl Into<String>) -> Self {
        Self { extension_name: extension_name.into(), event: event.into(), error: error.into(), stack: None, timestamp: chrono::Utc::now().timestamp_millis() }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Event Enums
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSwitchReason { New, Resume }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionShutdownReason { Quit, Reload, New, Resume, Fork }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectSource { Set, Cycle, Restore }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource { Interactive, Rpc, Extension }

#[derive(Debug, Clone)]
pub enum InputEventResult { Continue, Transform { text: String }, Handled }

// ═══════════════════════════════════════════════════════════════════════════
// Event Structs
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)] pub struct SessionBeforeSwitchEvent { pub reason: SessionSwitchReason, pub target_session_file: Option<String> }
#[derive(Debug, Clone)] pub struct SessionBeforeForkEvent { pub entry_id: String, pub position: String }
#[derive(Debug, Clone)] pub struct SessionBeforeCompactEvent { pub messages_count: usize, pub tokens_before: usize, pub target_tokens: usize, pub custom_instructions: Option<String> }
#[derive(Debug, Clone)] pub struct SessionCompactEvent { pub messages_count: usize, pub tokens_after: usize, pub from_extension: bool }
#[derive(Debug, Clone)] pub struct SessionShutdownEvent { pub reason: SessionShutdownReason, pub target_session_file: Option<String> }
#[derive(Debug, Clone)] pub struct SessionBeforeTreeEvent { pub target_id: String, pub old_leaf_id: Option<String> }
#[derive(Debug, Clone)] pub struct SessionTreeEvent { pub new_leaf_id: Option<String>, pub old_leaf_id: Option<String>, pub from_extension: bool }
#[derive(Debug, Clone)] pub struct ContextEvent { pub messages: Vec<Message> }
#[derive(Debug, Clone)] pub struct BeforeProviderRequestEvent { pub payload: Value }
#[derive(Debug, Clone)] pub struct AfterProviderResponseEvent { pub status: u16, pub headers: HashMap<String, String> }
#[derive(Debug, Clone)] pub struct ModelSelectEvent { pub model: String, pub previous_model: Option<String>, pub source: ModelSelectSource }
#[derive(Debug, Clone)] pub struct ThinkingLevelSelectEvent { pub level: String, pub previous_level: String }
#[derive(Debug, Clone)] pub struct BashEvent { pub command: String, pub exclude_from_context: bool, pub cwd: PathBuf }
#[derive(Debug, Clone)] pub struct InputEvent { pub text: String, pub source: InputSource }

// ═══════════════════════════════════════════════════════════════════════════
// Extension State
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState { Pending, Active, Disabled, Failed, Unloaded }

impl fmt::Display for ExtensionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtensionState::Pending => write!(f, "pending"),
            ExtensionState::Active => write!(f, "active"),
            ExtensionState::Disabled => write!(f, "disabled"),
            ExtensionState::Failed => write!(f, "failed"),
            ExtensionState::Unloaded => write!(f, "unloaded"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Emit Result Types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug)] pub struct ToolCallEmitResult { pub blocked: bool, pub block_reason: Option<String>, pub errors: Vec<(String, String)> }
impl Default for ToolCallEmitResult { fn default() -> Self { Self { blocked: false, block_reason: None, errors: Vec::new() } } }

#[derive(Debug)] pub struct ToolResultEmitResult { pub output: Option<String>, pub success: Option<bool>, pub errors: Vec<(String, String)> }
impl Default for ToolResultEmitResult { fn default() -> Self { Self { output: None, success: None, errors: Vec::new() } } }

#[derive(Debug)] pub struct ContextEmitResult { pub modified: bool, pub messages: Vec<Message>, pub errors: Vec<(String, String)> }

#[derive(Debug)] pub struct ProviderRequestEmitResult { pub modified: bool, pub payload: Value, pub errors: Vec<(String, String)> }

#[derive(Debug)] pub struct SessionBeforeEmitResult { pub cancelled: bool, pub cancelled_by: Option<String>, pub errors: Vec<(String, String)> }
impl Default for SessionBeforeEmitResult { fn default() -> Self { Self { cancelled: false, cancelled_by: None, errors: Vec::new() } } }

// ═══════════════════════════════════════════════════════════════════════════
// Commands
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct Command { pub name: String, pub description: String, pub usage: String }
impl Command { pub fn new(name: impl Into<String>, description: impl Into<String>, usage: impl Into<String>) -> Self { Self { name: name.into(), description: description.into(), usage: usage.into() } } }

// ═══════════════════════════════════════════════════════════════════════════
// Error Listener
// ═══════════════════════════════════════════════════════════════════════════

pub type ExtensionErrorListener = dyn Fn(&ExtensionErrorRecord) + Send + Sync;
// Self-re-exports for module path consistency
