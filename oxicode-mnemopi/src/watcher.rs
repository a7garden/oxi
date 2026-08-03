//! Memory file watcher — ported from grok-build
//! `xai-grok-memory/src/watcher.rs` (Apache-2.0).
//!
//! Watches `memory_dir` for `.md` file changes (create, modify, remove)
//! and accumulates affected paths. The search path checks
//! [`MemoryFileWatcher::is_dirty`] before each query and syncs the index
//! for all dirty paths:
//! - **created / modified** files trigger reindex
//! - **deleted** files trigger stale-chunk removal
//!
//! ## Lock-free design
//!
//! Uses [`arc_swap::ArcSwap`] for lock-free dirty-path tracking:
//! - **Insert** (notify thread): `dirty_files.rcu(|old| clone + insert)`
//! - **Take** (search path): `dirty_files.swap(empty)` — single atomic
//!   pointer exchange
//! - **Quick check**: `dirty.load(Relaxed)` — single atomic load
//!
//! The watcher must NOT cause feedback loops: internal writes that go
//! through `MemoryStorage` (or any in-process write) ALSO produce notify
//! events. The host is responsible for either (a) routing internal
//! writes to a non-watched directory, (b) accepting redundant reindex
//! work as idempotent (chunk content-hashes dedup), or (c) using a
//! debounce / ignore-list. grok-build accepts (b) — the index is
//! idempotent and content-hashed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arc_swap::ArcSwap;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Watches a memory directory for `.md` file changes.
///
/// Holds the underlying `notify::RecommendedWatcher` (so it stays alive)
/// plus a lock-free dirty-path set.
pub struct MemoryFileWatcher {
    dirty_files: Arc<ArcSwap<HashSet<PathBuf>>>,
    dirty: Arc<AtomicBool>,
    _watcher: Option<RecommendedWatcher>,
}

impl MemoryFileWatcher {
    /// Start watching `memory_dir` recursively for `.md` file changes.
    ///
    /// Returns `None` (logged at INFO) when the watcher fails to
    /// initialize — non-fatal, callers continue without live updates.
    /// Returns `Some(watcher)` on success.
    pub fn start(memory_dir: &Path) -> Option<Self> {
        let dirty_files: Arc<ArcSwap<HashSet<PathBuf>>> =
            Arc::new(ArcSwap::from_pointee(HashSet::new()));
        let dirty = Arc::new(AtomicBool::new(false));

        let dirty_files_for_cb = Arc::clone(&dirty_files);
        let dirty_for_cb = Arc::clone(&dirty);
        let mut watcher = match notify::recommended_watcher(move |res: Result<notify::Event, _>| {
            if let Ok(event) = res {
                let relevant: Vec<PathBuf> = event
                    .paths
                    .into_iter()
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
                    .collect();
                if relevant.is_empty() {
                    return;
                }
                dirty_files_for_cb.rcu(|old| {
                    let mut next: HashSet<PathBuf> = (**old).clone();
                    next.extend(relevant.iter().cloned());
                    next
                });
                dirty_for_cb.store(true, Ordering::Release);
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::info!(
                    dir = %memory_dir.display(),
                    error = %e,
                    "memory file watcher unavailable; live external edits will not refresh the index"
                );
                return None;
            }
        };

        if let Err(e) = watcher.watch(memory_dir, RecursiveMode::Recursive) {
            tracing::info!(
                dir = %memory_dir.display(),
                error = %e,
                "memory file watcher cannot attach; live external edits will not refresh the index"
            );
            return None;
        }

        tracing::debug!(dir = %memory_dir.display(), "memory file watcher attached");

        Some(Self {
            dirty_files,
            dirty,
            _watcher: Some(watcher),
        })
    }

    /// Construct a watcher that is always clean (never fires). Useful for
    /// tests and for hosts that disable live updates explicitly.
    pub fn disabled() -> Self {
        Self {
            dirty_files: Arc::new(ArcSwap::from_pointee(HashSet::new())),
            dirty: Arc::new(AtomicBool::new(false)),
            _watcher: None,
        }
    }

    /// Quick check: true if any files have been modified since last take.
    /// Single atomic load — safe to call on every search.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    /// Take all accumulated dirty paths, resetting the dirty state.
    /// Returns the paths that changed since the last take. The returned
    /// `Vec` is in unspecified order; deduplication is up to the caller
    /// (though each path is unique within a single take).
    pub fn take_dirty(&self) -> Vec<PathBuf> {
        let empty: HashSet<PathBuf> = HashSet::new();
        let snapshot = self.dirty_files.swap(Arc::new(empty));
        self.dirty.store(false, Ordering::Release);
        snapshot.iter().cloned().collect()
    }

    /// Manually mark a path as dirty (e.g. for explicit reindex requests).
    /// No-op if the path is already in the set.
    pub fn mark_dirty(&self, path: PathBuf) {
        let existed = self.dirty_files.rcu(|old| {
            if old.contains(&path) {
                (**old).clone()
            } else {
                let mut next: HashSet<PathBuf> = (**old).clone();
                next.insert(path.clone());
                next
            }
        });
        // `rcu` returns the previous value; if our path is in it, we
        // didn't change anything. Either way, mark dirty.
        let _ = existed;
        self.dirty.store(true, Ordering::Release);
    }
}

impl std::fmt::Debug for MemoryFileWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryFileWatcher")
            .field("watching", &self._watcher.is_some())
            .field("dirty", &self.dirty.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Filter helper for callers: returns `true` when an event kind
/// represents an interesting change (create / modify / remove).
pub fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    /// Wait up to 2 seconds for `cond` to return true. Notify events
    /// arrive asynchronously, so tests need a grace window.
    fn wait_for<F: Fn() -> bool>(cond: F) -> bool {
        for _ in 0..200 {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn disabled_watcher_never_dirty() {
        let w = MemoryFileWatcher::disabled();
        assert!(!w.is_dirty());
        assert!(w.take_dirty().is_empty());
    }

    #[test]
    fn mark_dirty_round_trips_through_take() {
        let w = MemoryFileWatcher::disabled();
        w.mark_dirty(PathBuf::from("/tmp/a.md"));
        w.mark_dirty(PathBuf::from("/tmp/b.md"));
        assert!(w.is_dirty());
        let dirty = w.take_dirty();
        assert_eq!(dirty.len(), 2);
        assert!(!w.is_dirty(), "take resets dirty flag");
        // Second take is empty.
        assert!(w.take_dirty().is_empty());
    }

    #[test]
    fn mark_dirty_idempotent() {
        let w = MemoryFileWatcher::disabled();
        let p = PathBuf::from("/tmp/x.md");
        w.mark_dirty(p.clone());
        w.mark_dirty(p.clone());
        let dirty = w.take_dirty();
        assert_eq!(dirty.len(), 1);
    }

    #[test]
    fn start_detects_external_create() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let w = MemoryFileWatcher::start(&dir).expect("watcher should attach");

        let target = dir.join("MEMORY.md");
        let mut f = std::fs::File::create(&target).expect("create");
        writeln!(f, "# Project notes").expect("write");
        drop(f);

        assert!(
            wait_for(|| w.is_dirty()),
            "watcher should flag dirty after external create"
        );
        let dirty = w.take_dirty();
        // macOS resolves `/var/folders/...` to `/private/var/folders/...`
        // — accept either form.
        let canonical_target = std::fs::canonicalize(&target).unwrap_or_else(|_| target.clone());
        let matched = dirty.iter().any(|p| {
            // Exact match or canonical-exact match.
            p == &target || p == &canonical_target || p.file_name() == canonical_target.file_name()
        });
        assert!(
            matched,
            "dirty set should contain the created file, got {dirty:?}"
        );
    }
    #[test]
    fn start_detects_external_modify() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let target = dir.join("notes.md");
        std::fs::write(&target, "before").expect("initial write");

        let w = MemoryFileWatcher::start(&dir).expect("watcher should attach");
        // Drain any initial events from the file already existing.
        let _ = w.take_dirty();

        std::fs::write(&target, "after some edits").expect("modify");
        assert!(
            wait_for(|| w.is_dirty()),
            "watcher should flag dirty after external modify"
        );
    }

    #[test]
    fn start_detects_external_remove() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let target = dir.join("gone.md");
        std::fs::write(&target, "will delete").expect("initial write");

        let w = MemoryFileWatcher::start(&dir).expect("watcher should attach");
        let _ = w.take_dirty();

        std::fs::remove_file(&target).expect("remove");
        assert!(
            wait_for(|| w.is_dirty()),
            "watcher should flag dirty after external remove"
        );
    }

    #[test]
    fn start_ignores_non_md_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let w = MemoryFileWatcher::start(&dir).expect("watcher should attach");

        let non_md = dir.join("data.json");
        std::fs::write(&non_md, "{}").expect("write");

        std::thread::sleep(Duration::from_millis(200));
        assert!(!w.is_dirty(), "non-.md files should not trip the watcher");
    }

    #[test]
    fn is_relevant_event_classifies_create_modify_remove() {
        assert!(is_relevant_event(&EventKind::Create(
            notify::event::CreateKind::File
        )));
        assert!(is_relevant_event(&EventKind::Modify(
            notify::event::ModifyKind::Data(notify::event::DataChange::Content)
        )));
        assert!(is_relevant_event(&EventKind::Remove(
            notify::event::RemoveKind::File
        )));
        assert!(!is_relevant_event(&EventKind::Access(
            notify::event::AccessKind::Read
        )));
        assert!(!is_relevant_event(&EventKind::Any));
    }
}
