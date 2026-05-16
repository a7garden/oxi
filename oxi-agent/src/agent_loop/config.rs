//! Agent loop configuration types

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
    /// Working directory for file tools. Defaults to current directory if None.
    pub workspace_dir: Option<std::path::PathBuf>,
}

// Re-export ToolExecutionMode from crate::config to avoid duplicate definitions.
pub use crate::config::ToolExecutionMode;

use crate::AgentToolResult;
use anyhow::{Error, Result};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Hook invoked before each tool call; may return an override result.
pub type BeforeToolCallHook = Arc<
    dyn Fn(
            &str,
            &Value,
        ) -> Pin<Box<dyn Future<Output = Result<Option<AgentToolResult>, Error>> + Send>>
        + Send
        + Sync,
>;

/// Hook invoked after each tool call; may return a modified result.
pub type AfterToolCallHook = Arc<
    dyn Fn(
            &str,
            &AgentToolResult,
        ) -> Pin<Box<dyn Future<Output = Result<Option<AgentToolResult>, Error>> + Send>>
        + Send
        + Sync,
>;

// MAX_RETRIES and BACKOFF_BASE_SECS are now defined in crate::stream_retry
// and re-exported from crate::agent_loop::retry.
pub use crate::stream_retry::{BACKOFF_BASE_SECS, MAX_RETRIES};
