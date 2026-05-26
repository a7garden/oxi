# oxi-sdk 개선 설계 문서

> **버전**: 0.23.0 → 0.24.0 마일스톤
> **작성일**: 2026-05-26
> **범위**: SDK 완성도 분석에서 도출된 Critical 3건, Important 5건, Nice-to-Have 4건

---

## 목차

1. [개요](#1-개요)
2. [C-01: Token Usage 추출 파이프라인](#2-c-01-token-usage-추출-파이프라인)
3. [C-02: Orchestrated 전략 구현](#3-c-02-orchestrated-전략-구현)
4. [C-03: Doc Examples 컴파일 검증](#4-c-03-doc-examples-컴파일-검증)
5. [I-01: 에러 타입 일관성 — SdkResult 통일](#5-i-01-에러-타입-일관성--sdkresult-통일)
6. [I-02: AgentGroup Send 안전 래퍼](#6-i-02-agentgroup-send-안전-래퍼)
7. [I-03: README 및 예제 현대화](#7-i-03-readme-및-예제-현대화)
8. [I-04: PluginLoader 실구현](#8-i-04-pluginloader-실구현)
9. [I-05: Integration Test 스위트](#9-i-05-integration-test-스위트)
10. [N-01: 스트리밍 응답 API](#10-n-01-스트리밍-응답-api)
11. [N-02: Configuration Hot-Reload](#11-n-02-configuration-hot-reload)
12. [N-03: Metrics Export (OpenTelemetry)](#12-n-03-metrics-export-opentelemetry)
13. [N-04: Agent 간 고급 협업 패턴](#13-n-04-agent-간-고급-협업-패턴)
14. [마이그레이션 가이드](#14-마이그레이션-가이드)
15. [구현 우선순위 및 일정](#15-구현-우선순위-및-일정)

---

## 1. 개요

### 1.1 현재 상태

| 항목 | 수치 |
|------|------|
| 총 라인 수 | 9,464줄 (35개 `.rs`) |
| 공개 API | 392개 |
| 테스트 | 172개 (전부 통과) |
| 모듈 | 14개 최상위 + 하위 |
| TODO | 2개 (token count 추출) |

### 1.2 개선 매트릭스

```
┌──────────┬───────────────────────────────────────────────┬──────────┐
│ 우선순위  │ 항목                                          │ 작업 규모 │
├──────────┼───────────────────────────────────────────────┼──────────┤
│ Critical │ C-01 Token Usage 추출 파이프라인              │ M        │
│ Critical │ C-02 Orchestrated 전략 구현                   │ M        │
│ Critical │ C-03 Doc Examples 컴파일 검증                 │ S        │
│ Important│ I-01 SdkResult 에러 타입 통일                 │ M        │
│ Important│ I-02 AgentGroup Send 안전 래퍼                │ L        │
│ Important│ I-03 README 및 예제 현대화                     │ S        │
│ Important│ I-04 PluginLoader 실구현                      │ M        │
│ Important│ I-05 Integration Test 스위트                  │ M        │
│ Nice     │ N-01 스트리밍 응답 API                        │ L        │
│ Nice     │ N-02 Configuration Hot-Reload                 │ M        │
│ Nice     │ N-03 Metrics Export (OTLP)                    │ M        │
│ Nice     │ N-04 Agent 간 고급 협업 패턴                  │ L        │
└──────────┴───────────────────────────────────────────────┴──────────┘
```

---

## 2. C-01: Token Usage 추출 파이프라인

### 2.1 문제

`AgentHandle::run()`이 `metrics.record_success(elapsed, 0, events.len())`를 호출한다.
`AgentEvent::Usage { input_tokens, output_tokens }`가 이미 이벤트 스트림에 존재하지만,
이를 집계하여 metrics에 반영하지 않는다.

```rust
// 현재 코드 (supervisor.rs:207-212)
self.metrics.record_success(
    elapsed.as_millis() as u64,
    0, // ← BUG: 항상 0
    events.len() as u64,
);
// TODO: extract token/usage metrics from events  ← 이 줄
```

### 2.2 해결 설계

`AgentEvent` 스트림에서 `Usage` 이벤트를 순회하여 합산하는 헬퍼 함수를 추가한다.

```rust
// oxi-sdk/src/metrics.rs에 추가

/// AgentEvent 벡터에서 토큰 사용량을 추출하여 합산.
pub fn extract_token_usage(events: &[oxi_agent::AgentEvent]) -> (u64, u64) {
    let mut input: u64 = 0;
    let mut output: u64 = 0;
    for event in events {
        if let AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } = event
        {
            input += *input_tokens as u64;
            output += *output_tokens as u64;
        }
    }
    (input, output)
}

/// AgentEvent 벡터에서 툴 실행 횟수를 계산.
pub fn extract_tool_call_count(events: &[oxi_agent::AgentEvent]) -> u64 {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        .count() as u64
}
```

### 2.3 AgentMetrics 확장

현재 `record_success`는 `total_tokens: u64` 하나만 받는다. 입력/출력을 구분해야 한다.

```rust
// oxi-sdk/src/metrics.rs

pub struct AgentMetrics {
    pub total_runs: AtomicU64,
    pub successful_runs: AtomicU64,
    pub failed_runs: AtomicU64,
    pub total_input_tokens: AtomicU64,   // ← 추가
    pub total_output_tokens: AtomicU64,  // ← 추가
    pub total_tokens: AtomicU64,         // 기존 유지 (input + output)
    pub tool_calls: AtomicU64,
    pub total_duration_ms: AtomicU64,
}

impl AgentMetrics {
    /// Record a successful run with full token breakdown.
    pub fn record_success(
        &self,
        duration_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        tool_call_count: u64,
    ) {
        self.total_runs.fetch_add(1, Ordering::Relaxed);
        self.successful_runs.fetch_add(1, Ordering::Relaxed);
        self.total_input_tokens.fetch_add(input_tokens, Ordering::Relaxed);
        self.total_output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
        self.total_tokens.fetch_add(input_tokens + output_tokens, Ordering::Relaxed);
        self.tool_calls.fetch_add(tool_call_count, Ordering::Relaxed);
        self.total_duration_ms.fetch_add(duration_ms, Ordering::Relaxed);
    }
}
```

`MetricsSnapshot`도 동일하게 확장:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub total_runs: u64,
    pub successful_runs: u64,
    pub failed_runs: u64,
    pub total_input_tokens: u64,   // ← 추가
    pub total_output_tokens: u64,  // ← 추가
    pub total_tokens: u64,
    pub tool_calls: u64,
    pub total_duration_ms: u64,
}
```

### 2.4 AgentHandle 수정

```rust
// oxi-sdk/src/lifecycle/supervisor.rs — AgentHandle::run()

match result {
    Ok((response, events)) => {
        let (input_tokens, output_tokens) =
            crate::metrics::extract_token_usage(&events);
        let tool_count =
            crate::metrics::extract_tool_call_count(&events);

        self.metrics.record_success(
            elapsed.as_millis() as u64,
            input_tokens,
            output_tokens,
            tool_count,
        );

        // CostTracker 연동
        if let Some(tracker) = &self.cost_tracker {
            let usage = crate::observability::TokenUsage {
                input: input_tokens,
                output: output_tokens,
                cache_read: 0, // TODO: 캐시 토큰은 ProviderEvent에서 추출
                cache_write: 0,
            };
            let model_id = self.config.read().model_id.clone();
            if let Some(model) = self.resolver.resolve_model(&model_id) {
                tracker.record(&self.agent_id, &model, usage);
            }
        }
        // ... 기존 코드 계속
    }
}
```

### 2.5 AgentHandle에 cost_tracker 필드 추가

```rust
pub struct AgentHandle {
    // ... 기존 필드
    cost_tracker: Option<Arc<CostTracker>>,  // ← 추가
}
```

`AgentHandle::new()`에서 `None`으로 초기화하고, supervisor의 `spawn()`에서
주입 가능한 빌더 메서드를 추가:

```rust
impl AgentHandle {
    /// Attach a cost tracker to this handle.
    pub fn with_cost_tracker(&mut self, tracker: Arc<CostTracker>) {
        self.cost_tracker = Some(tracker);
    }
}
```

### 2.6 영향 범위

| 파일 | 변경 내용 |
|------|-----------|
| `metrics.rs` | `record_success` 시그니처 변경, `extract_*` 함수 추가 |
| `lifecycle/supervisor.rs` | `run()`에서 usage 추출 로직, `cost_tracker` 필드 |
| `lifecycle/snapshot.rs` | `MetricsSnapshot` 필드 확장 |
| 테스트 | 기존 `record_success` 호출부 업데이트 |

### 2.7 하위 호환성

`record_success(duration_ms, tokens, tools)` → `record_success(duration_ms, input, output, tools)`

기존 호출부는 모두 SDK 내부 테스트뿐이므로 breaking change가 외부에 영향을 주지 않는다.
`MetricsSnapshot`에 필드가 추가되지만 `#[serde(default)]`로 역직렬화 호환성을 유지한다.

---

## 3. C-02: Orchestrated 전략 구현

### 3.1 문제

현재 `AgentGroup::run_orchestrated()`는 leader 에이전트만 실행하고,
worker 에이전트들을 전혀 활용하지 않는다.

```rust
// 현재 (agent_group.rs:178-191)
async fn run_orchestrated(&self, prompt: String, leader_idx: usize) -> Result<...> {
    let leader = &self.agents[leader_idx];
    let (response, _events) = leader.run(prompt).await?;
    // ← worker 에이전트가 무시됨
    Ok(vec![AgentGroupOutput { ... }])
}
```

### 3.2 설계: Leader-Worker 위임 패턴

Leader 에이전트가 응답에서 "지시사항"을 파싱하여 worker들에게 분배하는 방식.

```rust
/// Worker에게 전달할 위임 작업.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegatedTask {
    pub task_id: String,
    pub worker_index: usize,
    pub instruction: String,
    pub context: Option<String>,
}

/// Leader 응답에서 위임 작업을 파싱하는 전략.
pub trait DelegationParser: Send + Sync {
    /// Leader 응답에서 위임 작업 목록을 추출.
    fn parse(&self, leader_response: &str, worker_count: usize) -> Vec<DelegatedTask>;
}
```

### 3.3 기본 DelegationParser 구현

Leader에게 구조화된 JSON 출력을 요청하는 방식:

```rust
/// 기본 위임 파서 — leader 응답에서 JSON 블록을 추출.
pub struct JsonDelegationParser;

impl DelegationParser for JsonDelegationParser {
    fn parse(&self, leader_response: &str, worker_count: usize) -> Vec<DelegatedTask> {
        // ```json ... ``` 블록 또는 마지막 JSON 객체 추출
        let tasks = extract_json_tasks(leader_response);
        tasks.into_iter().take(worker_count).collect()
    }
}

fn extract_json_tasks(response: &str) -> Vec<DelegatedTask> {
    // 1. ```json 코드 블록에서 배열 추출 시도
    // 2. { ... } JSON 객체 직접 파싱 시도
    // 3. 파싱 실패 시 빈 벡터 반환
    // ...
}
```

### 3.4 개선된 run_orchestrated

```rust
async fn run_orchestrated(
    &self,
    prompt: String,
    leader_idx: usize,
) -> Result<Vec<AgentGroupOutput>> {
    if leader_idx >= self.agents.len() {
        anyhow::bail!("Leader index {} out of range", leader_idx);
    }

    let leader = &self.agents[leader_idx];
    let workers: Vec<_> = self.agents.iter()
        .enumerate()
        .filter(|(i, _)| *i != leader_idx)
        .collect();

    // Phase 1: Leader가 작업을 분석하고 위임
    let orchestration_prompt = format!(
        "{prompt}\n\n\
        You are the LEADER of a team with {} workers.\n\
        Analyze the task and delegate subtasks.\n\
        Respond with a JSON array of tasks:\n\
        ```json\n\
        [{{\"worker_index\": 0, \"instruction\": \"...\", \"context\": \"...\"}}]\n\
        ```",
        workers.len()
    );

    let (leader_response, _events) = leader.run(orchestration_prompt).await?;

    let mut results = vec![AgentGroupOutput {
        name: leader.model_id(),
        content: leader_response.content.clone(),
        success: true,
        error: None,
    }];

    // Phase 2: Parse delegations
    let parser = JsonDelegationParser;
    let tasks = parser.parse(&leader_response.content, workers.len());

    if tasks.is_empty() || workers.is_empty() {
        // 위임 실패 — leader 결과만 반환
        return Ok(results);
    }

    // Phase 3: Workers execute in parallel
    let worker_handles: Vec<_> = tasks.into_iter()
        .filter_map(|task| {
            let (actual_idx, worker) = workers.get(task.worker_index)?;
            let worker = Arc::clone(worker);
            let instruction = task.instruction;
            let context = task.context.unwrap_or_default();
            Some(tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all().build()?;
                rt.block_on(async {
                    let prompt = if context.is_empty() {
                        instruction
                    } else {
                        format!("Context:\n{context}\n\nTask:\n{instruction}")
                    };
                    worker.run(prompt).await
                })
            }))
        })
        .collect();

    for (i, handle) in worker_handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok((response, _))) => results.push(AgentGroupOutput {
                name: format!("worker-{i}"),
                content: response.content,
                success: true,
                error: None,
            }),
            Ok(Err(e)) => results.push(AgentGroupOutput {
                name: format!("worker-{i}"),
                content: String::new(),
                success: false,
                error: Some(e.to_string()),
            }),
            Err(e) => results.push(AgentGroupOutput {
                name: format!("worker-{i}"),
                content: String::new(),
                success: false,
                error: Some(format!("Join error: {e}")),
            }),
        }
    }

    // Phase 4: (선택) Leader가 worker 결과를 취합
    // — 현재는 각 worker 결과를 개별 반환
    Ok(results)
}
```

### 3.5 AgentGroupBuilder에 parser 주입

```rust
pub struct AgentGroup {
    agents: Vec<Arc<Agent>>,
    strategy: GroupStrategy,
    delegation_parser: Option<Arc<dyn DelegationParser>>,  // ← 추가
}

pub enum GroupStrategy {
    Pipeline,
    Parallel { max_concurrency: usize },
    Orchestrated {
        leader: usize,
        /// 사용자 정의 위임 파서. None이면 JsonDelegationParser 사용.
        delegation_parser: Option<Arc<dyn DelegationParser>>,
    },
}
```

### 3.6 영향 범위

| 파일 | 변경 내용 |
|------|-----------|
| `agent_group.rs` | `run_orchestrated` 전면 재작성, `DelegatedTask` 타입 추가 |
| `agent_group.rs` | `GroupStrategy::Orchestrated`에 `delegation_parser` 필드 추가 |
| `lib.rs` | `DelegatedTask`, `DelegationParser`, `JsonDelegationParser` re-export |
| `prelude.rs` | 동일 |

---

## 4. C-03: Doc Examples 컴파일 검증

### 4.1 문제

현재 20개 doc example이 모두 ` ```ignore ` 블록이다. `cargo test --doc`로 검증 불가.

### 4.2 해결 전략

doc example을 세 단계로 분류:

1. **Runnable** — `oxi_sdk`만 사용, 외부 의존 없이 컴파일 가능
2. **Requires Secrets** — API key가 필요하므로 `#[cfg(feature = "test-live")]`
3. **Conceptual** — 의사코드, `ignore` 유지

### 4.3 Runnable 예제 변환

```rust
// BEFORE:
/// ```ignore
/// let oxi = OxiBuilder::new().with_builtins().build();
/// ```

// AFTER:
/// ```
/// use oxi_sdk::OxiBuilder;
///
/// let oxi = OxiBuilder::new().build();
/// assert!(!oxi.has_builtins());
/// ```
```

### 4.4 변환 대상 및 방법

| 모듈 | 예제 수 | 전략 |
|------|---------|------|
| `builder.rs` | 4 | `OxiBuilder::new().build()`로 축소 → runnable |
| `agent_builder.rs` | 3 | workspace를 `/tmp`로 설정 → runnable (provider 없이 build만) |
| `closure_tool.rs` | 1 | sync handler만 → runnable |
| `kernel_bridge.rs` | 2 | Conceptual 유지 (kernel 의존) |
| `multi_provider.rs` | 3 | `create_builtin_provider` 없이 빌더만 → runnable |
| `tool_factory.rs` | 2 | `Path::new("/tmp")` → runnable |
| `message_bus.rs` | 2 | 이미 runnable 가능 |

### 4.5 CI 통합

```yaml
# .github/workflows/ci.yml에 추가
- name: Doc tests
  run: cargo test -p oxi-sdk --doc --all-features
```

---

## 5. I-01: 에러 타입 일관성 — SdkResult 통일

### 5.1 문제

SDK 전체에 `anyhow::Result`와 `SdkError`가 혼재:

| 위치 | 반환 타입 | 문제 |
|------|-----------|------|
| `AgentHandle::run()` | `anyhow::Result` | `SdkError`여야 함 |
| `AgentGroup::run()` | `anyhow::Result` | `SdkError`여야 함 |
| `AgentBuilder::build()` | `anyhow::Result` | 현재는 적절 (빌더 에러) |
| `Oxi::resolve_model()` | `anyhow::Result` | `SdkError::ModelNotFound`여야 함 |
| `Oxi::create_provider()` | `anyhow::Result` | `SdkError::ProviderNotFound`여야 함 |
| `Authorizer::require()` | `Result<(), SdkError>` | ✅ 올바름 |

### 5.2 설계

```rust
// oxi-sdk/src/error.rs

/// SDK의 통일 결과 타입.
pub type SdkResult<T> = Result<T, SdkError>;

// SdkError에 Agent 계층 에러 추가
#[derive(Debug, Error)]
pub enum SdkError {
    // ... 기존 variant

    // ── Agent Execution ─────────────────────────────────────
    #[error("agent execution failed: {reason}")]
    ExecutionFailed {
        reason: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("agent group execution failed: {failed}/{total} agents failed")]
    GroupExecutionFailed {
        failed: usize,
        total: usize,
        /// 개별 에러 (인덱스 → 에러 메시지).
        errors: HashMap<usize, String>,
    },

    #[error("run cancelled by caller")]
    Cancelled,
}
```

### 5.3 마이그레이션 대상

```rust
// Oxi 메서드
impl Oxi {
    pub fn resolve_model(&self, model_id: &str) -> SdkResult<Model> { ... }
    pub fn create_provider(&self, name: &str) -> SdkResult<Arc<dyn Provider>> { ... }
}

// AgentHandle
impl AgentHandle {
    pub async fn run(&self, prompt: String) -> SdkResult<(Response, Vec<AgentEvent>)> {
        // ...
        // AgentError → SdkError 변환
        result.map_err(|e| SdkError::ExecutionFailed {
            reason: e.to_string(),
            source: Some(e.into()),
        })
    }
}

// AgentGroup
impl AgentGroup {
    pub async fn run(&self, prompt: String) -> SdkResult<GroupResult> { ... }
}
```

### 5.4 하위 호환성

`anyhow::Result` → `SdkResult` 변경은 공개 API 시그니처 변경이다.
그러나 `SdkError: From<anyhow::Error>`이므로 `?` 연산자로 `anyhow` 에러를
자동 변환할 수 있어 외부 사용자의 마이그레이션 부담이 적다.

---

## 6. I-02: AgentGroup Send 안전 래퍼

### 6.1 문제

`Agent::run()`이 내부적으로 `Rc` 기반 타입을 사용하여 future가 `!Send`이다.
이 때문에 `AgentGroup::run_parallel()`이 `spawn_blocking` + `new_current_thread`
런타임 생성이라는 비용이 큰 워크어라운드를 사용한다.

```rust
// 현재 (agent_group.rs:192-206)
handles.push(tokio::task::spawn_blocking(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create runtime");  // ← 에이전트마다 런타임 생성
    rt.block_on(async move {
        // ...
    })
}));
```

### 6.2 설계: AgentRunner 래퍼

`!Send` future를 `Send` future로 변환하는 전용 런타임 풀:

```rust
// oxi-sdk/src/runner.rs — 신규 파일

use oxi_agent::{Agent, AgentEvent};
use oxi_agent::types::Response;
use std::sync::Arc;
use tokio::runtime::Runtime;

/// `!Send` agent future를 안전하게 실행하는 런타임 풀.
///
/// 내부적으로 전용 스레드 풀에서 agent를 실행하고,
/// 결과를 `Send` 채널로 반환한다.
pub struct AgentRunner {
    /// 에이전트 실행 전용 런타임 (multi-thread).
    runtime: Arc<Runtime>,
}

impl AgentRunner {
    /// 새 런타임을 생성.
    pub fn new(worker_threads: usize) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .enable_all()
            .build()
            .expect("Failed to create agent runtime");
        Self {
            runtime: Arc::new(runtime),
        }
    }

    /// 기본 4개 워커 스레드.
    pub fn default_pool() -> Self {
        Self::new(4)
    }

    /// Agent를 실행하고 `Send` future를 반환.
    ///
    /// 내부적으로 `spawn_blocking`으로 실행하여
    /// `tokio::task::spawn`이 가능한 future를 반환.
    pub fn run(
        &self,
        agent: Arc<Agent>,
        prompt: String,
    ) -> tokio::task::JoinHandle<anyhow::Result<(Response, Vec<AgentEvent>)>> {
        let runtime = Arc::clone(&self.runtime);
        tokio::task::spawn_blocking(move || {
            runtime.block_on(async move { agent.run(prompt).await })
        })
    }
}
```

### 6.3 AgentGroup에 Runner 주입

```rust
pub struct AgentGroup {
    agents: Vec<Arc<Agent>>,
    strategy: GroupStrategy,
    runner: Option<Arc<AgentRunner>>,  // ← 추가
}

impl AgentGroup {
    /// 커스텀 러너를 사용하는 그룹 생성.
    pub fn with_runner(runner: Arc<AgentRunner>) -> AgentGroupBuilder {
        AgentGroupBuilder {
            agents: Vec::new(),
            strategy: GroupStrategy::default(),
            runner: Some(runner),
        }
    }
}
```

### 6.4 개선된 run_parallel

```rust
async fn run_parallel(
    &self,
    prompt: String,
    max_concurrency: usize,
) -> Result<Vec<AgentGroupOutput>> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));

    // 러너가 있으면 공유 런타임 재사용, 없으면 기존 방식
    let handles: Vec<_> = self.agents.iter().map(|agent| {
        let agent = Arc::clone(agent);
        let prompt = prompt.clone();
        let sem = Arc::clone(&semaphore);

        match &self.runner {
            Some(runner) => {
                // 공유 런타임 사용 — 스레드 생성 비용 절약
                                let _permit = sem.acquire().await.unwrap();
                                match agent.run(prompt).await {
                                    Ok((response, _)) => AgentGroupOutput {
                                        name: agent.model_id(),
                                        content: response.content,
                                        success: true,
                                        error: None,
                                    },
                                    Err(e) => AgentGroupOutput {
                                        name: agent.model_id(),
                                        content: String::new(),
                                        success: false,
                                        error: Some(e.to_string()),
                                    },
                                }
                            }
                        })
                    }
                    None => {
                        // 기존 방식 (호환성)
                        tokio::task::spawn_blocking(move || {
                            let rt = tokio::runtime::Builder::new_current_thread()
                                .enable_all().build()?;
                            rt.block_on(async {
                                let _permit = sem.acquire().await.unwrap();
                                match agent.run(prompt).await {
                                    Ok((response, _)) => AgentGroupOutput {
                                        name: agent.model_id(),
                                        content: response.content,
                                        success: true,
                                        error: None,
                                    },
                                    Err(e) => AgentGroupOutput {
                                        name: agent.model_id(),
                                        content: String::new(),
                                        success: false,
                                        error: Some(e.to_string()),
                                    },
                                }
                            })
                        })
                    }
                }
    }).collect();
    // ... 결과 수집
}
```

### 6.5 장기적 해결 (oxi-agent)

근본적으로 `oxi-agent`의 `Agent::run()`이 `Send` future를 반환하도록
내부 `Rc`를 `Arc`로 교체하는 작업이 필요하다. 이는 별도 마일스톤으로 분리한다.

---

## 7. I-03: README 및 예제 현대화

### 7.1 문제

- README.md가 구버전 API (`include_builtins`, `model_id`) 사용
- 예제가 `builder_demo.rs` 하나뿐, 사실상 println 데모
- 아키텍처 다이어그램 없음

### 7.2 README 재작성

```markdown
# oxi-sdk

Multi-agent SDK for oxi — build isolated, secure, observable AI agent systems in Rust.

## Quick Start

```rust
use oxi_sdk::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Build the engine
    let oxi = OxiBuilder::new()
        .with_builtins()
        .api_key("anthropic", "sk-ant-...")
        .build();

    // 2. Build an agent
    let agent = oxi.agent(AgentConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".into(),
        max_iterations: 20,
        ..Default::default()
    })
    .workspace("/my/project")
    .coding_tools()
    .system_prompt("You are a senior Rust developer.")
    .build()?;

    // 3. Run
    let (response, events) = agent.run("Refactor main.rs".into()).await?;
    println!("{}", response.content);
    Ok(())
}
```

## Architecture

```
┌─────────────────────────────────────────────┐
│  OxiBuilder → Oxi                           │  Engine + Registry
├─────────────────────────────────────────────┤
│  AgentBuilder → Agent                       │  Per-agent config
├─────────────────────────────────────────────┤
│  AgentGroup  │ MessageBus │ Supervisor      │  Orchestration
├─────────────────────────────────────────────┤
│  Security │ Middleware │ Observability      │  Cross-cutting
├─────────────────────────────────────────────┤
│  Coordination (Queue + Memory + Consensus)  │  Inter-agent
└─────────────────────────────────────────────┘
```

## Feature Flags

| Flag | Description |
|------|-------------|
| `default` | Core SDK |
| `native-browser` | Built-in headless browser tools |

## Modules

| Module | Purpose |
|--------|---------|
| `security` | Capability-based access control |
| `middleware` | Pipeline hooks (rate limit, logging, content filter) |
| `observability` | Tracing, audit, cost tracking, event store |
| `coordination` | Work queue, shared memory, consensus |
| `lifecycle` | Supervisor, snapshot, suspend/resume |
```

### 7.3 예제 추가

| 파일 | 내용 |
|------|------|
| `examples/minimal.rs` | 최소 에이전트 빌드 + 실행 |
| `examples/multi_agent.rs` | AgentGroup 병렬/파이프라인 |
| `examples/security.rs` | Capability + Authorizer + SecurityMiddleware |
| `examples/observability.rs` | Tracer + AuditLog + CostTracker 통합 |
| `examples/coordination.rs` | WorkQueue + SharedMemory + Consensus |

**`examples/multi_agent.rs` 예시:**

```rust
//! Multi-agent parallel execution example.
//!
//! Run with: cargo run -p oxi-sdk --example multi_agent

use oxi_sdk::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let oxi = OxiBuilder::new().with_builtins().build();

    let config = AgentConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".into(),
        max_iterations: 10,
        ..Default::default()
    };

    let reviewer = oxi.agent(config.clone())
        .system_prompt("You are a code reviewer.")
        .workspace("/tmp")
        .build()?;

    let tester = oxi.agent(config)
        .system_prompt("You are a test engineer.")
        .workspace("/tmp")
        .build()?;

    let group = AgentGroup::new(
        GroupStrategy::Parallel { max_concurrency: 2 }
    )
    .agent(Arc::new(reviewer))
    .agent(Arc::new(tester));

    let result = group.run("Analyze this codebase for issues.".into()).await?;

    for output in &result.results {
        println!("=== {} ({}) ===", output.name, output.success);
        println!("{}", &output.content[..output.content.len().min(200)]);
    }

    Ok(())
}
```

---

## 8. I-04: PluginLoader 실구현

### 8.1 문제

`PluginLoader`가 manifest 로딩만 하고, `loaded` 맵이 항상 비어 있다.
실제 동적 라이브러리 로딩이 구현되지 않았다.

### 8.2 설계: trait 기반 플러그인 아키텍처

동적 라이브러리 로딩 대신, trait 기반 접근으로 안전성을 확보:

```rust
// oxi-sdk/src/middleware/plugin.rs

/// 플러그인이 구현해야 하는 trait.
///
/// 동적 로딩(unsafe) 대신, 플러그인은 이 trait을 구현하는
/// struct를 제공하고 `register_plugin()`으로 등록한다.
pub trait Plugin: Send + Sync {
    /// 플러그인 이름.
    fn name(&self) -> &str;

    /// 플러그인이 제공하는 미들웨어 목록.
    fn middlewares(&self) -> Vec<Arc<dyn Middleware>>;

    /// (선택) 초기화. 엔진 컨텍스트에 접근 가능.
    fn initialize(&mut self, _ctx: &PluginContext) -> anyhow::Result<()> {
        Ok(())
    }

    /// (선택) 종료 시 정리.
    fn shutdown(&self) {}
}

/// 플러그인 초기화 컨텍스트.
pub struct PluginContext {
    /// 엔진의 모델 레지스트리 접근.
    pub models: Arc<oxi_ai::ModelRegistry>,
    /// 메시지 버스 (선택).
    pub message_bus: Option<Arc<crate::MessageBus>>,
}
```

### 8.3 PluginLoader 재구현

```rust
pub struct PluginLoader {
    plugins: RwLock<Vec<Box<dyn Plugin>>>,
    manifests: RwLock<HashMap<String, PluginManifest>>,
}

impl PluginLoader {
    pub fn new(_plugins_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugins: RwLock::new(Vec::new()),
            manifests: RwLock::new(HashMap::new()),
        }
    }

    /// trait 기반 플러그인 등록.
    pub fn register_plugin(&self, plugin: impl Plugin + 'static) -> anyhow::Result<()> {
        let name = plugin.name().to_string();
        let mut plugins = self.plugins.write();

        // 초기화
        let ctx = PluginContext {
            models: Arc::new(oxi_ai::ModelRegistry::new()),
            message_bus: None,
        };

        plugins.push(Box::new(plugin));
        tracing::info!(plugin = %name, "Plugin registered");
        Ok(())
    }

    /// manifest에서 메타데이터 로딩 (기존 API 유지).
    pub async fn load(&self, manifest_path: &Path) -> anyhow::Result<String> {
        let manifest = PluginManifest::from_file(manifest_path)?;
        let name = manifest.name.clone();
        self.manifests.write().insert(name.clone(), manifest);
        Ok(name)
    }

    /// 등록된 모든 플러그인의 미들웨어를 반환.
    pub fn middlewares(&self) -> Vec<Arc<dyn Middleware>> {
        self.plugins
            .read()
            .iter()
            .flat_map(|p| p.middlewares())
            .collect()
    }

    /// 특정 플러그인 제거.
    pub fn unload(&self, name: &str) -> bool {
        let mut plugins = self.plugins.write();
        let len_before = plugins.len();
        plugins.retain(|p| p.name() != name);
        plugins.len() != len_before
    }
}
```

### 8.4 (향후) unsafe 동적 로딩 지원

별도 feature flag `unsafe-plugins`로 분리:

```toml
[features]
unsafe-plugins = ["libloading"]

[dependencies]
libloading = { version = "0.8", optional = true }
```

---

## 9. I-05: Integration Test 스위트

### 9.1 문제

현재 테스트는 모두 단위 테스트이며, MockProvider 기반 end-to-end 테스트가 없다.

### 9.2 설계: MockProvider 기반 E2E 테스트

```rust
// oxi-sdk/tests/integration.rs — 신규 파일

use oxi_sdk::prelude::*;
use oxi_ai::{Provider, ProviderEvent, Model, Api};
use futures::Stream;

struct EchoProvider;

#[async_trait::async_trait]
impl Provider for EchoProvider {
    fn name(&self) -> &str { "echo" }

    async fn stream(
        &self,
        _model: &Model,
        context: &oxi_ai::Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, oxi_ai::ProviderError> {
        // 사용자 마지막 메시지를 에코
        let last_msg = context.messages.last()
            .and_then(|m| m.content().first())
            .and_then(|b| b.as_text())
            .unwrap_or("echo")
            .to_string();

        let stream = futures::stream::iter(vec![
            ProviderEvent::TextDelta { text: last_msg },
            ProviderEvent::EndTurn,
        ]);
        Ok(Box::pin(stream))
    }
}

fn echo_model() -> Model {
    Model::new("echo/model", "Echo", Api::OpenAiChat, "echo", "http://localhost")
}

fn echo_oxi() -> Oxi {
    OxiBuilder::new()
        .provider("echo", EchoProvider)
        .model(echo_model())
        .build()
}

#[tokio::test]
async fn full_pipeline_builder_to_run() {
    let oxi = echo_oxi();

    let agent = oxi.agent(AgentConfig {
        model_id: "echo/model".into(),
        max_iterations: 5,
        ..Default::default()
    })
    .workspace("/tmp")
    .system_prompt("Echo agent")
    .build()
    .expect("build should succeed");

    let (response, events) = agent.run("Hello world".into()).await
        .expect("run should succeed");

    assert!(!response.content.is_empty());
    assert!(!events.is_empty());
}

#[tokio::test]
async fn security_blocks_unauthorized_tool() {
    let oxi = echo_oxi();
    let audit = Arc::new(AuditLog::new(64));
    let authorizer = Arc::new(Authorizer::new(Arc::clone(&audit)));

    // read-only 권한만 부여
    authorizer.grant(
        CapabilitySubject::Agent("secured-agent".into()),
        CapabilitySet::read_only("/workspace"),
    );

    let agent = oxi.agent(AgentConfig {
        model_id: "echo/model".into(),
        name: "secured-agent".into(),
        max_iterations: 5,
        ..Default::default()
    })
    .workspace("/workspace")
    .readonly_tools()
    .authorizer(authorizer)
    .audit_log(audit)
    .build()
    .unwrap();

    // agent가 도구를 호출하려 하면 security middleware가 차단하는지 확인
    let _ = agent.run("Read /workspace/file.txt".into()).await;
    // audit log에 security decision이 기록되었는지 확인
}

#[tokio::test]
async fn supervisor_spawn_run_terminate() {
    let oxi = echo_oxi();
    let resolver: Arc<dyn ProviderResolver> = Arc::new(oxi.clone());
    let snapshot_store: Arc<dyn SnapshotStore> = Arc::new(
        FileSnapshotStore::new(std::env::temp_dir().join("oxi-test-snapshots")).unwrap()
    );
    let supervisor = AgentSupervisor::new(resolver, snapshot_store);

    let handle = supervisor.spawn(AgentConfig {
        model_id: "echo/model".into(),
        max_iterations: 5,
        ..Default::default()
    }).unwrap();

    assert_eq!(handle.status(), AgentStatus::Created);

    let (response, _) = handle.run("test".into()).await.unwrap();
    assert!(!response.content.is_empty());

    handle.terminate().unwrap();
    assert!(handle.status().is_terminal());
}

#[tokio::test]
async fn message_bus_pub_sub() {
    let bus = MessageBus::new(16);
    let mut rx = bus.subscribe();

    bus.publish(InterAgentMessage::broadcast(
        "coordinator",
        "start",
        serde_json::json!({"phase": 1}),
    ));

    let msg = rx.recv().await.unwrap();
    assert_eq!(msg.message_type, "start");
}

#[tokio::test]
async fn work_queue_claim_complete() {
    let queue = WorkQueue::new(WorkQueueConfig::default());
    let id = queue.enqueue("review", serde_json::json!({"file": "main.rs"}), 5);

    let item = queue.claim("agent-1", None).unwrap();
    assert_eq!(item.id, id);

    queue.start(&id).unwrap();
    queue.complete(&id, WorkResult {
        success: true,
        content: "LGTM".into(),
        error: None,
        duration_ms: 100,
        tokens_used: None,
    }).unwrap();

    assert_eq!(queue.stats().completed, 1);
}

#[tokio::test]
async fn observability_pipeline() {
    let tracer = Arc::new(Tracer::new());
    let audit = Arc::new(AuditLog::new(64));
    let registry = Arc::new(ModelRegistry::new());
    let cost = Arc::new(CostTracker::new(registry, CostTrackerConfig::default()));

    // Tracer span
    {
        let _span = tracer.start("test-run", SpanKind::Agent);
    }

    // Audit entry
    audit.log(AuditEntry::lifecycle("agent-1".into(), "started".into()));

    // Cost record
    let model = echo_model();
    cost.record("agent-1", &model, TokenUsage {
        input: 1000,
        output: 500,
        ..Default::default()
    });

    // Verify
    let spans: Vec<_> = { tracer.spans.read().clone() };
    assert!(!spans.is_empty());
    assert_eq!(audit.entries().len(), 1);
    assert!(cost.snapshot("agent-1").is_some());
}
```

### 9.3 테스트 인프라 공통화

```rust
// oxi-sdk/tests/common/mod.rs

pub fn echo_oxi() -> Oxi { ... }
pub fn echo_model() -> Model { ... }
```

---

## 10. N-01: 스트리밍 응답 API

### 10.1 설계

`Agent::run_streaming()`이 `oxi-agent`에 이미 존재한다.
SDK 레이어에서 이를 래핑하여 콜백 기반 + 채널 기반 API를 제공한다.

```rust
// oxi-sdk/src/streaming.rs — 신규 파일

use oxi_agent::AgentEvent;
use tokio::sync::mpsc;

/// 스트리밍 실행 결과 수신기.
pub struct EventStream {
    rx: mpsc::Receiver<AgentEvent>,
}

impl EventStream {
    /// 다음 이벤트를 비동기 대기.
    pub async fn next(&mut self) -> Option<AgentEvent> {
        self.rx.recv().await
    }

    /// 이벤트를 동기적으로 peek (non-blocking).
    pub fn try_next(&mut self) -> Option<AgentEvent> {
        self.rx.try_recv().ok()
    }
}

// AgentHandle에 추가
impl AgentHandle {
    /// 스트리밍 실행 — 이벤트를 실시간으로 수신.
    pub async fn run_streaming(
        &self,
        prompt: String,
    ) -> SdkResult<(EventStream, tokio::task::JoinHandle<SdkResult<Response>>)> {
        let (tx, rx) = mpsc::channel(256);

        let agent = Arc::clone(&self.agent);
        let terminate = Arc::new(AtomicBool::new(false));

        // CAS 상태 전환
        // ...

        let handle = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build()?;

            let tx_clone = tx.clone();
            rt.block_on(async {
                agent.run_streaming(prompt, move |event| {
                    let _ = tx_clone.blocking_send(event);
                }).await
            })
        });

        let stream = EventStream { rx };
        // ... 결과 조립
        Ok((stream, handle))
    }
}
```

### 10.2 사용 예시

```rust
let (mut stream, handle) = agent.run_streaming("Explain Rust".into()).await?;

while let Some(event) = stream.next().await {
    match event {
        AgentEvent::TextChunk { text } => print!("{text}"),
        AgentEvent::ToolExecutionStart { tool_name, .. } => {
            println!("\n[Tool: {tool_name}]")
        }
        AgentEvent::Usage { input_tokens, output_tokens } => {
            println!("\n[Usage: {input_tokens} in, {output_tokens} out]")
        }
        _ => {}
    }
}

let response = handle.await??;
println!("\nDone: {}", response.content);
```

---

## 11. N-02: Configuration Hot-Reload

### 11.1 설계

실행 중인 에이전트의 설정을 파일 변경 감지로 자동 갱신.

```rust
// oxi-sdk/src/config_watcher.rs — 신규 파일

use notify::{Watcher, RecommendedWatcher, Event};
use std::path::PathBuf;
use std::sync::Arc;

/// 설정 파일 변경 감시기.
pub struct ConfigWatcher {
    watcher: RecommendedWatcher,
    tx: tokio::sync::watch::Sender<ConfigUpdate>,
}

#[derive(Debug, Clone)]
pub enum ConfigUpdate {
    SystemPrompt(String),
    ModelSwitch(String),
    RateLimit(usize),
    TokenBudget(usize),
}

impl ConfigWatcher {
    pub fn new(path: PathBuf) -> anyhow::Result<(Self, tokio::sync::watch::Receiver<ConfigUpdate>)> {
        let (tx, rx) = tokio::sync::watch::channel(ConfigUpdate::SystemPrompt(String::new()));

        let tx_clone = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                if event.kind.is_modify() {
                    // 파일 재로드 + 파싱
                    if let Ok(update) = Self::parse_config(&path) {
                        let _ = tx_clone.send(update);
                    }
                }
            }
        })?;

        watcher.watch(&path, notify::RecursiveMode::NonRecursive)?;

        Ok((Self { watcher, tx }, rx))
    }

    fn parse_config(path: &PathBuf) -> anyhow::Result<ConfigUpdate> {
        // TOML/JSON 파싱
        // ...
    }
}
```

### 11.2 AgentHandle에 watch 통합

```rust
impl AgentHandle {
    /// 설정 변경을 감시하여 자동 적용.
    pub async fn watch_config(&self, path: PathBuf) -> anyhow::Result<()> {
        let (_, mut rx) = ConfigWatcher::new(path)?;
        let handle = self.clone();

        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let update = rx.borrow().clone();
                match update {
                    ConfigUpdate::SystemPrompt(p) => handle.set_system_prompt(p),
                    ConfigUpdate::ModelSwitch(m) => {
                        if let Err(e) = handle.switch_model(&m, None) {
                            tracing::warn!("Config hot-reload model switch failed: {e}");
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }
}
```

---

## 12. N-03: Metrics Export (OpenTelemetry)

### 12.1 설계

`AgentMetrics`의 atomic counter를 OpenTelemetry 포맷으로 내보내는 익스포터.

```rust
// oxi-sdk/src/export/mod.rs — 신규 파일

pub mod otlp;
pub mod prometheus;
pub mod json;

/// 메트릭 익스포터 trait.
pub trait MetricsExporter: Send + Sync {
    /// 현재 스냅샷을 내보냄.
    fn export(&self, snapshot: &MetricsSnapshot, agent_id: &str);

    /// 모든 에이전트의 글로벌 스냅샷을 내보냄.
    fn export_global(&self, snapshot: &GlobalMetricsSnapshot);
}

// oxi-sdk/src/export/otlp.rs
pub struct OtlpExporter {
    endpoint: String,
    client: reqwest::Client,
}

impl MetricsExporter for OtlpExporter {
    fn export(&self, snapshot: &MetricsSnapshot, agent_id: &str) {
        // OpenTelemetry OTLP HTTP payload 구성
        // ...
    }
}
```

### 12.2 AgentRunner에 익스포터 연결

```rust
impl AgentRunner {
    /// 주기적 메트릭 익스포트 시작.
    pub fn start_export(
        &self,
        exporter: Arc<dyn MetricsExporter>,
        interval: std::time::Duration,
        metrics: Arc<AgentMetrics>,
        agent_id: String,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);
            loop {
                interval.tick().await;
                let snapshot = metrics.snapshot();
                exporter.export(&snapshot, &agent_id);
            }
        })
    }
}
```

---

## 13. N-04: Agent 간 고급 협업 패턴

### 13.1 Delegation Chain

```rust
// oxi-sdk/src/coordination/delegation.rs — 신규 파일

/// 위임 체인 — 에이전트가 다른 에이전트에게 작업을 위임할 수 있는 메커니즘.
pub struct DelegationChain {
    bus: Arc<MessageBus>,
    /// agent_id → 위임받은 에이전트 목록.
    delegates: RwLock<HashMap<String, Vec<String>>>,
}

impl DelegationChain {
    /// A가 B에게 작업 위임.
    pub async fn delegate(
        &self,
        from: &str,
        to: &str,
        task: DelegatedTask,
    ) -> anyhow::Result<()> {
        self.delegates
            .write()
            .entry(from.into())
            .or_default()
            .push(to.into());

        self.bus.publish(InterAgentMessage::direct(
            from, to,
            "delegation",
            serde_json::to_value(&task)?,
        ));

        Ok(())
    }

    /// 위임받은 작업의 결과를 보고.
    pub async fn report(
        &self,
        from: &str,
        to: &str,
        result: DelegationResult,
    ) -> PublishResult {
        self.bus.publish(InterAgentMessage::direct(
            from, to,
            "delegation_result",
            serde_json::to_value(&result).unwrap(),
        ))
    }
}
```

### 13.2 Recursive Spawn

```rust
impl AgentSupervisor {
    /// 에이전트가 자신의 자식을 동적으로 스폰할 수 있도록 허용.
    pub fn enable_recursive_spawn(
        &self,
        max_depth: usize,
        max_children_per_agent: usize,
    ) -> RecursiveSpawnGuard {
        RecursiveSpawnGuard {
            supervisor: self.clone(),
            max_depth,
            max_children_per_agent,
            depth: Arc::new(AtomicUsize::new(0)),
        }
    }
}

pub struct RecursiveSpawnGuard {
    supervisor: AgentSupervisor,
    max_depth: usize,
    max_children_per_agent: usize,
    depth: Arc<AtomicUsize>,
}

impl RecursiveSpawnGuard {
    /// 깊이 제한 내에서 자식 에이전트 스폰.
    pub fn spawn_child(&self, parent_id: &str, config: AgentConfig) -> anyhow::Result<AgentHandle> {
        let current = self.depth.load(Ordering::SeqCst);
        if current >= self.max_depth {
            return Err(SdkError::ExecutionFailed {
                reason: format!("Max recursion depth ({}) reached", self.max_depth),
                source: None,
            }.into());
        }

        self.depth.fetch_add(1, Ordering::SeqCst);
        let handle = self.supervisor.spawn_child(parent_id, config)?;
        Ok(handle)
    }
}
```

---

## 14. 마이그레이션 가이드

### 14.1 v0.23 → v0.24 Breaking Changes

#### `AgentMetrics::record_success` 시그니처 변경

```rust
// BEFORE (v0.23)
metrics.record_success(duration_ms, total_tokens, tool_calls);

// AFTER (v0.24)
metrics.record_success(duration_ms, input_tokens, output_tokens, tool_calls);
```

SDK 외부에서 `AgentMetrics`를 직접 호출하는 경우만 영향.
`AgentHandle::run()`을 사용하면 자동 처리됨.

#### `SdkResult` 도입

```rust
// BEFORE
let (response, events) = handle.run(prompt).await?;  // anyhow::Result

// AFTER
let (response, events) = handle.run(prompt).await?;  // SdkResult

// match로 세분화 가능
match handle.run(prompt).await {
    Ok((response, events)) => { ... },
    Err(SdkError::ExecutionFailed { reason, .. }) => { ... },
    Err(SdkError::Cancelled) => { ... },
    Err(e) => { ... },
}
```

`anyhow` 에러가 `SdkError::Internal`로 자동 래핑되므로 기존 `?` 사용은 그대로 작동.

#### `GroupStrategy::Orchestrated` variant 변경

```rust
// BEFORE
GroupStrategy::Orchestrated { leader: 0 }

// AFTER
GroupStrategy::Orchestrated {
    leader: 0,
    delegation_parser: None,  // Option<Arc<dyn DelegationParser>>
}
```

#### `MetricsSnapshot` 필드 추가

```rust
// v0.24에 추가된 필드 (#[serde(default)]로 역호환)
pub struct MetricsSnapshot {
    pub total_input_tokens: u64,   // NEW
    pub total_output_tokens: u64,  // NEW
    // ... 기존 필드 유지
}
```

### 14.2 Non-Breaking Additions

다음은 순수 추가이므로 기존 코드에 영향 없음:

- `AgentRunner`, `EventStream`
- `DelegationParser`, `JsonDelegationParser`, `DelegatedTask`
- `SdkResult<T>` type alias
- `extract_token_usage()`, `extract_tool_call_count()`
- `Plugin` trait
- `ConfigWatcher`
- `MetricsExporter` trait
- `DelegationChain`, `RecursiveSpawnGuard`

---

## 15. 구현 우선순위 및 일정

### 15.1 Phase 1 — Critical (v0.24.0)

```
Week 1-2
├── C-01 Token Usage 추출 (metrics.rs, supervisor.rs)
├── C-03 Doc Examples (20개 ignore → runnable 변환)
└── I-01 SdkResult 통일 (error.rs, Oxi, AgentHandle, AgentGroup)

Week 3-4
├── C-02 Orchestrated 전략 (agent_group.rs 전면 재작성)
├── I-03 README + examples/ (5개 예제 추가)
└── I-05 Integration tests (tests/integration.rs)
```

### 15.2 Phase 2 — Important (v0.25.0)

```
Week 5-6
├── I-02 AgentRunner (runner.rs, agent_group.rs 리팩터)
├── I-04 PluginLoader 재구현 (middleware/plugin.rs)
└── N-01 스트리밍 API (streaming.rs)

Week 7-8
├── N-02 ConfigWatcher (config_watcher.rs)
└── N-04 고급 협업 (coordination/delegation.rs)
```

### 15.3 Phase 3 — Nice-to-Have (v0.26.0)

```
Week 9-10
├── N-03 Metrics Export (export/*.rs)
└── oxi-agent Send future 근본 해결
```

### 15.4 검증 체크리스트

각 Phase 종료 시:

```bash
# 1. 전체 테스트 통과
cargo test -p oxi-sdk --all-features

# 2. Doc test 통과
cargo test -p oxi-sdk --doc --all-features

# 3. Clippy clean
cargo clippy -p oxi-sdk --all-features -- -D warnings

# 4. Format
cargo fmt -p oxi-sdk -- --check

# 5. Integration tests
cargo test -p oxi-sdk --test integration

# 6. 예제 빌드
cargo build -p oxi-sdk --examples --all-features
```

---

## Appendix A: 신규 파일 목록

```
oxi-sdk/
├── src/
│   ├── runner.rs                  # AgentRunner (I-02)
│   ├── streaming.rs               # EventStream (N-01)
│   ├── config_watcher.rs          # ConfigWatcher (N-02)
│   ├── export/
│   │   ├── mod.rs                 # MetricsExporter trait (N-03)
│   │   ├── otlp.rs
│   │   ├── prometheus.rs
│   │   └── json.rs
│   └── coordination/
│       └── delegation.rs          # DelegationChain (N-04)
├── tests/
│   ├── common/
│   │   └── mod.rs                 # 테스트 유틸리티 (I-05)
│   └── integration.rs             # E2E 테스트 (I-05)
├── examples/
│   ├── minimal.rs                 # (I-03)
│   ├── multi_agent.rs             # (I-03)
│   ├── security.rs                # (I-03)
│   ├── observability.rs           # (I-03)
│   └── coordination.rs            # (I-03)
└── DESIGN_IMPROVEMENTS.md         # 이 문서
```

## Appendix B: 신규 공개 타입 요약

| 타입 | 모듈 | 설명 |
|------|------|------|
| `SdkResult<T>` | `error` | 통일 결과 타입 |
| `AgentRunner` | `runner` | `!Send` agent 실행 래퍼 |
| `EventStream` | `streaming` | 스트리밍 이벤트 수신기 |
| `DelegatedTask` | `agent_group` | 위임 작업 DTO |
| `DelegationParser` | `agent_group` | 위임 파싱 trait |
| `JsonDelegationParser` | `agent_group` | 기본 JSON 위임 파서 |
| `Plugin` | `middleware` | 플러그인 trait |
| `PluginContext` | `middleware` | 플러그인 초기화 컨텍스트 |
| `ConfigWatcher` | `config_watcher` | 설정 파일 감시 |
| `ConfigUpdate` | `config_watcher` | 설정 변경 이벤트 |
| `MetricsExporter` | `export` | 메트릭 익스포터 trait |
| `DelegationChain` | `coordination` | 에이전트 위임 체인 |
| `RecursiveSpawnGuard` | `coordination` | 재귀 스폰 가드 |
