//! Event types for oxi-agent
//!
//! Events are emitted by the Agent to notify subscribers of state changes,
//! streaming updates, and tool execution progress.

use serde::{Deserialize, Serialize};

/// Agent lifecycle events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    // Agent lifecycle
    /// Emitted when the agent starts processing a new prompt
    AgentStart,
    /// Emitted when the agent finishes processing (last event of a run)
    AgentEnd {
        messages: Vec<crate::types::AgentMessage>,
    },
    // Turn lifecycle - a turn is one assistant response + any tool calls/results
    /// Emitted when a new turn starts
    TurnStart,
    /// Emitted when a turn completes (after all tool results)
    TurnEnd {
        message: crate::types::AgentMessage,
        tool_results: Vec<oxi_ai::ToolResultMessage>,
    },
    // Message lifecycle
    /// Emitted when a new message starts being processed
    MessageStart {
        message: crate::types::AgentMessage,
    },
    /// Emitted during streaming with delta updates (assistant messages only)
    MessageUpdate {
        message: crate::types::AgentMessage,
        delta: crate::types::AssistantMessageEvent,
    },
    /// Emitted when a message finishes
    MessageEnd {
        message: crate::types::AgentMessage,
    },
    // Tool execution lifecycle
    /// Emitted when tool execution begins
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    /// Emitted with partial tool execution updates
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        partial_result: serde_json::Value,
    },
    /// Emitted when tool execution completes
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: serde_json::Value,
        is_error: bool,
    },
}

/// Receiver for agent events
pub type EventReceiver = tokio::sync::broadcast::Receiver<AgentEvent>;

impl AgentEvent {
    /// Check if this is a terminal event (agent_end)
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentEvent::AgentEnd { .. })
    }
}
