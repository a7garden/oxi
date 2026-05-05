/// Core types for oxi-agent

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: HashMap<String, serde_json::Value>,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// Tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }
}

/// Tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: true,
        }
    }
}

/// Response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub content: String,
    pub stop_reason: StopReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// StopReason.
pub enum StopReason {
/// stop variant.
    Stop,
/// length variant.
    Length,
/// tool use variant.
    ToolUse,
/// error variant.
    Error,
/// aborted variant.
    Aborted,
}
