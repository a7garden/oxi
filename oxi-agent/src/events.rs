/// Agent event system
/// Defines all events emitted during an agent run, including lifecycle,
/// streaming, tool execution, compaction, retry, and steering events.
use crate::compaction::CompactionEvent;
use serde::{Deserialize, Serialize};

// ── Tool context types ────────────────────────────────────────────────────

/// Semantic context for a tool execution event.
///
/// Carries structured information about *what* a tool call means,
/// derived from the tool name and arguments by the agent loop.
/// UI consumers that understand a context variant can render it
/// richly; older consumers simply ignore the field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolCallContext {
    // ── Web exploration ──────────────────────────────────────
    /// A search engine query.
    WebSearch {
        /// The search query string.
        query: String,
        /// Search engine used (e.g. "duckduckgo").
        #[serde(skip_serializing_if = "Option::is_none")]
        engine: Option<String>,
    },

    /// Visiting a web page.
    PageVisit {
        /// URL being visited.
        url: String,
        /// Why this page is being visited.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<VisitReason>,
        // ── Result fields (enriched by BrowseProgress::DocumentReady) ──
        /// Page `<title>` after load.
        #[serde(skip_serializing_if = "Option::is_none")]
        page_title: Option<String>,
        /// HTTP status code.
        #[serde(skip_serializing_if = "Option::is_none")]
        page_status: Option<u16>,
        /// HTML body size in bytes.
        #[serde(skip_serializing_if = "Option::is_none")]
        page_bytes: Option<u64>,
        /// Wall-clock page load duration in milliseconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        page_duration_ms: Option<u64>,
        // ── Error / enrichment fields ──
        /// Navigation error message (from BrowseProgress::NavigationFailed).
        #[serde(skip_serializing_if = "Option::is_none")]
        navigation_error: Option<String>,
        /// Screenshot metadata (from BrowseProgress::ScreenshotCaptured).
        #[serde(skip_serializing_if = "Option::is_none")]
        screenshot: Option<ScreenshotMeta>,
    },

    /// Extracting data from a web page.
    DataExtraction {
        /// Description of what is being extracted (e.g. CSS selector).
        target: String,
        /// URL of the page being extracted from.
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        // ── Result fields (enriched by BrowseProgress::DocumentReady) ──
        /// Number of items extracted.
        #[serde(skip_serializing_if = "Option::is_none")]
        result_count: Option<usize>,
        /// HTTP status code of the page.
        #[serde(skip_serializing_if = "Option::is_none")]
        page_status: Option<u16>,
        /// Page load duration in milliseconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        page_duration_ms: Option<u64>,
    },

    /// An action within a persistent browser session.
    SessionAction {
        /// The session action being performed (e.g. "goto", "click").
        action: String,
        /// URL if the action involves navigation.
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },

    /// A step within a browse script.
    ScriptStep {
        /// Current step index (1-based).
        current: usize,
        /// Total number of steps.
        total: usize,
        /// Human-readable step description.
        step: String,
    },
}

/// Screenshot metadata attached to PageVisit context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotMeta {
    /// PNG payload size in bytes.
    pub bytes: usize,
    /// Viewport width.
    pub width: u32,
    /// Capture duration in milliseconds.
    pub duration_ms: u64,
}

/// Reason for visiting a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisitReason {
    /// The agent specified the URL directly.
    DirectNavigation,
    /// Clicked a search result at the given position.
    SearchResult {
        /// 1-based position in search results.
        position: usize,
    },
    /// Followed a link from another page.
    LinkFollowed {
        /// The URL the link was on.
        from_url: String,
    },
}

/// Events emitted during agent execution.
///
/// Events are tagged with `type` and serialized as camelCase for JSON consumers.
/// This enum is `#[non_exhaustive]` — new variants may be added in future releases.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum AgentEvent {
    // ── Lifecycle events ──────────────────────────────────────────────
    /// Emitted when the agent begins processing a batch of prompts.
    AgentStart {
        /// The initial prompt messages sent to the agent.
        prompts: Vec<oxi_ai::Message>,
        /// Optional session identifier for correlation.
        session_id: Option<String>,
    },

    /// Emitted when the agent finishes all processing.
    AgentEnd {
        /// Final conversation messages.
        messages: Vec<oxi_ai::Message>,
        /// Why the agent stopped (e.g. `"end_turn"`, `"tool_use"`).
        stop_reason: Option<String>,
        /// Optional session identifier for correlation.
        session_id: Option<String>,
    },

    /// Emitted at the start of each agent loop turn.
    TurnStart {
        /// Zero-based turn index.
        turn_number: u32,
    },

    /// Emitted when a turn completes, including the assistant reply and tool results.
    TurnEnd {
        /// Turn index that just completed.
        turn_number: u32,
        /// The assistant message produced this turn.
        assistant_message: oxi_ai::Message,
        /// Tool results collected during this turn.
        tool_results: Vec<oxi_ai::ToolResultMessage>,
    },

    // ── Message events ────────────────────────────────────────────────
    /// A new message has been created in the conversation.
    MessageStart {
        /// The message that started.
        message: oxi_ai::Message,
    },

    /// A message has been updated with new content.
    MessageUpdate {
        /// The message in its current state.
        message: oxi_ai::Message,
        /// Incremental text delta since the last update, if available.
        delta: Option<String>,
    },

    /// A message has been finalized.
    MessageEnd {
        /// The completed message.
        message: oxi_ai::Message,
    },

    // ── Tool execution events ────────────────────────────────────────
    /// A tool is about to be executed.
    ToolExecutionStart {
        /// Unique identifier for this tool call.
        tool_call_id: String,
        /// Name of the tool being invoked.
        tool_name: String,
        /// JSON arguments passed to the tool.
        args: serde_json::Value,
        /// Intent trace — a concise description of what this tool call does.
        /// `None` for tools without intent tracing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<String>,
        /// Semantic context inferred from tool name and arguments.
        /// `None` for tools without a known context mapping.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ToolCallContext>,
    },

    /// Partial progress from a running tool execution.
    ToolExecutionUpdate {
        /// Identifier of the tool call producing the update.
        tool_call_id: String,
        /// Name of the tool.
        tool_name: String,
        /// Partial result text so far.
        partial_result: String,
        /// Browser tab id that produced this progress (if the tool is
        /// tab-aware). `None` for tools that don't have a tab concept,
        /// or for older tool implementations that don't propagate tab ids.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab_id: Option<uuid::Uuid>,
        /// Semantic context inferred from tool name and arguments.
        /// Carries structured information about what this update means.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ToolCallContext>,
    },

    /// A tool execution has finished.
    ToolExecutionEnd {
        /// Identifier of the completed tool call.
        tool_call_id: String,
        /// Name of the tool.
        tool_name: String,
        /// Intent trace — a concise description of what this tool call did.
        /// `None` for tools without intent tracing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<String>,
        /// The tool result payload.
        result: oxi_ai::ToolResult,
        /// Whether the tool execution resulted in an error.
        is_error: bool,
    },

    // ── Streaming tool-call events ───────────────────────────────────
    /// Partial tool-call arguments streamed by the LLM while it is still
    /// constructing a tool call. Emitted between the provider's
    /// `ToolCallStart` and `ToolCallEnd`, before [`AgentEvent::ToolExecutionStart`].
    ///
    /// Each `args_delta` is a raw JSON fragment (not valid JSON on its own) —
    /// downstream consumers accumulate per `tool_call_id`.
    ToolCallDelta {
        /// Tool call identifier (matches the id later carried by
        /// `ToolExecutionStart`).
        tool_call_id: String,
        /// Raw JSON argument fragment from the LLM stream.
        args_delta: String,
    },

    // ── Legacy events (kept for backward compatibility) ──────────
    /// Legacy: agent started processing a prompt.
    #[serde(rename = "start")]
    Start {
        /// The user prompt that triggered the run.
        prompt: String,
    },

    /// Agent is waiting for the first response token.
    Thinking,

    /// Incremental thinking / reasoning text from the model.
    ThinkingDelta {
        /// The reasoning text delta.
        text: String,
    },

    /// The model finished a reasoning/thinking span and is about to produce
    /// the answer (or begin another content block). Signal-only (no payload).
    ///
    /// Emitted when the provider reports `ThinkingEnd`. For models that
    /// interleave reasoning and text (Claude 4, o-series) this may fire more
    /// than once per turn — once per thinking span.
    ThinkingEnd,

    /// A chunk of generated text from the model.
    TextChunk {
        /// The text delta to append.
        text: String,
    },

    /// The model requested a tool call.
    ToolCall {
        /// The tool call descriptor from the provider.
        tool_call: oxi_ai::ToolCall,
    },

    /// A tool execution has started.
    ToolStart {
        /// Identifier of the tool call.
        tool_call_id: String,
        /// Name of the tool being invoked.
        tool_name: String,
        /// JSON arguments for the tool call.
        #[serde(default)]
        arguments: serde_json::Value,
    },

    /// Progress update from a running tool.
    ToolProgress {
        /// Identifier of the tool call.
        tool_call_id: String,
        /// Human-readable progress message.
        message: String,
    },

    /// A tool execution has completed.
    ToolComplete {
        /// The tool result payload.
        result: oxi_ai::ToolResult,
    },

    /// A tool execution failed.
    ToolError {
        /// Identifier of the failed tool call.
        tool_call_id: String,
        /// Error description.
        error: String,
    },

    /// The agent produced a final response.
    Complete {
        /// Full response text.
        content: String,
        /// Stop reason string (e.g. `"EndTurn"`).
        stop_reason: String,
    },

    /// An error occurred during agent execution.
    Error {
        /// Human-readable error message.
        message: String,
        /// Optional session identifier.
        session_id: Option<String>,
    },

    /// Agent loop iteration counter update.
    Iteration {
        /// Current iteration number.
        number: usize,
    },

    /// Token usage report for a completed turn.
    Usage {
        /// Number of prompt / input tokens consumed.
        input_tokens: usize,
        /// Number of completion / output tokens produced.
        output_tokens: usize,
    },

    /// Context compaction lifecycle event.
    Compaction {
        /// The underlying compaction event detail.
        event: CompactionEvent,
    },

    /// The agent is retrying after a transient error.
    Retry {
        /// Current retry attempt (1-based).
        attempt: usize,
        /// Maximum number of retries allowed.
        max_retries: usize,
        /// Seconds until the next attempt.
        retry_after_secs: u64,
        /// Why the previous attempt failed.
        reason: String,
        /// Optional session identifier.
        session_id: Option<String>,
    },

    /// A TTSR rule violation was detected during streaming.
    /// The stream was aborted and a system reminder will be injected.
    TtsrInterrupt {
        /// Name of the violated rule.
        rule_name: String,
        /// Session identifier for logging.
        session_id: Option<String>,
    },
    /// The agent run was cancelled by the caller.
    Cancelled,

    /// A partial response delivered mid-stream (useful for UI rendering).
    PartialResponse {
        /// Accumulated response content so far.
        content: String,
    },

    // ── Auto-retry events ─────────────────────────────────────────
    /// An automatic retry attempt is starting.
    AutoRetryStart {
        /// Current retry attempt (1-based).
        attempt: usize,
        /// Total retry attempts that will be made.
        max_attempts: usize,
        /// Milliseconds before this attempt is sent.
        delay_ms: u64,
        /// The error that triggered the retry.
        error_message: String,
    },

    /// An automatic retry attempt has concluded.
    AutoRetryEnd {
        /// Whether the retry succeeded.
        success: bool,
        /// Which attempt this was (1-based).
        attempt: usize,
        /// Final error if the retry failed, `None` on success.
        final_error: Option<String>,
    },

    // ── Loop-specific steering events ─────────────────────────────
    /// A system-level steering message injected into the conversation.
    SteeringMessage {
        /// The steering message to add to the context.
        message: oxi_ai::Message,
    },

    /// A follow-up message appended to continue the conversation.
    FollowUpMessage {
        /// The follow-up message.
        message: oxi_ai::Message,
    },
}

impl AgentEvent {
    /// Returns `true` if this event represents the end of the agent lifecycle.
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentEvent::AgentEnd { .. })
    }

    /// Returns the snake_case variant name of this event (useful for logging / serialization).
    pub fn type_name(&self) -> &'static str {
        match self {
            AgentEvent::AgentStart { .. } => "agent_start",
            AgentEvent::AgentEnd { .. } => "agent_end",
            AgentEvent::TurnStart { .. } => "turn_start",
            AgentEvent::TurnEnd { .. } => "turn_end",
            AgentEvent::MessageStart { .. } => "message_start",
            AgentEvent::MessageUpdate { .. } => "message_update",
            AgentEvent::MessageEnd { .. } => "message_end",
            AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
            AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
            AgentEvent::ToolCallDelta { .. } => "tool_call_delta",
            AgentEvent::Start { .. } => "start",
            AgentEvent::Thinking => "thinking",
            AgentEvent::ThinkingDelta { .. } => "thinking_delta",
            AgentEvent::ThinkingEnd => "thinking_end",
            AgentEvent::TextChunk { .. } => "text_chunk",
            AgentEvent::ToolCall { .. } => "tool_call",
            AgentEvent::ToolStart { .. } => "tool_start",
            AgentEvent::ToolProgress { .. } => "tool_progress",
            AgentEvent::ToolComplete { .. } => "tool_complete",
            AgentEvent::ToolError { .. } => "tool_error",
            AgentEvent::Complete { .. } => "complete",
            AgentEvent::Error { .. } => "error",
            AgentEvent::Iteration { .. } => "iteration",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::Compaction { .. } => "compaction",
            AgentEvent::Retry { .. } => "retry",
            AgentEvent::TtsrInterrupt { .. } => "ttsr_interrupt",
            AgentEvent::Cancelled => "cancelled",
            AgentEvent::PartialResponse { .. } => "partial_response",
            AgentEvent::AutoRetryStart { .. } => "auto_retry_start",
            AgentEvent::AutoRetryEnd { .. } => "auto_retry_end",
            AgentEvent::SteeringMessage { .. } => "steering_message",
            AgentEvent::FollowUpMessage { .. } => "follow_up_message",
        }
    }
}
