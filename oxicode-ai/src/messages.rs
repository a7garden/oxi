//! Message types for oxicode-ai

use crate::Api;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Text content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContent {
    /// Discriminator for untagged deserialization.
    #[serde(rename = "type")]
    pub content_type: TextContentType,
    /// The text payload.
    pub text: String,
    /// Optional signature carrying provider-specific metadata (e.g. OpenAI message ID, phase).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "text")]
/// TextContentType.
pub enum TextContentType {
    /// text variant.
    Text,
}

impl TextContent {
    /// Create a plain text content block.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            content_type: TextContentType::Text,
            text: text.into(),
            text_signature: None,
        }
    }

    /// Create a text content block with a signature.
    pub fn with_signature(text: impl Into<String>, signature: impl Into<String>) -> Self {
        Self {
            content_type: TextContentType::Text,
            text: text.into(),
            text_signature: Some(signature.into()),
        }
    }
}

/// Thinking content block (extended thinking / chain-of-thought output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingContent {
    /// Discriminator for untagged deserialization.
    #[serde(rename = "type")]
    pub content_type: ThinkingContentType,
    /// The raw thinking text from the model.
    pub thinking: String,
    /// Optional provider-specific signature for the thinking block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    /// Whether the thinking content was redacted by the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "thinking")]
/// ThinkingContentType.
pub enum ThinkingContentType {
    /// thinking variant.
    Thinking,
}

impl ThinkingContent {
    /// Create a new thinking content block.
    pub fn new(thinking: impl Into<String>) -> Self {
        Self {
            content_type: ThinkingContentType::Thinking,
            thinking: thinking.into(),
            thinking_signature: None,
            redacted: None,
        }
    }
}

/// Image content block (base64-encoded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    /// Discriminator for untagged deserialization.
    #[serde(rename = "type")]
    pub content_type: ImageContentType,
    /// Base64-encoded image data.
    pub data: String,
    /// MIME type of the image (e.g. `"image/png"`).
    pub mime_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "image")]
/// ImageContentType.
pub enum ImageContentType {
    /// image variant.
    Image,
}

impl ImageContent {
    /// Create a new image content block.
    pub fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            content_type: ImageContentType::Image,
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }
}

/// Tool call content block emitted by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Discriminator for untagged deserialization.
    #[serde(rename = "type")]
    pub content_type: ToolCallType,
    /// Provider-assigned tool call identifier.
    pub id: String,
    /// Name of the tool being invoked.
    pub name: String,
    /// JSON arguments for the tool invocation.
    pub arguments: JsonValue,
    /// Optional provider-specific signature linking to a thinking block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "toolCall")]
/// ToolCallType.
pub enum ToolCallType {
    /// tool call variant.
    ToolCall,
}

impl ToolCall {
    /// Create a new tool call.
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: JsonValue) -> Self {
        Self {
            content_type: ToolCallType::ToolCall,
            id: id.into(),
            name: name.into(),
            arguments,
            thought_signature: None,
        }
    }
}

/// Content block union (untagged for flexibility).
///
/// Represents a single piece of content inside a message – text, thinking,
/// an image, a tool call, or an unrecognized block kept as raw JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentBlock {
    /// Plain text content.
    Text(TextContent),
    /// Extended thinking / chain-of-thought output.
    Thinking(ThinkingContent),
    /// Base64-encoded image.
    Image(ImageContent),
    /// Tool invocation requested by the model.
    ToolCall(ToolCall),
    /// Unrecognised block preserved as raw JSON.
    Unknown(JsonValue),
}

impl ContentBlock {
    /// Returns the inner text if this is a `Text` block.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text(t) => Some(&t.text),
            _ => None,
        }
    }

    /// Returns a reference to the `ToolCall` if this is a `ToolCall` block.
    pub fn as_tool_call(&self) -> Option<&ToolCall> {
        match self {
            ContentBlock::ToolCall(t) => Some(t),
            _ => None,
        }
    }

    /// Returns a reference to the `ThinkingContent` if this is a `Thinking` block.
    pub fn as_thinking(&self) -> Option<&ThinkingContent> {
        match self {
            ContentBlock::Thinking(t) => Some(t),
            _ => None,
        }
    }
}

/// User message sent to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    /// Role discriminator (always `UserRole::User`).
    pub role: UserRole,
    /// Message content – either a plain string or a list of content blocks.
    pub content: MessageContent,
    /// Unix-epoch milliseconds when the message was created.
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "user")]
/// UserRole.
pub enum UserRole {
    #[serde(rename = "user")]
    /// user variant.
    User,
}

impl UserMessage {
    /// Create a new user message with the current timestamp.
    pub fn new(content: impl Into<MessageContent>) -> Self {
        Self {
            role: UserRole::User,
            content: content.into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}

/// Assistant message returned by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// Role discriminator (always `AssistantRole::Assistant`).
    pub role: AssistantRole,
    /// Ordered content blocks (text, thinking, tool calls, images).
    pub content: Vec<ContentBlock>,
    /// API dialect that produced this message.
    pub api: super::Api,
    /// Provider name (e.g. `"anthropic"`, `"openai"`).
    pub provider: String,
    /// Model identifier string.
    pub model: String,
    /// Token usage statistics.
    pub usage: super::Usage,
    /// Why the model stopped generating.
    pub stop_reason: super::StopReason,
    /// Non-fatal error message if the provider returned a partial error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Provider-assigned response ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    /// Unix-epoch milliseconds when the message was created.
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "assistant")]
/// AssistantRole.
pub enum AssistantRole {
    #[serde(rename = "assistant")]
    /// assistant variant.
    Assistant,
}

impl AssistantMessage {
    /// Create a new assistant message with the current timestamp.
    pub fn new(api: super::Api, provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            role: AssistantRole::Assistant,
            content: Vec::new(),
            api,
            provider: provider.into(),
            model: model.into(),
            usage: super::Usage::default(),
            stop_reason: super::StopReason::Stop,
            error_message: None,
            response_id: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Concatenate all `Text` blocks into a single string.
    pub fn text_content(&self) -> String {
        // Pre-compute capacity to avoid reallocations.
        let estimated_len: usize = self
            .content
            .iter()
            .map(|b| b.as_text().map(|t| t.len()).unwrap_or(0))
            .sum();
        let mut result = String::with_capacity(estimated_len);
        for block in &self.content {
            if let Some(text) = block.as_text() {
                result.push_str(text);
            }
        }
        result
    }
}

/// Tool result message carrying the output of a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    /// Role discriminator (always `ToolResultRole::ToolResult`).
    pub role: ToolResultRole,
    /// Matches the `ToolCall::id` this result is for.
    pub tool_call_id: String,
    /// Name of the tool that was executed.
    pub tool_name: String,
    /// Result content blocks.
    pub content: Vec<ContentBlock>,
    /// Optional structured details about the result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<JsonValue>,
    /// Whether this result represents an error.
    #[serde(default)]
    pub is_error: bool,
    /// Unix-epoch milliseconds when the message was created.
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "toolResult")]
/// ToolResultRole.
pub enum ToolResultRole {
    #[serde(rename = "toolResult")]
    /// tool result variant.
    ToolResult,
}

impl ToolResultMessage {
    /// Create a successful tool result.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: Vec<ContentBlock>,
    ) -> Self {
        Self {
            role: ToolResultRole::ToolResult,
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content,
            details: None,
            is_error: false,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create an error tool result.
    pub fn error(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            role: ToolResultRole::ToolResult,
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: vec![ContentBlock::Text(TextContent::new(error))],
            details: None,
            is_error: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Render all content blocks into a human-readable string.
    pub fn text_content(&self) -> Result<String, crate::error::ProviderError> {
        // Pre-compute capacity estimate.
        let estimated_len: usize = self
            .content
            .iter()
            .map(|b| match b {
                ContentBlock::Text(t) => t.text.len() + 1,
                ContentBlock::Image(_) => 7,
                ContentBlock::Thinking(t) => t.thinking.len() + 12,
                ContentBlock::ToolCall(tc) => tc.name.len() + 8,
                ContentBlock::Unknown(_) => 0,
            })
            .sum();
        let mut result = String::with_capacity(estimated_len);
        for block in &self.content {
            match block {
                ContentBlock::Text(t) => {
                    result.push_str(&t.text);
                    result.push('\n');
                }
                ContentBlock::Image(_) => {
                    result.push_str("[Image]\n");
                }
                ContentBlock::Thinking(t) => {
                    result.push_str(&format!("[Thinking: {}]\n", t.thinking));
                }
                ContentBlock::ToolCall(tc) => {
                    result.push_str(&format!("[Tool: {}]\n", tc.name));
                }
                ContentBlock::Unknown(_) => {
                    // Skip unknown blocks
                }
            }
        }
        Ok(result.trim().to_string())
    }
}

/// Message union tagged by role.
///
/// Every conversation turn is one of: a [`UserMessage`], an [`AssistantMessage`],
/// or a [`ToolResultMessage`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    /// A message from the user.
    User(UserMessage),
    /// A response from the assistant.
    Assistant(AssistantMessage),
    /// The output of a tool invocation.
    ToolResult(ToolResultMessage),
}

impl Message {
    /// Convenience constructor for a user text message.
    pub fn user(content: impl Into<MessageContent>) -> Self {
        Message::User(UserMessage::new(content))
    }

    /// Convenience constructor for an assistant message.
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Message::Assistant(AssistantMessage {
            role: AssistantRole::Assistant,
            content,
            api: Api::AnthropicMessages,
            provider: "assistant".to_string(),
            model: "assistant".to_string(),
            usage: super::Usage::default(),
            stop_reason: super::StopReason::Stop,
            error_message: None,
            response_id: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        })
    }

    /// Convenience constructor for a tool result message.
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: Vec<ContentBlock>,
    ) -> Self {
        Message::ToolResult(ToolResultMessage::new(tool_call_id, tool_name, content))
    }

    /// Return the timestamp (milliseconds since epoch) of this message.
    pub fn timestamp(&self) -> i64 {
        match self {
            Message::User(m) => m.timestamp,
            Message::Assistant(m) => m.timestamp,
            Message::ToolResult(m) => m.timestamp,
        }
    }

    /// Get the text content of this message
    pub fn text_content(&self) -> Result<String, crate::error::ProviderError> {
        match self {
            Message::User(m) => match &m.content {
                MessageContent::Text(s) => Ok(s.clone()),
                MessageContent::Blocks(blocks) => {
                    let estimated_len: usize = blocks
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text(t) => t.text.len() + 1,
                            ContentBlock::Image(_) => 8,
                            ContentBlock::Thinking(t) => t.thinking.len() + 1,
                            ContentBlock::ToolCall(_) => 12,
                            ContentBlock::Unknown(_) => 10,
                        })
                        .sum();
                    let mut result = String::with_capacity(estimated_len);
                    for block in blocks {
                        match block {
                            ContentBlock::Text(t) => {
                                result.push_str(&t.text);
                                result.push('\n');
                            }
                            ContentBlock::Image(_) => {
                                result.push_str("[Image]\n");
                            }
                            ContentBlock::Thinking(t) => {
                                result.push_str(&t.thinking);
                                result.push('\n');
                            }
                            ContentBlock::ToolCall(_) => {
                                result.push_str("[Tool Call]\n");
                            }
                            ContentBlock::Unknown(_) => {
                                result.push_str("[Unknown]\n");
                            }
                        }
                    }
                    Ok(result.trim().to_string())
                }
            },
            Message::Assistant(m) => Ok(m.text_content()),
            Message::ToolResult(m) => m.text_content(),
        }
    }
}

/// Message content – either a plain text string or a list of structured blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// A simple text string.
    Text(String),
    /// One or more content blocks.
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Returns `true` if this is a `Text` variant.
    pub fn is_text(&self) -> bool {
        matches!(self, MessageContent::Text(_))
    }

    /// Returns the inner `&str` if this is a `Text` variant.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s),
            MessageContent::Blocks(_) => None,
        }
    }
}

// String conversion for MessageContent
impl From<String> for MessageContent {
    fn from(text: String) -> Self {
        MessageContent::Text(text)
    }
}

impl From<&str> for MessageContent {
    fn from(text: &str) -> Self {
        MessageContent::Text(text.to_string())
    }
}

impl From<Vec<ContentBlock>> for MessageContent {
    fn from(blocks: Vec<ContentBlock>) -> Self {
        MessageContent::Blocks(blocks)
    }
}

impl From<TextContent> for MessageContent {
    fn from(block: TextContent) -> Self {
        MessageContent::Blocks(vec![ContentBlock::Text(block)])
    }
}

impl From<ContentBlock> for MessageContent {
    fn from(block: ContentBlock) -> Self {
        MessageContent::Blocks(vec![block])
    }
}

/// Transform messages for cross-provider compatibility.
///
/// When switching models mid-conversation, message history may contain
/// provider-specific content (e.g. thinking blocks from Anthropic) that
/// the new provider cannot handle. This function converts messages so
/// they are compatible with the target provider's API.
///
/// Key transformations:
/// - Thinking blocks → wrapped in `<thinking>` tags as plain text
/// - Tool calls and tool results are preserved unchanged
/// - User/assistant message structure is preserved
pub fn transform_for_provider(
    messages: &[Message],
    _from_api: &super::Api,
    to_api: &super::Api,
) -> Vec<Message> {
    messages
        .iter()
        .map(|msg| match msg {
            Message::Assistant(a) => {
                let mut new_msg = AssistantMessage::new(*to_api, &a.provider, &a.model);
                new_msg.content = transform_content_blocks(&a.content, to_api);
                new_msg.usage = a.usage.clone();
                new_msg.stop_reason = a.stop_reason;
                new_msg.error_message = a.error_message.clone();
                new_msg.response_id = a.response_id.clone();
                new_msg.timestamp = a.timestamp;
                Message::Assistant(new_msg)
            }
            Message::User(u) => Message::User(u.clone()),
            Message::ToolResult(t) => Message::ToolResult(t.clone()),
        })
        .collect()
}

/// Transform content blocks for a target provider.
///
/// Converts provider-specific blocks (like thinking) into formats
/// the target provider can understand.
fn transform_content_blocks(blocks: &[ContentBlock], to_api: &super::Api) -> Vec<ContentBlock> {
    match to_api {
        // Anthropic natively supports thinking blocks — keep as-is
        super::Api::AnthropicMessages => blocks.to_vec(),

        // OpenAI-compatible and other providers: convert thinking to text
        _ => {
            let mut transformed = Vec::with_capacity(blocks.len());
            for block in blocks {
                match block {
                    ContentBlock::Thinking(t) => {
                        // Convert thinking block to text wrapped in tags
                        let text = format!("<thinking>\n{}\n</thinking>", t.thinking);
                        transformed.push(ContentBlock::Text(TextContent::new(text)));
                    }
                    ContentBlock::Text(t) => {
                        transformed.push(ContentBlock::Text(t.clone()));
                    }
                    ContentBlock::ToolCall(tc) => {
                        transformed.push(ContentBlock::ToolCall(tc.clone()));
                    }
                    ContentBlock::Image(img) => {
                        transformed.push(ContentBlock::Image(img.clone()));
                    }
                    ContentBlock::Unknown(v) => {
                        // Try to extract text from unknown blocks
                        if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                            transformed.push(ContentBlock::Text(TextContent::new(text)));
                        }
                        // Otherwise silently drop unknown blocks
                    }
                }
            }
            // Merge adjacent text blocks
            merge_adjacent_text_blocks(transformed)
        }
    }
}

/// Merge adjacent `ContentBlock::Text` blocks into a single block.
fn merge_adjacent_text_blocks(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let mut result = Vec::with_capacity(blocks.len());
    let estimated_len = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text(t) => t.text.len() + 1,
            _ => 0,
        })
        .sum::<usize>();
    let mut pending_text = String::with_capacity(estimated_len.max(256));

    for block in blocks {
        match block {
            ContentBlock::Text(t) => {
                if !pending_text.is_empty() {
                    pending_text.push('\n');
                }
                pending_text.push_str(&t.text);
            }
            other => {
                if !pending_text.is_empty() {
                    result.push(ContentBlock::Text(TextContent::new(std::mem::take(
                        &mut pending_text,
                    ))));
                }
                result.push(other);
            }
        }
    }

    if !pending_text.is_empty() {
        result.push(ContentBlock::Text(TextContent::new(pending_text)));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Api, StopReason, Usage};

    // ---- ContentBlock serialization roundtrip ----

    #[test]
    fn text_content_roundtrip() {
        let block = ContentBlock::Text(TextContent::new("hello world"));
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_text(), Some("hello world"));
    }

    #[test]
    fn thinking_content_roundtrip() {
        let block = ContentBlock::Thinking(ThinkingContent::new("inner thoughts"));
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(back.as_thinking().is_some());
        assert_eq!(back.as_thinking().unwrap().thinking, "inner thoughts");
    }

    #[test]
    fn image_content_roundtrip() {
        let block = ContentBlock::Image(ImageContent::new("base64data==", "image/png"));
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            ContentBlock::Image(img) => {
                assert_eq!(img.data, "base64data==");
                assert_eq!(img.mime_type, "image/png");
            }
            _ => panic!("Expected Image block"),
        }
    }

    #[test]
    fn tool_call_roundtrip() {
        let block = ContentBlock::ToolCall(ToolCall::new(
            "call_123",
            "read_file",
            serde_json::json!({"path": "/foo.rs"}),
        ));
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        let tc = back.as_tool_call().unwrap();
        assert_eq!(tc.id, "call_123");
        assert_eq!(tc.name, "read_file");
        assert_eq!(tc.arguments["path"], "/foo.rs");
    }

    // ---- Inner message type roundtrip (Message enum has duplicate role key issue) ----

    #[test]
    fn user_message_inner_roundtrip() {
        let msg = UserMessage::new("Hello, assistant!");
        let json = serde_json::to_string(&msg).unwrap();
        let back: UserMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(&back.content, MessageContent::Text(s) if s == "Hello, assistant!"));
        assert_eq!(back.role, UserRole::User);
    }

    #[test]
    fn user_message_blocks_roundtrip() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("part one")),
            ContentBlock::Text(TextContent::new("part two")),
        ];
        let msg = UserMessage::new(MessageContent::Blocks(blocks));
        let json = serde_json::to_string(&msg).unwrap();
        let back: UserMessage = serde_json::from_str(&json).unwrap();
        match &back.content {
            MessageContent::Blocks(blocks) => assert_eq!(blocks.len(), 2),
            _ => panic!("Expected Blocks"),
        }
    }

    #[test]
    fn assistant_message_inner_roundtrip() {
        let mut msg = AssistantMessage::new(Api::AnthropicMessages, "anthropic", "claude-3");
        msg.content
            .push(ContentBlock::Text(TextContent::new("Hi!")));
        msg.content
            .push(ContentBlock::Thinking(ThinkingContent::new("hmm")));
        msg.usage = Usage {
            input: 100,
            output: 50,
            ..Default::default()
        };
        msg.stop_reason = StopReason::Stop;
        msg.response_id = Some("resp_abc".to_string());

        let json = serde_json::to_string(&msg).unwrap();
        let back: AssistantMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(back.content.len(), 2);
        assert_eq!(back.usage.input, 100);
        assert_eq!(back.response_id.as_deref(), Some("resp_abc"));
        assert_eq!(back.role, AssistantRole::Assistant);
    }

    #[test]
    fn tool_result_message_inner_roundtrip() {
        let msg = ToolResultMessage::new(
            "call_1",
            "bash",
            vec![ContentBlock::Text(TextContent::new("output"))],
        );
        let json = serde_json::to_string(&msg).unwrap();
        let back: ToolResultMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_call_id, "call_1");
        assert_eq!(back.tool_name, "bash");
        assert!(!back.is_error);
        assert_eq!(back.role, ToolResultRole::ToolResult);
    }

    #[test]
    fn message_construction_and_accessors() {
        let user = Message::user("test");
        assert!(matches!(user, Message::User(_)));

        let ts = user.timestamp();
        assert!(ts > 0);
    }

    #[test]
    fn message_content_roundtrip() {
        // Text variant
        let mc = MessageContent::Text("hello".to_string());
        let json = serde_json::to_string(&mc).unwrap();
        let back: MessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_str(), Some("hello"));

        // Blocks variant
        let mc = MessageContent::Blocks(vec![ContentBlock::Text(TextContent::new("block"))]);
        let json = serde_json::to_string(&mc).unwrap();
        let back: MessageContent = serde_json::from_str(&json).unwrap();
        assert!(!back.is_text());
    }

    // ---- text_content() ----

    #[test]
    fn user_text_content() {
        let msg = Message::user("Hello!");
        assert_eq!(msg.text_content().unwrap(), "Hello!");
    }

    #[test]
    fn user_blocks_text_content() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("line 1")),
            ContentBlock::Text(TextContent::new("line 2")),
        ];
        let msg = Message::User(UserMessage::new(MessageContent::Blocks(blocks)));
        assert_eq!(msg.text_content().unwrap(), "line 1\nline 2");
    }

    #[test]
    fn assistant_text_content() {
        let mut a = AssistantMessage::new(Api::OpenAiCompletions, "openai", "gpt-4");
        a.content
            .push(ContentBlock::Text(TextContent::new("part A")));
        a.content
            .push(ContentBlock::Thinking(ThinkingContent::new("hidden")));
        a.content
            .push(ContentBlock::Text(TextContent::new("part B")));

        let msg = Message::Assistant(a);
        let text = msg.text_content().unwrap();
        // text_content on assistant only returns Text blocks
        assert_eq!(text, "part Apart B");
    }

    #[test]
    fn tool_result_text_content() {
        let msg = ToolResultMessage::new(
            "call_1",
            "read",
            vec![
                ContentBlock::Text(TextContent::new("file contents")),
                ContentBlock::Image(ImageContent::new("aaa", "image/png")),
            ],
        );
        let text = msg.text_content().unwrap();
        assert!(text.contains("file contents"));
        assert!(text.contains("[Image]"));
    }

    // ---- transform_for_provider ----

    #[test]
    fn transform_openai_to_anthropic_keeps_thinking() {
        let mut a = AssistantMessage::new(Api::OpenAiCompletions, "openai", "gpt-4");
        a.content
            .push(ContentBlock::Text(TextContent::new("Hello")));
        a.content
            .push(ContentBlock::Thinking(ThinkingContent::new("pondering")));
        let messages = vec![Message::Assistant(a)];

        let transformed =
            transform_for_provider(&messages, &Api::OpenAiCompletions, &Api::AnthropicMessages);
        match &transformed[0] {
            Message::Assistant(a) => {
                // Anthropic keeps thinking blocks as-is
                assert_eq!(a.content.len(), 2);
                assert!(matches!(&a.content[1], ContentBlock::Thinking(_)));
            }
            _ => panic!("Expected Assistant"),
        }
    }

    #[test]
    fn transform_anthropic_to_openai_converts_thinking() {
        let mut a = AssistantMessage::new(Api::AnthropicMessages, "anthropic", "claude-3");
        a.content
            .push(ContentBlock::Text(TextContent::new("Hello")));
        a.content
            .push(ContentBlock::Thinking(ThinkingContent::new("pondering")));
        let messages = vec![Message::Assistant(a)];

        let transformed =
            transform_for_provider(&messages, &Api::AnthropicMessages, &Api::OpenAiCompletions);
        match &transformed[0] {
            Message::Assistant(a) => {
                // Thinking converted to text, then merged with adjacent text
                assert!(a.content.iter().all(|b| matches!(b, ContentBlock::Text(_))));
                let full_text: String = a.content.iter().filter_map(|b| b.as_text()).collect();
                assert!(full_text.contains("Hello"));
                assert!(full_text.contains("<thinking>"));
                assert!(full_text.contains("pondering"));
            }
            _ => panic!("Expected Assistant"),
        }
    }

    #[test]
    fn transform_roundtrip_openai_anthropic_openai() {
        let mut a = AssistantMessage::new(Api::OpenAiCompletions, "openai", "gpt-4");
        a.content
            .push(ContentBlock::Text(TextContent::new("Hello")));
        a.content
            .push(ContentBlock::Thinking(ThinkingContent::new("pondering")));
        a.content
            .push(ContentBlock::Text(TextContent::new("World")));
        let original = vec![Message::Assistant(a)];

        // OpenAI -> Anthropic (keeps thinking)
        let step1 =
            transform_for_provider(&original, &Api::OpenAiCompletions, &Api::AnthropicMessages);
        // Anthropic -> OpenAI (converts thinking to text)
        let step2 =
            transform_for_provider(&step1, &Api::AnthropicMessages, &Api::OpenAiCompletions);

        match &step2[0] {
            Message::Assistant(a) => {
                let full_text: String = a.content.iter().filter_map(|b| b.as_text()).collect();
                assert!(full_text.contains("Hello"));
                assert!(full_text.contains("World"));
                assert!(full_text.contains("<thinking>"));
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ---- Adjacent text block merging ----

    #[test]
    fn merge_adjacent_text_blocks_basic() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Text(TextContent::new("b")),
            ContentBlock::Text(TextContent::new("c")),
        ];
        let merged = merge_adjacent_text_blocks(blocks);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].as_text(), Some("a\nb\nc"));
    }

    #[test]
    fn merge_adjacent_text_blocks_with_intervening() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Text(TextContent::new("b")),
            ContentBlock::ToolCall(ToolCall::new("1", "tool", serde_json::json!({}))),
            ContentBlock::Text(TextContent::new("c")),
        ];
        let merged = merge_adjacent_text_blocks(blocks);
        assert_eq!(merged.len(), 3); // "a\nb", ToolCall, "c"
        assert_eq!(merged[0].as_text(), Some("a\nb"));
        assert!(merged[1].as_tool_call().is_some());
        assert_eq!(merged[2].as_text(), Some("c"));
    }

    #[test]
    fn merge_adjacent_text_blocks_empty() {
        let blocks: Vec<ContentBlock> = vec![];
        let merged = merge_adjacent_text_blocks(blocks);
        assert!(merged.is_empty());
    }

    #[test]
    fn message_content_from_conversions() {
        let mc: MessageContent = "hello".into();
        assert!(mc.is_text());
        assert_eq!(mc.as_str(), Some("hello"));

        let mc: MessageContent = "world".to_string().into();
        assert!(mc.is_text());

        let mc: MessageContent = TextContent::new("block").into();
        assert!(!mc.is_text());
    }
}
