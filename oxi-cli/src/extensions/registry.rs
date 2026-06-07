//! Extension registry and runner.
//!
//! Manages in-process extensions: registration, lifecycle hooks, and event dispatch.
//! Extensions implement the `Extension` trait and are registered via
//! `ExtensionRegistry::register()` at compile time or runtime.

#![allow(unused)]

use crate::extensions::types::{
    AgentEvent, AgentToolResult, BashEvent, BeforeProviderRequestEvent, Command, ContextEmitResult,
    ContextEvent, ExtensionError, ExtensionErrorListener, ExtensionErrorRecord, ExtensionManifest,
    ExtensionState, InputEvent, InputEventResult, ModelSelectEvent, ProviderRequestEmitResult,
    SessionBeforeCompactEvent, SessionBeforeEmitResult, SessionBeforeForkEvent,
    SessionBeforeSwitchEvent, SessionBeforeTreeEvent, SessionCompactEvent, SessionShutdownEvent,
    SessionTreeEvent, ThinkingLevelSelectEvent, ToolCallEmitResult, ToolResultEmitResult,
};

use crate::CompactionContext;
use crate::extensions::Extension;
use crate::extensions::context::ExtensionContext;
use crate::store::settings::Settings;

use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

type ToolType = dyn oxi_agent::AgentTool;
type ToolArc = Arc<ToolType>;

// ═══════════════════════════════════════════════════════════════════════════
// Extension Registry
// ═══════════════════════════════════════════════════════════════════════════

struct LoadedExtension {
    extension: Arc<dyn Extension>,
    enabled: bool,
}

/// pub.
pub struct ExtensionRegistry {
    entries: HashMap<String, LoadedExtension>,
    errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>,
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRegistry {
    /// Create a new empty extension registry.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            errors: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register an extension.
    pub fn register(&mut self, ext: Arc<dyn Extension>) {
        let name = ext.name().to_string();
        tracing::info!(name = %name, "extension registered");
        self.entries.insert(
            name,
            LoadedExtension {
                extension: ext,
                enabled: true,
            },
        );
    }

    /// Unregister an extension by name.
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

    /// Disable an extension (keeps it registered but inactive).
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

    /// Check if an extension is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.entries.get(name).map(|e| e.enabled).unwrap_or(false)
    }

    /// Collect all tools from all enabled extensions.
    pub fn all_tools(&self) -> Vec<ToolArc> {
        self.entries
            .values()
            .filter(|e| e.enabled)
            .flat_map(|e| e.extension.register_tools())
            .collect()
    }

    /// Collect all commands from all enabled extensions.
    pub fn all_commands(&self) -> Vec<Command> {
        self.entries
            .values()
            .filter(|e| e.enabled)
            .flat_map(|e| e.extension.register_commands())
            .collect()
    }

    /// Emit on_load to all enabled extensions.
    pub fn emit_load(&self, ctx: &ExtensionContext) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_load", || {
                entry.extension.on_load(ctx);
            });
        }
    }

    /// Emit on_unload to all enabled extensions.
    pub fn emit_unload(&self) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_unload", || {
                entry.extension.on_unload();
            });
        }
    }

    /// Emit on_message_sent to all enabled extensions.
    pub fn emit_message_sent(&self, msg: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_message_sent", || {
                entry.extension.on_message_sent(msg);
            });
        }
    }

    /// Emit on_message_received to all enabled extensions.
    pub fn emit_message_received(&self, msg: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_message_received", || {
                entry.extension.on_message_received(msg);
            });
        }
    }

    /// Emit on_tool_call to all enabled extensions.
    pub fn emit_tool_call(&self, tool: &str, params: &Value) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_tool_call", || {
                entry.extension.on_tool_call(tool, params);
            });
        }
    }

    /// Emit on_tool_result to all enabled extensions.
    pub fn emit_tool_result(&self, tool: &str, result: &AgentToolResult) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_tool_result", || {
                entry.extension.on_tool_result(tool, result);
            });
        }
    }

    /// Emit on_session_start to all enabled extensions.
    pub fn emit_session_start(&self, session_id: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_session_start", || {
                entry.extension.on_session_start(session_id);
            });
        }
    }

    /// Emit on_session_end to all enabled extensions.
    pub fn emit_session_end(&self, session_id: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_session_end", || {
                entry.extension.on_session_end(session_id);
            });
        }
    }

    /// Emit on_settings_changed to all enabled extensions.
    pub fn emit_settings_changed(&self, settings: &Settings) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_settings_changed", || {
                entry.extension.on_settings_changed(settings);
            });
        }
    }

    /// Emit on_event to all enabled extensions.
    pub fn emit_event(&self, event: &AgentEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_event", || {
                entry.extension.on_event(event);
            });
        }
    }

    /// Emit on_error to all enabled extensions.
    pub fn emit_error(&self, error: &anyhow::Error) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            if let Err(e) = entry.extension.on_error(error) {
                tracing::warn!(extension = name, error = %e, "on_error hook failed");
                errors.push((name.to_string(), e));
            }
        }
        errors
    }

    /// Emit session_shutdown to all enabled extensions.
    pub fn emit_session_shutdown(&self, event: &SessionShutdownEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "session_shutdown", || {
                entry.extension.session_shutdown(event);
            });
        }
    }

    /// Get an extension by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Extension>> {
        self.entries.get(name).map(|e| Arc::clone(&e.extension))
    }

    /// Get all extension names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    /// Iterate over all extensions.
    pub fn extensions(&self) -> impl Iterator<Item = &Arc<dyn Extension>> {
        self.entries.values().map(|e| &e.extension)
    }

    /// Get manifest for an extension.
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
    /// Clear recorded errors.
    pub fn clear_errors(&self) {
        self.errors.write().clear();
    }

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
            tracing::error!(extension = ext_name, hook = hook, error = %msg, "Extension hook panicked");
            self.errors.write().push(ExtensionErrorRecord::new(
                ext_name,
                hook,
                format!("panic: {}", msg),
            ));
        }
    }
}

impl fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRegistry")
            .field("count", &self.entries.len())
            .finish()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Runner
// ═══════════════════════════════════════════════════════════════════════════

/// Orchestrates extension lifecycle and event dispatch.
pub struct ExtensionRunner {
    registry: ExtensionRegistry,
    states: HashMap<String, ExtensionState>,
    order: Vec<String>,
    error_listeners: Vec<Arc<ExtensionErrorListener>>,
    cwd: PathBuf,
}

impl Default for ExtensionRunner {
    fn default() -> Self {
        Self::new(PathBuf::from("."))
    }
}

impl ExtensionRunner {
    /// Create a new extension runner for the given working directory.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            registry: ExtensionRegistry::new(),
            states: HashMap::new(),
            order: Vec::new(),
            error_listeners: Vec::new(),
            cwd,
        }
    }

    /// Register an error listener and return a handle for unregistering.
    pub fn on_error<F>(&mut self, listener: F) -> ExtensionErrorHandle
    where
        F: Fn(&ExtensionErrorRecord) + Send + Sync + 'static,
    {
        let arc: Arc<ExtensionErrorListener> = Arc::new(listener);
        self.error_listeners.push(Arc::clone(&arc));
        ExtensionErrorHandle {
            listener: Some(arc),
        }
    }

    /// Emit an error record to all listeners.
    pub fn emit_error_record(&self, record: ExtensionErrorRecord) {
        for listener in &self.error_listeners {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener(&record);
            }));
        }
        self.registry.errors.write().push(record);
    }

    /// Register an extension into the runner.
    pub fn register(&mut self, ext: Arc<dyn Extension>, ctx: &ExtensionContext) {
        let name = ext.name().to_string();
        self.registry.register(ext);
        self.set_state(&name, ExtensionState::Active);
        self.registry.call_hook_safe(&name, "on_load", || {
            if let Some(e) = self.registry.get(&name) {
                e.on_load(ctx);
            }
        });
    }

    /// Unregister an extension by name.
    pub fn unload_extension(&mut self, name: &str) -> bool {
        let had = self.registry.unregister(name);
        if had {
            self.set_state(name, ExtensionState::Unloaded);
            tracing::info!(name = %name, "extension unloaded");
        }
        had
    }

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
    /// Get the extension execution order.
    pub fn extension_order(&self) -> &[String] {
        &self.order
    }

    /// Disable an extension.
    pub fn disable(&mut self, name: &str) -> Result<(), ExtensionError> {
        self.registry.disable(name)?;
        self.set_state(name, ExtensionState::Disabled);
        Ok(())
    }
    /// Enable an extension.
    pub fn enable(&mut self, name: &str, ctx: &ExtensionContext) -> Result<(), ExtensionError> {
        self.registry.enable(name, ctx)?;
        self.set_state(name, ExtensionState::Active);
        Ok(())
    }
    /// Check if an extension is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.registry.is_enabled(name)
    }

    /// Whether any handlers exist for an event type.
    pub fn has_handlers(&self, _event_type: &str) -> bool {
        self.has_enabled_extensions()
    }
    /// Whether any extensions are enabled.
    pub fn has_enabled_extensions(&self) -> bool {
        self.registry.extensions().any(|_| true)
            && self
                .order
                .iter()
                .any(|name| self.state(name) == ExtensionState::Active)
    }

    /// Collect all tools from enabled extensions.
    pub fn all_tools(&self) -> Vec<ToolArc> {
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

    /// Collect all commands from enabled extensions.
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

    /// Wrap a tool with extension error handling.
    pub fn wrap_tool(&self, tool: ToolArc) -> ToolArc {
        Arc::new(WrappedTool {
            inner: tool,
            runner_state: Arc::new(RwLock::new(RunnerState {
                errors: self.registry.errors.clone(),
                error_listeners: self.error_listeners.clone(),
            })),
        })
    }

    /// Wrap multiple tools with extension error handling.
    pub fn wrap_tools(&self, tools: Vec<ToolArc>) -> Vec<ToolArc> {
        tools.into_iter().map(|t| self.wrap_tool(t)).collect()
    }

    /// Emit before_tool_call and on_tool_call events.
    pub fn emit_tool_call(&self, tool_name: &str, params: &Value) -> ToolCallEmitResult {
        let mut result = ToolCallEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                if let Err(e) = ext.on_before_tool_call(tool_name, params) {
                    let err_str = e.to_string();
                    result.errors.push((name.clone(), err_str.clone()));
                    self.emit_error_record(ExtensionErrorRecord::new(
                        name,
                        "on_before_tool_call",
                        &err_str,
                    ));
                }
                self.registry.call_hook_safe(name, "on_tool_call", || {
                    ext.on_tool_call(tool_name, params);
                });
            }
        }
        result
    }

    /// Emit after_tool_call and on_tool_result events.
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
                if let Err(e) = ext.on_after_tool_call(tool_name, tool_result) {
                    result.errors.push((name.clone(), e.to_string()));
                }
                self.registry.call_hook_safe(name, "on_tool_result", || {
                    ext.on_tool_result(tool_name, tool_result);
                });
            }
        }
        result
    }

    /// Emit input event through extensions.
    pub fn emit_input_event(&self, event: &mut InputEvent) -> InputEventResult {
        let mut final_result = InputEventResult::Continue;
        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name) {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ext.input(event)));
                match result {
                    Ok(InputEventResult::Handled) => return InputEventResult::Handled,
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
                        self.emit_error_record(ExtensionErrorRecord::new(
                            name,
                            "input",
                            format!("panic: {}", msg),
                        ));
                    }
                }
            }
        }
        final_result
    }

    /// Emit context event (message modification).
    pub fn emit_context_event(&self, messages: Vec<oxi_sdk::Message>) -> ContextEmitResult {
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
                if let Err(e) = ext.context(&mut event) {
                    errors.push((name.clone(), e.to_string()));
                } else if event.messages.len() != prev_len {
                    current_messages = event.messages;
                    modified = true;
                }
            }
        }
        ContextEmitResult {
            modified,
            messages: current_messages,
            errors,
        }
    }

    /// Emit before_provider_request event.
    pub fn emit_before_provider_request_event(&self, payload: Value) -> ProviderRequestEmitResult {
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
                if let Err(e) = ext.before_provider_request(&mut event) {
                    errors.push((name.clone(), e.to_string()));
                } else if event.payload != current_payload {
                    current_payload = event.payload;
                    modified = true;
                }
            }
        }
        ProviderRequestEmitResult {
            modified,
            payload: current_payload,
            errors,
        }
    }

    /// Emit session_before_switch event.
    pub fn emit_session_before_switch_event(
        &self,
        event: &SessionBeforeSwitchEvent,
    ) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name)
                && let Err(e) = ext.session_before_switch(event)
            {
                result.cancelled = true;
                result.cancelled_by = Some(name.clone());
                result.errors.push((name.clone(), e.to_string()));
                return result;
            }
        }
        result
    }

    /// Emit session_before_fork event.
    pub fn emit_session_before_fork_event(
        &self,
        event: &SessionBeforeForkEvent,
    ) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name)
                && let Err(e) = ext.session_before_fork(event)
            {
                result.cancelled = true;
                result.cancelled_by = Some(name.clone());
                result.errors.push((name.clone(), e.to_string()));
                return result;
            }
        }
        result
    }

    /// Emit session_before_compact event.
    pub fn emit_session_before_compact_event(
        &self,
        event: &SessionBeforeCompactEvent,
    ) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name)
                && let Err(e) = ext.session_before_compact(event)
            {
                result.cancelled = true;
                result.cancelled_by = Some(name.clone());
                result.errors.push((name.clone(), e.to_string()));
                return result;
            }
        }
        result
    }

    /// Emit session_before_tree event.
    pub fn emit_session_before_tree_event(
        &self,
        event: &SessionBeforeTreeEvent,
    ) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active {
                continue;
            }
            if let Some(ext) = self.registry.get(name)
                && let Err(e) = ext.session_before_tree(event)
            {
                result.cancelled = true;
                result.cancelled_by = Some(name.clone());
                result.errors.push((name.clone(), e.to_string()));
                return result;
            }
        }
        result
    }

    /// Emit session_shutdown event.
    pub fn emit_session_shutdown_event(&self, event: &SessionShutdownEvent) -> bool {
        if !self.has_enabled_extensions() {
            return false;
        }
        self.registry.emit_session_shutdown(event);
        true
    }

    /// Emit a generic agent event.
    pub fn emit_event(&self, event: &AgentEvent) {
        self.registry.emit_event(event);
    }

    /// Access the underlying registry.
    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }
    /// Access the underlying registry mutably.
    pub fn registry_mut(&mut self) -> &mut ExtensionRegistry {
        &mut self.registry
    }
    /// Get an extension by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Extension>> {
        self.registry.get(name)
    }
    /// Get all extension names in execution order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(|s| s.as_str())
    }
    /// Number of extensions.
    pub fn len(&self) -> usize {
        self.order.len()
    }
    /// Whether any extensions exist.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    /// Get all recorded errors.
    pub fn errors(&self) -> Vec<ExtensionErrorRecord> {
        self.registry.errors()
    }
    /// Clear recorded errors.
    pub fn clear_errors(&self) {
        self.registry.clear_errors();
    }
}

impl fmt::Debug for ExtensionRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRunner")
            .field("cwd", &self.cwd)
            .field("extensions", &self.order)
            .finish()
    }
}

/// Handle returned by [`ExtensionRunner::on_error`] for unregistering a listener.
pub struct ExtensionErrorHandle {
    listener: Option<Arc<ExtensionErrorListener>>,
}
impl ExtensionErrorHandle {
    /// Remove and return the listener, preventing further error notifications.
    pub fn unregister(&mut self) -> Option<Arc<ExtensionErrorListener>> {
        self.listener.take()
    }
}
impl Drop for ExtensionErrorHandle {
    fn drop(&mut self) {}
}

struct RunnerState {
    errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>,
    error_listeners: Vec<Arc<ExtensionErrorListener>>,
}
struct WrappedTool {
    inner: ToolArc,
    runner_state: Arc<RwLock<RunnerState>>,
}

#[async_trait::async_trait]
impl oxi_agent::AgentTool for WrappedTool {
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
        ctx: &oxi_agent::ToolContext,
    ) -> Result<AgentToolResult, String> {
        self.inner.execute(tool_call_id, params, signal, ctx).await
    }
}
