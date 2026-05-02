//! Agent state management
//!
//! This module provides the AgentState struct which holds all the runtime
//! state for an agent session.

use std::collections::HashSet;
use std::sync::Arc;
use parking_lot::RwLock;

use oxi_ai::{Model, ThinkingLevel};
use crate::types::{AgentConfig, AgentMessage, ToolExecutionMode};
use crate::tools::AgentTool;

/// Agent runtime state
///
/// This struct holds all the mutable state needed during agent execution:
/// - Configuration (system prompt, model, tools)
/// - Message history
/// - Streaming state
/// - Pending operations tracking
pub struct AgentState {
    /// System prompt for the agent
    pub system_prompt: String,
    /// Model to use for completions
    pub model: Model,
    /// Thinking/reasoning level
    pub thinking_level: ThinkingLevel,
    /// Tool execution mode
    pub tool_execution_mode: ToolExecutionMode,
    /// Available tools
    pub tools: Vec<Arc<dyn AgentTool>>,
    /// Message history
    pub messages: Vec<AgentMessage>,
    /// Whether currently streaming
    pub is_streaming: bool,
    /// Current streaming message (partial)
    pub streaming_message: Option<StreamingState>,
    /// Currently pending tool call IDs
    pub pending_tool_calls: Arc<RwLock<HashSet<String>>>,
    /// Last error message if any
    pub error_message: Option<String>,
    /// Maximum turns allowed (None = unlimited)
    pub max_turns: Option<usize>,
    /// Current turn number
    pub turn_count: usize,
    /// Token usage tracking
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
}

impl std::fmt::Debug for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentState")
            .field("system_prompt", &self.system_prompt.len())
            .field("model", &self.model.id)
            .field("thinking_level", &self.thinking_level)
            .field("tool_execution_mode", &self.tool_execution_mode)
            .field("tools_count", &self.tools.len())
            .field("messages_count", &self.messages.len())
            .field("is_streaming", &self.is_streaming)
            .field("turn_count", &self.turn_count)
            .field("max_turns", &self.max_turns)
            .field("total_input_tokens", &self.total_input_tokens)
            .field("total_output_tokens", &self.total_output_tokens)
            .field("error_message", &self.error_message)
            .finish()
    }
}

/// Streaming state for in-progress messages
#[derive(Debug, Clone)]
pub struct StreamingState {
    /// Message ID
    pub message_id: String,
    /// Accumulated text content
    pub text: String,
    /// Accumulated thinking content
    pub thinking: Option<String>,
    /// Tool calls started but not completed
    pub pending_tool_calls: Vec<PendingToolCall>,
}

/// A tool call that has been started but not completed
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    /// Unique ID for this tool call
    pub id: String,
    /// Tool name
    pub name: String,
    /// Partial input (may be incomplete during streaming)
    pub input: String,
}

impl Default for AgentState {
    fn default() -> Self {
        Self::new(AgentConfig::default())
    }
}

impl AgentState {
    /// Create a new agent state from configuration
    pub fn new(config: AgentConfig) -> Self {
        Self {
            system_prompt: config.system_prompt,
            model: config.model,
            thinking_level: config.thinking_level,
            tool_execution_mode: config.tool_execution_mode,
            tools: Vec::new(),
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Arc::new(RwLock::new(HashSet::new())),
            error_message: None,
            max_turns: config.max_turns,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    }

    /// Add a tool to the agent
    pub fn add_tool<T: AgentTool + 'static>(&mut self, tool: T) {
        self.tools.push(Arc::new(tool));
    }

    /// Add a boxed tool to the agent
    pub fn add_boxed_tool(&mut self, tool: Box<dyn AgentTool>) {
        self.tools.push(tool.into());
    }

    /// Add a message to history
    pub fn add_message(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    /// Get all messages
    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    /// Clear all messages
    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    /// Add a pending tool call ID
    pub fn add_pending_tool_call(&self, id: &str) {
        self.pending_tool_calls.write().insert(id.to_string());
    }

    /// Remove a pending tool call ID
    pub fn remove_pending_tool_call(&self, id: &str) {
        self.pending_tool_calls.write().remove(id);
    }

    /// Check if any tool calls are pending
    pub fn has_pending_tool_calls(&self) -> bool {
        !self.pending_tool_calls.read().is_empty()
    }

    /// Get count of pending tool calls
    pub fn pending_tool_call_count(&self) -> usize {
        self.pending_tool_calls.read().len()
    }

    /// Set error message
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error_message = Some(error.into());
    }

    /// Clear error message
    pub fn clear_error(&mut self) {
        self.error_message = None;
    }

    /// Check if agent has reached max turns
    pub fn has_reached_max_turns(&self) -> bool {
        match self.max_turns {
            Some(max) => self.turn_count >= max,
            None => false,
        }
    }

    /// Increment turn counter
    pub fn increment_turn(&mut self) {
        self.turn_count += 1;
    }

    /// Start streaming a new message
    pub fn start_streaming(&mut self, message_id: String) {
        self.is_streaming = true;
        self.streaming_message = Some(StreamingState {
            message_id,
            text: String::new(),
            thinking: None,
            pending_tool_calls: Vec::new(),
        });
    }

    /// Stop streaming and return the final state
    pub fn stop_streaming(&mut self) -> Option<StreamingState> {
        self.is_streaming = false;
        self.streaming_message.take()
    }

    /// Update streaming text
    pub fn append_streaming_text(&mut self, text: &str) {
        if let Some(ref mut state) = self.streaming_message {
            state.text.push_str(text);
        }
    }

    /// Update streaming thinking
    pub fn append_streaming_thinking(&mut self, thinking: &str) {
        if let Some(ref mut state) = self.streaming_message {
            if state.thinking.is_none() {
                state.thinking = Some(String::new());
            }
            if let Some(ref mut t) = state.thinking {
                t.push_str(thinking);
            }
        }
    }

    /// Add a pending tool call to streaming state
    pub fn add_streaming_tool_call(&mut self, id: String, name: String) {
        if let Some(ref mut state) = self.streaming_message {
            state.pending_tool_calls.push(PendingToolCall {
                id,
                name,
                input: String::new(),
            });
        }
    }

    /// Update the last pending tool call's input
    pub fn append_streaming_tool_input(&mut self, input: &str) {
        if let Some(ref mut state) = self.streaming_message {
            if let Some(tc) = state.pending_tool_calls.last_mut() {
                tc.input.push_str(input);
            }
        }
    }

    /// Find a tool by name
    pub fn find_tool(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    /// Update token usage
    pub fn update_usage(&mut self, input_tokens: usize, output_tokens: usize) {
        self.total_input_tokens += input_tokens;
        self.total_output_tokens += output_tokens;
    }

    /// Reset state for a new session
    pub fn reset(&mut self) {
        self.messages.clear();
        self.is_streaming = false;
        self.streaming_message = None;
        self.pending_tool_calls.write().clear();
        self.error_message = None;
        self.turn_count = 0;
        self.total_input_tokens = 0;
        self.total_output_tokens = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use serde_json::json;

    #[test]
    fn test_new_state() {
        let config = AgentConfig::default();
        let state = AgentState::new(config);

        assert!(state.system_prompt.is_empty());
        assert!(state.tools.is_empty());
        assert!(state.messages.is_empty());
        assert!(!state.is_streaming);
        assert_eq!(state.turn_count, 0);
    }

    #[test]
    fn test_add_tool() {
        struct TestTool;

        impl AgentTool for TestTool {
            fn name(&self) -> &str { "test" }
            fn label(&self) -> &str { "Test Tool" }
            fn description(&self) -> &str { "A test tool" }
            fn parameters_schema(&self) -> &JsonValue { &json!({}) }

            fn execute(
                &self,
                _tool_call_id: &str,
                _params: JsonValue,
                _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            ) -> Pin<Box<dyn std::future::Future<Output = Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>> + Send>> {
                Box::pin(async { Ok(AgentToolResult::default()) })
            }
        }

        let config = AgentConfig::default();
        let mut state = AgentState::new(config);
        state.add_tool(TestTool);

        assert_eq!(state.tools.len(), 1);
        assert_eq!(state.find_tool("test").map(|t| t.name()), Some("test"));
    }

    #[test]
    fn test_pending_tool_calls() {
        let config = AgentConfig::default();
        let state = AgentState::new(config);

        assert!(!state.has_pending_tool_calls());

        state.add_pending_tool_call("call_1");
        assert!(state.has_pending_tool_calls());
        assert_eq!(state.pending_tool_call_count(), 1);

        state.add_pending_tool_call("call_2");
        assert_eq!(state.pending_tool_call_count(), 2);

        state.remove_pending_tool_call("call_1");
        assert_eq!(state.pending_tool_call_count(), 1);
    }

    #[test]
    fn test_max_turns() {
        let mut config = AgentConfig::default();
        config.max_turns = Some(5);

        let mut state = AgentState::new(config);

        assert!(!state.has_reached_max_turns());

        for _ in 0..5 {
            state.increment_turn();
        }

        assert!(state.has_reached_max_turns());
    }

    #[test]
    fn test_streaming() {
        let config = AgentConfig::default();
        let mut state = AgentState::new(config);

        state.start_streaming("msg_123".to_string());
        assert!(state.is_streaming);

        state.append_streaming_text("Hello ");
        state.append_streaming_text("world!");
        assert_eq!(state.streaming_message.as_ref().unwrap().text, "Hello world!");

        state.append_streaming_thinking("Let me think...");
        assert_eq!(
            state.streaming_message.as_ref().unwrap().thinking.as_deref(),
            Some("Let me think...")
        );

        let final_state = state.stop_streaming();
        assert!(final_state.is_some());
        assert!(!state.is_streaming);
        assert!(state.streaming_message.is_none());
    }
}