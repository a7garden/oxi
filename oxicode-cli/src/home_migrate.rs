//! Journaled, resumable migration of the legacy oxicode home (`~/.oxicode`)
//! into the unified Oxi home layout (`<oxicode_home>`).
//!
//! Contract:
//!
//! - **Preflight** produces a plan: source, destination, file count, total
//!   bytes, and a state ([`MigrationState::NothingToDo`],
//!   [`MigrationState::Ready`], [`MigrationState::AlreadyMigrated`],
//!   [`MigrationState::Conflict`]).
//! - **Conflict** = the destination already contains a file that differs
//!   (size or SHA-256) from its source counterpart. The migration aborts,
//!   reports both paths, and touches nothing. A destination identical to the
//!   source means [`MigrationState::AlreadyMigrated`] (no-op).
//! - The **journal** (`<oxi_home>/oxicode.migration-journal.json`) is written
//!   atomically (temp + rename) BEFORE the first filesystem mutation, with
//!   `status: "in_progress"`; on success it is rewritten with
//!   `status: "complete"`.
//! - The **copy step is idempotent per file**: a destination file with the
//!   same size + SHA-256 is skipped; otherwise the file is copied to
//!   `<dest>.part-<pid>`, fsynced, and renamed into place (destination
//!   directory fsynced best-effort). Because of this, a run that dies
//!   mid-copy can simply be re-run: the journal stays `in_progress` and the
//!   rerun repairs/resumes.
//! - The **verify step** walks both trees and requires every source file to
//!   exist in the destination with a matching hash. Any mismatch is an error
//!   and the journal stays `in_progress` for the next run to repair.
//! - The **source is never deleted or modified** — migration is copy-only.
//!   (Rename optimization is deferred to a later cutover release.)

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Current journal schema version.
pub const JOURNAL_VERSION: u32 = 1;

// ── Plan ───────────────────────────────────────────────────────────────────

/// Outcome of the preflight analysis. See module docs for the semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationState {
    /// No legacy home, or it contains no files.
    NothingToDo,
    /// Files can be copied without overwriting anything that differs.
    Ready,
    /// Destination already matches the source byte-for-byte.
    AlreadyMigrated,
    /// At least one file exists in both trees with different content.
    /// Carries `(source, destination)` pairs of the differing files.
    Conflict {
        /// Differing file pairs (absolute paths).
        conflicts: Vec<(PathBuf, PathBuf)>,
    },
}

/// Result of a preflight run.
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationPlan {
    /// Legacy source home.
    pub source: PathBuf,
    /// Canonical destination home.
    pub destination: PathBuf,
    /// Number of files under the source.
    pub file_count: usize,
    /// Total size of the source files, in bytes.
    pub total_bytes: u64,
    /// Preflight state.
    pub state: MigrationState,
    /// Source-relative paths still needing a copy (empty unless `Ready`).
    pub pending: Vec<PathBuf>,
}

// ── Journal ────────────────────────────────────────────────────────────────

/// On-disk migration journal. Written atomically before the first mutation;
/// a missing or unreadable journal is treated as "no journal".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationJournal {
    /// Journal schema version.
    pub version: u32,
    /// Legacy source home.
    pub source: PathBuf,
    /// Canonical destination home.
    pub destination: PathBuf,
    /// `"in_progress"` until the copy + verify completes.
    pub status: String,
    /// Migration start time (Unix seconds).
    pub started_at: u64,
}

impl MigrationJournal {
    /// A fresh `in_progress` journal stamped with the current Unix time.
    pub fn new_in_progress(source: &Path, destination: &Path) -> Self {
        Self {
            version: JOURNAL_VERSION,
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            status: "in_progress".to_string(),
            started_at: unix_now(),
        }
    }

    /// Load the journal from disk. Missing or unreadable file is treated as
    /// "no journal" (`None`).
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Atomically write the journal (temp + rename). Parent directories are
    /// created on demand.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.part");
        let bytes =
            serde_json::to_vec_pretty(self).expect("migration journal is JSON-serializable");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Whether this journal records an in-progress migration.
    pub fn is_in_progress(&self) -> bool {
        self.status == "in_progress"
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Errors ─────────────────────────────────────────────────────────────────

/// Errors the migration engine can surface.
#[derive(Debug, thiserror::Error)]
pub enum HomeMigrationError {
    /// Filesystem error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Journal (de)serialization failure.
    #[error("journal error: {0}")]
    Journal(String),
    /// Post-copy verification failed; the journal stays `in_progress`.
    #[error("verification failed (rerun `oxicode migrate home` to repair): {0}")]
    Verify(String),
}

// ── Preflight ──────────────────────────────────────────────────────────────

/// Recursively list files under `root` as `root`-relative paths, sorted for
/// deterministic plans.
pub fn walk_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // Not a directory (or unreadable): nothing to walk.
            Err(_) if dir != root => continue,
            Err(e) if dir == root => return Err(e),
            Err(_) => continue,
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            }
        }
    }
    out.sort();
    Ok(out)
}

fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// Whether `candidate` exists with the same size and SHA-256 as `source`.
fn files_identical(source: &Path, candidate: &Path) -> bool {
    let Ok(src_meta) = std::fs::metadata(source) else {
        return false;
    };
    let Ok(dst_meta) = std::fs::metadata(candidate) else {
        return false;
    };
    if src_meta.len() != dst_meta.len() {
        return false;
    }
    match (sha256_file(source), sha256_file(candidate)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Analyze a legacy → canonical migration without touching the filesystem.
pub fn preflight(source: &Path, destination: &Path) -> Result<MigrationPlan, HomeMigrationError> {
    let rel_files = match walk_files(source) {
        Ok(files) => files,
        // A missing (or vanished) source is simply nothing to migrate.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MigrationPlan {
                source: source.to_path_buf(),
                destination: destination.to_path_buf(),
                file_count: 0,
                total_bytes: 0,
                state: MigrationState::NothingToDo,
                pending: Vec::new(),
            });
        }
        Err(e) => return Err(e.into()),
    };
    if rel_files.is_empty() {
        return Ok(MigrationPlan {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            file_count: 0,
            total_bytes: 0,
            state: MigrationState::NothingToDo,
            pending: Vec::new(),
        });
    }

    let mut total_bytes = 0u64;
    let mut pending = Vec::new();
    let mut conflicts = Vec::new();
    let mut all_present = true;

    for rel in &rel_files {
        let src = source.join(rel);
        let dst = destination.join(rel);
        total_bytes += std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);

        if !dst.exists() {
            all_present = false;
            pending.push(rel.clone());
            continue;
        }
        if files_identical(&src, &dst) {
            continue;
        }
        // Same relative path, different content: a hard conflict.
        conflicts.push((src, dst));
    }

    let state = if !conflicts.is_empty() {
        MigrationState::Conflict { conflicts }
    } else if all_present {
        MigrationState::AlreadyMigrated
    } else {
        MigrationState::Ready
    };

    Ok(MigrationPlan {
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        file_count: rel_files.len(),
        total_bytes,
        state,
        pending,
    })
}

// ── Copy + verify ──────────────────────────────────────────────────────────

/// Copy `source` → `destination` if the destination does not already match.
///
/// Idempotent: a destination file with the same size + SHA-256 is skipped.
/// Otherwise the content is written to `<destination>.part-<pid>`, fsynced,
/// and renamed into place; the destination directory is fsynced best-effort.
pub fn copy_file_idempotent(source: &Path, destination: &Path) -> std::io::Result<bool> {
    if destination.exists() && files_identical(source, destination) {
        return Ok(false); // skipped: already identical
    }

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = destination.with_file_name(format!(
        "{}.part-{}",
        destination
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        std::process::id()
    ));

    {
        let mut src = std::fs::File::open(source)?;
        let mut dst = std::fs::File::create(&part)?;
        std::io::copy(&mut src, &mut dst)?;
        dst.sync_all()?;
    }
    std::fs::rename(&part, destination)?;

    // Best-effort directory fsync so the rename is durable.
    #[cfg(unix)]
    if let Some(parent) = destination.parent()
        && let Ok(dir) = std::fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }

    Ok(true) // copied
}

/// Verify the migration: every source file must exist in the destination
/// with a matching size + SHA-256. Extra destination files are not part of
/// the migration set and are ignored.
pub fn verify(source: &Path, destination: &Path) -> Result<(), HomeMigrationError> {
    for rel in walk_files(source)? {
        let src = source.join(&rel);
        let dst = destination.join(&rel);
        if !dst.exists() {
            return Err(HomeMigrationError::Verify(format!(
                "missing in destination: {}",
                dst.display()
            )));
        }
        if !files_identical(&src, &dst) {
            return Err(HomeMigrationError::Verify(format!(
                "content mismatch: {} vs {}",
                src.display(),
                dst.display()
            )));
        }
    }
    Ok(())
}

// ── Run ────────────────────────────────────────────────────────────────────

/// What a [`run`] invocation did.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// Nothing to migrate (no legacy home, or it is empty).
    NothingToDo,
    /// Conflicting files exist; nothing was touched.
    Conflict { conflicts: Vec<(PathBuf, PathBuf)> },
    /// Destination already matched; optionally completed a stale
    /// `in_progress` journal.
    AlreadyMigrated { completed_journal: bool },
    /// Dry run: preflight only, filesystem untouched.
    DryRun(Box<MigrationPlan>),
    /// Copy completed and verified.
    Copied { copied: usize, skipped: usize },
}

/// Execute (or dry-run) the migration.
///
/// All paths are injected so the engine is testable without touching the
/// real `$HOME`. Safe to re-run at any point: the copy step is idempotent
/// per file and a failed verify leaves the journal `in_progress` for repair.
pub fn run(
    source: &Path,
    destination: &Path,
    journal_path: &Path,
    dry_run: bool,
) -> Result<RunOutcome, HomeMigrationError> {
    let plan = preflight(source, destination)?;

    if dry_run {
        return Ok(RunOutcome::DryRun(Box::new(plan)));
    }

    match plan.state {
        MigrationState::NothingToDo => Ok(RunOutcome::NothingToDo),
        MigrationState::Conflict { conflicts } => Ok(RunOutcome::Conflict { conflicts }),
        MigrationState::AlreadyMigrated => {
            // Complete a stale in-progress journal, if any; otherwise this is
            // a pure no-op.
            let mut completed_journal = false;
            if let Some(journal) = MigrationJournal::load(journal_path)
                && journal.is_in_progress()
            {
                let mut done = journal;
                done.status = "complete".to_string();
                done.save(journal_path)?;
                completed_journal = true;
            }
            Ok(RunOutcome::AlreadyMigrated { completed_journal })
        }
        MigrationState::Ready => {
            // Journal BEFORE the first filesystem mutation.
            let journal = MigrationJournal::new_in_progress(source, destination);
            journal.save(journal_path)?;

            let mut copied = 0usize;
            let mut skipped = 0usize;
            for rel in &plan.pending {
                let src = source.join(rel);
                let dst = destination.join(rel);
                if copy_file_idempotent(&src, &dst)? {
                    copied += 1;
                } else {
                    skipped += 1;
                }
            }

            verify(source, destination)?;

            let mut done = MigrationJournal::new_in_progress(source, destination);
            done.status = "complete".to_string();
            done.save(journal_path)?;

            Ok(RunOutcome::Copied { copied, skipped })
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Legacy tree with three files; fresh destination.
    fn setup_source() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("skills/my-skill")).unwrap();
        fs::write(tmp.path().join("auth.json"), r#"{"p":"k"}"#).unwrap();
        fs::write(tmp.path().join("skills/my-skill/SKILL.md"), "# skill").unwrap();
        fs::write(tmp.path().join("WATCHDOG.md"), "watch").unwrap();
        tmp
    }

    #[test]
    fn walk_files_lists_recursive_relative_paths() {
        let tmp = setup_source();
        let files = walk_files(tmp.path()).unwrap();
        assert_eq!(
            files,
            vec![
                PathBuf::from("WATCHDOG.md"),
                PathBuf::from("auth.json"),
                PathBuf::from("skills/my-skill/SKILL.md"),
            ]
        );
    }

    #[test]
    fn preflight_ready_when_destination_missing() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        let plan = preflight(src.path(), dst.path()).unwrap();
        assert_eq!(plan.state, MigrationState::Ready);
        assert_eq!(plan.file_count, 3);
        assert_eq!(plan.total_bytes, 9 + 7 + 5);
        assert_eq!(plan.pending.len(), 3);
    }

    #[test]
    fn preflight_nothing_to_do_when_source_missing_or_empty() {
        let plan = preflight(Path::new("/nonexistent/legacy-home"), Path::new("/tmp/x")).unwrap();
        assert_eq!(plan.state, MigrationState::NothingToDo);

        let empty = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let plan = preflight(empty.path(), dst.path()).unwrap();
        assert_eq!(plan.state, MigrationState::NothingToDo);
    }

    #[test]
    fn preflight_already_migrated_when_identical() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        run(src.path(), dst.path(), &dst.path().join("j.json"), false).unwrap();
        // Rerun: everything identical.
        let plan = preflight(src.path(), dst.path()).unwrap();
        assert_eq!(plan.state, MigrationState::AlreadyMigrated);
        assert!(plan.pending.is_empty());
    }

    #[test]
    fn preflight_conflict_on_differing_file() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir_all(dst.path().join("skills")).unwrap();
        fs::write(dst.path().join("auth.json"), r#"{"different":true}"#).unwrap();

        let plan = preflight(src.path(), dst.path()).unwrap();
        match plan.state {
            MigrationState::Conflict { conflicts } => {
                assert_eq!(conflicts.len(), 1);
                assert_eq!(conflicts[0].0, src.path().join("auth.json"));
                assert_eq!(conflicts[0].1, dst.path().join("auth.json"));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[test]
    fn run_copies_and_completes_journal() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        let journal_dir = tempfile::tempdir().unwrap();
        let journal = journal_dir.path().join("journal.json");

        match run(src.path(), dst.path(), &journal, false).unwrap() {
            RunOutcome::Copied { copied, skipped } => {
                assert_eq!(copied, 3);
                assert_eq!(skipped, 0);
            }
            other => panic!("expected Copied, got {other:?}"),
        }

        // Content landed.
        assert_eq!(
            fs::read_to_string(dst.path().join("auth.json")).unwrap(),
            r#"{"p":"k"}"#
        );
        assert!(dst.path().join("skills/my-skill/SKILL.md").is_file());

        // Journal marked complete.
        let j = MigrationJournal::load(&journal).unwrap();
        assert_eq!(j.status, "complete");
        assert_eq!(j.version, JOURNAL_VERSION);
        assert_eq!(j.source, src.path());
        assert_eq!(j.destination, dst.path());

        // Source untouched.
        assert!(src.path().join("auth.json").is_file());
    }

    #[test]
    fn run_is_idempotent_and_resumes_partial_copy() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        let journal_dir = tempfile::tempdir().unwrap();
        let journal = journal_dir.path().join("journal.json");

        run(src.path(), dst.path(), &journal, false).unwrap();

        // Simulate a restart after everything is already copied: rerun.
        match run(src.path(), dst.path(), &journal, false).unwrap() {
            RunOutcome::AlreadyMigrated { completed_journal } => {
                // First run completed the journal, so the rerun does not
                // need to complete anything.
                assert!(!completed_journal);
            }
            other => panic!("expected AlreadyMigrated, got {other:?}"),
        }
    }

    #[test]
    fn run_resume_after_partial_copy_completes() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        let journal_dir = tempfile::tempdir().unwrap();
        let journal = journal_dir.path().join("journal.json");

        // Simulate a crashed first run: journal in_progress, one file copied.
        let journal_entry = MigrationJournal::new_in_progress(src.path(), dst.path());
        journal_entry.save(&journal).unwrap();
        fs::create_dir_all(dst.path().join("skills/my-skill")).unwrap();
        fs::write(dst.path().join("auth.json"), r#"{"p":"k"}"#).unwrap();

        match run(src.path(), dst.path(), &journal, false).unwrap() {
            RunOutcome::Copied { copied, skipped } => {
                // The pre-copied auth.json was already excluded from the
                // pending set by preflight, so the copy pass has no skips.
                assert_eq!(copied, 2);
                assert_eq!(skipped, 0);
            }
            other => panic!("expected Copied, got {other:?}"),
        }

        let j = MigrationJournal::load(&journal).unwrap();
        assert_eq!(j.status, "complete");
    }

    #[test]
    fn verify_fails_on_post_migration_divergence() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        let journal_dir = tempfile::tempdir().unwrap();
        let journal = journal_dir.path().join("journal.json");

        run(src.path(), dst.path(), &journal, false).unwrap();
        // Post-migration divergence in the destination.
        fs::write(dst.path().join("WATCHDOG.md"), "tampered").unwrap();

        // Verify surfaces the mismatch as an error...
        let err = verify(src.path(), dst.path()).unwrap_err();
        assert!(err.to_string().contains("content mismatch"));

        // ...and preflight classifies the divergence as a hard conflict, so
        // a blind rerun cannot overwrite the changed destination file.
        let plan = preflight(src.path(), dst.path()).unwrap();
        assert!(matches!(plan.state, MigrationState::Conflict { .. }));
    }

    #[test]
    fn dry_run_mutates_nothing() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        let journal_dir = tempfile::tempdir().unwrap();
        let journal = journal_dir.path().join("journal.json");
        let before = walk_files(dst.path()).unwrap();

        match run(src.path(), dst.path(), &journal, true).unwrap() {
            RunOutcome::DryRun(plan) => {
                assert_eq!(plan.state, MigrationState::Ready);
                assert_eq!(plan.file_count, 3);
            }
            other => panic!("expected DryRun, got {other:?}"),
        }

        assert_eq!(walk_files(dst.path()).unwrap(), before);
        assert!(!journal.exists());
    }

    #[test]
    fn stale_in_progress_journal_is_completed_on_already_migrated() {
        let src = setup_source();
        let dst = tempfile::tempdir().unwrap();
        let journal_dir = tempfile::tempdir().unwrap();
        let journal = journal_dir.path().join("journal.json");

        // Full copy done by hand, journal left in_progress (crashed run).
        let opts = copy_tree(src.path(), dst.path());
        assert_eq!(opts, 3);
        let entry = MigrationJournal::new_in_progress(src.path(), dst.path());
        entry.save(&journal).unwrap();

        match run(src.path(), dst.path(), &journal, false).unwrap() {
            RunOutcome::AlreadyMigrated { completed_journal } => {
                assert!(completed_journal);
            }
            other => panic!("expected AlreadyMigrated, got {other:?}"),
        }
        assert_eq!(MigrationJournal::load(&journal).unwrap().status, "complete");
    }

    fn copy_tree(source: &Path, destination: &Path) -> usize {
        let mut n = 0;
        for rel in walk_files(source).unwrap() {
            let dst = destination.join(&rel);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::copy(source.join(&rel), &dst).unwrap();
            n += 1;
        }
        n
    }
}
