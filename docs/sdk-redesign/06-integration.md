# 06. 통합, 에러 타이핑, 런타임 제어, 마이그레이션

---

## 6.1 lib.rs 확장

```rust
// oxi-sdk/src/lib.rs

// ── 기존 모듈 ──
pub mod agent_builder;
pub mod agent_group;
pub mod builder;
pub mod closure_tool;
pub mod kernel_bridge;
pub mod message_bus;
pub mod metrics;
pub mod multi_provider;
pub mod tool_factory;
pub mod prelude;

// ── 새 모듈 ──
pub mod lifecycle;
pub mod security;
pub mod coordination;
pub mod observability;
pub mod middleware;
pub mod error;
pub mod routing;

// ── 브라우저 엔진 (항상 사용 가능) ──
pub use oxi_agent::tools::browse::{
    BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine, BrowserError, BrowserTab,
    ElementInfo, LinkInfo, PageContent, TabGuard,
};
#[cfg(feature = "native-browser")]
pub use oxi_agent::tools::browse::{BrowseScriptTool, OxiBrowserEngine};

// ── 새 re-export ──
pub use error::SdkError;
pub use routing::RoutingControl;
pub use lifecycle::{AgentHandle, AgentLifecycleEvent, AgentSnapshot, AgentStatus, AgentSupervisor, SupervisorPolicy, ...};
pub use security::{Authorizer, Capability, CapabilitySet, CapabilitySubject, SecurityMiddleware, StringPattern};
pub use coordination::{WorkQueue, WorkItem, WorkStatus, WorkResult, SharedMemory, MemoryKey, Consensus, CoordinatedGroup, ...};
pub use observability::{Tracer, Span, SpanContext, TraceId, SpanId, AuditLog, AuditEntry, CostTracker, TokenUsage, CostBreakdown, EventStore, ...};
pub use middleware::{Middleware, MiddlewarePipeline, MiddlewareBridge, MiddlewarePhase, ...};

// ── 기존 re-export 유지 ──
pub use oxi_ai::{ ... };
pub use oxi_agent::{ ... };
```

---

## 6.2 SdkError — 구조화된 에러 타입

> **이전 설계에서 누락.** `anyhow::Result`는 SDK 소비자가 에러를 매칭할 수 없음.
> typed error enum으로 대체.

```rust
// src/error.rs

use thiserror::Error;

/// oxi-sdk의 구조화된 에러 타입.
///
/// SDK 소비자는 `match`로 에러를 분기 처리할 수 있음.
/// 내부 구현에서는 `anyhow`를 유지하되, 공개 API 경계에서 `SdkError`로 변환.
#[derive(Debug, Error)]
pub enum SdkError {
    // ── 모델/프로바이더 ──
    #[error("model not found: {model_id}")]
    ModelNotFound { model_id: String },

    #[error("provider not found: {provider}")]
    ProviderNotFound { provider: String },

    #[error("all providers exhausted: {attempts} attempts")]
    AllProvidersExhausted { attempts: usize },

    // ── 에이전트 수명 주기 ──
    #[error("agent {agent_id} not runnable (status: {status})")]
    AgentNotRunnable { agent_id: String, status: AgentStatus },

    #[error("agent {agent_id} already running")]
    AgentAlreadyRunning { agent_id: String },

    #[error("snapshot not found: {agent_id}")]
    SnapshotNotFound { agent_id: String },

    #[error("snapshot corrupt: {agent_id}: {reason}")]
    SnapshotCorrupt { agent_id: String, reason: String },

    // ── 보안 ──
    #[error("permission denied: {subject} requires {capability}")]
    PermissionDenied { subject: String, capability: String },

    #[error("capability expired: {subject}")]
    CapabilityExpired { subject: String },

    // ── 조정 ──
    #[error("work item not found: {item_id}")]
    WorkItemNotFound { item_id: String },

    #[error("version conflict on {key}: expected {expected}, current {current}")]
    VersionConflict { key: String, expected: u64, current: u64 },

    #[error("vote session not found: {vote_id}")]
    VoteNotFound { vote_id: String },

    // ── 미들웨어 ──
    #[error("middleware blocked: {middleware}: {reason}")]
    MiddlewareBlocked { middleware: String, reason: String },

    #[error("token budget exceeded: {used} / {budget}")]
    TokenBudgetExceeded { used: usize, budget: usize },

    #[error("cost budget exceeded: ${used:.4} / ${budget:.4}")]
    CostBudgetExceeded { used: f64, budget: f64 },

    // ── 라우팅 ──
    #[error("routing disabled")]
    RoutingDisabled,

    #[error("no route available for model: {model_id}")]
    NoRouteAvailable { model_id: String },

    // ── 일반 ──
    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

/// SdkError를 anyhow에서 변환
impl From<SdkError> for anyhow::Error {
    fn from(e: SdkError) -> Self {
        anyhow::anyhow!("{}", e)
    }
}
```

**사용 예시:**

```rust
match agent.run(prompt).await {
    Ok((response, _)) => { ... }
    Err(e) => {
        let sdk_err = e.downcast_ref::<SdkError>();
        match sdk_err {
            Some(SdkError::PermissionDenied { capability, .. }) => {
                tracing::warn!("Lacking permission: {}", capability);
            }
            Some(SdkError::TokenBudgetExceeded { used, budget }) => {
                tracing::warn!("Budget used: {}/{}", used, budget);
            }
            Some(SdkError::AllProvidersExhausted { attempts }) => {
                tracing::error!("All {} providers failed", attempts);
            }
            _ => return Err(e),
        }
    }
}
```

---

## 6.3 런타임 라우팅 제어

> **이전 설계에서 누락.** `enable_routing()`은 빌드 타임에만 설정 가능했음.
> oxios가 런타임에 라우팅을 on/off하거나 모델 후보를 교체하려면 별도의 API가 필요.

```rust
// src/routing.rs

/// 런타임 라우팅 제어 인터페이스.
///
/// AgentHandle 또는 Agent를 통해 런타임에 라우팅을 조정.
#[derive(Debug, Clone)]
pub struct RoutingControl {
    enabled: Arc<AtomicBool>,
    config: Arc<RwLock<RoutingConfig>>,
}

#[derive(Debug, Clone)]
pub struct RoutingConfig {
    pub auto_routing: bool,
    pub prefer_cost_efficient: bool,
    pub fallback_models: Vec<String>,
    pub excluded_models: Vec<String>,
}

impl RoutingControl {
    pub fn new(config: RoutingConfig) -> Self;

    /// 라우팅 활성화/비활성화
    pub fn set_enabled(&self, enabled: bool);
    pub fn is_enabled(&self) -> bool;

    /// 설정 업데이트
    pub fn update_config(&self, f: impl FnOnce(&mut RoutingConfig));

    /// 폴백 모델 교체
    pub fn set_fallback_models(&self, models: Vec<String>);

    /// 특정 모델 제외 (예: 장애 발생 시)
    pub fn exclude_model(&self, model_id: &str);
    pub fn unexclude_model(&self, model_id: &str);
}
```

**AgentHandle 통합:**

```rust
impl AgentHandle {
    /// 런타임 라우팅 제어
    pub fn routing(&self) -> RoutingControl;
    pub fn set_routing_enabled(&self, enabled: bool);
    pub fn exclude_model(&self, model_id: &str);
}
```

**사용 예시:**

```rust
let handle = supervisor.spawn(config)?;

// 라우팅 끄기 (직접 모델만 사용)
handle.routing().set_enabled(false);

// 장애 모델 제외
handle.routing().exclude_model("openai/gpt-4o");

// 폴백 모델 교체
handle.routing().set_fallback_models(vec![
    "anthropic/claude-sonnet-4-20250514".into(),
    "google/gemini-2.5-pro".into(),
]);

// 라우팅 다시 켜기
handle.routing().set_enabled(true);
```

---

## 6.4 OxiBuilder 확장

```rust
impl OxiBuilder {
    // ── 기존 ──
    pub fn new() -> Self;
    pub fn with_builtins(self) -> Self;
    pub fn provider(self, name: &str, p: impl Provider + 'static) -> Self;
    pub fn provider_factory(self, name: &str, factory: ...) -> Self;
    pub fn model(self, model: Model) -> Self;
    pub fn enable_routing(self, config: RoutingConfig) -> Self;
    pub fn build(self) -> Oxi;

    // ── 새로운 진입점 ──

    /// AgentSupervisor 빌더
    pub fn supervisor(self) -> SupervisorBuilder;
}

pub struct SupervisorBuilder {
    oxi_builder: OxiBuilder,
    policy: SupervisorPolicy,
    snapshot_dir: Option<PathBuf>,
    audit: Option<Arc<AuditLog>>,
    authorizer: Option<Arc<Authorizer>>,
    tracer: Option<Arc<Tracer>>,
    cost_tracker: Option<Arc<CostTracker>>,
}

impl SupervisorBuilder {
    pub fn policy(self, policy: SupervisorPolicy) -> Self;
    pub fn snapshot_dir(self, dir: impl Into<PathBuf>) -> Self;
    pub fn with_audit(self, audit: Arc<AuditLog>) -> Self;
    pub fn with_authorizer(self, authorizer: Arc<Authorizer>) -> Self;
    pub fn with_tracer(self, tracer: Arc<Tracer>) -> Self;
    pub fn with_cost_tracker(self, tracker: Arc<CostTracker>) -> Self;

    pub fn build(self) -> anyhow::Result<AgentSupervisor>;
}
```

---

## 6.5 AgentBuilder 확장

```rust
pub struct AgentBuilder<'a> {
    oxi: &'a Oxi,
    config: AgentConfig,
    tools: ToolRegistry,
    workspace_dir: Option<PathBuf>,
    system_prompt: Option<String>,

    // ── 새 필드 ──
    middlewares: Vec<Arc<dyn Middleware>>,
    capabilities: Option<CapabilitySet>,
    authorizer: Option<Arc<Authorizer>>,
    tracer: Option<Arc<Tracer>>,
    audit_log: Option<Arc<AuditLog>>,
    cost_tracker: Option<Arc<CostTracker>>,
}

impl<'a> AgentBuilder<'a> {
    // ── 기존 ──
    pub fn workspace(self, dir: impl Into<PathBuf>) -> Self;
    pub fn system_prompt(self, prompt: impl Into<String>) -> Self;
    pub fn coding_tools(self) -> Self;
    pub fn readonly_tools(self) -> Self;
    pub fn browsing(self, engine: Arc<dyn BrowserEngine>) -> Self;
    pub fn custom_tool(self, ...) -> Self;
    pub fn kernel_tools(self, provider: &dyn KernelToolProvider, context: &KernelToolContext) -> Self;

    // ── 보안 ──
    pub fn capabilities(self, caps: CapabilitySet) -> Self;
    pub fn coding_capabilities(self) -> Self;
    pub fn readonly_capabilities(self) -> Self;
    pub fn authorizer(self, authorizer: Arc<Authorizer>) -> Self;

    // ── 관측 ──
    pub fn tracer(self, tracer: Arc<Tracer>) -> Self;
    pub fn audit_log(self, audit: Arc<AuditLog>) -> Self;
    pub fn cost_tracker(self, tracker: Arc<CostTracker>) -> Self;

    // ── 미들웨어 ──
    pub fn middleware(self, mw: impl Middleware + 'static) -> Self;
    pub fn with_rate_limit(self, max_per_minute: usize) -> Self;
    pub fn with_token_budget(self, max_tokens: usize) -> Self;
    pub fn with_logging(self) -> Self;

    /// 에이전트 빌드.
    ///
    /// 1. ProviderResolver로 모델/프로바이더 해석
    /// 2. 툴 등록
    /// 3. Authorizer에 capabilities 부여
    /// 4. MiddlewarePipeline → AgentHooks 변환 (bridge)
    /// 5. Agent 생성 + hooks 설정
    pub fn build(self) -> Result<Agent, SdkError>;
}
```

**`build()` 구현 (핵심 로직):**

```rust
pub fn build(self) -> Result<Agent, SdkError> {
    // 1. 모델 + 프로바이더 해석
    let model = self.oxi.resolve_model(&self.config.model_id)
        .map_err(|_| SdkError::ModelNotFound { model_id: self.config.model_id.clone() })?;
    let provider = self.oxi.create_provider(&model.provider)
        .map_err(|_| SdkError::ProviderNotFound { provider: model.provider.clone() })?;

    // 2. 기본 Agent 생성
    let resolver = /* OxiResolver 생성 */;
    let agent = Agent::new_with_resolver(provider, config, Arc::new(self.tools), resolver);

    // 3. Authorizer에 capabilities 부여
    if let Some(authorizer) = &self.authorizer {
        let agent_id = self.config.name.clone();
        let subject = CapabilitySubject::Agent(agent_id);
        if let Some(caps) = self.capabilities {
            authorizer.grant(subject, caps);
        }
    }

    // 4. MiddlewarePipeline → AgentHooks 변환
    if !self.middlewares.is_empty() {
        let pipeline = Arc::new(MiddlewarePipeline::new());
        for mw in self.middlewares {
            pipeline.add_arc(mw);
        }
        let terminate_flag = Arc::new(AtomicBool::new(false));
        let hooks = MiddlewareBridge::into_hooks(
            pipeline, self.config.name.clone(), terminate_flag,
        );
        agent.set_hooks(hooks);
    }

    Ok(agent)
}
```

---

## 6.6 prelude.rs 확장

```rust
// 기존
pub use crate::builder::{Oxi, OxiBuilder};
pub use crate::tool_factory::{browsing_tools, coding_tools, full_tools, readonly_tools};
pub use oxi_agent::{ Agent, AgentConfig, AgentEvent, ToolRegistry, ... };
pub use oxi_agent::tools::browse::{ BrowseConfig, BrowseTool, BrowserEngine, ... };
pub use oxi_ai::{ Model, Provider, CompactionStrategy, ... };

// 새로운
pub use crate::error::SdkError;
pub use crate::routing::RoutingControl;
pub use crate::lifecycle::{ AgentHandle, AgentStatus, AgentSnapshot, AgentSupervisor, SupervisorPolicy };
pub use crate::security::{ Authorizer, Capability, CapabilitySet, SecurityMiddleware };
pub use crate::coordination::{ WorkQueue, SharedMemory, Consensus, CoordinatedGroup };
pub use crate::observability::{ Tracer, AuditLog, CostTracker, TokenUsage };
pub use crate::middleware::{ Middleware, MiddlewarePipeline, MiddlewareBridge };
```

---

## 6.7 마이그레이션 가이드

### 기존 코드 — 변경 없음

```rust
let oxi = OxiBuilder::new().with_builtins().build();
let agent = oxi.agent(config).workspace("/tmp").coding_tools().build()?;
let (response, _) = agent.run("hello".into()).await?;
```

### Lifecycle 추가 (Phase 1)

```rust
let supervisor = oxi.supervisor().build()?;
let handle = supervisor.spawn(config)?;
handle.add_tool(read_tool);
let (response, _) = handle.run("Review code".into()).await?;
let snapshot = supervisor.suspend(handle.agent_id()).await?;
```

### Security 추가 (Phase 2)

```rust
let authorizer = Arc::new(Authorizer::new(audit.clone()));
let agent = oxi.agent(config)
    .coding_tools()
    .authorizer(authorizer)
    .coding_capabilities()
    .build()?;
```

### 전체 스택 (Phase 1-6)

```rust
// 관측
let tracer = Arc::new(Tracer::new());
let audit = Arc::new(AuditLog::new(2048));
let cost_tracker = Arc::new(CostTracker::new(oxi.models(), CostTrackerConfig {
    per_agent_budget: Some(5.0),
    global_budget: Some(50.0),
}));

// 보안
let authorizer = Arc::new(Authorizer::new(audit.clone()));
authorizer.define_role("coder", CapabilitySet::coding("/workspace"));

// Supervisor
let supervisor = oxi.supervisor()
    .with_audit(audit.clone())
    .with_authorizer(authorizer.clone())
    .with_tracer(tracer.clone())
    .with_cost_tracker(cost_tracker.clone())
    .build()?;

// 에이전트 spawn
let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514")
    .with_system_prompt("You are a code reviewer.")
    .with_max_iterations(20);

let handle = supervisor.spawn(config)?;
handle.add_tool(read_tool);
handle.add_tool(grep_tool);

// 실행
let (response, _) = handle.run("Review src/main.rs".into()).await?;

// 비용 확인
let cost = cost_tracker.snapshot(handle.agent_id()).unwrap();
tracing::info!("Cost: ${:.4}, Tokens: {}", cost.cost.total(), cost.usage.total());

// 런타임 라우팅 제어
handle.routing().exclude_model("openai/gpt-4o");
handle.routing().set_fallback_models(vec!["anthropic/claude-sonnet-4-20250514".into()]);
```

---

## 6.8 oxi-agent 최소 변경 요약

| 파일 | 변경 | 줄 수 |
|------|------|------|
| `agent.rs` | `get_config()`, `resolver()` pub getter 추가 | ~8줄 |
| `config.rs` | 변경 없음 | 0 |
| `state.rs` | 변경 없음 (이미 `Serialize`/`Deserialize`) | 0 |
| `tools.rs` | 변경 없음 | 0 |

**총 oxi-agent 변경:** ~8줄

---

## 6.9 테스트 계획

### Unit Tests

```rust
// error.rs
#[test] fn sdk_error_display() { ... }

// lifecycle/snapshot.rs
#[test] fn snapshot_roundtrip() { ... }

// security/authorizer.rs
#[test]
fn capability_satisfaction() {
    let authorizer = Authorizer::new(AuditLog::new(64));
    authorizer.grant(
        CapabilitySubject::Agent("a1".into()),
        CapabilitySet::coding("/ws"),
    );
    assert!(authorizer.check(&CapabilitySubject::Agent("a1".into()),
        &Capability::FileRead { path_pattern: "/ws/src/main.rs".into() }));
    assert!(!authorizer.check(&CapabilitySubject::Agent("a1".into()),
        &Capability::FileWrite { path_pattern: "/etc/passwd".into() }));
}

// security/authorizer.rs — role hierarchy
#[test]
fn role_based_access() {
    let authorizer = Authorizer::new(AuditLog::new(64));
    authorizer.define_role("coder", CapabilitySet::coding("/ws"));
    authorizer.bind_role("agent-001", "coder");
    assert!(authorizer.check(&CapabilitySubject::Agent("agent-001".into()),
        &Capability::FileRead { path_pattern: "/ws/any".into() }));
}

// coordination/work_queue.rs
#[test]
fn claim_is_atomic() { ... }

// middleware/bridge.rs
#[test]
fn pipeline_to_hooks_bridge() { ... }

// routing.rs
#[test]
fn runtime_routing_toggle() { ... }

// observability/cost.rs
#[test]
fn token_cost_calculation() { ... }
```

### Integration Tests

```rust
#[tokio::test]
async fn spawn_suspend_restore() { ... }

#[tokio::test]
async fn middleware_blocks_unauthorized_tool() { ... }

#[tokio::test]
async fn cost_tracking_across_runs() { ... }

#[tokio::test]
async fn coordinated_group_fan_out() { ... }
```
