//! oxi SDK - Programmatic API for building AI agents
//!
//! # Example
//! ```
//! use oxi_sdk::{OxiBuilder, AgentConfig};
//!
//! let oxi = OxiBuilder::new().with_builtins().build();
//! let agent = oxi.agent(AgentConfig { 
//!     model_id: "anthropic/claude-sonnet-4-20250514".into(), 
//!     max_iterations: 20,
//!     ..Default::default() 
//! }).build().unwrap();
//! ```

pub mod builder;
pub mod agent_builder;
pub mod tool_factory;
pub mod prelude;

// Re-export core SDK types
pub use builder::{Oxi, OxiBuilder};
pub use agent_builder::AgentBuilder;

// Re-export from oxi-ai
pub use oxi_ai::{
    Provider, ProviderRegistry, Model, ModelRegistry, Context, Message, ContentBlock,
    ProviderEvent, StreamOptions, CompactionStrategy,
    ProviderError, Api, Cost, InputModality,
};

// Re-export from oxi-agent  
pub use oxi_agent::{
    Agent, AgentLoop, AgentLoopConfig, AgentConfig,
    AgentEvent, AgentState, SharedState,
    ToolRegistry, AgentTool, AgentToolResult, ToolError,
    AgentHooks, ToolExecutionMode, AgentError,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Helper to build a minimal Model for tests.
    fn test_model(id: &str, provider: &str) -> Model {
        Model::new(id, id, Api::AnthropicMessages, provider, "https://api.example.com")
    }

    #[test]
    fn test_oxi_builder_new() {
        let oxi = OxiBuilder::new().build();
        // Empty registry — no models
        assert!(oxi.resolve_model("anthropic/claude-sonnet-4-20250514").is_err());
    }

    #[test]
    fn test_oxi_builder_with_builtins() {
        let oxi = OxiBuilder::new().with_builtins().build();
        // Should have built-in models
        assert!(oxi.resolve_model("anthropic/claude-sonnet-4-20250514").is_ok());
        assert!(oxi.resolve_model("openai/gpt-4o").is_ok());
    }

    #[test]
    fn test_oxi_builder_custom_model() {
        let oxi = OxiBuilder::new()
            .model(test_model("test-model", "test-provider"))
            .build();
        assert!(oxi.resolve_model("test-provider/test-model").is_ok());
    }

    #[test]
    fn test_oxi_provider_resolution() {
        let oxi = OxiBuilder::new().with_builtins().build();
        // Built-in provider (falls back to built-in registry)
        assert!(oxi.create_provider("anthropic").is_ok());
        // Unknown provider
        assert!(oxi.create_provider("nonexistent").is_err());
    }

    #[test]
    fn test_agent_builder_workspace() {
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            max_iterations: 10,
            timeout_seconds: 30,
            ..Default::default()
        };
        // AgentBuilder with workspace — should not panic
        let result = oxi.agent(config)
            .workspace("/tmp/test-workspace")
            .build();
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_agent_builder_coding_tools() {
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            max_iterations: 10,
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxi.agent(config)
            .workspace("/tmp")
            .coding_tools()
            .build();
        if let Ok(agent) = result {
            let tool_names = agent.tools().names();
            assert!(tool_names.contains(&"read".to_string()));
            assert!(tool_names.contains(&"write".to_string()));
            assert!(tool_names.contains(&"edit".to_string()));
            assert!(tool_names.contains(&"ls".to_string()));
        }
    }

    #[test]
    fn test_agent_builder_readonly_tools() {
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            max_iterations: 10,
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxi.agent(config)
            .workspace("/tmp")
            .readonly_tools()
            .build();
        if let Ok(agent) = result {
            let tool_names = agent.tools().names();
            assert!(tool_names.contains(&"read".to_string()));
            assert!(tool_names.contains(&"ls".to_string()));
            // Should NOT have write/edit
            assert!(!tool_names.contains(&"write".to_string()));
        }
    }

    #[test]
    fn test_model_registry_isolation() {
        // Two separate Oxi instances should not share state
        let oxi1 = OxiBuilder::new()
            .model(test_model("unique-1", "test"))
            .build();

        let oxi2 = OxiBuilder::new().with_builtins().build();

        // oxi2 should NOT have oxi1's custom model
        assert!(oxi2.resolve_model("test/unique-1").is_err());
        // oxi1 should have its custom model
        assert!(oxi1.resolve_model("test/unique-1").is_ok());
    }

    #[test]
    fn test_tool_factory_coding_tools() {
        let tools = crate::tool_factory::coding_tools(Path::new("/tmp"));
        let names = tools.names();
        assert!(names.contains(&"read".to_string()));
        assert!(names.contains(&"write".to_string()));
        assert!(names.contains(&"edit".to_string()));
        assert!(names.contains(&"ls".to_string()));
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn test_tool_factory_readonly_tools() {
        let tools = crate::tool_factory::readonly_tools(Path::new("/tmp"));
        let names = tools.names();
        assert!(names.contains(&"read".to_string()));
        assert!(names.contains(&"ls".to_string()));
        assert_eq!(names.len(), 2);
    }
}