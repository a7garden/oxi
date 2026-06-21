//! oxi-sdk error types
//!
//! Structured error enum for SDK consumers to match against.

use thiserror::Error;

/// Unified SDK result type.
pub type SdkResult<T> = Result<T, SdkError>;

/// oxi-sdk structured error type.
///
/// SDK consumers can use `match` to handle specific error cases.
/// Internal implementations may still use `anyhow`, converted at public API boundaries.
#[derive(Debug, Error)]
pub enum SdkError {
    // ── Model/Provider ────────────────────────────────────────────────────────
    /// The requested model could not be resolved by any configured provider.
    #[error("model not found: {model_id}")]
    ModelNotFound {
        /// Identifier of the model that was requested.
        model_id: String,
    },

    /// No provider matching the requested name is registered with the SDK.
    #[error("provider not found: {provider}")]
    ProviderNotFound {
        /// Name of the provider that was requested.
        provider: String,
    },

    /// Every provider in the failover chain was attempted and failed.
    #[error("all providers exhausted: {attempts} attempts")]
    AllProvidersExhausted {
        /// Number of provider attempts made before giving up.
        attempts: usize,
    },

    // ── Agent Lifecycle ────────────────────────────────────────────────────────
    /// The agent exists but cannot run in its current lifecycle status.
    #[error("agent {agent_id} not runnable (status: {status})")]
    AgentNotRunnable {
        /// Identifier of the agent.
        agent_id: String,
        /// Current lifecycle status of the agent.
        status: String,
    },

    /// An attempt was made to start an agent that is already running.
    #[error("agent {agent_id} already running")]
    AgentAlreadyRunning {
        /// Identifier of the agent.
        agent_id: String,
    },

    /// No persisted snapshot could be found for the agent.
    #[error("snapshot not found: {agent_id}")]
    SnapshotNotFound {
        /// Identifier of the agent whose snapshot was requested.
        agent_id: String,
    },

    /// A persisted snapshot exists but failed integrity validation.
    #[error("snapshot corrupt: {agent_id}: {reason}")]
    SnapshotCorrupt {
        /// Identifier of the agent whose snapshot is corrupt.
        agent_id: String,
        /// Human-readable explanation of the corruption.
        reason: String,
    },

    // ── Security ─────────────────────────────────────────────────────────────
    /// The principal lacks a required capability for the requested action.
    #[error("permission denied: {subject} requires {capability}")]
    PermissionDenied {
        /// The principal (agent, user, or component) requesting access.
        subject: String,
        /// The capability that would be required to permit the action.
        capability: String,
    },

    /// A previously granted capability has expired and is no longer valid.
    #[error("capability expired: {subject}")]
    CapabilityExpired {
        /// The principal whose capability expired.
        subject: String,
    },

    // ── Coordination ─────────────────────────────────────────────────────────
    /// A referenced coordination work item does not exist.
    #[error("work item not found: {item_id}")]
    WorkItemNotFound {
        /// Identifier of the missing work item.
        item_id: String,
    },

    /// An optimistic-concurrency guard detected a stale version of a value.
    #[error("version conflict on {key}: expected {expected}, current {current}")]
    VersionConflict {
        /// The key whose version was in conflict.
        key: String,
        /// Version the caller expected to observe.
        expected: u64,
        /// Version currently stored.
        current: u64,
    },

    /// A referenced vote session could not be found.
    #[error("vote session not found: {vote_id}")]
    VoteNotFound {
        /// Identifier of the missing vote session.
        vote_id: String,
    },

    // ── Middleware ───────────────────────────────────────────────────────────
    /// A middleware intercepted and blocked the request.
    #[error("middleware blocked: {middleware}: {reason}")]
    MiddlewareBlocked {
        /// Name of the middleware that blocked the request.
        middleware: String,
        /// Why the middleware blocked the request.
        reason: String,
    },

    /// The configured token-usage budget for a run was exceeded.
    #[error("token budget exceeded: {used} / {budget}")]
    TokenBudgetExceeded {
        /// Number of tokens consumed so far.
        used: usize,
        /// Maximum number of tokens permitted by the budget.
        budget: usize,
    },

    /// The configured monetary cost budget for a run was exceeded.
    #[error("cost budget exceeded: ${used:.4} / ${budget:.4}")]
    CostBudgetExceeded {
        /// Cost consumed so far, in currency units.
        used: f64,
        /// Maximum cost permitted by the budget.
        budget: f64,
    },

    // ── Routing ─────────────────────────────────────────────────────────────
    /// Routing is disabled in the current configuration.
    #[error("routing disabled")]
    RoutingDisabled,

    /// No route could be determined for the requested model.
    #[error("no route available for model: {model_id}")]
    NoRouteAvailable {
        /// Identifier of the model for which no route exists.
        model_id: String,
    },

    // ── Agent Execution ─────────────────────────────────────────────────
    /// A single agent's execution failed.
    #[error("agent execution failed: {reason}")]
    ExecutionFailed {
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// One or more agents in a group run failed.
    #[error("agent group failed: {failed}/{total} agents")]
    GroupExecutionFailed {
        /// Number of agents in the group that failed.
        failed: usize,
        /// Total number of agents in the group.
        total: usize,
    },

    /// The run was cancelled before completing.
    #[error("run cancelled")]
    Cancelled,

    // ── Ports ───────────────────────────────────────────────────────────────
    /// A required port was never configured on the builder.
    #[error("port not configured: {port} (use OxiBuilder::with_port_*(...))")]
    PortNotConfigured {
        /// Name of the missing port.
        port: &'static str,
    },

    /// A URL scheme has no registered handler.
    #[error("unknown URL scheme: {scheme} (no handler registered)")]
    UnknownScheme {
        /// The unrecognized URL scheme.
        scheme: String,
    },

    // ── Catalog ─────────────────────────────────────────────────────────────
    /// The catalog port is not wired or returned no data for the request.
    #[error("catalog unavailable: {reason}")]
    CatalogUnavailable {
        /// Why the catalog was unavailable.
        reason: String,
    },

    /// A user-supplied catalog override file failed to parse.
    #[error("catalog override parse error at {path}: {reason}")]
    CatalogOverrideParse {
        /// Path to the override file that failed to parse.
        path: String,
        /// Why the override file could not be parsed.
        reason: String,
    },

    /// A catalog refresh attempt failed (network, HTTP, or parse error).
    /// The stale snapshot is still served; this error is informational.
    #[error("catalog refresh failed: {reason}")]
    CatalogRefresh {
        /// Why the refresh attempt failed.
        reason: String,
    },

    // ── General ─────────────────────────────────────────────────────────────
    /// An unexpected internal error, converted from an [`anyhow::Error`].
    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

impl SdkError {
    /// Returns true if this is an internal/unexpected error.
    pub fn is_internal(&self) -> bool {
        matches!(self, SdkError::Internal(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sdk_error_display() {
        let err = SdkError::ModelNotFound {
            model_id: "test-model".into(),
        };
        assert!(err.to_string().contains("test-model"));

        let err = SdkError::PermissionDenied {
            subject: "agent-001".into(),
            capability: "FileWrite".into(),
        };
        assert!(err.to_string().contains("agent-001"));
        assert!(err.to_string().contains("FileWrite"));
    }

    #[test]
    fn test_sdk_error_from_anyhow() {
        let anyhow_err = anyhow::anyhow!("test error");
        let sdk_err: SdkError = SdkError::from(anyhow_err);
        assert!(sdk_err.is_internal());
    }

    #[test]
    fn test_version_conflict_error() {
        let err = SdkError::VersionConflict {
            key: "counter".into(),
            expected: 5,
            current: 7,
        };
        let msg = err.to_string();
        assert!(msg.contains("counter"));
        assert!(msg.contains("5"));
        assert!(msg.contains("7"));
    }

    #[test]
    fn test_execution_failed_display() {
        let err = SdkError::ExecutionFailed {
            reason: "provider timeout".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("agent execution failed"));
        assert!(msg.contains("provider timeout"));
    }

    #[test]
    fn test_group_execution_failed_display() {
        let err = SdkError::GroupExecutionFailed {
            failed: 2,
            total: 5,
        };
        let msg = err.to_string();
        assert!(msg.contains("agent group failed"));
        assert!(msg.contains("2/5"));
    }

    #[test]
    fn test_cancelled_display() {
        let err = SdkError::Cancelled;
        assert_eq!(err.to_string(), "run cancelled");
    }

    #[test]
    fn test_sdk_result_ok() {
        let result: SdkResult<i32> = Ok(42);
        assert!(matches!(result, Ok(42)));
    }

    #[test]
    fn test_sdk_result_err() {
        let result: SdkResult<i32> = Err(SdkError::Cancelled);
        assert!(matches!(result, Err(SdkError::Cancelled)));
    }
}
