//! Agent loop configuration types

/// Default max auto-retry attempts (for errors detected in assistant
/// messages, not provider-level errors).
#[allow(dead_code)]
pub const AUTO_RETRY_MAX_ATTEMPTS: usize = 3;

/// Default base delay in ms for exponential backoff during auto-retry.
#[allow(dead_code)]
pub const AUTO_RETRY_BASE_DELAY_MS: u64 = 2000;

#[derive(Clone)]
/// AgentLoopConfig.
pub struct AgentLoopConfig {
    pub model_id: String,
    pub system_prompt: Option<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_iterations: usize,
    pub tool_execution: ToolExecutionMode,
    pub compaction_strategy: oxi_ai::CompactionStrategy,
    pub context_window: usize,
    pub compaction_instruction: Option<String>,
    pub session_id: Option<String>,
    pub transport: Option<String>,
    pub compact_on_start: bool,
    pub max_retry_delay_ms: Option<u64>,
    pub auto_retry_enabled: bool,
    pub auto_retry_max_attempts: usize,
    pub auto_retry_base_delay_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// ToolExecutionMode.
pub enum ToolExecutionMode {
/// parallel variant.
    Parallel,
/// sequential variant.
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

pub type BeforeToolCallHook = Arc<
    dyn Fn(&str, &Value) -> Pin<Box<dyn Future<Output = Result<Option<AgentToolResult>, Error>> + Send>>
        + Send + Sync,
>;

pub type AfterToolCallHook = Arc<
    dyn Fn(&str, &AgentToolResult) -> Pin<Box<dyn Future<Output = Result<Option<AgentToolResult>, Error>> + Send>>
        + Send + Sync,
>;

/// Maximum retry attempts for provider stream requests.
pub const MAX_RETRIES: usize = 3;

/// Base delay in seconds for exponential backoff.
pub const BACKOFF_BASE_SECS: u64 = 2;