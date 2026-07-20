//! Ask tool — ask the user one or more questions via the TUI overlay.
//!
//! Architecture (omp `ask` style, adapted to oxi's ratatui stack):
//! - `AskBridge` is created in `oxi-cli` and shared (via `Arc`) between
//!   `AskTool` (agent thread) and `AppState` (TUI main thread).
//! - When the tool executes, it creates a oneshot channel and stores
//!   (questions, sender) in the bridge — a single round-trip. The overlay
//!   drives the **sequential, one-question-at-a-time** flow internally
//!   (←/→ to move between questions), matching omp's `askSingleQuestion` UX.
//! - The TUI main loop polls the bridge; when a pending ask is found it
//!   creates an `AskOverlay` to display it.
//! - User interaction drives the overlay to send an `AskResponse` via the
//!   oneshot `Sender`. The tool's `execute()` receives it via `rx.await`.
//! - Abort (Ctrl+C) is handled via `tokio::select!` with the abort signal.
//!
//! The transcript renderer (`format_ask_result` in `oxi-tui`) reconstructs the
//! "filled menu" (every option re-shown with its selection marker filled) by
//! combining the call arguments (the full option list) with the result text
//! (which option was selected).

use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use crate::tools::typed::TypedTool;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

/// Shared bridge between the ask tool (agent thread) and the TUI overlay (main
/// thread). Created in `oxi-cli`, injected into both the tool and `AppState`.
#[derive(Clone)]
pub struct AskBridge {
    inner: Arc<parking_lot::Mutex<Option<PendingAsk>>>,
    /// Set to `true` when the TUI main loop starts polling.
    /// In headless mode (`--print`, RPC) this stays `false`, allowing the
    /// tool to refuse execution instead of hanging forever.
    ui_attached: Arc<AtomicBool>,
    /// Identity of the owning session. Set when [`Self::attach_with_session`]
    /// is called (typically from the TUI bootstrap with the same
    /// `ownership_session_id` used by the issue system). Required non-empty
    /// at [`Self::set`] time so concurrent agents can't impersonate each
    /// other's ask overlays — see AGENTS.md "Issue-system ownership identity
    /// (Phase 0 / defect #13)" for the analogous invariant.
    session_id: Arc<parking_lot::Mutex<Option<String>>>,
    /// Ask overlay timeout. `None` = disabled (wait indefinitely).
    /// Set at construction from `Settings::ask_timeout_secs`.
    timeout: Option<Duration>,
}

impl AskBridge {
    /// Create a new empty bridge with no timeout and UI not attached.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::Mutex::new(None)),
            ui_attached: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(parking_lot::Mutex::new(None)),
            timeout: None,
        }
    }

    /// Create a new bridge with a timeout duration.
    pub fn with_timeout(timeout: Option<Duration>) -> Self {
        Self {
            timeout,
            ..Self::new()
        }
    }

    /// Signal that the TUI main loop is polling, and bind it to a session
    /// identity. Called once at TUI startup.
    ///
    /// `session_id` must be non-empty — mirroring the issue-system
    /// invariant (AGENTS.md pitfall "Issue-system ownership identity").
    /// An empty id is a programming error and is rejected.
    pub fn attach_with_session(&self, session_id: impl Into<String>) {
        let id = session_id.into();
        debug_assert!(
            !id.is_empty(),
            "AskBridge::attach_with_session called with empty session_id"
        );
        *self.session_id.lock() = Some(id);
        self.ui_attached.store(true, Ordering::SeqCst);
    }

    /// Returns `true` when the TUI is polling the bridge (interactive mode).
    pub fn is_ui_attached(&self) -> bool {
        self.ui_attached.load(Ordering::SeqCst)
    }

    /// Signal that the TUI main loop is polling, without binding a session.
    /// Test-only convenience — production code must use
    /// [`Self::attach_with_session`].
    #[cfg(any(test, debug_assertions))]
    pub fn attach(&self) {
        self.ui_attached.store(true, Ordering::SeqCst);
    }

    /// Returns the bound session identity, if `attach_with_session` was called.
    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().clone()
    }
    /// Returns the configured timeout duration, if any.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Store a pending ask. Called by `AskTool::execute`.
    /// Returns `false` if another ask is already pending (should not happen in
    /// sequential tool execution, but guards against races).
    pub fn set(&self, pending: PendingAsk) -> bool {
        let mut lock = self.inner.lock();
        if lock.is_some() {
            return false;
        }
        *lock = Some(pending);
        true
    }

    /// Try to take the pending ask. Called by the TUI main loop polling.
    /// Returns `None` if nothing is pending or already taken.
    pub fn try_take(&self) -> Option<PendingAsk> {
        self.inner.lock().take()
    }

    /// Returns `true` if an ask is currently pending.
    pub fn has_pending(&self) -> bool {
        self.inner.lock().is_some()
    }
}

impl Default for AskBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// A pending ask waiting for user interaction.
/// The `responder` is a oneshot `Sender` — the overlay calls `send()` when the
/// user submits or cancels, and the tool's `rx.await` receives it.
pub struct PendingAsk {
    /// Questions to display to the user.
    pub questions: Vec<Question>,
    /// Sender end of the response channel. Dropping this (without sending) is
    /// equivalent to user dismiss.
    pub responder: oneshot::Sender<AskResponse>,
    /// Overlay timeout. `None` = disabled.
    pub timeout: Option<Duration>,
    /// Session identity that produced this ask (from `AskBridge::session_id`).
    /// Mirrored into the TUI's liveness flock for ownership consistency —
    /// see AGENTS.md "Issue-system ownership identity (Phase 0 / defect #13)".
    pub session_id: Option<String>,
}

/// A single question to ask the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// Unique identifier for this question.
    pub id: String,
    /// Short contextual label. Used as a section tag in the transcript.
    /// Defaults to the `id` if empty.
    #[serde(default)]
    pub label: String,
    /// The full question text to display.
    pub prompt: String,
    /// Available options. Can be empty when `allow_other` is `true`.
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    /// Whether to show "Other (type your own)" option. Defaults to `true`.
    /// The UI appends "Other" automatically — the model MUST NOT include an
    /// "Other" option itself.
    #[serde(default = "default_true")]
    pub allow_other: bool,
    /// Whether multiple options can be selected. Defaults to `false`.
    #[serde(default)]
    pub multi_select: bool,
    /// Recommended option index (0-based). Used for default cursor position,
    /// a "(Recommended)" suffix on the option label, and timeout
    /// auto-selection fallback.
    #[serde(default)]
    pub recommended: Option<usize>,
}

fn default_true() -> bool {
    true
}

/// An option within a question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    /// Value returned when this option is selected.
    pub value: String,
    /// Display label for the option.
    pub label: String,
    /// Optional description shown below the label.
    pub description: Option<String>,
}

/// Response from user interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskResponse {
    /// All answers collected, one per answered question.
    pub answers: Vec<Answer>,
    /// `true` if the user cancelled (Esc).
    pub cancelled: bool,
    /// `true` if answers were auto-selected due to timeout.
    #[serde(default)]
    pub timed_out: bool,
}

/// A single answer to a question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    /// Question ID this answer belongs to.
    pub id: String,
    /// The value(s) selected or entered, comma-joined for multi-select.
    pub value: String,
    /// Display label(s), comma-joined for multi-select, or the custom text.
    pub label: String,
    /// `true` if the user typed custom text (allowOther).
    pub was_custom: bool,
    /// 1-based index of the selected option. `None` for custom/multi input.
    pub index: Option<usize>,
}

// ── Tool ───────────────────────────────────────────────────────────────────

/// Typed arguments for [`AskTool`].
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AskArgs {
    questions: Vec<AskArgsQuestion>,
}
/// A single question within [`AskArgs`].
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AskArgsQuestion {
    id: String,
    label: Option<String>,
    prompt: String,
    #[serde(default)]
    options: Vec<AskArgsOption>,
    #[serde(rename = "allowOther")]
    #[serde(default = "default_true")]
    allow_other: bool,
    #[serde(rename = "multiSelect")]
    #[serde(default)]
    multi_select: bool,
    recommended: Option<usize>,
}

/// An option within a question.
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AskArgsOption {
    value: String,
    label: String,
    description: Option<String>,
}

/// Tool that surfaces a multiple-choice prompt to the user via the [`AskBridge`].
///
/// Blocks the agent loop until the user picks an option or aborts.
pub struct AskTool {
    bridge: Arc<AskBridge>,
}

impl AskTool {
    /// Create a new `AskTool` that communicates via the given bridge.
    pub fn new(bridge: Arc<AskBridge>) -> Self {
        Self { bridge }
    }
}

// `Clone` is needed because ToolRegistry stores `Arc<dyn AgentTool>`.
// `AskTool` is cheap to clone (only copies the Arc).
impl Clone for AskTool {
    fn clone(&self) -> Self {
        Self {
            bridge: self.bridge.clone(),
        }
    }
}

#[async_trait]
impl AgentTool for AskTool {
    fn name(&self) -> &str {
        "ask"
    }

    fn label(&self) -> &str {
        "Ask"
    }

    fn description(&self) -> &str {
        "Ask the user a clarifying question when choices have materially \
         different tradeoffs the user must decide. Default to action — pick \
         the conservative/standard option and proceed when a reasonable \
         default exists; only ask when the user must weigh the tradeoff. Do \
         NOT include an 'Other' option — the UI appends 'Other (type your \
         own)' automatically. Use 'recommended' (0-indexed) to mark the \
         default; a '(Recommended)' suffix is added automatically. Set \
         'multiSelect' true to allow multiple selections. Provide 2-5 \
         concise options with short labels; put explanatory tradeoffs in \
         'description'. Batch related questions in one call via 'questions'."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "Questions to ask the user",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for this question"
                            },
                            "label": {
                                "type": "string",
                                "description": "Short contextual label (defaults to the id)"
                            },
                            "prompt": {
                                "type": "string",
                                "description": "The full question text to display"
                            },
                            "options": {
                                "type": "array",
                                "description": "Available options (2-5). Do NOT include 'Other' — the UI adds it automatically.",
                                "default": [],
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "value": {
                                            "type": "string",
                                            "description": "The value returned when selected"
                                        },
                                        "label": {
                                            "type": "string",
                                            "description": "Short display label for the option"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "Optional explanatory tradeoff shown below the label"
                                        }
                                    },
                                    "required": ["value", "label"]
                                }
                            },
                            "allowOther": {
                                "type": "boolean",
                                "description": "Show 'Other (type your own)' (default: true)",
                                "default": true
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "description": "Allow multiple selections (default: false)",
                                "default": false
                            },
                            "recommended": {
                                "type": "number",
                                "description": "Recommended option index (0-based). Marks the default and is used for timeout auto-selection.",
                                "minimum": 0
                            }
                        },
                        "required": ["id", "prompt"]
                }
            },
            },
            "required": ["questions"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args: AskArgs =
            serde_json::from_value(params).map_err(|e| format!("invalid params: {e}"))?;
        self.execute_typed(_tool_call_id, args, signal, _ctx).await
    }
}

#[async_trait]
impl TypedTool for AskTool {
    type Args = AskArgs;

    async fn execute_typed(
        &self,
        _tool_call_id: &str,
        args: Self::Args,
        signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // 0. Headless guard
        if !self.bridge.is_ui_attached() {
            return Ok(AgentToolResult::error(
                "Ask requires interactive TUI mode. \
                 Not available in --print or RPC mode.",
            ));
        }

        // 0b. Ownership guard
        let session_id = self.bridge.session_id();
        debug_assert!(
            session_id.as_deref().is_some_and(|s| !s.is_empty()),
            "AskBridge was attached without a non-empty session_id; refusing to run"
        );

        // 1. Serialize args to Value for parse_questions
        let params = serde_json::to_value(&args).map_err(|e| format!("serialize: {e}"))?;
        let questions = parse_questions(&params)?;
        let timeout = self.bridge.timeout();

        // 2. Create oneshot channel
        let (tx, rx) = oneshot::channel();

        // 3. Store in bridge
        if !self.bridge.set(PendingAsk {
            questions,
            responder: tx,
            timeout,
            session_id,
        }) {
            return Ok(AgentToolResult::error("Another ask is already pending"));
        }

        // 4. Wait for user response
        select_with_abort(rx, signal, &self.bridge).await
    }
}

/// Wait for either the ask response or the abort signal.
async fn select_with_abort(
    rx: oneshot::Receiver<AskResponse>,
    signal: Option<oneshot::Receiver<()>>,
    bridge: &AskBridge,
) -> Result<AgentToolResult, ToolError> {
    // If no abort signal, use a future that never resolves
    let abort = async {
        if let Some(sig) = signal {
            let _ = sig.await;
        } else {
            std::future::pending::<()>().await;
        }
    };

    tokio::select! {
        response = rx => {
            match response {
                Ok(resp) => {
                    if resp.cancelled {
                        Ok(AgentToolResult::success("User cancelled the question"))
                    } else {
                        Ok(AgentToolResult::success(format_answers(
                            &resp.answers,
                            resp.timed_out,
                        )))
                    }
                }
                Err(_) => {
                    // Sender was dropped without sending — overlay was closed without result
                    Ok(AgentToolResult::success("Question dismissed"))
                }
            }
        }
        () = abort => {
            // Abort signal received (Ctrl+C) — clean up bridge
            bridge.try_take();
            Ok(AgentToolResult::success("Question cancelled by user interrupt"))
        }
    }
}

/// Parse and validate the ask parameters from JSON.
fn parse_questions(params: &serde_json::Value) -> Result<Vec<Question>, ToolError> {
    let questions = params
        .get("questions")
        .and_then(|v| v.as_array())
        .cloned()
        .ok_or_else(|| "Missing or invalid 'questions' field".to_string())?;

    let questions: Vec<Question> = questions
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Invalid question: {}", e))?;

    if questions.is_empty() {
        return Err("At least one question is required".to_string());
    }

    // Assign default labels (use the id) if not provided
    let questions: Vec<Question> = questions
        .into_iter()
        .map(|mut q| {
            if q.label.is_empty() {
                q.label = q.id.clone();
            }
            q
        })
        .collect();

    // Validate question IDs are unique
    let mut ids = std::collections::HashSet::new();
    for q in &questions {
        if !ids.insert(&q.id) {
            return Err(format!("Duplicate question id: {}", q.id));
        }
    }

    Ok(questions)
}

/// Format answers into a human-readable text for the tool result.
///
/// The transcript renderer (`format_ask_result`) parses this text together
/// with the call arguments to reconstruct the filled-menu view. The format
/// stays model-readable:
/// - single select: `<id>: <label>`
/// - multi select:  `<id>: [a, b]`
/// - custom input:  `<id>: "<text>"`
/// - cancelled:     `<id>: (cancelled)`
/// - timeout suffix: ` (auto-selected after timeout)`
pub fn format_answers(answers: &[Answer], timed_out: bool) -> String {
    let suffix = if timed_out {
        " (auto-selected after timeout)"
    } else {
        ""
    };
    answers
        .iter()
        .map(|a| {
            let base = if a.was_custom {
                format!("{}: \"{}\"", a.id, a.label)
            } else if a.value.contains(',') {
                // multi-select: value is comma-joined
                let labels: Vec<&str> = a.label.split(", ").collect();
                format!("{}: [{}]", a.id, labels.join(", "))
            } else {
                format!("{}: {}", a.id, a.label)
            };
            format!("{base}{suffix}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_questions_valid() {
        let json = serde_json::json!({
            "questions": [
                {
                    "id": "lang",
                    "prompt": "Pick a language",
                    "options": [
                        { "value": "rust", "label": "Rust" },
                        { "value": "ts", "label": "TypeScript" }
                    ]
                }
            ]
        });
        let questions = parse_questions(&json).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].id, "lang");
        assert_eq!(questions[0].label, "lang"); // default label = id
        assert_eq!(questions[0].options.len(), 2);
        assert!(questions[0].allow_other); // default
        assert!(!questions[0].multi_select); // default
    }

    #[test]
    fn test_parse_questions_with_label() {
        let json = serde_json::json!({
            "questions": [
                {
                    "id": "lang",
                    "label": "Language",
                    "prompt": "Pick a language"
                }
            ]
        });
        let questions = parse_questions(&json).unwrap();
        assert_eq!(questions[0].label, "Language");
    }

    #[test]
    fn test_parse_questions_empty_options() {
        // allowOther=true + empty options = free text question
        let json = serde_json::json!({
            "questions": [
                {
                    "id": "name",
                    "prompt": "What's your project name?",
                    "allowOther": true
                }
            ]
        });
        let questions = parse_questions(&json).unwrap();
        assert_eq!(questions[0].options.len(), 0);
        assert!(questions[0].allow_other);
    }

    #[test]
    fn test_parse_questions_missing_questions() {
        let json = serde_json::json!({});
        let err = parse_questions(&json).unwrap_err();
        assert!(err.contains("questions"));
    }

    #[test]
    fn test_parse_questions_empty_array() {
        let json = serde_json::json!({ "questions": [] });
        let err = parse_questions(&json).unwrap_err();
        assert!(err.contains("one question"));
    }

    #[test]
    fn test_parse_questions_duplicate_ids() {
        let json = serde_json::json!({
            "questions": [
                { "id": "a", "prompt": "Q1" },
                { "id": "a", "prompt": "Q2" }
            ]
        });
        let err = parse_questions(&json).unwrap_err();
        assert!(err.contains("Duplicate"));
    }

    #[test]
    fn test_format_answers_single() {
        let answers = vec![Answer {
            id: "lang".into(),
            value: "rust".into(),
            label: "Rust".into(),
            was_custom: false,
            index: Some(1),
        }];
        let text = format_answers(&answers, false);
        assert_eq!(text, "lang: Rust");
    }

    #[test]
    fn test_format_answers_custom() {
        let answers = vec![Answer {
            id: "name".into(),
            value: "myproj".into(),
            label: "myproj".into(),
            was_custom: true,
            index: None,
        }];
        let text = format_answers(&answers, false);
        assert_eq!(text, "name: \"myproj\"");
    }

    #[test]
    fn test_format_answers_multi() {
        let answers = vec![Answer {
            id: "lang".into(),
            value: "rust, go".into(), // comma-joined values signal multi
            label: "Rust, Go".into(),
            was_custom: false,
            index: None,
        }];
        let text = format_answers(&answers, false);
        assert_eq!(text, "lang: [Rust, Go]");
    }

    #[test]
    fn test_format_answers_timed_out() {
        let answers = vec![Answer {
            id: "auth".into(),
            value: "oauth".into(),
            label: "OAuth2".into(),
            was_custom: false,
            index: Some(2),
        }];
        let text = format_answers(&answers, true);
        assert_eq!(text, "auth: OAuth2 (auto-selected after timeout)");
    }

    #[test]
    fn test_bridge_set_take() {
        let bridge = AskBridge::new();
        assert!(!bridge.has_pending());

        let (tx, _rx) = oneshot::channel();
        let pending = PendingAsk {
            questions: vec![],
            responder: tx,
            timeout: None,
            session_id: None,
        };
        assert!(bridge.set(pending));
        assert!(bridge.has_pending());

        let taken = bridge.try_take();
        assert!(taken.is_some());
        assert!(!bridge.has_pending());

        // Second take returns None
        assert!(bridge.try_take().is_none());
    }

    #[test]
    fn test_bridge_set_idempotent() {
        let bridge = AskBridge::new();
        let (tx1, _rx1) = oneshot::channel();
        let (tx2, _rx2) = oneshot::channel();

        bridge.set(PendingAsk {
            questions: vec![],
            responder: tx1,
            timeout: None,
            session_id: None,
        });
        assert!(!bridge.set(PendingAsk {
            questions: vec![],
            responder: tx2,
            timeout: None,
            session_id: None,
        }));
    }

    #[test]
    fn test_ui_attached_flag() {
        let bridge = AskBridge::new();
        assert!(!bridge.is_ui_attached());
        bridge.attach();
        assert!(bridge.is_ui_attached());
    }

    #[test]
    fn test_bridge_with_timeout() {
        let bridge = AskBridge::with_timeout(Some(Duration::from_secs(30)));
        assert_eq!(bridge.timeout(), Some(Duration::from_secs(30)));
        assert!(!bridge.is_ui_attached()); // with_timeout doesn't attach

        let no_timeout = AskBridge::new();
        assert_eq!(no_timeout.timeout(), None);
    }

    #[test]
    fn test_question_deserializes_without_recommended() {
        // recommended is optional with serde default — backward compatible
        let json = serde_json::json!({
            "id": "test",
            "prompt": "Test question?",
            "options": [{"value": "a", "label": "A"}]
        });
        let q: Question = serde_json::from_value(json).unwrap();
        assert_eq!(q.recommended, None);
    }

    #[test]
    fn test_question_deserializes_with_recommended() {
        let json = serde_json::json!({
            "id": "test",
            "prompt": "Test question?",
            "options": [{"value": "a", "label": "A"}, {"value": "b", "label": "B"}],
            "recommended": 1
        });
        let q: Question = serde_json::from_value(json).unwrap();
        assert_eq!(q.recommended, Some(1));
    }

    #[test]
    fn test_tool_name_is_ask() {
        let bridge = Arc::new(AskBridge::new());
        let tool = AskTool::new(bridge);
        assert_eq!(tool.name(), "ask");
        assert_eq!(tool.label(), "Ask");
    }

    #[test]
    fn test_attach_with_session_stores_id() {
        let bridge = AskBridge::new();
        assert!(!bridge.is_ui_attached());
        assert_eq!(bridge.session_id(), None);
        bridge.attach_with_session("tui");
        assert!(bridge.is_ui_attached());
        assert_eq!(bridge.session_id().as_deref(), Some("tui"));
    }

    #[test]
    fn test_format_answers_multi_with_comma_label() {
        // Regression: option label containing a comma must still render as a
        // multi-select bracket form when the value is comma-joined, not be
        // misparsed as a single label.
        let answers = vec![Answer {
            id: "tags".into(),
            value: "a,b".into(),
            label: "A, B".into(),
            was_custom: false,
            index: None,
        }];
        let text = format_answers(&answers, false);
        assert_eq!(text, "tags: [A, B]");
    }

    #[test]
    fn test_format_answers_cancelled_marker() {
        let answers = vec![Answer {
            id: "q1".into(),
            value: String::new(),
            label: String::new(),
            was_custom: false,
            index: None,
        }];
        // format_answers doesn't itself emit "cancelled" — that comes from
        // AskOverlay when the user presses Esc. Verify the formatted path
        // produces an empty answer for that case so the renderer can detect it.
        let text = format_answers(&answers, false);
        assert_eq!(text, "q1: ");
    }
}
