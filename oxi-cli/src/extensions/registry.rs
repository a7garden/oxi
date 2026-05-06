//! Extension registry and runner.

#![allow(unused)]

use crate::extensions::types::{
    AgentEvent, AgentToolResult, BashEvent, BeforeProviderRequestEvent,
    Command, ContextEmitResult, ContextEvent, ExtensionError, ExtensionErrorListener,
    ExtensionErrorRecord, ExtensionManifest, ExtensionState,
    InputEvent, InputEventResult, ModelSelectEvent,
    ProviderRequestEmitResult, SessionBeforeCompactEvent, SessionBeforeEmitResult,
    SessionBeforeForkEvent, SessionBeforeSwitchEvent, SessionBeforeTreeEvent,
    SessionCompactEvent, SessionShutdownEvent, SessionTreeEvent, ThinkingLevelSelectEvent,
    ToolCallEmitResult, ToolResultEmitResult,
};

use crate::extensions::context::ExtensionContext;
use crate::extensions::Extension;
use crate::CompactionContext;
use crate::settings::Settings;

use anyhow::{bail, Context, Result};
use libloading::Library;
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use async_trait::async_trait;

type ToolType = dyn oxi_agent::AgentTool;
type ToolArc = Arc<ToolType>;

// ═══════════════════════════════════════════════════════════════════════════
// Extension Registry
// ═══════════════════════════════════════════════════════════════════════════

struct LoadedExtension {
    extension: Arc<dyn Extension>,
    enabled: bool,
    source_path: Option<PathBuf>,
}

/// pub.
pub struct ExtensionRegistry {
    entries: HashMap<String, LoadedExtension>,
    errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>,
    #[allow(dead_code)]
    libraries: Vec<Library>,
}

impl Default for ExtensionRegistry {
    fn default() -> Self { Self::new() }
}

impl ExtensionRegistry {
    /// Create a new empty extension registry.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            errors: Arc::new(RwLock::new(Vec::new())),
            libraries: Vec::new(),
        }
    }

/// TODO: document.
    pub fn register(&mut self, ext: Arc<dyn Extension>) {
        let name = ext.name().to_string();
        tracing::info!(name = %name, "extension registered");
        self.entries.insert(name, LoadedExtension { extension: ext, enabled: true, source_path: None });
    }

/// TODO: document.
    pub fn register_with_library(&mut self, ext: Arc<dyn Extension>, source_path: PathBuf, library: Library) {
        let name = ext.name().to_string();
        tracing::info!(name = %name, path = %source_path.display(), "extension registered (dynamic)");
        self.libraries.push(library);
        self.entries.insert(name, LoadedExtension { extension: ext, enabled: true, source_path: Some(source_path) });
    }

/// TODO: document.
    pub fn unregister(&mut self, name: &str) -> bool {
        if let Some(entry) = self.entries.remove(name) {
            self.call_hook_safe(name, "on_unload", || { entry.extension.on_unload(); });
            tracing::info!(name = %name, "extension unregistered");
            true
        } else {
            false
        }
    }

/// TODO: document.
    pub fn disable(&mut self, name: &str) -> Result<(), ExtensionError> {
        let ext = {
            let entry = self.entries.get_mut(name).ok_or_else(|| ExtensionError::NotFound { name: name.to_string() })?;
            if !entry.enabled { return Ok(()); }
            entry.enabled = false;
            Arc::clone(&entry.extension)
        };
        self.call_hook_safe(name, "on_unload", || { ext.on_unload(); });
        tracing::info!(name = %name, "extension disabled");
        Ok(())
    }

/// TODO: document.
    pub fn enable(&mut self, name: &str, ctx: &ExtensionContext) -> Result<(), ExtensionError> {
        let ext = {
            let entry = self.entries.get_mut(name).ok_or_else(|| ExtensionError::NotFound { name: name.to_string() })?;
            if entry.enabled { return Ok(()); }
            entry.enabled = true;
            Arc::clone(&entry.extension)
        };
        self.call_hook_safe(name, "on_load", || { ext.on_load(ctx); });
        tracing::info!(name = %name, "extension enabled");
        Ok(())
    }

/// TODO: document.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.entries.get(name).map(|e| e.enabled).unwrap_or(false)
    }

/// TODO: document.
    pub fn hot_reload(&mut self, name: &str, ctx: &ExtensionContext) -> Result<(), ExtensionError> {
        let source_path = {
            let entry = self.entries.get(name).ok_or_else(|| ExtensionError::NotFound { name: name.to_string() })?;
            entry.source_path.clone()
        }.ok_or_else(|| ExtensionError::HotReloadFailed { name: name.to_string(), reason: "no source path recorded".to_string() })?;

        self.unregister(name);
        let new_ext = crate::extensions::loading::load_extension(&source_path).map_err(|e| ExtensionError::HotReloadFailed { name: name.to_string(), reason: e.to_string() })?;
        let library = unsafe { Library::new(&source_path).map_err(|e| ExtensionError::HotReloadFailed { name: name.to_string(), reason: format!("Failed to re-open library: {}", e) })? };
        self.call_hook_safe(name, "on_load", || { new_ext.on_load(ctx); });
        self.register_with_library(new_ext, source_path, library);
        tracing::info!(name = %name, "extension hot-reloaded");
        Ok(())
    }

/// TODO: document.
    pub fn all_tools(&self) -> Vec<ToolArc> {
        self.entries.values().filter(|e| e.enabled).flat_map(|e| e.extension.register_tools()).collect()
    }

/// TODO: document.
    pub fn all_commands(&self) -> Vec<Command> {
        self.entries.values().filter(|e| e.enabled).flat_map(|e| e.extension.register_commands()).collect()
    }

/// TODO: document.
    pub fn emit_load(&self, ctx: &ExtensionContext) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_load", || { entry.extension.on_load(ctx); });
        }
    }

/// TODO: document.
    pub fn emit_unload(&self) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_unload", || { entry.extension.on_unload(); });
        }
    }

/// TODO: document.
    pub fn emit_message_sent(&self, msg: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_message_sent", || { entry.extension.on_message_sent(msg); });
        }
    }

/// TODO: document.
    pub fn emit_message_received(&self, msg: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_message_received", || { entry.extension.on_message_received(msg); });
        }
    }

/// TODO: document.
    pub fn emit_tool_call(&self, tool: &str, params: &Value) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_tool_call", || { entry.extension.on_tool_call(tool, params); });
        }
    }

/// TODO: document.
    pub fn emit_tool_result(&self, tool: &str, result: &AgentToolResult) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_tool_result", || { entry.extension.on_tool_result(tool, result); });
        }
    }

/// TODO: document.
    pub fn emit_session_start(&self, session_id: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_session_start", || { entry.extension.on_session_start(session_id); });
        }
    }

/// TODO: document.
    pub fn emit_session_end(&self, session_id: &str) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_session_end", || { entry.extension.on_session_end(session_id); });
        }
    }

/// TODO: document.
    pub fn emit_settings_changed(&self, settings: &Settings) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_settings_changed", || { entry.extension.on_settings_changed(settings); });
        }
    }

/// TODO: document.
    pub fn emit_event(&self, event: &AgentEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "on_event", || { entry.extension.on_event(event); });
        }
    }

/// TODO: document.
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

/// TODO: document.
    pub fn emit_session_shutdown(&self, event: &SessionShutdownEvent) {
        for entry in self.entries.values().filter(|e| e.enabled) {
            let name = entry.extension.name();
            self.call_hook_safe(name, "session_shutdown", || { entry.extension.session_shutdown(event); });
        }
    }

/// TODO: document.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Extension>> {
        self.entries.get(name).map(|e| Arc::clone(&e.extension))
    }

/// TODO: document.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

/// TODO: document.
    pub fn extensions(&self) -> impl Iterator<Item = &Arc<dyn Extension>> {
        self.entries.values().map(|e| &e.extension)
    }

/// TODO: document.
    pub fn manifest(&self, name: &str) -> Option<ExtensionManifest> {
        self.entries.get(name).map(|e| e.extension.manifest())
    }

/// TODO: document.
    pub fn len(&self) -> usize { self.entries.len() }
/// TODO: document.
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
/// TODO: document.
    pub fn errors(&self) -> Vec<ExtensionErrorRecord> { self.errors.read().clone() }
/// TODO: document.
    pub fn clear_errors(&self) { self.errors.write().clear(); }

    fn call_hook_safe<F>(&self, ext_name: &str, hook: &str, f: F) where F: FnOnce() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if let Err(payload) = result {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() { s.to_string() }
                else if let Some(s) = payload.downcast_ref::<String>() { s.clone() }
                else { "unknown panic".to_string() };
            tracing::error!(extension = ext_name, hook = hook, error = %msg, "Extension hook panicked");
            self.errors.write().push(ExtensionErrorRecord::new(ext_name, hook, &format!("panic: {}", msg)));
        }
    }
}

impl fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRegistry").field("count", &self.entries.len()).finish()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Runner
// ═══════════════════════════════════════════════════════════════════════════

/// pub.
pub struct ExtensionRunner {
    registry: ExtensionRegistry,
    states: HashMap<String, ExtensionState>,
    order: Vec<String>,
    error_listeners: Vec<Arc<ExtensionErrorListener>>,
    cwd: PathBuf,
    load_errors: Vec<(PathBuf, String)>,
}

impl Default for ExtensionRunner {
    fn default() -> Self { Self::new(PathBuf::from(".")) }
}

impl ExtensionRunner {
    /// Create a new extension runner for the given working directory.
    pub fn new(cwd: PathBuf) -> Self {
        Self { registry: ExtensionRegistry::new(), states: HashMap::new(), order: Vec::new(), error_listeners: Vec::new(), cwd, load_errors: Vec::new() }
    }

    /// Register an error listener and return a handle for unregistering.
    pub fn on_error<F>(&mut self, listener: F) -> ExtensionErrorHandle where F: Fn(&ExtensionErrorRecord) + Send + Sync + 'static {
        let arc: Arc<ExtensionErrorListener> = Arc::new(listener);
        self.error_listeners.push(Arc::clone(&arc));
        ExtensionErrorHandle { listener: Some(arc) }
    }

/// TODO: document.
    pub fn emit_error_record(&self, record: ExtensionErrorRecord) {
        for listener in &self.error_listeners {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { listener(&record); }));
        }
        self.registry.errors.write().push(record);
    }

/// TODO: document.
    pub fn load_extension(&mut self, path: &Path, ctx: &ExtensionContext) -> Result<(), ExtensionError> {
        let path_display = path.display().to_string();

        // Pre-load validation (existence, size, extension, checksum)
        let validated = crate::extensions::loading::validate_extension(path)?;
        tracing::debug!(
            path = %path_display,
            checksum = %validated.checksum,
            "extension binary validated"
        );

        let ext_os = path.extension().and_then(OsStr::to_str).unwrap_or("");
        if !matches!(ext_os, "so" | "dylib" | "dll") {
            return Err(ExtensionError::LoadFailed { name: path_display, reason: format!("Unsupported extension file format: .{}", ext_os) });
        }

        let library = unsafe { Library::new(path).map_err(|e| { let r = format!("Failed to load library: {}", e); self.load_errors.push((path.to_path_buf(), r.clone())); ExtensionError::LoadFailed { name: path_display.clone(), reason: r } })? };

        const ENTRY_SYMBOL: &[u8] = b"oxi_extension_create\0";
        type CreateFn = unsafe fn() -> *mut dyn Extension;
        let create: libloading::Symbol<CreateFn> = unsafe { library.get(ENTRY_SYMBOL).map_err(|e| { let r = format!("Symbol not found: {}", e); self.load_errors.push((path.to_path_buf(), r.clone())); ExtensionError::LoadFailed { name: path_display.clone(), reason: r } })? };

        let raw_ptr = unsafe { create() };
        if raw_ptr.is_null() {
            let r = "oxi_extension_create returned null".to_string();
            self.load_errors.push((path.to_path_buf(), r.clone()));
            return Err(ExtensionError::LoadFailed { name: path_display, reason: r });
        }

        let boxed: Box<dyn Extension> = unsafe { Box::from_raw(raw_ptr) };
        let ext: Arc<dyn Extension> = Arc::from(boxed);
        let name = ext.name().to_string();

        self.registry.register_with_library(ext, path.to_path_buf(), library);
        self.set_state(&name, ExtensionState::Active);
        self.registry.call_hook_safe(&name, "on_load", || { if let Some(e) = self.registry.get(&name) { e.on_load(ctx); } });

        tracing::info!(name = %name, path = %path_display, "extension loaded");
        Ok(())
    }

/// TODO: document.
    pub fn load_extensions_from_paths(&mut self, paths: &[PathBuf], ctx: &ExtensionContext) -> Vec<anyhow::Error> {
        let mut errors = Vec::new();
        for path in paths { if let Err(e) = self.load_extension(path, ctx) { errors.push(anyhow::anyhow!("{}", e)); } }
        errors
    }

/// TODO: document.
    pub fn unload_extension(&mut self, name: &str) -> bool {
        let had = self.registry.unregister(name);
        if had { self.set_state(name, ExtensionState::Unloaded); tracing::info!(name = %name, "extension unloaded"); }
        had
    }

    fn set_state(&mut self, name: &str, state: ExtensionState) {
        self.states.insert(name.to_string(), state);
        if state == ExtensionState::Active && !self.order.contains(&name.to_string()) { self.order.push(name.to_string()); }
        if state == ExtensionState::Unloaded { self.order.retain(|n| n != name); }
    }

/// TODO: document.
    pub fn state(&self, name: &str) -> ExtensionState { self.states.get(name).copied().unwrap_or(ExtensionState::Unloaded) }
/// TODO: document.
    pub fn states(&self) -> &HashMap<String, ExtensionState> { &self.states }
/// TODO: document.
    pub fn extension_order(&self) -> &[String] { &self.order }
/// TODO: document.
    pub fn load_errors(&self) -> &[(PathBuf, String)] { &self.load_errors }

/// TODO: document.
    pub fn disable(&mut self, name: &str) -> Result<(), ExtensionError> { self.registry.disable(name)?; self.set_state(name, ExtensionState::Disabled); Ok(()) }
/// TODO: document.
    pub fn enable(&mut self, name: &str, ctx: &ExtensionContext) -> Result<(), ExtensionError> { self.registry.enable(name, ctx)?; self.set_state(name, ExtensionState::Active); Ok(()) }
/// TODO: document.
    pub fn is_enabled(&self, name: &str) -> bool { self.registry.is_enabled(name) }

/// TODO: document.
    pub fn has_handlers(&self, _event_type: &str) -> bool { self.has_enabled_extensions() }
/// TODO: document.
    pub fn has_enabled_extensions(&self) -> bool { self.registry.extensions().any(|_| true) && self.order.iter().any(|name| self.state(name) == ExtensionState::Active) }

/// TODO: document.
    pub fn all_tools(&self) -> Vec<ToolArc> {
        let mut tools = Vec::new();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) { tools.extend(ext.register_tools()); }
        }
        tools
    }

/// TODO: document.
    pub fn all_commands(&self) -> Vec<Command> {
        let mut commands = Vec::new();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) { commands.extend(ext.register_commands()); }
        }
        commands
    }

/// TODO: document.
    pub fn wrap_tool(&self, tool: ToolArc) -> ToolArc {
        Arc::new(WrappedTool { inner: tool, runner_state: Arc::new(RwLock::new(RunnerState { errors: self.registry.errors.clone(), error_listeners: self.error_listeners.clone() })) })
    }

/// TODO: document.
    pub fn wrap_tools(&self, tools: Vec<ToolArc>) -> Vec<ToolArc> { tools.into_iter().map(|t| self.wrap_tool(t)).collect() }

/// TODO: document.
    pub fn emit_tool_call(&self, tool_name: &str, params: &Value) -> ToolCallEmitResult {
        let mut result = ToolCallEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                if let Err(e) = ext.on_before_tool_call(tool_name, params) {
                    let err_str = e.to_string();
                    result.errors.push((name.clone(), err_str.clone()));
                    self.emit_error_record(ExtensionErrorRecord::new(name, "on_before_tool_call", &err_str));
                }
                self.registry.call_hook_safe(name, "on_tool_call", || { ext.on_tool_call(tool_name, params); });
            }
        }
        result
    }

/// TODO: document.
    pub fn emit_tool_result_event(&self, tool_name: &str, tool_result: &AgentToolResult) -> ToolResultEmitResult {
        let mut result = ToolResultEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                if let Err(e) = ext.on_after_tool_call(tool_name, tool_result) {
                    result.errors.push((name.clone(), e.to_string()));
                }
                self.registry.call_hook_safe(name, "on_tool_result", || { ext.on_tool_result(tool_name, tool_result); });
            }
        }
        result
    }

/// TODO: document.
    pub fn emit_input_event(&self, event: &mut InputEvent) -> InputEventResult {
        let mut final_result = InputEventResult::Continue;
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { ext.input(event) }));
                match result {
                    Ok(InputEventResult::Handled) => return InputEventResult::Handled,
                    Ok(InputEventResult::Transform { text }) => { event.text = text.clone(); final_result = InputEventResult::Transform { text }; }
                    Ok(InputEventResult::Continue) => {}
                    Err(payload) => {
                        let msg = if let Some(s) = payload.downcast_ref::<&str>() { s.to_string() }
                            else if let Some(s) = payload.downcast_ref::<String>() { s.clone() }
                            else { "unknown panic".to_string() };
                        self.emit_error_record(ExtensionErrorRecord::new(name, "input", &format!("panic: {}", msg)));
                    }
                }
            }
        }
        final_result
    }

/// TODO: document.
    pub fn emit_context_event(&self, messages: Vec<oxi_ai::Message>) -> ContextEmitResult {
        let mut current_messages = messages;
        let mut errors = Vec::new();
        let mut modified = false;
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                let prev_len = current_messages.len();
                let mut event = ContextEvent { messages: current_messages.clone() };
                if let Err(e) = ext.context(&mut event) {
                    errors.push((name.clone(), e.to_string()));
                } else if event.messages.len() != prev_len {
                    current_messages = event.messages;
                    modified = true;
                }
            }
        }
        ContextEmitResult { modified, messages: current_messages, errors }
    }

/// TODO: document.
    pub fn emit_before_provider_request_event(&self, payload: Value) -> ProviderRequestEmitResult {
        let mut current_payload = payload;
        let mut modified = false;
        let mut errors = Vec::new();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                let mut event = BeforeProviderRequestEvent { payload: current_payload.clone() };
                if let Err(e) = ext.before_provider_request(&mut event) {
                    errors.push((name.clone(), e.to_string()));
                } else if event.payload != current_payload {
                    current_payload = event.payload;
                    modified = true;
                }
            }
        }
        ProviderRequestEmitResult { modified, payload: current_payload, errors }
    }

/// TODO: document.
    pub fn emit_session_before_switch_event(&self, event: &SessionBeforeSwitchEvent) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                if let Err(e) = ext.session_before_switch(event) {
                    result.cancelled = true;
                    result.cancelled_by = Some(name.clone());
                    result.errors.push((name.clone(), e.to_string()));
                    return result;
                }
            }
        }
        result
    }

/// TODO: document.
    pub fn emit_session_before_fork_event(&self, event: &SessionBeforeForkEvent) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                if let Err(e) = ext.session_before_fork(event) {
                    result.cancelled = true;
                    result.cancelled_by = Some(name.clone());
                    result.errors.push((name.clone(), e.to_string()));
                    return result;
                }
            }
        }
        result
    }

/// TODO: document.
    pub fn emit_session_before_compact_event(&self, event: &SessionBeforeCompactEvent) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                if let Err(e) = ext.session_before_compact(event) {
                    result.cancelled = true;
                    result.cancelled_by = Some(name.clone());
                    result.errors.push((name.clone(), e.to_string()));
                    return result;
                }
            }
        }
        result
    }

/// TODO: document.
    pub fn emit_session_before_tree_event(&self, event: &SessionBeforeTreeEvent) -> SessionBeforeEmitResult {
        let mut result = SessionBeforeEmitResult::default();
        for name in &self.order {
            if self.state(name) != ExtensionState::Active { continue; }
            if let Some(ext) = self.registry.get(name) {
                if let Err(e) = ext.session_before_tree(event) {
                    result.cancelled = true;
                    result.cancelled_by = Some(name.clone());
                    result.errors.push((name.clone(), e.to_string()));
                    return result;
                }
            }
        }
        result
    }

/// TODO: document.
    pub fn emit_session_shutdown_event(&self, event: &SessionShutdownEvent) -> bool {
        if !self.has_enabled_extensions() { return false; }
        self.registry.emit_session_shutdown(event);
        true
    }

/// TODO: document.
    pub fn emit_event(&self, event: &AgentEvent) { self.registry.emit_event(event); }

/// TODO: document.
    pub fn registry(&self) -> &ExtensionRegistry { &self.registry }
/// TODO: document.
    pub fn registry_mut(&mut self) -> &mut ExtensionRegistry { &mut self.registry }
/// TODO: document.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Extension>> { self.registry.get(name) }
/// TODO: document.
    pub fn names(&self) -> impl Iterator<Item = &str> { self.order.iter().map(|s| s.as_str()) }
/// TODO: document.
    pub fn len(&self) -> usize { self.order.len() }
/// TODO: document.
    pub fn is_empty(&self) -> bool { self.order.is_empty() }
/// TODO: document.
    pub fn errors(&self) -> Vec<ExtensionErrorRecord> { self.registry.errors() }
/// TODO: document.
    pub fn clear_errors(&self) { self.registry.clear_errors(); }
}

impl fmt::Debug for ExtensionRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRunner").field("cwd", &self.cwd).field("extensions", &self.order).finish()
    }
}

/// Handle returned by [`ExtensionRunner::on_error`] for unregistering a listener.
pub struct ExtensionErrorHandle { listener: Option<Arc<ExtensionErrorListener>> }
impl ExtensionErrorHandle {
    /// Remove and return the listener, preventing further error notifications.
    pub fn unregister(&mut self) -> Option<Arc<ExtensionErrorListener>> { self.listener.take() } }
impl Drop for ExtensionErrorHandle { fn drop(&mut self) {} }

struct RunnerState { errors: Arc<RwLock<Vec<ExtensionErrorRecord>>>, error_listeners: Vec<Arc<ExtensionErrorListener>> }
struct WrappedTool { inner: ToolArc, runner_state: Arc<RwLock<RunnerState>> }

#[async_trait::async_trait]
impl oxi_agent::AgentTool for WrappedTool {
    fn name(&self) -> &str { self.inner.name() }
    fn label(&self) -> &str { self.inner.label() }
    fn description(&self) -> &str { self.inner.description() }
    fn parameters_schema(&self) -> Value { self.inner.parameters_schema() }
    async fn execute(&self, tool_call_id: &str, params: Value, signal: Option<tokio::sync::oneshot::Receiver<()>>) -> Result<AgentToolResult, String> {
        self.inner.execute(tool_call_id, params, signal).await
    }
}