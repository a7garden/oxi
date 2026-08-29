//! Local issue tracking system — GitHub-style issues stored as markdown files.
//!
//! Issues live in `.oxicode/issues/` at the project root (discovered by walking
//! up from the current directory until `.oxicode/` is found, mirroring
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
//!   blob. Different workload, different storage shape.
//! - **Optimistic concurrency (content-hash CAS).** Mutations take an optional
//!   `content_hash` captured at the last read. The write is rejected if the
//!   on-disk content has changed since. This is the exact pattern used by the
//!   `edit` tool (`oxicode-agent/src/tools/edit.rs`), so external edits (e.g.
//!   someone editing the file in vim) are detected without any locking.
//! - **Atomic writes** via temp+rename.
//! - **Assignment is process-liveness based, not time based.** An assigned
//!   issue records the owning session id. Whether that session is still alive
//!   is determined by an OS-held advisory lock on
//!   `.oxicode/issues/.alive/<session_id>` (see [`liveness`]). When the owning
//!   process exits — including `kill -9`, crash, or terminal close — the OS
//!   releases the lock and the assignment becomes stale and reclaimable.

use std::io;
use std::path::Path;
pub mod error;
pub mod filter;
pub mod liveness;
pub mod serialize;
pub mod store;
pub mod types;

// Re-export the public surface so callers can keep using
// `crate::issues::*` exactly as before the directory split.
pub use error::IssueError;
pub use filter::IssueFilter;
pub use serialize::{content_hash, issue_filename, issues_dir, parse_issue, serialize_issue};
pub use store::{FileIssueStore, IssueSummary};
pub use types::{Assignment, GithubRef, Issue, IssueMeta, IssuePatch, Priority, Status};

/// Atomically write a string to `path` (temp file + rename).
///
/// Moved from the CLI's `store::fs_util` when the issues module relocated to
/// this crate; semantics preserved byte-for-byte so on-disk temp artifacts
/// stay identical.
pub(crate) fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&tmp, content)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Best-effort cleanup; the rename error is the one we propagate.
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
