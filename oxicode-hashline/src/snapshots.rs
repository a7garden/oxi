//! Per-session snapshot store used by recovery and the patcher to bind hashline
//! section tags to the exact file content that minted them.
//!
//! A section tag is a content-derived hash of the *whole file* (see
//! [`compute_file_hash`]). Any read of byte-identical
//! content mints the same tag, so reads of one file state fuse onto one anchor,
//! and a follow-up edit anchored at any line validates whenever the live file
//! still hashes to it.
//!
//! Producers (`read` / `search` / `write` tools) call
//! [`SnapshotStore::record`] with the full normalized text they observed. The
//! store hashes it, dedups against the per-path history, and returns the tag.
//! Consumers (the patcher) resolve a stale tag back to the recorded full text
//! via [`SnapshotStore::by_hash`] and 3-way-merge the would-be edit onto the
//! live content.
//!
//! [`InMemorySnapshotStore`] ships as a sensible default backed by [`lru`]:
//! a bounded set of paths, each with a short history of full-file versions so
//! in-session edit chains can still recover against the version a stale tag
//! names.
//!
//! Ported from omp `packages/hashline/src/snapshots.ts`.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::time::SystemTime;

use lru::LruCache;
use parking_lot::RwLock;

use crate::format::compute_file_hash;
use crate::normalize::{normalize_to_lf, strip_bom};

// ── Limits ───────────────────────────────────────────────────────────────

/// Default maximum number of distinct paths tracked at once (LRU eviction).
pub const DEFAULT_MAX_PATHS: usize = 30;
/// Default maximum full-file versions retained per path (oldest dropped first).
pub const DEFAULT_MAX_VERSIONS_PER_PATH: usize = 4;
/// Default global ceiling on retained snapshot text, summed across every path's
/// version history, measured in bytes (UTF-8).
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

// ── Snapshot ─────────────────────────────────────────────────────────────

/// One full-file version observed at a point in time. The tag the model sees is
/// [`Snapshot::hash`]; recovery replays edits against [`Snapshot::text`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Canonical path this version belongs to.
    pub path: String,
    /// Full normalized (LF, no BOM) file text as observed.
    pub text: String,
    /// Content-derived tag for [`Snapshot::text`] (see [`compute_file_hash`]).
    pub hash: String,
    /// Wall-clock time the version was recorded.
    pub recorded_at: SystemTime,
    /// 1-indexed file lines a producer actually *displayed* under this tag. A
    /// partial read leaves this sparse; a whole-file read fills every line.
    /// Multiple reads of the same content union into one set. `None` means "no
    /// provenance recorded" — the patcher then skips the seen-line check.
    pub seen_lines: Option<HashSet<u32>>,
}

// ── Trait ────────────────────────────────────────────────────────────────

/// Storage seam for full-file version snapshots. The patcher calls [`head`]
/// for the latest version of a path and [`by_hash`] when it needs the specific
/// historical version a section's stale tag names.
///
/// [`head`]: SnapshotStore::head
/// [`by_hash`]: SnapshotStore::by_hash
pub trait SnapshotStore: Send + Sync + std::fmt::Debug {
    /// Most-recently recorded version for `path`, or `None` if none.
    fn head(&self, _path: &str) -> Option<Snapshot> {
        None
    }
    /// Recorded version for `path` whose tag equals `hash`, or `None`.
    fn by_hash(&self, _path: &str, _hash: &str) -> Option<Snapshot> {
        None
    }
    /// Record the full normalized text of `path` and return its content tag.
    /// `seen_lines` (optional) are the 1-indexed lines the producer displayed;
    /// they merge into [`Snapshot::seen_lines`] across reads of identical text.
    fn record(&self, _path: &str, _full_text: &str, _seen_lines: Option<&[u32]>) -> String {
        String::new()
    }
    /// Merge `lines` into the [`Snapshot::seen_lines`] of the version whose tag
    /// equals `hash`. No-op when no such version is retained.
    fn record_seen_lines(&self, _path: &str, _hash: &str, _lines: &[u32]) {}
    /// Drop the version history for a single path.
    fn invalidate(&self, _path: &str) {}
    /// Drop every version history.
    fn clear(&self) {}
}

// ── InMemorySnapshotStore ────────────────────────────────────────────────

/// Knobs for [`InMemorySnapshotStore`].
#[derive(Debug, Clone, Copy)]
pub struct InMemorySnapshotStoreOptions {
    /// Maximum number of distinct paths tracked at once. LRU eviction.
    pub max_paths: usize,
    /// Maximum full-file versions retained per path. Oldest dropped first.
    pub max_versions_per_path: usize,
    /// Global ceiling on retained snapshot text summed across every path's
    /// version history, measured in bytes. Least-recently-used path histories
    /// are evicted to stay under it.
    pub max_total_bytes: usize,
}

impl Default for InMemorySnapshotStoreOptions {
    fn default() -> Self {
        Self {
            max_paths: DEFAULT_MAX_PATHS,
            max_versions_per_path: DEFAULT_MAX_VERSIONS_PER_PATH,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

/// Mutable inner state guarded by the [`RwLock`]. `LruCache` is `!Sync`, hence
/// the lock; the per-path value is a short ring of full-file versions.
struct Inner {
    cache: LruCache<String, Vec<Snapshot>>,
    max_versions_per_path: usize,
    max_total_bytes: usize,
}

/// In-memory [`SnapshotStore`] backed by [`lru`]. Per-path history is a short
/// ring of full-file versions (oldest dropped first); per-session path tracking
/// is LRU-bounded so cold paths age out automatically.
///
/// Recording byte-identical content again refreshes recency and reuses the
/// existing tag (read fusion); recording new content unshifts a fresh version
/// onto the front of the path history.
pub struct InMemorySnapshotStore {
    inner: RwLock<Inner>,
}

impl InMemorySnapshotStore {
    /// Build a store with the default limits (30 paths × 4 versions × 64 MiB).
    pub fn new() -> Self {
        Self::with_options(InMemorySnapshotStoreOptions::default())
    }

    /// Build a store with custom limits.
    pub fn with_options(opts: InMemorySnapshotStoreOptions) -> Self {
        let cap = NonZeroUsize::new(opts.max_paths.max(1)).expect("clamped to >= 1");
        Self {
            inner: RwLock::new(Inner {
                cache: LruCache::new(cap),
                max_versions_per_path: opts.max_versions_per_path.max(1),
                max_total_bytes: opts.max_total_bytes,
            }),
        }
    }
}

impl Default for InMemorySnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for InMemorySnapshotStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemorySnapshotStore")
            .finish_non_exhaustive()
    }
}

impl SnapshotStore for InMemorySnapshotStore {
    fn head(&self, path: &str) -> Option<Snapshot> {
        let mut inner = self.inner.write();
        // `get` refreshes LRU recency for `path`, matching omp's `.get`.
        inner.cache.get(path).and_then(|hist| hist.first().cloned())
    }

    fn by_hash(&self, path: &str, hash: &str) -> Option<Snapshot> {
        let mut inner = self.inner.write();
        inner
            .cache
            .get(path)
            .and_then(|hist| hist.iter().find(|s| s.hash == hash).cloned())
    }

    fn record(&self, path: &str, full_text: &str, seen_lines: Option<&[u32]>) -> String {
        let mut inner = self.inner.write();
        // Normalize once: LF + no BOM. `compute_file_hash` trims trailing
        // whitespace for hashing internally, so the tag is stable regardless.
        let text = normalize_to_lf(strip_bom(full_text).text);
        let hash = compute_file_hash(&text);
        // `get` refreshes LRU recency for `path`.
        let mut history = inner.cache.get(path).cloned().unwrap_or_default();

        if let Some(pos) = history.iter().position(|s| s.hash == hash) {
            // Same content observed again: refresh timestamp, promote to head,
            // and union any newly-displayed lines. Reuse the tag.
            let mut snap = history.remove(pos);
            snap.recorded_at = SystemTime::now();
            if let Some(lines) = seen_lines {
                snap.seen_lines
                    .get_or_insert_with(HashSet::new)
                    .extend(lines.iter().copied());
            }
            history.insert(0, snap);
        } else {
            let mut snap = Snapshot {
                path: path.to_string(),
                text,
                hash: hash.clone(),
                recorded_at: SystemTime::now(),
                seen_lines: None,
            };
            if let Some(lines) = seen_lines {
                snap.seen_lines = Some(lines.iter().copied().collect());
            }
            history.insert(0, snap);
            // Oldest versions drop off the back of the ring.
            while history.len() > inner.max_versions_per_path {
                history.pop();
            }
        }

        inner.cache.put(path.to_string(), history);
        enforce_byte_limit(&mut inner);
        hash
    }

    fn record_seen_lines(&self, path: &str, hash: &str, lines: &[u32]) {
        let mut inner = self.inner.write();
        if let Some(hist) = inner.cache.get_mut(path)
            && let Some(snap) = hist.iter_mut().find(|s| s.hash == hash)
        {
            snap.seen_lines
                .get_or_insert_with(HashSet::new)
                .extend(lines.iter().copied());
        }
    }

    fn invalidate(&self, path: &str) {
        let mut inner = self.inner.write();
        inner.cache.pop(path);
    }

    fn clear(&self) {
        let mut inner = self.inner.write();
        inner.cache.clear();
    }
}

/// Evict least-recently-used path histories until retained text fits the global
/// byte ceiling. Always keeps the most-recently-recorded path (even a single
/// file larger than the ceiling is retained rather than dropped outright).
fn enforce_byte_limit(inner: &mut Inner) {
    loop {
        let total: usize = inner
            .cache
            .iter()
            .flat_map(|(_, hist)| hist.iter())
            .map(|s| s.text.len())
            .sum();
        if total <= inner.max_total_bytes || inner.cache.len() <= 1 {
            break;
        }
        if inner.cache.pop_lru().is_none() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "src/foo.rs";

    fn record(store: &impl SnapshotStore, path: &str, text: &str) -> String {
        store.record(path, text, None)
    }

    #[test]
    fn record_and_head_round_trip() {
        let store = InMemorySnapshotStore::new();
        let text = "fn main() {}\n";
        let tag = record(&store, PATH, text);
        assert_eq!(tag.len(), 4);
        let head = store.head(PATH).expect("head after record");
        assert_eq!(head.hash, tag);
        assert_eq!(head.text, text);
        assert_eq!(head.path, PATH);
        assert!(head.seen_lines.is_none());
    }

    #[test]
    fn head_missing_returns_none() {
        let store = InMemorySnapshotStore::new();
        assert!(store.head(PATH).is_none());
    }

    #[test]
    fn by_hash_finds_recorded_version() {
        let store = InMemorySnapshotStore::new();
        let tag = record(&store, PATH, "alpha\n");
        assert_eq!(
            store.by_hash(PATH, &tag).map(|s| s.text),
            Some("alpha\n".to_string())
        );
        assert!(store.by_hash(PATH, "DEAD").is_none());
        assert!(store.by_hash("other.rs", &tag).is_none());
    }

    #[test]
    fn record_normalizes_text_before_storing() {
        let store = InMemorySnapshotStore::new();
        // CRLF + BOM must collapse to canonical LF; the tag is the same as the
        // already-canonical text.
        let canonical = "line one\nline two\n";
        let raw = "\u{feff}line one\r\nline two\r\n";
        let tag_raw = store.record(PATH, raw, None);
        let tag_canonical = store.record(PATH, canonical, None);
        assert_eq!(tag_raw, tag_canonical, "hash must be normalization-stable");
        let head = store.head(PATH).expect("head");
        assert_eq!(head.text, canonical, "stored text must be normalized");
    }

    #[test]
    fn record_dedups_identical_content() {
        let store = InMemorySnapshotStore::new();
        let tag1 = record(&store, PATH, "same\n");
        let tag2 = record(&store, PATH, "same\n");
        assert_eq!(tag1, tag2, "identical content reuses the tag");
        // Still exactly one version for this hash.
        assert!(store.by_hash(PATH, &tag1).is_some());
    }

    #[test]
    fn record_promotes_existing_content_to_head() {
        let store = InMemorySnapshotStore::new();
        let _a = record(&store, PATH, "a\n");
        let _b = record(&store, PATH, "b\n");
        // Re-record a: it should become head again even though b was newer.
        let tag_a = record(&store, PATH, "a\n");
        let head = store.head(PATH).expect("head");
        assert_eq!(head.hash, tag_a);
        assert_eq!(head.text, "a\n");
    }

    #[test]
    fn seen_lines_union_on_identical_content() {
        let store = InMemorySnapshotStore::new();
        let tag = store.record(PATH, "a\nb\nc\n", Some(&[1, 2]));
        let head = store.head(PATH).expect("head");
        assert_eq!(head.seen_lines.as_ref().map(|s| s.len()), Some(2));

        // Re-read the same content but display a different line range: union.
        let _ = store.record(PATH, "a\nb\nc\n", Some(&[2, 3]));
        let head = store.head(PATH).expect("head");
        let seen = head.seen_lines.expect("seen_lines");
        assert_eq!(seen, [1, 2, 3].into_iter().collect::<HashSet<_>>());
        // Same tag reused.
        assert_eq!(head.hash, tag);
    }

    #[test]
    fn record_seen_lines_merges_into_existing_version() {
        let store = InMemorySnapshotStore::new();
        let tag = record(&store, PATH, "a\nb\nc\n");
        store.record_seen_lines(PATH, &tag, &[1]);
        store.record_seen_lines(PATH, &tag, &[3]);
        let head = store.head(PATH).expect("head");
        let seen = head.seen_lines.expect("seen_lines");
        assert_eq!(seen, [1, 3].into_iter().collect::<HashSet<_>>());
    }

    #[test]
    fn record_seen_lines_noop_for_unknown_hash() {
        let store = InMemorySnapshotStore::new();
        store.record(PATH, "a\n", None);
        // Must not panic / must be a silent no-op.
        store.record_seen_lines(PATH, "NOPE", &[1]);
    }

    #[test]
    fn version_cap_drops_oldest() {
        let store = InMemorySnapshotStore::with_options(InMemorySnapshotStoreOptions {
            max_versions_per_path: 2,
            ..Default::default()
        });
        let a = record(&store, PATH, "a\n");
        let b = record(&store, PATH, "b\n");
        let c = record(&store, PATH, "c\n");
        // [c, b] retained; a aged out.
        assert_eq!(store.head(PATH).map(|s| s.hash), Some(c.clone()));
        assert!(store.by_hash(PATH, &a).is_none(), "oldest version evicted");
        assert!(store.by_hash(PATH, &b).is_some());
        assert!(store.by_hash(PATH, &c).is_some());
    }

    #[test]
    fn lru_evicts_coldest_path() {
        let store = InMemorySnapshotStore::with_options(InMemorySnapshotStoreOptions {
            max_paths: 2,
            ..Default::default()
        });
        let _ = record(&store, "p1", "one\n");
        let _ = record(&store, "p2", "two\n");
        let _ = record(&store, "p3", "three\n");
        // p1 is least-recently-used and must have aged out.
        assert!(store.head("p1").is_none(), "coldest path evicted");
        assert!(store.head("p2").is_some());
        assert!(store.head("p3").is_some());
    }

    #[test]
    fn byte_ceiling_evicts_coldest_path() {
        // Keep path count high so the byte ceiling is the binding constraint.
        let store = InMemorySnapshotStore::with_options(InMemorySnapshotStoreOptions {
            max_paths: 30,
            max_total_bytes: 10,
            ..Default::default()
        });
        let _ = store.record("p1", "aaaa", None); // 4 bytes
        let _ = store.record("p2", "bbbb", None); // 4 bytes
        let _ = store.record("p3", "cccc", None); // 4 bytes -> total 12 > 10
        assert!(
            store.head("p1").is_none(),
            "oldest path evicted by byte ceiling"
        );
        assert!(store.head("p2").is_some());
        assert!(store.head("p3").is_some());
    }

    #[test]
    fn invalidate_drops_single_path() {
        let store = InMemorySnapshotStore::new();
        let _ = record(&store, "p1", "one\n");
        let _ = record(&store, "p2", "two\n");
        store.invalidate("p1");
        assert!(store.head("p1").is_none());
        assert!(store.head("p2").is_some());
    }

    #[test]
    fn clear_drops_everything() {
        let store = InMemorySnapshotStore::new();
        let _ = record(&store, "p1", "one\n");
        let _ = record(&store, "p2", "two\n");
        store.clear();
        assert!(store.head("p1").is_none());
        assert!(store.head("p2").is_none());
    }

    #[test]
    fn store_is_send_sync() {
        // Compile-time assertion: the store must be usable across threads.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<InMemorySnapshotStore>();
    }
}
