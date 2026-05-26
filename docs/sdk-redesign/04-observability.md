# 04. 관측 가능성 (Observability)

모듈 경로: `oxi-sdk/src/observability/`

---

## 4.1 설계 개요

```
┌──────────────────────────────────────────────────────┐
│                  Observability Layer                   │
│                                                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌───────┐ │
│  │  Tracer  │  │ AuditLog │  │CostTrackr│  │EventStore│
│  │          │  │          │  │          │  │         │ │
│  │ span()   │  │ log()    │  │ track()  │  │ append()│ │
│  │ trace()  │  │ query()  │  │ snapshot │  │ replay()│ │
│  └──────────┘  └──────────┘  └──────────┘  └─────────┘ │
│       │             │             │              │      │
│       └─────────────┴─────────────┴──────────────┘      │
│                          │                              │
│              export (broadcast/file/OTLP)               │
└──────────────────────────────────────────────────────────┘
```

---

## 4.2 Tracer — 분산 Tracing

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpanId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanContext {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub context: SpanContext,
    pub name: String,
    pub kind: SpanKind,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub status: SpanStatus,
    pub attributes: HashMap<String, Value>,
    pub events: Vec<SpanEvent>,
    pub links: Vec<SpanContext>,
}

pub enum SpanKind { Agent, Tool, Llm, Internal }
pub enum SpanStatus { Ok, Error { message: String } }
```

```rust
impl Tracer {
    pub fn new() -> Self;

    /// 새 루트 span (새 trace)
    pub fn start(&self, name: &str, kind: SpanKind) -> SpanGuard<'_>;

    /// 자식 span (parent context에 연결)
    pub fn start_with_parent(&self, name: &str, kind: SpanKind, parent: Option<&SpanContext>) -> SpanGuard<'_>;

    /// span 완료 처리
    pub fn end(&self, span: Span);

    /// trace ID로 모든 span 조회
    pub fn trace(&self, trace_id: TraceId) -> Vec<Span>;

    /// span 완료 이벤트 구독
    pub fn subscribe(&self) -> broadcast::Receiver<Span>;
}

/// RAII guard. drop 시 자동 end.
pub struct SpanGuard<'a> { ... }
impl SpanGuard<'_> {
    pub fn context(&self) -> Option<&SpanContext>;
    pub fn set_attribute(&mut self, key: &str, value: Value);
    pub fn add_event(&mut self, name: &str, attrs: HashMap<String, Value>);
    pub fn set_error(&mut self, message: &str);
    pub fn end(self);  // 명시적 종료
}
```

---

## 4.3 AuditLog — 보안 감사 추적

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEntry {
    SecurityDecision { subject: String, capability: String, granted: bool, timestamp_ms: u64 },
    ToolExecution { agent_id: String, tool_name: String, params_summary: String, success: bool, duration_ms: u64, timestamp_ms: u64 },
    Lifecycle { agent_id: String, event: String, timestamp_ms: u64 },
    Custom { category: String, message: String, metadata: HashMap<String, Value>, timestamp_ms: u64 },
}

impl AuditLog {
    pub fn new(channel_capacity: usize) -> Self;
    pub fn log(&self, entry: AuditEntry);
    pub fn query(&self, filter: AuditFilter) -> Vec<AuditEntry>;
    pub fn entries(&self) -> Vec<AuditEntry>;
    pub fn subscribe(&self) -> broadcast::Receiver<AuditEntry>;
}
```

---

## 4.4 EventStore — Event Sourcing

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    pub sequence: u64,
    pub stream_id: String,
    pub event_type: String,
    pub payload: Value,
    pub timestamp_ms: u64,
}

impl EventStore {
    pub fn new(config: EventStoreConfig) -> Self;

    /// 이벤트 추가. sequence 반환.
    pub fn append(&self, stream_id: impl Into<String>, event_type: impl Into<String>, payload: Value) -> u64;

    /// 조회
    pub fn query(&self, q: EventQuery) -> Vec<StoredEvent>;

    /// 스트림 재생 (상태 복원)
    pub fn replay(&self, stream_id: &str) -> Vec<StoredEvent>;

    pub fn subscribe(&self) -> broadcast::Receiver<StoredEvent>;
}
```

---

## 4.5 CostTracker — 토큰 세분화 + 비용 추적

> **이전 설계에서 누락되었던 핵심 컴포넌트.** Agent OS에서 각 에이전트의 비용을 추적하고 예산을 관리하려면 토큰을 input/output/cache_read/cache_write로 세분화하고, 모델별 단가를 곱해 실제 비용을 계산해야 함.

```rust
/// 토큰 사용량 세분화
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// 모델 단가를 곱해 비용 계산 (USD)
    pub fn cost(&self, model: &Model) -> CostBreakdown {
        CostBreakdown {
            input_cost: self.input as f64 * model.cost.input / 1_000_000.0,
            output_cost: self.output as f64 * model.cost.output / 1_000_000.0,
            cache_read_cost: self.cache_read as f64 * model.cost.cache_read.unwrap_or(0.0) / 1_000_000.0,
            cache_write_cost: self.cache_write as f64 * model.cost.cache_write.unwrap_or(0.0) / 1_000_000.0,
        }
    }
}

/// 비용 분석 결과
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
}

impl CostBreakdown {
    pub fn total(&self) -> f64 {
        self.input_cost + self.output_cost + self.cache_read_cost + self.cache_write_cost
    }
}
```

```rust
/// 에이전트별 비용 추적기
pub struct CostTracker {
    /// agent_id → 누적 사용량
    usage: Arc<RwLock<HashMap<String, TokenUsage>>>,
    /// agent_id → 누적 비용
    costs: Arc<RwLock<HashMap<String, CostBreakdown>>>,
    /// model_db에서 단가 조회용
    model_registry: Arc<ModelRegistry>,
    config: CostTrackerConfig,
}

#[derive(Debug, Clone)]
pub struct CostTrackerConfig {
    /// 에이전트당 예산 (USD). 초과 시 이벤트 발생
    pub per_agent_budget: Option<f64>,
    /// 전체 예산
    pub global_budget: Option<f64>,
}

impl CostTracker {
    pub fn new(model_registry: Arc<ModelRegistry>, config: CostTrackerConfig) -> Self;

    /// LLM 호출 후 토큰 사용량 기록
    pub fn record(&self, agent_id: &str, model: &Model, usage: TokenUsage);

    /// 에이전트별 스냅샷
    pub fn snapshot(&self, agent_id: &str) -> Option<CostSnapshot>;

    /// 전체 스냅샷
    pub fn global_snapshot(&self) -> GlobalCostSnapshot;

    /// 예산 초과 여부
    pub fn is_over_budget(&self, agent_id: &str) -> bool;
    pub fn is_over_global_budget(&self) -> bool;

    /// 리셋 (새 billing period)
    pub fn reset(&self, agent_id: &str);
    pub fn reset_all(&self);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSnapshot {
    pub agent_id: String,
    pub usage: TokenUsage,
    pub cost: CostBreakdown,
    pub budget_remaining: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalCostSnapshot {
    pub total_agents: usize,
    pub total_usage: TokenUsage,
    pub total_cost: CostBreakdown,
    pub global_budget_remaining: Option<f64>,
    pub per_agent: Vec<CostSnapshot>,
}
```

**비용 추적 흐름:**

```
LLM 응답 → ProviderEvent::Usage { input, output, cache_read, cache_write }
    │
    ▼
CostTracker::record(agent_id, model, usage)
    │
    ├─ usage 누적
    ├─ model.cost로 비용 계산
    ├─ costs 누적
    ├─ 예산 체크 → 초과 시 TokenBudgetMiddleware에게 통지
    └─ AuditLog: ToolExecution에 비용 포함
```

**TokenBudgetMiddleware와의 통합:**

```rust
// middleware/builtins.rs
pub struct TokenBudgetMiddleware {
    max_tokens: usize,
    usage: Arc<AtomicU64>,
    cost_tracker: Option<Arc<CostTracker>>,  // ← 비용 기반 종료도 지원
}

impl TokenBudgetMiddleware {
    pub fn new(max_tokens: usize) -> Self;
    pub fn with_cost_tracker(max_tokens: usize, tracker: Arc<CostTracker>) -> Self;
}

// AfterLlm 단계에서:
// 1. 토큰 누적
// 2. cost_tracker에 기록
// 3. 토큰 예산 또는 비용 예산 초과 시 Terminate
```

---

## 4.6 AgentMetrics 통합

기존 `AgentMetrics`는 `CostTracker`와 연동:

```rust
impl AgentMetrics {
    /// CostTracker가 연결된 메트릭
    pub fn with_cost_tracker(tracker: Arc<CostTracker>) -> Self;

    /// 기존 record_success를 확장하여 토큰 세분화 기록
    pub fn record_detailed(&self, duration_ms: u64, usage: TokenUsage, tools: u64) {
        self.total_tokens.fetch_add(usage.total(), Relaxed);
        // CostTracker에도 전파 (연결된 경우)
    }
}
```

---

## 4.7 전체 다이어그램

```
Agent Run
  │
  ├── Tracer: start("agent_run", Agent)
  │     │
  │     ├── Tracer: start("llm_call", Llm)          ← provider.stream()
  │     │     ├── CostTracker: record(usage)         ← 토큰 + 비용 추적
  │     │     └── end() → duration, tokens, model, cost
  │     │
  │     ├── Tracer: start("tool:bash", Tool)         ← tool.execute()
  │     │     ├── AuditLog: ToolExecution { cost: $0.003 }
  │     │     ├── EventStore: append("tool_call", ...)
  │     │     └── end() → duration, exit_code
  │     │
  │     └── end() → total_duration, total_tokens, total_cost
  │
  ├── CostTracker: snapshot() → CostSnapshot
  └── Metrics: record_detailed(duration, usage, tools)
```

---

## 4.8 사용 예시

```rust
use oxi_sdk::observability::*;

let tracer = Arc::new(Tracer::new());
let audit = Arc::new(AuditLog::new(2048));
let cost_tracker = Arc::new(CostTracker::new(
    oxi.models(),
    CostTrackerConfig {
        per_agent_budget: Some(5.0),  // 에이전트당 $5
        global_budget: Some(50.0),     // 전체 $50
    },
));

// AgentBuilder에 통합
let agent = oxi.agent(config)
    .tracer(tracer.clone())
    .audit_log(audit.clone())
    .cost_tracker(cost_tracker.clone())
    .build()?;

// 실행 후 비용 확인
let snapshot = cost_tracker.snapshot("agent-001");
println!("Cost: ${:.4} (tokens: {})", snapshot.cost.total(), snapshot.usage.total());

// 전체 비용
let global = cost_tracker.global_snapshot();
println!("Total: ${:.2} across {} agents", global.total_cost.total(), global.total_agents);
```
