/// Agent configuration
use oxi_ai::CompactionStrategy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn default_context_window() -> usize {
    128_000
}

/// Hook context for `shouldStopAfterTurn`.
#[derive(Debug, Clone)]
pub struct ShouldStopAfterTurnContext {
    /// The assistant message that completed the turn.
    pub message: oxi_ai::AssistantMessage,
    /// Tool result messages from this turn.
    pub tool_results: Vec<oxi_ai::ToolResultMessage>,
    /// Current iteration number.
    pub iteration: usize,
}

/// Result of `beforeToolCall` hook.
#[derive(Debug, Clone, Default)]
pub struct BeforeToolCallResult {
    /// If `true`, the tool call is blocked and an error result is returned.
    pub block: bool,
    /// Human-readable reason for blocking.
    pub reason: Option<String>,
}

/// Result of `afterToolCall` hook.
#[derive(Debug, Clone, Default)]
pub struct AfterToolCallResult {
    /// Override content for the tool result.
    pub content: Option<String>,
    /// Override error status.
    pub is_error: Option<bool>,
    /// Signal that the agent should stop after this batch.
    pub terminate: Option<bool>,
    /// Arbitrary structured details returned by the hook.
    ///
    /// Consumers (e.g. telemetry, middleware) can use this to attach
    /// extra context without extending the struct.
    pub details: Option<serde_json::Value>,
}

/// Hook context for `beforeToolCall`.
#[derive(Debug, Clone)]
pub struct BeforeToolCallContext {
    /// The tool call being made.
    pub tool_call_id: String,
    /// Tool name.
    pub tool_name: String,
    /// Validated arguments.
    pub args: serde_json::Value,
}

/// Hook context for `afterToolCall`.
#[derive(Debug, Clone)]
pub struct AfterToolCallContext {
    /// The tool call that was made.
    pub tool_call_id: String,
    /// Tool name.
    pub tool_name: String,
    /// The tool result content.
    pub result: String,
    /// Whether the result is an error.
    pub is_error: bool,
    /// Arbitrary structured details provided to the hook.
    ///
    /// Set by the agent loop before invoking the hook so that consumers
    /// receive extra context (e.g. execution timing, tool-specific metadata).
    pub details: Option<serde_json::Value>,
}

/// Callback hooks for the agent loop.
///
/// These mirror pi-mono's `AgentLoopConfig` hooks, allowing callers to
/// inject custom logic at key points in the agentic loop.
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct AgentHooks {
    /// Called after each turn completes. Return `true` to stop the agent loop.
    ///
    /// Wrapped in `Arc` so the hook can be invoked multiple times without
    /// being consumed (unlike `Box<dyn Fn>` which requires `take()`).
    pub should_stop_after_turn:
        Option<Arc<dyn Fn(&ShouldStopAfterTurnContext) -> bool + Send + Sync>>,

    /// Called before a tool is executed. Return a `BeforeToolCallResult` with
    /// `block: true` to prevent execution.
    #[allow(clippy::type_complexity)]
    pub before_tool_call:
        Option<Box<dyn Fn(&BeforeToolCallContext) -> BeforeToolCallResult + Send + Sync>>,

    /// Called after a tool execution completes. Can override the result.
    #[allow(clippy::type_complexity)]
    pub after_tool_call:
        Option<Box<dyn Fn(&AfterToolCallContext) -> AfterToolCallResult + Send + Sync>>,

    /// Returns steering messages to inject mid-run. Called after each turn
    /// (unless stopped).
    #[allow(clippy::type_complexity)]
    pub get_steering_messages: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,

    /// Returns follow-up messages to process after the agent would stop.
    /// Called when the agent has no more tool calls and no steering messages.
    #[allow(clippy::type_complexity)]
    pub get_follow_up_messages: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,

    /// Tool execution mode.
    pub tool_execution: ToolExecutionMode,
}

/// How tool calls are executed within a single assistant turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExecutionMode {
    /// Execute tool calls sequentially, one at a time.
    Sequential,
    /// Execute tool calls concurrently (in parallel).
    #[default]
    Parallel,
}

/// Agent runtime configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent name
    pub name: String,
    /// Agent description
    pub description: Option<String>,
    /// Model ID to use
    pub model_id: String,
    /// System prompt
    pub system_prompt: Option<String>,
    /// Timeout in seconds for the entire agent run
    pub timeout_seconds: u64,
    /// Temperature for generation (0.0 to 1.0)
    pub temperature: Option<f64>,
    /// Maximum tokens to generate
    pub max_tokens: Option<usize>,
    /// Compaction strategy for long conversations
    #[serde(default)]
    pub compaction_strategy: CompactionStrategy,
    /// Custom instruction passed to the compactor
    #[serde(default)]
    pub compaction_instruction: Option<String>,
    /// Model context window size (used for threshold-based compaction)
    #[serde(default = "default_context_window")]
    pub context_window: usize,
    /// API key override for the provider.
    ///
    /// When set, this is injected into [`oxi_ai::StreamOptions`] so the
    /// provider uses it instead of an environment variable.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Working directory for file tools. Defaults to current directory if None.
    #[serde(default)]
    pub workspace_dir: Option<std::path::PathBuf>,
    /// Output mode for agent responses.
    ///
    /// When set, the agent extracts structured output from the final response.
    /// See [`OutputMode`] for available modes.
    ///
    /// [`OutputMode`]: crate::structured_output::OutputMode
    #[serde(default)]
    pub output_mode: Option<String>,
    /// Session identity used by tools that gate behavior on liveness (e.g. the
    /// `issue` tool's `start`/`close` ownership checks). When `Some`, this value
    /// is threaded through to [`crate::tools::ToolContext::session_id`].
    /// `None` means the tool receives `session_id == None` and ownership-gated
    /// operations will reject the call (defensive default).
    #[serde(default)]
    pub session_id: Option<String>,

    /// Per-provider options for fine-grained control.
    ///
    /// When set, these are passed through to [`oxi_ai::StreamOptions::provider_options`]
    /// so the provider can read provider-specific settings (e.g. Anthropic adaptive
    /// thinking, OpenAI reasoning_effort, Google thinkingConfig).
    #[serde(default)]
    pub provider_options: Option<oxi_ai::ProviderOptions>,

    /// TTSR engine for stream rule checking. When set, streaming output
    /// is checked against registered rules and violations trigger
    /// [`crate::agent_loop::StreamOutcome::RuleInterrupt`].
    #[serde(skip, default)]
    pub ttsr_engine: Option<std::sync::Arc<crate::agent_loop::ttsr::TtsrEngine>>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: "oxi-agent".to_string(),
            description: None,
            model_id: "claude-sonnet-4-20250514".to_string(),
            system_prompt: None,
            timeout_seconds: 300,
            temperature: None,
            max_tokens: None,
            compaction_strategy: CompactionStrategy::default(),
            compaction_instruction: None,
            context_window: 128_000,
            api_key: None,
            workspace_dir: None,
            output_mode: None,
            provider_options: None,
            session_id: None,
            ttsr_engine: None,
        }
    }
}

impl AgentConfig {
    /// Create a new config with the given model ID.
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            ..Default::default()
        }
    }

    /// Set the agent name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the timeout in seconds for the entire agent run.
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    /// Set the compaction strategy for long conversations.
    pub fn with_compaction_strategy(mut self, strategy: CompactionStrategy) -> Self {
        self.compaction_strategy = strategy;
        self
    }

    /// Set a custom instruction passed to the compactor.
    pub fn with_compaction_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.compaction_instruction = Some(instruction.into());
        self
    }

    /// Set the session identity threaded into [`crate::tools::ToolContext::session_id`].
    ///
    /// Tools that gate behavior on liveness (e.g. an `issue` tool's
    /// `start`/`close` ownership checks) use this to identify the caller.
    /// Leaving it `None` causes those tools to see an empty caller id and
    /// reject ownership-gated operations (defensive default).
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_defaults_to_none() {
        let c = AgentConfig::default();
        assert!(c.session_id.is_none(), "default session_id must be None");
    }

    #[test]
    fn with_session_id_sets_the_field() {
        let c = AgentConfig::new("m").with_session_id("proc-42");
        assert_eq!(c.session_id.as_deref(), Some("proc-42"));
    }

    #[test]
    fn session_id_round_trips_through_serde() {
        // Forward-compat: a serialized config with the new field deserializes back.
        let with = AgentConfig::new("m").with_session_id("proc-7");
        let json = serde_json::to_string(&with).unwrap();
        assert!(json.contains("\"session_id\":"));
        let back: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_id.as_deref(), Some("proc-7"));

        // Backward-compat: a payload WITHOUT the session_id key must still
        // deserialize and default the field to None. We build that payload by
        // serializing a config, then stripping the key with serde_json::Value.
        let mut v: serde_json::Value =
            serde_json::from_str(&json).expect("config serializes to valid JSON");
        if let Some(obj) = v.as_object_mut() {
            obj.remove("session_id");
        }
        let stripped = serde_json::to_string(&v).unwrap();
        let legacy: AgentConfig = serde_json::from_str(&stripped).unwrap();
        assert!(
            legacy.session_id.is_none(),
            "payload missing session_id must default to None"
        );
    }
}
