//! Issue domain types: status, priority, assignment, metadata, body.

use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Issue status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Open,
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
    Low,
    #[default]
    Medium,
    High,
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
/// [`crate::store::issues::liveness::is_session_alive`]) — there is no expiry
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

/// A precise update payload for [`crate::store::issues::FileIssueStore::apply_patch`].
///
/// Every field is `Option`: `None` = keep the existing value, `Some` = replace
/// it. `labels` is the only field with a meaningful empty state —
/// `Some(vec![])` clears all labels while `None` keeps them. This resolves
/// defect #3: through the tool schema, "field absent" vs `[]` were previously
/// indistinguishable, so labels could never be cleared without resending the
/// full set.
///
/// Used by the `issue` tool's `update` action (via
/// [`crate::store::issues::FileIssueStore::apply_patch`]) and is the
/// recommended mutation surface for callers that want precise keep-vs-replace
/// semantics.
#[derive(Debug, Clone, Default)]
pub struct IssuePatch {
    /// Replace the title.
    pub title: Option<String>,
    /// Replace the markdown body.
    pub body: Option<String>,
    /// Replace the status. Setting [`Status::Open`] also clears `closed_at`
    /// (see [`crate::store::issues::FileIssueStore::apply_patch`], which fixes
    /// the latent reopen bug #4).
    pub status: Option<Status>,
    /// Replace the priority.
    pub priority: Option<Priority>,
    /// Replace the labels wholesale. `Some(vec![])` clears all labels.
    pub labels: Option<Vec<String>>,
}
