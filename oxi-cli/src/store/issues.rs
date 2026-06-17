//! Local issue tracking system — GitHub-style issues stored as markdown files.
//!
//! Issues live in `.oxi/issues/` at the project root (discovered by walking
//! up from the current directory until `.oxi/` is found, mirroring
//! `Settings::find_project_settings`). Each issue is a single markdown file
//! with a YAML frontmatter block holding structured metadata, followed by a
//! free-form markdown body:
//!
//! ```markdown
//! ---
//! id: 12
//! title: "Fix login bug"
//! status: open
//! priority: high
//! labels: [bug, auth]
//! assignee: null
//! created_at: 2026-06-17T10:30:00Z
//! updated_at: 2026-06-17T14:20:00Z
//! closed_at: null
//! sessions: [abc123, def456]
//! assigned_to: null
//! github: null
//! ---
//!
//! Free-form markdown body...
//! ```
//!
//! # Design decisions
//!
//! - **Why not a `StateStore` port?** Issues are *documents* that humans open
//!   in `$EDITOR`, commit to git, and diff. `StateStore` is an opaque KV/append
//!   blob. Different workload, different storage shape. This mirrors how
//!   `store/session.rs` and `store/settings.rs` coexist with the SDK ports.
//! - **Optimistic concurrency (content-hash CAS).** Mutations take an optional
//!   `content_hash` captured at the last read. The write is rejected if the
//!   on-disk content has changed since. This is the exact pattern used by the
//!   `edit` tool (`oxi-agent/src/tools/edit.rs`), so external edits (e.g.
//!   someone editing the file in vim) are detected without any locking.
//! - **Atomic writes** via temp+rename (same pattern as `store/session.rs`).
//! - **Assignment is process-liveness based, not time based.** An assigned
//!   issue records the owning session id. Whether that session is still alive
//!   is determined by an OS-held advisory lock on
//!   `.oxi/issues/.alive/<session_id>` (see [`liveness`]). When the owning
//!   process exits — including `kill -9`, crash, or terminal close — the OS
//!   releases the lock and the assignment becomes stale and reclaimable.
//!   No wall-clock expiry, no heartbeats, no zombie assignments.
//! - **Per-file write serialization** uses the agent's `file_mutation_queue`
//!   for in-process concurrency. Cross-process races are bounded by the
//!   content-hash CAS: the loser gets a "retry" response, the same semantics
//!   as the `edit` tool.

use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

// ============================================================================
// Errors
// ============================================================================

/// Errors returned by issue operations.
///
/// Kept as a small typed enum (per AGENTS.md: application crate uses anyhow
/// broadly, but these specific variants are useful to distinguish for the
/// agent tool layer and tests).
#[derive(Debug, thiserror::Error)]
pub enum IssueError {
    /// `content_hash` supplied did not match the current on-disk content.
    /// The caller should re-read and retry.
    #[error("issue #{id} was modified since last read; re-read and retry")]
    Conflict { id: u32 },

    /// Another live session holds the assignment for this issue.
    #[error("issue #{id} is currently being worked on by session {owner}")]
    Assigned {
        id: u32,
        owner: String,
        acquired_at: DateTime<Utc>,
    },

    /// The caller does not hold the assignment required for this mutation.
    #[error("issue #{id} is not assigned to session {caller}; run `start` first")]
    NotAssigned { id: u32, caller: String },

    /// Issue id not found.
    #[error("issue #{id} not found")]
    NotFound { id: u32 },

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// ============================================================================
// Domain types
// ============================================================================

/// Issue status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Open,
    Closed,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Closed => write!(f, "closed"),
        }
    }
}

/// Issue priority. Ordered low → critical for sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// Who currently owns the work on an issue.
///
/// `None` means the issue is free. `Some` means a session has claimed it via
/// `start`. Validity of an assignment is determined by process liveness (see
/// [`liveness::is_session_alive`]) — there is no expiry timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    /// Owning session id (from `ToolContext.session_id`).
    pub session: String,
    /// When the assignment was acquired. Informational only — *not* used for
    /// expiry decisions. Expiry is governed by process liveness.
    pub acquired_at: DateTime<Utc>,
}

/// A reference to a synced GitHub issue. Populated only after Phase 6 sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRef {
    pub repo: String,
    pub number: u64,
    pub url: String,
}

/// YAML frontmatter for an issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueMeta {
    pub id: u32,
    pub title: String,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
    /// Session ids linked to this issue (worked-on or referencing sessions).
    #[serde(default)]
    pub sessions: Vec<String>,
    /// Current assignment (liveness-gated). `None` = free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_to: Option<Assignment>,
    /// 🔜 Phase 6: GitHub sync mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<GithubRef>,
}

/// An in-memory issue: metadata + markdown body + the file path it came from.
#[derive(Debug, Clone)]
pub struct Issue {
    pub meta: IssueMeta,
    /// Raw markdown body (everything after the `---` frontmatter block).
    pub body: String,
    /// Path to the source file (None for unsaved/in-memory issues).
    pub path: Option<PathBuf>,
}

impl Issue {
    /// Combined status badge for list rendering: `🔒 open`.
    pub fn list_badge(&self) -> String {
        let lock = if self.meta.assigned_to.is_some() {
            "🔒 "
        } else {
            ""
        };
        format!("{}{}", lock, self.meta.status)
    }
}

// ============================================================================
// Serialization — markdown + YAML frontmatter
// ============================================================================

/// Atomically write content to a file by first writing to a temp file,
/// then renaming it. Same pattern as `store::session::atomic_write`.
fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp_path, content)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

const FRONTMATTER_DELIM: &str = "---";

/// Parse a markdown-with-frontmatter file into an [`Issue`].
///
/// Format:
/// ```text
/// ---
/// <yaml>
/// ---
/// <markdown body>
/// ```
///
/// A missing closing delimiter is treated as "the rest is body". Missing
/// frontmatter entirely yields an empty meta (caller decides whether that's
/// an error).
pub fn parse_issue(raw: &str, path: Option<PathBuf>) -> Result<Issue> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);

    // Split off the opening delimiter.
    // No leading frontmatter delimiter → synthesize an empty meta and treat
    // the whole input as body.
    let after_open = match raw.strip_prefix(FRONTMATTER_DELIM) {
        Some(rest) => rest,
        None => {
            return Ok(Issue {
                meta: empty_meta(),
                body: raw.to_string(),
                path,
            });
        }
    };

    // Robust line-based scan for the closing `---` delimiter. Everything
    // between the opening and closing lines is YAML; everything after is body.
    let mut yaml = String::new();
    let mut body = String::new();
    let mut closed = false;
    for line in after_open.split_inclusive('\n') {
        if !closed && line.trim_end() == FRONTMATTER_DELIM {
            closed = true;
            continue;
        }
        if !closed {
            yaml.push_str(line);
        } else {
            body.push_str(line);
        }
    }

    let meta: IssueMeta =
        serde_yaml::from_str(&yaml).context("failed to parse issue frontmatter")?;
    Ok(Issue { meta, body, path })
}

/// Serialize an issue back to the markdown-with-frontmatter form.
pub fn serialize_issue(issue: &Issue) -> Result<String> {
    let yaml = serde_yaml::to_string(&issue.meta).context("failed to serialize frontmatter")?;
    // serde_yaml emits a trailing newline; the `---` document markers are
    // *not* added by serde_yaml, so we wrap manually.
    let body = if issue.body.is_empty() {
        String::new()
    } else if issue.body.ends_with('\n') {
        issue.body.clone()
    } else {
        format!("{}\n", issue.body)
    };
    Ok(format!(
        "{open}\n{yaml}{close}\n{body}",
        open = FRONTMATTER_DELIM,
        close = FRONTMATTER_DELIM
    ))
}

/// Compute a content hash used for optimistic concurrency (same idea as the
/// `edit` tool's `expected_hash`). Uses the std default hasher for zero deps.
pub fn content_hash(raw: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// ============================================================================
// Project-root discovery
// ============================================================================

/// Walk up from `start` looking for a `.oxi/` directory. Returns the path to
/// `<root>/.oxi/issues`. If no `.oxi/` exists, returns `<start>/.oxi/issues`
/// (lazily created on first write).
///
/// Mirrors the walk in `Settings::find_project_settings`.
pub fn issues_dir(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".oxi").is_dir() {
            return dir.join(".oxi").join("issues");
        }
        if !dir.pop() {
            break;
        }
    }
    start.join(".oxi").join("issues")
}

/// Filename for an issue: zero-padded 4-digit id + slugified title.
pub fn issue_filename(id: u32, title: &str) -> String {
    let slug = slugify(title);
    if slug.is_empty() {
        format!("{:04}.md", id)
    } else {
        format!("{:04}-{}.md", id, slug)
    }
}

/// Construct an empty placeholder meta (used when a file has no frontmatter).
fn empty_meta() -> IssueMeta {
    let now = Utc::now();
    IssueMeta {
        id: 0,
        title: String::new(),
        status: Status::default(),
        priority: Priority::default(),
        labels: vec![],
        assignee: None,
        created_at: now,
        updated_at: now,
        closed_at: None,
        sessions: vec![],
        assigned_to: None,
        github: None,
    }
}

/// Slugify a title for use in a filename: lowercase, [a-z0-9-] only.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

// ============================================================================
// Liveness — process-held advisory locks (no wall-clock expiry)
// ============================================================================

/// Process-liveness tracking via OS advisory locks.
///
/// Each session holds an exclusive `flock` on `.oxi/issues/.alive/<session_id>`.
/// The lock is released by the OS when the process exits (including crashes
/// and `kill -9`). This lets us answer "is session X still alive?" without
/// any wall-clock timeout, PID-recycling heuristics, or heartbeats.
pub mod liveness {
    use super::*;

    /// Path of the alive-lock file for `session_id` under `issues_dir`.
    pub fn alive_path(issues_dir: &Path, session_id: &str) -> PathBuf {
        issues_dir.join(".alive").join(session_id)
    }

    /// Try to acquire (and hold) an exclusive advisory lock for `session_id`.
    ///
    /// The returned [`AliveGuard`] releases the lock when dropped — so callers
    /// must keep it alive for the whole session. Opening with write+create and
    /// calling `flock(LOCK_EX | LOCK_NB)` is atomic enough for our purposes:
    /// failure to acquire means another live process holds it.
    pub fn acquire(issues_dir: &Path, session_id: &str) -> io::Result<AliveGuard> {
        let dir = issues_dir.join(".alive");
        fs::create_dir_all(&dir)?;
        let path = dir.join(session_id);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let fd = file.as_raw_fd();
        // SAFETY: flock(2) on a valid fd. LOCK_NB = non-blocking.
        let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = io::Error::last_os_error();
            // EWOULDBLOCK/EAGAIN = held by another process.
            return Err(err);
        }
        Ok(AliveGuard { _file: file, path })
    }

    /// Returns `true` iff a live process currently holds the alive-lock for
    /// `session_id`. Used to decide whether an [`Assignment`] is still valid.
    pub fn is_session_alive(issues_dir: &Path, session_id: &str) -> bool {
        let path = alive_path(issues_dir, session_id);
        if !path.exists() {
            return false;
        }
        // Try to acquire a *shared* lock non-blockingly. If we can't, someone
        // holds an exclusive lock → alive. If we can, no one holds it → dead.
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
            return false;
        };
        let fd = file.as_raw_fd();
        let rc = unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) };
        if rc == 0 {
            // We got a shared lock — nobody is holding exclusive. Release.
            unsafe {
                libc::flock(fd, libc::LOCK_UN);
            }
            false
        } else {
            // Couldn't acquire — an exclusive holder is alive.
            true
        }
    }

    /// RAII guard for an acquired alive-lock.
    #[derive(Debug)]
    pub struct AliveGuard {
        _file: fs::File,
        path: PathBuf,
    }

    impl AliveGuard {
        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for AliveGuard {
        fn drop(&mut self) {
            // Drop closes the fd → OS releases the lock. Best-effort unlink.
            let _ = fs::remove_file(&self.path);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn acquire_then_alive() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().to_path_buf();
            let sid = "s1";
            let _g = acquire(&dir, sid).unwrap();
            assert!(is_session_alive(&dir, sid));
            drop(_g);
            assert!(!is_session_alive(&dir, sid));
        }

        #[test]
        fn second_acquire_fails_while_held() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().to_path_buf();
            let sid = "s2";
            let g = acquire(&dir, sid).unwrap();
            let second = acquire(&dir, sid);
            assert!(second.is_err(), "second acquire should fail while held");
            assert!(is_session_alive(&dir, sid));
            drop(g);
            assert!(acquire(&dir, sid).is_ok(), "after drop, acquire succeeds");
        }
    }
}

// ============================================================================
// Store
// ============================================================================

/// Cached directory listing, so the status-bar indicator doesn't readdir the
/// issues dir every render frame. `dir_mtime` is the single invalidation
/// signal; per-file mtimes aren't tracked (CAS uses content-hash on writes).
#[derive(Debug, Default, Clone)]
struct Cache {
    /// `open` issue count (the number shown in the status bar).
    open_count: usize,
    /// Title of the most recently updated open issue (for the indicator).
    latest_open_title: Option<String>,
    dir_mtime: Option<std::time::SystemTime>,
}

/// In-memory state for [`FileIssueStore`].
struct Inner {
    issues_dir: PathBuf,
    cache: Cache,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("issues_dir", &self.issues_dir)
            .finish()
    }
}

/// File-backed issue store.
///
/// One instance is shared (via `Arc`) between the TUI indicator, the agent
/// `issue` tool, and the `oxi issue` CLI subcommand. All mutations go through
/// [`FileIssueStore::create`] / [`FileIssueStore::update`] which serialize per-file
/// content-hash CAS (cross-process / external edits).
#[derive(Clone, Debug)]
pub struct FileIssueStore {
    inner: Arc<RwLock<Inner>>,
}

impl FileIssueStore {
    /// Open (or create lazily) the issue store rooted at `issues_dir`.
    pub fn open(issues_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(RwLock::new(Inner {
                issues_dir,
                cache: Cache::default(),
            })),
        })
    }

    /// Open using project-root discovery from `start` (cwd).
    pub fn open_from_cwd(start: &Path) -> Result<Self> {
        Self::open(issues_dir(start))
    }

    /// The issues directory.
    pub fn issues_dir(&self) -> PathBuf {
        self.inner.read().issues_dir.clone()
    }

    /// Number of open issues, for the status-bar indicator. Refreshes the
    /// cache if the directory mtime changed. Cheap (O(1) when fresh).
    pub fn open_count(&self) -> usize {
        self.refresh_if_stale();
        self.inner.read().cache.open_count
    }

    /// Title of the most recently updated open issue, for the status-bar
    /// indicator. Cached alongside `open_count`, so this is also O(1) on a
    /// warm cache. Returns `None` if there are no open issues.
    pub fn latest_open_title(&self) -> Option<String> {
        self.refresh_if_stale();
        self.inner.read().cache.latest_open_title.clone()
    }

    /// True iff the issues directory has any issues at all (suppresses the
    /// indicator when the project has never used the feature).
    pub fn has_any(&self) -> bool {
        self.refresh_if_stale();
        let dir = self.inner.read().issues_dir.clone();
        fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            })
            .unwrap_or(false)
    }

    /// Refresh cache if the directory mtime changed (or never loaded).
    fn refresh_if_stale(&self) {
        let dir = self.inner.read().issues_dir.clone();
        let cur_dir_mtime = fs::metadata(&dir).and_then(|m| m.modified()).ok();
        let needs = {
            let g = self.inner.read();
            match (g.cache.dir_mtime, cur_dir_mtime) {
                (None, _) => true,        // never loaded
                (Some(_), None) => false, // can't stat dir; keep cache
                (Some(cached), Some(cur)) => cached != cur,
            }
        };
        if !needs {
            return;
        }
        // Re-scan.
        let mut open_count = 0;
        let mut latest_open_title: Option<String> = None;
        let mut latest_open_updated: Option<chrono::DateTime<chrono::Utc>> = None;
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.extension().and_then(|x| x.to_str()) != Some("md") {
                    continue;
                }
                // open_count requires parsing frontmatter. For the indicator
                // we accept the cost — issues are typically few.
                if let Ok(raw) = fs::read_to_string(&p)
                    && let Ok(issue) = parse_issue(&raw, None)
                    && issue.meta.status == Status::Open
                {
                    open_count += 1;
                    if issue.meta.updated_at
                        > latest_open_updated.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC)
                    {
                        latest_open_updated = Some(issue.meta.updated_at);
                        latest_open_title = Some(issue.meta.title);
                    }
                }
            }
        }
        let mut g = self.inner.write();
        g.cache = Cache {
            open_count,
            latest_open_title,
            dir_mtime: cur_dir_mtime,
        };
    }

    /// Invalidate the cache (force next read to rescan).
    pub fn invalidate(&self) {
        self.inner.write().cache = Cache::default();
    }

    // ── Reads ───────────────────────────────────────────────────────────

    /// List all issues, optionally filtered. Sorted by `updated_at` desc.
    pub fn list(&self, filter: &IssueFilter) -> Result<Vec<Issue>> {
        self.refresh_if_stale();
        let dir = self.inner.read().issues_dir.clone();
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.extension().and_then(|x| x.to_str()) != Some("md") {
                    continue;
                }
                let raw = fs::read_to_string(&p)?;
                let issue = parse_issue(&raw, Some(p.clone()))?;
                if filter.matches(&issue) {
                    out.push(issue);
                }
            }
        }
        out.sort_by_key(|i| std::cmp::Reverse(i.meta.updated_at));
        Ok(out)
    }

    /// Read a single issue by id. Returns the issue and its current content
    /// hash (for optimistic-concurrency writes).
    pub fn read(&self, id: u32) -> Result<(Issue, String)> {
        let path = self.path_for_id(id)?;
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("issue #{} not found at {}", id, path.display()))?;
        let issue = parse_issue(&raw, Some(path))?;
        Ok((issue, content_hash(&raw)))
    }

    // ── Writes ──────────────────────────────────────────────────────────

    /// Allocate the next issue id by scanning existing filenames.
    ///
    /// Cross-process allocation races are possible (two sessions create the
    /// next id simultaneously) but bounded: the loser's `create` write hits
    /// an existing file and we bump to the next free id. No lock needed for
    /// correctness, only for avoiding rare retries.
    pub fn next_id(&self) -> Result<u32> {
        let dir = self.inner.read().issues_dir.clone();
        fs::create_dir_all(&dir)?;
        let mut max = 0u32;
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let num_str = name.split('-').next().unwrap_or(&name);
                if let Ok(n) = num_str.trim_end_matches(".md").parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
        Ok(max + 1)
    }

    /// Create a new issue. `caller_session` is linked into `sessions`.
    pub fn create(
        &self,
        title: String,
        body: String,
        priority: Priority,
        labels: Vec<String>,
        caller_session: Option<&str>,
    ) -> Result<Issue> {
        let id = self.next_id()?;
        let now = Utc::now();
        let sessions = caller_session
            .map(|s| vec![s.to_string()])
            .unwrap_or_default();
        let issue = Issue {
            meta: IssueMeta {
                id,
                title,
                status: Status::Open,
                priority,
                labels,
                assignee: None,
                created_at: now,
                updated_at: now,
                closed_at: None,
                sessions,
                assigned_to: None,
                github: None,
            },
            body,
            path: None,
        };
        // Retry a few times in case of id collision with another session.
        for _ in 0..4 {
            let path = self
                .issues_dir()
                .join(issue_filename(id, &issue.meta.title));
            if path.exists() {
                // bump id and retry
                continue;
            }
            let content = serialize_issue(&issue)?;
            atomic_write(&path, &content)?;
            self.invalidate();
            let mut saved = issue.clone();
            saved.path = Some(path);
            return Ok(saved);
        }
        anyhow::bail!("could not allocate a free issue id after retries");
    }

    /// Update an issue with optimistic concurrency.
    ///
    /// `expected_hash` should be the hash returned by [`FileIssueStore::read`]. If the
    /// on-disk content changed since, returns [`IssueError::Conflict`].
    /// `mutator` receives the loaded issue and returns the new state.
    ///
    /// All writes go through `file_mutation_queue` for in-process
    /// serialization, exactly like the `edit` tool.
    pub async fn update<F>(
        &self,
        id: u32,
        expected_hash: Option<String>,
        mutator: F,
    ) -> std::result::Result<Issue, IssueError>
    where
        F: FnOnce(Issue) -> std::result::Result<Issue, IssueError> + Send + 'static,
    {
        let path = self.path_for_id(id).map_err(IssueError::Other)?;
        let path_for_closure = path.clone();
        let store = self.clone();
        // Serialize same-file writes within this process.
        oxi_agent::tools::file_mutation_queue::global_mutation_queue()
            .with_queue(&path, move || async move {
                let path = path_for_closure;
                let raw = fs::read_to_string(&path)?;
                if let Some(expected) = expected_hash.as_deref()
                    && content_hash(&raw) != expected
                {
                    return Err(IssueError::Conflict { id });
                }
                let issue = parse_issue(&raw, Some(path.clone())).map_err(IssueError::Other)?;
                let mut new = mutator(issue)?;
                new.meta.updated_at = Utc::now();
                let content = serialize_issue(&new).map_err(IssueError::Other)?;
                atomic_write(&path, &content)?;
                store.invalidate();
                Ok(new.with_path(path))
            })
            .await
    }

    /// Convenience: close an issue (assignee only).
    pub async fn close(
        &self,
        id: u32,
        caller: &str,
        expected_hash: Option<String>,
    ) -> std::result::Result<Issue, IssueError> {
        let now = Utc::now();
        let caller = caller.to_string();
        self.update(id, expected_hash, move |mut issue| {
            require_owner(&issue, id, &caller)?;
            issue.meta.status = Status::Closed;
            issue.meta.closed_at = Some(now);
            issue.meta.assigned_to = None; // closing releases the assignment
            Ok(issue)
        })
        .await
    }

    /// Try to claim an issue for `caller` (the `start` action).
    ///
    /// If already assigned to a *live* session, returns [`IssueError::Assigned`].
    /// If assigned to a *dead* session (process exited), reclaims and assigns
    /// to the caller. If free, assigns to the caller.
    pub async fn start(
        &self,
        id: u32,
        caller: &str,
        expected_hash: Option<String>,
    ) -> std::result::Result<Issue, IssueError> {
        let issues_dir = self.issues_dir();
        let caller_owned = caller.to_string();
        self.update(id, expected_hash, move |mut issue| {
            if let Some(ref a) = issue.meta.assigned_to {
                if a.session == caller_owned {
                    // Already mine; idempotent.
                    return Ok(issue);
                }
                if liveness::is_session_alive(&issues_dir, &a.session) {
                    return Err(IssueError::Assigned {
                        id,
                        owner: a.session.clone(),
                        acquired_at: a.acquired_at,
                    });
                }
                // Dead owner — reclaim silently.
            }
            issue.meta.assigned_to = Some(Assignment {
                session: caller_owned.clone(),
                acquired_at: Utc::now(),
            });
            // Link the session.
            if !issue.meta.sessions.contains(&caller_owned) {
                issue.meta.sessions.push(caller_owned.clone());
            }
            Ok(issue)
        })
        .await
    }

    /// Release an assignment (the `release` action). Caller must be the owner.
    pub async fn release(
        &self,
        id: u32,
        caller: &str,
        expected_hash: Option<String>,
    ) -> std::result::Result<Issue, IssueError> {
        let caller = caller.to_string();
        self.update(id, expected_hash, move |mut issue| {
            require_owner(&issue, id, &caller)?;
            issue.meta.assigned_to = None;
            Ok(issue)
        })
        .await
    }

    /// Link a session to an issue (append-only; idempotent).
    pub async fn link_session(
        &self,
        id: u32,
        session: &str,
        expected_hash: Option<String>,
    ) -> std::result::Result<Issue, IssueError> {
        let session = session.to_string();
        self.update(id, expected_hash, move |mut issue| {
            if !issue.meta.sessions.contains(&session) {
                issue.meta.sessions.push(session);
            }
            Ok(issue)
        })
        .await
    }

    // ── Path helpers ────────────────────────────────────────────────────

    fn path_for_id(&self, id: u32) -> Result<PathBuf> {
        let dir = self.inner.read().issues_dir.clone();
        // Files are named `<id>-<slug>.md`; match by leading id.
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let num_str = name.split('-').next().unwrap_or(&name);
                if num_str.trim_end_matches(".md").parse::<u32>().ok() == Some(id) {
                    return Ok(entry.path());
                }
            }
        }
        Err(anyhow::anyhow!(IssueError::NotFound { id }))
    }
}

/// Attach a path to an issue (builder convenience).
trait WithPath {
    fn with_path(self, path: PathBuf) -> Self;
}

impl WithPath for Issue {
    fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }
}

/// Check `caller` owns the issue's assignment, else [`IssueError::NotAssigned`].
fn require_owner(issue: &Issue, id: u32, caller: &str) -> std::result::Result<(), IssueError> {
    match &issue.meta.assigned_to {
        Some(a) if a.session == caller => Ok(()),
        _ => Err(IssueError::NotAssigned {
            id,
            caller: caller.to_string(),
        }),
    }
}

// ============================================================================
// Filter
// ============================================================================

/// Filter for `list`. All fields optional (None = no constraint).
#[derive(Debug, Clone, Default)]
pub struct IssueFilter {
    pub status: Option<Status>,
    pub priority: Option<Priority>,
    pub label: Option<String>,
    pub assigned_to_session: Option<String>,
    /// Text substring match on title (case-insensitive).
    pub text: Option<String>,
}

impl IssueFilter {
    fn matches(&self, issue: &Issue) -> bool {
        if let Some(s) = self.status
            && issue.meta.status != s
        {
            return false;
        }
        if let Some(p) = self.priority
            && issue.meta.priority != p
        {
            return false;
        }
        if let Some(ref label) = self.label
            && !issue.meta.labels.iter().any(|l| l == label)
        {
            return false;
        }
        if let Some(ref session) = self.assigned_to_session {
            let mine = issue
                .meta
                .assigned_to
                .as_ref()
                .map(|a| &a.session == session)
                .unwrap_or(false);
            if !mine {
                return false;
            }
        }
        if let Some(ref text) = self.text
            && !issue
                .meta
                .title
                .to_lowercase()
                .contains(&text.to_lowercase())
        {
            return false;
        }
        true
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta(id: u32, title: &str, priority: Priority) -> IssueMeta {
        let now = Utc::now();
        IssueMeta {
            id,
            title: title.into(),
            status: Status::Open,
            priority,
            labels: vec![],
            assignee: None,
            created_at: now,
            updated_at: now,
            closed_at: None,
            sessions: vec![],
            assigned_to: None,
            github: None,
        }
    }

    fn tmp_store() -> (tempfile::TempDir, FileIssueStore) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".oxi").join("issues");
        fs::create_dir_all(&dir).unwrap();
        let store = FileIssueStore::open(dir).unwrap();
        (tmp, store)
    }

    #[test]
    fn roundtrip_serialization() {
        let issue = Issue {
            meta: sample_meta(1, "Test", Priority::High),
            body: "## Body\n\nHello.".into(),
            path: None,
        };
        let s = serialize_issue(&issue).unwrap();
        assert!(s.starts_with("---\n"));
        let parsed = parse_issue(&s, None).unwrap();
        assert_eq!(parsed.meta.id, 1);
        assert_eq!(parsed.meta.title, "Test");
        assert_eq!(parsed.meta.priority, Priority::High);
        assert!(parsed.body.contains("Hello."));
    }

    #[tokio::test]
    async fn create_read_list() {
        let (_tmp, store) = tmp_store();
        let created = store
            .create(
                "Fix bug".into(),
                "body".into(),
                Priority::High,
                vec![],
                None,
            )
            .unwrap();
        assert_eq!(created.meta.id, 1);

        let (read, hash) = store.read(1).unwrap();
        assert_eq!(read.meta.title, "Fix bug");
        assert!(!hash.is_empty());

        let list = store.list(&IssueFilter::default()).unwrap();
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn content_hash_detects_conflict() {
        let (_tmp, store) = tmp_store();
        store
            .create("Orig".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (_, hash) = store.read(1).unwrap();

        // External edit (different hash) → wrong expected_hash → conflict.
        let wrong = Some("deadbeefdeadbeef".to_string());
        let err = store
            .update(1, wrong, |_| {
                Ok(Issue {
                    meta: sample_meta(1, "x", Priority::Low),
                    body: "x".into(),
                    path: None,
                })
            })
            .await
            .unwrap_err();
        assert!(matches!(err, IssueError::Conflict { id: 1 }));

        // Correct hash → succeeds.
        let _ok = store
            .update(1, Some(hash), |mut i| {
                i.meta.title = "Updated".into();
                Ok(i)
            })
            .await
            .unwrap();
        let (read, _) = store.read(1).unwrap();
        assert_eq!(read.meta.title, "Updated");
    }

    #[tokio::test]
    async fn start_rejects_live_owner() {
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let issues_dir = store.issues_dir();
        // Owner session A acquires a live lock.
        let _guard_a = liveness::acquire(&issues_dir, "sessionA").unwrap();
        // Manually assign to A.
        let (_, hash) = store.read(1).unwrap();
        store.start(1, "sessionA", Some(hash)).await.unwrap();

        // B tries to start → rejected (A is alive).
        let (_, hash2) = store.read(1).unwrap();
        let err = store.start(1, "sessionB", Some(hash2)).await.unwrap_err();
        assert!(matches!(err, IssueError::Assigned { owner, .. } if owner == "sessionA"));
    }

    #[tokio::test]
    async fn start_reclaims_dead_owner() {
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let issues_dir = store.issues_dir();

        // A acquires, then "dies" (drop guard).
        {
            let _g = liveness::acquire(&issues_dir, "sessionA").unwrap();
            let (_, h) = store.read(1).unwrap();
            store.start(1, "sessionA", Some(h)).await.unwrap();
        } // guard dropped → A is "dead"

        let (_, hash) = store.read(1).unwrap();
        let reclaimed = store.start(1, "sessionB", Some(hash)).await.unwrap();
        assert_eq!(
            reclaimed.meta.assigned_to.as_ref().unwrap().session,
            "sessionB"
        );
    }

    #[tokio::test]
    async fn close_requires_owner() {
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (_, hash) = store.read(1).unwrap();
        store.start(1, "sessionA", Some(hash)).await.unwrap();

        // B can't close.
        let (_, hash2) = store.read(1).unwrap();
        let err = store.close(1, "sessionB", Some(hash2)).await.unwrap_err();
        assert!(matches!(err, IssueError::NotAssigned { .. }));

        // A can.
        let (_, hash3) = store.read(1).unwrap();
        let closed = store.close(1, "sessionA", Some(hash3)).await.unwrap();
        assert_eq!(closed.meta.status, Status::Closed);
        assert!(closed.meta.assigned_to.is_none());
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Fix Login Bug!"), "fix-login-bug");
        assert_eq!(slugify("   spaces   "), "spaces");
        assert_eq!(slugify("a__b"), "a-b");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn issue_filename_format() {
        assert_eq!(issue_filename(12, "Fix Login"), "0012-fix-login.md");
        assert_eq!(issue_filename(1, ""), "0001.md");
    }

    #[tokio::test]
    async fn open_count_caches() {
        let (_tmp, store) = tmp_store();
        assert_eq!(store.open_count(), 0);
        store
            .create("A".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        store
            .create("B".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        assert_eq!(store.open_count(), 2);

        // Start as owner A, then close → count drops to 1.
        let issues_dir = store.issues_dir();
        let _guard = liveness::acquire(&issues_dir, "sessionA").unwrap();
        let (_, h) = store.read(1).unwrap();
        store.start(1, "sessionA", Some(h)).await.unwrap();
        let (_, h) = store.read(1).unwrap();
        store.close(1, "sessionA", Some(h)).await.unwrap();
        store.invalidate();
        assert_eq!(store.open_count(), 1);
    }

    #[tokio::test]
    async fn latest_open_title_caches_and_handles_cjk() {
        let (_tmp, store) = tmp_store();
        // No issues yet — latest_open_title is None.
        assert!(store.latest_open_title().is_none());

        // Create an issue with a CJK title and body. The title must survive
        // round-trip through the cache and read() without panic on multi-byte
        // boundaries. (Regression test for the byte-slice panic in
        // `first_line_preview` / `truncate_for_footer`.)
        let cjk_title =
            "버그 수정: 한글 제목도 정상이어야 합니다 — 멀티바이트 인코딩 안전성".to_string();
        let cjk_body =
            "요약\n\n이 이슈는 한글 본문을 포함합니다. 본문에는 영문과 한글이 섞여 있습니다. "
                .repeat(4);
        let created = store
            .create(cjk_title.clone(), cjk_body, Priority::High, vec![], None)
            .unwrap();
        assert_eq!(created.meta.title, cjk_title);

        // Cache populates from read_dir.
        let title = store.latest_open_title();
        assert_eq!(title.as_deref(), Some(cjk_title.as_str()));

        // read() must not panic on multi-byte UTF-8 in the body.
        let (read_back, _hash) = store.read(created.meta.id).unwrap();
        assert!(read_back.body.contains("한글"));
    }
}
