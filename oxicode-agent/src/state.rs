/// Agent state management
use crate::types::{StopReason, ToolResult};
use oxicode_ai::{ContentBlock, Message, TextContent};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Agent execution state
///
/// Tracks the full lifecycle of an agent conversation including messages,
/// token usage, tool results, and iteration progress.
///
/// Derives `Serialize`/`Deserialize` for session persistence and
/// cross-process state transfer (e.g. oxios supervisor serialization).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    /// **Most-recent** reported input-token count from a single LLM response.
    ///
    /// Unlike [`Self::input_tokens`] (which is cumulative across all turns),
    /// this is overwritten on every `ProviderEvent::Done` with the count
    /// that turn actually sent to the model. It is the **ground-truth** signal
    /// used by compaction when available — see [`Self::current_token_source`].
    ///
    /// `None` until the first `Done` event has been observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_input_tokens: Option<usize>,
    /// Heuristic token estimate (`bytes/4`) that was current when
    /// `last_input_tokens` was last set. Used to detect the bytes/4 drift
    /// described in #28 and surface it as a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_estimate_at_report: Option<usize>,
    /// Divergence factor (reported / estimate) at the time of the last
    /// `Done`. Surfaced in logs so the operator can see how badly the
    /// `bytes/4` heuristic is undercounting on token-dense workloads.
    /// `None` until first divergence observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_estimate_divergence: Option<f64>,
}

/// Source of the value driving compaction / context-size decisions.
///
/// `Real` is the provider-reported input-token count from the most recent
/// `Done` event — ground truth. `Heuristic` is the legacy
/// `serialized_json.len() / 4` estimate. `None` means no messages have
/// been sent yet (the loop has not observed any size at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// No size observation available yet (cold start).
    None,
    /// `bytes/4` estimate; reliable only for token-sparse prose.
    Heuristic(usize),
    /// Provider-reported input token count from the most recent `Done`.
    Real(usize),
}

impl AgentState {
    /// Create a new, default-initialized agent state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: String) {
        self.messages
            .push(Message::User(oxicode_ai::UserMessage::new(content)));
    }

    /// Add an assistant message
    pub fn add_assistant_message(&mut self, content: String) {
        let mut assistant = oxicode_ai::AssistantMessage::new(
            oxicode_ai::Api::AnthropicMessages,
            "agent",
            "agent-model",
        );
        assistant.content = vec![ContentBlock::Text(TextContent::new(content))];
        self.messages.push(Message::Assistant(assistant));
    }

    /// Add a tool result message to both the message history and the tool results list.
    pub fn add_tool_result(&mut self, tool_call_id: String, content: String) {
        let content_for_result = content.clone();
        let tool_result_msg = oxicode_ai::ToolResultMessage::new(
            tool_call_id.clone(),
            "tool",
            vec![ContentBlock::Text(TextContent::new(content))],
        );
        self.messages
            .push(oxicode_ai::Message::ToolResult(tool_result_msg));
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
    ///
    /// `input` is **the input-token count for the just-completed turn** —
    /// NOT a per-turn delta. The provider reports a fresh per-turn
    /// `usage.input_tokens` on every `Done`; we accumulate it into
    /// [`Self::input_tokens`] for lifetime accounting and **also** cache
    /// it as the most-recent observation via [`Self::record_provider_turn`].
    pub fn record_usage(&mut self, input: usize, output: usize) {
        self.input_tokens += input;
        self.output_tokens += output;
        self.total_tokens += input + output;
    }

    /// Record the most recent provider-reported input-token count and the
    /// heuristic estimate that was current at the time of the report, so
    /// the loop can use the real count for compaction decisions and
    /// surface drift (issue #28 gap 2).
    ///
    /// `input_tokens` is the value from `ProviderEvent::Done.message.usage.input`.
    /// `estimate_at_report` is the bytes/4 estimate taken at the same moment
    /// (i.e. the value the legacy path *would* have used for compaction).
    pub fn record_provider_turn(&mut self, input_tokens: usize, estimate_at_report: usize) {
        self.last_input_tokens = Some(input_tokens);
        self.last_estimate_at_report = Some(estimate_at_report);
        // Divergence = reported / estimate. A value > 1.0 means the
        // heuristic under-counted; the documented failure in #28 saw
        // ~3.5×. Guard against the zero-estimate case to avoid Inf/NaN.
        self.last_estimate_divergence = if estimate_at_report > 0 {
            Some(input_tokens as f64 / estimate_at_report as f64)
        } else if input_tokens > 0 {
            // An estimate of 0 against a non-zero report is the worst-case
            // divergence: the heuristic was essentially blind to the
            // context. Surface this as a high multiplier.
            Some(f64::INFINITY)
        } else {
            Some(1.0)
        };
    }

    /// Current best estimate of context size, tagged with its source.
    ///
    /// - `Real(n)` if the last completed turn reported `usage.input_tokens`.
    /// - `Heuristic(n)` only before the first `Done` is observed (cold start).
    /// - `None` if there are no messages yet.
    ///
    /// Callers (notably `maybe_compact`) should **prefer** `Real` and **only**
    /// fall back to `Heuristic` on cold start. See issue #28.
    pub fn current_token_source(&self) -> TokenSource {
        if let Some(real) = self.last_input_tokens {
            TokenSource::Real(real)
        } else if !self.messages.is_empty() {
            TokenSource::Heuristic(self.estimate_tokens())
        } else {
            TokenSource::None
        }
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
        self.last_input_tokens = None;
        self.last_estimate_at_report = None;
        self.last_estimate_divergence = None;
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
#[derive(Default, Clone)]
pub struct SharedState {
    state: Arc<RwLock<AgentState>>,
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
