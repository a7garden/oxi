//! Agent state management

use crate::types::{StopReason, ToolResult};
use oxi_ai::{Message, ContentBlock, TextContent};
use parking_lot::RwLock;

/// Agent execution state
#[derive(Debug, Clone)]
pub struct AgentState {
    pub messages: Vec<Message>,
    pub iteration: usize,
    pub stop_reason: Option<StopReason>,
    pub tool_results: Vec<ToolResult>,
    pub total_tokens: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            iteration: 0,
            stop_reason: None,
            tool_results: Vec::new(),
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
        }
    }
}

impl AgentState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(Message::User(oxi_ai::UserMessage::new(content)));
    }

    /// Add an assistant message
    pub fn add_assistant_message(&mut self, content: String) {
        let mut assistant = oxi_ai::AssistantMessage::new(
            oxi_ai::Api::AnthropicMessages,
            "agent",
            "agent-model",
        );
        assistant.content = vec![ContentBlock::Text(TextContent::new(content))];
        self.messages.push(Message::Assistant(assistant));
    }

    /// Add a tool result message
    pub fn add_tool_result(&mut self, tool_call_id: String, content: String) {
        let content_for_result = content.clone();
        let tool_result_msg = oxi_ai::ToolResultMessage::new(
            tool_call_id.clone(),
            "tool",
            vec![ContentBlock::Text(TextContent::new(content))],
        );
        self.messages.push(oxi_ai::Message::ToolResult(tool_result_msg));
        self.tool_results.push(ToolResult::success(tool_call_id, content_for_result));
    }

    /// Increment iteration counter
    pub fn increment_iteration(&mut self) {
        self.iteration += 1;
    }

    /// Set stop reason
    pub fn set_stop_reason(&mut self, reason: StopReason) {
        self.stop_reason = Some(reason);
    }

    /// Record token usage
    pub fn record_usage(&mut self, input: usize, output: usize) {
        self.input_tokens += input;
        self.output_tokens += output;
        self.total_tokens += input + output;
    }

    /// Clear all messages (for new conversation)
    pub fn clear(&mut self) {
        self.messages.clear();
        self.iteration = 0;
        self.stop_reason = None;
        self.tool_results.clear();
        self.total_tokens = 0;
        self.input_tokens = 0;
        self.output_tokens = 0;
    }

    /// Check if state indicates completion
    pub fn is_complete(&self) -> bool {
        self.stop_reason.is_some()
    }
}

/// Thread-safe agent state wrapper
#[derive(Default)]
pub struct SharedState {
    state: RwLock<AgentState>,
}

impl SharedState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_state(&self) -> AgentState {
        self.state.read().clone()
    }

    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut AgentState),
    {
        let mut state = self.state.write();
        f(&mut state);
    }

    pub fn reset(&self) {
        let mut state = self.state.write();
        state.clear();
    }
}
