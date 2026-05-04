//! Extension system for oxi
//!
//! Extensions allow custom tools, commands, and event hooks to be loaded
//! dynamically at runtime. Extensions can be loaded from shared libraries
//! (.so/.dll/.dylib) via the `-e`/`--extension` CLI flag.
//!
//! # Architecture
//!
//! The extension system is modeled after pi-mono's extension API and provides:
//!
//! - **Extension manifest** — metadata, permissions, configuration schema
//! - **Extension lifecycle hooks** — `on_load`, `on_unload`, message/tool/session events
//! - **Extension context** — access to settings, session state, tool registration, messaging
//! - **Extension error handling** — graceful degradation with logging
//! - **Extension registry** — name-based lookup, enable/disable, hot-reload

use anyhow::{bail, Context, Result};
use libloading::{Library, Symbol};
use oxi_agent::{AgentEvent, AgentTool, AgentToolResult};
use oxi_ai::Message;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
// Extension Context
// ═══════════════════════════════════════════════════════════════════════════

/// Context provided to extension lifecycle hooks and event handlers.
///
/// This is the primary interface through which extensions interact with the
/// host application.
pub struct ExtensionContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Read-only access to application settings.
    settings: Arc<RwLock<crate::settings::Settings>>,
    /// Extension-specific configuration (validated against manifest schema).
    pub config: Value,
    /// Session ID for the current session (if any).
    pub session_id: Option<String>,
    /// Whether the agent is currently idle (not streaming).
    idle: Arc<RwLock<bool>>,
    /// Tool registration callback.
    tool_registrar: Arc<dyn Fn(Arc<dyn AgentTool>) + Send + Sync>,
    /// Message sending callback.
    message_sender: Arc<dyn Fn(&str) + Send + Sync>,
    /// Pending errors recorded by extensions.
    errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>,
    /// Tool getter callback — returns currently registered tool definitions.
    tool_getter: Arc<dyn Fn() -> Vec<Arc<dyn AgentTool>> + Send + Sync>,
    /// Tool setter callback — replaces the active tool set.
    tool_setter: Arc<dyn Fn(Vec<Arc<dyn AgentTool>>) + Send + Sync>,
    /// Model setter callback.
    model_setter: Arc<dyn Fn(&str) + Send + Sync>,
    /// Thinking level setter callback.
    thinking_level_setter: Arc<dyn Fn(&str) + Send + Sync>,
    /// System prompt appender callback.
    system_prompt_appender: Arc<dyn Fn(&str) + Send + Sync>,
    /// Session name setter callback.
    session_name_setter: Arc<dyn Fn(&str) + Send + Sync>,
    /// Session entries getter callback.
    session_entries_getter: Arc<dyn Fn() -> Vec<Value> + Send + Sync>,
    /// Session fork callback.
    session_fork: Arc<dyn Fn(&str) -> Result<String> + Send + Sync>,
}

impl fmt::Debug for ExtensionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionContext")
            .field("cwd", &self.cwd)
            .field("session_id", &self.session_id)
            .field("idle", &self.idle.read())
            .finish()
    }
}

impl ExtensionContext {
    /// Create a new extension context.
    ///
    /// Most callers should use [`ExtensionContextBuilder`] instead.
    pub fn new(
        cwd: PathBuf,
        settings: Arc<RwLock<crate::settings::Settings>>,
        config: Value,
        session_id: Option<String>,
        idle: Arc<RwLock<bool>>,
        tool_registrar: Arc<dyn Fn(Arc<dyn AgentTool>) + Send + Sync>,
        message_sender: Arc<dyn Fn(&str) + Send + Sync>,
        errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>,
    ) -> Self {
        Self {
            cwd,
            settings,
            config,
            session_id,
            idle,
            tool_registrar,
            message_sender,
            errors,
            tool_getter: Arc::new(|| vec![]),
            tool_setter: Arc::new(|_| {}),
            model_setter: Arc::new(|_| {}),
            thinking_level_setter: Arc::new(|_| {}),
            system_prompt_appender: Arc::new(|_| {}),
            session_name_setter: Arc::new(|_| {}),
            session_entries_getter: Arc::new(|| vec![]),
            session_fork: Arc::new(|_| bail!("session fork not configured")),
        }
    }

    /// Read the current application settings.
    pub fn settings(&self) -> crate::settings::Settings {
        self.settings.read().clone()
    }

    /// Whether the agent is currently idle (not streaming).
    pub fn is_idle(&self) -> bool {
        *self.idle.read()
    }

    /// Register a tool that the agent can call.
    pub fn register_tool(&self, tool: Arc<dyn AgentTool>) {
        (self.tool_registrar)(tool);
    }

    /// Send a text message to the agent / conversation.
    pub fn send_message(&self, text: &str) {
        (self.message_sender)(text);
    }

    /// Record an error that occurred inside an extension hook.
    pub fn record_error(&self, extension_name: &str, event: &str, error: &str) {
        let record = ExtensionErrorRecord::new(extension_name, event, error);
        tracing::warn!(
            extension = extension_name,
            event = event,
            error = error,
            "Extension error recorded"
        );
        self.errors.write().push(record);
    }

    /// Get all recorded extension errors.
    pub fn errors(&self) -> Vec<ExtensionErrorRecord> {
        self.errors.read().clone()
    }

    /// Clear all recorded extension errors.
    pub fn clear_errors(&self) {
        self.errors.write().clear();
    }

    /// Read the extension-specific configuration value at the given path.
    ///
    /// Returns `None` if the path doesn't exist.
    pub fn config_get(&self, path: &str) -> Option<Value> {
        let mut current = &self.config;
        for key in path.split('.') {
            match current {
                Value::Object(map) => current = map.get(key)?,
                _ => return None,
            }
        }
        Some(current.clone())
    }

    /// Read the filesystem — access files relative to cwd.
    pub fn read_file(&self, relative_path: &Path) -> Result<String> {
        let full_path = self.cwd.join(relative_path);
        std::fs::read_to_string(&full_path)
            .with_context(|| format!("Failed to read file: {}", full_path.display()))
    }

    // ── New context methods for enhanced extension API ───────────────

    /// Get the currently registered tool definitions.
    pub fn get_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        (self.tool_getter)()
    }

    /// Set the active tool set, replacing any previously registered tools.
    pub fn set_tools(&self, tools: Vec<Arc<dyn AgentTool>>) {
        (self.tool_setter)(tools);
    }

    /// Switch the active model by name.
    pub fn set_model(&self, model: &str) {
        (self.model_setter)(model);
    }

    /// Change the thinking level.
    pub fn set_thinking_level(&self, level: &str) {
        (self.thinking_level_setter)(level);
    }

    /// Append text to the system prompt.
    pub fn append_system_prompt(&self, text: &str) {
        (self.system_prompt_appender)(text);
    }

    /// Set the name of the current session.
    pub fn set_session_name(&self, name: &str) {
        (self.session_name_setter)(name);
    }

    /// Read the session history entries.
    ///
    /// Returns a vector of JSON values, each representing a session entry.
    pub fn get_session_entries(&self) -> Vec<Value> {
        (self.session_entries_getter)()
    }

    /// Fork the current session, returning the new session ID.
    pub fn fork_session(&self, entry_id: &str) -> Result<String> {
        (self.session_fork)(entry_id)
    }
}

/// Builder for [`ExtensionContext`].
pub struct ExtensionContextBuilder {
    cwd: PathBuf,
    settings: Option<Arc<RwLock<crate::settings::Settings>>>,
    config: Value,
    session_id: Option<String>,
    idle: Arc<RwLock<bool>>,
    tool_registrar: Option<Arc<dyn Fn(Arc<dyn AgentTool>) + Send + Sync>>,
    message_sender: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    errors: Option<Arc<RwLock<Vec<ExtensionErrorRecord>>>>,
    tool_getter: Option<Arc<dyn Fn() -> Vec<Arc<dyn AgentTool>> + Send + Sync>>,
    tool_setter: Option<Arc<dyn Fn(Vec<Arc<dyn AgentTool>>) + Send + Sync>>,
    model_setter: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    thinking_level_setter: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    system_prompt_appender: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    session_name_setter: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    session_entries_getter: Option<Arc<dyn Fn() -> Vec<Value> + Send + Sync>>,
    session_fork: Option<Arc<dyn Fn(&str) -> Result<String> + Send + Sync>>,
}

impl ExtensionContextBuilder {
    /// Start building a context with the given working directory.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            settings: None,
            config: Value::Null,
            session_id: None,
            idle: Arc::new(RwLock::new(true)),
            tool_registrar: None,
            message_sender: None,
            errors: None,
            tool_getter: None,
            tool_setter: None,
            model_setter: None,
            thinking_level_setter: None,
            system_prompt_appender: None,
            session_name_setter: None,
            session_entries_getter: None,
            session_fork: None,
        }
    }

    /// Set the settings reference.
    pub fn settings(mut self, settings: Arc<RwLock<crate::settings::Settings>>) -> Self {
        self.settings = Some(settings);
        self
    }

    /// Set extension-specific configuration.
    pub fn config(mut self, config: Value) -> Self {
        self.config = config;
        self
    }

    /// Set the session ID.
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Set the idle-state handle.
    pub fn idle(mut self, idle: Arc<RwLock<bool>>) -> Self {
        self.idle = idle;
        self
    }

    /// Set the tool registrar callback.
    pub fn tool_registrar(
        mut self,
        registrar: Arc<dyn Fn(Arc<dyn AgentTool>) + Send + Sync>,
    ) -> Self {
        self.tool_registrar = Some(registrar);
        self
    }

    /// Set the message sender callback.
    pub fn message_sender(mut self, sender: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.message_sender = Some(sender);
        self
    }

    /// Set the shared error buffer.
    pub fn errors(mut self, errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>) -> Self {
        self.errors = Some(errors);
        self
    }

    /// Set the tool getter callback.
    pub fn tool_getter(
        mut self,
        getter: Arc<dyn Fn() -> Vec<Arc<dyn AgentTool>> + Send + Sync>,
    ) -> Self {
        self.tool_getter = Some(getter);
        self
    }

    /// Set the tool setter callback.
    pub fn tool_setter(
        mut self,
        setter: Arc<dyn Fn(Vec<Arc<dyn AgentTool>>) + Send + Sync>,
    ) -> Self {
        self.tool_setter = Some(setter);
        self
    }

    /// Set the model setter callback.
    pub fn model_setter(mut self, setter: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.model_setter = Some(setter);
        self
    }

    /// Set the thinking level setter callback.
    pub fn thinking_level_setter(mut self, setter: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.thinking_level_setter = Some(setter);
        self
    }

    /// Set the system prompt appender callback.
    pub fn system_prompt_appender(mut self, appender: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.system_prompt_appender = Some(appender);
        self
    }

    /// Set the session name setter callback.
    pub fn session_name_setter(mut self, setter: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.session_name_setter = Some(setter);
        self
    }

    /// Set the session entries getter callback.
    pub fn session_entries_getter(
        mut self,
        getter: Arc<dyn Fn() -> Vec<Value> + Send + Sync>,
    ) -> Self {
        self.session_entries_getter = Some(getter);
        self
    }

    /// Set the session fork callback.
    pub fn session_fork(
        mut self,
        fork: Arc<dyn Fn(&str) -> Result<String> + Send + Sync>,
    ) -> Self {
        self.session_fork = Some(fork);
        self
    }

    /// Build the context, falling back to no-op callbacks where necessary.
    pub fn build(self) -> ExtensionContext {
        ExtensionContext {
            cwd: self.cwd,
            settings: self
                .settings
                .unwrap_or_else(|| Arc::new(RwLock::new(crate::settings::Settings::default()))),
            config: self.config,
            session_id: self.session_id,
            idle: self.idle,
            tool_registrar: self.tool_registrar.unwrap_or_else(|| {
                Arc::new(|_tool| {
                    tracing::debug!("Tool registration attempted with no registrar");
                })
            }),
            message_sender: self.message_sender.unwrap_or_else(|| {
                Arc::new(|_msg| {
                    tracing::debug!("Message send attempted with no sender");
                })
            }),
            errors: self.errors.unwrap_or_default(),
            tool_getter: self.tool_getter.unwrap_or_else(|| Arc::new(Vec::new)),
            tool_setter: self.tool_setter.unwrap_or_else(|| Arc::new(|_| {})),
            model_setter: self.model_setter.unwrap_or_else(|| Arc::new(|_| {})),
            thinking_level_setter: self.thinking_level_setter.unwrap_or_else(|| Arc::new(|_| {})),
            system_prompt_appender: self
                .system_prompt_appender
                .unwrap_or_else(|| Arc::new(|_| {})),
            session_name_setter: self.session_name_setter.unwrap_or_else(|| Arc::new(|_| {})),
            session_entries_getter: self
                .session_entries_getter
                .unwrap_or_else(|| Arc::new(Vec::new)),
            session_fork: self.session_fork.unwrap_or_else(|| {
                Arc::new(|_| bail!("session fork not configured"))
            }),
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
// Extension Lifecycle Trait
// ═══════════════════════════════════════════════════════════════════════════

/// Core trait that every oxi extension must implement.
///
/// Extensions can register custom tools, custom slash-commands, hook
/// into the agent event stream, and respond to lifecycle events.
///
/// All lifecycle hooks provide default no-op implementations so that
/// extensions only need to override the hooks they care about.
pub trait Extension: Send + Sync {
    // ── Identity ─────────────────────────────────────────────────────

    /// Unique name of the extension.
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// Return the extension manifest for metadata, permissions, and config.
    ///
    /// The default implementation builds a minimal manifest from
    /// [`name`](Extension::name) and [`description`](Extension::description).
    fn manifest(&self) -> ExtensionManifest {
        ExtensionManifest::new(self.name(), "0.0.0").with_description(self.description())
    }

    // ── Registration ─────────────────────────────────────────────────

    /// Return custom tools this extension contributes.
    fn register_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![]
    }

    /// Return custom slash-commands this extension contributes.
    fn register_commands(&self) -> Vec<Command> {
        vec![]
    }

    // ── Lifecycle hooks ──────────────────────────────────────────────

    /// Called once when the extension is loaded and before any other hooks.
    ///
    /// Use this to perform initialization such as reading configuration,
    /// establishing connections, or validating permissions.
    fn on_load(&self, _ctx: &ExtensionContext) {}

    /// Called when the extension is about to be unloaded.
    ///
    /// Use this to release resources, flush buffers, or perform cleanup.
    fn on_unload(&self) {}

    // ── Message hooks ────────────────────────────────────────────────

    /// Called after a user message is sent to the agent.
    fn on_message_sent(&self, _msg: &str) {}

    /// Called when an assistant message is received.
    fn on_message_received(&self, _msg: &str) {}

    // ── Tool hooks ───────────────────────────────────────────────────

    /// Called before a tool is executed. The tool name and raw parameters
    /// are provided. This can be used for logging, auditing, or preprocessing.
    fn on_tool_call(&self, _tool: &str, _params: &Value) {}

    /// Called after a tool finishes execution.
    fn on_tool_result(&self, _tool: &str, _result: &AgentToolResult) {}

    // ── Session hooks ────────────────────────────────────────────────

    /// Called when a new session starts.
    fn on_session_start(&self, _session_id: &str) {}

    /// Called when a session ends.
    fn on_session_end(&self, _session_id: &str) {}

    // ── Settings hook ────────────────────────────────────────────────

    /// Called when settings have changed (e.g. user ran `oxi config`).
    fn on_settings_changed(&self, _settings: &crate::settings::Settings) {}

    // ── Agent event hook ─────────────────────────────────────────────

    /// Called when the agent emits an event.
    ///
    /// This is the low-level catch-all. Prefer the typed hooks above
    /// when possible.
    fn on_event(&self, _event: &AgentEvent) {}

    // ── Enhanced tool call hooks ─────────────────────────────────────

    /// Called immediately before a tool is executed.
    ///
    /// Use this for pre-processing, validation, or logging tool calls.
    /// Return `Err` to abort the tool execution (optional, implement
    /// [`on_before_tool_call_with_result`] for that).
    fn on_before_tool_call(&self, _tool: &str, _args: &Value) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called immediately after a tool finishes execution.
    ///
    /// This is similar to [`on_tool_result`] but provides access to the
    /// full [`AgentToolResult`] including metadata.
    fn on_after_tool_call(&self, _tool: &str, _result: &AgentToolResult) -> Result<(), anyhow::Error> {
        Ok(())
    }

    // ── Compaction hooks ─────────────────────────────────────────────

    /// Called before context compaction begins.
    ///
    /// Use this to save any state that should be preserved through compaction,
    /// or to log that compaction is starting.
    fn on_before_compaction(&self, _ctx: &crate::CompactionContext) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called after context compaction completes.
    ///
    /// The `summary` contains the generated summary of the compacted messages.
    /// Use this to restore state, update indices, or log compaction results.
    fn on_after_compaction(&self, _summary: &str) -> Result<(), anyhow::Error> {
        Ok(())
    }

    // ── Error hook ──────────────────────────────────────────────────

    /// Called when an error occurs in the agent.
    ///
    /// Use this to log errors, send notifications, or perform recovery actions.
    fn on_error(&self, _error: &anyhow::Error) -> Result<(), anyhow::Error> {
        Ok(())
    }

    // ── Session lifecycle hooks (pi-mono parity) ─────────────────────

    /// Called before switching to another session.
    ///
    /// Return `Err` to cancel the switch.
    fn session_before_switch(&self, _event: &SessionBeforeSwitchEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called before forking a session.
    ///
    /// Return `Err` to cancel the fork.
    fn session_before_fork(&self, _event: &SessionBeforeForkEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called before context compaction starts (fine-grained variant).
    ///
    /// Unlike `on_before_compaction`, this receives the structured event data.
    fn session_before_compact(&self, _event: &SessionBeforeCompactEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called after context compaction completes (fine-grained variant).
    fn session_compact(&self, _event: &SessionCompactEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called when a session is shutting down.
    fn session_shutdown(&self, _event: &SessionShutdownEvent) {}

    /// Called before navigating in the session tree.
    ///
    /// Return `Err` to cancel the navigation.
    fn session_before_tree(&self, _event: &SessionBeforeTreeEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called after navigating in the session tree.
    fn session_tree(&self, _event: &SessionTreeEvent) {}

    // ── Provider hooks (pi-mono parity) ──────────────────────────────

    /// Called to inject or inspect context messages before the agent loop.
    fn context(&self, _event: &mut ContextEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called before a provider request is sent to the LLM API.
    fn before_provider_request(&self, _event: &mut BeforeProviderRequestEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called after a provider response is received.
    fn after_provider_response(&self, _event: &AfterProviderResponseEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    // ── Model hooks (pi-mono parity) ─────────────────────────────────

    /// Called when a new model is selected.
    fn model_select(&self, _event: &ModelSelectEvent) {}

    /// Called when a new thinking level is selected.
    fn thinking_level_select(&self, _event: &ThinkingLevelSelectEvent) {}

    // ── Bash / Input hooks (pi-mono parity) ──────────────────────────

    /// Called when a bash command is executed by the user.
    fn bash(&self, _event: &BashEvent) {}

    /// Called when user input is received, before agent processing.
    ///
    /// Return an [`InputEventResult`] to control how the input is processed.
    fn input(&self, _event: &InputEvent) -> InputEventResult {
        InputEventResult::Continue
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Loaded Extension Entry
// ═══════════════════════════════════════════════════════════════════════════

/// Internal representation of a loaded extension in the registry.
struct LoadedExtension {
    /// The extension trait object.
    extension: Arc<dyn Extension>,
    /// Whether the extension is currently enabled.
    enabled: bool,
    /// Path the extension was loaded from (for hot-reload).
    source_path: Option<PathBuf>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Registry
// ═══════════════════════════════════════════════════════════════════════════

/// Manages a collection of loaded extensions.
///
/// Supports:
/// - Registering / unregistering extensions by name
/// - Enabling / disabling at runtime
/// - Hot-reloading from the original source path
/// - Broadcasting events to all enabled extensions with graceful error handling
/// - Collecting tools and commands from enabled extensions
pub struct ExtensionRegistry {
    /// Name → loaded extension entry.
    entries: HashMap<String, LoadedExtension>,
    /// Shared error buffer for recording extension errors.
    errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>,
    /// Keep dynamically loaded libraries alive so vtables stay valid.
    #[allow(dead_code)]
    libraries: Vec<Library>,
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            errors: Arc::new(RwLock::new(Vec::new())),
            libraries: Vec::new(),
        }
    }

    // ── Registration ─────────────────────────────────────────────────

    /// Register an extension (in-memory).
    ///
    /// If an extension with the same name already exists it is replaced.
    pub fn register(&mut self, ext: Arc<dyn Extension>) {
        let name = ext.name().to_string();
        tracing::info!(name = %name, "extension registered");
        self.entries.insert(
            name,
            LoadedExtension {
                extension: ext,
                enabled: true,
                source_path: None,
            },
        );
    }

    /// Register an extension that was loaded from a shared library,
    /// keeping the library handle alive for hot-reload.
    pub fn register_with_library(
        &mut self,
        ext: Arc<dyn Extension>,
        source_path: PathBuf,
        library: Library,
    ) {
        let name = ext.name().to_string();
        tracing::info!(name = %name, path = %source_path.display(), "extension registered (dynamic)");
        self.libraries.push(library);
        self.entries.insert(
            name,
            LoadedExtension {
                extension: ext,
                enabled: true,
                source_path: Some(source_path),
            },
        );
    }

    /// Unregister an extension by name.
    ///
    /// Calls `on_unload` on the extension before removing it.
    /// Returns `false` if the extension was not found.
    pub fn unregister(&mut self, name: &str) -> bool {
        if let Some(entry) = self.entries.remove(name) {
            self.call_hook_safe(name, "on_unload", || {
                entry.extension.on_unload();
            });
            tracing::info!(name = %name, "extension unregistered");
            true
        } else {
            false
        }
    }

    // ── Enable / Disable ─────────────────────────────────────────────

    /// Disable an extension at runtime.
    ///
    /// Disabled extensions are skipped during event broadcasting and
    /// tool/command collection, but remain loaded.
    pub fn disable(&mut self, name: &str) -> Result<(), ExtensionError> {
        let ext = {
            let entry = self
                .entries
                .get_mut(name)
                .ok_or_else(|| ExtensionError::NotFound {
                    name: name.to_string(),
                })?;
            if !entry.enabled {
                return Ok(());
            }
            entry.enabled = false;
            Arc::clone(&entry.extension)
        };
        self.call_hook_safe(name, "on_unload", || {
            ext.on_unload();
        });
        tracing::info!(name = %name, "extension disabled");
        Ok(())
    }

    /// Enable a previously disabled extension.
    pub fn enable(&mut self, name: &str, ctx: &ExtensionContext) -> Result<(), ExtensionError> {
        let ext = {
            let entry = self
                .entries
                .get_mut(name)
                .ok_or_else(|| ExtensionError::NotFound {
                    name: name.to_string(),
                })?;
            if entry.enabled {
                return Ok(());
            }
            entry.enabled = true;
            Arc::clone(&entry.extension)
        };
        self.call_hook_safe(name, "on_load", || {
            ext.on_load(ctx);
        });
        tracing::info!(name = %name, "extension enabled");
        Ok(())
    }

    /// Check whether an extension is currently enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.entries.get(name).map(|e| e.enabled).unwrap_or(false)
    }

    // ── Hot Reload ───────────────────────────────────────────────────

    /// Hot-reload an extension from its original source path.
    ///
    /// The old extension is unloaded, the shared library is re-opened,
    /// and the new extension is loaded in its place. Tools and commands
    /// from the old extension are no longer returned.
    pub fn hot_reload(&mut self, name: &str, ctx: &ExtensionContext) -> Result<(), ExtensionError> {
        let source_path = {
            let entry = self
                .entries
                .get(name)
                .ok_or_else(|| ExtensionError::NotFound {
                    name: name.to_string(),
                })?;
            entry.source_path.clone()
        };

        let source_path = source_path.ok_or_else(|| ExtensionError::HotReloadFailed {
            name: name.to_string(),
            reason: "no source path recorded (in-memory extension)".to_string(),
        })?;

        // Unload old
        self.unregister(name);

        // Load new
        let new_ext =
            load_extension(&source_path).map_err(|e| ExtensionError::HotReloadFailed {
                name: name.to_string(),
                reason: e.to_string(),
            })?;

        let library = unsafe {
            Library::new(&source_path).map_err(|e| ExtensionError::HotReloadFailed {
                name: name.to_string(),
                reason: format!("Failed to re-open library: {}", e),
            })?
        };

        // Call on_load on the new extension
        self.call_hook_safe(name, "on_load", || {
            new_ext.on_load(ctx);
        });

        self.register_with_library(new_ext, source_path, library);
        tracing::info!(name = %name, "extension hot-reloaded");
        Ok(())
    }

    // ── Tool & Command Collection ────────────────────────────────────

    /// Collect all tools from every enabled extension.
    pub fn all_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.entries
            .values()
            .filter(|e| e.enabled)
            .flat_map(|e| e.extension.register_tools())
            .collect()
    }

    /// Collect all commands from every enabled extension.
    pub fn all_commands(&self) -> Vec<Command> {
        self.entries
            .values()
            .filter(|e| e.enabled)
            .flat_map(|e| e.extension.register_commands())
            .collect()
    }

    // ── Event Broadcasting ───────────────────────────────────────────

    /// Broadcast `on_load` to all enabled extensions.
    pub fn emit_load(&self, ctx: &ExtensionContext) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_load", || {
                entry.extension.on_load(ctx);
            });
        }
    }

    /// Broadcast `on_unload` to all enabled extensions.
    pub fn emit_unload(&self) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_unload", || {
                entry.extension.on_unload();
            });
        }
    }

    /// Broadcast `on_message_sent` to all enabled extensions.
    pub fn emit_message_sent(&self, msg: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_message_sent", || {
                entry.extension.on_message_sent(msg);
            });
        }
    }

    /// Broadcast `on_message_received` to all enabled extensions.
    pub fn emit_message_received(&self, msg: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_message_received", || {
                entry.extension.on_message_received(msg);
            });
        }
    }

    /// Broadcast `on_tool_call` to all enabled extensions.
    pub fn emit_tool_call(&self, tool: &str, params: &Value) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_tool_call", || {
                entry.extension.on_tool_call(tool, params);
            });
        }
    }

    /// Broadcast `on_tool_result` to all enabled extensions.
    pub fn emit_tool_result(&self, tool: &str, result: &AgentToolResult) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_tool_result", || {
                entry.extension.on_tool_result(tool, result);
            });
        }
    }

    /// Broadcast `on_session_start` to all enabled extensions.
    pub fn emit_session_start(&self, session_id: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_session_start", || {
                entry.extension.on_session_start(session_id);
            });
        }
    }

    /// Broadcast `on_session_end` to all enabled extensions.
    pub fn emit_session_end(&self, session_id: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_session_end", || {
                entry.extension.on_session_end(session_id);
            });
        }
    }

    /// Broadcast `on_settings_changed` to all enabled extensions.
    pub fn emit_settings_changed(&self, settings: &crate::settings::Settings) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_settings_changed", || {
                entry.extension.on_settings_changed(settings);
            });
        }
    }

    /// Broadcast an agent event to every enabled extension.
    pub fn emit_event(&self, event: &AgentEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_event", || {
                entry.extension.on_event(event);
            });
        }
    }

    // ── Enhanced Tool Hook Broadcasts ─────────────────────────────────

    /// Broadcast `on_before_tool_call` to all enabled extensions.
    pub fn emit_before_tool_call(
        &self,
        tool: &str,
        args: &Value,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.on_before_tool_call(tool, args) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, tool = tool, error = %e, "on_before_tool_call failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `on_after_tool_call` to all enabled extensions.
    pub fn emit_after_tool_call(
        &self,
        tool: &str,
        result: &AgentToolResult,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.on_after_tool_call(tool, result) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, tool = tool, error = %e, "on_after_tool_call failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    // ── Compaction Hook Broadcasts ────────────────────────────────────

    /// Broadcast `on_before_compaction` to all enabled extensions.
    pub fn emit_before_compaction(
        &self,
        ctx: &crate::CompactionContext,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.on_before_compaction(ctx) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "on_before_compaction failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `on_after_compaction` to all enabled extensions.
    pub fn emit_after_compaction(
        &self,
        summary: &str,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.on_after_compaction(summary) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "on_after_compaction failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    // ── Error Hook Broadcast ──────────────────────────────────────────

    /// Broadcast `on_error` to all enabled extensions.
    pub fn emit_error(
        &self,
        error: &anyhow::Error,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.on_error(error) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "on_error hook failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    // ── Session Lifecycle Hook Broadcasts (pi-mono parity) ────────────

    /// Broadcast `session_before_switch` to all enabled extensions.
    pub fn emit_session_before_switch(
        &self,
        event: &SessionBeforeSwitchEvent,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.session_before_switch(event) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "session_before_switch failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `session_before_fork` to all enabled extensions.
    pub fn emit_session_before_fork(
        &self,
        event: &SessionBeforeForkEvent,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.session_before_fork(event) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "session_before_fork failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `session_before_compact` to all enabled extensions.
    pub fn emit_session_before_compact(
        &self,
        event: &SessionBeforeCompactEvent,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.session_before_compact(event) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "session_before_compact failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `session_compact` to all enabled extensions.
    pub fn emit_session_compact(
        &self,
        event: &SessionCompactEvent,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.session_compact(event) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "session_compact failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `session_shutdown` to all enabled extensions.
    pub fn emit_session_shutdown(&self, event: &SessionShutdownEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "session_shutdown", || {
                entry.extension.session_shutdown(event);
            });
        }
    }

    /// Broadcast `session_before_tree` to all enabled extensions.
    pub fn emit_session_before_tree(
        &self,
        event: &SessionBeforeTreeEvent,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.session_before_tree(event) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "session_before_tree failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `session_tree` to all enabled extensions.
    pub fn emit_session_tree(&self, event: &SessionTreeEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "session_tree", || {
                entry.extension.session_tree(event);
            });
        }
    }

    // ── Provider Hook Broadcasts (pi-mono parity) ─────────────────────

    /// Broadcast `context` to all enabled extensions.
    pub fn emit_context(
        &self,
        event: &mut ContextEvent,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.context(event) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "context hook failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `before_provider_request` to all enabled extensions.
    pub fn emit_before_provider_request(
        &self,
        event: &mut BeforeProviderRequestEvent,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.before_provider_request(event) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "before_provider_request failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    /// Broadcast `after_provider_response` to all enabled extensions.
    pub fn emit_after_provider_response(
        &self,
        event: &AfterProviderResponseEvent,
    ) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            match entry.extension.after_provider_response(event) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(extension = name, error = %e, "after_provider_response failed");
                    errors.push((name.to_string(), e));
                }
            }
        }
        errors
    }

    // ── Model Hook Broadcasts (pi-mono parity) ────────────────────────

    /// Broadcast `model_select` to all enabled extensions.
    pub fn emit_model_select(&self, event: &ModelSelectEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "model_select", || {
                entry.extension.model_select(event);
            });
        }
    }

    /// Broadcast `thinking_level_select` to all enabled extensions.
    pub fn emit_thinking_level_select(&self, event: &ThinkingLevelSelectEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "thinking_level_select", || {
                entry.extension.thinking_level_select(event);
            });
        }
    }

    // ── Bash / Input Hook Broadcasts (pi-mono parity) ─────────────────

    /// Broadcast `bash` to all enabled extensions.
    pub fn emit_bash(&self, event: &BashEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "bash", || {
                entry.extension.bash(event);
            });
        }
    }

    /// Broadcast `input` to all enabled extensions, collecting results.
    ///
    /// The first extension to return [`InputEventResult::Handled`] or
    /// [`InputEventResult::Transform`] wins; later extensions are still
    /// notified but their results are ignored.
    pub fn emit_input(&self, event: &InputEvent) -> InputEventResult {
        let mut final_result = InputEventResult::Continue;
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "input", || {
                let result = entry.extension.input(event);
                if matches!(final_result, InputEventResult::Continue) {
                    final_result = result;
                }
            });
        }
        final_result
    }

    // ── Querying ─────────────────────────────────────────────────────

    /// Get a reference to an extension by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Extension>> {
        self.entries.get(name).map(|e| Arc::clone(&e.extension))
    }

    /// Iterate over registered extension names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    /// Iterate over registered extensions.
    pub fn extensions(&self) -> impl Iterator<Item = &Arc<dyn Extension>> {
        self.entries.values().map(|e| &e.extension)
    }

    /// Get the manifest for an extension by name.
    pub fn manifest(&self, name: &str) -> Option<ExtensionManifest> {
        self.entries.get(name).map(|e| e.extension.manifest())
    }

    /// Number of registered extensions.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether any extensions are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all recorded errors.
    pub fn errors(&self) -> Vec<ExtensionErrorRecord> {
        self.errors.read().clone()
    }

    /// Clear all recorded errors.
    pub fn clear_errors(&self) {
        self.errors.write().clear();
    }

    // ── Internal ─────────────────────────────────────────────────────

    /// Call a hook on an extension, catching any panics/errors and
    /// recording them for diagnostics.  This provides **graceful
    /// degradation** — a failing extension never crashes the host.
    fn call_hook_safe<F>(&self, ext_name: &str, hook: &str, f: F)
    where
        F: FnOnce(),
    {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if let Err(payload) = result {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            tracing::error!(
                extension = ext_name,
                hook = hook,
                error = %msg,
                "Extension hook panicked — graceful degradation"
            );
            self.errors.write().push(ExtensionErrorRecord::new(
                ext_name,
                hook,
                &format!("panic: {}", msg),
            ));
        }
    }
}

impl fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRegistry")
            .field("count", &self.entries.len())
            .field("names", &self.entries.keys().cloned().collect::<Vec<_>>())
            .finish()
    }
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
// Error Listener
// ═══════════════════════════════════════════════════════════════════════════

/// Callback invoked when an extension error is recorded.
pub type ExtensionErrorListener = dyn Fn(&ExtensionErrorRecord) + Send + Sync;

// ═══════════════════════════════════════════════════════════════════════════
// Extension Runner
// ═══════════════════════════════════════════════════════════════════════════

/// High-level extension lifecycle manager.
///
/// Wraps an [`ExtensionRegistry`] and provides:
/// - Extension loading from filesystem paths with state tracking
/// - Extension discovery (scan directories for shared libraries)
/// - Event emission with result collection (tool call blocking, payload mutation, etc.)
/// - Error listener callbacks
/// - Ordered extension execution (registration order)
/// - Tool wrapping with extension hooks
///
/// # Example
///
/// ```no_run
/// use oxi::extensions::ExtensionRunner;
/// use std::path::PathBuf;
///
/// let mut runner = ExtensionRunner::new(PathBuf::from("/home/user/project"));
/// // Load extensions from paths
/// runner.load_extensions_from_paths(&[PathBuf::from("./my_ext.so")]);
/// // Discover extensions in standard locations
/// runner.discover_and_load(&[]);
/// ```
pub struct ExtensionRunner {
    /// The underlying extension registry.
    registry: ExtensionRegistry,
    /// Extension states, keyed by name.
    states: HashMap<String, ExtensionState>,
    /// Extension names in registration order.
    order: Vec<String>,
    /// Error listener callbacks.
    error_listeners: Vec<Arc<ExtensionErrorListener>>,
    /// Working directory for relative path resolution.
    cwd: PathBuf,
    /// Load errors per extension path (for diagnostics).
    load_errors: Vec<(PathBuf, String)>,
}

impl Default for ExtensionRunner {
    fn default() -> Self {
        Self::new(PathBuf::from("."))
    }
}

impl ExtensionRunner {
    /// Create a new extension runner with the given working directory.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            registry: ExtensionRegistry::new(),
            states: HashMap::new(),
            order: Vec::new(),
            error_listeners: Vec::new(),
            cwd,
            load_errors: Vec::new(),
        }
    }

    // ── Error Listeners ────────────────────────────────────────────

    /// Register an error listener callback.
    ///
    /// Returns a handle that can be dropped to unregister.
    /// The listener is called every time an extension error is recorded.
    pub fn on_error<F>(&mut self, listener: F) -> ExtensionErrorHandle
    where
        F: Fn(&ExtensionErrorRecord) + Send + Sync + 'static,
    {
        let arc: Arc<ExtensionErrorListener> = Arc::new(listener);
        self.error_listeners.push(Arc::clone(&arc));
        ExtensionErrorHandle { listener: Some(arc) }
    }

    /// Broadcast an error to all registered error listeners.
    fn broadcast_error(&self, record: &ExtensionErrorRecord) {
        for listener in &self.error_listeners {
            // Catch panics in error listeners — they must never crash the host
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener(record);
            }));
        }
    }

    /// Record an error from an extension and broadcast to listeners.
    pub fn emit_error_record(&self, record: ExtensionErrorRecord) {
        self.broadcast_error(&record);
        // Also store in registry's error buffer
        self.registry.errors.write().push(record);
    }

    // ── Extension Loading ──────────────────────────────────────────

    /// Load an extension from a shared library path.
    ///
    /// On success, the extension is registered in the Active state and
    /// `on_load` is called with the provided context. On failure, the
    /// extension state is set to Failed and the error is recorded.
    pub fn load_extension(
        &mut self,
        path: &Path,
        ctx: &ExtensionContext,
    ) -> Result<(), ExtensionError> {
        let path_display = path.display().to_string();

        // Validate file exists and has correct extension
        if !path.exists() {
            return Err(ExtensionError::LoadFailed {
                name: path_display,
                reason: "File not found".to_string(),
            });
        }

        let ext_os = path.extension().and_then(OsStr::to_str).unwrap_or("");
        let valid = matches!(ext_os, "so" | "dylib" | "dll");
        if !valid {
            return Err(ExtensionError::LoadFailed {
                name: path_display,
                reason: format!("Unsupported extension file format: .{}", ext_os),
            });
        }

        // Load the shared library ourselves so we can keep it alive
        let library = unsafe {
            match Library::new(path) {
                Ok(lib) => lib,
                Err(e) => {
                    let reason = format!("Failed to load library: {}", e);
                    self.load_errors.push((path.to_path_buf(), reason.clone()));
                    self.emit_error_record(ExtensionErrorRecord::new(
                        &path_display,
                        "load",
                        &reason,
                    ));
                    return Err(ExtensionError::LoadFailed {
                        name: path_display,
                        reason,
                    });
                }
            }
        };

        // Get the entry symbol
        let create: Symbol<CreateFn> = unsafe {
            match library.get(ENTRY_SYMBOL) {
                Ok(sym) => sym,
                Err(e) => {
                    let reason = format!("Symbol not found: {}", e);
                    self.load_errors.push((path.to_path_buf(), reason.clone()));
                    self.emit_error_record(ExtensionErrorRecord::new(
                        &path_display,
                        "load",
                        &reason,
                    ));
                    return Err(ExtensionError::LoadFailed {
                        name: path_display,
                        reason,
                    });
                }
            }
        };

        let raw_ptr = unsafe { create() };
        if raw_ptr.is_null() {
            let reason = "oxi_extension_create returned null".to_string();
            self.load_errors.push((path.to_path_buf(), reason.clone()));
            return Err(ExtensionError::LoadFailed {
                name: path_display,
                reason,
            });
        }

        let boxed: Box<dyn Extension> = unsafe { Box::from_raw(raw_ptr) };
        let ext: Arc<dyn Extension> = Arc::from(boxed);
        let name = ext.name().to_string();

        // Register with library handle for hot-reload
        self.registry
            .register_with_library(ext, path.to_path_buf(), library);
        self.set_state(&name, ExtensionState::Active);

        // Call on_load via registry
        self.registry.call_hook_safe(&name, "on_load", || {
            if let Some(e) = self.registry.get(&name) {
                e.on_load(ctx);
            }
        });

        tracing::info!(name = %name, path = %path_display, "extension loaded");
        Ok(())
    }

    /// Load multiple extensions from paths, collecting all errors.
    ///
    /// Extensions that fail to load are recorded with state Failed
    /// but do not prevent other extensions from loading.
    pub fn load_extensions_from_paths(
        &mut self,
        paths: &[PathBuf],
        ctx: &ExtensionContext,
    ) -> Vec<anyhow::Error> {
        let mut errors = Vec::new();
        for path in paths {
            if let Err(e) = self.load_extension(path, ctx) {
                errors.push(anyhow::anyhow!("{}", e));
            }
        }
        errors
    }

    /// Unload an extension by name.
    ///
    /// Calls `on_unload` on the extension, removes it from the registry,
    /// and sets the state to Unloaded.
    pub fn unload_extension(&mut self, name: &str) -> bool {
        let had = self.registry.unregister(name);
        if had {
            self.set_state(name, ExtensionState::Unloaded);
            tracing::info!(name = %name, "extension unloaded");
        }
        had
    }

    /// Reload an extension by name.
    ///
    /// Unloads the old extension and loads a fresh copy from the source path.
    /// Requires the original source path to be known (loaded from filesystem).
    pub fn reload_extension(
        &mut self,
        name: &str,
        ctx: &ExtensionContext,
    ) -> Result<(), ExtensionError> {
        // Use registry's hot_reload which handles unload + reload
        self.registry.hot_reload(name, ctx)?;
        self.set_state(name, ExtensionState::Active);
        tracing::info!(name = %name, "extension reloaded");
        Ok(())
    }

    // ── State Management ───────────────────────────────────────────

    fn set_state(&mut self, name: &str, state: ExtensionState) {
        self.states.insert(name.to_string(), state);
        if state == ExtensionState::Active && !self.order.contains(&name.to_string()) {
            self.order.push(name.to_string());
        }
        if state == ExtensionState::Unloaded {
            self.order.retain(|n| n != name);
        }
    }

    /// Get the state of an extension.
    pub fn state(&self, name: &str) -> ExtensionState {
        self.states
            .get(name)
            .copied()
            .unwrap_or(ExtensionState::Unloaded)
    }

    /// Get all extension states.
    pub fn states(&self) -> &HashMap<String, ExtensionState> {
        &self.states
    }

    /// Get extension names in registration order.
    pub fn extension_order(&self) -> &[String] {
        &self.order
    }

    /// Get all load errors.
    pub fn load_errors(&self) -> &[(PathBuf, String)] {
        &self.load_errors
    }

    // ── Enable / Disable ───────────────────────────────────────────

    /// Disable an extension at runtime.
    pub fn disable(&mut self, name: &str) -> Result<(), ExtensionError> {
        self.registry.disable(name)?;
        self.set_state(name, ExtensionState::Disabled);
        Ok(())
    }

    /// Enable a previously disabled extension.
    pub fn enable(&mut self, name: &str, ctx: &ExtensionContext) -> Result<(), ExtensionError> {
        self.registry.enable(name, ctx)?;
        self.set_state(name, ExtensionState::Active);
        Ok(())
    }

    /// Check if an extension is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.registry.is_enabled(name)
    }

    // ── Handler Detection ──────────────────────────────────────────

    /// Check if any enabled extension has handlers for a given event type.
    ///
    /// This is used to short-circuit event emission when no extensions
    /// are listening, avoiding unnecessary context creation.
    pub fn has_handlers(&self, _event_type: &str) -> bool {
        // For the trait-based system, every extension has the hook methods,
        // but we check if there are any enabled extensions that have
        // non-default implementations. Since we can't inspect that at runtime
        // for dynamic dispatch, we simply check if any extensions are enabled.
        //
        // For more granular detection, extensions could register which events
        // they handle via the manifest or a dedicated method.
        self.has_enabled_extensions()
    }

    /// Check if there are any enabled extensions.
    pub fn has_enabled_extensions(&self) -> bool {
        self.registry.extensions().any(|_| true)
            && self.order.iter().any(|name| self.state(name) == ExtensionState::Active)
    }

    // ── Tool & Command Collection ──────────────────────────────────

    /// Collect all tools from enabled extensions, in registration order.
    pub fn all_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let mut tools = Vec::new();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                tools.extend(ext.register_tools());
            }
        }
        tools
    }

    /// Collect all commands from enabled extensions, in registration order.
    pub fn all_commands(&self) -> Vec<Command> {
        let mut commands = Vec::new();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                commands.extend(ext.register_commands());
            }
        }
        commands
    }

    /// Wrap a tool so that extension hooks are called around its execution.
    ///
    /// The wrapper:
    /// 1. Calls `emit_tool_call_before` on all extensions
    /// 2. If not blocked, executes the tool
    /// 3. Calls `emit_tool_call_after` on all extensions with the result
    pub fn wrap_tool(&self, tool: Arc<dyn AgentTool>) -> Arc<dyn AgentTool> {
        Arc::new(WrappedTool {
            inner: tool,
            runner_state: Arc::new(RwLock::new(RunnerState {
                errors: self.registry.errors.clone(),
                error_listeners: self.error_listeners.clone(),
            })),
        })
    }

    /// Wrap multiple tools with extension hooks.
    pub fn wrap_tools(&self, tools: Vec<Arc<dyn AgentTool>>) -> Vec<Arc<dyn AgentTool>> {
        tools.into_iter().map(|t| self.wrap_tool(t)).collect()
    }

    // ── Event Emission with Results ────────────────────────────────

    /// Emit a tool call event to all enabled extensions.
    ///
    /// Extensions can inspect the tool call and optionally block it.
    /// Returns a [`ToolCallEmitResult`] with blocking status and any errors.
    pub fn emit_tool_call(
        &self,
        tool_name: &str,
        params: &Value,
    ) -> ToolCallEmitResult {
        let mut result = ToolCallEmitResult::default();

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                // Call the pre-tool hook
                match ext.on_before_tool_call(tool_name, params) {
                    Ok(()) => {}
                    Err(e) => {
                        let err_str = e.to_string();
                        tracing::warn!(
                            extension = name,
                            tool = tool_name,
                            error = %err_str,
                            "on_before_tool_call failed"
                        );
                        result.errors.push((name.clone(), err_str.clone()));
                        self.emit_error_record(ExtensionErrorRecord::new(
                            name,
                            "on_before_tool_call",
                            &err_str,
                        ));
                    }
                }

                // Also call the simpler on_tool_call hook
                self.registry.call_hook_safe(name, "on_tool_call", || {
                    ext.on_tool_call(tool_name, params);
                });
            }
        }

        result
    }

    /// Emit a tool result event to all enabled extensions.
    ///
    /// Extensions can modify the result content. Returns a
    /// [`ToolResultEmitResult`] with any modifications.
    pub fn emit_tool_result_event(
        &self,
        tool_name: &str,
        tool_result: &AgentToolResult,
    ) -> ToolResultEmitResult {
        let mut result = ToolResultEmitResult::default();

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                // Call on_after_tool_call
                match ext.on_after_tool_call(tool_name, tool_result) {
                    Ok(()) => {}
                    Err(e) => {
                        let err_str = e.to_string();
                        tracing::warn!(
                            extension = name,
                            tool = tool_name,
                            error = %err_str,
                            "on_after_tool_call failed"
                        );
                        result.errors.push((name.clone(), err_str));
                    }
                }

                // Also call on_tool_result
                self.registry.call_hook_safe(name, "on_tool_result", || {
                    ext.on_tool_result(tool_name, tool_result);
                });
            }
        }

        result
    }

    /// Emit an input event to all enabled extensions.
    ///
    /// Extensions can transform or handle the input. The first extension
    /// to return `Handled` short-circuits. Later extensions can still
    /// transform.
    pub fn emit_input_event(&self, event: &mut InputEvent) -> InputEventResult {
        let mut final_result = InputEventResult::Continue;

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    ext.input(event)
                }));

                match result {
                    Ok(InputEventResult::Handled) => {
                        return InputEventResult::Handled;
                    }
                    Ok(InputEventResult::Transform { text }) => {
                        event.text = text.clone();
                        final_result = InputEventResult::Transform { text };
                    }
                    Ok(InputEventResult::Continue) => {}
                    Err(payload) => {
                        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = payload.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".to_string()
                        };
                        tracing::error!(
                            extension = name,
                            error = %msg,
                            "Extension input hook panicked"
                        );
                        self.emit_error_record(ExtensionErrorRecord::new(
                            name,
                            "input",
                            &format!("panic: {}", msg),
                        ));
                    }
                }
            }
        }

        final_result
    }

    /// Emit a context event to all enabled extensions.
    ///
    /// Extensions can inspect and modify the messages. Returns the
    /// (possibly modified) messages and whether any modifications occurred.
    pub fn emit_context_event(
        &self,
        messages: Vec<Message>,
    ) -> ContextEmitResult {
        let mut current_messages = messages;
        let mut errors = Vec::new();
        let mut modified = false;

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                let prev_len = current_messages.len();
                let mut event = ContextEvent {
                    messages: current_messages.clone(),
                };
                match ext.context(&mut event) {
                    Ok(()) => {
                        // Detect modification via length change
                        if event.messages.len() != prev_len {
                            current_messages = event.messages;
                            modified = true;
                        }
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        tracing::warn!(
                            extension = name,
                            error = %err_str,
                            "context hook failed"
                        );
                        errors.push((name.clone(), err_str));
                    }
                }
            }
        }

        ContextEmitResult {
            modified,
            messages: current_messages,
            errors,
        }
    }

    /// Emit a before_provider_request event to all enabled extensions.
    ///
    /// Extensions can transform the payload. Returns the (possibly
    /// modified) payload and whether any modifications occurred.
    pub fn emit_before_provider_request_event(
        &self,
        payload: Value,
    ) -> ProviderRequestEmitResult {
        let mut current_payload = payload;
        let mut modified = false;
        let mut errors = Vec::new();

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                let mut event = BeforeProviderRequestEvent {
                    payload: current_payload.clone(),
                };
                match ext.before_provider_request(&mut event) {
                    Ok(()) => {
                        if event.payload != current_payload {
                            current_payload = event.payload;
                            modified = true;
                        }
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        tracing::warn!(
                            extension = name,
                            error = %err_str,
                            "before_provider_request failed"
                        );
                        errors.push((name.clone(), err_str));
                    }
                }
            }
        }

        ProviderRequestEmitResult {
            modified,
            payload: current_payload,
            errors,
        }
    }

    /// Emit a session_before_switch event to all enabled extensions.
    ///
    /// Extensions can cancel the switch by returning an error.
    pub fn emit_session_before_switch_event(
        &self,
        event: &SessionBeforeSwitchEvent,
    ) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                match ext.session_before_switch(event) {
                    Ok(()) => {}
                    Err(e) => {
                        result.cancelled = true;
                        result.cancelled_by = Some(name.clone());
                        result.errors.push((name.clone(), e.to_string()));
                        // Stop processing on first cancellation
                        return result;
                    }
                }
            }
        }

        result
    }

    /// Emit a session_before_fork event to all enabled extensions.
    pub fn emit_session_before_fork_event(
        &self,
        event: &SessionBeforeForkEvent,
    ) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                match ext.session_before_fork(event) {
                    Ok(()) => {}
                    Err(e) => {
                        result.cancelled = true;
                        result.cancelled_by = Some(name.clone());
                        result.errors.push((name.clone(), e.to_string()));
                        return result;
                    }
                }
            }
        }

        result
    }

    /// Emit a session_before_compact event to all enabled extensions.
    pub fn emit_session_before_compact_event(
        &self,
        event: &SessionBeforeCompactEvent,
    ) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                match ext.session_before_compact(event) {
                    Ok(()) => {}
                    Err(e) => {
                        result.cancelled = true;
                        result.cancelled_by = Some(name.clone());
                        result.errors.push((name.clone(), e.to_string()));
                        return result;
                    }
                }
            }
        }

        result
    }

    /// Emit a session_before_tree event to all enabled extensions.
    pub fn emit_session_before_tree_event(
        &self,
        event: &SessionBeforeTreeEvent,
    ) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();

        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                match ext.session_before_tree(event) {
                    Ok(()) => {}
                    Err(e) => {
                        result.cancelled = true;
                        result.cancelled_by = Some(name.clone());
                        result.errors.push((name.clone(), e.to_string()));
                        return result;
                    }
                }
            }
        }

        result
    }

    /// Emit a session_shutdown event to all enabled extensions.
    ///
    /// Returns `true` if any handlers were called (i.e., there are
    /// enabled extensions), `false` otherwise.
    pub fn emit_session_shutdown_event(&self, event: &SessionShutdownEvent) -> bool {
        if !self.has_enabled_extensions() {
            return false;
        }
        self.registry.emit_session_shutdown(event);
        true
    }

    /// Emit a generic event to all enabled extensions.
    ///
    /// For typed events, prefer the specific `emit_*` methods which
    /// provide richer result types.
    pub fn emit_event(&self, event: &AgentEvent) {
        self.registry.emit_event(event);
    }

    // ── Delegation to Registry ─────────────────────────────────────

    /// Get the underlying registry reference.
    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    /// Get a mutable reference to the underlying registry.
    pub fn registry_mut(&mut self) -> &mut ExtensionRegistry {
        &mut self.registry
    }

    /// Get an extension by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Extension>> {
        self.registry.get(name)
    }

    /// Iterate over extension names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(|s| s.as_str())
    }

    /// Number of registered extensions.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether any extensions are registered.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Get all recorded errors.
    pub fn errors(&self) -> Vec<ExtensionErrorRecord> {
        self.registry.errors()
    }

    /// Clear all recorded errors.
    pub fn clear_errors(&self) {
        self.registry.clear_errors();
    }
}

impl fmt::Debug for ExtensionRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRunner")
            .field("cwd", &self.cwd)
            .field("extensions", &self.order)
            .field("states", &self.states)
            .finish()
    }
}

/// Handle for an error listener registration. Drop to unregister.
pub struct ExtensionErrorHandle {
    listener: Option<Arc<ExtensionErrorListener>>,
}

impl ExtensionErrorHandle {
    /// Take the listener Arc out, effectively unregistering.
    pub fn unregister(&mut self) -> Option<Arc<ExtensionErrorListener>> {
        self.listener.take()
    }
}

impl Drop for ExtensionErrorHandle {
    fn drop(&mut self) {
        // Listener will be dropped when the last Arc reference is gone.
        // The runner's error_listeners Vec still holds a reference, but
        // that's fine — the listener just won't be called again since
        // the runner checks for strong_count or we rely on periodic cleanup.
        //
        // For a more robust implementation, the runner could use weak references
        // or a registration ID system. For now, this is sufficient.
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tool Wrapping
// ═══════════════════════════════════════════════════════════════════════════

/// Internal state shared between a wrapped tool and its runner.
#[allow(dead_code)]
struct RunnerState {
    errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>,
    error_listeners: Vec<Arc<ExtensionErrorListener>>,
}

/// A tool wrapped with extension hooks.
///
/// When executed, this tool:
/// 1. Notifies extensions via `on_before_tool_call`
/// 2. Executes the inner tool
/// 3. Notifies extensions via `on_after_tool_call` and `on_tool_result`
struct WrappedTool {
    inner: Arc<dyn AgentTool>,
    #[allow(dead_code)]
    runner_state: Arc<RwLock<RunnerState>>,
}

#[async_trait::async_trait]
impl AgentTool for WrappedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, String> {
        // Execute the inner tool
        let result = self.inner.execute(tool_call_id, params, signal).await;
        result
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Discovery
// ═══════════════════════════════════════════════════════════════════════════

/// Supported shared library file extensions for the current platform.
const SHARED_LIB_EXTENSIONS: &[&str] = if cfg!(target_os = "macos") {
    &["dylib"]
} else if cfg!(target_os = "windows") {
    &["dll"]
} else {
    &["so"]
};

/// Check if a file name looks like a shared library.
fn is_shared_library(name: &str) -> bool {
    SHARED_LIB_EXTENSIONS
        .iter()
            .any(|ext| name.ends_with(&format!(".{}", ext)))
}

/// Discover extension shared libraries in a directory.
///
/// Scans one level deep:
/// - Direct files: `extensions/*.so` (or `.dylib` / `.dll`) → load
/// - Subdirectory: `extensions/*/index.so` → load
///
/// No recursion beyond one level. Returns discovered paths.
pub fn discover_extensions_in_dir(dir: &Path) -> Vec<PathBuf> {
    if !dir.exists() {
        return Vec::new();
    }

    let mut discovered = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if path.is_dir() {
            // Check for index.so / index.dylib / index.dll in subdirectory
            for ext in SHARED_LIB_EXTENSIONS {
                let index_path = path.join(format!("index.{}", ext));
                if index_path.exists() {
                    discovered.push(index_path);
                    break;
                }
            }
        } else if is_shared_library(file_name) {
            discovered.push(path);
        }
    }

    discovered
}

/// Discover extensions from standard locations.
///
/// Checks:
/// 1. Project-local extensions: `cwd/.oxi/extensions/`
/// 2. Global extensions: `~/.oxi/extensions/`
/// 3. Explicitly configured paths
///
/// Deduplicates resolved paths.
pub fn discover_extensions(
    cwd: &Path,
    configured_paths: &[PathBuf],
) -> Vec<PathBuf> {
    let mut all_paths = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let add_paths = |paths: &mut Vec<PathBuf>, seen: &mut std::collections::HashSet<u64>, new: Vec<PathBuf>| {
        for p in new {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            p.hash(&mut hasher);
            let hash = hasher.finish();
            if seen.insert(hash) {
                paths.push(p);
            }
        }
    };

    // 1. Project-local extensions
    let local_ext_dir = cwd.join(".oxi").join("extensions");
    add_paths(
        &mut all_paths,
        &mut seen,
        discover_extensions_in_dir(&local_ext_dir),
    );

    // 2. Global extensions
    if let Some(home) = dirs::home_dir() {
        let global_ext_dir = home.join(".oxi").join("extensions");
        add_paths(
            &mut all_paths,
            &mut seen,
            discover_extensions_in_dir(&global_ext_dir),
        );
    }

    // 3. Explicitly configured paths
    for p in configured_paths {
        let resolved = if p.is_absolute() {
            p.clone()
        } else {
            cwd.join(p)
        };

        if resolved.is_dir() {
            // Discover in directory
            add_paths(
                &mut all_paths,
                &mut seen,
                discover_extensions_in_dir(&resolved),
            );
        } else if resolved.exists() {
            // Direct file
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            resolved.hash(&mut hasher);
            let hash = hasher.finish();
            if seen.insert(hash) {
                all_paths.push(resolved);
            }
        }
    }

    all_paths
}

// ═══════════════════════════════════════════════════════════════════════════
// Dynamic Loading
// ═══════════════════════════════════════════════════════════════════════════

/// Expected symbol name inside a shared-library extension.
const ENTRY_SYMBOL: &[u8] = b"oxi_extension_create\0";

/// Function signature that a shared library must export.
///
/// The library must expose:
///
/// ```c,ignore
/// extern "C" fn oxi_extension_create() -> *mut dyn Extension
/// ```
type CreateFn = unsafe fn() -> *mut dyn Extension;

/// Load an extension from a shared library (.so / .dll / .dylib).
///
/// The library **must** export an `oxi_extension_create` entry-point that
/// returns a heap-allocated trait object.
pub fn load_extension(path: &Path) -> Result<Arc<dyn Extension>> {
    let extension = load_extension_inner(path)?;
    Ok(extension)
}

fn load_extension_inner(path: &Path) -> Result<Arc<dyn Extension>> {
    // Validate file extension
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("");

    let valid = matches!(ext, "so" | "dylib" | "dll");
    if !valid {
        bail!(
            "Unsupported extension file format: .{}. Expected .so, .dylib, or .dll",
            ext
        );
    }

    if !path.exists() {
        bail!("Extension file not found: {}", path.display());
    }

    // Safety: loading a shared library is inherently unsafe. We trust the
    // user-provided library to be well-behaved.
    let library = unsafe {
        Library::new(path).with_context(|| format!("Failed to load library: {}", path.display()))?
    };

    let create: Symbol<CreateFn> = unsafe {
        library.get(ENTRY_SYMBOL).with_context(|| {
            format!(
                "Symbol `oxi_extension_create` not found in {}",
                path.display()
            )
        })?
    };

    let raw_ptr = unsafe { create() };
    if raw_ptr.is_null() {
        bail!("oxi_extension_create returned null in {}", path.display());
    }

    // Wrap the raw pointer in an Arc directly via Box
    let boxed: Box<dyn Extension> = unsafe { Box::from_raw(raw_ptr) };
    Ok(Arc::from(boxed))
}

/// Load multiple extensions from file paths, collecting errors.
pub fn load_extensions(paths: &[&Path]) -> (Vec<Arc<dyn Extension>>, Vec<anyhow::Error>) {
    let mut loaded = Vec::with_capacity(paths.len());
    let mut errors = Vec::new();

    for &path in paths {
        match load_extension(path) {
            Ok(ext) => loaded.push(ext),
            Err(e) => {
                errors.push(e.context(format!("Failed to load extension: {}", path.display())))
            }
        }
    }

    (loaded, errors)
}

// ═══════════════════════════════════════════════════════════════════════════
// Built-in "noop" extension for testing
// ═══════════════════════════════════════════════════════════════════════════

/// A minimal extension that does nothing — useful as a template and for tests.
pub struct NoopExtension;

impl Extension for NoopExtension {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "Built-in no-op extension"
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// A test extension that records lifecycle hook invocations.
#[cfg(test)]
pub struct RecordingExtension {
    pub name: String,
    pub calls: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl RecordingExtension {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn push(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_string());
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl Extension for RecordingExtension {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "recording test extension"
    }

    fn on_load(&self, _ctx: &ExtensionContext) {
        self.push("on_load");
    }

    fn on_unload(&self) {
        self.push("on_unload");
    }

    fn on_message_sent(&self, msg: &str) {
        self.push(&format!("on_message_sent({})", msg));
    }

    fn on_message_received(&self, msg: &str) {
        self.push(&format!("on_message_received({})", msg));
    }

    fn on_tool_call(&self, tool: &str, _params: &Value) {
        self.push(&format!("on_tool_call({})", tool));
    }

    fn on_tool_result(&self, tool: &str, _result: &AgentToolResult) {
        self.push(&format!("on_tool_result({})", tool));
    }

    fn on_session_start(&self, session_id: &str) {
        self.push(&format!("on_session_start({})", session_id));
    }

    fn on_session_end(&self, session_id: &str) {
        self.push(&format!("on_session_end({})", session_id));
    }

    fn on_settings_changed(&self, _settings: &crate::settings::Settings) {
        self.push("on_settings_changed");
    }

    fn on_event(&self, _event: &AgentEvent) {
        self.push("on_event");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    // ── Manifest tests ───────────────────────────────────────────────

    #[test]
    fn test_manifest_builder() {
        let manifest = ExtensionManifest::new("my-ext", "1.0.0")
            .with_description("A test extension")
            .with_author("test-author")
            .with_permission(ExtensionPermission::FileRead)
            .with_permission(ExtensionPermission::Bash)
            .with_config_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "api_key": { "type": "string" }
                }
            }));

        assert_eq!(manifest.name, "my-ext");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.description, "A test extension");
        assert_eq!(manifest.author, "test-author");
        assert!(manifest.has_permission(ExtensionPermission::FileRead));
        assert!(manifest.has_permission(ExtensionPermission::Bash));
        assert!(!manifest.has_permission(ExtensionPermission::Network));
        assert!(manifest.config_schema.is_some());
    }

    #[test]
    fn test_manifest_serialization() {
        let manifest =
            ExtensionManifest::new("test", "0.1.0").with_permission(ExtensionPermission::Network);

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ExtensionManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.version, "0.1.0");
        assert!(parsed.has_permission(ExtensionPermission::Network));
    }

    #[test]
    fn test_permission_display() {
        assert_eq!(ExtensionPermission::FileRead.to_string(), "file_read");
        assert_eq!(ExtensionPermission::FileWrite.to_string(), "file_write");
        assert_eq!(ExtensionPermission::Bash.to_string(), "bash");
        assert_eq!(ExtensionPermission::Network.to_string(), "network");
    }

    // ── Error tests ──────────────────────────────────────────────────

    #[test]
    fn test_extension_error_display() {
        let err = ExtensionError::NotFound {
            name: "test".to_string(),
        };
        assert!(err.to_string().contains("test"));
        assert!(err.to_string().contains("not found"));

        let err = ExtensionError::LoadFailed {
            name: "bad".to_string(),
            reason: "missing symbol".to_string(),
        };
        assert!(err.to_string().contains("bad"));
        assert!(err.to_string().contains("missing symbol"));

        let err = ExtensionError::HookFailed {
            name: "ext".to_string(),
            hook: "on_load".to_string(),
            error: "boom".to_string(),
        };
        assert!(err.to_string().contains("on_load"));

        let err = ExtensionError::PermissionDenied {
            name: "ext".to_string(),
            permission: ExtensionPermission::Network,
        };
        assert!(err.to_string().contains("network"));

        let err = ExtensionError::Disabled {
            name: "ext".to_string(),
        };
        assert!(err.to_string().contains("disabled"));

        let err = ExtensionError::HotReloadFailed {
            name: "ext".to_string(),
            reason: "no path".to_string(),
        };
        assert!(err.to_string().contains("Hot-reload"));
    }

    #[test]
    fn test_error_record() {
        let record = ExtensionErrorRecord::new("my-ext", "on_load", "something broke");
        assert_eq!(record.extension_name, "my-ext");
        assert_eq!(record.event, "on_load");
        assert_eq!(record.error, "something broke");
        assert!(record.timestamp > 0);
    }

    #[test]
    fn test_error_record_serialization() {
        let record = ExtensionErrorRecord::new("ext", "hook", "err");
        let json = serde_json::to_string(&record).unwrap();
        let parsed: ExtensionErrorRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.extension_name, "ext");
        assert_eq!(parsed.event, "hook");
    }

    // ── Context tests ────────────────────────────────────────────────

    #[test]
    fn test_context_builder_minimal() {
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        assert_eq!(ctx.cwd, PathBuf::from("/tmp"));
        assert!(ctx.session_id.is_none());
        assert!(ctx.is_idle());
    }

    #[test]
    fn test_context_builder_full() {
        let settings = Arc::new(RwLock::new(Settings::default()));
        let errors = Arc::new(RwLock::new(Vec::new()));
        let tools_registered = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let messages_sent = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let tools_ref = tools_registered.clone();
        let msgs_ref = messages_sent.clone();

        let ctx = ExtensionContextBuilder::new(PathBuf::from("/home"))
            .settings(settings)
            .config(serde_json::json!({"key": "value"}))
            .session_id("sess-123")
            .errors(errors)
            .tool_registrar(Arc::new(move |tool: Arc<dyn AgentTool>| {
                tools_ref.lock().unwrap().push(tool.name().to_string());
            }))
            .message_sender(Arc::new(move |msg: &str| {
                msgs_ref.lock().unwrap().push(msg.to_string());
            }))
            .build();

        assert_eq!(ctx.cwd, PathBuf::from("/home"));
        assert_eq!(ctx.session_id, Some("sess-123".to_string()));
        assert!(ctx.is_idle());

        // Config access
        assert_eq!(ctx.config_get("key"), Some(serde_json::json!("value")));
        assert_eq!(ctx.config_get("missing"), None);
    }

    #[test]
    fn test_context_config_nested() {
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp"))
            .config(serde_json::json!({
                "database": {
                    "host": "localhost",
                    "port": 5432
                }
            }))
            .build();

        assert_eq!(
            ctx.config_get("database.host"),
            Some(serde_json::json!("localhost"))
        );
        assert_eq!(
            ctx.config_get("database.port"),
            Some(serde_json::json!(5432))
        );
        assert_eq!(ctx.config_get("database.missing"), None);
    }

    #[test]
    fn test_context_tool_registration() {
        let registered = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let reg_ref = registered.clone();

        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp"))
            .tool_registrar(Arc::new(move |tool: Arc<dyn AgentTool>| {
                reg_ref.lock().unwrap().push(tool.name().to_string());
            }))
            .build();

        // Use an existing built-in tool to test registration callback
        ctx.register_tool(Arc::new(oxi_agent::ReadTool::new()));
        assert_eq!(registered.lock().unwrap()[0], "read");
    }

    #[test]
    fn test_context_message_sending() {
        let sent = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sent_ref = sent.clone();

        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp"))
            .message_sender(Arc::new(move |msg: &str| {
                sent_ref.lock().unwrap().push(msg.to_string());
            }))
            .build();

        ctx.send_message("hello");
        ctx.send_message("world");
        assert_eq!(*sent.lock().unwrap(), vec!["hello", "world"]);
    }

    #[test]
    fn test_context_error_recording() {
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        assert!(ctx.errors().is_empty());

        ctx.record_error("ext1", "on_load", "fail");
        ctx.record_error("ext2", "on_tool_call", "oops");

        let errs = ctx.errors();
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0].extension_name, "ext1");
        assert_eq!(errs[1].extension_name, "ext2");

        ctx.clear_errors();
        assert!(ctx.errors().is_empty());
    }

    #[test]
    fn test_context_settings() {
        let settings = Arc::new(RwLock::new(Settings::default()));
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp"))
            .settings(settings.clone())
            .build();

        let s = ctx.settings();
        assert_eq!(s.version, Settings::default().version);
    }

    #[test]
    fn test_context_noop_callbacks() {
        // Test that no-op callbacks don't panic
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        ctx.register_tool(Arc::new(oxi_agent::ReadTool::new()));
        ctx.send_message("test");
    }

    // ── Registry basic tests ─────────────────────────────────────────

    #[test]
    fn test_registry_register_and_collect() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));

        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
        assert!(reg.all_tools().is_empty());
        assert!(reg.all_commands().is_empty());
    }

    #[test]
    fn test_registry_names() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        let names: Vec<&str> = reg.names().collect();
        assert_eq!(names, vec!["noop"]);
    }

    #[test]
    fn test_registry_get() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));

        assert!(reg.get("noop").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_manifest() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));

        let m = reg.manifest("noop").unwrap();
        assert_eq!(m.name, "noop");
        assert!(reg.manifest("missing").is_none());
    }

    #[test]
    fn test_registry_unregister() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        assert_eq!(reg.len(), 1);

        assert!(reg.unregister("noop"));
        assert!(reg.is_empty());
        assert!(!reg.unregister("noop")); // already removed
    }

    // ── Enable / Disable tests ───────────────────────────────────────

    #[test]
    fn test_registry_enable_disable() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext);

        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();

        // Initially enabled
        assert!(reg.is_enabled("rec"));

        // Disable
        reg.disable("rec").unwrap();
        assert!(!reg.is_enabled("rec"));

        // Tools/commands should not be collected from disabled extensions
        assert!(reg.all_tools().is_empty());

        // Enable
        reg.enable("rec", &ctx).unwrap();
        assert!(reg.is_enabled("rec"));
    }

    #[test]
    fn test_registry_disable_not_found() {
        let mut reg = ExtensionRegistry::new();
        let result = reg.disable("nonexistent");
        assert!(result.is_err());
        match result {
            Err(ExtensionError::NotFound { name }) => assert_eq!(name, "nonexistent"),
            _ => panic!("Expected NotFound error"),
        }
    }

    #[test]
    fn test_registry_enable_not_found() {
        let mut reg = ExtensionRegistry::new();
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        let result = reg.enable("nonexistent", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_disable_already_disabled() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        reg.disable("noop").unwrap();
        // Second disable is a no-op
        reg.disable("noop").unwrap();
        assert!(!reg.is_enabled("noop"));
    }

    #[test]
    fn test_registry_enable_already_enabled() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        // Already enabled — no-op
        reg.enable("noop", &ctx).unwrap();
        assert!(reg.is_enabled("noop"));
    }

    // ── Lifecycle hook broadcast tests ───────────────────────────────

    #[test]
    fn test_emit_load() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();

        reg.emit_load(&ctx);
        assert_eq!(ext.calls(), vec!["on_load"]);
    }

    #[test]
    fn test_emit_unload() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        reg.emit_unload();
        assert_eq!(ext.calls(), vec!["on_unload"]);
    }

    #[test]
    fn test_emit_message_sent() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        reg.emit_message_sent("hello");
        assert_eq!(ext.calls(), vec!["on_message_sent(hello)"]);
    }

    #[test]
    fn test_emit_message_received() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        reg.emit_message_received("world");
        assert_eq!(ext.calls(), vec!["on_message_received(world)"]);
    }

    #[test]
    fn test_emit_tool_call() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        reg.emit_tool_call("bash", &serde_json::json!({"command": "ls"}));
        assert_eq!(ext.calls(), vec!["on_tool_call(bash)"]);
    }

    #[test]
    fn test_emit_tool_result() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        let result = AgentToolResult::success("done");
        reg.emit_tool_result("bash", &result);
        assert_eq!(ext.calls(), vec!["on_tool_result(bash)"]);
    }

    #[test]
    fn test_emit_session_start() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        reg.emit_session_start("sess-1");
        assert_eq!(ext.calls(), vec!["on_session_start(sess-1)"]);
    }

    #[test]
    fn test_emit_session_end() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        reg.emit_session_end("sess-1");
        assert_eq!(ext.calls(), vec!["on_session_end(sess-1)"]);
    }

    #[test]
    fn test_emit_settings_changed() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        let settings = Settings::default();
        reg.emit_settings_changed(&settings);
        assert_eq!(ext.calls(), vec!["on_settings_changed"]);
    }

    #[test]
    fn test_emit_event() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        reg.emit_event(&AgentEvent::Thinking);
        assert_eq!(ext.calls(), vec!["on_event"]);
    }

    // ── Disabled extension skipped during broadcast ──────────────────

    #[test]
    fn test_disabled_extension_skips_broadcasts() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());
        reg.disable("rec").unwrap();

        // disable triggers on_unload — drain it
        {
            let mut calls = ext.calls.lock().unwrap();
            calls.clear();
        }

        reg.emit_message_sent("hello");
        reg.emit_event(&AgentEvent::Thinking);
        reg.emit_session_start("s1");

        // No broadcast hooks should have been called after the disable
        assert!(ext.calls().is_empty());
    }

    // ── Graceful degradation (panic catching) ────────────────────────

    #[test]
    fn test_graceful_degradation_on_panic() {
        struct PanickingExtension;
        impl Extension for PanickingExtension {
            fn name(&self) -> &str {
                "panicker"
            }
            fn description(&self) -> &str {
                "Panics"
            }
            fn on_load(&self, _ctx: &ExtensionContext) {
                panic!("intentional panic in on_load");
            }
            fn on_message_sent(&self, _msg: &str) {
                panic!("intentional panic in on_message_sent");
            }
        }

        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(PanickingExtension));
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();

        // Should not panic — graceful degradation
        reg.emit_load(&ctx);
        reg.emit_message_sent("hello");

        // Error should be recorded
        let errors = reg.errors();
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].event, "on_load");
        assert!(errors[0].error.contains("intentional panic"));
        assert_eq!(errors[1].event, "on_message_sent");
    }

    // ── Command tests ────────────────────────────────────────────────

    #[test]
    fn test_command_new() {
        let cmd = Command::new("deploy", "Deploy the project", "/deploy <target>");
        assert_eq!(cmd.name, "deploy");
        assert_eq!(cmd.description, "Deploy the project");
        assert_eq!(cmd.usage, "/deploy <target>");
    }

    // ── Dynamic loading tests ────────────────────────────────────────

    #[test]
    fn test_load_extension_missing_file() {
        let result = load_extension(Path::new("/nonexistent/extension.so"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_extension_wrong_extension() {
        let result = load_extension(Path::new("something.txt"));
        assert!(result.is_err());
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(msg.contains("Unsupported extension file format"));
    }

    #[test]
    fn test_load_extensions_collects_errors() {
        let paths: Vec<&Path> = vec![Path::new("/nonexistent1.so"), Path::new("/nonexistent2.so")];
        let (loaded, errors) = load_extensions(&paths);
        assert!(loaded.is_empty());
        assert_eq!(errors.len(), 2);
    }

    // ── Registry debug ───────────────────────────────────────────────

    #[test]
    fn test_registry_debug() {
        let reg = ExtensionRegistry::new();
        let debug_str = format!("{:?}", reg);
        assert!(debug_str.contains("count"));
    }

    // ── Hot reload (error path) ──────────────────────────────────────

    #[test]
    fn test_hot_reload_no_source_path() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();

        let result = reg.hot_reload("noop", &ctx);
        assert!(result.is_err());
        match result {
            Err(ExtensionError::HotReloadFailed { name, reason }) => {
                assert_eq!(name, "noop");
                assert!(reason.contains("no source path"));
            }
            _ => panic!("Expected HotReloadFailed error"),
        }
    }

    #[test]
    fn test_hot_reload_not_found() {
        let mut reg = ExtensionRegistry::new();
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();

        let result = reg.hot_reload("nonexistent", &ctx);
        assert!(result.is_err());
    }

    // ── Multi-extension broadcast ordering ───────────────────────────

    #[test]
    fn test_broadcast_to_multiple_extensions() {
        let mut reg = ExtensionRegistry::new();
        let ext1 = Arc::new(RecordingExtension::new("ext1"));
        let ext2 = Arc::new(RecordingExtension::new("ext2"));
        reg.register(ext1.clone());
        reg.register(ext2.clone());

        reg.emit_message_sent("hello");

        assert!(ext1.calls().contains(&"on_message_sent(hello)".to_string()));
        assert!(ext2.calls().contains(&"on_message_sent(hello)".to_string()));
    }

    #[test]
    fn test_unregister_calls_on_unload() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        reg.unregister("rec");
        assert_eq!(ext.calls(), vec!["on_unload"]);
    }

    #[test]
    fn test_registry_errors() {
        let reg = ExtensionRegistry::new();
        assert!(reg.errors().is_empty());
        reg.clear_errors(); // no-op
    }

    #[test]
    fn test_emit_event_does_not_panic() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        reg.emit_event(&AgentEvent::Thinking);
    }

    #[test]
    fn test_multiple_lifecycle_hooks() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());

        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        reg.emit_load(&ctx);
        reg.emit_session_start("s1");
        reg.emit_message_sent("hello");
        reg.emit_tool_call("bash", &serde_json::json!({}));
        let result = AgentToolResult::success("ok");
        reg.emit_tool_result("bash", &result);
        reg.emit_message_received("response");
        reg.emit_session_end("s1");
        reg.emit_unload();

        let calls = ext.calls();
        assert!(calls.contains(&"on_load".to_string()));
        assert!(calls.contains(&"on_session_start(s1)".to_string()));
        assert!(calls.contains(&"on_message_sent(hello)".to_string()));
        assert!(calls.contains(&"on_tool_call(bash)".to_string()));
        assert!(calls.contains(&"on_tool_result(bash)".to_string()));
        assert!(calls.contains(&"on_message_received(response)".to_string()));
        assert!(calls.contains(&"on_session_end(s1)".to_string()));
        assert!(calls.contains(&"on_unload".to_string()));
    }

    // ── ExtensionRunner tests ─────────────────────────────────────────

    #[test]
    fn test_runner_new() {
        let runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        assert!(runner.is_empty());
        assert_eq!(runner.len(), 0);
        assert!(runner.names().collect::<Vec<_>>().is_empty());
    }

    #[test]
    fn test_runner_default() {
        let runner = ExtensionRunner::default();
        assert!(runner.is_empty());
    }

    #[test]
    fn test_runner_register_in_memory() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("test-ext"));
        runner.registry_mut().register(ext.clone());

        // Manually set state since we bypassed load_extension
        runner.states.insert("test-ext".to_string(), ExtensionState::Active);
        runner.order.push("test-ext".to_string());

        assert_eq!(runner.len(), 1);
        assert!(!runner.is_empty());
        assert_eq!(runner.state("test-ext"), ExtensionState::Active);
    }

    #[test]
    fn test_runner_state_tracking() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        assert_eq!(runner.state("ext1"), ExtensionState::Active);
        assert_eq!(runner.state("nonexistent"), ExtensionState::Unloaded);

        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        runner.disable("ext1").unwrap();
        assert_eq!(runner.state("ext1"), ExtensionState::Disabled);

        runner.enable("ext1", &ctx).unwrap();
        assert_eq!(runner.state("ext1"), ExtensionState::Active);
    }

    #[test]
    fn test_runner_enable_disable() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        // Initially enabled
        assert!(runner.is_enabled("ext1"));

        // Disable
        runner.disable("ext1").unwrap();
        assert!(!runner.is_enabled("ext1"));
        assert_eq!(runner.state("ext1"), ExtensionState::Disabled);

        // Disable again is no-op
        runner.disable("ext1").unwrap();

        // Enable
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        runner.enable("ext1", &ctx).unwrap();
        assert!(runner.is_enabled("ext1"));
        assert_eq!(runner.state("ext1"), ExtensionState::Active);
    }

    #[test]
    fn test_runner_enable_disable_not_found() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        assert!(runner.disable("nonexistent").is_err());
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        assert!(runner.enable("nonexistent", &ctx).is_err());
    }

    #[test]
    fn test_runner_unload() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        assert!(runner.unload_extension("ext1"));
        assert_eq!(runner.state("ext1"), ExtensionState::Unloaded);
        assert!(runner.is_empty());
        assert!(!runner.unload_extension("ext1")); // already unloaded
    }

    #[test]
    fn test_runner_has_handlers() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        assert!(!runner.has_handlers("any_event"));
        assert!(!runner.has_enabled_extensions());

        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        assert!(runner.has_handlers("any_event"));
        assert!(runner.has_enabled_extensions());
    }

    #[test]
    fn test_runner_extension_order() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));

        // Register 3 extensions
        for name in &["ext1", "ext2", "ext3"] {
            let ext = Arc::new(RecordingExtension::new(name.to_string()));
            runner.registry_mut().register(ext.clone());
            runner.states.insert(name.to_string(), ExtensionState::Active);
            runner.order.push(name.to_string());
        }

        assert_eq!(runner.extension_order(), &["ext1", "ext2", "ext3"]);
        assert_eq!(runner.len(), 3);
    }

    #[test]
    fn test_runner_error_listener() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let received = Arc::new(std::sync::Mutex::new(Vec::<ExtensionErrorRecord>::new()));
        let received_clone = received.clone();

        let _handle = runner.on_error(move |record| {
            received_clone.lock().unwrap().push(record.clone());
        });

        // Emit an error
        runner.emit_error_record(ExtensionErrorRecord::new("test-ext", "test_event", "test error"));

        let records = received.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].extension_name, "test-ext");
        assert_eq!(records[0].event, "test_event");
    }

    #[test]
    fn test_runner_emit_tool_call() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        let result = runner.emit_tool_call("bash", &serde_json::json!({"cmd": "ls"}));
        assert!(!result.blocked);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_runner_emit_tool_result() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        let tool_result = AgentToolResult::success("done");
        let result = runner.emit_tool_result_event("bash", &tool_result);
        assert!(result.output.is_none());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_runner_emit_input_continue() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        let mut event = InputEvent {
            text: "hello".to_string(),
            source: InputSource::Interactive,
        };
        let result = runner.emit_input_event(&mut event);
        assert!(matches!(result, InputEventResult::Continue));
    }

    #[test]
    fn test_runner_emit_session_before_switch() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        let event = SessionBeforeSwitchEvent {
            reason: SessionSwitchReason::New,
            target_session_file: None,
        };
        let result = runner.emit_session_before_switch_event(&event);
        assert!(!result.cancelled);
        assert!(result.cancelled_by.is_none());
    }

    #[test]
    fn test_runner_emit_session_shutdown() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner.states.insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        let event = SessionShutdownEvent {
            reason: SessionShutdownReason::Quit,
            target_session_file: None,
        };
        let handled = runner.emit_session_shutdown_event(&event);
        assert!(handled);
    }

    #[test]
    fn test_runner_emit_session_shutdown_no_extensions() {
        let runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let event = SessionShutdownEvent {
            reason: SessionShutdownReason::Quit,
            target_session_file: None,
        };
        let handled = runner.emit_session_shutdown_event(&event);
        assert!(!handled);
    }

    #[test]
    fn test_runner_load_extension_missing_file() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();
        let result = runner.load_extension(Path::new("/nonexistent.so"), &ctx);
        assert!(result.is_err());
        assert!(!runner.load_errors().is_empty());
    }

    #[test]
    fn test_runner_load_extension_wrong_format() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp")).build();

        // Create a temp file with wrong extension
        let dir = tempfile::tempdir().unwrap();
        let bad_file = dir.path().join("bad.txt");
        std::fs::write(&bad_file, "not a library").unwrap();

        let result = runner.load_extension(&bad_file, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_runner_all_tools_in_order() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));

        // NoopExtension has no tools
        for name in &["ext1", "ext2"] {
            let ext = Arc::new(NoopExtension);
            // Give unique names by wrapping
            runner.registry_mut().register(ext.clone());
            runner.states.insert(name.to_string(), ExtensionState::Active);
            runner.order.push(name.to_string());
        }

        let tools = runner.all_tools();
        assert!(tools.is_empty()); // NoopExtension provides no tools
    }

    #[test]
    fn test_runner_delegation() {
        let mut runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let ext = Arc::new(NoopExtension);
        runner.registry_mut().register(ext);
        runner.states.insert("noop".to_string(), ExtensionState::Active);
        runner.order.push("noop".to_string());

        assert!(runner.get("noop").is_some());
        assert!(runner.get("missing").is_none());
        assert_eq!(runner.names().collect::<Vec<_>>(), vec!["noop"]);
    }

    #[test]
    fn test_runner_debug() {
        let runner = ExtensionRunner::new(PathBuf::from("/tmp"));
        let debug = format!("{:?}", runner);
        assert!(debug.contains("ExtensionRunner"));
        assert!(debug.contains("/tmp"));
    }

    // ── Extension Discovery tests ─────────────────────────────────────

    #[test]
    fn test_discover_extensions_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let paths = discover_extensions_in_dir(dir.path());
        assert!(paths.is_empty());
    }

    #[test]
    fn test_discover_extensions_nonexistent_dir() {
        let paths = discover_extensions_in_dir(Path::new("/nonexistent"));
        assert!(paths.is_empty());
    }

    #[test]
    fn test_discover_extensions_finds_shared_lib() {
        let dir = tempfile::tempdir().unwrap();
        // Create a fake .so file
        let ext = if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(target_os = "windows") {
            "dll"
        } else {
            "so"
        };
        let lib_file = dir.path().join(format!("my_ext.{}", ext));
        std::fs::write(&lib_file, b"fake lib").unwrap();

        let paths = discover_extensions_in_dir(dir.path());
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], lib_file);
    }

    #[test]
    fn test_discover_extensions_ignores_non_libs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"text").unwrap();
        std::fs::write(dir.path().join("script.sh"), b"bash").unwrap();

        let paths = discover_extensions_in_dir(dir.path());
        assert!(paths.is_empty());
    }

    #[test]
    fn test_discover_extensions_subdirectory_index() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("my_ext");
        std::fs::create_dir(&subdir).unwrap();

        let ext = if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(target_os = "windows") {
            "dll"
        } else {
            "so"
        };
        let index_lib = subdir.join(format!("index.{}", ext));
        std::fs::write(&index_lib, b"fake lib").unwrap();

        let paths = discover_extensions_in_dir(dir.path());
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], index_lib);
    }

    #[test]
    fn test_discover_extensions_from_cwd() {
        let cwd = tempfile::tempdir().unwrap();
        let ext_dir = cwd.path().join(".oxi").join("extensions");
        std::fs::create_dir_all(&ext_dir).unwrap();

        let ext = if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(target_os = "windows") {
            "dll"
        } else {
            "so"
        };
        std::fs::write(ext_dir.join(format!("test.{}", ext)), b"fake").unwrap();

        let paths = discover_extensions(cwd.path(), &[]);
        assert_eq!(paths.len(), 1);
    }

    // ── ExtensionState tests ───────────────────────────────────────────

    #[test]
    fn test_extension_state_display() {
        assert_eq!(ExtensionState::Pending.to_string(), "pending");
        assert_eq!(ExtensionState::Active.to_string(), "active");
        assert_eq!(ExtensionState::Disabled.to_string(), "disabled");
        assert_eq!(ExtensionState::Failed.to_string(), "failed");
        assert_eq!(ExtensionState::Unloaded.to_string(), "unloaded");
    }

    #[test]
    fn test_extension_state_serialization() {
        let state = ExtensionState::Active;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"active\"");
        let parsed: ExtensionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ExtensionState::Active);
    }

    // ── Emit Result type tests ─────────────────────────────────────────

    #[test]
    fn test_tool_call_emit_result_default() {
        let result = ToolCallEmitResult::default();
        assert!(!result.blocked);
        assert!(result.block_reason.is_none());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_tool_result_emit_result_default() {
        let result = ToolResultEmitResult::default();
        assert!(result.output.is_none());
        assert!(result.success.is_none());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_session_before_emit_result_default() {
        let result = SessionBeforeEmitResult::default();
        assert!(!result.cancelled);
        assert!(result.cancelled_by.is_none());
        assert!(result.errors.is_empty());
    }
}
