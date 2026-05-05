//! Extension system for oxi
//!
//! Extensions allow custom tools, commands, and event hooks to be loaded
//! dynamically at runtime. Extensions can be loaded from shared libraries
//! (.so/.dll/.dylib) via the `-e`/`--extension` CLI flag.
//!
//! # Architecture
//!
//! The extension system provides:
//!
//! - **Extension manifest** — metadata, permissions, configuration schema
//! - **Extension lifecycle hooks** — `on_load`, `on_unload`, message/tool/session events
//! - **Extension context** — access to settings, session state, tool registration, messaging
//! - **Extension error handling** — graceful degradation with logging
//! - **Extension registry** — name-based lookup, enable/disable, hot-reload

pub mod context;
pub mod loading;
pub mod registry;
pub mod types;

// Re-export all public types from submodules
pub use crate::extensions::context::{
    ExtensionContext, ExtensionContextBuilder,
};
pub use crate::extensions::loading::{
    discover_extensions, discover_extensions_in_dir, load_extension, load_extensions,
    NoopExtension,
};
pub use crate::extensions::registry::{
    ExtensionErrorHandle, ExtensionRegistry, ExtensionRunner, LoadedExtension, RunnerState,
};
pub use crate::extensions::types::{
    AfterProviderResponseEvent, AfterProviderResponseEvent, BashEvent,
    BeforeProviderRequestEvent, Command, ContextEmitResult, ContextEvent,
    ExtensionError, ExtensionErrorListener, ExtensionErrorRecord, ExtensionManifest,
    ExtensionPermission, ExtensionState, InputEvent, InputEventResult, InputSource,
    ModelSelectEvent, ModelSelectSource, ProviderRequestEmitResult,
    SessionBeforeCompactEvent, SessionBeforeEmitResult, SessionBeforeForkEvent,
    SessionBeforeSwitchEvent, SessionShutdownEvent, SessionShutdownReason,
    SessionSwitchReason, SessionTreeEvent, SessionBeforeTreeEvent, ThinkingLevelSelectEvent,
    ToolCallEmitResult, ToolResultEmitResult,
};

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
    fn register_tools(&self) -> Vec<std::sync::Arc<dyn crate::extensions::types::AgentTool>> {
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
    fn on_tool_call(&self, _tool: &str, _params: &serde_json::Value) {}

    /// Called after a tool finishes execution.
    fn on_tool_result(&self, _tool: &str, _result: &crate::extensions::types::AgentToolResult) {}

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
    fn on_event(&self, _event: &crate::extensions::types::AgentEvent) {}

    // ── Enhanced tool call hooks ─────────────────────────────────────

    /// Called immediately before a tool is executed.
    ///
    /// Use this for pre-processing, validation, or logging tool calls.
    /// Return `Err` to abort the tool execution (optional, implement
    /// [`on_before_tool_call_with_result`] for that).
    fn on_before_tool_call(
        &self,
        _tool: &str,
        _args: &serde_json::Value,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called immediately after a tool finishes execution.
    ///
    /// This is similar to [`on_tool_result`] but provides access to the
    /// full [`AgentToolResult`] including metadata.
    fn on_after_tool_call(
        &self,
        _tool: &str,
        _result: &crate::extensions::types::AgentToolResult,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    // ── Compaction hooks ─────────────────────────────────────────────

    /// Called before context compaction begins.
    ///
    /// Use this to save any state that should be preserved through compaction,
    /// or to log that compaction is starting.
    fn on_before_compaction(
        &self,
        _ctx: &crate::CompactionContext,
    ) -> Result<(), anyhow::Error> {
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

    // ── Session lifecycle hooks ─────────────────────

    /// Called before switching to another session.
    ///
    /// Return `Err` to cancel the switch.
    fn session_before_switch(
        &self,
        _event: &SessionBeforeSwitchEvent,
    ) -> Result<(), anyhow::Error> {
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
    fn session_before_compact(
        &self,
        _event: &SessionBeforeCompactEvent,
    ) -> Result<(), anyhow::Error> {
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
    fn session_before_tree(
        &self,
        _event: &SessionBeforeTreeEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called after navigating in the session tree.
    fn session_tree(&self, _event: &SessionTreeEvent) {}

    // ── Provider hooks ──────────────────────────────

    /// Called to inject or inspect context messages before the agent loop.
    fn context(&self, _event: &mut ContextEvent) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called before a provider request is sent to the LLM API.
    fn before_provider_request(
        &self,
        _event: &mut BeforeProviderRequestEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called after a provider response is received.
    fn after_provider_response(
        &self,
        _event: &AfterProviderResponseEvent,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    // ── Model hooks ─────────────────────────────────

    /// Called when a new model is selected.
    fn model_select(&self, _event: &ModelSelectEvent) {}

    /// Called when a new thinking level is selected.
    fn thinking_level_select(&self, _event: &ThinkingLevelSelectEvent) {}

    // ── Bash / Input hooks ──────────────────────────

    /// Called when a bash command is executed by the user.
    fn bash(&self, _event: &BashEvent) {}

    /// Called when user input is received, before agent processing.
    ///
    /// Return an [`InputEventResult`] to control how the input is processed.
    fn input(&self, _event: &InputEvent) -> InputEventResult {
        InputEventResult::Continue
    }
}

// Internal re-exports for use in this module
use crate::extensions::types::{
    AgentEvent, AgentTool, AgentToolResult, SessionCompactEvent,
};

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

    fn on_tool_call(&self, tool: &str, _params: &serde_json::Value) {
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
    use std::sync::Arc;

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
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        assert_eq!(ctx.cwd, std::path::PathBuf::from("/tmp"));
        assert!(ctx.session_id.is_none());
        assert!(ctx.is_idle());
    }

    #[test]
    fn test_context_builder_full() {
        use parking_lot::RwLock;
        let settings = Arc::new(RwLock::new(Settings::default()));
        let errors = Arc::new(RwLock::new(Vec::new()));
        let tools_registered = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let messages_sent = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let tools_ref = tools_registered.clone();
        let msgs_ref = messages_sent.clone();

        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/home"))
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

        assert_eq!(ctx.cwd, std::path::PathBuf::from("/home"));
        assert_eq!(ctx.session_id, Some("sess-123".to_string()));
        assert!(ctx.is_idle());

        // Config access
        assert_eq!(ctx.config_get("key"), Some(serde_json::json!("value")));
        assert_eq!(ctx.config_get("missing"), None);
    }

    #[test]
    fn test_context_config_nested() {
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp"))
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

        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp"))
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

        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp"))
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
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
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
        use parking_lot::RwLock;
        let settings = Arc::new(RwLock::new(Settings::default()));
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp"))
            .settings(settings.clone())
            .build();

        let s = ctx.settings();
        assert_eq!(s.version, Settings::default().version);
    }

    #[test]
    fn test_context_noop_callbacks() {
        // Test that no-op callbacks don't panic
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
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

        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();

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
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
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
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        // Already enabled — no-op
        reg.enable("noop", &ctx).unwrap();
        assert!(reg.is_enabled("noop"));
    }

    // ── Lifecycle hook broadcast tests ────────────────────────────────

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
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();

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
        let result = load_extension(std::path::Path::new("/nonexistent/extension.so"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_extension_wrong_extension() {
        let result = load_extension(std::path::Path::new("something.txt"));
        assert!(result.is_err());
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(msg.contains("Unsupported extension file format"));
    }

    #[test]
    fn test_load_extensions_collects_errors() {
        use std::path::Path;
        let paths: Vec<&Path> = vec![
            Path::new("/nonexistent1.so"),
            Path::new("/nonexistent2.so"),
        ];
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

    // ── Hot reload (error path) ─────────────────────────────────────

    #[test]
    fn test_hot_reload_no_source_path() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();

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
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();

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

        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
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
        let runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
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
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
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
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        assert_eq!(runner.state("ext1"), ExtensionState::Active);
        assert_eq!(runner.state("nonexistent"), ExtensionState::Unloaded);

        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        runner.disable("ext1").unwrap();
        assert_eq!(runner.state("ext1"), ExtensionState::Disabled);

        runner.enable("ext1", &ctx).unwrap();
        assert_eq!(runner.state("ext1"), ExtensionState::Active);
    }

    #[test]
    fn test_runner_enable_disable() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
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
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        runner.enable("ext1", &ctx).unwrap();
        assert!(runner.is_enabled("ext1"));
        assert_eq!(runner.state("ext1"), ExtensionState::Active);
    }

    #[test]
    fn test_runner_enable_disable_not_found() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        assert!(runner.disable("nonexistent").is_err());
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        assert!(runner.enable("nonexistent", &ctx).is_err());
    }

    #[test]
    fn test_runner_unload() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        assert!(runner.unload_extension("ext1"));
        assert_eq!(runner.state("ext1"), ExtensionState::Unloaded);
        assert!(runner.is_empty());
        assert!(!runner.unload_extension("ext1")); // already unloaded
    }

    #[test]
    fn test_runner_has_handlers() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        assert!(!runner.has_handlers("any_event"));
        assert!(!runner.has_enabled_extensions());

        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        assert!(runner.has_handlers("any_event"));
        assert!(runner.has_enabled_extensions());
    }

    #[test]
    fn test_runner_extension_order() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));

        // Register 3 extensions
        for name in &["ext1", "ext2", "ext3"] {
            let ext = Arc::new(RecordingExtension::new(name.to_string()));
            runner.registry_mut().register(ext.clone());
            runner
                .states
                .insert(name.to_string(), ExtensionState::Active);
            runner.order.push(name.to_string());
        }

        assert_eq!(
            runner.extension_order(),
            &["ext1".to_string(), "ext2".to_string(), "ext3".to_string()]
        );
        assert_eq!(runner.len(), 3);
    }

    #[test]
    fn test_runner_error_listener() {
        use std::sync::Mutex;
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let received = Arc::new(Mutex::new(Vec::<ExtensionErrorRecord>::new()));
        let received_clone = received.clone();

        let _handle = runner.on_error(move |record| {
            received_clone.lock().unwrap().push(record.clone());
        });

        // Emit an error
        runner.emit_error_record(ExtensionErrorRecord::new(
            "test-ext",
            "test_event",
            "test error",
        ));

        let records = received.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].extension_name, "test-ext");
        assert_eq!(records[0].event, "test_event");
    }

    #[test]
    fn test_runner_emit_tool_call() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        let result = runner.emit_tool_call("bash", &serde_json::json!({"cmd": "ls"}));
        assert!(!result.blocked);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_runner_emit_tool_result() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
        runner.order.push("ext1".to_string());

        let tool_result = AgentToolResult::success("done");
        let result = runner.emit_tool_result_event("bash", &tool_result);
        assert!(result.output.is_none());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_runner_emit_input_continue() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
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
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
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
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(RecordingExtension::new("ext1"));
        runner.registry_mut().register(ext.clone());
        runner
            .states
            .insert("ext1".to_string(), ExtensionState::Active);
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
        let runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let event = SessionShutdownEvent {
            reason: SessionShutdownReason::Quit,
            target_session_file: None,
        };
        let handled = runner.emit_session_shutdown_event(&event);
        assert!(!handled);
    }

    #[test]
    fn test_runner_load_extension_missing_file() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();
        let result = runner.load_extension(std::path::Path::new("/nonexistent.so"), &ctx);
        assert!(result.is_err());
        assert!(!runner.load_errors().is_empty());
    }

    #[test]
    fn test_runner_load_extension_wrong_format() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ctx = ExtensionContextBuilder::new(std::path::PathBuf::from("/tmp")).build();

        // Create a temp file with wrong extension
        let dir = tempfile::tempdir().unwrap();
        let bad_file = dir.path().join("bad.txt");
        std::fs::write(&bad_file, "not a library").unwrap();

        let result = runner.load_extension(&bad_file, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_runner_all_tools_in_order() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));

        // NoopExtension has no tools
        for name in &["ext1", "ext2"] {
            let ext = Arc::new(NoopExtension);
            // Give unique names by wrapping
            runner.registry_mut().register(ext.clone());
            runner
                .states
                .insert(name.to_string(), ExtensionState::Active);
            runner.order.push(name.to_string());
        }

        let tools = runner.all_tools();
        assert!(tools.is_empty()); // NoopExtension provides no tools
    }

    #[test]
    fn test_runner_delegation() {
        let mut runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
        let ext = Arc::new(NoopExtension);
        runner.registry_mut().register(ext);
        runner
            .states
            .insert("noop".to_string(), ExtensionState::Active);
        runner.order.push("noop".to_string());

        assert!(runner.get("noop").is_some());
        assert!(runner.get("missing").is_none());
        assert_eq!(runner.names().collect::<Vec<_>>(), vec!["noop"]);
    }

    #[test]
    fn test_runner_debug() {
        let runner = ExtensionRunner::new(std::path::PathBuf::from("/tmp"));
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
        let paths = discover_extensions_in_dir(std::path::Path::new("/nonexistent"));
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
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper(ExtensionState);

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