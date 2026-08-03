# oxicode-sdk 설계 문서 검토 보고서

> 각 개선 항목(C-01 ~ N-04)을 실제 코드베이스와 교차 검증한 결과.
> 설계 문서: `oxicode-sdk/DESIGN_IMPROVEMENTS.md`

---

## 검토 요약

```
┌──────────┬──────────────────────────────────────────┬────────────┬──────────┐
│ ID       │ 항목                                     │ 설계 정확도 │ 판정     │
├──────────┼──────────────────────────────────────────┼────────────┼──────────┤
│ C-01     │ Token Usage 추출 파이프라인              │ 🔴 부정확   │ 재설계   │
│ C-02     │ Orchestrated 전략 구현                   │ 🟡 부분정확 │ 보완     │
│ C-03     │ Doc Examples 컴파일 검증                 │ 🟢 정확     │ 승인     │
│ I-01     │ SdkResult 에러 타입 통일                 │ 🟡 부분정확 │ 보완     │
│ I-02     │ AgentGroup Send 안전 래퍼                │ 🟡 부분정확 │ 보완     │
│ I-03     │ README 및 예제 현대화                    │ 🟢 정확     │ 승인     │
│ I-04     │ PluginLoader 실구현                      │ 🟡 부분정확 │ 보완     │
│ I-05     │ Integration Test 스위트                  │ 🟡 부분정확 │ 보완     │
│ N-01     │ 스트리밍 응답 API                        │ 🟡 부분정확 │ 보완     │
│ N-02     │ Configuration Hot-Reload                 │ 🟢 정확     │ 승인     │
│ N-03     │ Metrics Export (OTLP)                    │ 🟢 정확     │ 승인     │
│ N-04     │ Agent 간 고급 협업 패턴                  │ 🟢 정확     │ 승인     │
└──────────┴──────────────────────────────────────────┴────────────┴──────────┘
```

---

## C-01: Token Usage 추출 파이프라인 — 🔴 재설계 필요

### 설계 문서의 가정

> `AgentEvent::Usage { input_tokens, output_tokens }`가 이미 이벤트 스트림에 존재하므로,
> 이벤트 벡터에서 순회하여 합산하면 된다.

### 실제 코드 조사 결과

**`AgentEvent::Usage`는 정의되어 있지만 emit되지 않는다.**

```
oxicode-agent/src/events.rs          → AgentEvent::Usage { input_tokens, output_tokens } 정의됨 ✅
oxicode-agent/src/agent_loop/mod.rs  → emit(AgentEvent::Usage ...) 호출 없음 ❌
oxicode-agent/src/agent_loop/streaming.rs → Usage 관련 코드 전혀 없음 ❌
```

`stream_assistant_response()`에서 `ProviderEvent::Done { message }`를 수신하고
`message.usage` (`oxicode_ai::Usage { input, output, cache_read, cache_write }`)에
접근하지만, 이를 `AgentEvent::Usage`로 emit하지 않는다.

따라서 설계 문서의 `extract_token_usage(events)` 접근은 **동작하지 않는다**.

### 근본 원인

문제는 SDK 레벨이 아니라 **oxicode-agent 레벨**에 있다.
`stream_assistant_response()`에서 `Done` 이벤트를 받을 때 usage를 emit해야 한다.

### 올바른 해결 방법

**Step 1: oxicode-agent에서 `AgentEvent::Usage` emit (근본 수정)**

```rust
// oxicode-agent/src/agent_loop/streaming.rs
// ProviderEvent::Done 처리 블록 안에 추가:

ProviderEvent::Done { message, .. } => {
    // ✅ 기존 코드: circuit breaker, message 처리...

    // 🆕 Usage 이벤트 emit
    if message.usage.input > 0 || message.usage.output > 0 {
        emit(AgentEvent::Usage {
            input_tokens: message.usage.input,
            output_tokens: message.usage.output,
        });
    }

    // ✅ 기존 코드 계속...
}
```

**Step 2: 그 후 SDK에서 이벤트 순회 (설계 문서대로)**

```rust
// oxicode-sdk/src/metrics.rs
pub fn extract_token_usage(events: &[AgentEvent]) -> (u64, u64) {
    // Step 1이 완료된 후에만 동작
}
```

### 설계 문서 수정 사항

| 항목 | 기존 설계 | 수정 |
|------|-----------|------|
| 작업 범위 | SDK만 (`metrics.rs`, `supervisor.rs`) | **oxicode-agent 포함** (`streaming.rs`) |
| 작업 규모 | M | **M+** (크로스 크레이트) |
| `extract_token_usage` | 즉시 사용 가능 | **Step 1 완료 후에만 사용 가능** |

### 검증 체크리스트

- [ ] `oxicode-agent/src/agent_loop/streaming.rs`에 `AgentEvent::Usage` emit 추가
- [ ] `cargo test -p oxicode-agent` 통과
- [ ] SDK `extract_token_usage()` 단위 테스트 (mock events로)
- [ ] `AgentHandle::run()`에서 `record_success`에 실제 토큰 수 반영

---

## C-02: Orchestrated 전략 구현 — 🟡 보완 필요

### 설계 문서의 가정

> Leader가 JSON 배열로 위임 작업을 반환하고, workers가 병렬 실행.

### 실제 코드 조사 결과

현재 `run_orchestrated`가 leader만 실행하는 것은 정확히 파악됨.

**문제점 1: prompt engineering에 의존**

`JsonDelegationParser`가 LLM 응답에서 JSON을 파싱하는데, 이는 모델이 지시를
따르지 않을 경우 실패한다. fallback 전략이 필요하다.

**문제점 2: GroupStrategy enum 변경이 breaking**

```rust
// 현재
pub enum GroupStrategy {
    Orchestrated { leader: usize },
}

// 설계에서 제안
pub enum GroupStrategy {
    Orchestrated { leader: usize, delegation_parser: Option<Arc<dyn DelegationParser>> },
}
```

`GroupStrategy`는 `Clone`이어야 하고 (현재 `#[derive(Clone)]`), `Arc<dyn DelegationParser>`
를 포함하면 `Clone` 구현이 복잡해진다.

**문제점 3: `spawn_blocking` 사용이 설계에 누락**

`Agent::run()`이 `!Send`이므로 worker 실행도 `spawn_blocking`을 사용해야 한다.
설계 코드에 이 부분이 반영되어 있지 않다.

### 권장 수정

1. `GroupStrategy::Orchestrated`에 `delegation_parser`를 추가하지 말고,
   `AgentGroup` 필드로 분리:

   ```rust
   pub struct AgentGroup {
       agents: Vec<Arc<Agent>>,
       strategy: GroupStrategy,
       delegation_parser: Option<Arc<dyn DelegationParser>>,
   }
   ```

2. Fallback 전략 추가: JSON 파싱 실패 시 전체 prompt를 모든 worker에게 전달.

3. Worker 실행에 `spawn_blocking` 명시.

4. `DelegationParser`에 `Clone` 가능한 wrapper 제공 또는 `Arc` 필드만 사용.

### 검증 체크리스트

- [ ] `GroupStrategy`는 변경하지 않고 `AgentGroup` 필드로 parser 주입
- [ ] Fallback 전략: JSON 파싱 실패 → 전체 prompt를 worker에게 전달
- [ ] `spawn_blocking`으로 worker 실행
- [ ] `DelegationParser` trait에 `Clone` bounding 없이 `Arc`로 관리

---

## C-03: Doc Examples 컴파일 검증 — 🟢 승인

### 검증

| 검증 항목 | 결과 |
|-----------|------|
| ` ```ignore ` 블록 20개 존재 | ✅ 확인 |
| 변환 가능한 예제 파악 | ✅ 적절 |
| CI 통합 제안 | ✅ 적절 |

추가 발견: 일부 doc example이 존재하지 않는 API 사용.

```rust
// builder_demo.rs와 README.md에서
.include_builtins(true)   // ❌ 실제는 .with_builtins()
.model_id("...")          // ❌ AgentBuilder에 model_id() 메서드 없음
```

이것도 C-03 범위에 포함하여 수정해야 함.

### 보완 사항

- Doc example뿐 아니라 `README.md`와 `examples/builder_demo.rs`의
  존재하지 않는 API 호출도 함께 수정.

---

## I-01: SdkResult 에러 타입 통일 — 🟡 보완 필요

### 설계 문서의 가정

> `AgentHandle::run()`, `AgentGroup::run()` 등이 `anyhow::Result`를 반환.

### 실제 코드 조사 결과 — 정확함

```rust
// supervisor.rs:169
pub async fn run(&self, ...) -> anyhow::Result<(Response, Vec<AgentEvent>)>

// agent_group.rs:6, 89
use anyhow::Result;
pub async fn run(&self, prompt: String) -> Result<GroupResult>
```

**문제점: 변환 범위 과대**

설계는 `Oxicode::resolve_model()`과 `Oxicode::create_provider()`도 `SdkResult`로
변환하라고 제안하지만, 이 메서드들은 `AgentBuilder::build()` 내부에서만
호출되며, `build()`는 이미 `anyhow::Result`를 반환한다.

`Oxicode` 레벨의 공개 메서드를 `SdkResult`로 바꾸면 사용자가 `Oxicode`를
직접 사용할 때 `anyhow` 에러를 받지 못하게 되어 오히려 불편해진다.

### 권장 수정

**변환 범위를 좁게 설정:**

| 변환 대상 | 여부 | 이유 |
|-----------|------|------|
| `AgentHandle::run()` | ✅ 변환 | SDK의 핵심 실행 API |
| `AgentHandle::suspend()` | ✅ 변환 | 수명주기 API |
| `AgentHandle::terminate()` | ✅ 변환 | 수명주기 API |
| `AgentGroup::run()` | ✅ 변환 | 오케스트레이션 API |
| `AgentBuilder::build()` | ❌ 유지 | 빌더 패턴은 `anyhow`가 관례 |
| `Oxicode::resolve_model()` | ❌ 유지 | 내부용, `anyhow`가 간편 |
| `Oxicode::create_provider()` | ❌ 유지 | 내부용 |
| `AgentSupervisor::spawn()` | ❌ 유지 | 내부 구성 |

`SdkError`에 새 variant만 추가하고, 실행 계층(`AgentHandle`, `AgentGroup`)만
`SdkResult`로 변환.

### 검증 체크리스트

- [ ] `SdkError`에 `ExecutionFailed`, `GroupExecutionFailed`, `Cancelled` 추가
- [ ] `AgentHandle::run()`, `suspend()`, `terminate()` → `SdkResult`
- [ ] `AgentGroup::run()` → `SdkResult`
- [ ] 나머지는 `anyhow::Result` 유지
- [ ] `From<anyhow::Error>` impl으로 `?` 호환성 유지

---

## I-02: AgentGroup Send 안전 래퍼 — 🟡 보완 필요

### 설계 문서의 가정

> `Agent::run()`이 `!Send` future를 반환하므로 `spawn_blocking`이 필요.

### 실제 코드 조사 결과 — 정확함

```rust
// agent_group.rs:192-206
handles.push(tokio::task::spawn_blocking(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create runtime");
    rt.block_on(async move { ... })
}));
```

**문제점 1: AgentRunner 설계에 치명적 결함**

설계에서 제안한 `AgentRunner::run()`이 `tokio::spawn_blocking`을 사용한다면,
`new_current_thread()` 런타임과 같은 문제를 공유한다. `new_multi_thread` 런타임을
별도로 만들어도 결국 `block_on`으로 감싸야 하므로 근본 해결이 아니다.

**문제점 2: 런타임 중첩 위험**

SDK 사용자가 이미 tokio 런타임을 가지고 있는 경우, `AgentRunner::new()`에서
`Runtime::new()`를 호출하면 panic이 발생할 수 있다.

### 권장 수정

`AgentRunner`를 별도 런타임 생성이 아닌, `spawn_blocking` 풀의 재사용으로 단순화:

```rust
/// 단순화: 런타임 생성 없이 spawn_blocking 재사용
pub struct AgentRunner {
    semaphore: Arc<Semaphore>,
    max_concurrency: usize,
}

impl AgentRunner {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            max_concurrency,
        }
    }

    pub fn run(&self, agent: Arc<Agent>, prompt: String)
        -> JoinHandle<anyhow::Result<(Response, Vec<AgentEvent>)>>
    {
        let sem = Arc::clone(&self.semaphore);
        tokio::task::spawn_blocking(move || {
            // 현재 방식과 동일하지만 semaphore로 동시성 제어
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build()?;
            rt.block_on(async {
                let _permit = sem.acquire().await.unwrap();
                agent.run(prompt).await
            })
        })
    }
}
```

핵심: 별도 런타임 생성을 피하고, 동시성 제어만 추가.

### 검증 체크리스트

- [ ] 별도 `Runtime::new()` 생성 금지
- [ ] `spawn_blocking` 기반 + `Semaphore` 동시성 제어
- [ ] `AgentGroup`에 `AgentRunner` 주입 (선택)

---

## I-03: README 및 예제 현대화 — 🟢 승인

### 검증

README에서 실제로 구API를 사용하는 것을 확인:

```rust
// README.md 현재 코드 (작동하지 않음)
.include_builtins(true)   // ❌ 실제: .with_builtins()
.model_id("claude-sonnet-4-20250514")  // ❌ 실제: AgentConfig로 전달
```

설계 문서의 README 재작성 제안이 정확함. 새 예제 5개 구성도 적절.

### 검증 체크리스트

- [ ] README.md 재작성 (올바른 API 사용)
- [ ] `examples/builder_demo.rs` 수정 또는 삭제 후 대체
- [ ] 5개 신규 예제 추가

---

## I-04: PluginLoader 실구현 — 🟡 보완 필요

### 설계 문서의 가정

> `PluginLoader`가 manifest 로딩만 하고 실제 로딩이 없다.

### 실제 코드 조사 결과 — 정확함

```rust
// middleware/plugin.rs
pub struct PluginLoader {
    plugins_dir: PathBuf,
    loaded: Arc<RwLock<HashMap<String, Arc<dyn Middleware>>>>,
    manifests: Arc<RwLock<HashMap<String, PluginManifest>>>,
}

// load()는 manifest만 파싱하고 loaded에 아무것도 넣지 않음
pub async fn load(&self, manifest_path: &Path) -> anyhow::Result<String> {
    let manifest = PluginManifest::from_file(manifest_path)?;
    let name = manifest.name.clone();
    manifests.insert(name.clone(), manifest);
    Ok(name)  // ← loaded는 여전히 비어있음
}
```

**문제점: trait 기반 설계가 기존 API와 충돌**

기존 `PluginLoader`는 파일 시스템 기반 (manifest → plugin) 흐름이다.
설계에서 제안한 `Plugin` trait은 파일 없이 직접 등록하는 방식이다.
두 방식을 하나의 `PluginLoader`에 섞으면 혼란.

### 권장 수정

**두 가지 로딩 방식을 명확히 분리:**

```rust
// 방식 1: 직접 등록 (안전, Rust-native)
impl PluginLoader {
    pub fn register(&self, plugin: impl Plugin + 'static) -> anyhow::Result<()> { ... }
}

// 방식 2: Manifest 기반 (향후 unsafe-plugins feature에서 구현)
impl PluginLoader {
    pub async fn load(&self, manifest_path: &Path) -> anyhow::Result<String> { ... }
}
```

현재는 방식 1만 구현하고, 방식 2는 `unsafe-plugins` feature로 미룬다는
설계의 방향은 맞다. 다만 `Plugin` trait이 `middleware/`에 정의되는 것이
자연스러운지, 아니면 별도 `plugin.rs`에 있어야 하는지 고민이 필요.

`Plugin` trait을 `Middleware`와 분리하면:
- `Plugin`은 여러 middleware를 제공할 수 있음 ✅
- 초기화/종료 수명 주기를 가짐 ✅
- `Middleware` trait은 파이프라인 실행에만 집중 ✅

설계의 `Plugin` trait 방향은 올바름. `PluginContext`에 `ModelRegistry` 대신
빈 컨텍스트를 기본으로 제공하는 것이 좋겠음 (초기화 시점에 registry가 없을 수 있음).

### 검증 체크리스트

- [ ] `Plugin` trait 정의 (`middleware/` 내 또는 별도)
- [ ] `PluginContext` 기본값 제공
- [ ] `PluginLoader::register()` 구현 (trait 기반)
- [ ] `PluginLoader::load()`는 manifest만 (향후 확장)
- [ ] `middlewares()`가 등록된 plugin에서 middleware 수집

---

## I-05: Integration Test 스위트 — 🟡 보완 필요

### 설계 문서의 가정

> MockProvider 기반 E2E 테스트를 작성한다.

### 실제 코드 조사 결과

**문제점: EchoProvider가 동작하지 않을 수 있음**

```rust
struct EchoProvider;

impl Provider for EchoProvider {
    async fn stream(&self, ...) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ...> {
        let stream = futures::stream::iter(vec![
            ProviderEvent::TextDelta { text: last_msg },
            ProviderEvent::EndTurn,  // ← 존재하지 않는 variant!
        ]);
    }
}
```

`ProviderEvent` enum을 확인한 결과 `EndTurn` variant가 없다.
스트림 종료는 `ProviderEvent::Done { reason, message }`를 사용해야 한다.

`Done`에는 완전한 `AssistantMessage`가 필요하므로 MockProvider 구현이 더 복잡해진다.

### 올바른 MockProvider 설계

```rust
struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str { "mock" }

    async fn stream(
        &self,
        _model: &Model,
        context: &Context,
        _options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let last_msg = context.messages.last()
            .and_then(|m| m.content().first())
            .and_then(|b| b.as_text())
            .unwrap_or("mock response")
            .to_string();

        let mut msg = AssistantMessage::new(
            Api::OpenAiChat,
            "mock",
            "mock-model",
        );
        msg.content.push(ContentBlock::Text(TextContent::new(&last_msg)));
        msg.stop_reason = StopReason::EndTurn;

        let stream = futures::stream::iter(vec![
            ProviderEvent::Start { partial: msg.clone() },
            ProviderEvent::TextDelta {
                content_index: 0,
                delta: last_msg,
                partial: msg.clone(),
            },
            ProviderEvent::Done {
                reason: StopReason::EndTurn,
                message: msg,
            },
        ]);

        Ok(Box::pin(stream))
    }
}
```

### 추가 문제점

`Agent::run()`은 내부적으로 tool execution을 시도할 수 있으므로,
`max_iterations: 1`로 설정해도 여러 턴이 돌 수 있다.
mock agent가 tool call을 하지 않도록 tool-less config로 테스트해야 함.

### 검증 체크리스트

- [ ] `ProviderEvent::Done`을 사용하는 MockProvider
- [ ] `AssistantMessage::new()` 필수 필드 모두 설정
- [ ] 빈 `ToolRegistry`로 agent 생성 (tool call 유도 방지)
- [ ] `max_iterations: 1`로 설정
- [ ] `StopReason::EndTurn` 사용 (variant 이름 확인)

---

## N-01: 스트리밍 응답 API — 🟡 보완 필요

### 설계 문서의 가정

> `Agent::run_streaming()`이 존재하므로 SDK에서 래핑.

### 실제 코드 조사 결과

```rust
// agent.rs:742
pub async fn run_streaming<F>(&self, prompt: String, mut on_event: F) -> Result<Response>
where
    F: FnMut(AgentEvent) + Send,
```

**문제점: `run_streaming`도 `!Send` future를 반환한다**

`run_streaming`은 내부적으로 `run_with_channel`을 호출하며, 동일한 `!Send` 문제를 가진다.
따라서 `EventStream`이 `mpsc::Receiver`를 사용하더라도
`run_streaming` 자체를 `spawn_blocking`으로 감싸야 한다.

**문제점: `run_streaming`이 `FnMut`을 사용**

`run_streaming`은 `FnMut(AgentEvent)`를 받는데,
`spawn_blocking` 내부에서 `blocking_send`를 호출하려면
callback이 `Send`여야 한다. `FnMut + Send`는 closure가 `Send`이어야 함.

### 권장 수정

```rust
impl AgentHandle {
    pub async fn run_streaming(
        &self,
        prompt: String,
    ) -> SdkResult<EventStream> {
        let (tx, rx) = mpsc::channel(256);
        let agent = Arc::clone(&self.agent);

        // spawn_blocking 내에서 실행
        let handle = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build()?;

            rt.block_on(async {
                agent.run_streaming(prompt, move |event| {
                    // FnMut + Send closure
                    let _ = tx.blocking_send(event);
                }).await
            })
        });

        Ok(EventStream { rx, _handle: handle })
    }
}
```

### 검증 체크리스트

- [ ] `spawn_blocking`으로 `run_streaming` 실행
- [ ] `mpsc::blocking_send`를 callback 내에서 사용
- [ ] `EventStream`이 `JoinHandle`을 소유하여 완료 대기

---

## N-02: Configuration Hot-Reload — 🟢 승인

### 검증

`notify` crate을 사용한 파일 감시, `tokio::sync::watch` 채널로 업데이트 전달.
설계가 깔끔하고 실현 가능.

**주의점:** `notify` crate을 새 의존성으로 추가해야 함.
기존 의존성에 포함되어 있지 않으므로 Cargo.toml 수정 필요.

### 검증 체크리스트

- [ ] `notify` 의존성 추가 (optional feature로?)
- [ ] `ConfigWatcher`가 파일 없을 때 graceful 처리
- [ ] 잘못된 포맷의 config 파일에 대한 에러 핸들링

---

## N-03: Metrics Export (OTLP) — 🟢 승인

### 검증

`MetricsExporter` trait + `OtlpExporter` 구현.
`AgentMetrics::snapshot()`에서 이미 `MetricsSnapshot`을 제공하므로
export 포맷만 추가하면 됨.

`reqwest`는 이미 indirect dependency일 가능성이 높으나 확인 필요.
아니면 `oxicode-ai`의 HTTP 기반 provider들이 이미 끌어오고 있을 것.

### 검증 체크리스트

- [ ] `reqwest` 의존성 확인 (또는 feature gate)
- [ ] OTLP payload 포맷 검증
- [ ] 주기적 export 타이머가 `AgentRunner` 종료 시 정리되는지

---

## N-04: Agent 간 고급 협업 패턴 — 🟢 승인

### 검증

`DelegationChain`과 `RecursiveSpawnGuard` 모두 기존
`MessageBus`, `AgentSupervisor` 위에 자연스럽게 구축 가능.

`RecursiveSpawnGuard`의 `AtomicUsize` 깊이 추적이 단순하고 효과적.

### 주의점

`DelegationChain`이 `MessageBus`에만 의존하므로 테스트가 용이.
실제 agent 실행 없이 bus 메시지로 delegation 흐름을 테스트할 수 있음.

### 검증 체크리스트

- [ ] `DelegationChain`의 bus-only 테스트
- [ ] `RecursiveSpawnGuard`의 깊이 제한 테스트
- [ ] 순환 위임 방지 (A → B → A) 고려

---

## 종합: 설계 문서 수정 우선순위

### 반드시 수정 (구현 전)

| 항목 | 문제 | 액션 |
|------|------|------|
| **C-01** | `AgentEvent::Usage`가 emit되지 않음 | oxicode-agent 수정을 Step 0으로 추가 |
| **I-05** | `ProviderEvent::EndTurn` 미존재 | MockProvider를 `Done` 기반으로 재작성 |
| **C-03** | README에 구API 존재 | README 수정을 C-03 범위에 포함 |

### 권장 수정 (품질 향상)

| 항목 | 문제 | 액션 |
|------|------|------|
| **C-02** | `GroupStrategy` enum breaking change | parser를 `AgentGroup` 필드로 |
| **I-01** | 변환 범위 과대 | 실행 계층만 변환, 빌더/래지스트리는 유지 |
| **I-02** | 별도 런타임 생성 위험 | `spawn_blocking` + Semaphore로 단순화 |
| **N-01** | `!Send` future + `FnMut` 제약 | `spawn_blocking` + `blocking_send` 명시 |

### 승인 (그대로 구현 가능)

| 항목 | 비고 |
|------|------|
| **C-03** | Doc examples 변환 |
| **I-03** | README 현대화 |
| **I-04** | Plugin trait (PluginContext 기본값 제공 권장) |
| **N-02** | ConfigWatcher (`notify` 의존성 추가) |
| **N-03** | Metrics Export |
| **N-04** | 고급 협업 패턴 |
