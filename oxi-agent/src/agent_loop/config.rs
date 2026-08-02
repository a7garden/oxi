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
    /// Working directory for file tools. Defaults to current directory if None.
    pub workspace_dir: Option<std::path::PathBuf>,
    /// Per-provider options for fine-grained control.
    ///
    /// Passed through to [`oxi_ai::StreamOptions::provider_options`] so the
    /// provider can read provider-specific settings.
    pub provider_options: Option<oxi_ai::ProviderOptions>,

    /// Async hook invoked after context compaction completes.
    ///
    /// Unlike the `Compaction` event in the `Fn` callback, this hook is
    /// async and its future is awaited within the agent loop. Errors are
    /// logged at WARN level but don't fail the loop.
    ///
    /// Use this for side effects that require async I/O (e.g., persisting
    /// compaction summaries to a memory store) without resorting to
    /// `tokio::spawn` fire-and-forget.
    pub on_compaction: Option<CompactionHook>,
    /// Snapshot store for hashline edit mode.
    pub snapshot_store: Option<Arc<dyn oxi_hashline::SnapshotStore>>,
    /// Memory backend for memory tools.
    pub memory: Option<Arc<dyn crate::tools::MemoryBackend>>,
    /// URL resolver for internal protocol schemes.
    pub url_resolver: Option<Arc<dyn crate::tools::UrlResolver>>,
    /// Todo state provider for the `todo` tool.
    pub todo: Option<Arc<dyn crate::tools::TodoStateProvider>>,
    /// Agent pool for Hub display.
    pub agent_pool: Option<Arc<dyn crate::tools::AgentPoolProvider>>,
    /// LSP provider for the `lsp` tool.
    pub lsp: Option<Arc<dyn crate::tools::LspProvider>>,
    /// TTSR engine for stream rule checking.
    pub ttsr_engine: Option<Arc<crate::agent_loop::ttsr::TtsrEngine>>,
    /// In-process sub-agent runner (issue #28 gap 3).
    /// When `Some`, the `subagent` tool prefers an in-process isolated
    /// run over shelling out to the CLI. Library consumers set this so
    /// delegation works without an `oxi` subprocess.
    pub subagent_runner: Option<Arc<dyn crate::tools::SubagentRunner>>,
    /// Current sub-agent nesting depth (issue #28 gap 3).
    ///
    /// The CLI backend uses env vars (`OXI_SUBAGENT_DEPTH`) for this,
    /// which is safe because each subprocess has its own env. The
    /// in-process backend **cannot** use env vars (concurrent
    /// `set_var` is UB; state leaks between forks), so it reads this
    /// field instead. Default 0 (top-level). The `subagent` tool
    /// increments this when creating a forked `AgentLoopConfig`, and
    /// the fork checks it against the agent definition's
    /// `max_subagent_depth` to cap recursion.
    pub subagent_depth: u8,
    /// Maximum size (in bytes) of a single tool result's text content
    /// before it is truncated (issue #28 gap 1).
    ///
    /// When set, tool results exceeding this limit are truncated to
    /// the limit and a marker is appended:
    /// `"... [truncated: N bytes omitted]"`. This prevents a single
    /// large tool output (e.g. reading a huge file, verbose bash
    /// output) from consuming the entire context window.
    ///
    /// `None` (default) = no limit. Opt-in — existing behavior is
    /// preserved.
    pub max_tool_result_bytes: Option<usize>,
    /// Enable thinking-loop detection in the streaming layer. When true,
    /// each `ThinkingDelta` is fed to a detector that recognises verbatim
    /// tail repetition, near-duplicate paragraph clusters, and
    /// progress-lexicon stalls. On detection the stream is aborted with
    /// a transient error so the retry layer resamples.
    ///
    /// Default: `true`. Set to `false` to disable (e.g. for tests that
    /// exercise specific failure modes).
    pub thinking_loop_detection: bool,
    /// Settings for the cross-turn tool-call loop guard. When the same
    /// single-tool call repeats past the threshold, the agent emits a
    /// steering message to break the loop. Default: threshold 5, with
    /// `read`/`ls`/`grep` exempt.
    pub tool_call_loop_guard: oxi_ai::utils::tool_call_loop::ToolCallLoopGuardOptions,
    /// Approval/tier configuration for gating tool execution.
    ///
    /// When configured, tool calls at tiers in `require_approval_for` are
    /// checked against the approval hook before execution. Default: no
    /// approval gating (all tools allowed without check).
    pub approval_config: ApprovalConfig,

    /// Soft tool requirements: tools the agent should call.
    ///
    /// On the first turn where a soft-required tool is missing, the loop
    /// injects a reminder steering message. On the second consecutive miss,
    /// it escalates. Default: empty (no soft requirements).
    pub soft_requirements: Vec<SoftRequirement>,
    /// Enable GPT-5 Harmony protocol leak detection.
    ///
    /// When `true`, each text delta is scanned for Harmony markers
    /// (`to=functions.xxx`, `<|start|>`, etc.). On detection, the stream
    /// is aborted, a `HarmonyLeakDetected` event is emitted, and the
    /// turn is restarted. Default: `false`.
    pub harmony_leak_detection: bool,

    /// Owned (in-band) tool-calling dialect.
    ///
    /// When `Some`, the loop targets models **without native tool support**:
    /// it sends no native `tools`, injects the tool catalog into the system
    /// prompt, re-encodes prior tool calls/results as text in the history, and
    /// parses the model's text output back into canonical tool calls. Mirrors
    /// omp's `AgentLoopConfig.dialect` / `PI_DIALECT`.
    ///
    /// `None` (default) keeps provider-native tool calling.
    pub dialect: Option<oxi_ai::dialect::Dialect>,

    /// Optional circuit breaker for provider calls. When `Some`, the agent
    /// loop's retry path consults the breaker before each provider attempt
    /// (`breaker.check()`); an open circuit short-circuits the retry loop
    /// and returns immediately (the breaker's whole purpose is to stop
    /// hammering a failing upstream). On every successful call the breaker
    /// records success; on every error it records failure. When `None`, no
    /// circuit breaking occurs (default — preserves existing behavior).
    ///
    /// This is the SDK-owned behavior trait + reference impl; consumers
    /// implement [`oxi_ai::circuit_breaker::CircuitBreaker`] for their domain profile
    /// (A2A, HTTP, etc.) and pass the impl here. See
    /// `docs/oxi-sdk-ownership.md` §3.
    pub circuit_breaker: Option<oxi_ai::circuit_breaker::SharedBreaker>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            model_id: String::new(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            tool_execution: ToolExecutionMode::Parallel,
            compaction_strategy: oxi_ai::CompactionStrategy::default(),
            context_window: 128_000,
            compaction_instruction: None,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
            auto_retry_enabled: false,
            auto_retry_max_attempts: 3,
            auto_retry_base_delay_ms: 2000,
            workspace_dir: None,
            max_tool_result_bytes: None,
            provider_options: None,
            on_compaction: None,
            snapshot_store: None,
            memory: None,
            url_resolver: None,
            todo: None,
            agent_pool: None,
            lsp: None,
            ttsr_engine: None,
            subagent_runner: None,
            subagent_depth: 0,
            thinking_loop_detection: true,
            approval_config: ApprovalConfig::default(),
            soft_requirements: Vec::new(),
            harmony_leak_detection: false,
            dialect: None,
            circuit_breaker: None,
            tool_call_loop_guard: oxi_ai::utils::tool_call_loop::ToolCallLoopGuardOptions::default(
            ),
        }
    }
}

// Re-export ToolExecutionMode from crate::config to avoid duplicate definitions.
pub use crate::config::ToolExecutionMode;

use crate::AgentToolResult;
use crate::compaction::CompactedContext;
use anyhow::{Error, Result};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Async hook invoked after context compaction completes.
///
/// Receives the [`CompactedContext`] and returns a `Result<()>` future.
/// The future is awaited within the agent loop, so async operations
/// (memory storage, logging, etc.) are safe here.
///
/// # Example
///
/// ```ignore
/// let config = AgentLoopConfig {
///     on_compaction: Some(Arc::new(|ctx: CompactedContext| {
///         let summary = ctx.summary.clone();
///         Box::pin(async move {
///             memory_store.save(summary).await
///         })
///     })),
///     ..Default::default()
/// };
/// ```
pub type CompactionHook =
    Arc<dyn Fn(CompactedContext) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

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

// ── Approval system types ────────────────────────────────────────────────

/// Decision returned by the approval hook for a tool call.
#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    /// Allow without conditions.
    Allow,
    /// Deny with a reason.
    Deny(String),
    /// Request human approval with a reason.
    RequireApproval(String),
}

/// Async hook invoked before tool execution to check approval.
///
/// Receives the tool name and parsed arguments. Returns an `ApprovalDecision`.
/// When `None` is returned (no hook registered), all tools are allowed.
pub type ApprovalHook = Arc<
    dyn Fn(&str, &Value) -> Pin<Box<dyn Future<Output = Result<ApprovalDecision, Error>> + Send>>
        + Send
        + Sync,
>;

/// Configuration for the approval/tier system.
///
/// Controls which tool tiers require approval before execution.
/// When `hook` is `None`, all tools are allowed regardless of tier.
/// When empty `require_approval_for`, no tiers trigger approval checks.
///
/// Default: no tiers require approval (opt-in only).
#[derive(Clone, Default)]
pub struct ApprovalConfig {
    /// Tool tiers that require approval before execution.
    /// Empty = no approval gating.
    pub require_approval_for: Vec<crate::tools::ToolTier>,
    /// Approval hook. When `None`, decisions are permissive.
    pub hook: Option<ApprovalHook>,
}

// ── Soft requirement types ──────────────────────────────────────────────

/// State for soft requirement tracking across turns.
///
/// Tracks which soft-required tools have been reminded/escalated.
/// Resets when all soft requirements are satisfied or config changes.
#[derive(Debug, Clone, Default)]
pub struct SoftRequirementState {
    /// Set of tool names that have been reminded (missed once).
    /// When a tool appears here and is still missing next turn, escalate.
    pub reminded: std::collections::HashSet<String>,
}

/// Soft requirement: a tool the agent should ideally call.
/// First miss → reminder; second miss → escalation.
#[derive(Debug, Clone)]
pub struct SoftRequirement {
    /// Tool name to check for.
    pub tool_name: String,
    /// Reason shown to the model.
    pub reason: String,
}

// MAX_RETRIES and BACKOFF_BASE_SECS are now defined in crate::stream_retry
// and re-exported from crate::agent_loop::retry.
pub use crate::stream_retry::{BACKOFF_BASE_SECS, MAX_RETRIES};
