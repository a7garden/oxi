# oxicode-sdk 개선 설계 문서 v2

> **버전**: 0.23.0 → 0.24.0
> **이전**: `DESIGN_IMPROVEMENTS.md` (초안) → `DESIGN_IMPROVEMENTS_REVIEW.md` (검토) → **본 문서**
> **원칙**: 모든 설계를 실제 코드와 교차 검증. 가정 금지.

---

## 목차

1. [개요](#1-개요)
2. [C-01: Token Usage 추적 파이프라인](#2-c-01-token-usage-추적-파이프라인)
3. [C-02: Orchestrated 전략 구현](#3-c-02-orchestrated-전략-구현)
4. [C-03: Doc Examples + README + 구API 정리](#4-c-03-doc-examples--readme--구api-정리)
5. [I-01: 에러 타입 일관성 — 실행 계층만 SdkResult](#5-i-01-에러-타입-일관성--실행-계층만-sdkresult)
6. [I-02: AgentRunner — 동시성 제어 래퍼](#6-i-02-agentrunner--동시성-제어-래퍼)
7. [I-03: 예제 5종 추가](#7-i-03-예제-5종-추가)
8. [I-04: Plugin trait 기반 로딩](#8-i-04-plugin-trait-기반-로딩)
9. [I-05: Integration Test 스위트](#9-i-05-integration-test-스위트)
10. [N-01: 스트리밍 응답 API](#10-n-01-스트리밍-응답-api)
11. [N-02: Configuration Hot-Reload](#11-n-02-configuration-hot-reload)
12. [N-03: Metrics Export (OTLP)](#12-n-03-metrics-export-otlp)
13. [N-04: Agent 간 고급 협업 패턴](#13-n-04-agent-간-고급-협업-패턴)
14. [마이그레이션 가이드](#14-마이그레이션-가이드)
15. [구현 일정](#15-구현-일정)

---

## 1. 개요

### 1.1 현재 상태

| 항목 | 수치 |
|------|------|
| 총 라인 수 | 9,464줄 (35개 `.rs`) |
| 공개 API | 392개 |
| 테스트 | 172개 (전부 통과) |
| TODO | 2개 |
| `AgentEvent::Usage` emit | **0건** (정의만 있음) |
| `AgentState::record_usage()` 호출 | **0건** (테스트에서만) |

### 1.2 검증에서 발견한 근본 문제들

v1 설계에서 교차 검증으로 밝혀낸 사실:

```
문제 1: AgentEvent::Usage는 정의되어 있지만 어디에서도 emit되지 않는다.
        → oxicode-agent/src/agent_loop/streaming.rs에 emit 코드가 없다.
        → 하지만 ProviderEvent::Done { message }에서 message.usage에 접근 가능.

문제 2: AgentState에 input_tokens/output_tokens 필드와 record_usage()가 있다.
        → 그러나 agent_loop에서 호출하는 곳이 없다.
        → agent.state()로 실행 후 읽을 수 있지만 항상 0이다.

문제 3: ProviderEvent에 EndTurn variant가 없다. Done만 있다.
        → MockProvider는 Done + AssistantMessage를 생성해야 한다.

문제 4: Agent::run()과 Agent::run_streaming() 모두 !Send future를 반환한다.
        → spawn_blocking 없이 tokio::spawn으로 실행할 수 없다.
```

### 1.3 개선 매트릭스

```
┌──────────┬──────────────────────────────────────────────┬──────────┬───────────┐
│ 우선순위  │ 항목                                         │ 작업 범위 │ 규모      │
├──────────┼──────────────────────────────────────────────┼──────────┼───────────┤
│ Critical │ C-01 Token Usage 추적                        │ 크로스    │ M         │
│ Critical │ C-02 Orchestrated 전략                       │ SDK       │ M         │
│ Critical │ C-03 Doc/README/구API 정리                   │ SDK       │ S         │
│ Important│ I-01 SdkResult (실행 계층만)                  │ SDK       │ S         │
│ Important│ I-02 AgentRunner 동시성 제어                  │ SDK       │ M         │
│ Important│ I-03 예제 5종                                │ SDK       │ S         │
│ Important│ I-04 Plugin trait 로딩                       │ SDK       │ M         │
│ Important│ I-05 Integration Test                        │ SDK       │ M         │
│ Nice     │ N-01 스트리밍 API                             │ SDK       │ M         │
│ Nice     │ N-02 Config Hot-Reload                       │ SDK       │ S         │
│ Nice     │ N-03 Metrics Export                          │ SDK       │ M         │
│ Nice     │ N-04 고급 협업                                │ SDK       │ L         │
└──────────┴──────────────────────────────────────────────┴──────────┴───────────┘
```

---

## 2. C-01: Token Usage 추적 파이프라인

### 2.1 문제 정의 (검증 완료)

**사실 관계:**

| 코드 위치 | 내용 | 상태 |
|-----------|------|------|
| `oxicode-ai/src/types.rs:142` | `Usage { input, output, cache_read, cache_write }` | ✅ 정의됨 |
| `oxicode-ai/src/messages.rs:267` | `AssistantMessage { usage: Usage, ... }` | ✅ 필드 존재 |
| `oxicode-ai/src/providers/event.rs` | `ProviderEvent::Done { message: AssistantMessage }` | ✅ usage 포함 |
| `oxicode-agent/src/events.rs:189` | `AgentEvent::Usage { input_tokens, output_tokens }` | ✅ 정의됨 |
| `oxicode-agent/src/agent_loop/streaming.rs` | `ProviderEvent::Done` 처리 시 usage emit | ❌ 없음 |
| `oxicode-agent/src/state.rs:78` | `AgentState::record_usage(input, output)` | ✅ 존재 |
| `oxicode-agent/src/agent_loop/mod.rs` | `record_usage()` 호출 | ❌ 없음 |
| `oxicode-sdk/src/lifecycle/supervisor.rs:207` | `metrics.record_success(..., 0, ...)` | ❌ 항상 0 |

**결론:** Token usage 데이터는 ProviderEvent에 있지만, agent_loop가 이를
`AgentEvent::Usage`로 emit하지도, `AgentState`에 기록하지도 않는다.

### 2.2 수정 계획: 3-step 파이프라인

```
ProviderEvent::Done { message.usage }
    │
    ▼ Step 1: oxicode-agent/agent_loop/streaming.rs
    ├─ state.record_usage(message.usage.input, message.usage.output)
    └─ emit(AgentEvent::Usage { input_tokens, output_tokens })
    │
    ▼ Step 2: oxicode-agent/agent.rs — run() 반환 후 state에서 읽기
    │  agent.run() → (Response, Vec<AgentEvent>)
    │  agent.state().input_tokens, agent.state().output_tokens
    │
    ▼ Step 3: oxicode-sdk/lifecycle/supervisor.rs
    └─ metrics.record_success(elapsed, input_tokens, output_tokens, tool_count)
```

### 2.3 Step 1: oxicode-agent 수정

**파일: `oxicode-agent/src/agent_loop/streaming.rs`**

`ProviderEvent::Done` 처리 블록 안에 usage emit 추가:

```rust
ProviderEvent::Done { message, .. } => {
    loop_ref.circuit_breaker.record_success();

    // 🆕 Record token usage into shared state
    let (input, output) = (message.usage.input, message.usage.output);
    if input > 0 || output > 0 {
        loop_ref.state.update(|s| {
            s.record_usage(input, output);
        });
        emit(AgentEvent::Usage {
            input_tokens: input,
            output_tokens: output,
        });
    }

    // ... 기존 코드 계속 (message 처리)
}
```

**의존성 추가:**
```rust
// streaming.rs 상단에 이미 사용 가능한 emit 함수
// loop_ref.state는 SharedState 타입
// AgentEvent::Usage는 이미 events.rs에 정의됨
```

**검증:** `cargo test -p oxicode-agent` 통과해야 함.

### 2.4 Step 2: AgentMetrics 확장

**파일: `oxicode-sdk/src/metrics.rs`**

```rust
pub struct AgentMetrics {
    pub total_runs: AtomicU64,
    pub successful_runs: AtomicU64,
    pub failed_runs: AtomicU64,
    pub total_input_tokens: AtomicU64,   // 🆕
    pub total_output_tokens: AtomicU64,  // 🆕
    pub total_tokens: AtomicU64,
    pub tool_calls: AtomicU64,
    pub total_duration_ms: AtomicU64,
}

impl AgentMetrics {
    /// 기존: record_success(duration_ms, tokens, tools)
    /// 변경: record_success(duration_ms, input_tokens, output_tokens, tool_count)
    pub fn record_success(
        &self,
        duration_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        tool_count: u64,
    ) {
        self.total_runs.fetch_add(1, Ordering::Relaxed);
        self.successful_runs.fetch_add(1, Ordering::Relaxed);
        self.total_input_tokens.fetch_add(input_tokens, Ordering::Relaxed);
        self.total_output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
        self.total_tokens.fetch_add(input_tokens + output_tokens, Ordering::Relaxed);
        self.tool_calls.fetch_add(tool_count, Ordering::Relaxed);
        self.total_duration_ms.fetch_add(duration_ms, Ordering::Relaxed);
    }
}
```

`MetricsSnapshot`에 대응하는 필드 추가 (기존 필드 유지):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub total_runs: u64,
    pub successful_runs: u64,
    pub failed_runs: u64,
    #[serde(default)]  // 🆕 역직렬화 호환
    pub total_input_tokens: u64,
    #[serde(default)]  // 🆕 역직렬화 호환
    pub total_output_tokens: u64,
    pub total_tokens: u64,
    pub tool_calls: u64,
    pub total_duration_ms: u64,
}
```

### 2.5 Step 3: AgentHandle 수정

**파일: `oxicode-sdk/src/lifecycle/supervisor.rs`**

`AgentHandle::run()`의 성공 경로:

```rust
Ok((response, events)) => {
    // 🆕 agent.state()에서 실제 토큰 수 읽기
    let agent_state = self.agent.state();
    let input_tokens = agent_state.input_tokens as u64;
    let output_tokens = agent_state.output_tokens as u64;

    // 🆕 툴 실행 횟수 계산
    let tool_count = events.iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        .count() as u64;

    self.metrics.record_success(
        elapsed.as_millis() as u64,
        input_tokens,
        output_tokens,
        tool_count,
    );

    // 기존 상태 전환 + 이벤트 emit 코드...
    self.transition(STATUS_CREATED);
    // ...
}
```

**왜 이벤트 순회 대신 `agent.state()`를 읽는가?**

`agent.state()`는 agent_loop가 `record_usage()`로 누적한 값을 반환한다.
이벤트 순회도 가능하지만, state에서 직접 읽는 것이:
- 더 간단 (순회 불필요)
- 누적값 보장 (여러 턴에서 정확)
- 중복 계산 위험 없음

### 2.6 영향 범위

| 파일 | 크레이트 | 변경 |
|------|----------|------|
| `agent_loop/streaming.rs` | oxicode-agent | `Done` 처리에 `record_usage` + `emit(Usage)` 추가 |
| `metrics.rs` | oxicode-sdk | `record_success` 시그니처 변경, 필드 추가 |
| `lifecycle/supervisor.rs` | oxicode-sdk | `run()`에서 `agent.state()`로 토큰 수 읽기 |
| `lifecycle/snapshot.rs` | oxicode-sdk | `MetricsSnapshot` 필드 추가 (`#[serde(default)]`) |
| 테스트 | oxicode-sdk | `record_success` 호출부 시그니처 업데이트 |

### 2.7 하위 호환성

- `record_success(duration, tokens, tools)` → `record_success(duration, input, output, tools)`: SDK 내부만 호출. 외부 API 영향 없음.
- `MetricsSnapshot` 신규 필드에 `#[serde(default)]`로 역직렬화 호환.

---

## 3. C-02: Orchestrated 전략 구현

### 3.1 문제 정의 (검증 완료)

현재 `run_orchestrated()`는 leader만 실행:

```rust
// agent_group.rs:178-191
async fn run_orchestrated(&self, prompt: String, leader_idx: usize) -> Result<...> {
    let leader = &self.agents[leader_idx];
    let (response, _events) = leader.run(prompt).await?;
    Ok(vec![AgentGroupOutput { ... }])  // ← workers 무시
}
```

### 3.2 설계 원칙

1. **`GroupStrategy` enum은 변경하지 않는다** — breaking change 방지
2. 위임 파서는 `AgentGroup`의 필드로 주입
3. JSON 파싱 실패 시 fallback 전략 제공
4. Worker 실행은 `spawn_blocking` 사용 (`Agent::run()`이 `!Send`이므로)

### 3.3 신규 타입

```rust
// agent_group.rs에 추가

/// Leader가 Worker에게 위임할 작업.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegatedTask {
    pub worker_index: usize,
    pub instruction: String,
    #[serde(default)]
    pub context: Option<String>,
}

/// Leader 응답에서 위임 작업을 추출하는 trait.
pub trait DelegationParser: Send + Sync {
    fn parse(&self, leader_response: &str, worker_count: usize) -> Vec<DelegatedTask>;
}

/// 기본 파서 — JSON 블록에서 위임 작업 추출.
/// 실패 시 전체 prompt를 첫 번째 worker에게 전달 (fallback).
pub struct JsonDelegationParser;

impl DelegationParser for JsonDelegationParser {
    fn parse(&self, leader_response: &str, worker_count: usize) -> Vec<DelegatedTask> {
        // 1. ```json ... ``` 블록에서 배열 추출 시도
        if let Some(tasks) = try_parse_json_tasks(leader_response) {
            if !tasks.is_empty() {
                return tasks.into_iter().take(worker_count).collect();
            }
        }

        // 2. { ... } 또는 [ ... ] 직접 파싱 시도
        if let Some(tasks) = try_parse_raw_json(leader_response) {
            if !tasks.is_empty() {
                return tasks.into_iter().take(worker_count).collect();
            }
        }

        // 3. Fallback: leader 응답 전체를 첫 worker에게 전달
        vec![DelegatedTask {
            worker_index: 0,
            instruction: leader_response.to_string(),
            context: None,
        }]
    }
}

fn try_parse_json_tasks(response: &str) -> Option<Vec<DelegatedTask>> {
    // ```json ... ``` 블록 추출 후 serde_json::from_str
    let start = response.find("```json")? + 7;
    let end = response.find("```").unwrap_or(response.len());
    let json_str = &response[start..end];
    serde_json::from_str(json_str).ok()
}

fn try_parse_raw_json(response: &str) -> Option<Vec<DelegatedTask>> {
    // [ ... ] 배열 찾아서 파싱
    let start = response.find('[')?;
    let end = response.rfind(']')? + 1;
    serde_json::from_str(&response[start..end]).ok()
}
```

### 3.4 AgentGroup 확장

```rust
pub struct AgentGroup {
    agents: Vec<Arc<Agent>>,
    strategy: GroupStrategy,
    delegation_parser: Arc<dyn DelegationParser>,  // 🆕
}

impl AgentGroup {
    pub fn new(strategy: GroupStrategy) -> Self {
        Self {
            agents: Vec::new(),
            strategy,
            delegation_parser: Arc::new(JsonDelegationParser),  // 기본값
        }
    }

    /// 커스텀 위임 파서 설정.
    pub fn with_delegation_parser(mut self, parser: impl DelegationParser + 'static) -> Self {
        self.delegation_parser = Arc::new(parser);
        self
    }
}
```

### 3.5 run_orchestrated 재구현

```rust
async fn run_orchestrated(
    &self,
    prompt: String,
    leader_idx: usize,
) -> Result<Vec<AgentGroupOutput>> {
    if leader_idx >= self.agents.len() {
        anyhow::bail!("Leader index {} out of range ({} agents)",
            leader_idx, self.agents.len());
    }

    let workers: Vec<(usize, Arc<Agent>)> = self.agents.iter()
        .enumerate()
        .filter(|(i, _)| *i != leader_idx)
        .map(|(i, a)| (i, Arc::clone(a)))
        .collect();

    // ── Phase 1: Leader가 작업 분석 + 위임 ──

    let leader = &self.agents[leader_idx];
    let leader_prompt = if workers.is_empty() {
        prompt.clone()
    } else {
        format!(
            "{prompt}\n\n\
            You are the LEADER of a team with {} workers (indices 0..{}).\n\
            Delegate subtasks by responding with a JSON array:\n\
            ```json\n\
            [{{\"worker_index\": 0, \"instruction\": \"...\", \"context\": \"...\"}}]\n\
            ```",
            workers.len(),
            workers.len() - 1,
        )
    };

    let (leader_response, _) = leader.run(leader_prompt).await?;

    let mut results = vec![AgentGroupOutput {
        name: leader.model_id(),
        content: leader_response.content.clone(),
        success: true,
        error: None,
    }];

    if workers.is_empty() {
        return Ok(results);
    }

    // ── Phase 2: 위임 작업 파싱 ──

    let tasks = self.delegation_parser.parse(
        &leader_response.content,
        workers.len(),
    );

    // ── Phase 3: Workers 병렬 실행 (spawn_blocking) ──

    let worker_handles: Vec<_> = tasks.into_iter()
        .filter_map(|task| {
            let (_, worker) = workers.get(task.worker_index)?;
            let worker = Arc::clone(worker);
            let instruction = task.instruction;
            let context = task.context.unwrap_or_default();
            Some((task.worker_index, tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime creation");
                rt.block_on(async {
                    let prompt = if context.is_empty() {
                        instruction
                    } else {
                        format!("Context:\n{context}\n\nTask:\n{instruction}")
                    };
                    worker.run(prompt).await
                })
            })))
        })
        .collect();

    for (worker_idx, handle) in worker_handles {
        let output = match handle.await {
            Ok(Ok((response, _))) => AgentGroupOutput {
                name: format!("worker-{worker_idx}"),
                content: response.content,
                success: true,
                error: None,
            },
            Ok(Err(e)) => AgentGroupOutput {
                name: format!("worker-{worker_idx}"),
                content: String::new(),
                success: false,
                error: Some(e.to_string()),
            },
            Err(e) => AgentGroupOutput {
                name: format!("worker-{worker_idx}"),
                content: String::new(),
                success: false,
                error: Some(format!("Join error: {e}")),
            },
        };
        results.push(output);
    }

    Ok(results)
}
```

### 3.6 GroupStrategy 변경 없음

```rust
// 그대로 유지
pub enum GroupStrategy {
    Pipeline,
    Parallel { max_concurrency: usize },
    Orchestrated { leader: usize },
}
```

### 3.7 영향 범위

| 파일 | 변경 |
|------|------|
| `agent_group.rs` | `DelegatedTask`, `DelegationParser`, `JsonDelegationParser` 추가 |
| `agent_group.rs` | `AgentGroup`에 `delegation_parser` 필드 |
| `agent_group.rs` | `run_orchestrated` 재구현 |
| `lib.rs` | 신규 타입 re-export |
| `prelude.rs` | 신규 타입 re-export |

---

## 4. C-03: Doc Examples + README + 구API 정리

### 4.1 문제 정의 (검증 완료)

**README.md에 존재하지 않는 API 사용:**

```rust
// README.md (현재)
.include_builtins(true)               // ❌ 실제: .with_builtins()
.model_id("claude-sonnet-4-...")      // ❌ 실제: AgentConfig로 전달
```

**Doc examples 20개가 모두 ` ```ignore `:**

```bash
$ grep -c '```ignore' oxicode-sdk/src/**/*.rs
20
```

### 4.2 README.md 재작성

```markdown
# oxicode-sdk

Multi-agent SDK for oxicode — isolated, secure, observable AI agent systems in Rust.

## Quick Start

```rust
use oxicode_sdk::prelude::*;

let oxicode = OxicodeBuilder::new()
    .with_builtins()
    .api_key("anthropic", "sk-ant-...")
    .build();

let agent = oxicode.agent(AgentConfig {
    model_id: "anthropic/claude-sonnet-4-20250514".into(),
    max_iterations: 20,
    ..Default::default()
})
.workspace("/my/project")
.coding_tools()
.system_prompt("You are a senior developer.")
.build()?;

let (response, _events) = agent.run("Refactor main.rs".into()).await?;
println!("{}", response.content);
```

## Architecture

┌─────────────────────────────────────────────┐
│  OxicodeBuilder → Oxicode                           │
├─────────────────────────────────────────────┤
│  AgentBuilder → Agent                       │
├─────────────────────────────────────────────┤
│  AgentGroup │ MessageBus │ Supervisor       │
├─────────────────────────────────────────────┤
│  Security │ Middleware │ Observability      │
├─────────────────────────────────────────────┤
│  Coordination (Queue + Memory + Consensus)  │
└─────────────────────────────────────────────┘

## Feature Flags

| Flag | Description |
|------|-------------|
| `native-browser` | Built-in headless browser tools |

## License

MIT
```

### 4.3 Doc Examples 변환 규칙

| 패턴 | 변환 방법 |
|------|-----------|
| `OxicodeBuilder::new().build()` | ` ```rust ` 로 변경 — 외부 의존 없음 |
| `OxicodeBuilder::new().with_builtins().build()` | ` ```rust ` — 빌드만, API 호출 없음 |
| `AgentBuilder` 체인 | `workspace("/tmp")` + `.build()` — 컴파일 가능 |
| `create_builtin_provider()` | 제거 — 내부 API |
| Kernel / 외부 크레이트 의존 | ` ```ignore ` 유지 |

### 4.4 영향 범위

| 파일 | 변경 |
|------|------|
| `README.md` | 전면 재작성 |
| `examples/builder_demo.rs` | 실제 동작하는 코드로 교체 또는 삭제 |
| `src/builder.rs` doc | 4개 ignore → runnable |
| `src/agent_builder.rs` doc | 3개 ignore → runnable |
| `src/closure_tool.rs` doc | 1개 ignore → runnable |
| `src/multi_provider.rs` doc | 3개 ignore → runnable |

---

## 5. I-01: 에러 타입 일관성 — 실행 계층만 SdkResult

### 5.1 문제 정의 (검증 완료)

현재 `AgentHandle::run()`, `AgentGroup::run()`이 `anyhow::Result`를 반환.
호출자가 실패 원인을 match로 구분할 수 없다.

### 5.2 설계 원칙

**실행 계층만 변환. 빌더/래지스트리는 `anyhow::Result` 유지.**

| 메서드 | 현재 | 변경 | 이유 |
|--------|------|------|------|
| `AgentHandle::run()` | `anyhow::Result` | `SdkResult` | 핵심 실행 API |
| `AgentHandle::suspend()` | `anyhow::Result` | `SdkResult` | 수명주기 |
| `AgentHandle::terminate()` | `anyhow::Result` | `SdkResult` | 수명주기 |
| `AgentHandle::snapshot()` | `anyhow::Result` | `SdkResult` | 수명주기 |
| `AgentGroup::run()` | `anyhow::Result` | `SdkResult` | 오케스트레이션 |
| `AgentBuilder::build()` | `anyhow::Result` | **유지** | 빌더 관례 |
| `Oxicode::resolve_model()` | `anyhow::Result` | **유지** | 내부용 |
| `Oxicode::create_provider()` | `anyhow::Result` | **유지** | 내부용 |
| `AgentSupervisor::spawn()` | `anyhow::Result` | **유지** | 팩토리 |

### 5.3 SdkError 확장

```rust
// error.rs

pub type SdkResult<T> = Result<T, SdkError>;

#[derive(Debug, Error)]
pub enum SdkError {
    // ── 기존 variant 유지 ──
    ModelNotFound { model_id: String },
    ProviderNotFound { provider: String },
    // ...

    // ── 🆕 실행 계층 ──
    #[error("agent execution failed: {reason}")]
    ExecutionFailed {
        reason: String,
    },

    #[error("agent group failed: {failed}/{total} agents")]
    GroupExecutionFailed {
        failed: usize,
        total: usize,
    },

    #[error("run cancelled")]
    Cancelled,
}
```

### 5.4 AgentHandle 변환

```rust
impl AgentHandle {
    pub async fn run(
        &self,
        prompt: String,
    ) -> SdkResult<(Response, Vec<AgentEvent>)> {
        // CAS 상태 전환 실패
        if prev.is_err() {
            return Err(SdkError::AgentNotRunnable { ... });
        }

        match result {
            Ok((response, events)) => {
                // ... 기존 성공 처리
                Ok((response, events))
            }
            Err(e) => {
                // Agent 취소 vs 실행 에러 구분
                if self.cancel_flag() {
                    Err(SdkError::Cancelled)
                } else {
                    Err(SdkError::ExecutionFailed {
                        reason: e.to_string(),
                    })
                }
            }
        }
    }
}
```

### 5.5 하위 호환성

`SdkError`는 `From<anyhow::Error>`을 구현하므로 `?` 연산자 호환:

```rust
impl From<anyhow::Error> for SdkError {
    fn from(e: anyhow::Error) -> Self {
        SdkError::Internal(e)
    }
}
```

기존 코드에서 `handle.run(prompt).await?`는 그대로 동작.

---

## 6. I-02: AgentRunner — 동시성 제어 래퍼

### 6.1 문제 정의 (검증 완료)

`AgentGroup::run_parallel()`이 agent마다 `new_current_thread` 런타임을 생성:

```rust
tokio::task::spawn_blocking(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all().build().expect("...");
    rt.block_on(async { agent.run(prompt).await })
})
```

v1 설계에서 제안한 별도 `new_multi_thread` 런타임은:
- 이미 tokio runtime이 있는 사용자 환경에서 panic 가능
- 근본적 해결이 아님 (여전히 `block_on` 필요)

### 6.2 설계: Semaphore 기반 동시성 제어

별도 런타임 생성 없이 `spawn_blocking`의 기본 스레드 풀을 재사용하면서
동시성만 제어:

```rust
// oxicode-sdk/src/runner.rs — 신규 파일

/// Agent 실행 동시성 제어기.
///
/// 내부적으로 `spawn_blocking`을 사용하되, Semaphore로 동시 실행
/// 에이전트 수를 제한. 별도 런타임은 생성하지 않는다.
pub struct AgentRunner {
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl AgentRunner {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            semaphore: Arc::new(tokio::sync::Semaphore::new(max_concurrency)),
        }
    }

    /// 에이전트를 실행하고 JoinHandle을 반환.
    ///
    /// 내부적으로 current_thread 런타임에서 block_on 실행.
    /// Semaphore로 동시성 제한.
    pub fn run(
        &self,
        agent: Arc<Agent>,
        prompt: String,
    ) -> tokio::task::JoinHandle<anyhow::Result<(Response, Vec<AgentEvent>)>> {
        let sem = Arc::clone(&self.semaphore);
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(async {
                let _permit = sem.acquire().await.expect("semaphore closed");
                agent.run(prompt).await
            })
        })
    }
}
```

### 6.3 AgentGroup에 주입

```rust
pub struct AgentGroup {
    agents: Vec<Arc<Agent>>,
    strategy: GroupStrategy,
    delegation_parser: Arc<dyn DelegationParser>,
    runner: Option<Arc<AgentRunner>>,  // 🆕
}

impl AgentGroup {
    /// 커스텀 러너 설정.
    pub fn with_runner(mut self, runner: Arc<AgentRunner>) -> Self {
        self.runner = Some(runner);
        self
    }
}
```

`run_parallel()`에서 runner 사용 시 Semaphore 공유, 미사용 시 기존 방식.

---

## 7. I-03: 예제 5종 추가

### 7.1 예제 목록

| 파일 | 내용 | 의존 |
|------|------|-------|
| `examples/minimal.rs` | 최소 에이전트 빌드 + 실행 | API key 필요 |
| `examples/multi_agent.rs` | AgentGroup 병렬 실행 | API key 필요 |
| `examples/security.rs` | Capability + Authorizer | API key 불필요 |
| `examples/observability.rs` | Tracer + Audit + CostTracker | API key 불필요 |
| `examples/coordination.rs` | WorkQueue + SharedMemory + Consensus | API key 불필요 |

API key가 필요한 예제는 환경 변수에서 읽고, 없으면 graceful 종료.

### 7.2 `examples/security.rs` 예시

```rust
//! Security capability enforcement demo.
//!
//! Run: cargo run -p oxicode-sdk --example security

use oxicode_sdk::prelude::*;

fn main() {
    let audit = Arc::new(AuditLog::new(64));
    let authorizer = Arc::new(Authorizer::new(Arc::clone(&audit)));

    // coding 역할 정의 후 에이전트에 부여
    authorizer.define_role("coder", CapabilitySet::coding("/workspace"));
    authorizer.bind_role("dev-agent", "coder");

    // 권한 확인
    let subject = CapabilitySubject::Agent("dev-agent".into());

    assert!(authorizer.check(&subject, &Capability::FileRead {
        path_pattern: "/workspace/src/main.rs".into()
    }));
    assert!(!authorizer.check(&subject, &Capability::FileWrite {
        path_pattern: "/etc/passwd".into()
    }));

    // 감사 로그 확인
    let entries = audit.entries();
    println!("Audit entries: {}", entries.len());
    for entry in &entries {
        println!("  {:?}", entry);
    }
}
```

---

## 8. I-04: Plugin trait 기반 로딩

### 8.1 설계

기존 `PluginLoader`는 manifest 로딩만 하고 `loaded`가 항상 비어 있음.
안전한 `Plugin` trait을 추가하여 Rust-native 플러그인 등록 지원.

```rust
// middleware/plugin.rs에 추가

/// 플러그인 trait — 여러 미들웨어를 묶어서 제공.
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;

    /// 이 플러그인이 제공하는 미들웨어.
    fn middlewares(&self) -> Vec<Arc<dyn Middleware>>;

    /// 초기화 (선택). 기본 구현은 no-op.
    fn initialize(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// 종료 (선택).
    fn shutdown(&self) {}
}
```

`PluginLoader`에 `register_plugin()` 메서드 추가:

```rust
impl PluginLoader {
    /// Rust-native 플러그인 등록.
    pub fn register_plugin(&self, plugin: impl Plugin + 'static) -> anyhow::Result<()> {
        let mut plugins = self.plugins.write();
        let name = plugin.name().to_string();
        plugins.push(Box::new(plugin));
        tracing::info!(plugin = %name, "Plugin registered");
        Ok(())
    }

    /// 등록된 모든 플러그인의 미들웨어 수집.
    pub fn all_middlewares(&self) -> Vec<Arc<dyn Middleware>> {
        self.plugins.read()
            .iter()
            .flat_map(|p| p.middlewares())
            .collect()
    }
}
```

향후 `unsafe-plugins` feature에서 `.so`/`.dylib` 동적 로딩 지원은 별도.

---

## 9. I-05: Integration Test 스위트

### 9.1 MockProvider (검증된 구현)

```rust
// tests/common/mod.rs

use oxicode_ai::*;
use oxicode_ai::messages::*;
use oxicode_ai::types::*;
use futures::Stream;
use std::pin::Pin;

/// Stream mock — 마지막 사용자 메시지를 에코.
pub struct MockProvider;

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str { "mock" }

    async fn stream(
        &self,
        _model: &Model,
        context: &Context,
        _options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let last_msg = context.messages().last()
            .and_then(|m| m.content().first())
            .and_then(|b| b.as_text())
            .unwrap_or("mock response")
            .to_string();

        // AssistantMessage 생성 (필수 필드 모두 설정)
        let mut msg = AssistantMessage::new(
            Api::OpenAiChat,
            "mock",
            "mock/model",
        );
        msg.content.push(ContentBlock::Text(TextContent::new(&last_msg)));
        msg.stop_reason = StopReason::Stop;
        msg.usage = Usage {
            input: 100,
            output: 50,
            ..Default::default()
        };

        let partial = msg.clone();
        let events: Vec<ProviderEvent> = vec![
            ProviderEvent::Start { partial: partial.clone() },
            ProviderEvent::TextDelta {
                content_index: 0,
                delta: last_msg.clone(),
                partial: partial.clone(),
            },
            ProviderEvent::Done {
                reason: StopReason::Stop,
                message: msg,
            },
        ];

        Ok(Box::pin(futures::stream::iter(events)))
    }
}

pub fn mock_model() -> Model {
    Model::new(
        "mock/model",
        "Mock",
        Api::OpenAiChat,
        "mock",
        "http://localhost",
    )
}

pub fn mock_oxicode() -> Oxicode {
    OxicodeBuilder::new()
        .provider("mock", MockProvider)
        .model(mock_model())
        .build()
}
```

### 9.2 테스트 케이스

```rust
// tests/integration.rs

mod common;

#[tokio::test]
async fn full_pipeline_build_and_run() {
    let oxicode = common::mock_oxicode();
    let agent = oxicode.agent(AgentConfig {
        model_id: "mock/model".into(),
        max_iterations: 5,
        ..Default::default()
    })
    .workspace("/tmp")
    .build()
    .expect("build");

    let (response, events) = agent.run("Hello".into()).await.expect("run");
    assert!(!response.content.is_empty());
    assert!(!events.is_empty());
}

#[tokio::test]
async fn security_capability_enforcement() {
    let oxicode = common::mock_oxicode();
    let audit = Arc::new(AuditLog::new(64));
    let authorizer = Arc::new(Authorizer::new(Arc::clone(&audit)));

    authorizer.grant(
        CapabilitySubject::Agent("readonly".into()),
        CapabilitySet::read_only("/workspace"),
    );

    // 읽기 허용
    assert!(authorizer.check(
        &CapabilitySubject::Agent("readonly".into()),
        &Capability::FileRead { path_pattern: "/workspace/file".into() },
    ));
    // 쓰기 거부
    assert!(!authorizer.check(
        &CapabilitySubject::Agent("readonly".into()),
        &Capability::FileWrite { path_pattern: "/workspace/file".into() },
    ));
}

#[tokio::test]
async fn work_queue_lifecycle() {
    let q = WorkQueue::new(WorkQueueConfig::default());
    let id = q.enqueue("review", json!({"file": "a.rs"}), 5);
    let item = q.claim("agent-1", None).unwrap();
    assert_eq!(item.id, id);
    q.start(&id).unwrap();
    q.complete(&id, WorkResult {
        success: true, content: "ok".into(), error: None,
        duration_ms: 50, tokens_used: None,
    }).unwrap();
    assert_eq!(q.stats().completed, 1);
}

#[tokio::test]
async fn shared_memory_optimistic_locking() {
    let mem = SharedMemory::new();
    let key = MemoryKey::new("ns", "val");

    let v1 = mem.write(&key, json!("a"), "w1", None).unwrap();
    assert_eq!(v1, 1);

    // 정상 업데이트
    let v2 = mem.write(&key, json!("b"), "w2", Some(v1)).unwrap();
    assert_eq!(v2, 2);

    // 충돌
    let result = mem.write(&key, json!("c"), "w3", Some(1));
    assert!(matches!(result, Err(SdkError::VersionConflict { .. })));
}

#[tokio::test]
async fn consensus_voting() {
    let c = Consensus::new();
    c.start("v1", vec!["a".into(), "b".into(), "c".into()], 0.5);
    c.vote("v1", "a", "yes".into()).unwrap();
    let r = c.vote("v1", "b", "yes".into()).unwrap();
    assert!(r.decided);
    assert_eq!(r.decision.unwrap(), "yes");
}

#[tokio::test]
async fn message_bus_pub_sub() {
    let bus = MessageBus::new(16);
    let mut rx = bus.subscribe();
    bus.publish(InterAgentMessage::broadcast(
        "coord", "start", json!({"phase": 1}),
    ));
    let msg = rx.recv().await.unwrap();
    assert_eq!(msg.message_type, "start");
}

#[tokio::test]
async fn observability_smoke() {
    let tracer = Tracer::new();
    let audit = AuditLog::new(64);
    let registry = Arc::new(ModelRegistry::new());
    let cost = CostTracker::new(registry, CostTrackerConfig::default());

    {
        let _span = tracer.start("run", SpanKind::Agent);
    }
    audit.log(AuditEntry::lifecycle("a1".into(), "started".into()));
    cost.record("a1", &common::mock_model(), TokenUsage {
        input: 100, output: 50, ..Default::default()
    });

    // Tracer 검증 — subscribe로 완료 span 수신
    // Audit 검증
    assert_eq!(audit.entries().len(), 1);
    // Cost 검증
    let snap = cost.snapshot("a1").unwrap();
    assert_eq!(snap.usage.input, 100);
    assert_eq!(snap.usage.output, 50);
}

#[tokio::test]
async fn event_store_replay() {
    let store = EventStore::default();
    store.append("order-1", "Created", json!({"id": 1}));
    store.append("order-1", "Paid", json!({"amount": 100}));
    store.append("order-2", "Created", json!({"id": 2}));

    let events = store.replay("order-1");
    assert_eq!(events.len(), 2);
}
```

---

## 10. N-01: 스트리밍 응답 API

### 10.1 설계 (검증 반영)

`Agent::run_streaming(prompt, FnMut(AgentEvent))`가 `!Send` future를 반환하므로
`spawn_blocking` + `blocking_send` 필요:

```rust
// oxicode-sdk/src/streaming.rs — 신규 파일

/// 스트리밍 이벤트 수신기.
pub struct EventStream {
    rx: mpsc::Receiver<AgentEvent>,
    _handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl EventStream {
    pub async fn next(&mut self) -> Option<AgentEvent> {
        self.rx.recv().await
    }
}
```

```rust
// AgentHandle에 추가

impl AgentHandle {
    pub async fn run_streaming(&self, prompt: String) -> SdkResult<EventStream> {
        // CAS 상태 전환 (run()과 동일)
        let prev = self.status.compare_exchange(/* ... */);
        if prev.is_err() {
            return Err(SdkError::AgentNotRunnable { ... });
        }

        let (tx, rx) = mpsc::channel(256);
        let agent = Arc::clone(&self.agent);

        let handle = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build()?;
            rt.block_on(async {
                agent.run_streaming(prompt, move |event| {
                    let _ = tx.blocking_send(event);
                }).await
            })
        });

        Ok(EventStream { rx, _handle: handle })
    }
}
```

---

## 11. N-02: Configuration Hot-Reload

### 11.1 설계 (검토에서 승인)

`notify` crate으로 파일 변경 감지, `tokio::sync::watch`로 업데이트 전달.

```toml
# Cargo.toml — optional feature
[features]
config-watch = ["notify"]

[dependencies]
notify = { version = "7", optional = true }
```

```rust
#[cfg(feature = "config-watch")]
pub struct ConfigWatcher {
    watcher: RecommendedWatcher,
    rx: tokio::sync::watch::Receiver<ConfigUpdate>,
}
```

---

## 12. N-03: Metrics Export (OTLP)

### 12.1 설계 (검토에서 승인)

```rust
pub trait MetricsExporter: Send + Sync {
    fn export(&self, snapshot: &MetricsSnapshot, agent_id: &str);
}
```

`MetricsSnapshot`에서 이미 충분한 데이터 제공. 추가 의존성은 feature gate로.

---

## 13. N-04: Agent 간 고급 협업 패턴

### 13.1 설계 (검토에서 승인)

- `DelegationChain`: MessageBus 기반 위임/보고
- `RecursiveSpawnGuard`: 깊이 제한 재귀 스폰

순환 위임(A→B→A) 방지를 위해 `visited: HashSet<String>` 추적 추가:

```rust
pub struct DelegationChain {
    bus: Arc<MessageBus>,
    delegates: RwLock<HashMap<String, Vec<String>>>,
    active_delegations: RwLock<HashSet<(String, String)>>,  // (from, to)
}

impl DelegationChain {
    pub async fn delegate(&self, from: &str, to: &str, task: DelegatedTask)
        -> anyhow::Result<()>
    {
        // 순환 감지
        let key = (from.to_string(), to.to_string());
        if self.active_delegations.read().contains(&key) {
            anyhow::bail!("Circular delegation detected: {from} → {to}");
        }
        self.active_delegations.write().insert(key.clone());
        // ...
    }
}
```

---

## 14. 마이그레이션 가이드

### 14.1 Breaking Changes

#### `AgentMetrics::record_success` 시그니처

```rust
// BEFORE (v0.23)
metrics.record_success(duration, total_tokens, tool_calls);

// AFTER (v0.24)
metrics.record_success(duration, input_tokens, output_tokens, tool_count);
```

SDK 외부에서 `AgentMetrics`를 직접 사용하는 경우만 영향.
`AgentHandle::run()` 사용 시 자동 처리.

#### `MetricsSnapshot` 필드 추가

`#[serde(default)]`로 역직렬화 호환. 기존 JSON 파일에서 로드 시 새 필드는 0.

#### `AgentHandle::run()` 반환 타입

```rust
// BEFORE
let result: anyhow::Result<...> = handle.run(prompt).await;

// AFTER
let result: SdkResult<...> = handle.run(prompt).await;
// ? 연산자는 그대로 동작 (SdkError: From<anyhow::Error>)
// match로 세분화 가능:
match handle.run(prompt).await {
    Ok((response, events)) => { ... },
    Err(SdkError::Cancelled) => { /* 사용자 취소 */ },
    Err(SdkError::ExecutionFailed { reason }) => { /* 실행 에러 */ },
    Err(e) => { /* 기타 */ },
}
```

#### `AgentGroup` 신규 메서드

`with_delegation_parser()`, `with_runner()` — 기존 `new()` + `agent()` API는 변경 없음.

### 14.2 Non-Breaking Additions

- `DelegatedTask`, `DelegationParser`, `JsonDelegationParser`
- `AgentRunner`
- `SdkResult<T>`, `SdkError` 신규 variant
- `Plugin` trait
- `EventStream`
- `DelegationChain`, `RecursiveSpawnGuard`

---

## 15. 구현 일정

### Phase 1 — Critical (v0.24.0, 2주)

```
Week 1
├── C-01 Step 1: oxicode-agent streaming.rs에 record_usage + emit(Usage) 추가
├── C-01 Step 2: SDK metrics.rs 확장 (record_success 시그니처, MetricsSnapshot)
├── C-01 Step 3: SDK supervisor.rs에서 agent.state()로 토큰 수 읽기
└── C-03: README.md 재작성 + Doc examples 변환

Week 2
├── C-02: Orchestrated 전략 (DelegationParser, run_orchestrated 재구현)
├── I-01: SdkError 확장 + AgentHandle/AgentGroup만 SdkResult 변환
├── I-03: examples/ 5종 추가
└── I-05: Integration test suite (MockProvider + 8개 테스트)
```

### Phase 2 — Important (v0.24.1, 2주)

```
Week 3
├── I-02: AgentRunner (spawn_blocking + Semaphore)
├── I-04: Plugin trait + PluginLoader.register_plugin()
└── N-01: EventStream + AgentHandle::run_streaming()

Week 4
├── N-02: ConfigWatcher (config-watch feature)
└── N-04: DelegationChain + RecursiveSpawnGuard
```

### Phase 3 — Nice-to-Have (v0.25.0, 2주)

```
Week 5-6
├── N-03: MetricsExporter trait + JSON exporter
└── oxicode-agent: Agent::run() Send future 근본 해결 (별도 설계)
```

### 검증 체크리스트 (매 Phase 종료 시)

```bash
# 1. oxicode-agent 테스트 (Phase 1 필수)
cargo test -p oxicode-agent

# 2. SDK 전체 테스트
cargo test -p oxicode-sdk --all-features

# 3. Doc test
cargo test -p oxicode-sdk --doc --all-features

# 4. Integration test
cargo test -p oxicode-sdk --test integration

# 5. Clippy
cargo clippy -p oxicode-sdk --all-features -- -D warnings

# 6. Format
cargo fmt -p oxicode-sdk -- --check

# 7. Examples build
cargo build -p oxicode-sdk --examples --all-features
```

---

## Appendix: 신규 파일 및 타입 요약

### 신규 파일

```
oxicode-sdk/
├── src/
│   ├── runner.rs                  # AgentRunner (I-02)
│   └── streaming.rs               # EventStream (N-01)
├── tests/
│   ├── common/mod.rs              # MockProvider + 헬퍼 (I-05)
│   └── integration.rs             # 8개 E2E 테스트 (I-05)
└── examples/
    ├── minimal.rs                 # (I-03)
    ├── multi_agent.rs             # (I-03)
    ├── security.rs                # (I-03)
    ├── observability.rs           # (I-03)
    └── coordination.rs            # (I-03)
```

### 신규 공개 타입

| 타입 | 모듈 | 설명 |
|------|------|------|
| `SdkResult<T>` | `error` | 통일 결과 타입 |
| `AgentRunner` | `runner` | 동시성 제어 실행 래퍼 |
| `EventStream` | `streaming` | 스트리밍 이벤트 수신기 |
| `DelegatedTask` | `agent_group` | 위임 작업 DTO |
| `DelegationParser` | `agent_group` | 위임 파싱 trait |
| `JsonDelegationParser` | `agent_group` | 기본 JSON 위임 파서 |
| `Plugin` | `middleware` | 플러그인 trait |
| `DelegationChain` | `coordination` | 에이전트 위임 체인 (N-04) |
| `RecursiveSpawnGuard` | `coordination` | 재귀 스폰 가드 (N-04) |
| `ConfigWatcher` | `config_watcher` | 설정 감시 (N-02) |
| `MetricsExporter` | `export` | 메트릭 익스포터 trait (N-03) |

### oxicode-agent 변경 사항 (Phase 1)

| 파일 | 변경 |
|------|------|
| `agent_loop/streaming.rs` | `ProviderEvent::Done` 처리에 `record_usage` + `emit(Usage)` 추가 (~5줄) |
