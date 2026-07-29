//! Display metadata for the Agent Hub overlay (advisor + subagent monitoring).
//! Kept separate from supervisor.rs (lifecycle state machine) so display
//! concerns don't pollute the supervisor API.

use serde::{Deserialize, Serialize};

/// Role of an agent in the hub view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HubKind {
    /// The main session agent.
    Main,
    /// A subagent spawned by the `subagent` tool (in- or out-of-process).
    Subagent,
    /// The read-only advisor reviewer.
    Advisor,
}

impl HubKind {
    /// omp `kind` lower-case tag.
    pub const fn as_str(self) -> &'static str {
        match self {
            HubKind::Main => "main",
            HubKind::Subagent => "task",
            HubKind::Advisor => "advisor",
        }
    }
}

/// High-level status for the hub table. Maps onto the supervisor's atomic
/// status (Running/Suspended/Terminated/Failed) + a parked concept for
/// agents kept alive after completion awaiting revival.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HubStatus {
    /// Currently executing a task.
    Running,
    /// Alive but not actively executing (e.g. waiting on the next prompt).
    Idle,
    /// STOPPED in supervisor terms; held in memory for later revival.
    Parked,
    /// FAILED or unrecoverable.
    Aborted,
}

impl HubStatus {
    /// Lower-case tag for the status badge column.
    pub const fn as_str(self) -> &'static str {
        match self {
            HubStatus::Running => "running",
            HubStatus::Idle => "idle",
            HubStatus::Parked => "parked",
            HubStatus::Aborted => "aborted",
        }
    }

    /// Sort priority — lower comes first.
    pub const fn sort_key(self) -> u8 {
        match self {
            HubStatus::Running => 0,
            HubStatus::Idle => 1,
            HubStatus::Parked => 2,
            HubStatus::Aborted => 3,
        }
    }
}
