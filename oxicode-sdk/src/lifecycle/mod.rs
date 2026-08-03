//! Lifecycle module — agent lifecycle management.
//!
//! Provides `AgentHandle`, `AgentSupervisor`, and `SnapshotStore` for
//! spawn / suspend / resume / checkpoint / terminate operations.
//!
//! # Module layout
//!
//! | File              | Responsibility                                        |
//! |-------------------|-------------------------------------------------------|
//! | `mod.rs`          | `AgentStatus`, `AgentLifecycleEvent`, `MetricsSnapshot`, re-exports |
//! | `supervisor.rs`   | `AgentSupervisor`, `SupervisorPolicy`, `RestartBackoff`, `AgentHandle` |
//! | `snapshot.rs`     | `AgentSnapshot`, `ToolManifest`, `SnapshotStore`, `FileSnapshotStore` |

mod agent_pool;
pub mod hub;
mod snapshot;
mod subagent_coordinator;
mod supervisor;

// ── Re-exports (thin facade) ─────────────────────────────────────────────
pub use agent_pool::AgentPool;
pub use hub::{HubKind, HubStatus};
pub use snapshot::{AgentSnapshot, FileSnapshotStore, SnapshotStore, ToolManifest};
pub use subagent_coordinator::{
    DEFAULT_MAX_SUBAGENT_DEPTH, SubagentCoordinator, SubagentCoordinatorError,
    SubagentSpawnRequest, SubagentState, SubagentTracker,
};
pub use supervisor::{AgentHandle, AgentSupervisor, RestartBackoff, SupervisorPolicy};

use serde::{Deserialize, Serialize};

// ── AgentStatus ──────────────────────────────────────────────────────────

/// Lifecycle status of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Created but has not started any runs.
    #[default]
    Created,
    /// Actively processing.
    Running,
    /// Suspended (can be resumed).
    Suspended,
    /// Completed all work (terminal).
    Terminated,
    /// Fatal error, cannot be resumed (terminal).
    Failed,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Suspended => write!(f, "suspended"),
            Self::Terminated => write!(f, "terminated"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl AgentStatus {
    /// Whether this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminated | Self::Failed)
    }

    /// Whether the agent can accept a new `run()`.
    pub fn is_runnable(&self) -> bool {
        matches!(self, Self::Created | Self::Suspended)
    }
}

// ── AgentLifecycleEvent ──────────────────────────────────────────────────

/// Events emitted during agent lifecycle transitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentLifecycleEvent {
    /// An agent was spawned into the pool.
    Spawned {
        /// Identifier of the spawned agent.
        agent_id: String,
        /// Identifier of the parent agent, if spawned as a child.
        parent_id: Option<String>,
        /// Identifier of the model the agent is configured to use.
        model_id: String,
        /// Wall-clock time of the event, in ms since Unix epoch.
        timestamp_ms: u64,
    },
    /// An agent began a run.
    RunStart {
        /// Identifier of the agent.
        agent_id: String,
        /// Wall-clock time of the event, in ms since Unix epoch.
        timestamp_ms: u64,
    },
    /// An agent completed a run.
    RunEnd {
        /// Identifier of the agent.
        agent_id: String,
        /// Wall-clock time of the event, in ms since Unix epoch.
        timestamp_ms: u64,
        /// Whether the run completed without error.
        success: bool,
    },
    /// An agent was suspended and its state snapshotted.
    Suspended {
        /// Identifier of the agent.
        agent_id: String,
        /// Captured state snapshot at suspension time.
        snapshot: Box<AgentSnapshot>,
        /// Wall-clock time of the event, in ms since Unix epoch.
        timestamp_ms: u64,
    },
    /// A suspended agent resumed execution.
    Resumed {
        /// Identifier of the agent.
        agent_id: String,
        /// Identifier of the snapshot resumed from, if any.
        from_snapshot_id: Option<String>,
        /// Wall-clock time of the event, in ms since Unix epoch.
        timestamp_ms: u64,
    },
    /// An agent was permanently terminated.
    Terminated {
        /// Identifier of the agent.
        agent_id: String,
        /// Wall-clock time of the event, in ms since Unix epoch.
        timestamp_ms: u64,
    },
    /// An agent switched its underlying model.
    ModelSwitched {
        /// Identifier of the agent.
        agent_id: String,
        /// Identifier of the previous model.
        from_model: String,
        /// Identifier of the new model.
        to_model: String,
        /// Wall-clock time of the event, in ms since Unix epoch.
        timestamp_ms: u64,
    },
}

impl AgentLifecycleEvent {
    /// Current wall-clock time in ms since Unix epoch.
    pub fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

// Re-export metrics snapshot (used in snapshots and handles).
pub use crate::metrics::MetricsSnapshot;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_status_display() {
        assert_eq!(AgentStatus::Running.to_string(), "running");
        assert_eq!(AgentStatus::Suspended.to_string(), "suspended");
        assert_eq!(AgentStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn agent_status_terminal() {
        assert!(AgentStatus::Terminated.is_terminal());
        assert!(AgentStatus::Failed.is_terminal());
        assert!(!AgentStatus::Running.is_terminal());
        assert!(!AgentStatus::Created.is_terminal());
    }

    #[test]
    fn agent_status_runnable() {
        assert!(AgentStatus::Created.is_runnable());
        assert!(AgentStatus::Suspended.is_runnable());
        assert!(!AgentStatus::Running.is_runnable());
        assert!(!AgentStatus::Terminated.is_runnable());
    }

    #[test]
    fn lifecycle_event_now_ms() {
        let ms = AgentLifecycleEvent::now_ms();
        assert!(ms > 1_700_000_000_000); // after 2023
    }

    #[test]
    fn metrics_snapshot_default() {
        let m = MetricsSnapshot::default();
        assert_eq!(m.total_runs, 0);
    }
}
