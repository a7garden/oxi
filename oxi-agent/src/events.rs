//! Agent event types for streaming and state updates
//!
//! These events are emitted during agent execution to enable
//! streaming UI, logging, and state synchronization.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use crate::types::ContentBlock;

/// Agent turn/event types
///
/// These events are emitted during agent execution in chronological order
/// to provide real-time updates about what's happening in the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Agent session has started
    AgentStart {
        model: String,
        system_prompt: String,
    },
    /// Agent session has ended
    AgentEnd {
        reason: AgentEndReason,
    },
    /// A new turn has started (user message received)
    TurnStart {
        turn_number: usize,
    },
    /// A turn has ended (all tool calls completed or response sent)
    TurnEnd {
        turn_number: usize,
        stop_reason: String,
    },
    /// A message from the assistant is starting
    MessageStart {
        message_id: String,
    },
    /// A message is being updated (streaming content)
    MessageUpdate {
        message_id: String,
        delta: MessageDelta,
    },
    /// A message is complete
    MessageEnd {
        message_id: String,
        content: Vec<ContentBlock>,
        stop_reason: String,
    },
    /// Tool execution has started
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        input: JsonValue,
    },
    /// Tool execution is updating (streaming output)
    ToolExecutionUpdate {
        tool_call_id: String,
        delta: String,
    },
    /// Tool execution has completed
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: JsonValue,
        success: bool,
    },
    /// Error occurred during execution
    Error {
        code: String,
        message: String,
    },
    /// Ping event for keepalive during streaming
    Ping,
}

/// Reason for agent session ending
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEndReason {
    /// Agent completed successfully (stop token received)
    Completed,
    /// Maximum turns reached
    MaxTurnsReached,
    /// Maximum tokens reached
    MaxTokensReached,
    /// User terminated
    UserTerminated,
    /// Error occurred
    Error,
    /// Terminate tool was called
    ToolTerminate,
}

impl Default for AgentEndReason {
    fn default() -> Self {
        Self::Completed
    }
}

/// Delta updates for streaming messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDelta {
    /// Text delta (for text content blocks)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_delta: Option<String>,
    /// Thinking delta (for thinking blocks)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_delta: Option<String>,
    /// Tool use delta (for partial tool calls)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_delta: Option<ToolUseDelta>,
}

/// Partial tool call during streaming
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseDelta {
    /// Tool call ID
    pub id: String,
    /// Tool name delta
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_delta: Option<String>,
    /// Tool input delta (partial JSON)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_delta: Option<String>,
}

impl AgentEvent {
    /// Create an agent_start event
    pub fn agent_start(model: String, system_prompt: String) -> Self {
        Self::AgentStart {
            model,
            system_prompt,
        }
    }

    /// Create an agent_end event
    pub fn agent_end(reason: AgentEndReason) -> Self {
        Self::AgentEnd { reason }
    }

    /// Create a turn_start event
    pub fn turn_start(turn_number: usize) -> Self {
        Self::TurnStart { turn_number }
    }

    /// Create a turn_end event
    pub fn turn_end(turn_number: usize, stop_reason: String) -> Self {
        Self::TurnEnd { turn_number, stop_reason }
    }

    /// Create a message_start event
    pub fn message_start(message_id: String) -> Self {
        Self::MessageStart { message_id }
    }

    /// Create a message_update event with text delta
    pub fn message_text_update(message_id: String, text_delta: String) -> Self {
        Self::MessageUpdate {
            message_id,
            delta: MessageDelta {
                text_delta: Some(text_delta),
                thinking_delta: None,
                tool_use_delta: None,
            },
        }
    }

    /// Create a message_update event with thinking delta
    pub fn message_thinking_update(message_id: String, thinking_delta: String) -> Self {
        Self::MessageUpdate {
            message_id,
            delta: MessageDelta {
                text_delta: None,
                thinking_delta: Some(thinking_delta),
                tool_use_delta: None,
            },
        }
    }

    /// Create a message_end event
    pub fn message_end(message_id: String, content: Vec<ContentBlock>, stop_reason: String) -> Self {
        Self::MessageEnd {
            message_id,
            content,
            stop_reason,
        }
    }

    /// Create a tool_execution_start event
    pub fn tool_execution_start(tool_call_id: String, tool_name: String, input: JsonValue) -> Self {
        Self::ToolExecutionStart {
            tool_call_id,
            tool_name,
            input,
        }
    }

    /// Create a tool_execution_update event
    pub fn tool_execution_update(tool_call_id: String, delta: String) -> Self {
        Self::ToolExecutionUpdate {
            tool_call_id,
            delta,
        }
    }

    /// Create a tool_execution_end event
    pub fn tool_execution_end(
        tool_call_id: String,
        tool_name: String,
        result: JsonValue,
        success: bool,
    ) -> Self {
        Self::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            success,
        }
    }

    /// Create an error event
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Create a ping event
    pub fn ping() -> Self {
        Self::Ping
    }
}