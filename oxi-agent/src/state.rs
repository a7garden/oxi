/// Agent state management

use crate::types::{StopReason, ToolResult};
use oxi_ai::{ContentBlock, Message, TextContent};
use parking_lot::RwLock;

/// Agent execution state
///
/// Tracks the full lifecycle of an agent conversation including messages,
/// token usage, tool results, and iteration progress.
#[derive(Debug, Clone)]
pub struct AgentState {
    /// Conversation message history (user, assistant, and tool-result messages).
    pub messages: Vec<Message>,
    /// Current agent loop iteration (incremented after each assistant turn).
    pub iteration: usize,
    /// The reason the last turn stopped, if any.
    pub stop_reason: Option<StopReason>,
    /// Accumulated results from tool executions in the current conversation.
    pub tool_results: Vec<ToolResult>,
    /// Cumulative token count (input + output) across all turns.
    pub total_tokens: usize,
    /// Cumulative prompt / input tokens across all turns.
    pub input_tokens: usize,
    /// Cumulative completion / output tokens across all turns.
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
    /// Create a new, default-initialized agent state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: String) {
        self.messages
            .push(Message::User(oxi_ai::UserMessage::new(content)));
    }

    /// Add an assistant message
    pub fn add_assistant_message(&mut self, content: String) {
        let mut assistant =
            oxi_ai::AssistantMessage::new(oxi_ai::Api::AnthropicMessages, "agent", "agent-model");
        assistant.content = vec![ContentBlock::Text(TextContent::new(content))];
        self.messages.push(Message::Assistant(assistant));
    }

    /// Add a tool result message to both the message history and the tool results list.
    pub fn add_tool_result(&mut self, tool_call_id: String, content: String) {
        let content_for_result = content.clone();
        let tool_result_msg = oxi_ai::ToolResultMessage::new(
            tool_call_id.clone(),
            "tool",
            vec![ContentBlock::Text(TextContent::new(content))],
        );
        self.messages
            .push(oxi_ai::Message::ToolResult(tool_result_msg));
        self.tool_results
            .push(ToolResult::success(tool_call_id, content_for_result));
    }

    /// Increment the iteration counter after an assistant turn completes.
    pub fn increment_iteration(&mut self) {
        self.iteration += 1;
    }

    /// Record the reason the last turn stopped.
    pub fn set_stop_reason(&mut self, reason: StopReason) {
        self.stop_reason = Some(reason);
    }

    /// Accumulate token usage from a completed LLM call.
    pub fn record_usage(&mut self, input: usize, output: usize) {
        self.input_tokens += input;
        self.output_tokens += output;
        self.total_tokens += input + output;
    }

    /// Clear all state, resetting for a new conversation.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.iteration = 0;
        self.stop_reason = None;
        self.tool_results.clear();
        self.total_tokens = 0;
        self.input_tokens = 0;
        self.output_tokens = 0;
    }

    /// Replace the entire message history (used after context compaction).
    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Rough token-count estimate based on the serialized message JSON length.
    pub fn estimate_tokens(&self) -> usize {
        let json = serde_json::to_string(&self.messages).unwrap_or_default();
        json.len() / 4 // Rough approximation
    }

    /// Returns `true` if the agent has signaled a stop reason.
    pub fn is_complete(&self) -> bool {
        self.stop_reason.is_some()
    }
}

/// Thread-safe agent state wrapper.
#[derive(Default)]
pub struct SharedState {
    state: RwLock<AgentState>,
}

impl SharedState {
    /// Create a new SharedState with default (empty) agent state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Obtain a snapshot of the current agent state.
    pub fn get_state(&self) -> AgentState {
        self.state.read().clone()
    }

    /// Mutably update the agent state under a write lock.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut AgentState),
    {
        let mut state = self.state.write();
        f(&mut state);
    }

    /// Reset the state for a new conversation (delegates to [`AgentState::clear`]).
    pub fn reset(&self) {
        let mut state = self.state.write();
        state.clear();
    }
}
