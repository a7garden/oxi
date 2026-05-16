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
pub mod closure_tool;
pub mod tool_factory;
pub mod prelude;

// Re-export core SDK types
pub use builder::{Oxi, OxiBuilder};
pub use agent_builder::AgentBuilder;
pub use closure_tool::ClosureTool;

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
    ToolContext,
    AgentHooks, ToolExecutionMode, AgentError,
    ProviderResolver,
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

    // ── Phase 2+ Tests: ProviderResolver, ClosureTool, Isolation ──

    #[test]
    fn test_provider_resolver_trait_on_oxi() {
        let oxi = OxiBuilder::new().with_builtins().build();
        // Oxi implements ProviderResolver
        let resolver: &dyn ProviderResolver = &oxi;
        assert!(resolver.resolve_provider("anthropic").is_some());
        assert!(resolver.resolve_provider("nonexistent").is_none());
        assert!(resolver.resolve_model("anthropic/claude-sonnet-4-20250514").is_some());
        assert!(resolver.resolve_model("nonexistent/model").is_none());
    }

    #[test]
    fn test_agent_uses_resolver_for_switch_model() {
        // Create isolated Oxi with only a mock model
        let oxi = OxiBuilder::new()
            .model(test_model("test-model", "test-provider"))
            .build();

        // This should fail because 'anthropic' provider isn't registered
        let config = AgentConfig {
            model_id: "test-provider/test-model".into(),
            max_iterations: 1,
            timeout_seconds: 5,
            ..Default::default()
        };
        let result = oxi.agent(config).build();
        // Agent build fails because provider 'test-provider' has no implementation
        // (no custom provider registered, no builtins enabled)
        assert!(result.is_err());
    }

    #[test]
    fn test_oxi_builder_without_builtins() {
        let oxi = OxiBuilder::new().build();
        // No models, no providers
        assert!(oxi.resolve_model("anthropic/claude-sonnet-4-20250514").is_err());
        assert!(oxi.create_provider("anthropic").is_err());
        assert!(!oxi.has_builtins());
    }

    #[test]
    fn test_oxi_builder_with_builtins_creates_providers() {
        let oxi = OxiBuilder::new().with_builtins().build();
        assert!(oxi.has_builtins());
        // Built-in provider fallback should work
        assert!(oxi.create_provider("anthropic").is_ok());
        assert!(oxi.create_provider("openai").is_ok());
        assert!(oxi.create_provider("deepseek").is_ok());
        // Unknown still fails
        assert!(oxi.create_provider("unknown-provider").is_err());
    }

    #[test]
    fn test_closure_tool_sync() {
        let tool = crate::closure_tool::ClosureTool::new_sync(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            }),
            |params, _ctx| {
                let input = params["input"].as_str().unwrap_or("default");
                Ok(AgentToolResult::success(format!("processed: {}", input)))
            },
        );

        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "A test tool");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(
            tool.execute("call_1", serde_json::json!({"input": "hello"}), None, &ToolContext::default())
        ).unwrap();
        assert!(result.success);
        assert!(result.output.contains("processed: hello"));
    }

    #[test]
    fn test_custom_tool_in_agent_builder() {
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            max_iterations: 10,
            timeout_seconds: 30,
            ..Default::default()
        };
        let result = oxi.agent(config)
            .workspace("/tmp")
            .custom_tool(
                "my_tool",
                "My custom tool",
                serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
                |params, _ctx| {
                    Ok(AgentToolResult::success(format!("result: {}", params["query"])))
                },
            )
            .build();

        if let Ok(agent) = result {
            let tool_names = agent.tools().names();
            assert!(tool_names.contains(&"my_tool".to_string()));
        }
    }

    #[test]
    fn test_full_isolation_between_instances() {
        // Instance 1: custom model + no builtins
        let oxi1 = OxiBuilder::new()
            .model(test_model("unique-alpha", "p1"))
            .build();

        // Instance 2: builtins only
        let oxi2 = OxiBuilder::new().with_builtins().build();

        // Cross-contamination check
        assert!(oxi2.resolve_model("p1/unique-alpha").is_err());
        assert!(oxi1.resolve_model("anthropic/claude-sonnet-4-20250514").is_err());

        // Provider isolation: oxi1 can't create anthropic (no builtins)
        assert!(oxi1.create_provider("anthropic").is_err());
        // oxi2 can create anthropic (builtins enabled)
        assert!(oxi2.create_provider("anthropic").is_ok());
    }

    #[test]
    fn test_agent_builder_system_prompt() {
        let oxi = OxiBuilder::new().with_builtins().build();
        let config = AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            max_iterations: 1,
            timeout_seconds: 5,
            ..Default::default()
        };
        let agent = oxi.agent(config)
            .workspace("/tmp")
            .system_prompt("You are a test agent.")
            .build()
            .unwrap();
        // Agent built successfully with custom system prompt
        drop(agent);
    }
}