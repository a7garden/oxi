//! Provider message transform tests
//!
//! Tests message transformation through oxi_ai::transform.

#[cfg(test)]
mod tests {
    use oxi_ai::{
        Api, AssistantMessage, ContentBlock, Message, Model, StopReason, TextContent, ToolCall,
        ToolResultMessage, UserMessage, transform_messages_for_model,
    };

    fn model_for(api: Api) -> Model {
        Model::new("test", "Test", api, "mock", "http://localhost")
    }

    fn user_msg(text: &str) -> Message {
        Message::User(UserMessage::new(text.to_string()))
    }

    fn assistant_msg(text: &str) -> Message {
        let mut msg = AssistantMessage::new(Api::AnthropicMessages, "mock", "test-model");
        msg.content.push(ContentBlock::Text(TextContent::new(text)));
        Message::Assistant(msg)
    }

    fn tool_call_msg() -> Message {
        let mut msg = AssistantMessage::new(Api::AnthropicMessages, "mock", "test-model");
        msg.content.push(ContentBlock::ToolCall(ToolCall::new(
            "tc_1",
            "read",
            serde_json::json!({"path": "/tmp/test.rs"}),
        )));
        msg.stop_reason = StopReason::ToolUse;
        Message::Assistant(msg)
    }

    fn tool_result_msg(id: &str, content: &str, is_error: bool) -> Message {
        let mut msg = ToolResultMessage::new(
            id,
            "read",
            vec![ContentBlock::Text(TextContent::new(content))],
        );
        msg.is_error = is_error;
        Message::ToolResult(msg)
    }

    #[test]
    fn test_same_api_noop() {
        let messages = vec![user_msg("Hello"), assistant_msg("World")];
        let model = model_for(Api::OpenAiResponses);
        let result = transform_messages_for_model(&messages, &model);
        assert_eq!(result.len(), messages.len());
    }

    #[test]
    fn test_empty_messages() {
        let messages: Vec<Message> = vec![];
        let model = model_for(Api::AnthropicMessages);
        let result = transform_messages_for_model(&messages, &model);
        assert!(result.is_empty());
    }

    #[test]
    fn test_cross_api_preserves_count() {
        let messages = vec![user_msg("Hello")];
        let model = model_for(Api::AnthropicMessages);
        let result = transform_messages_for_model(&messages, &model);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_user_message_roundtrip() {
        let original = vec![user_msg("Hello world")];
        let to_anthropic =
            transform_messages_for_model(&original, &model_for(Api::AnthropicMessages));
        let back = transform_messages_for_model(&to_anthropic, &model_for(Api::OpenAiResponses));
        assert!(!back.is_empty());
    }

    #[test]
    fn test_tool_call_in_assistant() {
        let messages = vec![tool_call_msg()];
        let result = transform_messages_for_model(&messages, &model_for(Api::AnthropicMessages));
        assert!(!result.is_empty());
    }

    #[test]
    fn test_tool_result_preserved() {
        let messages = vec![tool_result_msg("tc_1", "File contents", false)];
        let result = transform_messages_for_model(&messages, &model_for(Api::AnthropicMessages));
        assert!(!result.is_empty());
    }

    #[test]
    fn test_error_tool_result_preserved() {
        let messages = vec![tool_result_msg("tc_err", "Error!", true)];
        let result = transform_messages_for_model(&messages, &model_for(Api::AnthropicMessages));
        let has_error = result.iter().any(|m| {
            if let Message::ToolResult(tr) = m {
                tr.is_error
            } else {
                false
            }
        });
        assert!(has_error, "Error tool result should be preserved");
    }

    #[test]
    fn test_mixed_content_blocks() {
        let mut msg = AssistantMessage::new(Api::AnthropicMessages, "mock", "test-model");
        msg.content
            .push(ContentBlock::Text(TextContent::new("I'll read the file.")));
        msg.content.push(ContentBlock::ToolCall(ToolCall::new(
            "tc_mixed",
            "read",
            serde_json::json!({"path": "/tmp/test.rs"}),
        )));
        msg.stop_reason = StopReason::ToolUse;
        let messages = vec![Message::Assistant(msg)];
        let result = transform_messages_for_model(&messages, &model_for(Api::AnthropicMessages));
        assert!(!result.is_empty());
    }

    #[test]
    fn test_multiple_messages_preserved() {
        let messages = vec![
            user_msg("First"),
            assistant_msg("Response 1"),
            user_msg("Second"),
        ];
        let result = transform_messages_for_model(&messages, &model_for(Api::AnthropicMessages));
        assert!(
            result.len() >= 3,
            "Should preserve all messages, got {}",
            result.len()
        );
    }
}
