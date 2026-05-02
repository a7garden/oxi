//! Core types for oxi-agent
//!
//! Defines the fundamental types used throughout the agent runtime.

use oxi_ai::{Model, Api};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// Re-export from oxi-ai
pub use oxi_ai::ThinkingLevel;

/// Content block types for messages and tool results
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content
    Text {
        #[serde(default)]
        text: String,
    },
    /// Tool use request
    ToolUse {
        id: String,
        name: String,
        input: JsonValue,
    },
    /// Tool result content
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: Option<bool>,
    },
    /// Image content
    Image {
        source: ImageSource,
    },
    /// Thinking content (for reasoning models)
    Thinking {
        thinking: String,
    },
}

impl Default for ContentBlock {
    fn default() -> Self {
        ContentBlock::Text { text: String::new() }
    }
}

/// Image source for content blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

/// Agent tool result with content and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolResult<T = JsonValue> {
    /// Content blocks returned by the tool
    pub content: Vec<ContentBlock>,
    /// Additional details about the tool execution
    pub details: T,
    /// Whether the agent should terminate after this result
    #[serde(default)]
    pub terminate: bool,
}

impl<T> AgentToolResult<T> {
    pub fn new(content: Vec<ContentBlock>, details: T) -> Self {
        Self {
            content,
            details,
            terminate: false,
        }
    }

    pub fn terminate(content: Vec<ContentBlock>, details: T) -> Self {
        Self {
            content,
            details,
            terminate: true,
        }
    }
}

impl AgentToolResult {
    pub fn json(content: Vec<ContentBlock>) -> Self {
        Self::new(content, serde_json::json!({}))
    }
}

impl<T: Default> Default for AgentToolResult<T> {
    fn default() -> Self {
        Self {
            content: Vec::new(),
            details: T::default(),
            terminate: false,
        }
    }
}

/// Tool execution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionMode {
    /// Execute tools one at a time, waiting for each to complete
    #[default]
    Sequential,
    /// Execute multiple tools in parallel
    Parallel,
}

/// Agent message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", content = "content", rename_all = "snake_case")]
pub enum AgentMessage {
    /// User message
    User {
        content: Vec<ContentBlock>,
    },
    /// Assistant message
    Assistant {
        content: Vec<ContentBlock>,
        thinking: Option<String>,
    },
    /// System message
    System {
        content: Vec<ContentBlock>,
    },
    /// Tool result message
    ToolResult {
        tool_use_id: String,
        content: Vec<ContentBlock>,
    },
}

impl AgentMessage {
    /// Create a new user message
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: vec![ContentBlock::Text { text: content.into() }],
        }
    }

    /// Create a new assistant message
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self::Assistant {
            content,
            thinking: None,
        }
    }

    /// Create a new system message
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: vec![ContentBlock::Text { text: content.into() }],
        }
    }

    /// Create a tool result message
    pub fn tool_result(tool_use_id: impl Into<String>, content: Vec<ContentBlock>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content,
        }
    }

    /// Check if this is a user message
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }

    /// Check if this is an assistant message
    pub fn is_assistant(&self) -> bool {
        matches!(self, Self::Assistant { .. })
    }
}

/// Configuration for agent initialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// System prompt for the agent
    pub system_prompt: String,
    /// Model to use
    pub model: Model,
    /// Thinking/reasoning level
    #[serde(default)]
    pub thinking_level: ThinkingLevel,
    /// Tool execution mode
    #[serde(default)]
    pub tool_execution_mode: ToolExecutionMode,
    /// Maximum turns before termination
    #[serde(default)]
    pub max_turns: Option<usize>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            model: Model::new(
                "claude-sonnet-4-20250514",
                "Claude Sonnet 4",
                Api::AnthropicMessages,
                "Anthropic",
                "https://api.anthropic.com",
            ),
            thinking_level: ThinkingLevel::default(),
            tool_execution_mode: ToolExecutionMode::default(),
            max_turns: None,
        }
    }
}