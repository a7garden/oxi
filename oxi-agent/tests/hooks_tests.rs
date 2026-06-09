//! Agent loop config and tool execution tests

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use oxi_agent::{
        AgentLoopConfig, SharedState,
        tools::{AgentTool, AgentToolResult, ToolContext, ToolRegistry},
    };
    use oxi_ai::CompactionStrategy;
    use std::path::PathBuf;

    // ── Mock Tool ─────────────────────────────────────────────────────

    struct EchoTool;

    #[async_trait]
    impl AgentTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn label(&self) -> &str {
            "Echo"
        }
        fn description(&self) -> &str {
            "Echoes back the input"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &ToolContext,
        ) -> Result<AgentToolResult, oxi_agent::tools::ToolError> {
            Ok(AgentToolResult::success(format!(
                "Echo: {}",
                params["text"].as_str().unwrap_or("")
            )))
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_agent_loop_config_default() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.max_iterations, 20);
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_tokens, 4096);
    }

    #[test]
    fn test_agent_loop_config_custom() {
        let config = AgentLoopConfig {
            model_id: "anthropic/claude-sonnet-4".to_string(),
            max_iterations: 50,
            temperature: 0.5,
            max_tokens: 8192,
            compaction_strategy: CompactionStrategy::Threshold(0.8),
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(config.model_id, "anthropic/claude-sonnet-4");
        assert_eq!(config.max_iterations, 50);
    }

    #[test]
    fn test_shared_state_creation() {
        let state = SharedState::new();
        // Just verify it can be created
        drop(state);
    }

    #[test]
    fn test_tool_registry() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);
        assert!(registry.get("echo").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_tool_registry_with_builtins() {
        let registry = ToolRegistry::with_builtins_cwd(PathBuf::from("/tmp"), &[]);
        // Essential tools should be present
        assert!(registry.get("bash").is_some(), "bash should be registered");
        assert!(registry.get("read").is_some(), "read should be registered");
        assert!(
            registry.get("write").is_some(),
            "write should be registered"
        );
        assert!(registry.get("edit").is_some(), "edit should be registered");
        assert!(registry.get("grep").is_some(), "grep should be registered");
        assert!(registry.get("find").is_some(), "find should be registered");
        assert!(registry.get("ls").is_some(), "ls should be registered");
    }

    #[test]
    fn test_compaction_strategy_threshold() {
        let strategy = CompactionStrategy::Threshold(0.8);
        assert!(matches!(strategy, CompactionStrategy::Threshold(_)));
    }

    #[test]
    fn test_compaction_strategy_disabled() {
        let strategy = CompactionStrategy::Disabled;
        assert!(matches!(strategy, CompactionStrategy::Disabled));
    }

    #[test]
    fn test_compaction_strategy_every_n_turns() {
        let strategy = CompactionStrategy::EveryNTurns(5);
        assert!(matches!(strategy, CompactionStrategy::EveryNTurns(_)));
    }

    #[test]
    fn test_compaction_strategy_absolute_tokens() {
        let strategy = CompactionStrategy::AbsoluteTokens(100_000);
        assert!(matches!(strategy, CompactionStrategy::AbsoluteTokens(_)));
    }

    #[test]
    fn test_agent_tool_result_success() {
        let result = AgentToolResult::success("Hello world");
        assert!(result.success);
        assert_eq!(result.output, "Hello world");
        assert!(!result.terminate);
    }

    #[test]
    fn test_agent_tool_result_error() {
        let result = AgentToolResult::error("Something failed");
        assert!(!result.success);
        assert_eq!(result.output, "Something failed");
    }
}
