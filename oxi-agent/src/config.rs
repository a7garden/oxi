/// Agent configuration

use oxi_ai::CompactionStrategy;
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone)]
pub struct BeforeToolCallResult {
    /// If `true`, the tool call is blocked and an error result is returned.
    pub block: bool,
    /// Human-readable reason for blocking.
    pub reason: Option<String>,
}

impl Default for BeforeToolCallResult {
    fn default() -> Self {
        Self { block: false, reason: None }
    }
}

/// Result of `afterToolCall` hook.
#[derive(Debug, Clone)]
pub struct AfterToolCallResult {
    /// Override content for the tool result.
    pub content: Option<String>,
    /// Override error status.
    pub is_error: Option<bool>,
    /// Signal that the agent should stop after this batch.
    pub terminate: Option<bool>,
}

impl Default for AfterToolCallResult {
    fn default() -> Self {
        Self { content: None, is_error: None, terminate: None }
    }
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
}

/// Callback hooks for the agent loop.
///
/// These mirror pi-mono's `AgentLoopConfig` hooks, allowing callers to
/// inject custom logic at key points in the agentic loop.
#[derive(Default)]
pub struct AgentHooks {
    /// Called after each turn completes. Return `true` to stop the agent loop.
    pub should_stop_after_turn: Option<Box<dyn Fn(&ShouldStopAfterTurnContext) -> bool + Send + Sync>>,

    /// Called before a tool is executed. Return a `BeforeToolCallResult` with
    /// `block: true` to prevent execution.
    pub before_tool_call: Option<Box<dyn Fn(&BeforeToolCallContext) -> BeforeToolCallResult + Send + Sync>>,

    /// Called after a tool execution completes. Can override the result.
    pub after_tool_call: Option<Box<dyn Fn(&AfterToolCallContext) -> AfterToolCallResult + Send + Sync>>,

    /// Returns steering messages to inject mid-run. Called after each turn
    /// (unless stopped).
    pub get_steering_messages: Option<Box<dyn Fn() -> Vec<String> + Send + Sync>>,

    /// Returns follow-up messages to process after the agent would stop.
    /// Called when the agent has no more tool calls and no steering messages.
    pub get_follow_up_messages: Option<Box<dyn Fn() -> Vec<String> + Send + Sync>>,

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
    /// Maximum number of agent iterations
    pub max_iterations: usize,
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
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: "oxi-agent".to_string(),
            description: None,
            model_id: "claude-sonnet-4-20250514".to_string(),
            system_prompt: None,
            max_iterations: 10,
            timeout_seconds: 300,
            temperature: None,
            max_tokens: None,
            compaction_strategy: CompactionStrategy::default(),
            compaction_instruction: None,
            context_window: 128_000,
            api_key: None,
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

    /// Set the maximum number of agent loop iterations.
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
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
}
