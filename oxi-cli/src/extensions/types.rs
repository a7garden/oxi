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

/// Permissions that an extension can request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPermission {
    /// Permission to read files.
    FileRead,
    /// Permission to write files.
    FileWrite,
    /// Permission to execute shell commands.
    Bash,
    /// Permission to make network requests.
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

/// Metadata describing an extension's identity, permissions, and configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    /// Extension name.
    pub name: String,
    /// Semantic version string.
    pub version: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Author or maintainers.
    #[serde(default)]
    pub author: String,
    /// Requested permissions.
    #[serde(default)]
    pub permissions: Vec<ExtensionPermission>,
    /// Optional JSON Schema for extension configuration.
    #[serde(default)]
    pub config_schema: Option<Value>,
}

impl ExtensionManifest {
    /// Create a new manifest with the given name and version.
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
    /// Set the description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }
    /// Set the author.
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = author.into();
        self
    }
    /// Add a permission.
    pub fn with_permission(mut self, perm: ExtensionPermission) -> Self {
        if !self.permissions.contains(&perm) {
            self.permissions.push(perm);
        }
        self
    }
    /// Set the configuration JSON Schema.
    pub fn with_config_schema(mut self, schema: Value) -> Self {
        self.config_schema = Some(schema);
        self
    }
    /// Check whether the manifest includes a specific permission.
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
    /// The requested extension was not found.
    #[error("Extension '{name}' not found")]
    NotFound {
        /// Extension name.
        name: String,
    },
    /// The extension failed to load.
    #[error("Failed to load extension '{name}': {reason}")]
    LoadFailed {
        /// Extension name.
        name: String,
        /// Reason for the failure.
        reason: String,
    },
    /// An extension hook invocation failed.
    #[error("Extension '{name}' hook '{hook}' failed: {error}")]
    HookFailed {
        /// Extension name.
        name: String,
        /// Hook that failed.
        hook: String,
        /// Error message.
        error: String,
    },
    /// The extension lacks the required permission.
    #[error("Extension '{name}' requires permission '{permission}'")]
    PermissionDenied {
        /// Extension name.
        name: String,
        /// Required permission.
        permission: ExtensionPermission,
    },
    /// The extension is currently disabled.
    #[error("Extension '{name}' is disabled")]
    Disabled {
        /// Extension name.
        name: String,
    },
    /// Hot-reload of the extension failed.
    #[error("Hot-reload of extension '{name}' failed: {reason}")]
    HotReloadFailed {
        /// Extension name.
        name: String,
        /// Reason for the failure.
        reason: String,
    },
    /// The extension configuration is invalid.
    #[error("Invalid configuration for extension '{name}': {reason}")]
    InvalidConfig {
        /// Extension name.
        name: String,
        /// Reason for the failure.
        reason: String,
    },
}

/// A recorded extension error for auditing and diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionErrorRecord {
    /// Name of the extension that produced the error.
    pub extension_name: String,
    /// Event or hook during which the error occurred.
    pub event: String,
    /// Error message.
    pub error: String,
    /// Optional stack trace.
    #[serde(default)]
    pub stack: Option<String>,
    /// Unix-millis timestamp when the error was recorded.
    pub timestamp: i64,
}

impl ExtensionErrorRecord {
    /// Create a new error record.
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
// Event Enums
// ═══════════════════════════════════════════════════════════════════════════

/// Reason why a session is being switched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSwitchReason {
    /// Starting a new session.
    New,
    /// Resuming an existing session.
    Resume,
}

/// Reason why a session is shutting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionShutdownReason {
    /// User quit the application.
    Quit,
    /// Session is being reloaded.
    Reload,
    /// Switching to a new session.
    New,
    /// Resuming a different session.
    Resume,
    /// Forking the session.
    Fork,
}

/// How a model selection was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectSource {
    /// Model was explicitly set.
    Set,
    /// Model was changed via cycling.
    Cycle,
    /// Model was restored to a previous value.
    Restore,
}

/// Source of user input in the extension system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    /// Interactive terminal input.
    Interactive,
    /// RPC call input.
    Rpc,
    /// Extension-generated input.
    Extension,
}

/// Result of an input event hook.
#[derive(Debug, Clone)]
pub enum InputEventResult {
    /// Continue processing without modification.
    Continue,
    /// Transform the input text.
    Transform {
        /// Replacement text.
        text: String,
    },
    /// Input was fully handled; stop propagation.
    Handled,
}

// ═══════════════════════════════════════════════════════════════════════════
// Event Structs
// ═══════════════════════════════════════════════════════════════════════════

/// Event fired before a session switch.
#[derive(Debug, Clone)]
pub struct SessionBeforeSwitchEvent {
    /// Why the switch is happening.
    pub reason: SessionSwitchReason,
    /// Target session file path, if applicable.
    pub target_session_file: Option<String>,
}

/// Event fired before a session fork.
#[derive(Debug, Clone)]
pub struct SessionBeforeForkEvent {
    /// Entry ID at which to fork.
    pub entry_id: String,
    /// Fork position descriptor.
    pub position: String,
}

/// Event fired before compaction runs.
#[derive(Debug, Clone)]
pub struct SessionBeforeCompactEvent {
    /// Number of messages before compaction.
    pub messages_count: usize,
    /// Token count before compaction.
    pub tokens_before: usize,
    /// Target token count.
    pub target_tokens: usize,
    /// Optional custom compaction instructions.
    pub custom_instructions: Option<String>,
}

/// Event fired after compaction completes.
#[derive(Debug, Clone)]
pub struct SessionCompactEvent {
    /// Number of messages after compaction.
    pub messages_count: usize,
    /// Token count after compaction.
    pub tokens_after: usize,
    /// Whether the compaction was requested by an extension.
    pub from_extension: bool,
}

/// Event fired when a session is shutting down.
#[derive(Debug, Clone)]
pub struct SessionShutdownEvent {
    /// Why the session is shutting down.
    pub reason: SessionShutdownReason,
    /// Target session file path, if applicable.
    pub target_session_file: Option<String>,
}

/// Event fired before navigating the session tree.
#[derive(Debug, Clone)]
pub struct SessionBeforeTreeEvent {
    /// Target entry ID to navigate to.
    pub target_id: String,
    /// Previous leaf entry ID, if any.
    pub old_leaf_id: Option<String>,
}

/// Event fired after a session tree navigation.
#[derive(Debug, Clone)]
pub struct SessionTreeEvent {
    /// New leaf entry ID after navigation.
    pub new_leaf_id: Option<String>,
    /// Previous leaf entry ID.
    pub old_leaf_id: Option<String>,
    /// Whether the navigation was triggered by an extension.
    pub from_extension: bool,
}

/// Event carrying the current context messages for modification.
#[derive(Debug, Clone)]
pub struct ContextEvent {
    /// Messages in the current context.
    pub messages: Vec<Message>,
}

/// Event fired before a provider request is sent.
#[derive(Debug, Clone)]
pub struct BeforeProviderRequestEvent {
    /// JSON payload that will be sent to the provider.
    pub payload: Value,
}

/// Event fired after a provider response is received.
#[derive(Debug, Clone)]
pub struct AfterProviderResponseEvent {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: HashMap<String, String>,
}

/// Event fired when a model is selected or changed.
#[derive(Debug, Clone)]
pub struct ModelSelectEvent {
    /// Newly selected model identifier.
    pub model: String,
    /// Previous model identifier.
    pub previous_model: Option<String>,
    /// How the selection was triggered.
    pub source: ModelSelectSource,
}

/// Event fired when a thinking level is selected or changed.
#[derive(Debug, Clone)]
pub struct ThinkingLevelSelectEvent {
    /// New thinking level name.
    pub level: String,
    /// Previous thinking level name.
    pub previous_level: String,
}

/// Event fired when a bash command is executed.
#[derive(Debug, Clone)]
pub struct BashEvent {
    /// Command being executed.
    pub command: String,
    /// Whether this command should be excluded from the LLM context.
    pub exclude_from_context: bool,
    /// Working directory for the command.
    pub cwd: PathBuf,
}

/// Event fired when user input is received.
#[derive(Debug, Clone)]
pub struct InputEvent {
    /// The input text.
    pub text: String,
    /// Where the input originated.
    pub source: InputSource,
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension State
// ═══════════════════════════════════════════════════════════════════════════

/// Lifecycle state of an extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState {
    /// Extension is loaded but not yet activated.
    Pending,
    /// Extension is active and running.
    Active,
    /// Extension has been disabled.
    Disabled,
    /// Extension failed to load or run.
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

/// Result of emitting a tool-call event to extensions.
#[derive(Debug, Default)]
pub struct ToolCallEmitResult {
    /// Whether the tool call was blocked by an extension.
    pub blocked: bool,
    /// Reason the call was blocked, if applicable.
    pub block_reason: Option<String>,
    /// Per-extension errors encountered.
    pub errors: Vec<(String, String)>,
}

/// Result of emitting a tool-result event to extensions.
#[derive(Debug, Default)]
pub struct ToolResultEmitResult {
    /// Optional replacement output from an extension.
    pub output: Option<String>,
    /// Optional override for the success flag.
    pub success: Option<bool>,
    /// Per-extension errors encountered.
    pub errors: Vec<(String, String)>,
}

/// Result of emitting a context event to extensions.
#[derive(Debug)]
pub struct ContextEmitResult {
    /// Whether the messages were modified by any extension.
    pub modified: bool,
    /// The (possibly modified) messages.
    pub messages: Vec<Message>,
    /// Per-extension errors encountered.
    pub errors: Vec<(String, String)>,
}

/// Result of emitting a before-provider-request event to extensions.
#[derive(Debug)]
pub struct ProviderRequestEmitResult {
    /// Whether the payload was modified by any extension.
    pub modified: bool,
    /// The (possibly modified) payload.
    pub payload: Value,
    /// Per-extension errors encountered.
    pub errors: Vec<(String, String)>,
}

/// Result of emitting a session-before event to extensions.
#[derive(Debug, Default)]
pub struct SessionBeforeEmitResult {
    /// Whether the operation was cancelled by an extension.
    pub cancelled: bool,
    /// Name of the extension that cancelled the operation.
    pub cancelled_by: Option<String>,
    /// Per-extension errors encountered.
    pub errors: Vec<(String, String)>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Commands
// ═══════════════════════════════════════════════════════════════════════════

/// A slash-command registered by an extension.
#[derive(Debug, Clone)]
pub struct Command {
    /// Command name (e.g. "my-cmd").
    pub name: String,
    /// Short description shown in help.
    pub description: String,
    /// Usage string (e.g. "/my-cmd <arg>").
    pub usage: String,
}
impl Command {
    /// Create a new command descriptor.
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

/// Type alias for a closure that listens to extension errors.
pub type ExtensionErrorListener = dyn Fn(&ExtensionErrorRecord) + Send + Sync;
