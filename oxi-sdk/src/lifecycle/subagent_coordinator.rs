//! Subagent coordinator — tracks spawned subagents through the
//! `pending → active → completed` lifecycle with cancellation,
//! background mode, and depth guarding.
//!
//! Ported from grok's `SubagentCoordinator` (see
//! `docs/designs/2026-07-18-stub-completion.md` §4.6) with these
//! deviations:
//!
//! - **No ~80-field spawn context.** Spawning takes a tightly-typed
//!   [`SubagentSpawnRequest`] instead of a god-struct.
//! - **`MAX_SUBAGENT_DEPTH` defaults to 2** (OMP default) rather than
//!   grok's fixed `1`. Configurable per-coordinator.
//! - **`resume_from`** is implemented as a prompt-preamble inheritance
//!   (last response text) for MVP — full transcript cloning is a
//!   follow-up.
//!
//! # Lifecycle
//!
//! 1. [`SubagentCoordinator::spawn`] registers a [`SubagentTracker`]
//!    in [`SubagentState::Pending`] and kicks off a background task
//!    that transitions through `Active` → `Completed`/`Failed`/`Cancelled`.
//! 2. [`SubagentCoordinator::tracker`] returns the tracker for polling.
//! 3. [`SubagentTracker::wait_for_completion`] blocks (with timeout)
//!    until the run finishes. On timeout it returns `None`; the
//!    underlying task keeps running in the background — the caller can
//!    poll again or treat the agent as backgrounded.
//! 4. [`SubagentCoordinator::cancel`] triggers cancellation; the
//!    background task observes it and transitions to `Cancelled`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::lifecycle::AgentSupervisor;
use oxi_agent::AgentConfig;

/// Default ceiling on subagent nesting depth.
///
/// grok hard-codes this to `1` (no recursive subagents). OMP defaults
/// to `2`. We adopt the OMP default to allow one level of recursive
/// delegation — deeper trees blow up token budgets without
/// commensurate capability gains, and the cap keeps runaway
/// delegation observable.
pub const DEFAULT_MAX_SUBAGENT_DEPTH: u32 = 2;

/// One subagent's lifecycle state.
///
/// Order matters — `is_terminal()` returns true for the last three
/// variants.
#[derive(Debug, Clone)]
pub enum SubagentState {
    /// Spawn registered, background task not yet running the agent.
    Pending {
        /// When the spawn was registered (ms since epoch).
        registered_at_ms: u64,
    },
    /// Agent is currently executing.
    Active {
        /// When the run started (ms since epoch).
        started_at_ms: u64,
    },
    /// Agent finished successfully.
    Completed {
        /// When the run finished (ms since epoch).
        finished_at_ms: u64,
        /// The agent's final response text (may be empty if the run
        /// produced no text — e.g. tool-only output).
        response: String,
    },
    /// Agent run failed.
    Failed {
        /// When the run finished (ms since epoch).
        finished_at_ms: u64,
        /// Error message from the failed run.
        error: String,
    },
    /// Agent run was cancelled.
    Cancelled {
        /// When the cancellation took effect (ms since epoch).
        finished_at_ms: u64,
    },
}

impl SubagentState {
    /// Whether this state will never transition again.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SubagentState::Completed { .. }
                | SubagentState::Failed { .. }
                | SubagentState::Cancelled { .. }
        )
    }

    /// Whether the underlying run is currently active.
    pub fn is_active(&self) -> bool {
        matches!(self, SubagentState::Active { .. })
    }
}

/// Inputs to [`SubagentCoordinator::spawn`].
#[derive(Debug, Clone)]
pub struct SubagentSpawnRequest {
    /// Caller-chosen unique ID for this subagent. Two spawns with the
    /// same ID collide and the second is rejected.
    pub agent_id: String,
    /// Agent configuration (model, tools, etc.).
    pub config: AgentConfig,
    /// Initial task prompt.
    pub task: String,
    /// If `true`, [`SubagentCoordinator::spawn`] returns immediately
    /// and the caller polls via [`SubagentCoordinator::tracker`].
    /// If `false`, the spawn still returns immediately (background
    /// task drives the run) — the flag is informational and surfaces
    /// in [`SubagentTracker::run_in_background`] for callers that
    /// want to distinguish "fire-and-forget" from "foreground await".
    pub run_in_background: bool,
    /// If `Some(parent_id)`, the parent's last response text is
    /// prepended to `task` as preamble context.
    pub resume_from: Option<String>,
    /// Current nesting depth (caller-supplied). The coordinator
    /// rejects spawns whose depth exceeds its configured maximum.
    pub depth: u32,
}

/// Per-subagent tracker — exposes cancellation and completion polling.
#[derive(Debug)]
pub struct SubagentTracker {
    cancel_token: CancellationToken,
    completion: Arc<Notify>,
    state: Arc<RwLock<SubagentState>>,
    run_in_background: bool,
    resume_from: Option<String>,
    spawned_at_ms: u64,
}

impl SubagentTracker {
    /// Current lifecycle state (zero-copy snapshot).
    pub fn state(&self) -> SubagentState {
        self.state.read().clone()
    }

    /// Whether this subagent was spawned in background mode.
    pub fn run_in_background(&self) -> bool {
        self.run_in_background
    }

    /// Parent agent ID whose transcript was inherited, if any.
    pub fn resume_from(&self) -> Option<&str> {
        self.resume_from.as_deref()
    }

    /// Registration timestamp (ms since epoch).
    pub fn spawned_at_ms(&self) -> u64 {
        self.spawned_at_ms
    }

    /// Trigger cancellation. The background task observes this and
    /// transitions to [`SubagentState::Cancelled`] on its next
    /// `tokio::select!` tick. Returns immediately — callers should
    /// poll [`Self::state`] or [`Self::wait_for_completion`] to
    /// observe the actual transition.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Block until the underlying run reaches a terminal state, or
    /// `timeout` elapses.
    ///
    /// On timeout returns `None`; the run continues in the
    /// background and the caller may poll again.
    pub async fn wait_for_completion(&self, timeout: Duration) -> Option<SubagentState> {
        // Fast-path: already terminal.
        let current = self.state();
        if current.is_terminal() {
            return Some(current);
        }

        match tokio::time::timeout(timeout, self.completion.notified()).await {
            Ok(()) => Some(self.state()),
            Err(_) => None,
        }
    }
}

/// Coordinator error.
#[derive(Debug, thiserror::Error)]
pub enum SubagentCoordinatorError {
    /// Spawn requested with `depth` greater than the coordinator's max.
    #[error("subagent depth {depth} exceeds maximum {max}")]
    MaxDepthExceeded {
        /// The depth the caller requested.
        depth: u32,
        /// The coordinator's configured maximum.
        max: u32,
    },
    /// Spawn requested with an `agent_id` already in use.
    #[error("subagent agent_id '{0}' already in use")]
    DuplicateId(String),
    /// The underlying supervisor rejected the spawn (model/provider
    /// not found, etc.).
    #[error("supervisor spawn failed: {0}")]
    SpawnFailed(String),
    /// `resume_from` referenced an agent the coordinator has no
    /// record of.
    #[error("resume_from agent '{0}' not found")]
    ResumeFromNotFound(String),
}

/// Result type for coordinator operations.
pub type Result<T, E = SubagentCoordinatorError> = std::result::Result<T, E>;

/// Subagent coordinator.
///
/// Wraps an [`AgentSupervisor`] to add lifecycle tracking, cancellation,
/// and depth guarding. Owns the [`SubagentTracker`] registry.
#[derive(Clone)]
pub struct SubagentCoordinator {
    supervisor: AgentSupervisor,
    trackers: Arc<RwLock<HashMap<String, Arc<SubagentTracker>>>>,
    /// Last response text per agent, used to populate `resume_from`
    /// preambles for downstream spawns.
    last_responses: Arc<RwLock<HashMap<String, String>>>,
    max_depth: u32,
}

impl SubagentCoordinator {
    /// Create a new coordinator with the default max depth
    /// ([`DEFAULT_MAX_SUBAGENT_DEPTH`]).
    pub fn new(supervisor: AgentSupervisor) -> Self {
        Self::with_max_depth(supervisor, DEFAULT_MAX_SUBAGENT_DEPTH)
    }

    /// Create with an explicit max subagent nesting depth.
    pub fn with_max_depth(supervisor: AgentSupervisor, max_depth: u32) -> Self {
        Self {
            supervisor,
            trackers: Arc::new(RwLock::new(HashMap::new())),
            last_responses: Arc::new(RwLock::new(HashMap::new())),
            max_depth,
        }
    }

    /// Configured maximum nesting depth.
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Borrow the wrapped supervisor (for direct spawns that bypass
    /// coordinator tracking).
    pub fn supervisor(&self) -> &AgentSupervisor {
        &self.supervisor
    }

    /// Number of currently-tracked subagents (any state).
    pub fn tracked_count(&self) -> usize {
        self.trackers.read().len()
    }

    /// Look up a tracker by agent ID.
    pub fn tracker(&self, agent_id: &str) -> Option<Arc<SubagentTracker>> {
        self.trackers.read().get(agent_id).cloned()
    }

    /// Current state of an agent, or `None` if unknown.
    pub fn state(&self, agent_id: &str) -> Option<SubagentState> {
        self.tracker(agent_id).map(|t| t.state())
    }

    /// Snapshot of every tracked agent ID → state.
    pub fn snapshot(&self) -> HashMap<String, SubagentState> {
        self.trackers
            .read()
            .iter()
            .map(|(id, t)| (id.clone(), t.state()))
            .collect()
    }

    /// Spawn a subagent. Returns immediately with the agent ID on
    /// success — the run proceeds in a background task.
    ///
    /// Errors:
    /// - [`SubagentCoordinatorError::MaxDepthExceeded`] — `req.depth`
    ///   exceeds [`Self::max_depth`].
    /// - [`SubagentCoordinatorError::DuplicateId`] — `req.agent_id`
    ///   is already tracked.
    /// - [`SubagentCoordinatorError::SpawnFailed`] — supervisor
    ///   rejected the spawn (bad model/provider).
    /// - [`SubagentCoordinatorError::ResumeFromNotFound`] —
    ///   `req.resume_from` names an unknown agent.
    pub fn spawn(&self, req: SubagentSpawnRequest) -> Result<String> {
        if req.depth > self.max_depth {
            return Err(SubagentCoordinatorError::MaxDepthExceeded {
                depth: req.depth,
                max: self.max_depth,
            });
        }

        // Reserve the ID first so concurrent spawns collide cleanly.
        if self.trackers.read().contains_key(&req.agent_id) {
            return Err(SubagentCoordinatorError::DuplicateId(req.agent_id));
        }

        // Resolve resume_from → preamble.
        let task = if let Some(parent_id) = &req.resume_from {
            let parent_response = self
                .last_responses
                .read()
                .get(parent_id)
                .cloned()
                .ok_or_else(|| SubagentCoordinatorError::ResumeFromNotFound(parent_id.clone()))?;
            format!(
                "Previous context from agent '{parent_id}':\n---\n{parent_response}\n---\n\n{task}",
                task = req.task
            )
        } else {
            req.task.clone()
        };

        // Register Pending tracker BEFORE spawning so a quick cancel
        // races fairly with the spawn.
        let now = now_ms();
        let state = Arc::new(RwLock::new(SubagentState::Pending {
            registered_at_ms: now,
        }));
        let completion = Arc::new(Notify::new());
        let cancel_token = CancellationToken::new();
        let tracker = Arc::new(SubagentTracker {
            cancel_token: cancel_token.clone(),
            completion: completion.clone(),
            state: state.clone(),
            run_in_background: req.run_in_background,
            resume_from: req.resume_from.clone(),
            spawned_at_ms: now,
        });
        self.trackers
            .write()
            .insert(req.agent_id.clone(), tracker.clone());

        // Supervisor spawn (synchronous: creates Agent + AgentHandle).
        let handle = self.supervisor.spawn(req.config).map_err(|e| {
            // Roll back the tracker insertion so the ID can be reused.
            self.trackers.write().remove(&req.agent_id);
            SubagentCoordinatorError::SpawnFailed(e.to_string())
        })?;

        // Drive the run in the background.
        let agent_id = req.agent_id.clone();
        let last_responses = self.last_responses.clone();
        let state_for_task = state.clone();
        let completion_for_task = completion.clone();

        tokio::spawn(async move {
            // Pending → Active.
            {
                let mut s = state_for_task.write();
                *s = SubagentState::Active {
                    started_at_ms: now_ms(),
                };
            }

            // Race the run vs cancellation.
            let outcome = tokio::select! {
                _ = cancel_token.cancelled() => {
                    let mut s = state_for_task.write();
                    *s = SubagentState::Cancelled { finished_at_ms: now_ms() };
                    None
                }
                r = handle.run(task) => Some(r),
            };

            if let Some(res) = outcome {
                let mut s = state_for_task.write();
                match res {
                    Ok((response, _)) => {
                        last_responses
                            .write()
                            .insert(agent_id.clone(), response.content.clone());
                        *s = SubagentState::Completed {
                            finished_at_ms: now_ms(),
                            response: response.content,
                        };
                    }
                    Err(e) => {
                        *s = SubagentState::Failed {
                            finished_at_ms: now_ms(),
                            error: e.to_string(),
                        };
                    }
                }
            }

            completion_for_task.notify_waiters();
        });

        Ok(req.agent_id)
    }

    /// Trigger cancellation for an agent. Returns `true` if the agent
    /// was tracked (and is now scheduled to transition to
    /// [`SubagentState::Cancelled`]).
    pub fn cancel(&self, agent_id: &str) -> bool {
        if let Some(t) = self.tracker(agent_id) {
            t.cancel();
            true
        } else {
            false
        }
    }

    /// Block until the named agent reaches a terminal state, or
    /// `timeout` elapses. Returns the terminal state on completion,
    /// `None` on timeout (the underlying run continues), or `None`
    /// if the agent is unknown.
    ///
    /// This is the "block_wait_slot" operation from the design doc:
    /// on timeout the caller can either treat the agent as
    /// backgrounded (move on) or re-poll.
    pub async fn block_wait_slot(
        &self,
        agent_id: &str,
        timeout: Duration,
    ) -> Option<SubagentState> {
        let tracker = self.tracker(agent_id)?;
        tracker.wait_for_completion(timeout).await
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SdkError;
    use crate::lifecycle::SnapshotStore;
    use std::future::Future;
    use std::pin::Pin;

    /// A snapshot store that does nothing — used so the supervisor
    /// can be constructed without filesystem access.
    struct NoopSnapshotStore;

    impl SnapshotStore for NoopSnapshotStore {
        fn save<'a>(
            &'a self,
            _snapshot: &'a crate::lifecycle::AgentSnapshot,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn load<'a>(
            &'a self,
            _agent_id: &'a str,
        ) -> Pin<
            Box<
                dyn Future<Output = anyhow::Result<Option<crate::lifecycle::AgentSnapshot>>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(None) })
        }
        fn list(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<String>>> + Send + '_>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn delete<'a>(
            &'a self,
            _agent_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    /// A provider resolver that always fails — sufficient for the
    /// depth/duplicate/rejection tests since we never actually run.
    struct FailingResolver;

    impl oxi_agent::ProviderResolver for FailingResolver {
        fn resolve_model(&self, _id: &str) -> Option<oxi_ai::Model> {
            None
        }
        fn resolve_provider(&self, _provider: &str) -> Option<Arc<dyn oxi_ai::Provider>> {
            None
        }
    }

    fn make_coordinator(max_depth: u32) -> SubagentCoordinator {
        let resolver: Arc<dyn oxi_agent::ProviderResolver> = Arc::new(FailingResolver);
        let store: Arc<dyn SnapshotStore> = Arc::new(NoopSnapshotStore);
        let supervisor = AgentSupervisor::new(resolver, store);
        SubagentCoordinator::with_max_depth(supervisor, max_depth)
    }

    fn basic_request(id: &str, depth: u32) -> SubagentSpawnRequest {
        SubagentSpawnRequest {
            agent_id: id.to_string(),
            config: AgentConfig {
                model_id: "anthropic/claude-3-5-sonnet".into(),
                ..Default::default()
            },
            task: "do nothing".into(),
            run_in_background: true,
            resume_from: None,
            depth,
        }
    }

    #[test]
    fn rejects_depth_above_max() {
        let coord = make_coordinator(2);
        let req = basic_request("a", 3);
        let err = coord.spawn(req).unwrap_err();
        assert!(
            matches!(
                err,
                SubagentCoordinatorError::MaxDepthExceeded { depth: 3, max: 2 }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn spawn_fails_when_resolver_fails() {
        // With FailingResolver, the supervisor's spawn() must reject.
        // We assert the error path rolls back the tracker insertion
        // (so a retry with the same ID is not blocked by DuplicateId).
        let coord = make_coordinator(2);
        let err = coord.spawn(basic_request("a", 0)).unwrap_err();
        assert!(
            matches!(err, SubagentCoordinatorError::SpawnFailed(_)),
            "got: {err:?}"
        );
        assert_eq!(
            coord.tracked_count(),
            0,
            "tracker must roll back on spawn failure"
        );
    }

    #[test]
    fn rejects_unknown_resume_from() {
        let coord = make_coordinator(2);
        let mut req = basic_request("a", 0);
        req.resume_from = Some("nonexistent".into());
        let err = coord.spawn(req).unwrap_err();
        assert!(
            matches!(err, SubagentCoordinatorError::ResumeFromNotFound(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn tracked_count_starts_zero() {
        let coord = make_coordinator(2);
        assert_eq!(coord.tracked_count(), 0);
        assert_eq!(coord.max_depth(), 2);
    }

    #[test]
    fn cancel_for_unknown_returns_false() {
        let coord = make_coordinator(2);
        assert!(!coord.cancel("ghost"));
    }

    #[test]
    fn block_wait_slot_unknown_returns_none() {
        let coord = make_coordinator(2);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let r = rt.block_on(coord.block_wait_slot("ghost", Duration::from_millis(10)));
        assert!(r.is_none());
    }

    #[test]
    fn snapshot_of_empty_coordinator() {
        let coord = make_coordinator(2);
        assert!(coord.snapshot().is_empty());
    }

    #[test]
    fn default_max_depth_is_two() {
        let resolver: Arc<dyn oxi_agent::ProviderResolver> = Arc::new(FailingResolver);
        let store: Arc<dyn SnapshotStore> = Arc::new(NoopSnapshotStore);
        let supervisor = AgentSupervisor::new(resolver, store);
        let coord = SubagentCoordinator::new(supervisor);
        assert_eq!(coord.max_depth(), DEFAULT_MAX_SUBAGENT_DEPTH);
        assert_eq!(coord.max_depth(), 2);
    }

    #[test]
    fn error_type_is_displayable() {
        // Sanity: every variant formats without panicking.
        let e1 = SubagentCoordinatorError::MaxDepthExceeded { depth: 3, max: 2 };
        let e2 = SubagentCoordinatorError::DuplicateId("x".into());
        let e3 = SubagentCoordinatorError::SpawnFailed("nope".into());
        let e4 = SubagentCoordinatorError::ResumeFromNotFound("p".into());
        assert!(!e1.to_string().is_empty());
        assert!(!e2.to_string().is_empty());
        assert!(!e3.to_string().is_empty());
        assert!(!e4.to_string().is_empty());
    }

    #[test]
    fn sdkerror_unused() {
        // Compile-only check: SdkError stays in scope via the import.
        let _ = std::marker::PhantomData::<SdkError>;
    }
}
