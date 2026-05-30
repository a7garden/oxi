//! Agent loop configuration tests

#[cfg(test)]
mod tests {
    use oxi_agent::AgentLoopConfig;
    use oxi_ai::CompactionStrategy;

    #[test]
    fn test_agent_loop_config_defaults() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.model_id, "");
        assert_eq!(config.max_iterations, 20);
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_tokens, 4096);
        assert!(matches!(
            config.compaction_strategy,
            CompactionStrategy::Threshold(_)
        ));
    }

    #[test]
    fn test_agent_loop_config_max_iterations() {
        let mut config = AgentLoopConfig::default();
        config.max_iterations = 5;
        assert_eq!(config.max_iterations, 5);
    }

    #[test]
    fn test_agent_loop_config_compaction_strategy() {
        let mut config = AgentLoopConfig::default();
        config.compaction_strategy = CompactionStrategy::Disabled;
        assert!(matches!(
            config.compaction_strategy,
            CompactionStrategy::Disabled
        ));

        config.compaction_strategy = CompactionStrategy::EveryNTurns(3);
        assert!(matches!(
            config.compaction_strategy,
            CompactionStrategy::EveryNTurns(_)
        ));
    }

    #[test]
    fn test_agent_loop_config_context_window() {
        let mut config = AgentLoopConfig::default();
        config.context_window = 200_000;
        assert_eq!(config.context_window, 200_000);
    }

    #[test]
    fn test_shared_state_creation() {
        let _state = oxi_agent::SharedState::new();
    }
}
