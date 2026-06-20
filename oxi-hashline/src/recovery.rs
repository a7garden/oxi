//! Snapshot-tag drift recovery.
//!
//! When a section's `[path#TAG]` header names a tag that no longer matches the
//! live file's content hash, the patcher delegates here. M1 implements only
//! **session-chain replay** (Phase 1): if the tag names an older in-session
//! snapshot — meaning a prior edit in the *same* session advanced the hash —
//! and the intermediate edit left the anchor lines (and line count) untouched,
//! the new edits are replayed directly onto the live content.
//!
//! Phase 2 (3-way merge via `similar`, for *external* modifications where the
//! tag IS the head but the live file differs) is deferred to M1.5 behind the
//! `three-way-merge` feature. Without it, an external modification cleanly
//! falls back to a `MismatchError` — the same behaviour as the legacy
//! str_replace path.
//!
//! Ported from omp `packages/hashline/src/recovery.ts`.

use crate::apply::apply_edits;
use crate::messages::{RECOVERY_SESSION_CHAIN_WARNING, RECOVERY_SESSION_REPLAY_WARNING};
use crate::snapshots::{Snapshot, SnapshotStore};
use crate::types::Edit;

// ── Input / output ───────────────────────────────────────────────────────

/// Arguments for [`Recovery::try_recover`].
pub struct RecoveryArgs<'a> {
    /// Canonical file path.
    pub path: &'a str,
    /// The stale tag from the section header.
    pub file_hash: &'a str,
    /// Current (live) file text, already normalized to LF / BOM-stripped.
    pub current_text: &'a str,
    /// The edits the section wants to apply.
    pub edits: &'a [Edit],
}

/// A successfully recovered edit application.
#[derive(Debug, Clone)]
pub struct RecoveryResult {
    /// Post-edit text (normalized).
    pub text: String,
    /// 1-indexed first changed line, if any.
    pub first_changed_line: Option<u32>,
    /// Warnings (always includes the session-chain notice).
    pub warnings: Vec<String>,
}

/// Why recovery could not proceed.
#[derive(Debug)]
pub enum RecoveryFailure {
    /// No snapshot was ever recorded for `path` + `file_hash`.
    NoSnapshot,
    /// The tag names the latest (head) snapshot, but the live file differs — an
    /// external modification. M1 rejects; Phase 2 (3-way merge) would handle
    /// this.
    ExternalModification {
        /// The head snapshot the tag resolved to.
        snapshot: Snapshot,
    },
    /// The session-chain guards failed (line count or anchor content changed
    /// between the tagged snapshot and live).
    ChainMismatch,
}

// ── Engine ───────────────────────────────────────────────────────────────

/// Borrowed recovery engine. Cheap to construct; borrows the snapshot store
/// for the duration of a single `try_recover` call.
pub struct Recovery<'a> {
    store: &'a dyn SnapshotStore,
}

impl<'a> Recovery<'a> {
    /// Create a recovery engine backed by `store`.
    pub fn new(store: &'a dyn SnapshotStore) -> Self {
        Self { store }
    }

    /// Attempt to recover a stale-tag edit.
    ///
    /// Decision tree (M1):
    /// 1. Look up the snapshot for `(path, file_hash)`. Missing → `NoSnapshot`.
    /// 2. If that snapshot IS the current head, the live file was modified
    ///    externally → `ExternalModification` (M1.5 would 3-way-merge).
    /// 3. Otherwise the tag is an older in-session version. Try session-chain
    ///    replay: if line count matches and every anchor line is byte-identical
    ///    between snapshot and live, apply the edits directly to live.
    pub fn try_recover(&self, args: RecoveryArgs<'a>) -> Result<RecoveryResult, RecoveryFailure> {
        let snapshot = self
            .store
            .by_hash(args.path, args.file_hash)
            .ok_or(RecoveryFailure::NoSnapshot)?;

        let is_head = self.store.head(args.path).as_ref() == Some(&snapshot);
        if is_head {
            return Err(RecoveryFailure::ExternalModification { snapshot });
        }

        replay_session_chain(&snapshot, args.current_text, args.edits)
            .ok_or(RecoveryFailure::ChainMismatch)
    }
}

// ── Session-chain replay ─────────────────────────────────────────────────

/// Replay `edits` onto `live` using `snapshot` as the drift guard.
///
/// Returns `None` when the guards fail (line count mismatch or an anchor line
/// differs between snapshot and live), signalling the caller to fall back to a
/// `MismatchError`.
fn replay_session_chain(snapshot: &Snapshot, live: &str, edits: &[Edit]) -> Option<RecoveryResult> {
    let snap_lines: Vec<&str> = snapshot.text.split('\n').collect();
    let live_lines: Vec<&str> = live.split('\n').collect();

    // Guard 1: line count must match. An intermediate in-session edit that
    // inserted or deleted lines would have shifted every anchor below it,
    // making replay unsafe.
    if snap_lines.len() != live_lines.len() {
        return None;
    }

    // Guard 2: every edit's anchor line must be byte-identical between the
    // tagged snapshot and the live file. If the intermediate edit touched an
    // anchor line, we cannot safely replay.
    for edit in edits {
        let anchor_line = edit.anchor_line();
        // Bof (0) and Eof (u32::MAX) are position-based, not content-based.
        if anchor_line == 0 || anchor_line == u32::MAX {
            continue;
        }
        let idx = (anchor_line as usize).saturating_sub(1);
        if idx >= snap_lines.len() {
            return None;
        }
        if snap_lines[idx] != live_lines[idx] {
            return None;
        }
    }

    // Guards passed — apply edits directly onto the live content.
    let result = apply_edits(live, edits).ok()?;
    if result.text == live {
        return None; // no-op
    }

    let mut warnings = result.warnings;
    warnings.insert(0, RECOVERY_SESSION_CHAIN_WARNING.to_string());
    // The replay fast-path verify hedge is secondary guidance.
    warnings.push(RECOVERY_SESSION_REPLAY_WARNING.to_string());

    Some(RecoveryResult {
        text: result.text,
        first_changed_line: result.first_changed_line,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshots::InMemorySnapshotStore;
    use crate::types::{Anchor, Cursor, Edit};
    use std::sync::Arc;

    fn insert_edit(line: u32, text: &str) -> Edit {
        Edit::Insert {
            cursor: Cursor::AfterAnchor(Anchor { line }),
            text: text.to_string(),
            line_num: 1,
            index: 0,
            mode: None,
        }
    }

    fn make_store() -> Arc<InMemorySnapshotStore> {
        Arc::new(InMemorySnapshotStore::new())
    }

    #[test]
    fn no_snapshot_for_tag_returns_no_snapshot() {
        let store = make_store();
        let recovery = Recovery::new(store.as_ref());
        let edits = vec![insert_edit(1, "x")];
        let result = recovery.try_recover(RecoveryArgs {
            path: "f.rs",
            file_hash: "AAAA",
            current_text: "a\nb",
            edits: &edits,
        });
        assert!(matches!(result, Err(RecoveryFailure::NoSnapshot)));
    }

    #[test]
    fn head_tag_with_drift_is_external_modification() {
        let store = make_store();
        // Record live content → becomes head.
        let tag = store.record("f.rs", "a\nb\n", Some(&[1, 2]));
        let recovery = Recovery::new(store.as_ref());
        let edits = vec![insert_edit(1, "x")];
        // Live text differs from what was recorded → external mod.
        let result = recovery.try_recover(RecoveryArgs {
            path: "f.rs",
            file_hash: &tag,
            current_text: "a\nCHANGED\n",
            edits: &edits,
        });
        assert!(matches!(
            result,
            Err(RecoveryFailure::ExternalModification { .. })
        ));
    }

    #[test]
    fn session_chain_replay_succeeds_when_anchors_untouched() {
        let store = make_store();
        // Step 1: read records snapshot with tag H1.
        let h1 = store.record("f.rs", "line1\nline2\nline3\n", Some(&[1, 2, 3]));
        // Step 2: an in-session edit advances the hash. Record the new state.
        store.record("f.rs", "line1\nline2\nCHANGED3\n", None);
        // Now head != H1, so recovery enters the session-chain path.
        let recovery = Recovery::new(store.as_ref());
        // Edit anchored at line 1 (unchanged between H1 and live).
        let edits = vec![insert_edit(1, "inserted")];
        let result = recovery.try_recover(RecoveryArgs {
            path: "f.rs",
            file_hash: &h1,
            current_text: "line1\nline2\nCHANGED3\n",
            edits: &edits,
        });
        assert!(result.is_ok());
        let recovered = result.unwrap();
        // The inserted text should appear in the result.
        assert!(recovered.text.contains("inserted"));
        assert!(recovered.warnings.iter().any(|w| w.contains("session")));
    }

    #[test]
    fn session_chain_replay_fails_when_anchor_line_changed() {
        let store = make_store();
        let h1 = store.record("f.rs", "line1\nline2\n", Some(&[1, 2]));
        // Intermediate edit changed line 1.
        store.record("f.rs", "CHANGED1\nline2\n", None);
        let recovery = Recovery::new(store.as_ref());
        // Edit anchored at line 1 — but line 1 changed between H1 and live.
        let edits = vec![insert_edit(1, "x")];
        let result = recovery.try_recover(RecoveryArgs {
            path: "f.rs",
            file_hash: &h1,
            current_text: "CHANGED1\nline2\n",
            edits: &edits,
        });
        assert!(matches!(result, Err(RecoveryFailure::ChainMismatch)));
    }

    #[test]
    fn session_chain_replay_fails_on_line_count_change() {
        let store = make_store();
        let h1 = store.record("f.rs", "a\nb\n", Some(&[1, 2]));
        // Intermediate edit added a line → line count differs.
        store.record("f.rs", "a\nb\nc\n", None);
        let recovery = Recovery::new(store.as_ref());
        let edits = vec![insert_edit(1, "x")];
        let result = recovery.try_recover(RecoveryArgs {
            path: "f.rs",
            file_hash: &h1,
            current_text: "a\nb\nc\n",
            edits: &edits,
        });
        assert!(matches!(result, Err(RecoveryFailure::ChainMismatch)));
    }

    #[test]
    fn bof_eof_anchors_skip_content_check() {
        let store = make_store();
        let h1 = store.record("f.rs", "a\nb\n", Some(&[1, 2]));
        store.record("f.rs", "X\nb\n", None);
        let recovery = Recovery::new(store.as_ref());
        // HEAD insert — anchor is Bof, content check is skipped.
        let edits = vec![Edit::Insert {
            cursor: Cursor::Bof,
            text: "prefix".to_string(),
            line_num: 1,
            index: 0,
            mode: None,
        }];
        let result = recovery.try_recover(RecoveryArgs {
            path: "f.rs",
            file_hash: &h1,
            current_text: "X\nb\n",
            edits: &edits,
        });
        assert!(result.is_ok());
    }
}
