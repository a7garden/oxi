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
    #[error("model not found: {model_id}")]
    ModelNotFound { model_id: String },

    #[error("provider not found: {provider}")]
    ProviderNotFound { provider: String },

    #[error("all providers exhausted: {attempts} attempts")]
    AllProvidersExhausted { attempts: usize },

    // ── Agent Lifecycle ────────────────────────────────────────────────────────
    #[error("agent {agent_id} not runnable (status: {status})")]
    AgentNotRunnable { agent_id: String, status: String },

    #[error("agent {agent_id} already running")]
    AgentAlreadyRunning { agent_id: String },

    #[error("snapshot not found: {agent_id}")]
    SnapshotNotFound { agent_id: String },

    #[error("snapshot corrupt: {agent_id}: {reason}")]
    SnapshotCorrupt { agent_id: String, reason: String },

    // ── Security ─────────────────────────────────────────────────────────────
    #[error("permission denied: {subject} requires {capability}")]
    PermissionDenied { subject: String, capability: String },

    #[error("capability expired: {subject}")]
    CapabilityExpired { subject: String },

    // ── Coordination ─────────────────────────────────────────────────────────
    #[error("work item not found: {item_id}")]
    WorkItemNotFound { item_id: String },

    #[error("version conflict on {key}: expected {expected}, current {current}")]
    VersionConflict {
        key: String,
        expected: u64,
        current: u64,
    },

    #[error("vote session not found: {vote_id}")]
    VoteNotFound { vote_id: String },

    // ── Middleware ───────────────────────────────────────────────────────────
    #[error("middleware blocked: {middleware}: {reason}")]
    MiddlewareBlocked { middleware: String, reason: String },

    #[error("token budget exceeded: {used} / {budget}")]
    TokenBudgetExceeded { used: usize, budget: usize },

    #[error("cost budget exceeded: ${used:.4} / ${budget:.4}")]
    CostBudgetExceeded { used: f64, budget: f64 },

    // ── Routing ─────────────────────────────────────────────────────────────
    #[error("routing disabled")]
    RoutingDisabled,

    #[error("no route available for model: {model_id}")]
    NoRouteAvailable { model_id: String },

    // ── Agent Execution ─────────────────────────────────────────────────
    #[error("agent execution failed: {reason}")]
    ExecutionFailed { reason: String },

    #[error("agent group failed: {failed}/{total} agents")]
    GroupExecutionFailed { failed: usize, total: usize },

    #[error("run cancelled")]
    Cancelled,

    // ── Ports ───────────────────────────────────────────────────────────────
    #[error("port not configured: {port} (use OxiBuilder::with_port_*(...))")]
    PortNotConfigured { port: &'static str },

    // ── Catalog ─────────────────────────────────────────────────────────────
    /// The catalog port is not wired or returned no data for the request.
    #[error("catalog unavailable: {reason}")]
    CatalogUnavailable { reason: String },

    /// A user-supplied catalog override file failed to parse.
    #[error("catalog override parse error at {path}: {reason}")]
    CatalogOverrideParse { path: String, reason: String },

    /// A catalog refresh attempt failed (network, HTTP, or parse error).
    /// The stale snapshot is still served; this error is informational.
    #[error("catalog refresh failed: {reason}")]
    CatalogRefresh { reason: String },

    // ── General ─────────────────────────────────────────────────────────────
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
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_sdk_result_err() {
        let result: SdkResult<i32> = Err(SdkError::Cancelled);
        assert!(matches!(result.unwrap_err(), SdkError::Cancelled));
    }
}
