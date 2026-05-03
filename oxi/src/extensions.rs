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
    pub fn new(extension_name: impl Into<String>, event: impl Into<String>, error: impl Into<String>) -> Self {
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
    pub fn tool_registrar(mut self, registrar: Arc<dyn Fn(Arc<dyn AgentTool>) + Send + Sync>) -> Self {
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

    /// Build the context, falling back to no-op callbacks where necessary.
    pub fn build(self) -> ExtensionContext {
        ExtensionContext {
            cwd: self.cwd,
            settings: self.settings.unwrap_or_else(|| {
                Arc::new(RwLock::new(crate::settings::Settings::default()))
            }),
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
        ExtensionManifest::new(self.name(), "0.0.0")
            .with_description(self.description())
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
        self.entries
            .get(name)
            .map(|e| e.enabled)
            .unwrap_or(false)
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
        let new_ext = load_extension(&source_path).map_err(|e| ExtensionError::HotReloadFailed {
            name: name.to_string(),
            reason: e.to_string(),
        })?;

        let library = unsafe {
            Library::new(&source_path)
                .map_err(|e| ExtensionError::HotReloadFailed {
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

    // ── Querying ─────────────────────────────────────────────────────

    /// Get a reference to an extension by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Extension>> {
        self.entries
            .get(name)
            .map(|e| Arc::clone(&e.extension))
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
            .field(
                "names",
                &self.entries.keys().cloned().collect::<Vec<_>>(),
            )
            .finish()
    }
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
        let manifest = ExtensionManifest::new("test", "0.1.0")
            .with_permission(ExtensionPermission::Network);

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
        let ctx = ExtensionContextBuilder::new(PathBuf::from("/tmp"))
            .build();
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
        assert_eq!(
            ctx.config_get("key"),
            Some(serde_json::json!("value"))
        );
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
            fn name(&self) -> &str { "panicker" }
            fn description(&self) -> &str { "Panics" }
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
}
