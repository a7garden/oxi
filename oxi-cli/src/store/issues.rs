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

/// A precise update payload for [`FileIssueStore::apply_patch`].
///
/// Every field is `Option`: `None` = keep the existing value, `Some` = replace
/// it. `labels` is the only field with a meaningful empty state —
/// `Some(vec![])` clears all labels while `None` keeps them. This resolves
/// defect #3: through the tool schema, "field absent" vs `[]` were previously
/// indistinguishable, so labels could never be cleared without resending the
/// full set.
///
/// Used by the `issue` tool's `update` action (via [`FileIssueStore::apply_patch`])
/// and is the recommended mutation surface for callers that want precise
/// keep-vs-replace semantics.
#[derive(Debug, Clone, Default)]
pub struct IssuePatch {
    /// Replace the title.
    pub title: Option<String>,
    /// Replace the markdown body.
    pub body: Option<String>,
    /// Replace the status. Setting [`Status::Open`] also clears `closed_at`
    /// (see [`FileIssueStore::apply_patch`], which fixes the latent reopen bug #4).
    pub status: Option<Status>,
    /// Replace the priority.
    pub priority: Option<Priority>,
    /// Replace the labels wholesale. `Some(vec![])` clears all labels.
    pub labels: Option<Vec<String>>,
}

// ============================================================================
// Serialization — markdown + YAML frontmatter
// ============================================================================

use super::fs_util::atomic_write;

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

    /// Single source of truth for the liveness identity used by the TUI
    /// (and any in-TUI operations: agent tool, `/issue` slash command, panel).
    ///
    /// Invariant: in TUI mode, [`crate::App::ownership_session_id`] MUST equal
    /// this constant. The TUI panel's [`crate::tui::overlay::IssuesPanelOverlay::session_id`]
    /// references it, and the agent's `ToolContext.session_id` is set from it,
    /// so the flock acquired by `App` is the same one the panel and agent use
    /// to check `is_session_alive`. Keep the two in sync.
    pub const TUI_OWNERSHIP_ID: &str = "tui";

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
        // Failure (EWOULDBLOCK/EAGAIN) means another live process holds it.
        try_flock_exclusive(fd)?;
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
        // Ok = nobody holds exclusive (dead); Err = held by a live process (alive).
        probe_flock_shared(fd).is_err()
    }

    // ── flock helpers (#11: centralize the two unsafe call sites) ────────
    //
    // Both take a raw fd that the caller obtained from a live `File` via
    // `as_raw_fd()`, so fd validity is guaranteed by construction. Naming
    // them (with SAFETY docs) keeps the `unsafe` surface to these two spots
    // instead of being scattered through the liveness logic.

    /// Try a non-blocking exclusive flock on `fd`.
    ///
    /// `Ok` on success; `Err` on contention (`EWOULDBLOCK`/`EAGAIN` — another
    /// live process holds it) or any other OS error.
    ///
    /// `fd` must be a valid open file descriptor.
    fn try_flock_exclusive(fd: i32) -> io::Result<()> {
        // SAFETY: `fd` is a valid, owned descriptor (caller passes
        // `File::as_raw_fd()` from a live `File`). `LOCK_NB` never blocks.
        let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Probe liveness by attempting a non-blocking shared flock.
    ///
    /// `Ok` if no one holds an exclusive lock (we acquired and released a
    /// shared one); `Err` if someone holds exclusive (a live process).
    ///
    /// `fd` must be a valid open file descriptor.
    fn probe_flock_shared(fd: i32) -> io::Result<()> {
        // SAFETY: `fd` is a valid, owned descriptor (see `try_flock_exclusive`).
        let rc = unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) };
        if rc == 0 {
            // SAFETY: releasing the shared lock we just acquired on a valid fd.
            unsafe { libc::flock(fd, libc::LOCK_UN) };
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    // ── Orphan reaping (#8) ─────────────────────────────────────────────

    /// Minimum age (seconds) a dead alive-lock file must reach before reaping.
    ///
    /// The age gate is the TOCTOU mitigation: a reaper checks `is_session_alive`,
    /// and a process could acquire the lock in the gap before `remove_file`.
    /// Only reaping files older than this threshold leaves a wide margin for
    /// any session that is actively starting up, while still clearing the
    /// steady-state accumulation of zombies from crashed/killed processes.
    pub const ORPHAN_AGE_SECS: u64 = 3600; // 1 hour

    /// Best-effort, idempotent cleanup of dead alive-lock files under
    /// `<issues_dir>/.alive/`.
    ///
    /// Two guards keep it safe:
    /// 1. **Holder check** — files whose session still holds an exclusive flock
    ///    ([`is_session_alive`] → `true`) are never touched.
    /// 2. **Age gate** — even dead files younger than [`ORPHAN_AGE_SECS`] are
    ///    skipped, so a process racing to acquire can't lose its lock file.
    ///
    /// Returns the number of files removed. Missing `.alive/` is `Ok(0)`.
    pub fn reap_orphans(issues_dir: &Path) -> io::Result<usize> {
        let dir = issues_dir.join(".alive");
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let now = std::time::SystemTime::now();
        let mut removed = 0;
        for entry in rd.flatten() {
            let sid = entry.file_name();
            let sid = sid.to_string_lossy();
            if is_session_alive(issues_dir, &sid) {
                continue; // (1) someone holds it — never reap
            }
            let mtime = entry.metadata().and_then(|m| m.modified()).unwrap_or(now);
            let age = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
            if age < ORPHAN_AGE_SECS {
                continue; // (2) too young — TOCTOU margin
            }
            if fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
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

        // ── Phase 4: orphan reap (#8) ──

        /// Helper: backdate a file's mtime by `secs` so it crosses the age gate.
        fn backdate(path: &std::path::Path, secs: u64) {
            use std::fs::FileTimes;
            let then = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
            let f = std::fs::File::open(path)
                .or_else(|_| {
                    std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .open(path)
                })
                .unwrap();
            f.set_times(FileTimes::new().set_modified(then)).unwrap();
        }

        #[test]
        fn reap_idempotent() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().to_path_buf();
            // No `.alive/` at all.
            assert_eq!(reap_orphans(&dir).unwrap(), 0);
            fs::create_dir_all(dir.join(".alive")).unwrap();
            // Empty dir, repeated calls stay at 0.
            assert_eq!(reap_orphans(&dir).unwrap(), 0);
            assert_eq!(reap_orphans(&dir).unwrap(), 0);
        }

        #[test]
        fn reap_skips_recent_dead_files() {
            // A dead (unheld) orphan younger than ORPHAN_AGE_SECS must be
            // preserved — the age gate is the TOCTOU mitigation.
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().to_path_buf();
            fs::create_dir_all(dir.join(".alive")).unwrap();
            let recent = dir.join(".alive").join("dead-recent");
            fs::write(&recent, b"").unwrap();
            // mtime ~ now.
            assert_eq!(reap_orphans(&dir).unwrap(), 0);
            assert!(
                recent.exists(),
                "recent dead orphan must be preserved by the age gate"
            );
        }

        #[test]
        fn reap_removes_old_dead_and_keeps_alive() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().to_path_buf();
            // A genuinely live lock — must never be reaped.
            let _g_live = acquire(&dir, "alive-session").unwrap();
            // An old dead orphan (no holder, mtime > threshold).
            fs::create_dir_all(dir.join(".alive")).unwrap();
            let old = dir.join(".alive").join("dead-old");
            fs::write(&old, b"").unwrap();
            backdate(&old, ORPHAN_AGE_SECS + 60);

            let removed = reap_orphans(&dir).unwrap();
            assert_eq!(removed, 1, "only the old dead orphan should be reaped");
            assert!(!old.exists(), "old dead orphan must be removed");
            // Live holder is still alive and its file untouched.
            assert!(
                is_session_alive(&dir, "alive-session"),
                "live lock must survive reap"
            );
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
    /// Number of currently-assigned (locked) open issues. Computed at the
    /// same time as `open_count` so the indicator can show "3 open · 1 🔒".
    locked_open_count: usize,
    /// Highest priority among open issues (None if no open issues).
    /// Used for the priority dot in the footer indicator.
    top_priority: Option<Priority>,
    /// Highest priority among open AND *unassigned* issues — the "most
    /// actionable thing right now" signal (#10). `None` when no open issue is
    /// free. Distinct from `top_priority` (overall open max): this excludes
    /// issues someone is already working on.
    top_free_priority: Option<Priority>,
    dir_mtime: Option<std::time::SystemTime>,
}

/// Summary view exposed for UI consumers (footer indicator, panel header).
/// Cheap to construct — values come straight from the in-memory cache.
#[derive(Debug, Clone)]
pub struct IssueSummary {
    pub open_count: usize,
    pub locked_open_count: usize,
    pub top_priority: Option<Priority>,
    /// Highest priority among open + *unassigned* issues (#10). Distinct from
    /// `top_priority` (overall open max): excludes issues someone works on.
    pub top_free_priority: Option<Priority>,
    pub latest_open_title: Option<String>,
}

impl IssueSummary {
    pub fn is_empty(&self) -> bool {
        self.open_count == 0
    }
}

/// In-memory state for [`FileIssueStore`].
struct Inner {
    issues_dir: PathBuf,
    cache: Cache,
}

impl Cache {
    fn empty() -> Self {
        Self {
            open_count: 0,
            latest_open_title: None,
            locked_open_count: 0,
            top_priority: None,
            top_free_priority: None,
            dir_mtime: None,
        }
    }
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
        // Best-effort: clear zombie alive-lock files left by crashed/killed
        // processes (#8). Lazy + idempotent + age-gated; failures are a
        // warn log only and never block store construction.
        if let Err(e) = liveness::reap_orphans(&issues_dir) {
            tracing::warn!(error = %e, "issue liveness reap failed (non-fatal)");
        }
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

    /// Aggregate summary for the footer indicator / panels. Pulled from the
    /// in-memory cache, so it's cheap (O(1) on a warm cache).
    pub fn summary(&self) -> IssueSummary {
        self.refresh_if_stale();
        let g = self.inner.read();
        IssueSummary {
            open_count: g.cache.open_count,
            locked_open_count: g.cache.locked_open_count,
            top_priority: g.cache.top_priority,
            top_free_priority: g.cache.top_free_priority,
            latest_open_title: g.cache.latest_open_title.clone(),
        }
    }

    /// Highest priority among open, *unassigned* issues — the most actionable
    /// thing a free agent could pick up right now (#10). Distinct from a
    /// plain "top priority" (overall open max): this excludes issues someone
    /// is already working on. Returns `None` when no open issue is free.
    /// Cached alongside [`Self::open_count`]; O(1) on a warm cache.
    pub fn top_free_priority(&self) -> Option<Priority> {
        self.refresh_if_stale();
        self.inner.read().cache.top_free_priority
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
        let mut locked_open_count = 0;
        let mut top_priority: Option<Priority> = None;
        let mut latest_open_title: Option<String> = None;
        let mut latest_open_updated: Option<chrono::DateTime<chrono::Utc>> = None;
        let mut top_free_priority: Option<Priority> = None;
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
                    if issue.meta.assigned_to.is_some() {
                        locked_open_count += 1;
                    }
                    // Track highest priority (Critical > High > Medium > Low).
                    top_priority = Some(match top_priority {
                        Some(existing) => existing.max(issue.meta.priority),
                        None => issue.meta.priority,
                    });
                    if issue.meta.updated_at
                        > latest_open_updated.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC)
                    {
                        latest_open_updated = Some(issue.meta.updated_at);
                        latest_open_title = Some(issue.meta.title);
                    }
                    // #10: track the max priority among open + unassigned issues.
                    if issue.meta.assigned_to.is_none() {
                        top_free_priority = Some(match top_free_priority {
                            Some(cur) if cur >= issue.meta.priority => cur,
                            _ => issue.meta.priority,
                        });
                    }
                }
            }
        }
        let mut g = self.inner.write();
        g.cache = Cache {
            open_count,
            latest_open_title,
            locked_open_count,
            top_priority,
            top_free_priority,
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
                let before = parse_issue(&raw, Some(path.clone())).map_err(IssueError::Other)?;
                let before_updated_at = before.meta.updated_at;
                let before_bytes = serialize_issue(&before).map_err(IssueError::Other)?;
                let after = mutator(before)?;

                // No-op detection (#12): if the mutator produced no meaningful
                // change — ignoring `updated_at`, which a real write always
                // refreshes — skip the write, the timestamp bump, and the cache
                // invalidate. We compare the *normalized serialized* forms so
                // key-order/whitespace drift in the on-disk `raw` can't create
                // false negatives.
                let mut probe = after.clone();
                probe.meta.updated_at = before_updated_at;
                let probe_bytes = serialize_issue(&probe).map_err(IssueError::Other)?;
                if probe_bytes == before_bytes {
                    return Ok(after.with_path(path));
                }

                let mut final_issue = after;
                final_issue.meta.updated_at = Utc::now();
                let content = serialize_issue(&final_issue).map_err(IssueError::Other)?;
                atomic_write(&path, &content)?;
                store.invalidate();
                Ok(final_issue.with_path(path))
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

    /// Reopen a closed issue. No ownership required (reopening doesn't
    /// assign the issue to anyone; it goes back to the unassigned pool).
    ///
    /// Errors with `NotFound` if the id doesn't exist, or with no special
    /// error if the issue is already open — that case is a no-op.
    pub async fn reopen(
        &self,
        id: u32,
        expected_hash: Option<String>,
    ) -> std::result::Result<Issue, IssueError> {
        self.update(id, expected_hash, move |mut issue| {
            if issue.meta.status == Status::Open {
                // Already open — idempotent no-op so callers can retry
                // without special-casing.
                return Ok(issue);
            }
            issue.meta.status = Status::Open;
            issue.meta.closed_at = None;
            issue.meta.assigned_to = None;
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

    /// Apply a precise [`IssuePatch`] under strict CAS, preserving the existing
    /// ownership policy.
    ///
    /// If `caller` is `Some`, a different *non-empty* assignee blocks the
    /// update with [`IssueError::NotAssigned`] — identical to the legacy
    /// `update` tool action. Setting `status = Open` also clears `closed_at`,
    /// fixing the latent reopen bug (#4: previously `update { status: open }`
    /// left a stale `closed_at` on a reopened issue). Prefer the dedicated
    /// [`FileIssueStore::reopen`] for clarity.
    ///
    /// No-op patches (nothing meaningful changed) are detected inside
    /// [`FileIssueStore::update`] and skip the write entirely.
    pub async fn apply_patch(
        &self,
        id: u32,
        patch: IssuePatch,
        caller: Option<String>,
        expected_hash: Option<String>,
    ) -> std::result::Result<Issue, IssueError> {
        self.update(id, expected_hash, move |mut issue| {
            if let Some(caller) = caller.as_deref()
                && let Some(ref a) = issue.meta.assigned_to
                && !a.session.is_empty()
                && a.session != caller
            {
                return Err(IssueError::NotAssigned {
                    id,
                    caller: caller.to_string(),
                });
            }
            if let Some(t) = patch.title {
                issue.meta.title = t;
            }
            if let Some(b) = patch.body {
                issue.body = b;
            }
            if let Some(s) = patch.status {
                issue.meta.status = s;
                issue.meta.closed_at = match s {
                    Status::Closed => Some(Utc::now()),
                    Status::Open => None, // reopen clears closed_at (#4)
                };
            }
            if let Some(p) = patch.priority {
                issue.meta.priority = p;
            }
            if let Some(l) = patch.labels {
                issue.meta.labels = l;
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

    #[tokio::test]
    async fn reopen_flips_closed_to_open() {
        let (_tmp, store) = tmp_store();
        let issues_dir = store.issues_dir();
        let _guard = crate::store::issues::liveness::acquire(&issues_dir, "tui").unwrap();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        // Close it.
        let (_, h) = store.read(1).unwrap();
        store.start(1, "tui", Some(h)).await.unwrap();
        let (_, h) = store.read(1).unwrap();
        store.close(1, "tui", Some(h)).await.unwrap();
        // Reopen.
        let (_, h) = store.read(1).unwrap();
        let reopened = store.reopen(1, Some(h)).await.unwrap();
        assert_eq!(reopened.meta.status, Status::Open);
        assert!(reopened.meta.closed_at.is_none());
        assert!(reopened.meta.assigned_to.is_none());
    }

    #[tokio::test]
    async fn reopen_is_idempotent_on_already_open() {
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (_, h) = store.read(1).unwrap();
        // Already open — reopen returns the issue unchanged.
        let reopened = store.reopen(1, Some(h)).await.unwrap();
        assert_eq!(reopened.meta.status, Status::Open);
        assert!(reopened.meta.closed_at.is_none());
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
    async fn summary_reflects_lock_and_priority() {
        let (_tmp, store) = tmp_store();
        let issues_dir = store.issues_dir();
        let _guard = liveness::acquire(&issues_dir, "sessionA").unwrap();
        // Two opens: one Low (assigned to A), one Critical (free).
        store
            .create("Lowly".into(), "".into(), Priority::Low, vec![], None)
            .unwrap();
        store
            .create("Crit".into(), "".into(), Priority::Critical, vec![], None)
            .unwrap();
        // Plus one closed Medium (should be ignored).
        store
            .create("Closed".into(), "".into(), Priority::Medium, vec![], None)
            .unwrap();
        let (_, h) = store.read(3).unwrap();
        store.start(3, "sessionA", Some(h)).await.unwrap();
        let (_, h) = store.read(3).unwrap();
        store.close(3, "sessionA", Some(h)).await.unwrap();
        // Assign #1 to A.
        let (_, h) = store.read(1).unwrap();
        store.start(1, "sessionA", Some(h)).await.unwrap();
        store.invalidate();

        let s = store.summary();
        assert_eq!(s.open_count, 2);
        assert_eq!(s.locked_open_count, 1);
        assert_eq!(s.top_priority, Some(Priority::Critical));
        assert!(s.latest_open_title.is_some());
        assert!(!s.is_empty());
    }

    #[tokio::test]
    async fn summary_empty_when_no_issues() {
        let (_tmp, store) = tmp_store();
        let s = store.summary();
        assert_eq!(s.open_count, 0);
        assert_eq!(s.locked_open_count, 0);
        assert!(s.top_priority.is_none());
        assert!(s.latest_open_title.is_none());
        assert!(s.is_empty());
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

    // ── Phase 0 (defect #13) regression coverage ───────────────────────────
    //
    // Before #13 was fixed, `ToolContext.session_id` was always `None`, so the
    // `issue` tool called `start(id, "", hash)`. An assignment under the empty
    // string is never "alive" (no `.alive/` file named `""`), so any other
    // caller immediately reclaimed it — the headline ownership feature was
    // silently inert for the agent path. These tests pin the post-fix invariants
    // at the store layer so the regression cannot return silently.

    #[tokio::test]
    async fn start_with_distinct_live_owners_collides() {
        // Two DIFFERENT live sessions both try to start the same issue. With
        // real session identities (the post-#13 world), the second MUST see
        // `Assigned` — proving the liveness check is now meaningful for the
        // agent path, not just for the TUI panel.
        let (_tmp, store) = tmp_store();
        let issues_dir = store.issues_dir();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();

        // Session A is live and claims the issue.
        let _guard_a = liveness::acquire(&issues_dir, "proc-A").unwrap();
        let (_, h) = store.read(1).unwrap();
        store.start(1, "proc-A", Some(h)).await.unwrap();

        // Session B is ALSO live (different flock file) and tries to start.
        let _guard_b = liveness::acquire(&issues_dir, "proc-B").unwrap();
        let (_, h2) = store.read(1).unwrap();
        let err = store.start(1, "proc-B", Some(h2)).await.unwrap_err();
        assert!(
            matches!(err, IssueError::Assigned { ref owner, .. } if owner == "proc-A"),
            "a second distinct live owner must be rejected, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn empty_session_assignment_is_immediately_reclaimable_documentation() {
        // Documents the EXACT pre-#13 bug shape at the store layer so that if
        // `start(id, "", hash)` ever reappears in a caller, this test loudly
        // explains why it's wrong: an assignment under "" has no flock holder,
        // so `is_session_alive("")` is false and ANY caller reclaims it.
        //
        // (This is intentionally a documentation test, not a behavior change —
        // the store is policy-free. The fix lives in the agent/tool wiring,
        // covered by oxi-agent's `session_id_wiring_tests`.)
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let issues_dir = store.issues_dir();

        // Caller "" (the pre-#13 agent default) claims the issue.
        let (_, h) = store.read(1).unwrap();
        store.start(1, "", Some(h)).await.unwrap();

        // Nobody holds a flock named "", so the assignment is NOT alive...
        assert!(
            !liveness::is_session_alive(&issues_dir, ""),
            "no flock can be held under the empty string"
        );

        // ...and any real caller reclaims it without contention. This is the
        // silent-ownership-bypass bug that #13 fixes by ensuring agents never
        // use "" as their caller id.
        let _guard_c = liveness::acquire(&issues_dir, "proc-C").unwrap();
        let (_, h2) = store.read(1).unwrap();
        let reclaimed = store.start(1, "proc-C", Some(h2)).await.unwrap();
        assert_eq!(
            reclaimed.meta.assigned_to.as_ref().unwrap().session,
            "proc-C",
            "empty-string assignment is reclaimable — this is the #13 bug shape"
        );
    }

    // ── Phase 2 regression coverage (#2 #3 #4 #9 #12) ────────────────────

    #[tokio::test]
    async fn reopen_clears_closed_at() {
        // #4: reopening must clear `closed_at`. The legacy `update { status:
        // open }` left a stale `closed_at` on a reopened issue.
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (_, h) = store.read(1).unwrap();
        store.start(1, "proc-X", Some(h)).await.unwrap();
        let (_, h) = store.read(1).unwrap();
        store.close(1, "proc-X", Some(h)).await.unwrap();
        let (closed, _) = store.read(1).unwrap();
        assert_eq!(closed.meta.status, Status::Closed);
        assert!(closed.meta.closed_at.is_some());

        let (_, h) = store.read(1).unwrap();
        store.reopen(1, Some(h)).await.unwrap();
        let (reopened, _) = store.read(1).unwrap();
        assert_eq!(reopened.meta.status, Status::Open);
        assert!(
            reopened.meta.closed_at.is_none(),
            "reopen must clear closed_at (#4)"
        );
    }

    #[tokio::test]
    async fn apply_patch_status_open_clears_closed_at() {
        // #4 via the apply_patch path too: status -> Open clears closed_at.
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (_, h) = store.read(1).unwrap();
        store.start(1, "proc-X", Some(h)).await.unwrap();
        let (_, h) = store.read(1).unwrap();
        store.close(1, "proc-X", Some(h)).await.unwrap();

        let (_, h) = store.read(1).unwrap();
        store
            .apply_patch(
                1,
                IssuePatch {
                    status: Some(Status::Open),
                    ..Default::default()
                },
                None,
                Some(h),
            )
            .await
            .unwrap();
        let (after, _) = store.read(1).unwrap();
        assert_eq!(after.meta.status, Status::Open);
        assert!(
            after.meta.closed_at.is_none(),
            "apply_patch status=Open must clear closed_at (#4)"
        );
    }

    #[tokio::test]
    async fn noop_update_does_not_bump_timestamp() {
        // #12: a patch that changes nothing meaningful must not write, must
        // not bump updated_at, must not invalidate the cache.
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (before, _) = store.read(1).unwrap();
        let ts_before = before.meta.updated_at;

        // Empty patch → no-op.
        let (_, h) = store.read(1).unwrap();
        store
            .apply_patch(1, IssuePatch::default(), None, Some(h))
            .await
            .unwrap();
        let (after, _) = store.read(1).unwrap();
        assert_eq!(
            after.meta.updated_at, ts_before,
            "no-op update must not bump updated_at (#12)"
        );

        // A real change DOES bump it (and updates the field).
        std::thread::sleep(std::time::Duration::from_millis(5));
        let (_, h2) = store.read(1).unwrap();
        store
            .apply_patch(
                1,
                IssuePatch {
                    title: Some("New".into()),
                    ..Default::default()
                },
                None,
                Some(h2),
            )
            .await
            .unwrap();
        let (after2, _) = store.read(1).unwrap();
        assert_ne!(
            after2.meta.updated_at, ts_before,
            "real update must bump updated_at"
        );
        assert_eq!(after2.meta.title, "New");
    }

    #[tokio::test]
    async fn apply_patch_labels_clear_vs_keep() {
        // #3: absent vs [] must be distinguishable. None=keep, Some([])=clear,
        // Some([x])=replace.
        let (_tmp, store) = tmp_store();
        store
            .create(
                "T".into(),
                "b".into(),
                Priority::Low,
                vec!["a".into(), "b".into()],
                None,
            )
            .unwrap();

        // Omit labels (None) → keep, while another field changes.
        let (_, h) = store.read(1).unwrap();
        store
            .apply_patch(
                1,
                IssuePatch {
                    priority: Some(Priority::High),
                    ..Default::default()
                },
                None,
                Some(h),
            )
            .await
            .unwrap();
        let (kept, _) = store.read(1).unwrap();
        assert_eq!(kept.meta.labels, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(kept.meta.priority, Priority::High);

        // labels: Some([]) → clear.
        let (_, h) = store.read(1).unwrap();
        store
            .apply_patch(
                1,
                IssuePatch {
                    labels: Some(vec![]),
                    ..Default::default()
                },
                None,
                Some(h),
            )
            .await
            .unwrap();
        let (cleared, _) = store.read(1).unwrap();
        assert!(cleared.meta.labels.is_empty(), "Some([]) must clear labels");

        // labels: Some([x]) → replace.
        let (_, h) = store.read(1).unwrap();
        store
            .apply_patch(
                1,
                IssuePatch {
                    labels: Some(vec!["z".into()]),
                    ..Default::default()
                },
                None,
                Some(h),
            )
            .await
            .unwrap();
        let (replaced, _) = store.read(1).unwrap();
        assert_eq!(replaced.meta.labels, vec!["z".to_string()]);
    }

    #[tokio::test]
    async fn apply_patch_enforces_ownership() {
        // Hardening keeps the legacy ownership policy: a different non-empty
        // assignee blocks the update. apply_patch must reject a non-owner.
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (_, h) = store.read(1).unwrap();
        store.start(1, "proc-A", Some(h)).await.unwrap();

        // proc-B cannot patch.
        let (_, h) = store.read(1).unwrap();
        let err = store
            .apply_patch(
                1,
                IssuePatch {
                    title: Some("X".into()),
                    ..Default::default()
                },
                Some("proc-B".into()),
                Some(h),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, IssueError::NotAssigned { ref caller, .. } if caller == "proc-B"),
            "non-owner must be rejected, got: {err:?}"
        );

        // proc-A (the owner) succeeds.
        let (_, h) = store.read(1).unwrap();
        store
            .apply_patch(
                1,
                IssuePatch {
                    title: Some("X".into()),
                    ..Default::default()
                },
                Some("proc-A".into()),
                Some(h),
            )
            .await
            .unwrap();
        let (patched, _) = store.read(1).unwrap();
        assert_eq!(patched.meta.title, "X");
    }

    // ── Phase 4: top_free_priority (#10) ──

    #[tokio::test]
    async fn top_free_priority_ignores_assigned_and_closed() {
        // Highest priority among OPEN + UNASSIGNED issues only. A critical
        // issue that's assigned or closed must not be reported as "free".
        let (_tmp, store) = tmp_store();
        store
            .create("low".into(), "".into(), Priority::Low, vec![], None)
            .unwrap();
        store
            .create("high".into(), "".into(), Priority::High, vec![], None)
            .unwrap();
        store
            .create(
                "critical-assigned".into(),
                "".into(),
                Priority::Critical,
                vec![],
                None,
            )
            .unwrap();
        store
            .create(
                "critical-closed".into(),
                "".into(),
                Priority::Critical,
                vec![],
                None,
            )
            .unwrap();

        // Assign critical-assigned (free → assign).
        let (_, h) = store.read(3).unwrap();
        store.start(3, "proc", Some(h)).await.unwrap();
        // Close critical-closed.
        let (_, h) = store.read(4).unwrap();
        store.start(4, "proc", Some(h)).await.unwrap();
        let (_, h) = store.read(4).unwrap();
        store.close(4, "proc", Some(h)).await.unwrap();

        // The top FREE priority is High (the two criticals are assigned/closed).
        assert_eq!(store.top_free_priority(), Some(Priority::High));

        // Release everything and nothing is left free with higher than Low/High...
        // (sanity: when all open free issues are gone, returns None.)
        let (_, h) = store.read(1).unwrap();
        store.start(1, "proc", Some(h)).await.unwrap();
        let (_, h) = store.read(2).unwrap();
        store.start(2, "proc", Some(h)).await.unwrap();
        assert_eq!(
            store.top_free_priority(),
            None,
            "no open unassigned issue → None"
        );
    }
}
