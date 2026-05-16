//! Questionnaire tool — ask the user one or more questions via TUI overlay.
//!
//! Architecture:
//! - `QuestionnaireBridge` is created in `oxi-cli` and shared (via `Arc`) between
//!   `QuestionnaireTool` (agent thread) and `AppState` (TUI main thread).
//! - When the tool executes, it creates a oneshot channel and stores (questions, sender)
//!   in the bridge.
//! - The TUI main loop polls the bridge, and if a pending questionnaire is found,
//!   creates a `QuestionnaireOverlay` to display it.
//! - User interaction drives the overlay to send a `QuestionnaireResponse` via the
//!   oneshot `Sender`. The tool's `execute()` receives it via `rx.await`.
//! - Abort (Ctrl+C) is handled via `tokio::select!` with the abort signal.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::oneshot;

use super::{AgentTool, AgentToolResult, ToolContext, ToolError};

/// Shared bridge between the questionnaire tool (agent thread) and the TUI
/// overlay (main thread). Created in `oxi-cli`, injected into both the tool
/// and `AppState`.
#[derive(Clone)]
pub struct QuestionnaireBridge {
    inner: Arc<parking_lot::Mutex<Option<PendingQuestionnaire>>>,
}

impl QuestionnaireBridge {
    /// Create a new empty bridge.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Store a pending questionnaire. Called by `QuestionnaireTool::execute`.
    /// Returns `false` if another questionnaire is already pending (should not
    /// happen in sequential tool execution, but guards against races).
    pub fn set(&self, pending: PendingQuestionnaire) -> bool {
        let mut lock = self.inner.lock();
        if lock.is_some() {
            return false;
        }
        *lock = Some(pending);
        true
    }

    /// Try to take the pending questionnaire. Called by the TUI main loop polling.
    /// Returns `None` if nothing is pending or already taken.
    pub fn try_take(&self) -> Option<PendingQuestionnaire> {
        self.inner.lock().take()
    }

    /// Returns `true` if a questionnaire is currently pending.
    ///
    pub fn has_pending(&self) -> bool {
        self.inner.lock().is_some()
    }
}

impl Default for QuestionnaireBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// A pending questionnaire waiting for user interaction.
/// The `responder` is a oneshot `Sender` — the overlay calls `send()` when
/// the user submits or cancels, and the tool's `rx.await` receives it.
pub struct PendingQuestionnaire {
    /// Questions to display to the user.
    pub questions: Vec<Question>,
    /// Sender end of the response channel. Dropping this (without sending) is
    /// equivalent to user dismiss.
    pub responder: oneshot::Sender<QuestionnaireResponse>,
}

/// A single question to ask the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// Unique identifier for this question.
    pub id: String,
    /// Short contextual label for the tab bar. Defaults to "Q1", "Q2", etc.
    #[serde(default)]
    pub label: String,
    /// The full question text to display.
    pub prompt: String,
    /// Available options. Can be empty when `allow_other` is `true`.
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    /// Whether to show "Type something..." option. Defaults to `true`.
    #[serde(default = "default_true")]
    pub allow_other: bool,
    /// Whether multiple options can be selected. Defaults to `false`.
    #[serde(default)]
    pub multi_select: bool,
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
pub struct QuestionnaireResponse {
    /// All answers collected.
    pub answers: Vec<Answer>,
    /// `true` if the user cancelled (Esc).
    pub cancelled: bool,
}

/// A single answer to a question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    /// Question ID this answer belongs to.
    pub id: String,
    /// The value selected or entered.
    pub value: String,
    /// Display label.
    pub label: String,
    /// `true` if the user typed custom text (allowOther).
    pub was_custom: bool,
    /// 1-based index of the selected option. `None` for custom input.
    pub index: Option<usize>,
}

// ── Tool ───────────────────────────────────────────────────────────────────

/// The questionnaire tool — asks the user one or more questions via TUI overlay.
pub struct QuestionnaireTool {
    bridge: Arc<QuestionnaireBridge>,
}

impl QuestionnaireTool {
    /// Create a new `QuestionnaireTool` that communicates via the given bridge.
    pub fn new(bridge: Arc<QuestionnaireBridge>) -> Self {
        Self { bridge }
    }
}

// `Clone` is needed because ToolRegistry stores `Arc<dyn AgentTool>`.
// `QuestionnaireTool` is cheap to clone (only copies the Arc).
impl Clone for QuestionnaireTool {
    fn clone(&self) -> Self {
        Self {
            bridge: self.bridge.clone(),
        }
    }
}

#[async_trait]
impl AgentTool for QuestionnaireTool {
    fn name(&self) -> &str {
        "questionnaire"
    }

    fn label(&self) -> &str {
        "Questionnaire"
    }

    fn description(&self) -> &str {
        "Ask the user one or more questions. Use for clarifying requirements, \
         getting preferences, or confirming decisions. For single questions, \
         shows a simple option list. For multiple questions, shows a tab-based \
         interface."
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
                                "description": "Short contextual label for tab bar (defaults to Q1, Q2)"
                            },
                            "prompt": {
                                "type": "string",
                                "description": "The full question text to display"
                            },
                            "options": {
                                "type": "array",
                                "description": "Available options to choose from. Can be empty for free-text questions.",
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
                                            "description": "Display label for the option"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "Optional description shown below label"
                                        }
                                    },
                                    "required": ["value", "label"]
                                }
                            },
                            "allowOther": {
                                "type": "boolean",
                                "description": "Allow 'Type something' option (default: true)",
                                "default": true
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "description": "Allow multiple selections (default: false)",
                                "default": false
                            }
                        },
                        "required": ["id", "prompt"]
                    }
                }
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
        // 1. Parse and validate
        let questions = parse_questions(&params)?;

        // 2. Create oneshot channel
        let (tx, rx) = oneshot::channel();

        // 3. Store in bridge — TUI polls it on the main thread
        if !self.bridge.set(PendingQuestionnaire {
            questions,
            responder: tx,
        }) {
            return Ok(AgentToolResult::error(
                "Another questionnaire is already pending",
            ));
        }

        // 4. Wait for user response — handle abort via tokio::select!
        let result = select_with_abort(rx, signal, &self.bridge).await;

        // 5. Format result
        result
    }
}

/// Wait for either the questionnaire response or the abort signal.
async fn select_with_abort(
    rx: oneshot::Receiver<QuestionnaireResponse>,
    signal: Option<oneshot::Receiver<()>>,
    bridge: &QuestionnaireBridge,
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
                        Ok(AgentToolResult::success("User cancelled the questionnaire"))
                    } else {
                        Ok(AgentToolResult::success(format_answers(&resp.answers)))
                    }
                }
                Err(_) => {
                    // Sender was dropped without sending — overlay was closed without result
                    Ok(AgentToolResult::success("Questionnaire dismissed"))
                }
            }
        }
        () = abort => {
            // Abort signal received (Ctrl+C) — clean up bridge
            bridge.try_take();
            Ok(AgentToolResult::success("Questionnaire cancelled by user interrupt"))
        }
    }
}

/// Parse and validate the questionnaire parameters from JSON.
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

    // Assign default labels if not provided
    let questions: Vec<Question> = questions
        .into_iter()
        .enumerate()
        .map(|(i, mut q)| {
            if q.label.is_empty() {
                q.label = format!("Q{}", i + 1);
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
fn format_answers(answers: &[Answer]) -> String {
    answers
        .iter()
        .map(|a| {
            if a.was_custom {
                format!("{}: user wrote: {}", a.id, a.label)
            } else if let Some(idx) = a.index {
                format!("{}: user selected: {}. {}", a.id, idx, a.label)
            } else {
                format!("{}: user selected: {}", a.id, a.label)
            }
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
        assert_eq!(questions[0].label, "Q1"); // default label (not id)
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
    fn test_format_answers_selected() {
        let answers = vec![Answer {
            id: "lang".into(),
            value: "rust".into(),
            label: "Rust".into(),
            was_custom: false,
            index: Some(1),
        }];
        let text = format_answers(&answers);
        assert_eq!(text, "lang: user selected: 1. Rust");
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
        let text = format_answers(&answers);
        assert_eq!(text, "name: user wrote: myproj");
    }

    #[test]
    fn test_format_answers_multi() {
        let answers = vec![
            Answer {
                id: "lang".into(),
                value: "rust".into(),
                label: "Rust".into(),
                was_custom: false,
                index: Some(1),
            },
            Answer {
                id: "db".into(),
                value: "pg".into(),
                label: "PostgreSQL".into(),
                was_custom: false,
                index: Some(2),
            },
            Answer {
                id: "auth".into(),
                value: "jwt".into(),
                label: "jwt".into(),
                was_custom: true,
                index: None,
            },
        ];
        let text = format_answers(&answers);
        assert_eq!(
            text,
            "lang: user selected: 1. Rust\ndb: user selected: 2. PostgreSQL\nauth: user wrote: jwt"
        );
    }

    #[test]
    fn test_bridge_set_take() {
        let bridge = QuestionnaireBridge::new();
        assert!(!bridge.has_pending());

        let (tx, _rx) = oneshot::channel();
        let pending = PendingQuestionnaire {
            questions: vec![],
            responder: tx,
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
        let bridge = QuestionnaireBridge::new();
        let (tx1, _rx1) = oneshot::channel();
        let (tx2, _rx2) = oneshot::channel();

        bridge.set(PendingQuestionnaire {
            questions: vec![],
            responder: tx1,
        });
        assert!(!bridge.set(PendingQuestionnaire {
            questions: vec![],
            responder: tx2
        }));
    }
}
