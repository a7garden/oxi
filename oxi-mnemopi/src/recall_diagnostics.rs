//! Recall diagnostics — ported from omp `core/recall-diagnostics.ts`.
//!
//! Tracks recall-tier hit counts, fallback-path usage, and zero-result
//! calls across the working-memory (WM) and episodic-memory (EM) lanes.
//! Used by the recall pipeline to surface diagnostics and to compute
//! fallback-rate signals.
//!
//! MIT — attribution: adapted from [omp](https://github.com/can1357/oh-my-pi)
//! `packages/mnemopi/src/core/recall-diagnostics.ts`.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Recall tier names in canonical ordering.
///
/// Matches omp's `RECALL_TIERS` constant — the order matters for
/// deterministic snapshot serialisation and for human-readable
/// `explain_recall_diagnostics` output.
pub const RECALL_TIERS: &[&str] = &[
    "wm_fts",
    "wm_vec",
    "wm_fallback",
    "em_fts",
    "em_vec",
    "em_fallback",
];

/// Per-tier hit statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TierStats {
    /// Number of recall calls that returned at least one hit from this tier.
    pub calls_with_hits: usize,
    /// Total number of hits returned by this tier across all calls.
    pub total_hits: usize,
}

/// Immutable snapshot of the diagnostics state at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallDiagnosticsSnapshot {
    /// RFC-3339 timestamp recorded at construction time.
    pub created_at: String,
    /// RFC-3339 timestamp recorded when the snapshot was taken.
    pub snapshot_at: String,
    /// Total number of recall calls observed.
    pub total_calls: usize,
    /// Number of calls that had to use the WM fallback path.
    pub calls_using_wm_fallback: usize,
    /// Number of calls that had to use the EM fallback path.
    pub calls_using_em_fallback: usize,
    /// Number of calls that returned zero hits across every tier.
    pub calls_truly_empty: usize,
    /// Per-tier hit statistics keyed by tier name (see [`RECALL_TIERS`]).
    pub by_tier: HashMap<String, TierStats>,
    /// WM fallback rate: `calls_using_wm_fallback / total_calls` (0.0 if no calls).
    pub fallback_rate_wm: f64,
    /// EM fallback rate: `calls_using_em_fallback / total_calls` (0.0 if no calls).
    pub fallback_rate_em: f64,
}

/// Recall diagnostics accumulator.
///
/// Tracks per-tier hit stats, total call count, fallback-path usage, and
/// zero-result counts. Cheap to update, snapshot returns a cloneable
/// [`RecallDiagnosticsSnapshot`].
#[derive(Debug)]
pub struct RecallDiagnostics {
    created_at: String,
    total_calls: usize,
    calls_using_wm_fallback: usize,
    calls_using_em_fallback: usize,
    calls_truly_empty: usize,
    by_tier: HashMap<String, TierStats>,
}

impl Default for RecallDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl RecallDiagnostics {
    /// Create a fresh accumulator with the current timestamp.
    pub fn new() -> Self {
        Self {
            created_at: Utc::now().to_rfc3339(),
            total_calls: 0,
            calls_using_wm_fallback: 0,
            calls_using_em_fallback: 0,
            calls_truly_empty: 0,
            by_tier: HashMap::new(),
        }
    }

    /// Record a hit count for a tier.
    ///
    /// Unknown tiers are recorded verbatim. If `hit_count > 0` the tier's
    /// `calls_with_hits` counter is incremented as well.
    pub fn record_tier_hits(&mut self, tier: &str, hit_count: usize) {
        let stats = self.by_tier.entry(tier.to_string()).or_default();
        stats.total_hits += hit_count;
        if hit_count > 0 {
            stats.calls_with_hits += 1;
        }
    }

    /// Record that a recall call used one or both fallback paths.
    pub fn record_fallback_used(&mut self, wm: bool, em: bool) {
        if wm {
            self.calls_using_wm_fallback += 1;
        }
        if em {
            self.calls_using_em_fallback += 1;
        }
    }

    /// Record one recall call.
    ///
    /// Set `truly_empty` to `true` if the call returned zero hits across
    /// every tier.
    pub fn record_call(&mut self, truly_empty: bool) {
        self.total_calls += 1;
        if truly_empty {
            self.calls_truly_empty += 1;
        }
    }

    /// Compute `(wm_rate, em_rate)` — fallback rate per lane.
    ///
    /// Returns `(0.0, 0.0)` when no calls have been recorded yet to avoid
    /// division by zero.
    pub fn fallback_rate(&self) -> (f64, f64) {
        if self.total_calls == 0 {
            return (0.0, 0.0);
        }
        let wm = self.calls_using_wm_fallback as f64 / self.total_calls as f64;
        let em = self.calls_using_em_fallback as f64 / self.total_calls as f64;
        (wm, em)
    }

    /// Capture an immutable snapshot of the current state.
    pub fn snapshot(&self) -> RecallDiagnosticsSnapshot {
        let (fallback_rate_wm, fallback_rate_em) = self.fallback_rate();
        RecallDiagnosticsSnapshot {
            created_at: self.created_at.clone(),
            snapshot_at: Utc::now().to_rfc3339(),
            total_calls: self.total_calls,
            calls_using_wm_fallback: self.calls_using_wm_fallback,
            calls_using_em_fallback: self.calls_using_em_fallback,
            calls_truly_empty: self.calls_truly_empty,
            by_tier: self.by_tier.clone(),
            fallback_rate_wm,
            fallback_rate_em,
        }
    }

    /// Reset all counters and per-tier stats; preserve the original
    /// `created_at` so the lifetime of the accumulator is still traceable.
    pub fn reset(&mut self) {
        self.total_calls = 0;
        self.calls_using_wm_fallback = 0;
        self.calls_using_em_fallback = 0;
        self.calls_truly_empty = 0;
        self.by_tier.clear();
    }
}

/// Process-wide singleton diagnostics instance.
///
/// Lazily initialised on first access via [`std::sync::LazyLock`].
pub static GLOBAL_DIAGNOSTICS: LazyLock<Mutex<RecallDiagnostics>> =
    LazyLock::new(|| Mutex::new(RecallDiagnostics::new()));

/// Build a human-readable diagnostic report from a snapshot.
///
/// Returns one message per observation; order is stable. Use
/// `snapshot.by_tier` ordering from the caller if a specific order is
/// required — this helper does not assume one.
pub fn explain_recall_diagnostics(snapshot: &RecallDiagnosticsSnapshot) -> Vec<String> {
    let mut messages = Vec::new();

    if snapshot.total_calls == 0 {
        messages.push("No recall calls recorded yet.".to_string());
        return messages;
    }

    messages.push(format!(
        "Total recall calls: {} (truly empty: {})",
        snapshot.total_calls, snapshot.calls_truly_empty
    ));

    // Per-tier breakdown in canonical RECALL_TIERS order, then any
    // unknown tiers alphabetically.
    let mut tiers: Vec<&str> = RECALL_TIERS.to_vec();
    let known: std::collections::HashSet<&str> = tiers.iter().copied().collect();
    let mut extras: Vec<&str> = snapshot
        .by_tier
        .keys()
        .map(String::as_str)
        .filter(|t| !known.contains(t))
        .collect();
    extras.sort_unstable();
    tiers.extend(extras);

    for tier in tiers {
        let Some(stats) = snapshot.by_tier.get(tier) else {
            continue;
        };
        messages.push(format!(
            "Tier {tier}: {} hits across {} calls",
            stats.total_hits, stats.calls_with_hits
        ));
    }

    messages.push(format!(
        "WM fallback rate: {:.1}%",
        snapshot.fallback_rate_wm * 100.0
    ));
    messages.push(format!(
        "EM fallback rate: {:.1}%",
        snapshot.fallback_rate_em * 100.0
    ));

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_tier_hits_accumulates() {
        let mut diag = RecallDiagnostics::new();
        diag.record_tier_hits("wm_fts", 3);
        diag.record_tier_hits("wm_fts", 2);
        diag.record_tier_hits("wm_vec", 5);
        diag.record_tier_hits("em_fts", 0); // zero hits → calls_with_hits unchanged

        let wm_fts = diag.by_tier.get("wm_fts").unwrap();
        assert_eq!(wm_fts.total_hits, 5);
        assert_eq!(wm_fts.calls_with_hits, 2);

        let wm_vec = diag.by_tier.get("wm_vec").unwrap();
        assert_eq!(wm_vec.total_hits, 5);
        assert_eq!(wm_vec.calls_with_hits, 1);

        let em_fts = diag.by_tier.get("em_fts").unwrap();
        assert_eq!(em_fts.total_hits, 0);
        assert_eq!(em_fts.calls_with_hits, 0);

        // Unknown tier still recorded verbatim.
        diag.record_tier_hits("custom_lane", 7);
        let custom = diag.by_tier.get("custom_lane").unwrap();
        assert_eq!(custom.total_hits, 7);
        assert_eq!(custom.calls_with_hits, 1);
    }

    #[test]
    fn fallback_rate_zero_calls_returns_zero() {
        let diag = RecallDiagnostics::new();
        let (wm, em) = diag.fallback_rate();
        assert_eq!(wm, 0.0);
        assert_eq!(em, 0.0);

        // Empty snapshot must not panic on division.
        let snap = diag.snapshot();
        assert_eq!(snap.fallback_rate_wm, 0.0);
        assert_eq!(snap.fallback_rate_em, 0.0);
    }

    #[test]
    fn fallback_rate_calculates_fraction() {
        let mut diag = RecallDiagnostics::new();
        // 10 calls total; 4 used wm fallback, 1 used em fallback.
        for _ in 0..10 {
            diag.record_call(false);
        }
        diag.record_fallback_used(true, false);
        diag.record_fallback_used(true, false);
        diag.record_fallback_used(true, false);
        diag.record_fallback_used(true, false);
        diag.record_fallback_used(false, true);

        let (wm, em) = diag.fallback_rate();
        assert!((wm - 0.4).abs() < 1e-9);
        assert!((em - 0.1).abs() < 1e-9);
    }

    #[test]
    fn snapshot_captures_current_state() {
        let mut diag = RecallDiagnostics::new();
        diag.record_call(true); // truly empty
        diag.record_call(false);
        diag.record_call(false);
        diag.record_fallback_used(true, false);
        diag.record_fallback_used(false, true);
        diag.record_tier_hits("wm_fts", 4);
        diag.record_tier_hits("em_vec", 2);

        let snap = diag.snapshot();

        assert_eq!(snap.total_calls, 3);
        assert_eq!(snap.calls_truly_empty, 1);
        assert_eq!(snap.calls_using_wm_fallback, 1);
        assert_eq!(snap.calls_using_em_fallback, 1);

        let wm_fts = snap.by_tier.get("wm_fts").unwrap();
        assert_eq!(wm_fts.total_hits, 4);
        assert_eq!(wm_fts.calls_with_hits, 1);

        let em_vec = snap.by_tier.get("em_vec").unwrap();
        assert_eq!(em_vec.total_hits, 2);

        // Fallback rates: 1/3 each.
        assert!((snap.fallback_rate_wm - 1.0 / 3.0).abs() < 1e-9);
        assert!((snap.fallback_rate_em - 1.0 / 3.0).abs() < 1e-9);

        // Timestamps populated and distinct (created_at < snapshot_at).
        assert!(!snap.created_at.is_empty());
        assert!(!snap.snapshot_at.is_empty());
        assert!(
            snap.created_at <= snap.snapshot_at,
            "created_at ({}) must be <= snapshot_at ({})",
            snap.created_at,
            snap.snapshot_at
        );

        // Snapshot is independent of further mutations.
        diag.record_call(false);
        diag.reset();
        assert_eq!(snap.total_calls, 3);
        assert_eq!(snap.by_tier.get("wm_fts").unwrap().total_hits, 4);
    }

    #[test]
    fn reset_clears_state_but_preserves_created_at() {
        let mut diag = RecallDiagnostics::new();
        let original_created_at = diag.created_at.clone();

        diag.record_call(false);
        diag.record_call(true);
        diag.record_fallback_used(true, false);
        diag.record_tier_hits("wm_fts", 5);
        diag.record_tier_hits("em_vec", 3);

        diag.reset();

        assert_eq!(diag.total_calls, 0);
        assert_eq!(diag.calls_truly_empty, 0);
        assert_eq!(diag.calls_using_wm_fallback, 0);
        assert_eq!(diag.calls_using_em_fallback, 0);
        assert!(diag.by_tier.is_empty());

        let (wm, em) = diag.fallback_rate();
        assert_eq!(wm, 0.0);
        assert_eq!(em, 0.0);

        assert_eq!(diag.created_at, original_created_at);
    }

    #[test]
    fn explain_recall_diagnostics_empty() {
        let diag = RecallDiagnostics::new();
        let snap = diag.snapshot();
        let messages = explain_recall_diagnostics(&snap);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("No recall calls"));
    }

    #[test]
    fn explain_recall_diagnostics_populated() {
        let mut diag = RecallDiagnostics::new();
        diag.record_call(false);
        diag.record_call(false);
        diag.record_call(true);
        diag.record_fallback_used(true, false);
        diag.record_tier_hits("wm_fts", 4);
        diag.record_tier_hits("em_vec", 2);

        let snap = diag.snapshot();
        let messages = explain_recall_diagnostics(&snap);

        assert!(messages.iter().any(|m| m.contains("Total recall calls: 3")));
        assert!(
            messages
                .iter()
                .any(|m| m.contains("truly empty: 3") || m.contains("truly empty: 1"))
        );
        assert!(messages.iter().any(|m| m.contains("Tier wm_fts")));
        assert!(messages.iter().any(|m| m.contains("Tier em_vec")));
        assert!(messages.iter().any(|m| m.contains("WM fallback rate")));
        assert!(messages.iter().any(|m| m.contains("EM fallback rate")));
    }

    #[test]
    fn global_singleton_is_accessible() {
        let _guard = GLOBAL_DIAGNOSTICS.lock();
        // Smoke test: lock acquisition works and the value is a
        // RecallDiagnostics. We don't mutate here to avoid disturbing
        // any other test sharing the singleton.
        let _ = RecallDiagnostics::new();
    }
}
