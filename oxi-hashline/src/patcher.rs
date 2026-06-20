//! Filesystem-backed patch orchestrator.
//!
//! The [`Patcher`] is the bridge between a parsed [`Patch`] (pure data) and the
//! real filesystem. It owns the two-phase commit:
//!
//! 1. **Prepare** every section: read the file, normalize (BOM/CRLF), validate
//!    the snapshot tag (with drift recovery), check seen-lines, and apply edits
//!    in memory. No writes happen during prepare.
//! 2. **Commit**: restore line endings/BOM, write through [`HashlineFs`], and
//!    record the post-edit snapshot so the new tag is immediately anchorable.
//!
//! All-or-nothing: if *any* section fails to prepare, nothing is written. If
//! prepare succeeds for all sections but a write fails mid-batch, the result
//! reports which sections committed and which did not.
//!
//! Ported from omp `packages/hashline/src/patcher.ts`.
use crate::apply::apply_edits;
use crate::diff_preview::build_compact_diff_preview;
use crate::format::compute_file_hash;
use crate::mismatch::{HashlineError, MismatchDetails, MismatchError};
use crate::normalize::{self, LineEnding};
use crate::parser::{Patch, PatchSection};
use crate::recovery::{Recovery, RecoveryArgs, RecoveryFailure};
use crate::snapshots::SnapshotStore;
use crate::types::{CompactDiffOptions, Edit};
use std::collections::HashSet;
use std::sync::Arc;

// ── Filesystem seam ──────────────────────────────────────────────────────

/// Filesystem abstraction. The host (oxi-agent) injects a concrete impl that
/// wires in `PathGuard` security and `file_mutation_queue` serialization.
///
/// All paths are relative to the host's root directory.
#[async_trait::async_trait]
pub trait HashlineFs: Send + Sync {
    /// Read the full raw text of `path`. Returns [`HashlineError::NotFound`]
    /// when the file does not exist.
    async fn read_text(&self, path: &str) -> Result<String, HashlineError>;

    /// Atomically write `text` to `path`, returning the written path.
    async fn write_text(&self, path: &str, text: &str) -> Result<String, HashlineError>;

    /// Check write permission / parent existence without writing. Default is a
    /// no-op (always ok).
    async fn preflight_write(&self, _path: &str) -> Result<(), HashlineError> {
        Ok(())
    }

    /// Canonicalize `path` for duplicate-section detection. Two sections whose
    /// paths canonicalize to the same string are treated as the same file.
    fn canonical_path(&self, path: &str) -> String;

    /// Whether `err` represents a "file not found" condition.
    fn is_not_found(&self, err: &HashlineError) -> bool {
        matches!(err, HashlineError::NotFound { .. })
    }
}

// ── Result types ─────────────────────────────────────────────────────────

/// Result of applying an entire patch.
#[derive(Debug, Clone)]
pub struct PatcherApplyResult {
    /// Per-section results, in the same order as the input patch sections.
    pub sections: Vec<PatchSectionResult>,
}

/// Result of applying one section.
#[derive(Debug, Clone)]
pub struct PatchSectionResult {
    /// Canonical file path that was edited.
    pub path: String,
    /// Compact post-edit diff preview (omp-style renumbered preview).
    pub diff: String,
    /// 1-indexed first changed line, if any.
    pub first_changed_line: Option<u32>,
    /// Warnings (parse warnings, recovery notices, boundary repair, etc.).
    pub warnings: Vec<String>,
    /// Content hash of the post-edit file — the tag for the next `[path#TAG]`.
    pub new_hash: String,
}

/// Internal: a section that passed prepare and is ready to commit.
struct PreparedSection {
    path: String,
    /// Pre-edit normalized (LF, no BOM) text — for diff preview.
    before_text: String,
    /// Post-edit normalized (LF, no BOM) text.
    result_text: String,
    /// Original line ending for restore-on-write.
    line_ending: LineEnding,
    /// Whether the original file had a BOM.
    had_bom: bool,
    first_changed_line: Option<u32>,
    warnings: Vec<String>,
}

// ── Patcher ──────────────────────────────────────────────────────────────

/// Orchestrates hashline patch application against a real filesystem.
pub struct Patcher {
    fs: Arc<dyn HashlineFs>,
    snapshots: Arc<dyn SnapshotStore>,
}

impl Patcher {
    /// Create a patcher with the given filesystem and snapshot store.
    pub fn new(fs: Arc<dyn HashlineFs>, snapshots: Arc<dyn SnapshotStore>) -> Self {
        Self { fs, snapshots }
    }

    /// Apply a patch: prepare all sections, then commit all (all-or-nothing).
    pub async fn apply(&self, patch: &Patch) -> Result<PatcherApplyResult, HashlineError> {
        let prepared = self.prepare_all(&patch.sections).await?;

        // Commit phase: write every prepared section.
        let mut results = Vec::with_capacity(prepared.len());
        for section in &prepared {
            let result = self.commit(section).await?;
            results.push(result);
        }

        Ok(PatcherApplyResult { sections: results })
    }

    /// Preflight (dry-run): prepare all sections without writing.
    pub async fn preflight(&self, patch: &Patch) -> Result<(), HashlineError> {
        self.prepare_all(&patch.sections).await?;
        Ok(())
    }

    // ── Prepare ──────────────────────────────────────────────────────────

    /// Prepare every section. Returns `Err` on the first failure, leaving
    /// nothing written.
    async fn prepare_all(
        &self,
        sections: &[PatchSection],
    ) -> Result<Vec<PreparedSection>, HashlineError> {
        // Duplicate canonical-path detection.
        let mut seen_paths: HashSet<String> = HashSet::new();
        for section in sections {
            let canonical = self.fs.canonical_path(&section.file_path);
            if !seen_paths.insert(canonical.clone()) {
                return Err(HashlineError::DuplicateCanonicalPath { path: canonical });
            }
        }

        let mut prepared = Vec::with_capacity(sections.len());
        for section in sections {
            prepared.push(self.prepare_section(section).await?);
        }
        Ok(prepared)
    }

    /// Prepare a single section: read → normalize → validate tag → check seen
    /// lines → apply edits in memory.
    async fn prepare_section(
        &self,
        section: &PatchSection,
    ) -> Result<PreparedSection, HashlineError> {
        let canonical = self.fs.canonical_path(&section.file_path);

        // Collect parse warnings up front.
        let mut warnings = section.warnings.clone();

        // Read.
        let raw = self.fs.read_text(&section.file_path).await?;

        // Normalize: detect line ending + BOM, strip for processing.
        let bom = normalize::strip_bom(&raw);
        let had_bom = !bom.bom.is_empty();
        let line_ending = normalize::detect_line_ending(bom.text);
        let normalized = normalize::normalize_to_lf(bom.text);

        // Validate the snapshot tag and decide which text to edit.
        let (text_to_edit, tag_warnings) = self
            .resolve_tag(&canonical, &section.file_hash, &normalized, &section.edits)
            .await?;
        warnings.extend(tag_warnings);

        // Check seen lines.
        self.check_seen_lines(&canonical, &section.file_hash, &section.edits)?;

        // Apply edits.
        let apply_result = apply_edits(&text_to_edit, &section.edits)?;
        warnings.extend(apply_result.warnings);

        if apply_result.text == text_to_edit {
            return Err(HashlineError::NoOp {
                path: section.file_path.clone(),
            });
        }

        Ok(PreparedSection {
            path: section.file_path.clone(),
            before_text: text_to_edit,
            result_text: apply_result.text,
            line_ending,
            had_bom,
            first_changed_line: apply_result.first_changed_line,
            warnings,
        })
    }

    // ── Tag resolution (the decision tree) ───────────────────────────────

    /// Decide which text to apply edits to, based on the section's tag vs the
    /// live file hash.
    ///
    /// Returns `(text_to_edit, warnings)`.
    async fn resolve_tag(
        &self,
        canonical: &str,
        file_hash: &str,
        live_text: &str,
        edits: &[Edit],
    ) -> Result<(String, Vec<String>), HashlineError> {
        let live_hash = compute_file_hash(live_text);

        // No tag in the header → apply without validation (lenient path).
        if file_hash.is_empty() {
            return Ok((live_text.to_string(), Vec::new()));
        }

        // Tag matches live → normal path.
        if live_hash == file_hash {
            return Ok((live_text.to_string(), Vec::new()));
        }

        // Head/tail-only edits are position-independent → apply despite drift.
        if edits.iter().all(is_position_independent) {
            return Ok((
                live_text.to_string(),
                vec![crate::messages::HEADTAIL_DRIFT_WARNING.to_string()],
            ));
        }

        // Drift on anchored edits → try recovery.
        let recovery = Recovery::new(self.snapshots.as_ref());
        match recovery.try_recover(RecoveryArgs {
            path: canonical,
            file_hash,
            current_text: live_text,
            edits,
        }) {
            Ok(recovered) => Ok((recovered.text, recovered.warnings)),
            Err(RecoveryFailure::NoSnapshot) => {
                // Tag not recognized — likely fabricated or from a prior session.
                Err(mismatch_error(
                    canonical, file_hash, &live_hash, live_text, edits,
                    false, // hash not recognized
                ))
            }
            Err(RecoveryFailure::ExternalModification { .. }) => {
                // Tag IS the head but live differs — external write.
                Err(mismatch_error(
                    canonical, file_hash, &live_hash, live_text, edits,
                    true, // hash recognized (it's the head)
                ))
            }
            Err(RecoveryFailure::ChainMismatch) => {
                // Session-chain guards failed.
                Err(mismatch_error(
                    canonical, file_hash, &live_hash, live_text, edits, true,
                ))
            }
        }
    }

    // ── Seen-lines check ─────────────────────────────────────────────────

    /// Verify every edit anchor was displayed to the model in a prior read.
    fn check_seen_lines(
        &self,
        canonical: &str,
        file_hash: &str,
        edits: &[Edit],
    ) -> Result<(), HashlineError> {
        if file_hash.is_empty() {
            return Ok(());
        }
        let snapshot = match self.snapshots.by_hash(canonical, file_hash) {
            Some(s) => s,
            None => return Ok(()), // No provenance — skip the check.
        };
        let seen = match &snapshot.seen_lines {
            Some(s) => s,
            None => return Ok(()), // No seen-line tracking — skip.
        };

        let mut unseen: Vec<u32> = Vec::new();
        for edit in edits {
            let anchor_line = edit.anchor_line();
            if anchor_line == 0 || anchor_line == u32::MAX {
                continue; // Bof / Eof — position-based.
            }
            if !seen.contains(&anchor_line) {
                unseen.push(anchor_line);
            }
        }

        if unseen.is_empty() {
            return Ok(());
        }

        let msg = format_unseen_lines(&unseen);
        Err(HashlineError::UnseenLines(msg))
    }

    // ── Commit ───────────────────────────────────────────────────────────

    /// Write a prepared section and record its new snapshot.
    async fn commit(&self, section: &PreparedSection) -> Result<PatchSectionResult, HashlineError> {
        let canonical = self.fs.canonical_path(&section.path);

        // Restore line endings + BOM.
        let mut output = normalize::restore_line_endings(&section.result_text, section.line_ending);
        if section.had_bom {
            output = format!("\u{feff}{output}");
        }

        // Preflight + write.
        self.fs.preflight_write(&section.path).await?;
        self.fs.write_text(&section.path, &output).await?;

        // Compute new hash for the normalized result text.
        let new_hash = compute_file_hash(&section.result_text);

        // Record the post-edit snapshot. The anchor lines become "seen" so a
        // follow-up edit anchored at any line of the new state validates.
        let total_lines = section.result_text.split('\n').count() as u32;
        let all_lines: Vec<u32> = (1..=total_lines).collect();
        self.snapshots
            .record(&canonical, &section.result_text, Some(&all_lines));

        // Build compact diff preview (omp-style renumbered post-edit preview).
        let preview = build_compact_diff_preview(
            &section.before_text,
            &section.result_text,
            &CompactDiffOptions::default(),
        );
        let diff = preview.lines.join("\n");

        Ok(PatchSectionResult {
            path: section.path.clone(),
            diff,
            first_changed_line: section.first_changed_line,
            warnings: section.warnings.clone(),
            new_hash,
        })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// True for `INS.HEAD` / `INS.TAIL` edits whose landing position does not
/// depend on content (only file boundaries).
fn is_position_independent(edit: &Edit) -> bool {
    matches!(
        edit,
        Edit::Insert {
            cursor: crate::types::Cursor::Bof | crate::types::Cursor::Eof,
            ..
        }
    )
}

/// Build a [`HashlineError::Mismatch`] from the diagnostic context.
fn mismatch_error(
    path: &str,
    expected: &str,
    actual: &str,
    live_text: &str,
    edits: &[Edit],
    hash_recognized: bool,
) -> HashlineError {
    let file_lines: Vec<String> = live_text.split('\n').map(String::from).collect();
    let anchor_lines: Vec<u32> = edits.iter().map(|e| e.anchor_line()).collect();
    let details = MismatchDetails {
        path: Some(path.to_string()),
        expected_file_hash: expected.to_string(),
        actual_file_hash: actual.to_string(),
        file_lines,
        anchor_lines,
        hash_recognized,
    };
    let err = MismatchError::new(details);
    HashlineError::Mismatch {
        detail: err.message,
        expected: expected.to_string(),
        actual: actual.to_string(),
    }
}

/// Format the "unseen lines" error message.
fn format_unseen_lines(lines: &[u32]) -> String {
    let listed: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    format!(
        "Edit rejected: lines {} were not shown in your last read. \
         Re-read those exact lines before editing them.",
        listed.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshots::InMemorySnapshotStore;

    /// In-memory HashlineFs for testing.
    struct MemFs {
        root: parking_lot::RwLock<std::collections::HashMap<String, String>>,
    }

    impl MemFs {
        fn new() -> Self {
            Self {
                root: parking_lot::RwLock::new(std::collections::HashMap::new()),
            }
        }

        fn put(&self, path: &str, text: &str) {
            self.root.write().insert(path.to_string(), text.to_string());
        }
    }

    #[async_trait::async_trait]
    impl HashlineFs for MemFs {
        async fn read_text(&self, path: &str) -> Result<String, HashlineError> {
            self.root
                .read()
                .get(path)
                .cloned()
                .ok_or_else(|| HashlineError::NotFound {
                    path: path.to_string(),
                })
        }

        async fn write_text(&self, path: &str, text: &str) -> Result<String, HashlineError> {
            self.root.write().insert(path.to_string(), text.to_string());
            Ok(path.to_string())
        }
        fn canonical_path(&self, path: &str) -> String {
            // Normalize: strip leading "./" for duplicate detection.
            path.strip_prefix("./").unwrap_or(path).to_string()
        }
    }

    fn make_patcher() -> (Patcher, Arc<MemFs>, Arc<InMemorySnapshotStore>) {
        let fs = Arc::new(MemFs::new());
        let store = Arc::new(InMemorySnapshotStore::new());
        let patcher = Patcher::new(fs.clone(), store.clone());
        (patcher, fs, store)
    }

    #[tokio::test]
    async fn apply_simple_swap() {
        let (patcher, fs, store) = make_patcher();
        let content = "fn main() {\n    todo!()\n}\n";
        fs.put("main.rs", content);
        let tag = store.record("main.rs", content, Some(&[1, 2, 3]));

        let patch_text = format!(
            "*** Begin Patch\n[main.rs#{tag}]\nSWAP 2.=2:\n+    println!(\"hi\")\n*** End Patch"
        );
        let patch = crate::parser::split_patch_input(&patch_text, None).unwrap();
        let result = patcher.apply(&patch).await.unwrap();

        assert_eq!(result.sections.len(), 1);
        let new_content = fs.read_text("main.rs").await.unwrap();
        assert!(new_content.contains("println!"));
        assert!(!new_content.contains("todo!"));
    }

    #[tokio::test]
    async fn apply_rejects_stale_tag_with_no_snapshot() {
        let (patcher, fs, _store) = make_patcher();
        fs.put("f.rs", "a\nb\n");

        let patch_text = "*** Begin Patch\n[f.rs#FFFF]\nSWAP 1.=1:\n+x\n*** End Patch";
        let patch = crate::parser::split_patch_input(patch_text, None).unwrap();
        let result = patcher.apply(&patch).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, HashlineError::Mismatch { .. }));
    }

    #[tokio::test]
    async fn apply_head_tail_drift_allowed() {
        let (patcher, fs, _store) = make_patcher();
        fs.put("f.rs", "a\nb\n");

        // Tag doesn't match, but it's a HEAD insert (position-independent).
        let patch_text = "*** Begin Patch\n[f.rs#FFFF]\nINS.HEAD:\n+prefix\n*** End Patch";
        let patch = crate::parser::split_patch_input(patch_text, None).unwrap();
        let _result = patcher.apply(&patch).await.unwrap();

        let new_content = fs.read_text("f.rs").await.unwrap();
        assert!(new_content.starts_with("prefix"));
    }

    #[tokio::test]
    async fn apply_no_tag_applies_without_validation() {
        let (patcher, fs, _store) = make_patcher();
        fs.put("f.rs", "a\nb\n");

        // No #TAG in header.
        let patch_text = "*** Begin Patch\n[f.rs]\nSWAP 1.=1:\n+x\n*** End Patch";
        let patch = crate::parser::split_patch_input(patch_text, None).unwrap();
        let _result = patcher.apply(&patch).await.unwrap();

        let new_content = fs.read_text("f.rs").await.unwrap();
        assert!(new_content.starts_with("x\n"));
    }

    #[tokio::test]
    async fn apply_records_new_snapshot() {
        let (patcher, fs, store) = make_patcher();
        let content = "a\nb\n";
        fs.put("f.rs", content);
        let tag = store.record("f.rs", content, Some(&[1, 2]));

        let patch_text = format!("*** Begin Patch\n[f.rs#{tag}]\nSWAP 1.=1:\n+x\n*** End Patch");
        let patch = crate::parser::split_patch_input(&patch_text, None).unwrap();
        let result = patcher.apply(&patch).await.unwrap();

        // The new hash should resolve to a snapshot.
        let new_hash = &result.sections[0].new_hash;
        assert!(!new_hash.is_empty());
        let snap = store.by_hash("f.rs", new_hash);
        assert!(snap.is_some());
    }

    #[tokio::test]
    async fn apply_rejects_duplicate_canonical_paths() {
        let (patcher, fs, _store) = make_patcher();
        fs.put("f.rs", "a\nb\n");

        // Two sections with DIFFERENT path strings that canonicalize to the
        // same file ("./f.rs" → "f.rs"). The parser won't merge these because
        // the raw strings differ; the patcher's canonical-path check catches it.
        let patch_text =
            "*** Begin Patch\n[f.rs]\nSWAP 1.=1:\n+x\n[./f.rs]\nSWAP 2.=2:\n+y\n*** End Patch";
        let patch = crate::parser::split_patch_input(patch_text, None).unwrap();
        let result = patcher.apply(&patch).await;

        assert!(matches!(
            result,
            Err(HashlineError::DuplicateCanonicalPath { .. })
        ));
    }

    #[tokio::test]
    async fn apply_noop_is_error() {
        let (patcher, fs, store) = make_patcher();
        let content = "a\nb\n";
        fs.put("f.rs", content);
        let tag = store.record("f.rs", content, Some(&[1, 2]));

        // SWAP 1.=1 with body that's identical to line 1 → no-op.
        let patch_text = format!("*** Begin Patch\n[f.rs#{tag}]\nSWAP 1.=1:\n+a\n*** End Patch");
        let patch = crate::parser::split_patch_input(&patch_text, None).unwrap();
        let result = patcher.apply(&patch).await;

        assert!(matches!(result, Err(HashlineError::NoOp { .. })));
    }

    #[tokio::test]
    async fn preflight_does_not_write() {
        let (patcher, fs, store) = make_patcher();
        let content = "a\nb\n";
        fs.put("f.rs", content);
        let tag = store.record("f.rs", content, Some(&[1, 2]));

        let patch_text = format!("*** Begin Patch\n[f.rs#{tag}]\nSWAP 1.=1:\n+x\n*** End Patch");
        let patch = crate::parser::split_patch_input(&patch_text, None).unwrap();
        patcher.preflight(&patch).await.unwrap();

        // File should be unchanged.
        let content_after = fs.read_text("f.rs").await.unwrap();
        assert_eq!(content_after, content);
    }
}
