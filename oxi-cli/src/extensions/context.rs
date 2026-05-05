//! Extension context and builder.
//!
//! The [`ExtensionContext`] provides extensions with access to host
//! application state, tool registration, and messaging capabilities.
//!
//! Use [`ExtensionContextBuilder`] to construct contexts with the
//! required callbacks.

use crate::settings::Settings;
use crate::extensions::types::{ExtensionErrorRecord, AgentTool};
use crate::CompactionContext;
use anyhow::{bail, Result};
use parking_lot::RwLock;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

// Re-export from types
pub use crate::extensions::types::ExtensionErrorRecord;

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
    settings: Arc<RwLock<Settings>>,
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

impl std::fmt::Debug for ExtensionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        settings: Arc<RwLock<Settings>>,
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
    pub fn settings(&self) -> Settings {
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

// ═══════════════════════════════════════════════════════════════════════════
// Extension Context Builder
// ═══════════════════════════════════════════════════════════════════════════

/// Builder for [`ExtensionContext`].
pub struct ExtensionContextBuilder {
    cwd: PathBuf,
    settings: Option<Arc<RwLock<Settings>>>,
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
    pub fn settings(mut self, settings: Arc<RwLock<Settings>>) -> Self {
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
                .unwrap_or_else(|| Arc::new(RwLock::new(Settings::default()))),
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