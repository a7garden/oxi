//! Extension registry and runner.
//!
//! The registry manages loaded extensions and provides broadcast/event APIs.
//! The runner adds lifecycle management, state tracking, and discovery.

#![allow(unused)]

use crate::extensions::types::{
    AgentEvent, AgentTool, AgentToolResult, BashEvent, BeforeProviderRequestEvent,
    Command, ContextEmitResult, ContextEvent, ExtensionError, ExtensionErrorRecord,
    ExtensionErrorListener, ExtensionManifest, ExtensionPermission, ExtensionState,
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
// Extension trait (imported from oxi_cli extensions)
// ═══════════════════════════════════════════════════════════════════════════

pub trait Extension: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn manifest(&self) -> ExtensionManifest { ExtensionManifest::new(self.name(), "0.0.0").with_description(self.description()) }
    fn register_tools(&self) -> Vec<Arc<dyn AgentTool>> { vec![] }
    fn register_commands(&self) -> Vec<Command> { vec![] }
    fn on_load(&self, _ctx: &crate::extensions::ExtensionContext) {}
    fn on_unload(&self) {}
    fn on_message_sent(&self, _msg: &str) {}
    fn on_message_received(&self, _msg: &str) {}
    fn on_event(&self, _event: &AgentEvent) {}
    fn on_settings_changed(&self, _settings: &Settings) {}
    fn on_session_start(&self, _session_id: &str) {}
    fn on_session_end(&self, _session_id: &str) {}
    fn on_tool_call(&self, _tool: &str, _params: &Value) {}
    fn on_tool_result(&self, _tool: &str, _result: &AgentToolResult) {}
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Registry
// ═══════════════════════════════════════════════════════════════════════════

/// Extension error listener handle (opaque).
pub struct ExtensionErrorHandle(Option<Box<dyn ExtensionErrorListener + Send + Sync>>);

/// Core registry of loaded extensions.
/// Broadcasts lifecycle events to all registered extensions and provides
/// a central API for extension queries.
pub struct ExtensionRegistry {
    extensions: RwLock<HashMap<String, Arc<dyn Extension>>>,
    enabled: RwLock<HashMap<String, bool>>,
    error_records: RwLock<Vec<ExtensionErrorRecord>>,
    error_listeners: RwLock<Vec<Box<dyn ExtensionErrorListener + Send + Sync>>,
}

impl Default for ExtensionRegistry {
    fn default() -> Self { Self::new() }
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            extensions: RwLock::new(HashMap::new()),
            enabled: RwLock::new(HashMap::new()),
            error_records: RwLock::new(Vec::new()),
            error_listeners: RwLock::new(Vec::new()),
        }
    }

    pub fn register(&mut self, ext: Arc<dyn Extension>) {
        let name = ext.name().to_string();
        self.extensions.write().insert(name.clone(), ext);
        self.enabled.write().insert(name, true);
    }

    pub fn register_arc(&self, ext: Arc<dyn Extension>) {
        let name = ext.name().to_string();
        self.extensions.write().insert(name.clone(), ext);
        self.enabled.write().insert(name, true);
    }

    pub fn unregister(&mut self, name: &str) -> bool {
        if self.extensions.write().remove(name).is_some() {
            self.enabled.write().remove(name);
            true
        } else { false }
    }

    pub fn is_empty(&self) -> bool {
        self.extensions.read().is_empty()
    }

    pub fn len(&self) -> usize {
        self.extensions.read().len()
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.enabled.read().get(name).copied().unwrap_or(false)
    }

    pub fn all_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let enabled = self.enabled.read();
        self.extensions.read().iter()
            .filter(|(name, _)| enabled.get(*name).copied().unwrap_or(false))
            .flat_map(|(_, ext)| ext.register_tools())
            .collect()
    }

    pub fn all_commands(&self) -> Vec<Command> {
        self.extensions.read().values()
            .flat_map(|ext| ext.register_commands())
            .collect()
    }

    pub fn emit_tool_call(&self, tool: &str, params: &Value) {
        for ext in self.extensions.read().values() {
            ext.on_tool_call(tool, params);
        }
    }

    pub fn emit_tool_result(&self, tool: &str, result: &AgentToolResult) {
        for ext in self.extensions.read().values() {
            ext.on_tool_result(tool, result);
        }
    }

    pub fn emit_event(&self, event: &AgentEvent) {
        for ext in self.extensions.read().values() {
            ext.on_event(event);
        }
    }

    pub fn errors(&self) -> Vec<ExtensionErrorRecord> {
        self.error_records.read().clone()
    }

    pub fn clear_errors(&self) {
        self.error_records.write().clear();
    }

    pub fn on_error<F>(&mut self, f: F) where F: Fn(&ExtensionErrorRecord) + Send + Sync + 'static {
        self.error_listeners.write().push(Box::new(f));
    }
}

impl fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRegistry")
            .field("count", &self.extensions.read().len())
            .finish()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Runner (stateful wrapper)
// ═══════════════════════════════════════════════════════════════════════════

pub struct ExtensionRunner {
    pub registry: ExtensionRegistry,
    pub states: HashMap<String, ExtensionState>,
    pub order: Vec<String>,
}

impl Default for ExtensionRunner {
    fn default() -> Self { Self::new(PathBuf::from(".")) }
}

impl ExtensionRunner {
    pub fn new(_cwd: PathBuf) -> Self {
        Self {
            registry: ExtensionRegistry::new(),
            states: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn registry_mut(&mut self) -> &mut ExtensionRegistry { &mut self.registry }
    pub fn is_empty(&self) -> bool { self.registry.is_empty() }
    pub fn len(&self) -> usize { self.registry.len() }
    pub fn state(&self, name: &str) -> ExtensionState {
        self.states.get(name).copied().unwrap_or(ExtensionState::Unloaded)
    }

    pub fn names(&self) -> impl Iterator<Item = &String> { self.order.iter() }

    pub fn emit_tool_call(&self, tool: &str, params: &Value) -> ToolCallEmitResult {
        self.registry.emit_tool_call(tool, params);
        ToolCallEmitResult::default()
    }

    pub fn emit_tool_result_event(&self, tool: &str, result: &AgentToolResult) -> ToolResultEmitResult {
        self.registry.emit_tool_result(tool, result);
        ToolResultEmitResult::default()
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.registry.is_enabled(name)
    }

    pub fn names_ordered(&self) -> &[String] { &self.order }
    pub fn extension_order(&self) -> &[String] { &self.order }
    pub fn has_enabled_extensions(&self) -> bool {
        !self.registry.is_empty()
    }
    pub fn get(&self, _name: &str) -> Option<&Arc<dyn Extension>> {
        None
    }

    pub fn load_extension<P: AsRef<Path>>(&mut self, _path: P, _ctx: &crate::extensions::ExtensionContext) -> Result<()> {
        bail!("Extension loading requires full implementation")
    }

    pub fn emit_error_record(&self, _record: ExtensionErrorRecord) {}
    pub fn load_errors(&self) -> Vec<String> { vec![] }
}

impl fmt::Debug for ExtensionRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRunner").finish()
    }
}

// Stub types
impl Default for ExtensionState {
    fn default() -> Self { ExtensionState::Active }
}
impl fmt::Display for ExtensionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self { ExtensionState::Pending => write!(f, "pending"), ExtensionState::Active => write!(f, "active"), ExtensionState::Disabled => write!(f, "disabled"), ExtensionState::Failed => write!(f, "failed"), ExtensionState::Unloaded => write!(f, "unloaded") }
    }
}