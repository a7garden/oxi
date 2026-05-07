/// Agent loop configuration types

/// Default max auto-retry attempts (for errors detected in assistant
/// messages, not provider-level errors).
#[allow(dead_code)]
pub const AUTO_RETRY_MAX_ATTEMPTS: usize = 3;

/// Default base delay in ms for exponential backoff during auto-retry.
#[allow(dead_code)]
pub const AUTO_RETRY_BASE_DELAY_MS: u64 = 2000;

/// Configuration for an [`crate::AgentLoop`] instance.
#[derive(Clone)]
pub struct AgentLoopConfig {
    /// Model identifier in `provider/model` format.
    pub model_id: String,
    /// Optional system prompt prepended to every request.
    pub system_prompt: Option<String>,
    /// Sampling temperature (0.0 – 2.0).
    pub temperature: f32,
    /// Maximum tokens the model may generate per request.
    pub max_tokens: u32,
    /// Maximum number of assistant turns before the loop stops.
    pub max_iterations: usize,
    /// Whether tool calls run in parallel or sequentially.
    pub tool_execution: ToolExecutionMode,
    /// Compaction strategy for managing context window usage.
    pub compaction_strategy: oxi_ai::CompactionStrategy,
    /// Approximate context window size in tokens.
    pub context_window: usize,
    /// Optional instruction injected into the compaction prompt.
    pub compaction_instruction: Option<String>,
    /// Optional session identifier for logging and tracing.
    pub session_id: Option<String>,
    /// Optional transport override (e.g. "sse", "stdio").
    pub transport: Option<String>,
    /// Whether to trigger compaction before the first turn.
    pub compact_on_start: bool,
    /// Optional cap on retry back-off delay (milliseconds).
    pub max_retry_delay_ms: Option<u64>,
    /// Enable automatic retry on retryable assistant errors.
    pub auto_retry_enabled: bool,
    /// Maximum number of auto-retry attempts.
    pub auto_retry_max_attempts: usize,
    /// Base delay in milliseconds for auto-retry exponential back-off.
    pub auto_retry_base_delay_ms: u64,
    /// API key override for the provider.
    ///
    /// When set, this is injected into [`oxi_ai::StreamOptions`] so the
    /// provider uses it instead of an environment variable.
    pub api_key: Option<String>,
}

/// Controls whether tool calls execute concurrently or one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    /// Execute all tool calls concurrently.
    Parallel,
    /// Execute tool calls one at a time, in order.
    Sequential,
}

impl Default for ToolExecutionMode {
    fn default() -> Self {
        ToolExecutionMode::Parallel
    }
}

use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;
use anyhow::{Error, Result};
use crate::AgentToolResult;
use serde_json::Value;

/// Hook invoked before each tool call; may return an override result.
pub type BeforeToolCallHook = Arc<
    dyn Fn(&str, &Value) -> Pin<Box<dyn Future<Output = Result<Option<AgentToolResult>, Error>> + Send>>
        + Send + Sync,
>;

/// Hook invoked after each tool call; may return a modified result.
pub type AfterToolCallHook = Arc<
    dyn Fn(&str, &AgentToolResult) -> Pin<Box<dyn Future<Output = Result<Option<AgentToolResult>, Error>> + Send>>
        + Send + Sync,
>;

// MAX_RETRIES and BACKOFF_BASE_SECS are now defined in crate::stream_retry
// and re-exported from crate::agent_loop::retry.
pub use crate::stream_retry::{MAX_RETRIES, BACKOFF_BASE_SECS};
