//! Extension system for oxi
//!
//! Extensions allow custom tools, commands, and event hooks to be loaded dynamically at runtime.

pub mod context;
#[allow(missing_docs)]
pub mod ext_cli;
pub mod loading;
pub mod registry;
pub mod stale;
pub mod types;
#[allow(missing_docs)]
pub mod wasm;
pub mod wasm_hooks;
pub mod wasm_tool;

// Re-export types from submodules
pub use crate::extensions::context::{ExtensionContext, ExtensionContextBuilder};
pub use crate::extensions::loading::{
    SHARED_LIB_EXTENSION, ValidatedExtension, discover_extensions, discover_extensions_in_dir,
    load_extension, load_extensions, validate_extension,
};
pub use crate::extensions::registry::{ExtensionErrorHandle, ExtensionRegistry, ExtensionRunner};
pub use crate::extensions::types::{
    AfterProviderResponseEvent, BashEvent, BeforeProviderRequestEvent, Command, ContextEmitResult,
    ContextEvent, ExtensionError, ExtensionErrorListener, ExtensionErrorRecord, ExtensionManifest,
    ExtensionPermission, ExtensionState, InputEvent, InputEventResult, InputSource,
    ModelSelectEvent, ModelSelectSource, ProviderRequestEmitResult, SessionBeforeCompactEvent,
    SessionBeforeEmitResult, SessionBeforeForkEvent, SessionBeforeSwitchEvent,
    SessionBeforeTreeEvent, SessionCompactEvent, SessionShutdownEvent, SessionShutdownReason,
    SessionSwitchReason, SessionTreeEvent, ThinkingLevelSelectEvent, ToolCallEmitResult,
    ToolResultEmitResult,
};
pub use crate::extensions::wasm::{
    ExtensionInfo, WasmCommandDef, WasmExtensionManager, WasmToolDef,
};
pub use crate::extensions::wasm_tool::WasmTool;

// Re-export from oxi-agent
pub use oxi_agent::{AgentEvent, AgentTool, AgentToolResult};

/// A keyboard shortcut registered by an extension.
#[derive(Debug, Clone)]
pub struct ExtensionShortcut {
    /// Key combination (e.g. "ctrl+shift+x")
    pub key: String,
    /// Human-readable description
    pub description: String,
    /// Action identifier (used to dispatch events)
    pub action: String,
}

// The Extension trait
/// Core trait that every oxi extension must implement.
pub trait Extension: Send + Sync {
    /// Returns the extension's unique identifier.
    fn name(&self) -> &str;
    /// Returns a human-readable description of what this extension does.
    fn description(&self) -> &str;
    /// Returns the extension manifest containing metadata and permissions.
    /// Default implementation constructs a minimal manifest from [`name()`](Extension::name) and
    /// [`description()`](Extension::description).
    fn manifest(&self) -> ExtensionManifest {
        ExtensionManifest::new(self.name(), "0.0.0").with_description(self.description())
    }
    /// Registers custom tools exposed by this extension.
    /// Each returned tool becomes available to the agent at runtime.
    /// Default returns an empty vector (no custom tools).
    fn register_tools(&self) -> Vec<std::sync::Arc<dyn oxi_agent::AgentTool>> {
        vec![]
    }
    /// Registers slash commands exposed by this extension.
    /// Each returned command becomes available as `/<name>` in the input field.
    /// Default returns an empty vector (no custom commands).
    fn register_commands(&self) -> Vec<Command> {
        vec![]
    }
    /// Called once when the extension is first loaded into a session.
    /// Use this to initialize resources, read config, or register with external services.
    fn on_load(&self, _ctx: &ExtensionContext) {}
    /// Called once when the extension is unloaded or the session ends.
    /// Use this to clean up resources allocated in [`on_load()`](Extension::on_load).
    fn on_unload(&self) {}
    /// Called after the agent sends a message to the LLM.
    /// `_msg` is the raw message content string.
    fn on_message_sent(&self, _msg: &str) {}
    /// Called after receiving a response from the LLM.
    /// `_msg` is the raw response content string.
    fn on_message_received(&self, _msg: &str) {}
    /// Called immediately before a tool is executed.
    /// `_tool` is the tool name and `_params` are the JSON arguments.
    fn on_tool_call(&self, _tool: &str, _params: &serde_json::Value) {}
    /// Called immediately after a tool execution completes.
    /// `_tool` is the tool name and `_result` contains the output or error.
    fn on_tool_result(&self, _tool: &str, _result: &oxi_agent::AgentToolResult) {}
    /// Called when a new session starts.
    /// `_session_id` uniquely identifies the session.
    fn on_session_start(&self, _session_id: &str) {}
    /// Called when a session ends.
    /// `_session_id` uniquely identifies the session that ended.
    fn on_session_end(&self, _session_id: &str) {}
    /// Called whenever the user saves or updates settings.
    fn on_settings_changed(&self, _settings: &crate::store::settings::Settings) {}
    /// Catch-all hook for any agent event not covered by a specific method.
    fn on_event(&self, _event: &oxi_agent::AgentEvent) {}
    /// Called before a tool is executed. Return `Err` to block the tool call.
    fn on_before_tool_call(
        &self,
        _tool: &str,
        _args: &serde_json::Value,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called after a tool completes. Return `Err` to surface an error to the agent.
    fn on_after_tool_call(
        &self,
        _tool: &str,
        _result: &oxi_agent::AgentToolResult,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called before the context window is compacted. Return `Err` to abort compaction.
    fn on_before_compaction(&self, _ctx: &crate::CompactionContext) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called after the context window is compacted with the generated summary.
    fn on_after_compaction(&self, _summary: &str) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called when any error occurs in the agent loop.
    fn on_error(&self, _error: &anyhow::Error) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called before the active session switches to a different branch or parent.
    fn session_before_switch(
        &self,
        _event: &crate::extensions::types::SessionBeforeSwitchEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called before a session is forked (branched) into a new subtree.
    fn session_before_fork(
        &self,
        _event: &crate::extensions::types::SessionBeforeForkEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called before the context window is compacted.
    fn session_before_compact(
        &self,
        _event: &crate::extensions::types::SessionBeforeCompactEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called when the context window is being compacted.
    fn session_compact(
        &self,
        _event: &crate::extensions::types::SessionCompactEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called when a session is shutting down.
    fn session_shutdown(&self, _event: &crate::extensions::types::SessionShutdownEvent) {}
    /// Called before a tree navigation action (branch listing, traversal, etc.).
    fn session_before_tree(
        &self,
        _event: &crate::extensions::types::SessionBeforeTreeEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called during a tree navigation action.
    fn session_tree(&self, _event: &crate::extensions::types::SessionTreeEvent) {}
    /// Emits into the agent context. Return `Err` to signal that the event was handled.
    fn context(
        &self,
        _event: &mut crate::extensions::types::ContextEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called before every LLM provider request. Allows the extension to mutate
    /// the request parameters (model, temperature, tools, etc.).
    fn before_provider_request(
        &self,
        _event: &mut crate::extensions::types::BeforeProviderRequestEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called after every LLM provider response. Allows the extension to read or
    /// annotate the response before it is processed by the agent loop.
    fn after_provider_response(
        &self,
        _event: &crate::extensions::types::AfterProviderResponseEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
    /// Called when the user or agent selects a model (via `/model` or auto-routing).
    fn model_select(&self, _event: &crate::extensions::types::ModelSelectEvent) {}
    /// Called when the thinking level is changed.
    fn thinking_level_select(&self, _event: &crate::extensions::types::ThinkingLevelSelectEvent) {}
    /// Called when a bash command is about to be executed.
    fn bash(&self, _event: &crate::extensions::types::BashEvent) {}
    /// Called for every user input keystroke. Return
    /// [`InputEventResult::Handled`] to suppress the default input handling, or
    /// [`InputEventResult::Transform { text }`] to replace the input text.
    fn input(
        &self,
        _event: &crate::extensions::types::InputEvent,
    ) -> crate::extensions::types::InputEventResult {
        crate::extensions::types::InputEventResult::Continue
    }
    /// Registers keyboard shortcuts exposed by this extension.
    /// Default returns an empty vector (no shortcuts).
    fn register_shortcuts(&self) -> Vec<ExtensionShortcut> {
        vec![]
    }
}

// Built-in "noop" extension
/// pub.
pub struct NoopExtension;
impl Extension for NoopExtension {
    fn name(&self) -> &str {
        "noop"
    }
    fn description(&self) -> &str {
        "Built-in no-op extension"
    }
}

// Test helpers
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
    fn on_tool_call(&self, tool: &str, _params: &serde_json::Value) {
        self.push(&format!("on_tool_call({})", tool));
    }
    fn on_tool_result(&self, tool: &str, _result: &oxi_agent::AgentToolResult) {
        self.push(&format!("on_tool_result({})", tool));
    }
    fn on_session_start(&self, session_id: &str) {
        self.push(&format!("on_session_start({})", session_id));
    }
    fn on_session_end(&self, session_id: &str) {
        self.push(&format!("on_session_end({})", session_id));
    }
    fn on_settings_changed(&self, _settings: &crate::store::settings::Settings) {
        self.push("on_settings_changed");
    }
    fn on_event(&self, _event: &oxi_agent::AgentEvent) {
        self.push("on_event");
    }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::settings::Settings;
    use std::sync::Arc;

    #[test]
    fn test_manifest_builder() {
        let manifest = ExtensionManifest::new("my-ext", "1.0.0")
            .with_description("A test extension")
            .with_author("test-author")
            .with_permission(ExtensionPermission::FileRead)
            .with_permission(ExtensionPermission::Bash)
            .with_config_schema(serde_json::json!({"type": "object", "properties": {"api_key": {"type": "string"}}}));

        assert_eq!(manifest.name, "my-ext");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.description, "A test extension");
        assert_eq!(manifest.author, "test-author");
        assert!(manifest.has_permission(ExtensionPermission::FileRead));
        assert!(manifest.has_permission(ExtensionPermission::Bash));
        assert!(!manifest.has_permission(ExtensionPermission::Network));
    }

    #[test]
    fn test_permission_display() {
        assert_eq!(ExtensionPermission::FileRead.to_string(), "file_read");
        assert_eq!(ExtensionPermission::Bash.to_string(), "bash");
    }

    #[test]
    fn test_context_builder_minimal() {
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        assert_eq!(ctx.cwd, std::path::PathBuf::from("/tmp"));
        assert!(ctx.session_id.is_none());
        assert!(ctx.is_idle());
    }

    #[test]
    fn test_context_builder_full() {
        use parking_lot::RwLock;
        let settings = Arc::new(RwLock::new(Settings::default()));
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/home"))
            .settings(settings)
            .config(serde_json::json!({"key": "value"}))
            .session_id("sess-123")
            .build();

        assert_eq!(ctx.cwd, std::path::PathBuf::from("/home"));
        assert_eq!(ctx.session_id, Some("sess-123".to_string()));
        assert_eq!(ctx.config_get("key"), Some(serde_json::json!("value")));
    }

    #[test]
    fn test_registry_register_and_collect() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
    }

    #[test]
    fn test_registry_enable_disable() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext);
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        assert!(reg.is_enabled("rec"));
        reg.disable("rec").unwrap();
        assert!(!reg.is_enabled("rec"));
        reg.enable("rec", &ctx).unwrap();
        assert!(reg.is_enabled("rec"));
    }

    #[test]
    fn test_emit_load() {
        let mut reg = ExtensionRegistry::new();
        let ext = Arc::new(RecordingExtension::new("rec"));
        reg.register(ext.clone());
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        reg.emit_load(&ctx);
        assert_eq!(ext.calls(), vec!["on_load"]);
    }

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
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        reg.emit_load(&ctx);
        reg.emit_message_sent("hello");
        let errors = reg.errors();
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn test_extension_state_display() {
        assert_eq!(ExtensionState::Pending.to_string(), "pending");
        assert_eq!(ExtensionState::Active.to_string(), "active");
    }

    #[test]
    fn test_tool_call_emit_result_default() {
        let result = ToolCallEmitResult::default();
        assert!(!result.blocked);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_runner_new() {
        let runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        assert!(runner.is_empty());
        assert_eq!(runner.len(), 0);
    }
}
