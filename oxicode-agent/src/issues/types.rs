//! Issue domain types: status, priority, assignment, metadata, body.

use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Issue status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Issue is open (work not complete).
    #[default]
    Open,
    /// Issue is closed (resolved or abandoned).
    Closed,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    /// Low urgency.
    Low,
    /// Default priority.
    #[default]
    Medium,
    /// High urgency.
    High,
    /// Drop everything.
    Critical,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
/// [`crate::issues::liveness::is_session_alive`]) — there is no expiry
/// timestamp.
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
    /// `owner/repo` of the synced issue.
    pub repo: String,
    /// GitHub issue number.
    pub number: u64,
    /// Canonical URL.
    pub url: String,
}

/// YAML frontmatter for an issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueMeta {
    /// Monotonic id assigned by the store.
    pub id: u32,
    /// One-line summary.
    pub title: String,
    /// Open/closed state.
    #[serde(default)]
    pub status: Status,
    /// Urgency ordering.
    #[serde(default)]
    pub priority: Priority,
    /// Free-form tags.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Human assignee (informational; distinct from [`Assignment`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last mutation timestamp.
    pub updated_at: DateTime<Utc>,
    /// When closed, if closed.
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
    /// Structured metadata (frontmatter).
    pub meta: IssueMeta,
    /// Raw markdown body (everything after the `---` frontmatter block).
    pub body: String,
    /// Path to the source file (None for unsaved/in-memory issues).
    pub path: Option<PathBuf>,
}

impl Issue {
    /// Combined status badge for list rendering: `▣ open`.
    pub fn list_badge(&self) -> String {
        let lock = if self.meta.assigned_to.is_some() {
            "▣ "
        } else {
            ""
        };
        format!("{}{}", lock, self.meta.status)
    }
}

/// A precise update payload for [`crate::issues::FileIssueStore::apply_patch`].
///
/// Every field is `Option`: `None` = keep the existing value, `Some` = replace
/// it. `labels` is the only field with a meaningful empty state —
/// `Some(vec![])` clears all labels while `None` keeps them. This resolves
/// defect #3: through the tool schema, "field absent" vs `[]` were previously
/// indistinguishable, so labels could never be cleared without resending the
/// full set.
///
/// Used by the `issue` tool's `update` action (via
/// [`crate::issues::FileIssueStore::apply_patch`]) and is the
/// recommended mutation surface for callers that want precise keep-vs-replace
/// semantics.
#[derive(Debug, Clone, Default)]
pub struct IssuePatch {
    /// Replace the title.
    pub title: Option<String>,
    /// Replace the markdown body.
    pub body: Option<String>,
    /// Replace the status. Setting [`Status::Open`] also clears `closed_at`
    /// (see [`crate::issues::FileIssueStore::apply_patch`], which fixes
    /// the latent reopen bug #4).
    pub status: Option<Status>,
    /// Replace the priority.
    pub priority: Option<Priority>,
    /// Replace the labels wholesale. `Some(vec![])` clears all labels.
    pub labels: Option<Vec<String>>,
}
