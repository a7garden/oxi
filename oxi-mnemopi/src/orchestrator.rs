//! Orchestrator — recall dispatch between linear and polyphonic recall.
//!
//! Ported from omp `core/orchestrator.ts`. Decides whether to use the
//! standard linear recall (FTS5 + vector hybrid) or the polyphonic
//! multi-voice recall, based on options and feature flags.
//!
//! MIT — attribution: adapted from [omp](https://github.com/earendil-works/pi)
//! `packages/mnemopi/src/core/orchestrator.ts`.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::polyphonic_recall::{self, PolyphonicResult};
use crate::recall::{self};
use crate::types::{RecallOptions, RecallResult};

/// Options for orchestrated recall.
#[derive(Debug, Clone, Default)]
pub struct OrchestrateRecallOptions {
    /// Use enhanced recall if available (same as standard in this port).
    pub enhanced: bool,
    /// Force polyphonic recall regardless of feature flag.
    pub force_polyphonic: bool,
    /// Force linear (non-polyphonic) recall.
    pub force_linear: bool,
    /// Session scope for recall.
    pub session_id: String,
    /// Result limit.
    pub limit: Option<usize>,
    /// Pre-computed query embedding (skip auto-derivation).
    pub query_embedding: Option<Vec<f32>>,
}

/// Unified recall result that may contain linear or polyphonic fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratedRecallResult {
    pub memory_id: String,
    pub content: String,
    pub score: f64,
    pub source: Option<String>,
    pub importance: f64,
    pub tier: String,
    // Polyphonic-only fields (None for linear recall)
    pub combined_score: Option<f64>,
    pub voice_scores: Option<std::collections::HashMap<String, f64>>,
}

/// Check whether polyphonic recall is enabled.
///
/// In omp this reads a runtime config flag. In this port we default to
/// `false` (linear recall is the safe default); callers can override
/// via `force_polyphonic`.
pub fn polyphonic_is_enabled() -> bool {
    // Could read from env var or config; default off for safety.
    std::env::var("MNEMOPI_POLYPHONIC")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Convert a linear [`RecallResult`] into an [`OrchestratedRecallResult`].
fn from_linear(r: RecallResult) -> OrchestratedRecallResult {
    OrchestratedRecallResult {
        memory_id: r.id,
        content: r.content,
        score: r.score as f64,
        source: r.source,
        importance: r.importance,
        tier: r
            .tier
            .map(|t| format!("{t:?}").to_lowercase())
            .unwrap_or_else(|| "unknown".into()),
        combined_score: None,
        voice_scores: None,
    }
}

/// Convert a polyphonic [`PolyphonicResult`] into an [`OrchestratedRecallResult`].
fn from_polyphonic(r: PolyphonicResult) -> OrchestratedRecallResult {
    OrchestratedRecallResult {
        memory_id: r.memory_id,
        content: r.content,
        score: r.combined_score,
        source: r.source,
        importance: r.importance,
        tier: r.tier,
        combined_score: Some(r.combined_score),
        voice_scores: Some(r.voice_scores),
    }
}

/// Orchestrated recall: dispatch between linear and polyphonic.
///
/// When `force_linear` is set, or polyphonic is neither forced nor enabled,
/// runs standard linear recall via [`recall::recall`]. Otherwise runs
/// [`polyphonic_recall::polyphonic_recall`].
pub fn orchestrate_recall(
    conn: &Connection,
    query: &str,
    top_k: usize,
    options: &OrchestrateRecallOptions,
) -> Result<Vec<OrchestratedRecallResult>> {
    let use_polyphonic =
        !options.force_linear && (options.force_polyphonic || polyphonic_is_enabled());

    if use_polyphonic {
        let output = polyphonic_recall::polyphonic_recall(conn, query, &options.session_id, top_k)?;
        return Ok(output.results.into_iter().map(from_polyphonic).collect());
    }

    // Linear recall
    let recall_opts = RecallOptions {
        limit: Some(top_k),
        query_embedding: options.query_embedding.clone(),
        ..Default::default()
    };
    let results = recall::recall(conn, query, &options.session_id, &recall_opts)?;
    Ok(results.into_iter().map(from_linear).collect())
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RememberOptions;

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_orchestrate_linear_by_default() {
        let conn = setup_conn();
        crate::store::remember(
            &conn,
            "User prefers dark theme",
            "test",
            &RememberOptions {
                source: Some("test".into()),
                importance: Some(0.8),
                ..Default::default()
            },
        )
        .unwrap();

        let results = orchestrate_recall(
            &conn,
            "dark theme",
            10,
            &OrchestrateRecallOptions {
                session_id: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(!results.is_empty());
        // Linear results don't have combined_score
        assert!(results[0].combined_score.is_none());
    }

    #[test]
    fn test_orchestrate_force_polyphonic() {
        let conn = setup_conn();
        crate::store::remember(
            &conn,
            "User prefers dark theme",
            "test",
            &RememberOptions {
                source: Some("test".into()),
                importance: Some(0.8),
                ..Default::default()
            },
        )
        .unwrap();

        let results = orchestrate_recall(
            &conn,
            "dark theme",
            10,
            &OrchestrateRecallOptions {
                session_id: "test".into(),
                force_polyphonic: true,
                ..Default::default()
            },
        )
        .unwrap();

        // Polyphonic results have combined_score
        // (might be empty if no episodic memories, but at least shouldn't error)
        for r in &results {
            assert!(r.combined_score.is_some());
        }
    }

    #[test]
    fn test_orchestrate_force_linear_overrides_env() {
        // SAFETY: single-threaded test
        unsafe {
            std::env::set_var("MNEMOPI_POLYPHONIC", "1");
        }
        let conn = setup_conn();
        crate::store::remember(&conn, "Test memory", "test", &RememberOptions::default()).unwrap();

        let results = orchestrate_recall(
            &conn,
            "Test",
            10,
            &OrchestrateRecallOptions {
                session_id: "test".into(),
                force_linear: true,
                ..Default::default()
            },
        )
        .unwrap();

        // Should be linear even though polyphonic is enabled via env
        for r in &results {
            assert!(r.combined_score.is_none());
        }
        unsafe {
            std::env::remove_var("MNEMOPI_POLYPHONIC");
        }
    }
}
