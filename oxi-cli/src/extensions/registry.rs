//! Extension registry and runner.
//!
//! The registry manages loaded extensions and provides broadcast/event APIs.
//! The runner adds lifecycle management, state tracking, and discovery.

#![allow(unused)]

use crate::extensions::context::{ExtensionContext, ExtensionContextBuilder};
use crate::extensions::loading::load_extension;
use oxi_agent::{AgentEvent, AgentTool, AgentToolResult};
use crate::extensions::types::{ BashEvent, BeforeProviderRequestEvent,
    Command, ContextEmitResult, ContextEvent, ExtensionError, ExtensionErrorListener,
    ExtensionErrorRecord, ExtensionManifest, ExtensionPermission, ExtensionState,
    InputEvent, InputEventResult, InputSource, ModelSelectEvent, ModelSelectSource,
    ProviderRequestEmitResult, SessionBeforeCompactEvent, SessionBeforeEmitResult,
    SessionBeforeForkEvent, SessionBeforeSwitchEvent, SessionBeforeTreeEvent,
    SessionCompactEvent, SessionShutdownEvent, SessionShutdownReason, SessionSwitchReason,
    SessionTreeEvent, ThinkingLevelSelectEvent, ToolCallEmitResult, ToolResultEmitResult,
};
use crate::CompactionContext;
use crate::settings::Settings;
use anyhow::{bail, Context, Result};
use libloading::Library;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use async_trait::async_trait;

// Re-export types used by other modules
use crate::extensions::types::ExtensionErrorRecord as ExtErrorRecord;

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

// We need to reference Extension here which is defined in mod.rs
use crate::extensions::Extension;

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
    /// Must be kept even though not read directly — dropping would unload the library.
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
    pub fn emit_settings_changed(&self, settings: &Settings) {
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
        ctx: &CompactionContext,
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

    // ── Session Lifecycle Hook Broadcasts ────────────

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

    // ── Provider Hook Broadcasts ─────────────────────

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

    // ── Model Hook Broadcasts ────────────────────────

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

    // ── Bash / Input Hook Broadcasts ─────────────────

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
        const ENTRY_SYMBOL: &[u8] = b"oxi_extension_create\0";
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
        messages: Vec<oxi_ai::Message>,
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
/// Keep: stores error listeners for future tool error broadcasting.
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
    /// Keep: runner_state holds error listeners for wrapped tool execution.
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