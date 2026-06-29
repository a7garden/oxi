//! Per-session policy gate for advisor `advise()` calls — ported from omp
//! `emission-guard.ts`.
//!
//! The advisor system prompt tells the watcher model "at most one `advise`
//! per update" and "NEVER repeat advice you already gave." Real advisor models
//! violate this. omp issue #3520 captured a session that recorded 309 `advise`
//! calls covering 92 unique notes — 114× "Stop.", 52× "No issue; continue." —
//! flooding the primary transcript with `<advisory severity="blocker">Stop.
//! </advisory>` after the task was already complete.
//!
//! The fix is to make the rules load-bearing in code, not prose: silently drop
//! duplicates, content-free self-talk, and over-budget calls at the `advise`
//! boundary so the primary stays clean even when the advisor misbehaves. The
//! gate is invisible to the advisor model — `AdviseTool` still returns
//! `Recorded.` for a suppressed call.
//!
//! # Attribution
//!
//! Translated to Rust from omp (oh-my-pi), MIT licensed.

use std::collections::{HashSet, VecDeque};

use parking_lot::Mutex;

/// Case-insensitive, punctuation-folded normalization. Lowercases, applies
/// NFKC, collapses every run of non-letter / non-digit characters into a single
/// space, and trims — so `"Stop."`, `"*Stop*"`, and `"  stop  "` all key to
/// `stop`, while `"No issue; continue."` keys to `no issue continue`.
/// omp `normalizeAdvisorNote`.
#[must_use]
pub fn normalize_advisor_note(note: &str) -> String {
    use unicode_normalization::UnicodeNormalization;

    let mut out = String::with_capacity(note.len());
    let mut prev_space = true; // suppress a leading space
    for c in note.to_lowercase().nfkc() {
        if c.is_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

/// Normalized phrases the advisor occasionally emits that carry no concrete
/// actionable content. Each must be the output of [`normalize_advisor_note`]
/// so a single membership check covers every punctuation/casing variant
/// (`"Stop."`, `"stop"`, `"STOP!"`). Ported verbatim from omp
/// `SUPPRESSED_NORMALIZED_PHRASES` — do not edit casually; it is the curated
/// set observed driving primary-transcript pollution (#3520). A genuine
/// `blocker` like `"Stop: 'await' missing on writeStream.end()..."` does not
/// match.
const SUPPRESSED_NORMALIZED_PHRASES: &[&str] = &[
    // Self-stop noise — telling the agent to "stop" without a reason is useless.
    "stop",
    "stop here",
    "stop now",
    "halt",
    "abort",
    // Completion self-talk — the agent already finished the task.
    "done",
    "task done",
    "task complete",
    "complete",
    "finished",
    "ok",
    "okay",
    "ok done",
    // "Nothing to flag" — silence is the correct expression of "no concerns".
    "no issue",
    "no issues",
    "no issue continue",
    "no concerns",
    "no concern",
    "nothing to add",
    "nothing to flag",
    "nothing to report",
    "no notes",
    "no further input",
    "no further input needed",
    "no further input required",
    "no further watcher input",
    "no further watcher input needed",
    "no further advice",
    "no further advice needed",
    // Endorsements — equivalent to silence.
    "lgtm",
    "looks good",
    "all good",
    "agent is on track",
    "agent on track",
    "on track",
    "continue",
    "carry on",
];

/// Bounds the dedupe history. omp `DEFAULT_HISTORY_CAPACITY`.
const DEFAULT_HISTORY_CAPACITY: usize = 4096;

#[derive(Default)]
struct State {
    /// Normalized notes already delivered, for dedupe.
    seen: HashSet<String>,
    /// Insertion-order log to drive FIFO eviction without an extra Map.
    seen_order: VecDeque<String>,
    /// Per-update budget: at most one accepted `advise` per update.
    consumed_this_update: bool,
}

/// Decides whether an advisor `advise()` call should reach the primary agent.
/// omp `AdvisorEmissionGuard`.
///
/// Thread-safe (`&self` + internal lock) because `accept` is driven from the
/// advisor agent's tool execution while `begin_update`/`reset` are driven from
/// the host's turn boundaries — they can overlap.
pub struct AdvisorEmissionGuard {
    state: Mutex<State>,
    capacity: usize,
}

impl AdvisorEmissionGuard {
    /// Construct with the default dedupe history capacity (4096).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_HISTORY_CAPACITY)
    }

    /// Construct with an explicit dedupe history capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            capacity,
        }
    }

    /// Drop all dedupe and per-update state. Called whenever the advisor
    /// runtime is reset (compaction, session switch, `/new`) so a re-primed
    /// advisor can re-raise old issues — the primary transcript was rewritten.
    /// omp `reset()`.
    pub fn reset(&self) {
        let mut s = self.state.lock();
        s.seen.clear();
        s.seen_order.clear();
        s.consumed_this_update = false;
    }

    /// Clear the per-update rate-limit gate. Called right before each advisor
    /// `prompt(batch)` cycle so the next advisor model cycle starts with a
    /// fresh budget of one advise. omp `beginUpdate()`.
    pub fn begin_update(&self) {
        self.state.lock().consumed_this_update = false;
    }

    /// Whether the proposed note should reach the primary. On `true` the gate
    /// has already recorded the note (consumed the per-update budget and added
    /// it to the dedupe history) — the caller delivers the note. On `false` the
    /// caller drops it. omp `accept()`.
    ///
    /// Empty/whitespace-only notes are suppressed (defense-in-depth; the
    /// tool-args contract requires a non-empty string). Content-free filler
    /// (per `SUPPRESSED_NORMALIZED_PHRASES`) and exact normalized duplicates
    /// are suppressed. Over-budget calls (a second accept in the same update)
    /// are suppressed.
    pub fn accept(&self, note: &str) -> bool {
        let key = normalize_advisor_note(note);
        if key.is_empty() {
            return false;
        }
        if SUPPRESSED_NORMALIZED_PHRASES.contains(&key.as_str()) {
            return false;
        }
        let mut s = self.state.lock();
        if s.seen.contains(&key) {
            return false;
        }
        if s.consumed_this_update {
            return false;
        }
        s.consumed_this_update = true;
        s.seen.insert(key.clone());
        s.seen_order.push_back(key);
        while s.seen_order.len() > self.capacity {
            if let Some(stale) = s.seen_order.pop_front() {
                s.seen.remove(&stale);
            }
        }
        true
    }
}

impl Default for AdvisorEmissionGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn guard() -> AdvisorEmissionGuard {
        AdvisorEmissionGuard::with_capacity(4)
    }

    #[test]
    fn normalize_folds_punctuation_and_case() {
        assert_eq!(normalize_advisor_note("Stop."), "stop");
        assert_eq!(normalize_advisor_note("*Stop*"), "stop");
        assert_eq!(normalize_advisor_note("  stop  "), "stop");
        assert_eq!(
            normalize_advisor_note("No issue; continue."),
            "no issue continue"
        );
        assert_eq!(normalize_advisor_note(""), "");
        assert_eq!(normalize_advisor_note("   "), "");
    }

    #[test]
    fn accept_suppresses_content_free_phrases() {
        let g = guard();
        for phrase in [
            "Stop.",
            "STOP!",
            "done",
            "No issues.",
            "looks good",
            "continue",
        ] {
            assert!(!g.accept(phrase), "should suppress {phrase:?}");
        }
    }

    #[test]
    fn accept_suppresses_empty() {
        let g = guard();
        assert!(!g.accept(""));
        assert!(!g.accept("   "));
    }

    #[test]
    fn accept_one_per_update_then_blocks_second() {
        let g = guard();
        assert!(g.accept("Use saturating_add to avoid overflow on the counter"));
        // same update -> blocked even if novel
        assert!(!g.accept("Different, valid note about naming"));
        // new update -> fresh budget, but the first note is now deduped
        g.begin_update();
        assert!(!g.accept("Use saturating_add to avoid overflow on the counter"));
        assert!(g.accept("Different, valid note about naming"));
    }

    #[test]
    fn accept_dedupes_normalized_variants_across_updates() {
        let g = guard();
        assert!(g.accept("Use saturating_add."));
        g.begin_update();
        // punctuation/case variant normalizes to the same key -> deduped
        assert!(!g.accept("use saturating add!"));
    }

    #[test]
    fn fifo_evicts_at_capacity_then_readmits() {
        let g = guard();
        // capacity 4
        for i in 0..4 {
            assert!(g.accept(&format!("note number {i}")));
            g.begin_update();
        }
        // 5th evicts the oldest ("note number 0")
        assert!(g.accept("note number 4"));
        g.begin_update();
        // evicted note is readmitted
        assert!(g.accept("note number 0"));
    }

    #[test]
    fn reset_clears_everything() {
        let g = guard();
        g.accept("some real advice");
        g.reset();
        // same note readmitted after reset
        assert!(g.accept("some real advice"));
    }

    #[test]
    fn genuine_blocker_is_not_suppressed() {
        let g = guard();
        assert!(g.accept("Stop: 'await' missing on writeStream.end() will lose buffered writes."));
    }
}
