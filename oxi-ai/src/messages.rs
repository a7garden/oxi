//! Message types for oxi-ai

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Text content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub content_type: TextContentType,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "text")]
pub enum TextContentType {
    Text,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            content_type: TextContentType::Text,
            text: text.into(),
        }
    }
}

/// Thinking content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingContent {
    #[serde(rename = "type")]
    pub content_type: ThinkingContentType,
    pub thinking: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "thinking")]
pub enum ThinkingContentType {
    Thinking,
}

impl ThinkingContent {
    pub fn new(thinking: impl Into<String>) -> Self {
        Self {
            content_type: ThinkingContentType::Thinking,
            thinking: thinking.into(),
            thinking_signature: None,
            redacted: None,
        }
    }
}

/// Image content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    #[serde(rename = "type")]
    pub content_type: ImageContentType,
    pub data: String,        // base64 encoded
    pub mime_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "image")]
pub enum ImageContentType {
    Image,
}

impl ImageContent {
    pub fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            content_type: ImageContentType::Image,
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }
}

/// Tool call content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(rename = "type")]
    pub content_type: ToolCallType,
    pub id: String,
    pub name: String,
    pub arguments: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "toolCall")]
pub enum ToolCallType {
    ToolCall,
}

impl ToolCall {
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

/// Content block union (untagged for flexibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    Image(ImageContent),
    ToolCall(ToolCall),
    Unknown(JsonValue),
}

impl ContentBlock {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text(t) => Some(&t.text),
            _ => None,
        }
    }

    pub fn as_tool_call(&self) -> Option<&ToolCall> {
        match self {
            ContentBlock::ToolCall(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_thinking(&self) -> Option<&ThinkingContent> {
        match self {
            ContentBlock::Thinking(t) => Some(t),
            _ => None,
        }
    }
}

/// User message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub role: UserRole,
    pub content: MessageContent,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "user")]
pub enum UserRole {
    #[serde(rename = "user")]
    User,
}

impl UserMessage {
    pub fn new(content: impl Into<MessageContent>) -> Self {
        Self {
            role: UserRole::User,
            content: content.into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}

/// Assistant message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub role: AssistantRole,
    pub content: Vec<ContentBlock>,
    pub api: super::Api,
    pub provider: String,
    pub model: String,
    pub usage: super::Usage,
    pub stop_reason: super::StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "assistant")]
pub enum AssistantRole {
    #[serde(rename = "assistant")]
    Assistant,
}

impl AssistantMessage {
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

    pub fn text_content(&self) -> String {
        let mut result = String::new();
        for block in &self.content {
            if let Some(text) = block.as_text() {
                result.push_str(text);
            }
        }
        result
    }
}

/// Tool result message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub role: ToolResultRole,
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<JsonValue>,
    #[serde(default)]
    pub is_error: bool,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "toolResult")]
pub enum ToolResultRole {
    #[serde(rename = "toolResult")]
    ToolResult,
}

impl ToolResultMessage {
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

    pub fn error(tool_call_id: impl Into<String>, tool_name: impl Into<String>, error: impl Into<String>) -> Self {
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

    pub fn text_content(&self) -> Result<String, crate::error::ProviderError> {
        let mut result = String::new();
        for block in &self.content {
            match block {
                ContentBlock::Text(t) => {
                    result.push_str(&t.text);
                    result.push('\n');
                }
                ContentBlock::Image(_) => {
                    result.push_str("[Image]");
                    result.push('\n');
                }
                ContentBlock::Thinking(t) => {
                    result.push_str(&format!("[Thinking: {}]", t.thinking));
                    result.push('\n');
                }
                ContentBlock::ToolCall(tc) => {
                    result.push_str(&format!("[Tool: {}]", tc.name));
                    result.push('\n');
                }
                ContentBlock::Unknown(_) => {
                    // Skip unknown blocks
                }
            }
        }
        Ok(result.trim().to_string())
    }
}

/// Message union
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

impl Message {
    pub fn user(content: impl Into<MessageContent>) -> Self {
        Message::User(UserMessage::new(content))
    }

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
            Message::User(m) => {
                match &m.content {
                    MessageContent::Text(s) => Ok(s.clone()),
                    MessageContent::Blocks(blocks) => {
                        let mut result = String::new();
                        for block in blocks {
                            match block {
                                ContentBlock::Text(t) => {
                                    result.push_str(&t.thinking);
                                    result.push('\n');
                                }
                                ContentBlock::Image(_) => {
                                    result.push_str("[Image]");
                                    result.push('\n');
                                }
                                ContentBlock::Thinking(t) => {
                                    result.push_str(&t.thinking);
                                    result.push('\n');
                                }
                                ContentBlock::ToolCall(_) => {
                                    result.push_str("[Tool Call]");
                                    result.push('\n');
                                }
                                ContentBlock::Unknown(_) => {
                                    result.push_str("[Unknown]");
                                    result.push('\n');
                                }
                            }
                        }
                        Ok(result.trim().to_string())
                    }
                }
            }
            Message::Assistant(m) => Ok(m.text_content()),
            Message::ToolResult(m) => m.text_content(),        }
    }
}

/// Message content (string or content blocks)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn is_text(&self) -> bool {
        matches!(self, MessageContent::Text(_))
    }

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
