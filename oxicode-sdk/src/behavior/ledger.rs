//! Compatibility ledger — machine-readable OMP-equivalence claims shipped
//! with a behavior pack release (design: "OMP compatibility contract").

use serde::{Deserialize, Serialize};

/// Honest OMP-equivalence status of one feature area.
///
/// "Equivalent" means the listed scenarios establish the externally relevant
/// semantics — not byte-for-byte source or UI identity. "Partial" is
/// mandatory when an exposed tool exists but lacks required persistence,
/// protocol coverage, or failure semantics. Hosts can render statuses but
/// never upgrade them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureStatus {
    /// Listed scenarios establish the externally relevant semantics.
    Equivalent,
    /// Exposed but lacking required persistence/coverage/failure semantics.
    Partial,
    /// The OMP behavior has no satisfying implementation in this pack.
    Unavailable,
    /// Outside the pack's contract (host-composition concern).
    NotApplicable,
}

impl FeatureStatus {
    /// Ordering used by rollup: Unavailable is the worst real claim;
    /// NotApplicable never drags a rollup down.
    pub fn rank(&self) -> u8 {
        match self {
            FeatureStatus::NotApplicable => 0,
            FeatureStatus::Equivalent => 1,
            FeatureStatus::Partial => 2,
            FeatureStatus::Unavailable => 3,
        }
    }

    /// Worst-of two statuses.
    pub fn worst(a: FeatureStatus, b: FeatureStatus) -> FeatureStatus {
        if a.rank() >= b.rank() { a } else { b }
    }
}

/// One ledger row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Stable feature slug (e.g. `persistent-shell`).
    pub feature: String,
    /// Claimed equivalence status.
    pub status: FeatureStatus,
    /// Fixture/scenario ids establishing the claim (empty for Unavailable).
    pub evidence: Vec<String>,
    /// Bounded policy notes and intentional deviations.
    pub notes: String,
}

/// Machine-readable compatibility contract shipped with a pack release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityContract {
    /// Compatibility target, pinned by release or commit (e.g.
    /// `omp@v18.0.11 (can1357/oh-my-pi@b8ce33a)`) — never a moving "latest".
    pub target: String,
    /// Ledger rows.
    pub entries: Vec<LedgerEntry>,
}

impl CompatibilityContract {
    /// Worst status across entries; `Unavailable` when empty or when every
    /// entry is NotApplicable (conservative).
    pub fn rollup(&self) -> FeatureStatus {
        let mut worst = FeatureStatus::NotApplicable;
        for entry in &self.entries {
            worst = FeatureStatus::worst(worst, entry.status);
        }
        if worst == FeatureStatus::NotApplicable {
            FeatureStatus::Unavailable
        } else {
            worst
        }
    }

    /// Merge for multi-pack resolution: targets joined, entries concatenated.
    pub fn merge(&self, other: &CompatibilityContract) -> CompatibilityContract {
        let target = if self.target.is_empty() {
            other.target.clone()
        } else {
            format!("{} + {}", self.target, other.target)
        };
        let mut entries = self.entries.clone();
        entries.extend(other.entries.iter().cloned());
        CompatibilityContract { target, entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(feature: &str, status: FeatureStatus) -> LedgerEntry {
        LedgerEntry {
            feature: feature.to_string(),
            status,
            evidence: Vec::new(),
            notes: String::new(),
        }
    }

    #[test]
    fn rollup_takes_worst_real_status() {
        let c = CompatibilityContract {
            target: "omp@v18.0.11".to_string(),
            entries: vec![
                entry("a", FeatureStatus::Equivalent),
                entry("b", FeatureStatus::Partial),
                entry("c", FeatureStatus::Unavailable),
                entry("d", FeatureStatus::NotApplicable),
            ],
        };
        assert_eq!(c.rollup(), FeatureStatus::Unavailable);
    }

    #[test]
    fn rollup_of_only_not_applicable_is_conservative() {
        let c = CompatibilityContract {
            target: String::new(),
            entries: vec![entry("a", FeatureStatus::NotApplicable)],
        };
        assert_eq!(c.rollup(), FeatureStatus::Unavailable);
    }

    #[test]
    fn empty_contract_rolls_up_unavailable() {
        let c = CompatibilityContract {
            target: String::new(),
            entries: Vec::new(),
        };
        assert_eq!(c.rollup(), FeatureStatus::Unavailable);
    }

    #[test]
    fn merge_concatenates_entries_and_joins_targets() {
        let a = CompatibilityContract {
            target: "omp@v18.0.11".to_string(),
            entries: vec![entry("a", FeatureStatus::Equivalent)],
        };
        let b = CompatibilityContract {
            target: "git-review-v1".to_string(),
            entries: vec![entry("b", FeatureStatus::Partial)],
        };
        let merged = a.merge(&b);
        assert_eq!(merged.target, "omp@v18.0.11 + git-review-v1");
        assert_eq!(merged.entries.len(), 2);
        assert_eq!(merged.rollup(), FeatureStatus::Partial);
    }

    #[test]
    fn ledger_serializes_snake_case() {
        let json = serde_json::to_string(&FeatureStatus::NotApplicable).unwrap();
        assert_eq!(json, "\"not_applicable\"");
    }
}
