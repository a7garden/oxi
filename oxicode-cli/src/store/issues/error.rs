//! Errors returned by issue operations.

use std::io;

use chrono::{DateTime, Utc};
use thiserror::Error;

/// Errors returned by issue operations.
///
/// Kept as a small typed enum (per AGENTS.md: application crate uses anyhow
/// broadly, but these specific variants are useful to distinguish for the
/// agent tool layer and tests).
#[derive(Debug, Error)]
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
