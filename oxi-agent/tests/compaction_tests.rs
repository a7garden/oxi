//! Compaction and branch summary tests

#[cfg(test)]
mod tests {
    use oxi_agent::CompactionStrategy;
    use oxi_ai::{
        generate_branch_summary, Api, AssistantMessage, ContentBlock, Message, TextContent,
        UserMessage,
    };

    fn user_msg(text: &str) -> Message {
        Message::User(UserMessage::new(text.to_string()))
    }

    fn assistant_msg(text: &str) -> Message {
        let mut msg = AssistantMessage::new(Api::AnthropicMessages, "mock", "test-model");
        msg.content.push(ContentBlock::Text(TextContent::new(text)));
        Message::Assistant(msg)
    }

    #[test]
    fn test_generate_branch_summary_basic() {
        let messages = vec![
            user_msg("Please create a file called main.rs"),
            assistant_msg("I'll create the file for you."),
        ];
        let summary = generate_branch_summary(&messages, 5);
        assert!(!summary.is_empty());
    }

    #[test]
    fn test_generate_branch_summary_empty() {
        let messages: Vec<Message> = vec![];
        let summary = generate_branch_summary(&messages, 5);
        assert!(summary.is_empty() || summary.contains("empty") || summary.contains("Empty"));
    }

    #[test]
    fn test_generate_branch_summary_keywords() {
        let messages = vec![
            user_msg("search for TODO comments in the codebase"),
            assistant_msg("I found 5 TODO items."),
        ];
        let summary = generate_branch_summary(&messages, 5);
        assert!(!summary.is_empty());
    }

    #[test]
    fn test_generate_branch_summary_edit_keyword() {
        let messages = vec![
            user_msg("fix the bug in the authentication module"),
            assistant_msg("I edited the auth module to fix the bug."),
        ];
        let summary = generate_branch_summary(&messages, 5);
        assert!(!summary.is_empty());
    }

    #[test]
    fn test_generate_branch_summary_created_file() {
        let messages = vec![
            user_msg("Create a new README.md"),
            assistant_msg("I created the file README.md with project details."),
        ];
        let summary = generate_branch_summary(&messages, 5);
        assert!(!summary.is_empty());
    }

    #[test]
    fn test_generate_branch_summary_debug() {
        let messages = vec![
            user_msg("debug the crash in main.rs line 42"),
            assistant_msg("Found the null pointer dereference, fixed it."),
        ];
        let summary = generate_branch_summary(&messages, 5);
        assert!(!summary.is_empty());
    }

    #[test]
    fn test_compaction_threshold_strategy() {
        let strategy = CompactionStrategy::Threshold(0.8);
        match strategy {
            CompactionStrategy::Threshold(pct) => {
                assert!((pct - 0.8).abs() < f32::EPSILON);
            }
            _ => panic!("Expected Threshold"),
        }
    }

    #[test]
    fn test_compaction_strategy_default() {
        let default = CompactionStrategy::default();
        assert!(matches!(
            default,
            CompactionStrategy::Threshold(_)
                | CompactionStrategy::Disabled
                | CompactionStrategy::EveryNTurns(_)
                | CompactionStrategy::AbsoluteTokens(_)
        ));
    }
}
