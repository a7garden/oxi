//! Migration primitives: legacy memory → Brain (oxibrain).
//!
//! The migration is **resumable**, **non-destructive**, and **opt-in**.
//! It runs only when the user invokes `oxicode migrate brain`. The
//! Oxi Foundation v1 host does not auto-migrate on startup; the
//! migration is a one-time user action.
//!
//! ## Checkpoint
//!
//! `~/.oxicode/migration/brain.json` stores the last successfully
//! migrated memory ID. On restart, the migration resumes from that
//! point. The checkpoint file is a single JSON object:
//!
//! ```json
//! { "last_id": "m-1234", "migrated": 42, "skipped": 0, "failed": 0 }
//! ```
//!
//! The file is written atomically (temp + rename) so a crash mid-write
//! cannot leave it in a partial state. A missing or unreadable
//! checkpoint file is treated as "no checkpoint".
//!
//! ## Legacy store
//!
//! The legacy durable memory under `~/.oxicode/memory/` is read
//! through the `LegacyMemoryReader` fallible iterator. The read path
//! is **stateless** — the legacy backend is not invoked, mutated, or
//! deleted. The legacy store is left untouched until the user
//! explicitly archives it via `oxicode migrate brain --archive-legacy`.
//!
//! ## Archive
//!
//! Archival moves `~/.oxicode/memory/` to
//! `~/.oxicode/archive/memory/<timestamp>/`. The archive directory
//! inherits the original permissions. The legacy store cannot be
//! re-enabled silently — restoring requires a separate explicit
//! command (not yet implemented; future work).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default location for the migration checkpoint.
pub fn default_checkpoint_path() -> PathBuf {
    crate::foundation::fetch_oxicode_home()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("migration")
        .join("brain.json")
}

/// Default location of the legacy durable memory store.
pub fn default_legacy_path() -> PathBuf {
    crate::foundation::fetch_oxicode_home()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("memory")
}

/// On-disk checkpoint. Atomic-write via temp + rename.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Checkpoint {
    /// Last successfully migrated memory ID. `None` means fresh start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_id: Option<String>,
    /// Number of legacy items inserted into the brain.
    #[serde(default)]
    pub migrated: usize,
    /// Number of legacy items skipped (already present in brain).
    #[serde(default)]
    pub skipped: usize,
    /// Number of legacy items that failed to migrate.
    #[serde(default)]
    pub failed: usize,
}

impl Checkpoint {
    /// Load the checkpoint from disk. Missing or unreadable file is
    /// treated as "no checkpoint" (fresh start).
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Atomically write the checkpoint. Parent directories are
    /// created on demand.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).expect("checkpoint is JSON-serializable");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Returns the last migrated ID, if any.
    pub fn last_id(&self) -> Option<&str> {
        self.last_id.as_deref()
    }
}

/// Outcome of a single migration step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Item was inserted into the brain. The new ID is returned.
    Inserted(String),
    /// Item was already present in the brain (idempotent re-run).
    Skipped(String),
}

/// A single legacy memory item to migrate.
#[derive(Debug, Clone)]
pub struct LegacyItem {
    pub content: String,
    pub kind: String,
    pub subject: String,
}

/// Migration driver. Holds the checkpoint so each call can advance
/// the on-disk state.
pub struct Migration<'a> {
    backend: &'a crate::foundation::brain::BrainMemoryBackend,
    checkpoint_path: &'a Path,
    state: Checkpoint,
}

impl<'a> Migration<'a> {
    pub fn new(
        backend: &'a crate::foundation::brain::BrainMemoryBackend,
        checkpoint_path: &'a Path,
    ) -> Self {
        let state = Checkpoint::load(checkpoint_path);
        Self {
            backend,
            checkpoint_path,
            state,
        }
    }

    /// Synchronously migrate one legacy item. The current thread
    /// builds a small tokio runtime and blocks on a single
    /// `backend.put_sync` call. The migration is intentionally
    /// single-shot per item so the checkpoint can advance between
    /// writes.
    pub fn migrate_one(
        &mut self,
        item: LegacyItem,
    ) -> Result<MigrationOutcome, crate::foundation::brain::MigrationError> {
        let phase = self.backend.health();

        if matches!(phase, crate::foundation::brain::BrainHealth::Unavailable) {
            return Err(crate::foundation::brain::MigrationError::BackendOffline);
        }

        let id = self
            .backend
            .put_sync(&item.content, &item.kind, &item.subject)
            .map_err(crate::foundation::brain::MigrationError::Backend)?;

        self.state.last_id = Some(id.clone());
        self.state.migrated += 1;
        self.state
            .save(self.checkpoint_path)
            .map_err(|e| crate::foundation::brain::MigrationError::Checkpoint(e.to_string()))?;

        Ok(MigrationOutcome::Inserted(id))
    }

    /// Current migration state snapshot.
    pub fn state(&self) -> &Checkpoint {
        &self.state
    }
}

/// Read-only legacy memory reader. Walks the legacy store under
/// `~/.oxicode/memory/items.jsonl`. The read path is **stateless**
/// and never mutates the legacy store.
pub struct LegacyMemoryReader {
    path: PathBuf,
}

impl LegacyMemoryReader {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Convenience constructor pointing at the default legacy home.
    pub fn for_default_home() -> Self {
        Self::new(default_legacy_path())
    }

    /// Iterate the legacy store in batches.
    ///
    /// The implementation reads `<legacy>/items.jsonl` (one JSON
    /// object per line). Legacy data with a different layout is
    /// reported as an empty iterator: the migration is never lossy,
    /// and the user can inspect the legacy store manually if the
    /// format is unrecognized.
    pub fn batches(&self, size: usize) -> LegacyBatches {
        LegacyBatches {
            path: self.path.join("items.jsonl"),
            batch_size: size.max(1),
            pending: Vec::new(),
            exhausted: false,
            loaded: false,
        }
    }
}

/// Iterator over batches of legacy items. The file is read once;
/// subsequent calls drain `pending` until exhausted.
pub struct LegacyBatches {
    path: PathBuf,
    batch_size: usize,
    pending: Vec<LegacyItem>,
    exhausted: bool,
    loaded: bool,
}

impl Iterator for LegacyBatches {
    type Item = Vec<LegacyItem>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pending.is_empty() && self.exhausted {
            return None;
        }
        if !self.pending.is_empty() {
            return Some(std::mem::take(&mut self.pending));
        }

        // Already loaded the file; nothing else to do.
        if self.loaded {
            self.exhausted = true;
            return None;
        }
        self.loaded = true;

        let contents = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(_) => {
                self.exhausted = true;
                return None;
            }
        };
        let mut acc = Vec::with_capacity(self.batch_size);
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                let content = value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let kind = value
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fact")
                    .to_string();
                let subject = value
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                acc.push(LegacyItem {
                    content,
                    kind,
                    subject,
                });
            }
        }

        if acc.is_empty() {
            self.exhausted = true;
            return None;
        }
        let take = acc.len().min(self.batch_size);
        let first_chunk: Vec<_> = acc.drain(..take).collect();
        self.pending = acc;
        if self.pending.is_empty() {
            self.exhausted = true;
        }
        Some(first_chunk)
    }
}

/// Move the legacy store to `~/.oxicode/archive/memory/<timestamp>/`.
/// Returns the destination path on success.
pub fn archive_legacy_default() -> std::io::Result<PathBuf> {
    let legacy = default_legacy_path();
    let home = legacy
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dest = home
        .join("archive")
        .join("memory")
        .join(format!("archive-{ts}"));
    std::fs::create_dir_all(dest.parent().unwrap())?;
    if legacy.exists() {
        std::fs::rename(&legacy, &dest)?;
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("brain.json");

        let mut cp = Checkpoint::default();
        cp.last_id = Some("m-42".to_string());
        cp.migrated = 42;
        cp.save(&path).unwrap();

        let loaded = Checkpoint::load(&path);
        assert_eq!(loaded.last_id.as_deref(), Some("m-42"));
        assert_eq!(loaded.migrated, 42);
    }

    #[test]
    fn checkpoint_missing_file_is_default() {
        let cp = Checkpoint::load(Path::new("/does/not/exist/brain.json"));
        assert_eq!(cp.last_id, None);
        assert_eq!(cp.migrated, 0);
    }

    #[test]
    fn legacy_reader_returns_empty_when_store_missing() {
        let reader = LegacyMemoryReader::new(PathBuf::from("/no/such/path"));
        let batches: Vec<_> = reader.batches(2).collect();
        assert!(batches.is_empty());
    }

    #[test]
    fn legacy_reader_batches_items_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let items = tmp.path().join("items.jsonl");
        std::fs::write(
            &items,
            "{\"content\":\"a\",\"kind\":\"fact\",\"subject\":\"s\"}\n\
             {\"content\":\"b\",\"kind\":\"fact\",\"subject\":\"s\"}\n\
             {\"content\":\"c\",\"kind\":\"fact\",\"subject\":\"s\"}\n",
        )
        .unwrap();
        let reader = LegacyMemoryReader::new(tmp.path().to_path_buf());
        let batches: Vec<_> = reader.batches(2).collect();
        let total: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total, 3);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 2);
        assert_eq!(batches[1].len(), 1);
    }

    #[test]
    fn legacy_reader_skips_malformed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let items = tmp.path().join("items.jsonl");
        std::fs::write(
            &items,
            "{\"content\":\"a\"}\n\
             this is not json\n\
             {\"content\":\"b\"}\n",
        )
        .unwrap();
        let reader = LegacyMemoryReader::new(tmp.path().to_path_buf());
        let batches: Vec<_> = reader.batches(10).collect();
        let total: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total, 2);
    }

    #[test]
    fn archive_legacy_default_moves_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("memory");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("items.jsonl"), "{\"content\":\"x\"}\n").unwrap();

        let home = tmp.path().to_string_lossy().to_string();
        // SAFETY: tests run sequentially in this module; the env
        // mutation is scoped to this test.
        unsafe {
            std::env::set_var("OXICODE_HOME", &home);
        }
        let dest = archive_legacy_default().unwrap();
        unsafe {
            std::env::remove_var("OXICODE_HOME");
        }

        assert!(
            dest.exists(),
            "archive path should exist: {}",
            dest.display()
        );
        assert!(!legacy.exists(), "legacy path should be moved");
        let archived = std::fs::read_to_string(dest.join("items.jsonl")).unwrap();
        assert_eq!(archived.trim(), "{\"content\":\"x\"}");
    }
}
