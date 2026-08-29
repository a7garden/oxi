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
    Conflict {
        /// Id of the modified issue.
        id: u32,
    },

    /// Another live session holds the assignment for this issue.
    #[error("issue #{id} is currently being worked on by session {owner}")]
    Assigned {
        /// Id of the contended issue.
        id: u32,
        /// Session id currently holding the assignment.
        owner: String,
        /// When the current assignment was taken.
        acquired_at: DateTime<Utc>,
    },

    /// The caller does not hold the assignment required for this mutation.
    #[error("issue #{id} is not assigned to session {caller}; run `start` first")]
    NotAssigned {
        /// Id of the issue being mutated.
        id: u32,
        /// Session id that attempted the mutation.
        caller: String,
    },

    /// Issue id not found.
    #[error("issue #{id} not found")]
    NotFound {
        /// Id that could not be resolved.
        id: u32,
    },

    /// Underlying I/O failure.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// Any other failure.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
