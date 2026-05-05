//! Extension types: enums, structs, events, and emit results.
//!
//! This module contains all the data types used across the extension system,
//! including permission enums, manifest, error types, event structures,
//! command definitions, and emit result types.

use oxi_ai::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

// Re-export common types used across modules
pub use oxi_agent::{AgentEvent, AgentTool, AgentToolResult};

// ═══════════════════════════════════════════════════════════════════════════
// Extension Permissions
// ═══════════════════════════════════════════════════════════════════════════

/// Permissions that an extension may request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPermission {
    /// Read files from the filesystem
    FileRead,
    /// Write/modify files on the filesystem
    FileWrite,
    /// Execute shell commands via bash
    Bash,
    /// Make network requests
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

/// Metadata describing an extension.
///
/// Every extension must provide a manifest (either statically via the trait
/// or loaded from a manifest file alongside the shared library).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    /// Unique extension name (e.g. "my-deploy-tool")
    pub name: String,
    /// Semantic version string (e.g. "1.2.0")
    pub version: String,
    /// Human-readable description
    #[serde(default)]
    pub description: String,
    /// Author / maintainer
    #[serde(default)]
    pub author: String,
    /// Permissions required by this extension
    #[serde(default)]
    pub permissions: Vec<ExtensionPermission>,
    /// Optional JSON Schema for extension-specific configuration
    #[serde(default)]
    pub config_schema: Option<Value>,
}

impl ExtensionManifest {
    /// Create a minimal manifest with just a name and version.
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

    /// Builder: set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Builder: set author.
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = author.into();
        self
    }

    /// Builder: add a permission.
    pub fn with_permission(mut self, perm: ExtensionPermission) -> Self {
        if !self.permissions.contains(&perm) {
            self.permissions.push(perm);
        }
        self
    }

    /// Builder: set config schema.
    pub fn with_config_schema(mut self, schema: Value) -> Self {
        self.config_schema = Some(schema);
        self
    }

    /// Check whether the extension requests a particular permission.
    pub fn has_permission(&self, perm: ExtensionPermission) -> bool {
        self.permissions.contains(&perm)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Error Handling
// ═══════════════════════════════════════════════════════════════════════════

/// Errors that can occur during extension operations.
#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    /// The extension was not found in the registry.
    #[error("Extension '{name}' not found")]
    NotFound { name: String },

    /// The extension failed to load.
    #[error("Failed to load extension '{name}': {reason}")]
    LoadFailed { name: String, reason: String },

    /// The extension failed during a lifecycle hook.
    #[error("Extension '{name}' hook '{hook}' failed: {error}")]
    HookFailed {
        name: String,
        hook: String,
        error: String,
    },

    /// A required permission was not granted.
    #[error("Extension '{name}' requires permission '{permission}'")]
    PermissionDenied {
        name: String,
        permission: ExtensionPermission,
    },

    /// The extension is disabled and cannot be used.
    #[error("Extension '{name}' is disabled")]
    Disabled { name: String },

    /// A hot-reload operation failed.
    #[error("Hot-reload of extension '{name}' failed: {reason}")]
    HotReloadFailed { name: String, reason: String },

    /// Configuration validation failed.
    #[error("Invalid configuration for extension '{name}': {reason}")]
    InvalidConfig { name: String, reason: String },
}

/// Recorded extension error for diagnostics and logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionErrorRecord {
    /// Name of the extension that caused the error.
    pub extension_name: String,
    /// The event or hook during which the error occurred.
    pub event: String,
    /// Error message.
    pub error: String,
    /// Optional stack trace (best-effort).
    #[serde(default)]
    pub stack: Option<String>,
    /// Timestamp of the error.
    pub timestamp: i64,
}

impl ExtensionErrorRecord {
    /// Create a new error record with the current timestamp.
    pub fn new(
        extension_name: impl Into<String>,
        event: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            extension_name: extension_name.into(),
            event: event.into(),
            error: error.into(),
            stack: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Event Data
// ═══════════════════════════════════════════════════════════════════════════

/// Reason for a session switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSwitchReason {
    /// Creating a brand-new session.
    New,
    /// Resuming an existing session.
    Resume,
}

/// Reason for a session shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionShutdownReason {
    /// Application is quitting.
    Quit,
    /// Extensions are being reloaded.
    Reload,
    /// Switching to a new session.
    New,
    /// Resuming a different session.
    Resume,
    /// Forking the session.
    Fork,
}

/// Reason for a model selection change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectSource {
    /// Explicit set via API/settings.
    Set,
    /// Cycled to next model.
    Cycle,
    /// Restored to a previous model.
    Restore,
}

/// Source of user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    /// Interactive terminal input.
    Interactive,
    /// Remote procedure call.
    Rpc,
    /// Generated by another extension.
    Extension,
}

/// Result from an input event handler.
#[derive(Debug, Clone)]
pub enum InputEventResult {
    /// Continue processing the input normally.
    Continue,
    /// Transform the input before processing.
    Transform { text: String },
    /// Input was fully handled; skip normal processing.
    Handled,
}

/// Event data for session_before_switch.
#[derive(Debug, Clone)]
pub struct SessionBeforeSwitchEvent {
    /// Why the session is being switched.
    pub reason: SessionSwitchReason,
    /// Target session file path (if known).
    pub target_session_file: Option<String>,
}

/// Event data for session_before_fork.
#[derive(Debug, Clone)]
pub struct SessionBeforeForkEvent {
    /// The entry ID from which to fork.
    pub entry_id: String,
    /// Whether to fork before or at the entry.
    pub position: String,
}

/// Event data for session_before_compact.
#[derive(Debug, Clone)]
pub struct SessionBeforeCompactEvent {
    /// Number of messages to be compacted.
    pub messages_count: usize,
    /// Estimated tokens before compaction.
    pub tokens_before: usize,
    /// Target token count after compaction.
    pub target_tokens: usize,
    /// Optional custom instructions for the compaction.
    pub custom_instructions: Option<String>,
}

/// Event data for session_compact (after compaction).
#[derive(Debug, Clone)]
pub struct SessionCompactEvent {
    /// Number of messages after compaction.
    pub messages_count: usize,
    /// Estimated tokens after compaction.
    pub tokens_after: usize,
    /// Whether the compaction was triggered by an extension.
    pub from_extension: bool,
}

/// Event data for session_shutdown.
#[derive(Debug, Clone)]
pub struct SessionShutdownEvent {
    /// Why the session is shutting down.
    pub reason: SessionShutdownReason,
    /// Destination session file when shutting down due to session replacement.
    pub target_session_file: Option<String>,
}

/// Event data for session_before_tree.
#[derive(Debug, Clone)]
pub struct SessionBeforeTreeEvent {
    /// Target node ID in the session tree.
    pub target_id: String,
    /// Old leaf node ID.
    pub old_leaf_id: Option<String>,
}

/// Event data for session_tree (after navigation).
#[derive(Debug, Clone)]
pub struct SessionTreeEvent {
    /// New leaf node ID after navigation.
    pub new_leaf_id: Option<String>,
    /// Old leaf node ID before navigation.
    pub old_leaf_id: Option<String>,
    /// Whether the tree navigation was triggered by an extension.
    pub from_extension: bool,
}

/// Event data for context (message injection).
#[derive(Debug, Clone)]
pub struct ContextEvent {
    /// Current agent messages that can be inspected or modified.
    pub messages: Vec<Message>,
}

/// Event data for before_provider_request.
#[derive(Debug, Clone)]
pub struct BeforeProviderRequestEvent {
    /// The raw payload about to be sent to the LLM provider.
    pub payload: Value,
}

/// Event data for after_provider_response.
#[derive(Debug, Clone)]
pub struct AfterProviderResponseEvent {
    /// HTTP status code of the response.
    pub status: u16,
    /// Response headers.
    pub headers: HashMap<String, String>,
}

/// Event data for model_select.
#[derive(Debug, Clone)]
pub struct ModelSelectEvent {
    /// Name of the newly selected model.
    pub model: String,
    /// Name of the previously selected model.
    pub previous_model: Option<String>,
    /// How the model was selected.
    pub source: ModelSelectSource,
}

/// Event data for thinking_level_select.
#[derive(Debug, Clone)]
pub struct ThinkingLevelSelectEvent {
    /// New thinking level.
    pub level: String,
    /// Previous thinking level.
    pub previous_level: String,
}

/// Event data for bash execution.
#[derive(Debug, Clone)]
pub struct BashEvent {
    /// The command to execute.
    pub command: String,
    /// Whether the command should be excluded from LLM context.
    pub exclude_from_context: bool,
    /// Current working directory.
    pub cwd: PathBuf,
}

/// Event data for input transform.
#[derive(Debug, Clone)]
pub struct InputEvent {
    /// The input text.
    pub text: String,
    /// Where the input came from.
    pub source: InputSource,
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension State
// ═══════════════════════════════════════════════════════════════════════════

/// Tracks the lifecycle state of an extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState {
    /// Extension is pending load.
    Pending,
    /// Extension is loaded and active.
    Active,
    /// Extension is loaded but disabled.
    Disabled,
    /// Extension failed to load.
    Failed,
    /// Extension has been unloaded.
    Unloaded,
}

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

/// Result from emitting a tool call event to extensions.
///
/// Extensions can inspect tool calls and optionally block them.
#[derive(Debug)]
pub struct ToolCallEmitResult {
    /// Whether any extension requested to block the tool call.
    pub blocked: bool,
    /// Optional reason for blocking.
    pub block_reason: Option<String>,
    /// Errors collected from extension handlers.
    pub errors: Vec<(String, String)>,
}

impl Default for ToolCallEmitResult {
    fn default() -> Self {
        Self {
            blocked: false,
            block_reason: None,
            errors: Vec::new(),
        }
    }
}

/// Result from emitting a tool result event to extensions.
///
/// Extensions can modify tool result content before it is returned.
#[derive(Debug)]
pub struct ToolResultEmitResult {
    /// Modified output content (if any extension changed it).
    pub output: Option<String>,
    /// Modified success flag.
    pub success: Option<bool>,
    /// Errors collected from extension handlers.
    pub errors: Vec<(String, String)>,
}

impl Default for ToolResultEmitResult {
    fn default() -> Self {
        Self {
            output: None,
            success: None,
            errors: Vec::new(),
        }
    }
}

/// Result from emitting a context event to extensions.
///
/// Extensions can inspect and modify messages in the agent context.
#[derive(Debug)]
pub struct ContextEmitResult {
    /// Whether any extension modified the messages.
    pub modified: bool,
    /// The (possibly modified) messages.
    pub messages: Vec<Message>,
    /// Errors collected from extension handlers.
    pub errors: Vec<(String, String)>,
}

/// Result from emitting a before_provider_request event to extensions.
///
/// Extensions can transform the payload before it is sent.
#[derive(Debug)]
pub struct ProviderRequestEmitResult {
    /// Whether the payload was modified.
    pub modified: bool,
    /// The (possibly modified) payload.
    pub payload: Value,
    /// Errors collected from extension handlers.
    pub errors: Vec<(String, String)>,
}

/// Result from emitting a session_before event.
///
/// Extensions can cancel session operations.
#[derive(Debug)]
pub struct SessionBeforeEmitResult {
    /// Whether any extension cancelled the operation.
    pub cancelled: bool,
    /// Name of the extension that cancelled (if any).
    pub cancelled_by: Option<String>,
    /// Errors collected from extension handlers.
    pub errors: Vec<(String, String)>,
}

impl Default for SessionBeforeEmitResult {
    fn default() -> Self {
        Self {
            cancelled: false,
            cancelled_by: None,
            errors: Vec::new(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Commands
// ═══════════════════════════════════════════════════════════════════════════

/// A simple command definition for the CLI.
#[derive(Debug, Clone)]
pub struct Command {
    /// Slash-command name (e.g. "deploy")
    pub name: String,
    /// Short description shown in /help
    pub description: String,
    /// Usage string (e.g. "/deploy <target>")
    pub usage: String,
}

impl Command {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        usage: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            usage: usage.into(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Error Listener
// ═══════════════════════════════════════════════════════════════════════════

/// Callback invoked when an extension error is recorded.
pub type ExtensionErrorListener = dyn Fn(&ExtensionErrorRecord) + Send + Sync;