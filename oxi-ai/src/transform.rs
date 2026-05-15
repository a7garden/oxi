//! Cross-provider message transformation.
//!
//! When switching models mid-conversation (e.g. Claude → GPT), message formats
//! need to be converted so the target provider can understand the history.
//!
//! # Supported conversions
//!
//! - **Anthropic ↔ OpenAI**: Tool calls (`tool_use` ↔ `function`), thinking blocks,
//!   image data URIs.
//! - **Google → OpenAI**: Function calls, inline images.
//! - **Any → Any**: Falls back through OpenAI as a universal intermediate.
//!
//! # Usage
//!
//! ```ignore
//! use oxi_ai::transform::{transform_messages, TransformOptions};
//!
//! let converted = transform_messages(
//!     &messages,
//!     Api::AnthropicMessages,
//!     Api::OpenAiCompletions,
//!     TransformOptions::default(),
//! );
//! ```

use serde_json::Value as JsonValue;

use crate::{
    Api, AssistantMessage, ContentBlock, ImageContent, ImageContentType, InputModality, Message,
    MessageContent, Model, StopReason, TextContent, TextContentType, ThinkingContent, ToolCall,
    ToolCallType, ToolResultMessage, Usage,
};

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Options that control how messages are transformed between providers.
#[derive(Debug, Clone)]
pub struct TransformOptions {
    /// Strip thinking blocks entirely instead of converting them to text.
    pub strip_thinking: bool,
    /// Convert tool calls / tool results (when `false`, tool calls are dropped).
    pub convert_tools: bool,
    /// Convert image blocks (when `false`, images are dropped).
    pub convert_images: bool,
    /// Merge adjacent text blocks produced by the transformation.
    pub merge_text: bool,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self {
            strip_thinking: false,
            convert_tools: true,
            convert_images: true,
            merge_text: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level API
// ---------------------------------------------------------------------------

/// Transform a slice of [`Message`]s from one provider API to another.
///
/// This is the primary entry-point.  It dispatches to the appropriate
/// directional converter and then applies the requested post-processing
/// options.
///
/// # Cross-provider transformation flow
///
/// ```ignore
/// use oxi_ai::{transform_messages, Api};
///
/// // Convert messages from Anthropic format to OpenAI format
/// let anthropic_msgs = vec![/* ... */];
/// let openai_msgs = transform_messages(
///     &anthropic_msgs,
///     Api::AnthropicMessages,
///     Api::OpenAiCompletions,
///     Default::default(),
/// );
/// ```
pub fn transform_messages(
    messages: &[Message],
    from_api: Api,
    to_api: Api,
    opts: TransformOptions,
) -> Vec<Message> {
    if from_api == to_api {
        return messages.to_vec();
    }

    // Convert every message through an internal JSON-based intermediate
    // representation, then back into native `Message`s for the target API.
    let intermediate: Vec<IntermediateMessage> = messages
        .iter()
        .map(|m| to_intermediate(m, &from_api))
        .collect();

    intermediate
        .into_iter()
        .map(|im| from_intermediate(&im, &to_api, &opts))
        .collect()
}

// ---------------------------------------------------------------------------
// Intermediate (provider-neutral) representation
// ---------------------------------------------------------------------------

/// A provider-neutral representation of a single message, used as the
/// intermediate format during cross-provider conversion.
#[derive(Debug, Clone)]
enum IntermediateMessage {
    User {
        content: IntermediateContent,
    },
    Assistant {
        content: Vec<IntermediateBlock>,
        model: String,
        provider: String,
        usage: Usage,
        stop_reason: StopReason,
        error_message: Option<String>,
        response_id: Option<String>,
        timestamp: i64,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<IntermediateBlock>,
        is_error: bool,
    },
}

/// Provider-neutral content for a user message.
#[derive(Debug, Clone)]
enum IntermediateContent {
    Text(String),
    Blocks(Vec<IntermediateBlock>),
}

/// Provider-neutral content block.
#[derive(Debug, Clone)]
enum IntermediateBlock {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    Image {
        data: String,
        mime_type: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: JsonValue,
    },
}

// ---------------------------------------------------------------------------
// Source → Intermediate
// ---------------------------------------------------------------------------

/// Convert a native [`Message`] into the intermediate representation.
fn to_intermediate(msg: &Message, _from_api: &Api) -> IntermediateMessage {
    match msg {
        Message::User(u) => {
            let content = match &u.content {
                MessageContent::Text(s) => IntermediateContent::Text(s.clone()),
                MessageContent::Blocks(blocks) => {
                    IntermediateContent::Blocks(blocks.iter().map(block_to_intermediate).collect())
                }
            };
            IntermediateMessage::User { content }
        }

        Message::Assistant(a) => IntermediateMessage::Assistant {
            content: a.content.iter().map(block_to_intermediate).collect(),
            model: a.model.clone(),
            provider: a.provider.clone(),
            usage: a.usage.clone(),
            stop_reason: a.stop_reason,
            error_message: a.error_message.clone(),
            response_id: a.response_id.clone(),
            timestamp: a.timestamp,
        },

        Message::ToolResult(t) => IntermediateMessage::ToolResult {
            tool_call_id: t.tool_call_id.clone(),
            tool_name: t.tool_name.clone(),
            content: t.content.iter().map(block_to_intermediate).collect(),
            is_error: t.is_error,
        },
    }
}

fn block_to_intermediate(block: &ContentBlock) -> IntermediateBlock {
    match block {
        ContentBlock::Text(t) => IntermediateBlock::Text(t.text.clone()),
        ContentBlock::Thinking(th) => IntermediateBlock::Thinking {
            text: th.thinking.clone(),
            signature: th.thinking_signature.clone(),
        },
        ContentBlock::Image(img) => IntermediateBlock::Image {
            data: img.data.clone(),
            mime_type: img.mime_type.clone(),
        },
        ContentBlock::ToolCall(tc) => IntermediateBlock::ToolCall {
            id: tc.id.clone(),
            name: tc.name.clone(),
            arguments: tc.arguments.clone(),
        },
        ContentBlock::Unknown(val) => {
            // Best-effort: try to extract text.
            if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
                IntermediateBlock::Text(text.to_string())
            } else {
                IntermediateBlock::Text(format!("[unknown block: {}]", val))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Intermediate → Target
// ---------------------------------------------------------------------------

/// Convert an [`IntermediateMessage`] into a native [`Message`] for the target API.
fn from_intermediate(im: &IntermediateMessage, to_api: &Api, opts: &TransformOptions) -> Message {
    match im {
        IntermediateMessage::User { content } => {
            let native_content = match content {
                IntermediateContent::Text(s) => MessageContent::Text(s.clone()),
                IntermediateContent::Blocks(blocks) => {
                    let native_blocks: Vec<ContentBlock> = blocks
                        .iter()
                        .flat_map(|b| intermediate_to_blocks(b, to_api, opts))
                        .collect();
                    let merged = if opts.merge_text {
                        merge_adjacent_text_blocks(native_blocks)
                    } else {
                        native_blocks
                    };
                    MessageContent::Blocks(merged)
                }
            };
            Message::User(crate::UserMessage {
                role: crate::UserRole::User,
                content: native_content,
                timestamp: chrono::Utc::now().timestamp_millis(),
            })
        }

        IntermediateMessage::Assistant {
            content,
            model,
            provider,
            usage,
            stop_reason,
            error_message,
            response_id,
            timestamp,
        } => {
            let mut native_blocks: Vec<ContentBlock> = content
                .iter()
                .flat_map(|b| intermediate_to_blocks(b, to_api, opts))
                .collect();
            if opts.merge_text {
                native_blocks = merge_adjacent_text_blocks(native_blocks);
            }

            let mut msg = AssistantMessage::new(*to_api, provider, model);
            msg.content = native_blocks;
            msg.usage = usage.clone();
            msg.stop_reason = *stop_reason;
            msg.error_message = error_message.clone();
            msg.response_id = response_id.clone();
            msg.timestamp = *timestamp;
            Message::Assistant(msg)
        }

        IntermediateMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } => {
            let mut native_blocks: Vec<ContentBlock> = content
                .iter()
                .flat_map(|b| intermediate_to_blocks(b, to_api, opts))
                .collect();
            if opts.merge_text {
                native_blocks = merge_adjacent_text_blocks(native_blocks);
            }

            let mut msg = ToolResultMessage::new(tool_call_id, tool_name, native_blocks);
            msg.is_error = *is_error;
            Message::ToolResult(msg)
        }
    }
}

/// Convert a single [`IntermediateBlock`] into zero or more [`ContentBlock`]s
/// appropriate for the target API.
fn intermediate_to_blocks(
    ib: &IntermediateBlock,
    to_api: &Api,
    opts: &TransformOptions,
) -> Vec<ContentBlock> {
    match ib {
        IntermediateBlock::Text(text) => {
            vec![ContentBlock::Text(TextContent {
                content_type: TextContentType::Text,
                text: text.clone(),
                text_signature: None,
            })]
        }

        IntermediateBlock::Thinking { text, signature } => {
            if opts.strip_thinking {
                return vec![];
            }
            match to_api {
                // Anthropic natively supports thinking blocks.
                Api::AnthropicMessages => {
                    let th = ThinkingContent {
                        content_type: crate::ThinkingContentType::Thinking,
                        thinking: text.clone(),
                        thinking_signature: signature.clone(),
                        redacted: None,
                    };
                    vec![ContentBlock::Thinking(th)]
                }
                // All other providers: convert to text wrapped in tags.
                _ => {
                    let wrapped = format!("<thinking>\n{}\n</thinking>", text);
                    vec![ContentBlock::Text(TextContent {
                        content_type: TextContentType::Text,
                        text: wrapped,
                        text_signature: None,
                    })]
                }
            }
        }

        IntermediateBlock::Image { data, mime_type } => {
            if !opts.convert_images {
                return vec![];
            }
            vec![ContentBlock::Image(ImageContent {
                content_type: ImageContentType::Image,
                data: data.clone(),
                mime_type: mime_type.clone(),
            })]
        }

        IntermediateBlock::ToolCall {
            id,
            name,
            arguments,
        } => {
            if !opts.convert_tools {
                return vec![];
            }
            vec![ContentBlock::ToolCall(ToolCall {
                content_type: ToolCallType::ToolCall,
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                thought_signature: None,
            })]
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Merge adjacent [`ContentBlock::Text`] blocks into a single block.
fn merge_adjacent_text_blocks(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let mut result = Vec::with_capacity(blocks.len());
    let estimated_len = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text(t) => t.text.len() + 1,
            _ => 0,
        })
        .sum::<usize>();
    let mut pending = String::with_capacity(estimated_len.max(256));

    for block in blocks {
        match block {
            ContentBlock::Text(t) => {
                if !pending.is_empty() {
                    pending.push('\n');
                }
                pending.push_str(&t.text);
            }
            other => {
                if !pending.is_empty() {
                    result.push(ContentBlock::Text(TextContent {
                        content_type: TextContentType::Text,
                        text: std::mem::take(&mut pending),
                        text_signature: None,
                    }));
                }
                result.push(other);
            }
        }
    }

    if !pending.is_empty() {
        result.push(ContentBlock::Text(TextContent {
            content_type: TextContentType::Text,
            text: pending,
            text_signature: None,
        }));
    }

    result
}

// ---------------------------------------------------------------------------
// Message normalization (ported from transform-messages.ts)
// ---------------------------------------------------------------------------

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str =
    "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// Replace image content blocks with a text placeholder.
/// Collapses consecutive placeholders into one.
fn replace_images_with_placeholder(
    blocks: &[ContentBlock],
    placeholder: &str,
) -> Vec<ContentBlock> {
    let mut result = Vec::with_capacity(blocks.len());
    let mut prev_was_placeholder = false;

    for block in blocks {
        if matches!(block, ContentBlock::Image(_)) {
            if !prev_was_placeholder {
                result.push(ContentBlock::Text(TextContent::new(placeholder)));
            }
            prev_was_placeholder = true;
            continue;
        }

        result.push(block.clone());
        prev_was_placeholder = matches!(block, ContentBlock::Text(t) if t.text == placeholder);
    }

    result
}

/// Downgrade images to text placeholders when the target model does not support vision.
fn downgrade_unsupported_images(messages: &[Message], model: &Model) -> Vec<Message> {
    if model.input.contains(&InputModality::Image) {
        return messages.to_vec();
    }

    messages
        .iter()
        .map(|msg| match msg {
            Message::User(u) => match &u.content {
                MessageContent::Blocks(blocks) => {
                    let replaced =
                        replace_images_with_placeholder(blocks, NON_VISION_USER_IMAGE_PLACEHOLDER);
                    Message::User(crate::UserMessage {
                        role: u.role,
                        content: MessageContent::Blocks(replaced),
                        timestamp: u.timestamp,
                    })
                }
                _ => msg.clone(),
            },
            Message::ToolResult(t) => {
                let replaced =
                    replace_images_with_placeholder(&t.content, NON_VISION_TOOL_IMAGE_PLACEHOLDER);
                Message::ToolResult(ToolResultMessage {
                    role: t.role,
                    tool_call_id: t.tool_call_id.clone(),
                    tool_name: t.tool_name.clone(),
                    content: replaced,
                    details: t.details.clone(),
                    is_error: t.is_error,
                    timestamp: t.timestamp,
                })
            }
            _ => msg.clone(),
        })
        .collect()
}

/// Normalize a tool call ID for cross-provider compatibility.
///
/// Delegates to [`crate::utils::normalize_tool_call_id`].
pub fn normalize_tool_call_id(id: &str) -> String {
    crate::utils::normalize_tool_call_id(id)
}

/// Transform messages for a target model, handling:
/// - Image downgrade for non-vision models
/// - Thinking block conversion (cross-model → text, same-model → keep)
/// - Tool call ID normalization
/// - Tool call thought_signature stripping for cross-model
/// - Skipping error/aborted assistant messages
/// - Inserting synthetic tool results for orphaned tool calls
///
/// This is the main entry-point matching the behaviour of `transformMessages` in
/// `transform-messages.ts`.
pub fn transform_messages_for_model(messages: &[Message], model: &Model) -> Vec<Message> {
    let image_aware = downgrade_unsupported_images(messages, model);

    // Build a map of original tool call IDs → normalized IDs
    let mut tool_call_id_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // First pass: transform content blocks
    let transformed: Vec<Message> = image_aware
        .iter()
        .map(|msg| match msg {
            Message::User(_) => msg.clone(),

            Message::ToolResult(t) => {
                if let Some(normalized) = tool_call_id_map.get(&t.tool_call_id) {
                    if normalized != &t.tool_call_id {
                        return Message::ToolResult(ToolResultMessage {
                            tool_call_id: normalized.clone(),
                            ..t.clone()
                        });
                    }
                }
                msg.clone()
            }

            Message::Assistant(a) => {
                let is_same_model = a.provider == model.provider
                    && a.api == model.api
                    && a.model == model.id;

                let new_content: Vec<ContentBlock> = a
                    .content
                    .iter()
                    .flat_map(|block| match block {
                        ContentBlock::Thinking(th) => {
                            // Redacted thinking: only valid for same model
                            if th.redacted == Some(true) && !is_same_model {
                                return vec![];
                            }
                            // Same model: keep thinking with signatures
                            if is_same_model && th.thinking_signature.is_some() {
                                return vec![block.clone()];
                            }
                            // Skip empty thinking
                            if th.thinking.trim().is_empty() {
                                return vec![];
                            }
                            // Same model: keep
                            if is_same_model {
                                return vec![block.clone()];
                            }
                            // Cross-model: convert to text
                            vec![ContentBlock::Text(TextContent::new(&th.thinking))]
                        }

                        ContentBlock::Text(_) => vec![block.clone()],

                        ContentBlock::ToolCall(tc) => {
                            let mut new_tc = tc.clone();

                            // Strip thought_signature for cross-model
                            if !is_same_model && tc.thought_signature.is_some() {
                                new_tc.thought_signature = None;
                            }

                            // Normalize tool call ID for cross-model
                            if !is_same_model {
                                let normalized = normalize_tool_call_id(&tc.id);
                                if normalized != tc.id {
                                    tool_call_id_map
                                        .insert(tc.id.clone(), normalized.clone());
                                    new_tc.id = normalized;
                                }
                            }

                            vec![ContentBlock::ToolCall(new_tc)]
                        }

                        _ => vec![block.clone()],
                    })
                    .collect();

                Message::Assistant(AssistantMessage {
                    content: new_content,
                    ..a.clone()
                })
            }
        })
        .collect();

    // Second pass: insert synthetic tool results for orphaned tool calls and
    // skip error/aborted assistant messages.
    let mut result: Vec<Message> = Vec::with_capacity(transformed.len());
    let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
    let mut existing_tool_result_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    let insert_synthetic_results =
        |pending: &mut Vec<ToolCall>,
         existing: &mut std::collections::HashSet<String>,
         out: &mut Vec<Message>| {
            for tc in pending.drain(..) {
                if !existing.contains(&tc.id) {
                    out.push(Message::ToolResult(ToolResultMessage {
                        role: crate::ToolResultRole::ToolResult,
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        content: vec![ContentBlock::Text(TextContent::new(
                            "No result provided",
                        ))],
                        details: None,
                        is_error: true,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    }));
                }
            }
            existing.clear();
        };

    for msg in &transformed {
        match msg {
            Message::Assistant(a) => {
                // Insert synthetic results for any pending orphaned tool calls
                insert_synthetic_results(
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                    &mut result,
                );

                // Skip errored/aborted assistant messages
                if a.stop_reason == StopReason::Error || a.stop_reason == StopReason::Aborted {
                    continue;
                }

                // Track tool calls from this assistant message
                let tool_calls: Vec<&ToolCall> = a
                    .content
                    .iter()
                    .filter_map(|b| b.as_tool_call())
                    .collect();
                if !tool_calls.is_empty() {
                    pending_tool_calls = tool_calls.into_iter().cloned().collect();
                    existing_tool_result_ids.clear();
                }

                result.push(msg.clone());
            }

            Message::ToolResult(t) => {
                existing_tool_result_ids.insert(t.tool_call_id.clone());
                result.push(msg.clone());
            }

            Message::User(_) => {
                // User message interrupts tool flow — insert synthetic results for orphans
                insert_synthetic_results(
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                    &mut result,
                );
                result.push(msg.clone());
            }
        }
    }

    // Handle trailing unresolved tool calls
    insert_synthetic_results(
        &mut pending_tool_calls,
        &mut existing_tool_result_ids,
        &mut result,
    );

    result
}

// ---------------------------------------------------------------------------
// Convenience directional converters
// ---------------------------------------------------------------------------

/// Convert Anthropic-format messages to OpenAI format.
pub fn anthropic_to_openai(messages: &[Message]) -> Vec<Message> {
    transform_messages(
        messages,
        Api::AnthropicMessages,
        Api::OpenAiCompletions,
        TransformOptions::default(),
    )
}

/// Convert OpenAI-format messages to Anthropic format.
pub fn openai_to_anthropic(messages: &[Message]) -> Vec<Message> {
    transform_messages(
        messages,
        Api::OpenAiCompletions,
        Api::AnthropicMessages,
        TransformOptions::default(),
    )
}

/// Convert Google-format messages to OpenAI format.
pub fn google_to_openai(messages: &[Message]) -> Vec<Message> {
    transform_messages(
        messages,
        Api::GoogleGenerativeAi,
        Api::OpenAiCompletions,
        TransformOptions::default(),
    )
}

/// Convert Anthropic-format messages to Google format.
pub fn anthropic_to_google(messages: &[Message]) -> Vec<Message> {
    transform_messages(
        messages,
        Api::AnthropicMessages,
        Api::GoogleGenerativeAi,
        TransformOptions::default(),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UserMessage;

    /// Helper: create a simple user text message.
    fn user_msg(text: &str) -> Message {
        Message::User(UserMessage::new(text))
    }

    /// Helper: create an assistant message with given content blocks and source API.
    fn assistant_msg(api: Api, provider: &str, model: &str, blocks: Vec<ContentBlock>) -> Message {
        let mut msg = AssistantMessage::new(api, provider, model);
        msg.content = blocks;
        Message::Assistant(msg)
    }

    /// Helper: create a tool result message.
    fn tool_result_msg(tool_call_id: &str, tool_name: &str, text: &str) -> Message {
        Message::ToolResult(ToolResultMessage::new(
            tool_call_id,
            tool_name,
            vec![ContentBlock::Text(TextContent::new(text))],
        ))
    }

    // ---- Test 1: Anthropic → OpenAI basic text ----

    #[test]
    fn test_anthropic_to_openai_text() {
        let msgs = vec![
            user_msg("Hello"),
            assistant_msg(
                Api::AnthropicMessages,
                "anthropic",
                "claude-3.5-sonnet",
                vec![ContentBlock::Text(TextContent::new("Hi there!"))],
            ),
        ];

        let result = anthropic_to_openai(&msgs);
        assert_eq!(result.len(), 2);

        // User message preserved
        match &result[0] {
            Message::User(u) => assert_eq!(u.content.as_str(), Some("Hello")),
            _ => panic!("Expected User message"),
        }

        // Assistant message converted
        match &result[1] {
            Message::Assistant(a) => {
                assert_eq!(a.api, Api::OpenAiCompletions);
                assert_eq!(a.text_content(), "Hi there!");
            }
            _ => panic!("Expected Assistant message"),
        }
    }

    // ---- Test 2: Thinking block conversion (Anthropic → OpenAI) ----

    #[test]
    fn test_thinking_block_anthropic_to_openai() {
        let msgs = vec![assistant_msg(
            Api::AnthropicMessages,
            "anthropic",
            "claude-3.5-sonnet",
            vec![
                ContentBlock::Thinking(ThinkingContent::new("Let me think...")),
                ContentBlock::Text(TextContent::new("Here's the answer.")),
            ],
        )];

        let result = anthropic_to_openai(&msgs);
        match &result[0] {
            Message::Assistant(a) => {
                // Thinking should be wrapped in tags, text preserved
                let text = a.text_content();
                assert!(text.contains("<thinking>"));
                assert!(text.contains("Let me think..."));
                assert!(text.contains("Here's the answer."));
                // No native thinking blocks left
                assert!(!a
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Thinking(_))));
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 3: Thinking block strip option ----

    #[test]
    fn test_thinking_block_stripped() {
        let msgs = vec![assistant_msg(
            Api::AnthropicMessages,
            "anthropic",
            "claude-3.5-sonnet",
            vec![
                ContentBlock::Thinking(ThinkingContent::new("Internal thought")),
                ContentBlock::Text(TextContent::new("Final answer.")),
            ],
        )];

        let opts = TransformOptions {
            strip_thinking: true,
            ..Default::default()
        };
        let result =
            transform_messages(&msgs, Api::AnthropicMessages, Api::OpenAiCompletions, opts);

        match &result[0] {
            Message::Assistant(a) => {
                // Thinking should be completely removed
                assert_eq!(a.content.len(), 1);
                assert_eq!(a.text_content(), "Final answer.");
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 4: Tool call preservation ----

    #[test]
    fn test_tool_calls_preserved() {
        let tool_call = ContentBlock::ToolCall(ToolCall::new(
            "call_123",
            "get_weather",
            serde_json::json!({"city": "Tokyo"}),
        ));

        let msgs = vec![
            assistant_msg(
                Api::AnthropicMessages,
                "anthropic",
                "claude-3.5-sonnet",
                vec![
                    ContentBlock::Text(TextContent::new("Let me check.")),
                    tool_call,
                ],
            ),
            tool_result_msg("call_123", "get_weather", "Sunny, 22°C"),
        ];

        let result = anthropic_to_openai(&msgs);

        // Assistant message should still have the tool call
        match &result[0] {
            Message::Assistant(a) => {
                let tc = a.content.iter().find_map(|b| b.as_tool_call());
                assert!(tc.is_some(), "Tool call should be preserved");
                let tc = tc.unwrap();
                assert_eq!(tc.id, "call_123");
                assert_eq!(tc.name, "get_weather");
            }
            _ => panic!("Expected Assistant"),
        }

        // Tool result preserved
        match &result[1] {
            Message::ToolResult(t) => {
                assert_eq!(t.tool_call_id, "call_123");
                assert_eq!(t.tool_name, "get_weather");
            }
            _ => panic!("Expected ToolResult"),
        }
    }

    // ---- Test 5: Tool calls dropped when convert_tools = false ----

    #[test]
    fn test_tool_calls_dropped_with_option() {
        let msgs = vec![assistant_msg(
            Api::AnthropicMessages,
            "anthropic",
            "claude-3.5-sonnet",
            vec![
                ContentBlock::Text(TextContent::new("I will call a tool.")),
                ContentBlock::ToolCall(ToolCall::new("tc_1", "search", serde_json::json!({}))),
            ],
        )];

        let opts = TransformOptions {
            convert_tools: false,
            ..Default::default()
        };
        let result =
            transform_messages(&msgs, Api::AnthropicMessages, Api::OpenAiCompletions, opts);

        match &result[0] {
            Message::Assistant(a) => {
                assert_eq!(a.content.len(), 1);
                assert_eq!(a.text_content(), "I will call a tool.");
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 6: Image block conversion ----

    #[test]
    fn test_image_block_conversion() {
        let msgs = vec![assistant_msg(
            Api::AnthropicMessages,
            "anthropic",
            "claude-3.5-sonnet",
            vec![
                ContentBlock::Text(TextContent::new("Here's the image:")),
                ContentBlock::Image(ImageContent::new("iVBORw0KGgo=", "image/png")),
            ],
        )];

        let result = anthropic_to_openai(&msgs);

        match &result[0] {
            Message::Assistant(a) => {
                let has_text = a.content.iter().any(|b| matches!(b, ContentBlock::Text(_)));
                let has_image = a
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Image(_)));
                assert!(has_text, "Text block should be preserved");
                assert!(has_image, "Image block should be preserved");
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 7: OpenAI → Anthropic round-trip preserves text ----

    #[test]
    fn test_openai_to_anthropic_roundtrip() {
        let original = vec![
            user_msg("What is 2+2?"),
            assistant_msg(
                Api::OpenAiCompletions,
                "openai",
                "gpt-4o",
                vec![ContentBlock::Text(TextContent::new("The answer is 4."))],
            ),
        ];

        let to_anthropic = openai_to_anthropic(&original);
        let back_to_openai = anthropic_to_openai(&to_anthropic);

        // Text content should survive the round trip
        match (&original[1], &back_to_openai[1]) {
            (Message::Assistant(orig), Message::Assistant(rt)) => {
                assert_eq!(orig.text_content(), rt.text_content());
            }
            _ => panic!("Expected Assistant messages"),
        }
    }

    // ---- Test 8: Google → OpenAI ----

    #[test]
    fn test_google_to_openai() {
        let msgs = vec![
            user_msg("Summarize this"),
            assistant_msg(
                Api::GoogleGenerativeAi,
                "google",
                "gemini-2.0-flash",
                vec![ContentBlock::Text(TextContent::new("Here's a summary."))],
            ),
        ];

        let result = google_to_openai(&msgs);
        assert_eq!(result.len(), 2);

        match &result[1] {
            Message::Assistant(a) => {
                assert_eq!(a.api, Api::OpenAiCompletions);
                assert_eq!(a.text_content(), "Here's a summary.");
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 9: Same-API is a no-op clone ----

    #[test]
    fn test_same_api_noop() {
        let msgs = vec![user_msg("Hello")];
        let result = transform_messages(
            &msgs,
            Api::AnthropicMessages,
            Api::AnthropicMessages,
            TransformOptions::default(),
        );
        assert_eq!(result.len(), 1);
        match &result[0] {
            Message::User(u) => assert_eq!(u.content.as_str(), Some("Hello")),
            _ => panic!("Expected User"),
        }
    }

    // ---- Test 10: Thinking preserved when target is Anthropic ----

    #[test]
    fn test_thinking_preserved_for_anthropic_target() {
        let msgs = vec![assistant_msg(
            Api::OpenAiCompletions,
            "openai",
            "gpt-4o",
            vec![
                ContentBlock::Thinking(ThinkingContent::new("Reasoning...")),
                ContentBlock::Text(TextContent::new("Answer.")),
            ],
        )];

        let result = openai_to_anthropic(&msgs);

        match &result[0] {
            Message::Assistant(a) => {
                // Thinking block should be preserved natively for Anthropic
                let has_thinking = a
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Thinking(_)));
                assert!(
                    has_thinking,
                    "Thinking block should be preserved for Anthropic"
                );
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 11: Anthropic → Google converts thinking to text ----

    #[test]
    fn test_anthropic_to_google_thinking() {
        let msgs = vec![assistant_msg(
            Api::AnthropicMessages,
            "anthropic",
            "claude-3.5-sonnet",
            vec![
                ContentBlock::Thinking(ThinkingContent::new("Deep thought")),
                ContentBlock::Text(TextContent::new("Result.")),
            ],
        )];

        let result = anthropic_to_google(&msgs);

        match &result[0] {
            Message::Assistant(a) => {
                // No native thinking blocks for Google
                let has_thinking = a
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Thinking(_)));
                assert!(
                    !has_thinking,
                    "Google target should not have thinking blocks"
                );
                // Text should contain wrapped thinking
                let text = a.text_content();
                assert!(text.contains("<thinking>"));
                assert!(text.contains("Deep thought"));
                assert!(text.contains("Result."));
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 12: Full conversation with mixed blocks ----

    #[test]
    fn test_full_conversation_mixed_blocks() {
        let msgs = vec![
            user_msg("What's the weather in Paris?"),
            assistant_msg(
                Api::AnthropicMessages,
                "anthropic",
                "claude-3.5-sonnet",
                vec![
                    ContentBlock::Thinking(ThinkingContent::new("User wants weather.")),
                    ContentBlock::Text(TextContent::new("Let me check.")),
                    ContentBlock::ToolCall(ToolCall::new(
                        "tc_001",
                        "get_weather",
                        serde_json::json!({"location": "Paris"}),
                    )),
                ],
            ),
            tool_result_msg("tc_001", "get_weather", "Rainy, 15°C"),
            assistant_msg(
                Api::AnthropicMessages,
                "anthropic",
                "claude-3.5-sonnet",
                vec![ContentBlock::Text(TextContent::new(
                    "It's rainy and 15°C in Paris.",
                ))],
            ),
        ];

        let result = anthropic_to_openai(&msgs);
        assert_eq!(result.len(), 4, "All 4 messages should be preserved");

        // First assistant: thinking converted + tool call preserved
        match &result[1] {
            Message::Assistant(a) => {
                let has_tool = a
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolCall(_)));
                assert!(has_tool, "Tool call should be preserved");
                let has_thinking = a
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Thinking(_)));
                assert!(
                    !has_thinking,
                    "Thinking should be converted to text for OpenAI"
                );
            }
            _ => panic!("Expected Assistant"),
        }

        // Tool result preserved
        match &result[2] {
            Message::ToolResult(t) => {
                assert_eq!(t.tool_call_id, "tc_001");
            }
            _ => panic!("Expected ToolResult"),
        }

        // Final assistant: pure text
        match &result[3] {
            Message::Assistant(a) => {
                assert_eq!(a.text_content(), "It's rainy and 15°C in Paris.");
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 13: Images dropped when convert_images = false ----

    #[test]
    fn test_images_dropped_with_option() {
        let msgs = vec![Message::User(UserMessage::new(vec![
            ContentBlock::Text(TextContent::new("Describe this:")),
            ContentBlock::Image(ImageContent::new("AAAA", "image/jpeg")),
        ]))];

        let opts = TransformOptions {
            convert_images: false,
            ..Default::default()
        };
        let result =
            transform_messages(&msgs, Api::AnthropicMessages, Api::OpenAiCompletions, opts);

        match &result[0] {
            Message::User(u) => match &u.content {
                MessageContent::Blocks(blocks) => {
                    let has_image = blocks.iter().any(|b| matches!(b, ContentBlock::Image(_)));
                    assert!(!has_image, "Image should be dropped");
                    assert_eq!(blocks.len(), 1);
                }
                _ => panic!("Expected blocks"),
            },
            _ => panic!("Expected User"),
        }
    }

    // ---- Test 14: Assistant metadata preserved through transform ----

    #[test]
    fn test_assistant_metadata_preserved() {
        let mut a = AssistantMessage::new(Api::AnthropicMessages, "anthropic", "claude-3.5-sonnet");
        a.content = vec![ContentBlock::Text(TextContent::new("Hi"))];
        a.usage = Usage {
            input: 100,
            output: 50,
            cache_read: 10,
            cache_write: 5,
            total_tokens: 165,
            cost: Default::default(),
        };
        a.stop_reason = StopReason::Stop;
        a.error_message = None;
        a.response_id = Some("msg_abc123".to_string());
        let original_ts = a.timestamp;

        let msgs = vec![Message::Assistant(a)];
        let result = anthropic_to_openai(&msgs);

        match &result[0] {
            Message::Assistant(a) => {
                assert_eq!(a.usage.input, 100);
                assert_eq!(a.usage.output, 50);
                assert_eq!(a.stop_reason, StopReason::Stop);
                assert_eq!(a.response_id, Some("msg_abc123".to_string()));
                assert_eq!(a.timestamp, original_ts);
                assert_eq!(a.api, Api::OpenAiCompletions);
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Test 15: Error tool result preserved ----

    #[test]
    fn test_error_tool_result_preserved() {
        let err = ToolResultMessage::error("tc_err", "failing_tool", "Something went wrong");
        let msgs = vec![Message::ToolResult(err)];

        let result = anthropic_to_openai(&msgs);
        match &result[0] {
            Message::ToolResult(t) => {
                assert!(t.is_error);
                assert_eq!(t.tool_call_id, "tc_err");
                assert_eq!(t.tool_name, "failing_tool");
            }
            _ => panic!("Expected ToolResult"),
        }
    }
}
