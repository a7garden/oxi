//! The `advise` agent tool — ported from omp `AdviseTool` (advise-tool.ts).
//!
//! The advisor LLM calls this to surface one piece of advice to the primary
//! agent. The tool applies a severity-rank dedupe (a call passes only when its
//! rank strictly exceeds the previously-delivered rank for that note — a real
//! escalation `nit → concern → blocker`), then hands accepted notes to a
//! host-provided [`EnqueueAdviceFn`] callback. The host owns the
//! [`crate::advisor::emission_guard::AdvisorEmissionGuard`] (the final
//! per-update/dedupe/content-free gate) and routes the note to the primary.
//!
//! # Attribution
//!
//! Translated to Rust from omp (oh-my-pi), MIT licensed.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::advisor::types::{AdvisorNote, AdvisorSeverity, note_dedupe_key};
use crate::{AgentTool, AgentToolResult, ToolContext};

/// The tool description, embedded from the omp prompt so the engine is
/// self-contained (SDK consumers get a ready tool without supplying the text).
const ADVISE_DESCRIPTION: &str = include_str!("prompts/advise-tool.md");

/// Host callback invoked for each *dedupe-passing* note. The host's
/// [`crate::advisor::emission_guard::AdvisorEmissionGuard`] (not this tool) is
/// the final accept/reject authority; this callback only fires when this tool's
/// own severity-rank gate admits the note.
pub type EnqueueAdviceFn = Arc<dyn Fn(AdvisorNote) + Send + Sync>;

/// The `advise` agent tool.
pub struct AdviseTool {
    enqueue: EnqueueAdviceFn,
    /// Highest delivered severity rank per dedupe key. A new call passes
    /// through only when its rank strictly exceeds the recorded one, so an
    /// advisor cannot bypass dedupe by retagging the same text at a lower or
    /// equal severity. omp `#deliveredNoteSeverities`.
    delivered: Mutex<HashMap<String, u8>>,
}

impl AdviseTool {
    /// Construct with the host-provided advice-enqueue callback.
    #[must_use]
    pub fn new(enqueue: EnqueueAdviceFn) -> Self {
        Self {
            enqueue,
            delivered: Mutex::new(HashMap::new()),
        }
    }

    /// Clear delivered-note memory when the advisor starts a fresh conversation
    /// (omp `resetDeliveredNotes`). Called from the host's advisor-reset path.
    pub fn reset_delivered(&self) {
        self.delivered.lock().clear();
    }
}

#[async_trait]
impl AgentTool for AdviseTool {
    fn name(&self) -> &str {
        "advise"
    }

    fn label(&self) -> &str {
        "Advise"
    }

    fn description(&self) -> &str {
        ADVISE_DESCRIPTION
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note": {
                    "type": "string",
                    "description": "One concrete piece of advice for the agent you are watching. Terse, specific, actionable."
                },
                "severity": {
                    "type": "string",
                    "enum": ["nit", "concern", "blocker"],
                    "description": "How strongly to weigh this. Omit for a plain nit."
                }
            },
            "required": ["note"]
        })
    }

    fn essential(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, String> {
        let note_text = params
            .get("note")
            .and_then(Value::as_str)
            .ok_or_else(|| "advise: missing string field 'note'".to_string())?
            .trim()
            .to_string();
        if note_text.is_empty() {
            return Err("advise: 'note' must be non-empty".into());
        }
        let severity = params
            .get("severity")
            .and_then(Value::as_str)
            .and_then(AdvisorSeverity::from_id);

        let key = note_dedupe_key(&note_text);
        let rank = severity.unwrap_or_default().rank();
        let prev = *self.delivered.lock().get(&key).unwrap_or(&0);
        if rank <= prev {
            // Duplicate — the same note at a lower or equal severity. Still
            // return success so the advisor model isn't prompted to retry;
            // omp returns `Duplicate advice ignored.`
            return Ok(
                AgentToolResult::success("Duplicate advice ignored.").with_metadata(json!({
                    "note": note_text,
                    "severity": severity.map(AdvisorSeverity::as_str),
                    "duplicate": true,
                })),
            );
        }
        self.delivered.lock().insert(key, rank);
        (self.enqueue)(AdvisorNote {
            note: note_text.clone(),
            severity,
        });
        Ok(AgentToolResult::success("Recorded.").with_metadata(json!({
            "note": note_text,
            "severity": severity.map(AdvisorSeverity::as_str),
            "duplicate": false,
        })))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::ToolContext;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    fn ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("."))
    }

    /// Capture enqueued notes in a vec behind a std Mutex (test-only; the
    /// closure itself must be Send+Sync so parking_lot is not required here).
    fn capture() -> (EnqueueAdviceFn, Arc<StdMutex<Vec<AdvisorNote>>>) {
        let sink = Arc::new(StdMutex::new(Vec::<AdvisorNote>::new()));
        let sink_clone = Arc::clone(&sink);
        let f: EnqueueAdviceFn = Arc::new(move |n| sink_clone.lock().unwrap().push(n));
        (f, sink)
    }

    #[tokio::test]
    async fn records_first_note_and_enqueues() {
        let (f, sink) = capture();
        let tool = AdviseTool::new(f);
        let res = tool
            .execute(
                "t1",
                json!({"note": "Use saturating_add", "severity": "concern"}),
                None,
                &ctx(),
            )
            .await
            .unwrap();
        assert!(res.success);
        assert_eq!(res.output, "Recorded.");
        assert_eq!(sink.lock().unwrap().len(), 1);
        assert_eq!(
            sink.lock().unwrap()[0].severity,
            Some(AdvisorSeverity::Concern)
        );
    }

    #[tokio::test]
    async fn dedupes_lower_or_equal_severity() {
        let (f, sink) = capture();
        let tool = AdviseTool::new(f);
        // nit first
        tool.execute("t1", json!({"note": "rename x"}), None, &ctx())
            .await
            .unwrap();
        // same note, nit again -> ignored
        let res = tool
            .execute("t2", json!({"note": "rename x"}), None, &ctx())
            .await
            .unwrap();
        assert_eq!(res.output, "Duplicate advice ignored.");
        // same note, concern -> escalation passes
        tool.execute(
            "t3",
            json!({"note": "rename x", "severity": "concern"}),
            None,
            &ctx(),
        )
        .await
        .unwrap();
        // 2 enqueued (first nit + escalation concern); the duplicate was not.
        assert_eq!(sink.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn whitespace_variants_dedupe() {
        let (f, sink) = capture();
        let tool = AdviseTool::new(f);
        tool.execute("t1", json!({"note": "rename   x"}), None, &ctx())
            .await
            .unwrap();
        let res = tool
            .execute("t2", json!({"note": "  rename x  "}), None, &ctx())
            .await
            .unwrap();
        assert_eq!(res.output, "Duplicate advice ignored.");
        assert_eq!(sink.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn empty_note_errors() {
        let (f, _sink) = capture();
        let tool = AdviseTool::new(f);
        let res = tool
            .execute("t1", json!({"note": "   "}), None, &ctx())
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn reset_delivered_readmits() {
        let (f, sink) = capture();
        let tool = AdviseTool::new(f);
        tool.execute("t1", json!({"note": "tip"}), None, &ctx())
            .await
            .unwrap();
        tool.reset_delivered();
        tool.execute("t2", json!({"note": "tip"}), None, &ctx())
            .await
            .unwrap();
        assert_eq!(sink.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn omitted_severity_is_nit() {
        let (f, sink) = capture();
        let tool = AdviseTool::new(f);
        tool.execute("t1", json!({"note": "tip"}), None, &ctx())
            .await
            .unwrap();
        assert_eq!(sink.lock().unwrap()[0].severity, None);
        assert_eq!(sink.lock().unwrap()[0].rank(), 1);
    }
}
