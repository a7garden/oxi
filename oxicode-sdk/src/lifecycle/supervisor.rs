//! Agent supervisor — manages a pool of agents with spawn / resume / policy.
//!
//! Also contains `AgentHandle`: the per-agent lifecycle handle wrapping
//! `Arc<Agent>` with atomic status transitions.

use crate::error::{SdkError, SdkResult};
use crate::lifecycle::snapshot::SnapshotStore;
use crate::lifecycle::{AgentLifecycleEvent, AgentSnapshot, AgentStatus, MetricsSnapshot};
use crate::routing::RoutingControl;
use oxicode_agent::{AgentConfig, AgentTool, ProviderResolver, ToolRegistry};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::broadcast;

// ── Internal status encoding (fits in AtomicU8) ──────────────────────────

const STATUS_CREATED: u8 = 0;
const STATUS_RUNNING: u8 = 1;
const STATUS_SUSPENDED: u8 = 2;
const STATUS_TERMINATED: u8 = 3;
const STATUS_FAILED: u8 = 4;

fn u8_to_status(v: u8) -> AgentStatus {
    match v {
        STATUS_CREATED => AgentStatus::Created,
        STATUS_RUNNING => AgentStatus::Running,
        STATUS_SUSPENDED => AgentStatus::Suspended,
        STATUS_TERMINATED => AgentStatus::Terminated,
        _ => AgentStatus::Failed,
    }
}

// ── SupervisorPolicy ─────────────────────────────────────────────────────

/// Supervisor restart policy.
#[derive(Debug, Clone)]
pub struct SupervisorPolicy {
    /// Max restart attempts within the window.
    pub max_restarts: usize,
    /// Time window for counting restarts (seconds).
    pub restart_window_secs: u64,
    /// Backoff strategy.
    pub backoff: RestartBackoff,
}

impl Default for SupervisorPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            restart_window_secs: 60,
            backoff: RestartBackoff::Exponential {
                base_ms: 1000,
                max_ms: 30_000,
            },
        }
    }
}

impl SupervisorPolicy {
    /// No automatic restarts.
    pub fn no_restart() -> Self {
        Self {
            max_restarts: 0,
            restart_window_secs: 0,
            backoff: RestartBackoff::None,
        }
    }
}

/// Restart backoff strategy.
#[derive(Debug, Clone)]
pub enum RestartBackoff {
    /// No delay.
    None,
    /// Fixed delay.
    Fixed {
        /// Delay before restarting, in milliseconds.
        delay_ms: u64,
    },
    /// Exponential with cap.
    Exponential {
        /// Initial delay before restarting, in milliseconds.
        base_ms: u64,
        /// Upper bound on the exponential delay, in milliseconds.
        max_ms: u64,
    },
}

// ── AgentHandle ──────────────────────────────────────────────────────────

/// Agent execution handle returned by `AgentSupervisor::spawn()`.
///
/// Wraps `Arc<Agent>` with lifecycle state management.
/// Thread-safe: status tracked via atomic, cancel via shared flag.
#[derive(Clone)]
pub struct AgentHandle {
    agent_id: String,
    status: Arc<AtomicU8>,
    agent: Arc<oxicode_agent::Agent>,
    config: Arc<RwLock<AgentConfig>>,
    metrics: Arc<crate::metrics::AgentMetrics>,
    lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    created_at_ms: u64,
    parent_id: Option<String>,
    routing: RoutingControl,
}

impl AgentHandle {
    /// Create a new handle wrapping an agent.
    pub(crate) fn new(
        agent: oxicode_agent::Agent,
        config: AgentConfig,
        parent_id: Option<String>,
        lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    ) -> Self {
        let routing = RoutingControl::new(crate::routing::RoutingConfig::default());
        Self {
            agent_id: if config.name.is_empty() {
                uuid::Uuid::new_v4().to_string()
            } else {
                config.name.clone()
            },
            status: Arc::new(AtomicU8::new(STATUS_CREATED)),
            agent: Arc::new(agent),
            config: Arc::new(RwLock::new(config)),
            metrics: Arc::new(crate::metrics::AgentMetrics::new()),
            lifecycle_tx,
            created_at_ms: AgentLifecycleEvent::now_ms(),
            parent_id,
            routing,
        }
    }

    // ── Accessors ──────────────────────────────────────────

    /// Agent identifier.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Parent agent ID (for delegation lineage).
    pub fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    /// Creation timestamp (ms since epoch).
    pub fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }

    /// Current metrics snapshot.
    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Whether the agent is currently running.
    pub fn is_running(&self) -> bool {
        self.status() == AgentStatus::Running
    }

    /// Current lifecycle status.
    pub fn status(&self) -> AgentStatus {
        u8_to_status(self.status.load(Ordering::SeqCst))
    }

    // ── Execution ─────────────────────────────────────────

    /// Run the agent with a prompt.
    ///
    /// Transitions `Created`/`Suspended` → `Running` → `Created` (on success)
    /// or `Failed` (on error).
    pub async fn run(
        &self,
        prompt: String,
    ) -> SdkResult<(
        oxicode_agent::types::Response,
        Vec<oxicode_agent::AgentEvent>,
    )> {
        // CAS: Created → Running or Suspended → Running
        let prev = self
            .status
            .compare_exchange(
                STATUS_CREATED,
                STATUS_RUNNING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .or_else(|_| {
                self.status.compare_exchange(
                    STATUS_SUSPENDED,
                    STATUS_RUNNING,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
            });

        if prev.is_err() {
            return Err(SdkError::AgentNotRunnable {
                agent_id: self.agent_id.clone(),
                status: self.status().to_string(),
            });
        }

        self.emit(AgentLifecycleEvent::RunStart {
            agent_id: self.agent_id.clone(),
            timestamp_ms: AgentLifecycleEvent::now_ms(),
        });

        let start = std::time::Instant::now();
        let result = self.agent.run(prompt).await;
        let elapsed = start.elapsed();

        match result {
            Ok((response, events)) => {
                let agent_state = self.agent.state();
                let input_tokens = agent_state.input_tokens as u64;
                let output_tokens = agent_state.output_tokens as u64;
                let tool_count = events
                    .iter()
                    .filter(|e| matches!(e, oxicode_agent::AgentEvent::ToolExecutionStart { .. }))
                    .count() as u64;
                self.metrics.record_success(
                    elapsed.as_millis() as u64,
                    input_tokens,
                    output_tokens,
                    tool_count,
                );
                self.transition(STATUS_CREATED);
                self.emit(AgentLifecycleEvent::RunEnd {
                    agent_id: self.agent_id.clone(),
                    timestamp_ms: AgentLifecycleEvent::now_ms(),
                    success: true,
                });
                Ok((response, events))
            }
            Err(e) => {
                self.transition(STATUS_FAILED);
                self.emit(AgentLifecycleEvent::RunEnd {
                    agent_id: self.agent_id.clone(),
                    timestamp_ms: AgentLifecycleEvent::now_ms(),
                    success: false,
                });
                Err(SdkError::ExecutionFailed {
                    reason: e.to_string(),
                })
            }
        }
    }

    /// Continue the conversation with a follow-up prompt.
    ///
    /// Equivalent to `run()` — the underlying agent maintains conversation history.
    pub async fn continue_with(
        &self,
        prompt: String,
    ) -> SdkResult<(
        oxicode_agent::types::Response,
        Vec<oxicode_agent::AgentEvent>,
    )> {
        self.run(prompt).await
    }

    /// Request cancellation of the current run.
    pub fn cancel(&self) {
        self.agent.cancel();
    }

    // ── Lifecycle ─────────────────────────────────────────

    /// Suspend the agent and create a checkpoint snapshot.
    ///
    /// Transitions `Created`/`Running` → `Suspended`.
    pub async fn suspend(&self) -> SdkResult<AgentSnapshot> {
        let cur = self.status();
        if !cur.is_runnable() && cur != AgentStatus::Running {
            return Err(SdkError::AgentNotRunnable {
                agent_id: self.agent_id.clone(),
                status: cur.to_string(),
            });
        }

        // Cancel running work first
        if cur == AgentStatus::Running {
            self.cancel();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let snapshot = AgentSnapshot::from_agent(
            self.agent_id.clone(),
            &self.config.read(),
            &self.agent.state(),
            &self.agent.tools(),
            self.parent_id.clone(),
            HashMap::new(),
        );

        self.transition(STATUS_SUSPENDED);
        self.emit(AgentLifecycleEvent::Suspended {
            agent_id: self.agent_id.clone(),
            snapshot: Box::new(snapshot.clone()),
            timestamp_ms: AgentLifecycleEvent::now_ms(),
        });

        Ok(snapshot)
    }

    /// Terminate the agent permanently (terminal state).
    pub fn terminate(&self) -> SdkResult<()> {
        if self.status().is_terminal() {
            return Err(SdkError::AgentNotRunnable {
                agent_id: self.agent_id.clone(),
                status: self.status().to_string(),
            });
        }
        self.transition(STATUS_TERMINATED);
        self.emit(AgentLifecycleEvent::Terminated {
            agent_id: self.agent_id.clone(),
            timestamp_ms: AgentLifecycleEvent::now_ms(),
        });
        Ok(())
    }

    /// Take a snapshot without changing state.
    pub fn snapshot(&self) -> SdkResult<AgentSnapshot> {
        Ok(AgentSnapshot::from_agent(
            self.agent_id.clone(),
            &self.config.read(),
            &self.agent.state(),
            &self.agent.tools(),
            self.parent_id.clone(),
            HashMap::new(),
        ))
    }

    // ── Dynamic configuration ──────────────────────────────

    /// Switch model mid-conversation.
    ///
    /// The new provider is re-credentialed via the resolver; the old
    /// `api_key` parameter was removed in 0.55.0 (issues #39/#40).
    pub fn switch_model(&self, model_id: &str) -> anyhow::Result<()> {
        let old = self.config.read().model_id.clone();
        self.agent.switch_model(model_id)?;
        self.config.write().model_id = model_id.to_string();
        self.emit(AgentLifecycleEvent::ModelSwitched {
            agent_id: self.agent_id.clone(),
            from_model: old,
            to_model: model_id.to_string(),
            timestamp_ms: AgentLifecycleEvent::now_ms(),
        });
        Ok(())
    }

    /// Update system prompt for future runs.
    pub fn set_system_prompt(&self, prompt: String) {
        self.config.write().system_prompt = Some(prompt.clone());
        self.agent.set_system_prompt(prompt);
    }

    /// Register a tool at runtime.
    pub fn add_tool(&self, tool: impl AgentTool + 'static) {
        self.agent.add_tool(tool);
    }

    // ── Runtime routing ─────────────────────────────────────

    /// Get the runtime routing control for this agent.
    ///
    /// Allows dynamic enabling/disabling of routing, model exclusion,
    /// and fallback model management.
    pub fn routing(&self) -> &RoutingControl {
        &self.routing
    }

    /// Convenience: disable routing.
    pub fn disable_routing(&self) {
        self.routing.set_enabled(false);
    }

    /// Convenience: enable routing.
    pub fn enable_routing(&self) {
        self.routing.set_enabled(true);
    }

    /// Convenience: exclude a model from routing.
    pub fn exclude_route_model(&self, model_id: &str) {
        self.routing.exclude_model(model_id);
    }

    // ── Internal ──────────────────────────────────────────

    fn transition(&self, new_status: u8) {
        self.status.store(new_status, Ordering::SeqCst);
    }

    fn emit(&self, event: AgentLifecycleEvent) {
        let _ = self.lifecycle_tx.send(event);
    }
}

impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("agent_id", &self.agent_id)
            .field("status", &self.status())
            .finish()
    }
}

// ── AgentSupervisor ──────────────────────────────────────────────────────

/// Manages a pool of agents with lifecycle operations.
///
/// Responsibilities:
/// - Spawn / terminate agents
/// - Persist snapshots via `SnapshotStore`
/// - Broadcast lifecycle events
/// - Supervise: auto-restart on failure (configurable)
#[derive(Clone)]
pub struct AgentSupervisor {
    agents: Arc<RwLock<HashMap<String, AgentHandle>>>,
    lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    snapshot_store: Arc<dyn SnapshotStore>,
    policy: SupervisorPolicy,
    /// Tracks restart timestamps per agent for window enforcement.
    restart_log: Arc<RwLock<HashMap<String, Vec<u64>>>>,
    resolver: Arc<dyn ProviderResolver>,
    /// Strong reference to the Oxicode engine, used to route spawns
    /// through [`crate::AgentBuilder`] when an agent decorator is
    /// configured. `None` when the supervisor was constructed
    /// without one (legacy / direct callers).
    oxicode: Option<Arc<crate::Oxicode>>,
    /// Cross-cutting decorator applied to every spawned agent when
    /// `oxicode` is `Some`. Ignored on the fast path (no decorator).
    agent_decorator: Option<Arc<dyn crate::observability::AgentDecorator>>,
}

impl AgentSupervisor {
    /// Create a new supervisor.
    pub fn new(
        resolver: Arc<dyn ProviderResolver>,
        snapshot_store: Arc<dyn SnapshotStore>,
    ) -> Self {
        Self::with_policy(resolver, snapshot_store, SupervisorPolicy::default())
    }
    /// Create with a specific restart policy.
    pub fn with_policy(
        resolver: Arc<dyn ProviderResolver>,
        snapshot_store: Arc<dyn SnapshotStore>,
        policy: SupervisorPolicy,
    ) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            lifecycle_tx: tx,
            snapshot_store,
            policy,
            restart_log: Arc::new(RwLock::new(HashMap::new())),
            resolver,
            oxicode: None,
            agent_decorator: None,
        }
    }

    /// Attach an [`crate::Oxicode`] reference and an [`crate::observability::AgentDecorator`]
    /// to this supervisor. Subsequent [`Self::spawn`] calls will
    /// route through `Oxicode::agent(config)` + `decorator.decorate()`
    /// instead of the bare `Agent::new()` fast path, so the
    /// decorator's audit / authorizer / tracer / cost hooks
    /// actually run on every spawned agent.
    ///
    /// Both must be supplied together — the decorator is only
    /// effective when the supervisor has an `Oxicode` to bind the
    /// builder against.
    pub fn with_agent_decorator(
        mut self,
        oxicode: Arc<crate::Oxicode>,
        decorator: Arc<dyn crate::observability::AgentDecorator>,
    ) -> Self {
        self.oxicode = Some(oxicode);
        self.agent_decorator = Some(decorator);
        self
    }

    /// Subscribe to lifecycle events from all agents.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentLifecycleEvent> {
        self.lifecycle_tx.subscribe()
    }

    // ── Agent management ──────────────────────────────────

    /// Spawn a new agent.
    ///
    /// When the supervisor was configured with both an `Oxicode` reference
    /// and an [`crate::observability::AgentDecorator`] (via
    /// [`with_agent_decorator`](Self::with_agent_decorator) or
    /// [`crate::builder::SupervisorBuilder::with_agent_decorator`]), this routes
    /// through `Oxicode::agent(config)` and lets the decorator apply
    /// audit / authorizer / tracer / cost hooks before `.build()`.
    /// Otherwise it takes the legacy fast path
    /// (`Agent::new(provider, config, tools)`), which leaves
    /// observability unset.
    pub fn spawn(&self, config: AgentConfig) -> anyhow::Result<AgentHandle> {
        let agent = if let (Some(oxicode), Some(decorator)) =
            (self.oxicode.as_ref(), self.agent_decorator.as_ref())
        {
            let builder = oxicode.agent(config.clone());
            let builder = decorator.decorate(builder);
            builder.build()?
        } else {
            let model = self
                .resolver
                .resolve_model(&config.model_id)
                .ok_or_else(|| SdkError::ModelNotFound {
                    model_id: config.model_id.clone(),
                })?;
            let provider = self
                .resolver
                .resolve_provider(&model.provider)
                .ok_or_else(|| SdkError::ProviderNotFound {
                    provider: model.provider.clone(),
                })?;
            let tools = Arc::new(ToolRegistry::new());
            oxicode_agent::Agent::new(provider, config.clone(), tools)
        };

        let handle = AgentHandle::new(agent, config.clone(), None, self.lifecycle_tx.clone());

        self.agents
            .write()
            .insert(handle.agent_id().to_string(), handle.clone());

        self.emit(AgentLifecycleEvent::Spawned {
            agent_id: handle.agent_id().to_string(),
            parent_id: None,
            model_id: config.model_id.clone(),
            timestamp_ms: AgentLifecycleEvent::now_ms(),
        });

        Ok(handle)
    }

    /// Spawn a child agent linked to a parent (delegation lineage).
    pub fn spawn_child(&self, parent_id: &str, config: AgentConfig) -> anyhow::Result<AgentHandle> {
        let model = self
            .resolver
            .resolve_model(&config.model_id)
            .ok_or_else(|| SdkError::ModelNotFound {
                model_id: config.model_id.clone(),
            })?;
        let provider = self
            .resolver
            .resolve_provider(&model.provider)
            .ok_or_else(|| SdkError::ProviderNotFound {
                provider: model.provider.clone(),
            })?;

        let tools = Arc::new(ToolRegistry::new());
        let agent = oxicode_agent::Agent::new(provider, config.clone(), tools);

        let handle = AgentHandle::new(
            agent,
            config.clone(),
            Some(parent_id.to_string()),
            self.lifecycle_tx.clone(),
        );

        self.agents
            .write()
            .insert(handle.agent_id().to_string(), handle.clone());

        self.emit(AgentLifecycleEvent::Spawned {
            agent_id: handle.agent_id().to_string(),
            parent_id: Some(parent_id.to_string()),
            model_id: config.model_id.clone(),
            timestamp_ms: AgentLifecycleEvent::now_ms(),
        });

        Ok(handle)
    }

    /// Get a handle by agent ID.
    pub fn get(&self, agent_id: &str) -> Option<AgentHandle> {
        self.agents.read().get(agent_id).cloned()
    }

    /// List all agents and their status.
    pub fn list(&self) -> Vec<(String, AgentStatus)> {
        self.agents
            .read()
            .iter()
            .map(|(id, h)| (id.clone(), h.status()))
            .collect()
    }

    /// Count agents by status.
    pub fn count_by_status(&self) -> HashMap<AgentStatus, usize> {
        let mut counts = HashMap::new();
        for handle in self.agents.read().values() {
            *counts.entry(handle.status()).or_insert(0) += 1;
        }
        counts
    }

    // ── Persistence ───────────────────────────────────────

    /// Suspend and persist snapshot.
    pub async fn suspend(&self, agent_id: &str) -> anyhow::Result<AgentSnapshot> {
        let handle = self
            .get(agent_id)
            .ok_or_else(|| SdkError::SnapshotNotFound {
                agent_id: agent_id.to_string(),
            })?;
        let snapshot = handle.suspend().await?;
        self.snapshot_store.save(&snapshot).await?;
        Ok(snapshot)
    }

    /// Restore agent from persisted snapshot.
    pub async fn restore(&self, agent_id: &str) -> anyhow::Result<AgentHandle> {
        // Check if already in pool
        if let Some(handle) = self.get(agent_id) {
            return Ok(handle);
        }

        let snapshot = self.snapshot_store.load(agent_id).await?.ok_or_else(|| {
            SdkError::SnapshotNotFound {
                agent_id: agent_id.to_string(),
            }
        })?;

        self.restore_from_snapshot(snapshot).await
    }

    /// Restore from an in-memory snapshot.
    pub async fn restore_from_snapshot(
        &self,
        snapshot: AgentSnapshot,
    ) -> anyhow::Result<AgentHandle> {
        let model = self
            .resolver
            .resolve_model(&snapshot.config.model_id)
            .ok_or_else(|| SdkError::ModelNotFound {
                model_id: snapshot.config.model_id.clone(),
            })?;
        let provider = self
            .resolver
            .resolve_provider(&model.provider)
            .ok_or_else(|| SdkError::ProviderNotFound {
                provider: model.provider.clone(),
            })?;

        let tools = Arc::new(ToolRegistry::new());
        let agent = oxicode_agent::Agent::new(provider, snapshot.config.clone(), tools);

        // Restore conversation state
        let state_json = serde_json::to_value(&snapshot.state)?;
        agent.import_state(state_json)?;

        let handle = AgentHandle::new(
            agent,
            snapshot.config.clone(),
            snapshot.parent_id.clone(),
            self.lifecycle_tx.clone(),
        );

        self.agents
            .write()
            .insert(handle.agent_id().to_string(), handle.clone());

        self.emit(AgentLifecycleEvent::Resumed {
            agent_id: handle.agent_id().to_string(),
            from_snapshot_id: Some(snapshot.agent_id.clone()),
            timestamp_ms: AgentLifecycleEvent::now_ms(),
        });

        Ok(handle)
    }

    /// Terminate an agent and remove from the pool.
    pub fn terminate(&self, agent_id: &str) -> anyhow::Result<()> {
        let handle = self
            .get(agent_id)
            .ok_or_else(|| SdkError::SnapshotNotFound {
                agent_id: agent_id.to_string(),
            })?;

        if handle.is_running() {
            return Err(SdkError::AgentNotRunnable {
                agent_id: agent_id.to_string(),
                status: "running".to_string(),
            }
            .into());
        }

        handle.terminate()?;
        self.agents.write().remove(agent_id);
        self.restart_log.write().remove(agent_id);

        Ok(())
    }

    // ── Auto-restart ───────────────────────────────────────

    /// Check whether an agent can be auto-restarted based on the policy.
    ///
    /// Returns `true` if:
    /// - `max_restarts > 0`
    /// - The number of restarts within the window is less than `max_restarts`
    pub fn can_restart(&self, agent_id: &str) -> bool {
        if self.policy.max_restarts == 0 {
            return false;
        }
        let now = AgentLifecycleEvent::now_ms();
        let window_ms = self.policy.restart_window_secs * 1000;
        let log = self.restart_log.read();
        let restarts = log
            .get(agent_id)
            .map(|ts| {
                ts.iter()
                    .filter(|&&t| now.saturating_sub(t) <= window_ms)
                    .count()
            })
            .unwrap_or(0);
        restarts < self.policy.max_restarts
    }

    /// Restart a failed agent with the same config.
    ///
    /// Records the restart in the restart log and applies backoff delay.
    /// Returns the new handle on success.
    pub async fn restart(&self, agent_id: &str) -> SdkResult<AgentHandle> {
        if !self.can_restart(agent_id) {
            return Err(SdkError::InvalidState {
                entity: "agent".into(),
                reason: format!(
                    "agent '{}' exceeded max restarts ({})",
                    agent_id, self.policy.max_restarts
                ),
            });
        }

        // Get the old handle's config
        let old = self.agents.read().get(agent_id).cloned();
        let config = match &old {
            Some(h) => h.config.read().clone(),
            None => {
                return Err(SdkError::SnapshotNotFound {
                    agent_id: agent_id.to_string(),
                });
            }
        };

        // Apply backoff delay
        if let Some(delay) = self.compute_backoff(agent_id) {
            tokio::time::sleep(delay).await;
        }

        // Record the restart
        self.restart_log
            .write()
            .entry(agent_id.to_string())
            .or_default()
            .push(AgentLifecycleEvent::now_ms());

        // Remove old handle
        self.agents.write().remove(agent_id);

        // Spawn fresh agent with the same config
        self.spawn(config).map_err(SdkError::from)
    }

    /// Compute backoff duration for a given agent based on restart history.
    fn compute_backoff(&self, agent_id: &str) -> Option<std::time::Duration> {
        let count = self
            .restart_log
            .read()
            .get(agent_id)
            .map(|ts| ts.len())
            .unwrap_or(0);
        if count == 0 {
            return None;
        }
        match &self.policy.backoff {
            RestartBackoff::None => None,
            RestartBackoff::Fixed { delay_ms } => Some(std::time::Duration::from_millis(*delay_ms)),
            RestartBackoff::Exponential { base_ms, max_ms } => {
                let delay = (*base_ms).saturating_mul(2u64.saturating_pow(count as u32));
                Some(std::time::Duration::from_millis(delay.min(*max_ms)))
            }
        }
    }

    // ── Internal ──────────────────────────────────────────

    fn emit(&self, event: AgentLifecycleEvent) {
        let _ = self.lifecycle_tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    // ── Mocks ──────────────────────────────────────────────

    fn mock_resolver() -> Arc<dyn ProviderResolver> {
        struct MockProvider;
        impl oxicode_ai::Provider for MockProvider {
            fn stream<'a>(
                &'a self,
                _model: &'a oxicode_ai::Model,
                _context: &'a oxicode_ai::Context,
                _options: Option<oxicode_ai::StreamOptions>,
            ) -> Pin<Box<dyn Future<Output = oxicode_ai::StreamResult> + Send + 'a>> {
                Box::pin(
                    async move { Err(oxicode_ai::ProviderError::NotImplemented("mock".into())) },
                )
            }
        }

        struct Mock;
        impl ProviderResolver for Mock {
            fn resolve_provider(&self, _name: &str) -> Option<Arc<dyn oxicode_ai::Provider>> {
                Some(Arc::new(MockProvider))
            }
            fn resolve_model(&self, _model_id: &str) -> Option<oxicode_ai::Model> {
                Some(oxicode_ai::Model::new(
                    "anthropic/claude-sonnet-4-20250514",
                    "Claude",
                    oxicode_ai::Api::AnthropicMessages,
                    "anthropic",
                    "https://api.anthropic.com",
                ))
            }
        }
        Arc::new(Mock)
    }

    struct NoopStore;

    impl SnapshotStore for NoopStore {
        fn save<'a>(
            &'a self,
            _snapshot: &'a AgentSnapshot,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn load<'a>(
            &'a self,
            _agent_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<AgentSnapshot>>> + Send + 'a>>
        {
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

    fn make_supervisor() -> AgentSupervisor {
        AgentSupervisor::new(
            mock_resolver(),
            Arc::new(NoopStore) as Arc<dyn SnapshotStore>,
        )
    }

    fn test_config() -> AgentConfig {
        AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            name: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        }
    }

    // ── Tests ──────────────────────────────────────────────

    #[test]
    fn supervisor_policy_default() {
        let policy = SupervisorPolicy::default();
        assert_eq!(policy.max_restarts, 3);
        assert!(matches!(policy.backoff, RestartBackoff::Exponential { .. }));
    }

    #[test]
    fn supervisor_policy_no_restart() {
        let policy = SupervisorPolicy::no_restart();
        assert_eq!(policy.max_restarts, 0);
        assert!(matches!(policy.backoff, RestartBackoff::None));
    }

    #[test]
    fn supervisor_spawn_and_get() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        assert!(!handle.agent_id().is_empty());
        assert_eq!(handle.status(), AgentStatus::Created);
        assert_eq!(handle.parent_id(), None);
    }

    #[test]
    fn supervisor_spawn_child() {
        let supervisor = make_supervisor();
        let parent = supervisor.spawn(test_config()).unwrap();
        let child = supervisor
            .spawn_child(parent.agent_id(), test_config())
            .unwrap();
        assert_eq!(child.parent_id(), Some(parent.agent_id()));
    }

    #[test]
    fn supervisor_terminate() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        let id = handle.agent_id().to_string();
        supervisor.terminate(&id).unwrap();
        assert!(supervisor.get(&id).is_none());
    }

    #[test]
    fn supervisor_list_and_count() {
        let supervisor = make_supervisor();
        supervisor.spawn(test_config()).unwrap();
        supervisor.spawn(test_config()).unwrap();

        let list = supervisor.list();
        assert_eq!(list.len(), 2);

        let counts = supervisor.count_by_status();
        assert_eq!(counts.get(&AgentStatus::Created), Some(&2));
    }

    #[test]
    fn handle_status_transitions() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();

        // Created → Terminated
        handle.terminate().unwrap();
        assert_eq!(handle.status(), AgentStatus::Terminated);
        assert!(handle.status().is_terminal());

        // Cannot terminate again
        assert!(handle.terminate().is_err());
    }

    #[test]
    fn handle_switch_model() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        // Will fail because provider doesn't actually exist, but tests the wiring
        let result = handle.switch_model("openai/gpt-4o");
        // Provider resolution happens at run time via agent, so this should propagate
        // the agent's error. We just check the method exists and compiles.
        let _ = result;
    }

    #[test]
    fn handle_set_system_prompt() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        handle.set_system_prompt("You are a test agent.".into());
        // Verify the config was updated
        assert_eq!(
            handle.config.read().system_prompt,
            Some("You are a test agent.".into())
        );
    }

    #[test]
    fn handle_snapshot() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        let snap = handle.snapshot().unwrap();
        assert_eq!(snap.agent_id, handle.agent_id());
    }

    #[test]
    fn lifecycle_events_received() {
        let supervisor = make_supervisor();
        let mut rx = supervisor.subscribe();
        supervisor.spawn(test_config()).unwrap();

        let event = rx.try_recv().expect("should receive Spawned event");
        match event {
            AgentLifecycleEvent::Spawned { agent_id, .. } => {
                assert!(!agent_id.is_empty());
            }
            _ => panic!("Expected Spawned event"),
        }
    }

    // ── New feature tests ──────────────────────────────────

    #[test]
    fn handle_has_routing_control() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        // Default: routing enabled
        assert!(handle.routing().is_enabled());
    }

    #[test]
    fn handle_routing_toggle() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        handle.disable_routing();
        assert!(!handle.routing().is_enabled());
        handle.enable_routing();
        assert!(handle.routing().is_enabled());
    }

    #[test]
    fn handle_routing_exclude_model() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        handle.exclude_route_model("openai/gpt-4o");
        assert!(
            handle
                .routing()
                .excluded_models()
                .contains(&"openai/gpt-4o".to_string())
        );
    }

    #[test]
    fn handle_routing_fallback_models() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        handle
            .routing()
            .set_fallback_models(vec!["anthropic/claude-sonnet-4-20250514".into()]);
        assert_eq!(handle.routing().fallback_models().len(), 1);
    }

    #[test]
    fn supervisor_can_restart_default_policy() {
        let supervisor = make_supervisor();
        let handle = supervisor.spawn(test_config()).unwrap();
        let id = handle.agent_id().to_string();
        // Default policy: max_restarts=3, so can_restart should be true
        assert!(supervisor.can_restart(&id));
    }

    #[test]
    fn supervisor_cannot_restart_no_restart_policy() {
        let supervisor = AgentSupervisor::with_policy(
            mock_resolver(),
            Arc::new(NoopStore) as Arc<dyn SnapshotStore>,
            SupervisorPolicy::no_restart(),
        );
        let handle = supervisor.spawn(test_config()).unwrap();
        let id = handle.agent_id().to_string();
        assert!(!supervisor.can_restart(&id));
    }

    #[tokio::test]
    async fn supervisor_restart_with_no_restart_policy_fails() {
        let supervisor = AgentSupervisor::with_policy(
            mock_resolver(),
            Arc::new(NoopStore) as Arc<dyn SnapshotStore>,
            SupervisorPolicy::no_restart(),
        );
        let handle = supervisor.spawn(test_config()).unwrap();
        let id = handle.agent_id().to_string();
        let result = supervisor.restart(&id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn supervisor_restart_spawns_new_agent() {
        // Use Fixed backoff with 0 delay for fast test
        let policy = SupervisorPolicy {
            max_restarts: 3,
            restart_window_secs: 60,
            backoff: RestartBackoff::Fixed { delay_ms: 0 },
        };
        let supervisor = AgentSupervisor::with_policy(
            mock_resolver(),
            Arc::new(NoopStore) as Arc<dyn SnapshotStore>,
            policy,
        );
        let handle = supervisor.spawn(test_config()).unwrap();
        let old_id = handle.agent_id().to_string();

        let new_handle = supervisor.restart(&old_id).await.unwrap();
        // Restart creates a new handle (same name = same ID, but fresh state)
        assert!(supervisor.get(new_handle.agent_id()).is_some());
        assert_eq!(new_handle.status(), AgentStatus::Created);
        // The restart_log should track the restart
        // Note: if the name is reused, the log is under the original id
        let log = supervisor.restart_log.read();
        assert!(log.values().any(|ts| !ts.is_empty()));
    }

    #[tokio::test]
    async fn supervisor_restart_respects_max_restarts() {
        let policy = SupervisorPolicy {
            max_restarts: 1,
            restart_window_secs: 60,
            backoff: RestartBackoff::None,
        };
        let supervisor = AgentSupervisor::with_policy(
            mock_resolver(),
            Arc::new(NoopStore) as Arc<dyn SnapshotStore>,
            policy,
        );
        let handle = supervisor.spawn(test_config()).unwrap();
        let id = handle.agent_id().to_string();

        // First restart should succeed
        let first = supervisor.restart(&id).await.unwrap();
        let first_id = first.agent_id().to_string();

        // Second restart should fail (max_restarts=1)
        assert!(!supervisor.can_restart(&first_id));
        let result = supervisor.restart(&first_id).await;
        assert!(result.is_err());
    }
}
