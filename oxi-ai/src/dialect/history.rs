//! In-band tool-history encoding.
//!
//! Port of omp's `dialect/history.ts`. Rewrites prior assistant tool calls and
//! tool results into the dialect's text form so a tools-less request still sees
//! a coherent transcript (and the message prefix stays stable for caching).

use super::{Dialect, RenderedToolResult};
use crate::messages::{ContentBlock, Message, MessageContent, TextContent, UserMessage};
use crate::tools::Tool;

/// Re-encode a message history for an owned-dialect (tools-less) request.
///
/// - Assistant messages with tool calls have those calls rendered as text and
///   merged into the message's prose.
/// - Runs of consecutive tool-result messages collapse into a single user
///   message whose text is the dialect's rendered result blocks.
/// - All other messages pass through unchanged.
pub fn encode_inband_tool_history(
    messages: &[Message],
    dialect: Dialect,
    tools: &[Tool],
) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut i = 0usize;
    while i < messages.len() {
        match &messages[i] {
            Message::Assistant(m) => {
                out.push(encode_assistant(m, dialect, tools));
                i += 1;
            }
            Message::ToolResult(_) => {
                // Collect the maximal run of consecutive tool results.
                let start = i;
                while i < messages.len() && matches!(messages[i], Message::ToolResult(_)) {
                    i += 1;
                }
                out.push(encode_tool_results(&messages[start..i], dialect));
            }
            other => {
                out.push(other.clone());
                i += 1;
            }
        }
    }
    out
}

/// Render an assistant message's tool calls as text merged with its prose.
fn encode_assistant(
    message: &crate::messages::AssistantMessage,
    dialect: Dialect,
    tools: &[Tool],
) -> Message {
    let tool_calls: Vec<_> = message
        .content
        .iter()
        .filter_map(|b| b.as_tool_call())
        .cloned()
        .collect();
    if tool_calls.is_empty() {
        return Message::Assistant(message.clone());
    }

    // Gather the prose (text blocks) and thinking blocks to preserve ordering.
    let prose: Vec<&str> = message.content.iter().filter_map(|b| b.as_text()).collect();
    let prose = prose.join("\n");
    let rendered = dialect.render_tool_calls(&tool_calls, tools);
    let text = if prose.trim().is_empty() {
        rendered
    } else {
        format!("{}\n{}", prose.trim_end(), rendered)
    };

    let mut out = message.clone();
    out.content = vec![ContentBlock::Text(TextContent::new(text))];
    Message::Assistant(out)
}
/// Collapse a run of tool-result messages into one user message of rendered text.
fn encode_tool_results(results: &[Message], dialect: Dialect) -> Message {
    let mut dialect_results = Vec::with_capacity(results.len());
    let mut timestamp = chrono::Utc::now().timestamp_millis();
    for (index, result) in results.iter().enumerate() {
        if let Message::ToolResult(tr) = result {
            if index == 0 {
                timestamp = tr.timestamp;
            }
            dialect_results.push(RenderedToolResult {
                id: tr.tool_call_id.clone(),
                name: tr.tool_name.clone(),
                index,
                text: tr.text_content().unwrap_or_default(),
                is_error: tr.is_error,
            });
        }
    }
    let text = dialect.render_tool_results(&dialect_results);
    let mut user = UserMessage::new(MessageContent::Text(text));
    user.timestamp = timestamp;
    Message::User(user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Api;
    use crate::messages::{AssistantMessage, ToolCall, ToolResultMessage};
    use serde_json::json;

    fn echo_tool() -> Tool {
        Tool {
            name: "echo".to_string(),
            description: "Echo".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"msg": {"type": "string"}},
            }),
        }
    }

    #[test]
    fn assistant_tool_calls_become_text() {
        let tools = vec![echo_tool()];
        let mut assistant = AssistantMessage::new(Api::OpenAiCompletions, "test", "model");
        assistant.content = vec![
            ContentBlock::Text(TextContent::new("Running echo.")),
            ContentBlock::ToolCall(ToolCall::new("c1", "echo", json!({"msg": "hi"}))),
        ];
        let messages = vec![Message::Assistant(assistant)];
        let encoded = encode_inband_tool_history(&messages, Dialect::Xml, &tools);

        assert_eq!(encoded.len(), 1);
        let Message::Assistant(a) = &encoded[0] else {
            panic!("expected assistant");
        };
        // Tool call rendered as text, prose preserved.
        let text = a.text_content();
        assert!(text.contains("Running echo."));
        assert!(text.contains("invoke"));
        assert!(text.contains("echo"));
        // No native tool-call blocks remain.
        assert!(a.content.iter().all(|b| b.as_tool_call().is_none()));
    }

    #[test]
    fn tool_result_run_collapses_to_single_user_message() {
        let results = vec![
            Message::ToolResult(ToolResultMessage::new(
                "c1",
                "echo",
                vec![ContentBlock::Text(TextContent::new("out1"))],
            )),
            Message::ToolResult(ToolResultMessage::new(
                "c2",
                "echo",
                vec![ContentBlock::Text(TextContent::new("out2"))],
            )),
        ];
        let encoded = encode_inband_tool_history(&results, Dialect::Xml, &[]);
        assert_eq!(encoded.len(), 1);
        let Message::User(u) = &encoded[0] else {
            panic!("expected user");
        };
        let text = match &u.content {
            MessageContent::Text(s) => s.clone(),
            _ => String::new(),
        };
        assert!(text.contains("out1"));
        assert!(text.contains("out2"));
    }

    #[test]
    fn plain_messages_pass_through() {
        let user = Message::User(UserMessage::new("hello"));
        let encoded = encode_inband_tool_history(std::slice::from_ref(&user), Dialect::Xml, &[]);
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].text_content().unwrap(), "hello");
    }

    #[test]
    fn mixed_history_preserves_order() {
        let tools = vec![echo_tool()];
        let mut assistant = AssistantMessage::new(Api::OpenAiCompletions, "test", "model");
        assistant.content = vec![ContentBlock::ToolCall(ToolCall::new(
            "c1",
            "echo",
            json!({"msg": "x"}),
        ))];
        let messages = vec![
            Message::User(UserMessage::new("go")),
            Message::Assistant(assistant),
            Message::ToolResult(ToolResultMessage::new(
                "c1",
                "echo",
                vec![ContentBlock::Text(TextContent::new("done"))],
            )),
        ];
        let encoded = encode_inband_tool_history(&messages, Dialect::Xml, &tools);
        assert_eq!(encoded.len(), 3);
        assert!(matches!(encoded[0], Message::User(_)));
        assert!(matches!(encoded[1], Message::Assistant(_)));
        assert!(matches!(encoded[2], Message::User(_))); // collapsed tool result
    }
}
