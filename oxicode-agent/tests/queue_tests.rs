//! Agent loop configuration tests

// Integration tests use the same test-idiom pattern as inline unit tests
// (`let mut c = Config::default(); c.field = ..;`), so relax the same lint
// here to keep `cargo clippy --all-targets` clean. (`clippy::unwrap_used` is
// already `allow` by default in integration-test crates.)
#![allow(clippy::field_reassign_with_default)]

#[cfg(test)]
mod tests {
    use oxicode_agent::AgentLoopConfig;
    use oxicode_ai::CompactionStrategy;

    #[test]
    fn test_agent_loop_config_defaults() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.model_id, "");
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_tokens, 4096);
        assert!(matches!(
            config.compaction_strategy,
            CompactionStrategy::Threshold(_)
        ));
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
        let _state = oxicode_agent::SharedState::new();
    }
}
