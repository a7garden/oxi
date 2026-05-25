# 01. 에이전트 수명 주기 (Agent Lifecycle)

모듈 경로: `oxi-sdk/src/lifecycle/`

---

## 1.1 현황 및 문제

현재 `Agent::run()`은 stateless one-shot 실행:

```rust
// 현재: 생성 → 실행 → 끝. 재사용은 continue_with()로 대화만 이어감
let agent = oxi.agent(config).build()?;
let (response, _) = agent.run(prompt).await?;
```

**문제점:**
- 중단(suspend) / 재개(resume) 불가
- 체크포인트(checkpoint) / 복원(restore) 불가
- 상태 직렬화 불완전 (`export_state`/`import_state`는 있으나 provider/resolver 정보 누락)
- Hot-reload (system prompt, tools 동적 교체) 불가
- 에이전트 간 부모-자식 관계 추적 불가
- Agent OS 커널이 에이전트를 프로세스처럼 관리할 수 없음

---

## 1.2 핵심 타입

### AgentSnapshot

에이전트를 특정 시점에 완전히 직렬화한 스냅샷. 디스크/DB/네트워크 저장 가능.

```rust
// src/lifecycle/snapshot.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Executable snapshot of an agent at a point in time.
///
/// Captures everything needed to resume the agent:
/// - Agent configuration (model, tools, system prompt)
/// - Conversation history (messages, tool results)
/// - Execution state (iteration, token counts, stop_reason)
/// - Metadata (agent_id, created_at, parent_id for lineage)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Unique identifier for this agent instance.
    pub agent_id: String,
    /// Agent configuration at the time of snapshot.
    pub config: AgentConfig,
    /// Serialized conversation state (messages, tokens, iteration).
    pub state: AgentState,
    /// Tool registry snapshot (tool names + schemas, not closures).
    pub tool_manifest: ToolManifest,
    /// Parent agent ID (for delegation lineage tracking).
    pub parent_id: Option<String>,
    /// Creation timestamp (Unix epoch ms).
    pub created_at_ms: u64,
    /// Snapshot timestamp.
    pub snapshot_at_ms: u64,
    /// Execution metrics at snapshot time.
    pub metrics: MetricsSnapshot,
    /// Custom metadata (workspace path, permissions, etc.).
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl AgentSnapshot {
    /// Create a snapshot from a running agent.
    pub fn from_agent(
        agent_id: String,
        config: &AgentConfig,
        state: &AgentState,
        tools: &ToolRegistry,
        parent_id: Option<String>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        let now = now_ms();
        Self {
            agent_id,
            config: config.clone(),
            state: state.clone(),
            tool_manifest: ToolManifest::from_registry(tools),
            parent_id,
            created_at_ms: now,
            snapshot_at_ms: now,
            metrics: MetricsSnapshot::default(),
            metadata,
        }
    }

    /// Serialize to JSON bytes.
    pub fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Deserialize from JSON bytes.
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Estimate serialized size.
    pub fn estimated_size_bytes(&self) -> usize {
        serde_json::to_vec(self).map(|b| b.len()).unwrap_or(0)
    }
}
```

### ToolManifest

Tool 클로저는 직렬화할 수 없으므로, 이름/스키마만 저장 후 복원 시 re-register.

```rust
/// Minimal tool metadata for serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    pub tools: Vec<ToolManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifestEntry {
    pub name: String,
    pub description: String,
    pub input_schema: std::collections::HashMap<String, serde_json::Value>,
    pub essential: bool,
}

impl ToolManifest {
    pub fn from_registry(registry: &ToolRegistry) -> Self {
        Self {
            tools: registry
                .definitions()
                .into_iter()
                .map(|def| ToolManifestEntry {
                    name: def.name,
                    description: def.description,
                    input_schema: def.input_schema,
                    essential: false,
                })
                .collect(),
        }
    }

    /// Check if all manifest tools are present in a registry.
    pub fn missing_from(&self, registry: &ToolRegistry) -> Vec<&str> {
        self.tools
            .iter()
            .map(|t| t.name.as_str())
            .filter(|name| registry.get(name).is_none())
            .collect()
    }
}
```

### AgentStatus

```rust
/// Lifecycle status of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Created but has not started any runs.
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

    /// Whether the agent can accept a new run().
    pub fn is_runnable(&self) -> bool {
        matches!(self, Self::Created | Self::Suspended)
    }
}
```

---

## 1.3 AgentHandle

에이전트의 lifecycle handle. `Agent`를 직접 다루지 않고 상태 전이를 관리.

```rust
// src/lifecycle/mod.rs

/// Agent execution handle returned by `AgentSupervisor::spawn()`.
///
/// Wraps `Arc<Agent>` with lifecycle state management.
/// Thread-safe: status is tracked via atomic, cancel via shared flag.
#[derive(Clone)]
pub struct AgentHandle {
    agent_id: String,
    status: Arc<AtomicU8>,
    agent: Arc<Agent>,
    config: Arc<RwLock<AgentConfig>>,
    metrics: Arc<AgentMetrics>,
    lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    created_at_ms: u64,
    parent_id: Option<String>,
}

// Internal status encoding (fits in AtomicU8)
const STATUS_CREATED: u8 = 0;
const STATUS_RUNNING: u8 = 1;
const STATUS_SUSPENDED: u8 = 2;
const STATUS_TERMINATED: u8 = 3;
const STATUS_FAILED: u8 = 4;

impl AgentHandle {
    pub(crate) fn new(
        agent: Agent,
        config: AgentConfig,
        parent_id: Option<String>,
        lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    ) -> Self {
        Self {
            agent_id: uuid::Uuid::new_v4().to_string(),
            status: Arc::new(AtomicU8::new(STATUS_CREATED)),
            agent: Arc::new(agent),
            config: Arc::new(RwLock::new(config)),
            metrics: Arc::new(AgentMetrics::new()),
            lifecycle_tx,
            created_at_ms: now_ms(),
            parent_id,
        }
    }

    // ── Accessors ──────────────────────────────────────────

    pub fn agent_id(&self) -> &str { &self.agent_id }
    pub fn parent_id(&self) -> Option<&str> { self.parent_id.as_deref() }
    pub fn created_at_ms(&self) -> u64 { self.created_at_ms }
    pub fn metrics(&self) -> MetricsSnapshot { self.metrics.snapshot() }
    pub fn is_running(&self) -> bool { self.status() == AgentStatus::Running }

    pub fn status(&self) -> AgentStatus {
        match self.status.load(Ordering::SeqCst) {
            STATUS_CREATED => AgentStatus::Created,
            STATUS_RUNNING => AgentStatus::Running,
            STATUS_SUSPENDED => AgentStatus::Suspended,
            STATUS_TERMINATED => AgentStatus::Terminated,
            _ => AgentStatus::Failed,
        }
    }

    // ── Execution ─────────────────────────────────────────

    /// Run the agent with a prompt. Transitions Created/Suspended → Running → Created.
    ///
    /// Error if not in a runnable state or if agent is already running.
    pub async fn run(&self, prompt: String) -> anyhow::Result<(Response, Vec<AgentEvent>)> {
        // CAS: Created → Running or Suspended → Running
        let prev = self.status.compare_exchange(
            STATUS_CREATED, STATUS_RUNNING, Ordering::SeqCst, Ordering::SeqCst,
        ).or_else(|_| self.status.compare_exchange(
            STATUS_SUSPENDED, STATUS_RUNNING, Ordering::SeqCst, Ordering::SeqCst,
        ));
        if prev.is_err() {
            return Err(anyhow::anyhow!(
                "Agent {} not runnable: {:?}", self.agent_id, self.status()
            ));
        }

        self.emit(AgentLifecycleEvent::RunStart {
            agent_id: self.agent_id.clone(),
            timestamp_ms: now_ms(),
        });

        let start = std::time::Instant::now();
        let result = self.agent.run(prompt).await;
        let elapsed = start.elapsed();

        match result {
            Ok((response, events)) => {
                self.metrics.record_success(
                    elapsed.as_millis() as u64,
                    0, // TODO: extract token count from events
                    events.len() as u64,
                );
                self.transition(STATUS_CREATED);
                self.emit(AgentLifecycleEvent::RunEnd {
                    agent_id: self.agent_id.clone(),
                    timestamp_ms: now_ms(),
                    success: true,
                });
                Ok((response, events))
            }
            Err(e) => {
                self.metrics.record_failure(elapsed.as_millis() as u64);
                self.transition(STATUS_FAILED);
                self.emit(AgentLifecycleEvent::RunEnd {
                    agent_id: self.agent_id.clone(),
                    timestamp_ms: now_ms(),
                    success: false,
                });
                Err(e)
            }
        }
    }

    /// Continue the conversation with a follow-up prompt.
    pub async fn continue_with(&self, prompt: String) -> anyhow::Result<(Response, Vec<AgentEvent>)> {
        // Same lifecycle as run()
        self.run(prompt).await
    }

    /// Request cancellation of the current run.
    pub fn cancel(&self) {
        self.agent.cancel();
    }

    // ── Lifecycle ─────────────────────────────────────────

    /// Suspend the agent and create a checkpoint snapshot.
    ///
    /// Transitions Created/Running → Suspended.
    /// Returns snapshot that can be persisted and later restored.
    pub async fn suspend(&self) -> anyhow::Result<AgentSnapshot> {
        if !self.status().is_runnable() && self.status() != AgentStatus::Running {
            return Err(anyhow::anyhow!("Cannot suspend in {:?} state", self.status()));
        }

        // Cancel running work first
        if self.status() == AgentStatus::Running {
            self.cancel();
            tokio::time::sleep(Duration::from_millis(100)).await;
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
            snapshot: snapshot.clone(),
            timestamp_ms: now_ms(),
        });

        Ok(snapshot)
    }

    /// Terminate the agent permanently. Terminal state.
    pub fn terminate(&self) -> anyhow::Result<()> {
        if self.status().is_terminal() {
            return Err(anyhow::anyhow!("Already terminated"));
        }
        self.transition(STATUS_TERMINATED);
        self.emit(AgentLifecycleEvent::Terminated {
            agent_id: self.agent_id.clone(),
            timestamp_ms: now_ms(),
        });
        Ok(())
    }

    /// Switch model mid-conversation.
    pub fn switch_model(&self, model_id: &str, api_key: Option<String>) -> anyhow::Result<()> {
        self.agent.switch_model(model_id, api_key)
    }

    /// Update system prompt for future runs.
    pub fn set_system_prompt(&self, prompt: String) {
        self.agent.set_system_prompt(prompt);
    }

    /// Register a tool at runtime.
    pub fn add_tool(&self, tool: impl AgentTool + 'static) {
        self.agent.add_tool(tool);
    }

    // ── Internal ──────────────────────────────────────────

    fn transition(&self, new_status: u8) {
        self.status.store(new_status, Ordering::SeqCst);
    }

    fn emit(&self, event: AgentLifecycleEvent) {
        self.lifecycle_tx.send(event).ok();
    }
}
```

---

## 1.4 AgentLifecycleEvent

```rust
/// Events emitted during agent lifecycle transitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentLifecycleEvent {
    Spawned {
        agent_id: String,
        parent_id: Option<String>,
        model_id: String,
        timestamp_ms: u64,
    },
    RunStart { agent_id: String, timestamp_ms: u64 },
    RunEnd { agent_id: String, timestamp_ms: u64, success: bool },
    Suspended {
        agent_id: String,
        snapshot: AgentSnapshot,
        timestamp_ms: u64,
    },
    Resumed {
        agent_id: String,
        from_snapshot_id: Option<String>,
        timestamp_ms: u64,
    },
    Terminated { agent_id: String, timestamp_ms: u64 },
    ModelSwitched {
        agent_id: String,
        from_model: String,
        to_model: String,
        timestamp_ms: u64,
    },
}
```

---

## 1.5 AgentSupervisor

에이전트 풀을 관리하는 supervisor. Agent OS 커널의 핵심 컴포넌트.

```rust
// src/lifecycle/supervisor.rs

/// Manages a pool of agents with lifecycle operations.
///
/// Responsibilities:
/// - Spawn / terminate agents
/// - Persist snapshots via SnapshotStore
/// - Broadcast lifecycle events
/// - Supervise: auto-restart on failure (configurable)
pub struct AgentSupervisor {
    agents: Arc<RwLock<HashMap<String, AgentHandle>>>,
    lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    snapshot_store: Arc<dyn SnapshotStore>,
    policy: SupervisorPolicy,
    metrics: Arc<AgentMetrics>,
    resolver: Arc<dyn ProviderResolver>,
}

impl AgentSupervisor {
    pub fn new(
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
            metrics: Arc::new(AgentMetrics::new()),
            resolver,
        }
    }

    // ── Agent management ──────────────────────────────────

    /// Spawn a new agent.
    pub fn spawn(&self, config: AgentConfig) -> anyhow::Result<AgentHandle> {
        let model = self.resolver.resolve_model(&config.model_id)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", config.model_id))?;
        let provider = self.resolver.resolve_provider(&model.provider)
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", model.provider))?;

        let tools = Arc::new(ToolRegistry::new());
        let agent = Agent::new_with_resolver(
            provider, config.clone(), tools, Arc::clone(&self.resolver),
        );

        let handle = AgentHandle::new(
            agent, config.clone(), None, self.lifecycle_tx.clone(),
        );

        self.agents.write().insert(handle.agent_id().to_string(), handle.clone());

        self.emit(AgentLifecycleEvent::Spawned {
            agent_id: handle.agent_id().to_string(),
            parent_id: None,
            model_id: config.model_id,
            timestamp_ms: now_ms(),
        });

        Ok(handle)
    }

    /// Spawn a child agent (delegation lineage).
    pub fn spawn_child(
        &self,
        parent_id: &str,
        config: AgentConfig,
    ) -> anyhow::Result<AgentHandle> {
        let handle = self.spawn(config)?;
        self.emit(AgentLifecycleEvent::Spawned {
            agent_id: handle.agent_id().to_string(),
            parent_id: Some(parent_id.to_string()),
            model_id: handle.config.read().model_id.clone(),
            timestamp_ms: now_ms(),
        });
        Ok(handle)
    }

    /// Get a handle by agent ID.
    pub fn get(&self, agent_id: &str) -> Option<AgentHandle> {
        self.agents.read().get(agent_id).cloned()
    }

    /// List all agent IDs.
    pub fn list(&self) -> Vec<(String, AgentStatus)> {
        self.agents.read().iter()
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
        let handle = self.get(agent_id)
            .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", agent_id))?;
        let snapshot = handle.suspend().await?;
        self.snapshot_store.save(&snapshot).await?;
        Ok(snapshot)
    }

    /// Restore agent from persisted snapshot.
    pub async fn restore(&self, agent_id: &str) -> anyhow::Result<AgentHandle> {
        let snapshot = self.snapshot_store.load(agent_id).await?
            .ok_or_else(|| anyhow::anyhow!("Snapshot '{}' not found", agent_id))?;
        self.restore_from_snapshot(snapshot).await
    }

    /// Restore from an in-memory snapshot.
    pub async fn restore_from_snapshot(&self, snapshot: AgentSnapshot) -> anyhow::Result<AgentHandle> {
        let model = self.resolver.resolve_model(&snapshot.config.model_id)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", snapshot.config.model_id))?;
        let provider = self.resolver.resolve_provider(&model.provider)
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", model.provider))?;

        let tools = Arc::new(ToolRegistry::new());
        let mut agent = Agent::new_with_resolver(
            provider, snapshot.config.clone(), tools, Arc::clone(&self.resolver),
        );

        // Restore conversation state
        let state_json = serde_json::to_value(&snapshot.state)?;
        agent.import_state(state_json)?;

        let handle = AgentHandle::new(
            agent, snapshot.config.clone(), snapshot.parent_id, self.lifecycle_tx.clone(),
        );

        self.agents.write().insert(handle.agent_id().to_string(), handle.clone());

        self.emit(AgentLifecycleEvent::Resumed {
            agent_id: handle.agent_id().to_string(),
            from_snapshot_id: Some(snapshot.agent_id),
            timestamp_ms: now_ms(),
        });

        Ok(handle)
    }

    // ── Events ────────────────────────────────────────────

    /// Subscribe to lifecycle events from all agents.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentLifecycleEvent> {
        self.lifecycle_tx.subscribe()
    }

    /// Aggregate metrics.
    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    fn emit(&self, event: AgentLifecycleEvent) {
        self.lifecycle_tx.send(event).ok();
    }
}
```

---

## 1.6 SnapshotStore

```rust
// src/lifecycle/snapshot.rs (continued)

/// Storage backend for agent snapshots.
pub trait SnapshotStore: Send + Sync {
    fn save(&self, snapshot: &AgentSnapshot) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
    fn load(&self, agent_id: &str) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<AgentSnapshot>>> + Send>>;
    fn list(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<String>>> + Send>>;
    fn delete(&self, agent_id: &str) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
}

/// File-based snapshot store (local disk).
pub struct FileSnapshotStore {
    base_dir: PathBuf,
}

impl FileSnapshotStore {
    pub fn new(base_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&base_dir).ok();
        Self { base_dir }
    }
}

impl SnapshotStore for FileSnapshotStore {
    fn save(&self, snapshot: &AgentSnapshot) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let path = self.base_dir.join(format!("{}.json", snapshot.agent_id));
        let bytes = snapshot.to_bytes();
        Box::pin(async move {
            let bytes = bytes?;
            tokio::fs::write(&path, bytes).await?;
            Ok(())
        })
    }

    fn load(&self, agent_id: &str) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<AgentSnapshot>>> + Send>> {
        let path = self.base_dir.join(format!("{}.json", agent_id));
        Box::pin(async move {
            if !path.exists() { return Ok(None); }
            let bytes = tokio::fs::read(&path).await?;
            Ok(Some(AgentSnapshot::from_bytes(&bytes)?))
        })
    }

    fn list(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<String>>> + Send>> {
        let dir = self.base_dir.clone();
        Box::pin(async move {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            let mut ids = Vec::new();
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false) {
                    if let Some(stem) = path.file_stem() {
                        ids.push(stem.to_string_lossy().to_string());
                    }
                }
            }
            Ok(ids)
        })
    }

    fn delete(&self, agent_id: &str) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let path = self.base_dir.join(format!("{}.json", agent_id));
        Box::pin(async move {
            tokio::fs::remove_file(&path).await.ok();
            Ok(())
        })
    }
}
```

---

## 1.7 SupervisorPolicy

```rust
/// Supervisor restart policy.
#[derive(Debug, Clone)]
pub struct SupervisorPolicy {
    /// Max restart attempts within the window.
    pub max_restarts: usize,
    /// Time window for counting restarts.
    pub restart_window_secs: u64,
    /// Backoff strategy.
    pub backoff: RestartBackoff,
}

#[derive(Debug, Clone)]
pub enum RestartBackoff {
    None,
    Fixed { delay_ms: u64 },
    Exponential { base_ms: u64, max_ms: u64 },
}

impl Default for SupervisorPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            restart_window_secs: 60,
            backoff: RestartBackoff::Exponential { base_ms: 1000, max_ms: 30_000 },
        }
    }
}

impl SupervisorPolicy {
    pub fn no_restart() -> Self {
        Self { max_restarts: 0, restart_window_secs: 0, backoff: RestartBackoff::None }
    }
}
```

---

## 1.8 상태 전이 다이어그램

```
                    ┌─────────────────────────────────────┐
                    │          AgentSupervisor              │
                    │                                      │
  spawn(config) ──▶ ┌──────────┐                          │
                    │ Created  │◀─── run() completes ──────┤
                    └────┬─────┘                          │
                         │ run()                          │
                         ▼                                │
                    ┌──────────┐                          │
                    │ Running  │──── cancel() ───────────▶│
                    └──┬───┬───┘                          │
           suspend() │   │ error                         │
                       │   ▼                                │
                    ┌──────────┐  ┌──────────┐             │
                    │ Suspended│  │  Failed  │ (terminal)  │
                    └────┬─────┘  └──────────┘             │
                         │                                  │
                  restore()                                │
                         │                                  │
                         ▼                                  │
                    ┌──────────┐                          │
                    │ Created  │ (new handle from snapshot)│
                    └──────────┘                          │
                                                             │
                  terminate() ──▶ ┌──────────┐              │
                                  │Terminated│ (terminal)   │
                                  └──────────┘              │
                    └─────────────────────────────────────┘
```

---

## 1.9 기존 oxi-agent 변경 사항

`oxi-agent/src/agent.rs`에 최소 변경만 필요:

```rust
// 추가 1: config getter (이미 내부에 config() 있으나 pub이 아님)
impl Agent {
    /// Public read access to config (for snapshot).
    pub fn get_config(&self) -> AgentConfig {
        self.inner.read().config.clone()
    }
}

// 추가 2: resolver getter (for supervisor restore)
impl Agent {
    /// Get the provider resolver (for snapshot/restore).
    pub fn resolver(&self) -> &Arc<dyn ProviderResolver> {
        &self.resolver
    }
}
```

---

## 1.10 사용 예시

```rust
use oxi_sdk::prelude::*;
use oxi_sdk::lifecycle::*;

// 1. Supervisor 생성
let oxi = OxiBuilder::new().with_builtins().build();
let supervisor = oxi.supervisor()
    .policy(SupervisorPolicy::default())
    .snapshot_dir("/data/oxi/snapshots")
    .build();

// 2. 에이전트 spawn
let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514")
    .with_system_prompt("You are a code reviewer.")
    .with_max_iterations(20);

let handle = supervisor.spawn(config)?;
handle.add_tool(read_tool);
handle.add_tool(grep_tool);

// 3. 실행
let (response, events) = handle.run("Review src/main.rs".into()).await?;

// 4. Suspend + persist
let snapshot = supervisor.suspend(handle.agent_id()).await?;
// Snapshot is saved to disk. Process can crash and resume later.

// 5. Restore (maybe in a new process)
let handle = supervisor.restore(&snapshot.agent_id).await?;
let (response, _) = handle.run("Now check tests/".into()).await?;

// 6. Lifecycle events
let mut rx = supervisor.subscribe();
tokio::spawn(async move {
    while let Ok(event) = rx.recv().await {
        tracing::info!("Lifecycle: {:?}", event);
    }
});
```
