//! Extension system for oxi
//!
//! Extensions allow custom tools, commands, and event hooks to be loaded dynamically at runtime.

pub mod context;
pub mod loading;
pub mod registry;
pub mod types;

// Re-export types from submodules
pub use crate::extensions::context::{ExtensionContext, ExtensionContextBuilder};
pub use crate::extensions::loading::{discover_extensions, discover_extensions_in_dir, load_extension, load_extensions};
pub use crate::extensions::registry::{ExtensionErrorHandle, ExtensionRegistry, ExtensionRunner};
pub use crate::extensions::types::{
    AfterProviderResponseEvent, BashEvent, BeforeProviderRequestEvent, Command,
    ContextEmitResult, ContextEvent, ExtensionError, ExtensionErrorListener,
    ExtensionErrorRecord, ExtensionManifest, ExtensionPermission, ExtensionState,
    InputEvent, InputEventResult, InputSource, ModelSelectEvent, ModelSelectSource,
    ProviderRequestEmitResult, SessionBeforeCompactEvent, SessionBeforeEmitResult,
    SessionBeforeForkEvent, SessionBeforeSwitchEvent, SessionBeforeTreeEvent,
    SessionCompactEvent, SessionShutdownEvent, SessionShutdownReason, SessionSwitchReason,
    SessionTreeEvent, ThinkingLevelSelectEvent, ToolCallEmitResult, ToolResultEmitResult,
};

// Re-export from oxi-agent
pub use oxi_agent::{AgentEvent, AgentTool, AgentToolResult};

// The Extension trait
/// Core trait that every oxi extension must implement.
pub trait Extension: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn manifest(&self) -> ExtensionManifest { ExtensionManifest::new(self.name(), "0.0.0").with_description(self.description()) }
    fn register_tools(&self) -> Vec<std::sync::Arc<dyn oxi_agent::AgentTool>> { vec![] }
    fn register_commands(&self) -> Vec<Command> { vec![] }
    fn on_load(&self, _ctx: &ExtensionContext) {}
    fn on_unload(&self) {}
    fn on_message_sent(&self, _msg: &str) {}
    fn on_message_received(&self, _msg: &str) {}
    fn on_tool_call(&self, _tool: &str, _params: &serde_json::Value) {}
    fn on_tool_result(&self, _tool: &str, _result: &oxi_agent::AgentToolResult) {}
    fn on_session_start(&self, _session_id: &str) {}
    fn on_session_end(&self, _session_id: &str) {}
    fn on_settings_changed(&self, _settings: &crate::settings::Settings) {}
    fn on_event(&self, _event: &oxi_agent::AgentEvent) {}
    fn on_before_tool_call(&self, _tool: &str, _args: &serde_json::Value) -> Result<(), anyhow::Error> { Ok(()) }
    fn on_after_tool_call(&self, _tool: &str, _result: &oxi_agent::AgentToolResult) -> Result<(), anyhow::Error> { Ok(()) }
    fn on_before_compaction(&self, _ctx: &crate::CompactionContext) -> Result<(), anyhow::Error> { Ok(()) }
    fn on_after_compaction(&self, _summary: &str) -> Result<(), anyhow::Error> { Ok(()) }
    fn on_error(&self, _error: &anyhow::Error) -> Result<(), anyhow::Error> { Ok(()) }
    fn session_before_switch(&self, _event: &crate::extensions::types::SessionBeforeSwitchEvent) -> Result<(), anyhow::Error> { Ok(()) }
    fn session_before_fork(&self, _event: &crate::extensions::types::SessionBeforeForkEvent) -> Result<(), anyhow::Error> { Ok(()) }
    fn session_before_compact(&self, _event: &crate::extensions::types::SessionBeforeCompactEvent) -> Result<(), anyhow::Error> { Ok(()) }
    fn session_compact(&self, _event: &crate::extensions::types::SessionCompactEvent) -> Result<(), anyhow::Error> { Ok(()) }
    fn session_shutdown(&self, _event: &crate::extensions::types::SessionShutdownEvent) {}
    fn session_before_tree(&self, _event: &crate::extensions::types::SessionBeforeTreeEvent) -> Result<(), anyhow::Error> { Ok(()) }
    fn session_tree(&self, _event: &crate::extensions::types::SessionTreeEvent) {}
    fn context(&self, _event: &mut crate::extensions::types::ContextEvent) -> Result<(), anyhow::Error> { Ok(()) }
    fn before_provider_request(&self, _event: &mut crate::extensions::types::BeforeProviderRequestEvent) -> Result<(), anyhow::Error> { Ok(()) }
    fn after_provider_response(&self, _event: &crate::extensions::types::AfterProviderResponseEvent) -> Result<(), anyhow::Error> { Ok(()) }
    fn model_select(&self, _event: &crate::extensions::types::ModelSelectEvent) {}
    fn thinking_level_select(&self, _event: &crate::extensions::types::ThinkingLevelSelectEvent) {}
    fn bash(&self, _event: &crate::extensions::types::BashEvent) {}
    fn input(&self, _event: &crate::extensions::types::InputEvent) -> crate::extensions::types::InputEventResult { crate::extensions::types::InputEventResult::Continue }
}

// Built-in "noop" extension
pub struct NoopExtension;
impl Extension for NoopExtension {
    fn name(&self) -> &str { "noop" }
    fn description(&self) -> &str { "Built-in no-op extension" }
}

// Test helpers
#[cfg(test)]
pub struct RecordingExtension {
    pub name: String,
    pub calls: std::sync::Mutex<Vec<String>>,
}
#[cfg(test)]
impl RecordingExtension {
    pub fn new(name: impl Into<String>) -> Self { Self { name: name.into(), calls: std::sync::Mutex::new(Vec::new()) } }
    pub fn push(&self, call: &str) { self.calls.lock().unwrap().push(call.to_string()); }
    pub fn calls(&self) -> Vec<String> { self.calls.lock().unwrap().clone() }
}
#[cfg(test)]
impl Extension for RecordingExtension {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { "recording test extension" }
    fn on_load(&self, _ctx: &ExtensionContext) { self.push("on_load"); }
    fn on_unload(&self) { self.push("on_unload"); }
    fn on_message_sent(&self, msg: &str) { self.push(&format!("on_message_sent({})", msg)); }
    fn on_message_received(&self, msg: &str) { self.push(&format!("on_message_received({})", msg)); }
    fn on_tool_call(&self, tool: &str, _params: &serde_json::Value) { self.push(&format!("on_tool_call({})", tool)); }
    fn on_tool_result(&self, tool: &str, _result: &oxi_agent::AgentToolResult) { self.push(&format!("on_tool_result({})", tool)); }
    fn on_session_start(&self, session_id: &str) { self.push(&format!("on_session_start({})", session_id)); }
    fn on_session_end(&self, session_id: &str) { self.push(&format!("on_session_end({})", session_id)); }
    fn on_settings_changed(&self, _settings: &crate::settings::Settings) { self.push("on_settings_changed"); }
    fn on_event(&self, _event: &oxi_agent::AgentEvent) { self.push("on_event"); }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
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
            fn name(&self) -> &str { "panicker" }
            fn description(&self) -> &str { "Panics" }
            fn on_load(&self, _ctx: &ExtensionContext) { panic!("intentional panic in on_load"); }
            fn on_message_sent(&self, _msg: &str) { panic!("intentional panic in on_message_sent"); }
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
    fn test_load_extension_missing_file() {
        let result = load_extension(std::path::Path::new("/nonexistent/extension.so"));
        assert!(result.is_err());
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

    #[test]
    fn test_discover_extensions_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let paths = discover_extensions_in_dir(dir.path());
        assert!(paths.is_empty());
    }
}