# oxi 프로젝트 아키텍처 분석

> 분석 일자: 2026-05-06
> 대상 버전: 0.5.0
> 분석 대상: `oxi-ai`, `oxi-agent`, `oxi-tui`, `oxi-cli` (Rust 워크스페이스)

---

## 프로젝트 개요

```
oxi/
├── Cargo.toml          # workspace root (resolver = "2")
├── oxi-ai/             # 통합 LLM API — 멀티 프로바이더 스트리밍 인터페이스
├── oxi-agent/          # 에이전트 런타임 — 툴 호출 루프, 상태 관리
├── oxi-tui/            # 터미널 UI 위젯, 테마 시스템 (ratatui 기반)
└── oxi-cli/            # CLI 애플리케이션 — 메인 진입점
```

| 크레이트       | 라인 수  | 파일 수 | pub struct/enum | 테스트 수 |
|--------------|---------|--------|----------------|----------|
| oxi-ai       | 27,310  | 38     | 88             | 424      |
| oxi-agent    | 13,047  | 35     | 62             | 210      |
| oxi-tui      | 3,257   | 10     | 34             | 60       |
| oxi-cli      | 48,224  | 70     | 338            | 958      |
| **합계**      | **91,838** | **153** | **522**     | **1,652** |

---

## 1. 모듈 분리 (Separation of Concerns)

**점수: 85/100**

### 평가

oxi 프로젝트는 명확한 4-레이어 아키텍처로 분리되어 있다:

| 레이어       | 크레이트     | 책임                                  |
|-------------|-------------|---------------------------------------|
| Provider    | `oxi-ai`    | LLM API 통합, 스트리밍, 메시지 변환, 토큰 추정 |
| Runtime     | `oxi-agent` | 에이전트 루프, 툴 실행, 상태 관리, 재시복구     |
| Presentation| `oxi-tui`   | TUI 위젯, 테마, 이벤트 타입               |
| Application | `oxi-cli`   | CLI 인터페이스, 세션, 설정, 확장 시스템       |

**강점:**

- **oxi-ai**는 순수 프로바이더 추상화에 집중한다. 13개 프로바이더(OpenAI, Anthropic, Google, Azure, Bedrock, Cloudflare, DeepSeek, Mistral, Vertex, Copilot, Codex 등)를 `Provider` 트레이트 하나로 통합한다.
- **oxi-agent**는 `AgentLoop` 기반의 에이전트 실행 로직, 툴 레지스트리, 회복 메커니즘(서킷 브레이커, 폴백 체인)을 캡슐화한다.
- **oxi-tui**는 완전히 독립적인 UI 라이브러리로, 어떤 비즈니스 로직도 포함하지 않는다. ratatui 위젯, 테마 시스템, 이벤트 타입만 정의한다.
- **oxi-cli**는 상위 3개 크레이트를 조합하는 애플리케이션 레이어로, 세션 관리, 설정, 확장 시스템 등 인프라를 담당한다.

**개선점:**

- **oxi-cli가 과도하게 크다** (48K 라인, 70개 파일). `agent_session.rs`, `agent_session_runtime.rs`, `bash_executor.rs`, `event_bus.rs`, `system_prompt.rs` 등은 `oxi-agent` 또는 별도 크레이트로 이동할 수 있다. 특히 `CompactionContext` 타입이 `lib.rs`에 정의된 것은 아키텍처 경계 위반이다.
- `oxi-agent`의 `agent.rs`와 `agent_loop/mod.rs`가 기능적으로 중복된다. `Agent` 구조체도 내부적으로 `run_with_channel`을 가지고 있고, `AgentLoop`도 독립적인 `run` 메서드를 제공한다. 두 진입점의 역할이 명확하지 않다.
- `oxi-ai`의 `high_level.rs` (complete 함수 등)이 에이전트 수준의 로직(컴팩션에서 LLM 호출)을 포함하는데, 이는 `oxi-ai`의 책임 경계를 넘는다.

**증거:**
```rust
// oxi-ai/src/lib.rs — 명확한 모듈 분리
mod compaction;     // 컨텍스트 압축
mod context;        // 대화 컨텍스트
mod error;          // 에러 타입
mod messages;       // 메시지 타입 시스템
mod providers;      // 프로바이더 추상화
mod tools;          // 툴 정의/검증
mod transform;      // 크로스 프로바이더 변환
mod types;          // 핵심 도메인 타입
```

---

## 2. 의존성 관리 (Dependency Management)

**점수: 92/100**

### 의존성 그래프 (DAG)

```
                ┌─────────┐
                │ oxi-ai  │   (최하위: 외부 oxi-* 의존 없음)
                └────┬────┘
                     │
          ┌──────────┼──────────┐
          │          │          │
    ┌─────┴─────┐   │   ┌──────┴──────┐
    │ oxi-agent │   │   │  oxi-tui    │   (oxi-tui는 완전 독립)
    └─────┬─────┘   │   └─────────────┘
          │          │
          └──────────┼──────────┐
                     │          │
              ┌──────┴──────────┴──────┐
              │       oxi-cli          │   (최상위: 모든 크레이트 사용)
              └────────────────────────┘
```

**검증 결과:**

| 크레이트       | oxi-* 의존성                          | DAG 준수 |
|-------------|---------------------------------------|---------|
| oxi-ai      | 없음                                   | ✅       |
| oxi-agent   | `oxi-ai`                              | ✅       |
| oxi-tui     | 없음 (완전 독립)                         | ✅       |
| oxi-cli     | `oxi-ai`, `oxi-agent`, `oxi-tui`      | ✅       |

**강점:**

- **순환 의존성 완전히 없음.** DAG 구조가 깔끔하다.
- `oxi-tui`가 UI 라이브러리로 완전히 독립적이다 — 비즈니스 로직에 대한 의존이 전혀 없다.
- `oxi-ai`가 최하위 계층으로, 순수 API 추상화만 제공한다.
- 모든 버전이 `0.5.0`으로 통일되어 있어 워크스페이스 버전 관리가 일관적이다.

**개선점:**

- `oxi-cli`가 `oxi-ai`에 **직접** 의존한다 (`oxi-ai::{get_model, get_provider, CompactionStrategy, estimate_tokens}` 등을 직접 사용). 이 중 일부는 `oxi-agent`를 통해 간접적으로 접근하는 것이 아키텍처적으로 더 깔끔하다.
- `oxi-agent`의 `thiserror` 버전이 `"1"` 인데, `oxi-ai`는 `"2"` 를 사용한다. 워크스페이스 차원의 버전 통일이 필요하다.

**증거:**
```toml
# oxi-agent/Cargo.toml
thiserror = "1"   # 버전 1

# oxi-ai/Cargo.toml  
thiserror = "2"   # 버전 2
```

---

## 3. API 설계 (API Design)

**점수: 82/100**

### 트레이트 설계

프로젝트는 12개의 공개 트레이트를 정의한다:

| 트레이트                | 크레이트     | 목적                              |
|-----------------------|-------------|-----------------------------------|
| `Provider`            | oxi-ai      | LLM 프로바이더 스트리밍 인터페이스    |
| `ProviderAuth`        | oxi-ai      | 프로바이더 인증 추상화              |
| `Compactor`           | oxi-ai      | 컨텍스트 압축 전략                  |
| `AgentTool`           | oxi-agent   | 에이전트 툴 실행 인터페이스          |
| `ToolDefinitionLike`  | oxi-agent   | 툴 정의 추상화                     |
| `Extension`           | oxi-cli     | 확장 시스템 핵심 트레이트            |
| `SessionCwdSource`    | oxi-cli     | 세션 작업 디렉토리 제공             |
| `FooterDataProvider`  | oxi-cli     | TUI 푸터 데이터 제공               |
| `Summarizer`          | oxi-cli     | 대화 요약 인터페이스               |
| `FsWatchHandler`      | oxi-cli     | 파일시스템 감시 핸들러              |
| `AuthStorageBackend`  | oxi-cli     | 인증 스토리지 백엔드               |
| `FallbackResolver`    | oxi-cli     | 인증 폴백 해결                     |

**강점:**

- **`Provider` 트레이트가 훌륭하다:**
  ```rust
  #[async_trait]
  pub trait Provider: Send + Sync + 'static {
      async fn stream(
          &self,
          model: &Model,
          context: &Context,
          options: Option<StreamOptions>,
      ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;
      fn name(&self) -> &str;
  }
  ```
  최소한의 인터페이스로 13개 프로바이더를 통합한다. `Option<StreamOptions>`으로 선택적 설정을 지원한다.

- **`AgentTool` 트레이트가 확장성이 뛰어나다:**
  ```rust
  #[async_trait]
  pub trait AgentTool: Send + Sync {
      fn name(&self) -> &str;
      fn label(&self) -> &str;
      fn description(&self) -> &str;
      fn parameters_schema(&self) -> Value;
      async fn execute(&self, tool_call_id: &str, params: Value, 
                       signal: Option<oneshot::Receiver<()>>) -> Result<AgentToolResult, ToolError>;
      fn on_progress(&self, _callback: ProgressCallback) { /* default no-op */ }
  }
  ```
  `signal` 파라미터로 취소 지원, `on_progress` 콜백으로 스트리밍 진행 상태, 기본 구현 제공으로 선택적 오버라이드.

- **`#[non_exhaustive]` 적극 활용:**
  `Api`, `ThinkingLevel`, `StopReason`, `ProviderEvent`, `AgentEvent` 등 열거형에 `#[non_exhaustive]`가 적용되어 하위 호환성을 보장한다.

- **빌더 패턴 일관적 사용:** `AgentConfig`, `CompactionConfig`, `StreamOptions`, `ExtensionManifest` 등이 빌더 패턴을 제공한다.

**개선점:**

- **`Provider` 트레이트의 `stream()`이 `Pin<Box<dyn Stream>>`을 반환**한다. 이는 힙 할당이 필수이며 `async_stream` 등의 대안이 고려되지 않았다. `impl Stream` 반환 타입으로 변경하면 호출측 성능이 개선될 수 있다.
- **`Extension` 트레이트가 너무 방대하다** (30개 메서드). 대부분 기본 구현이 있지만, 관심사 분리가 필요하다. `ExtensionLifecycle`, `ExtensionHooks`, `ExtensionTools` 등으로 분할하면 확장 작성자의 인지 부하가 줄어든다.
- **`prelude` 모듈이 모든 크레이트에 있으나** 실제 사용 패턴에서는 `oxi_cli::prelude`가 존재하지 않는다. 일관성 부족.
- `AgentEvent`가 25개 이상의 variant를 가진 거대한 enum이다. 이는 `#[non_exhaustive]`로 보호되지만, 직렬화/역직렬화 오버헤드와 패턴 매칭 복잡도가 높다.

---

## 4. 비동기 아키텍처 (Async Architecture)

**점수: 80/100**

### 비동기 패턴 사용 현황

| 메커니즘           | 사용 횟수  |
|-------------------|----------|
| `async_trait`     | 36       |
| `tokio::spawn`    | 17       |
| `mpsc::channel`   | 17       |
| `RwLock`          | 178      |
| `Arc`             | 175      |

**강점:**

- **스트리밍 퍼스트 설계:** `Provider::stream()`이 `futures::Stream<Item = ProviderEvent>`을 반환하여 진정한 스트리밍을 구현한다. 토큰 단위의 `TextDelta`, `ThinkingDelta`, `ToolCallDelta` 이벤트를 제공한다.

- **이벤트 파이프라인이 잘 설계됨:**
  ```
  ProviderEvent → AgentLoop → mpsc::channel → AgentEvent → TUI/Consumer
  ```
  `mpsc::channel(100)` 버퍼 크기로 백프레셔를 관리한다.

- **도구 실행 취소 지원:** `AgentTool::execute()`가 `oneshot::Receiver<()>` 시그널을 받아 실행 중인 도구를 취소할 수 있다.

- **병렬/순차 도구 실행 모드:**
  ```rust
  pub enum ToolExecutionMode {
      Parallel,   // 모든 툴콜 동시 실행
      Sequential, // 순차 실행
  }
  ```

- **서킷 브레이커가 lock-free 원자 연산으로 구현:**
  ```rust
  pub struct CircuitBreaker {
      state: AtomicU8,
      consecutive_failures: AtomicU64,
      consecutive_successes: AtomicU64,
      opened_at: parking_lot::Mutex<Option<Instant>>,
  }
  ```

**개선점:**

- **`parking_lot::RwLock`을 178번이나 사용한다.** `parking_lot::RwLock`은 async-aware가 아니다. `.read()`/`.write()` 호출이 긴 작업(slow task)을 블로킹하면 tokio 런타임이 기아(starvation) 상태가 될 수 있다. `tokio::sync::RwLock`을 고려해야 하는 부분이 많다.

- **`LocalSet` 사용이 불안정하다:**
  ```rust
  // oxi-cli/src/lib.rs
  let local = tokio::task::LocalSet::new();
  local.spawn_local(async move {
      let _ = agent.run_with_channel(prompt, tx).await;
  });
  ```
  주석에 "agent's internal RwLockReadGuard is not Send-safe"라고 명시되어 있다. 이는 아키텍처 근본 문제이다. `parking_lot::RwLock`의 가드가 `Send`가 아니기 때문이다.

- **백프레셔 관리가 미흡하다:** `mpsc::channel(100)`의 버퍼 크기가 하드코딩되어 있다. 컨텍스트 크기나 시스템 리소스에 따라 동적 조정이 필요하다. `stream.next().await` 루프에서 명시적인 백프레셔 체크가 없다.

- **재시도 로직이 두 곳에 중복 구현**되어 있다:
  - `Agent::stream_with_retry()` (agent.rs)
  - `AgentLoop`의 `retry::stream_with_retry()` (agent_loop/retry.rs)

---

## 5. 확장성 (Extensibility)

**점수: 88/100**

### 확장 포인트

**1. 프로바이더 확장 (`oxi-ai`)**

```rust
// Provider 트레이트 구현만으로 새 프로바이더 추가 가능
#[async_trait]
impl Provider for MyProvider {
    async fn stream(&self, model: &Model, context: &Context, 
                    options: Option<StreamOptions>) 
        -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        // ...
    }
    fn name(&self) -> &str { "my-provider" }
}
```

**2. 도구 확장 (`oxi-agent`)**

```rust
// AgentTool 트레이트로 커스텀 도구 등록
let registry = ToolRegistry::with_builtins();
registry.register(MyCustomTool::new());
// 또는 선택적 등록
let registry = ToolRegistry::with_selected_tools(cwd, &["read", "write", "bash"]);
```

내장 도구만 9개: `Read`, `Write`, `Edit`, `Bash`, `Grep`, `Find`, `Ls`, `WebSearch`, `Subagent`.

**3. 플러그인 시스템 (`oxi-cli`)**

```rust
// Extension 트레이트로 동적 확장 가능
pub trait Extension: Send + Sync {
    fn register_tools(&self) -> Vec<Arc<dyn AgentTool>> { vec![] }
    fn register_commands(&self) -> Vec<Command> { vec![] }
    fn on_load(&self, _ctx: &ExtensionContext) {}
    // ... 30개 라이프사이클 훅
}
```

- **동적 로딩:** `libloading`을 통한 `.so`/`.dylib` 런타임 로딩 지원
- **확장 매니페스트:** 권한 시스템(`FileRead`, `FileWrite`, `Bash`, `Network`)으로 보안 경계 제공
- **그레이스풀 디그레이데이션:** 확장 패닉 시 격리, 에러 기록, 나머지 확장은 계속 동작
- **핫 리로드:** `notify` 기반 파일시스템 감시로 확장 자동 갱신

**4. 크로스 프로바이더 메시지 변환**

```rust
// oxi-ai/src/transform.rs
pub fn transform_for_provider(
    messages: &[Message],
    from_api: &Api,
    to_api: &Api,
) -> Vec<Message>
```

모델 전환 시 자동으로 thinking 블록을 `<thinking>` 태그로 변환하는 등 크로스 호환성을 보장한다.

**5. 컴팩션 확장**

```rust
pub trait Compactor: Send + Sync {
    async fn compact(&self, messages: &[Message], instruction: Option<&str>) 
        -> Result<CompactedContext, CompactionError>;
}
```

4가지 컴팩션 전략: `Disabled`, `Threshold(f32)`, `EveryNTurns(usize)`, `AbsoluteTokens(usize)`.

**강점:**

- 확장 시스템이 매우 포괄적이다. 30개의 라이프사이클 훅이 거의 모든 관심사(sessions, tools, context, provider requests, input, compaction, errors, model selection)를 커버한다.
- `Arc<dyn AgentTool>` 레지스트리 패턴으로 동적 툴 등록이 깔끔하다.
- `#[non_exhaustive]` 열거형으로 API 진화가 보장된다.

**개선점:**

- `Extension` 트레이트가 "god trait" 안티패턴이다. 30개의 기본 구현 메서드를 가진 거대한 트레이트는 단일 책임 원칙(SRP)을 위반한다.
- `get_provider()`가 match 기반 팩토리 함수로, 커스텀 프로바이더를 런타임에 등록할 수 없다. `ProviderRegistry` 패턴이 필요하다.
- `ToolRegistry`에 툴 해제(unregister) 메서드가 없다.

---

## 6. 설정 관리 (Configuration)

**점수: 87/100**

### 레이어드 설정 아키텍처

```
┌─────────────────────────────────────────────┐
│ 5. CLI 인자 (clap derive)                     │ ← 최우선
├─────────────────────────────────────────────┤
│ 4. 환경변수 (OXI_MODEL, OXI_THEME, ...)      │
├─────────────────────────────────────────────┤
│ 3. 프로젝트 설정 (.oxi/settings.toml|json)    │
├─────────────────────────────────────────────┤
│ 2. 글로벌 설정 (~/.oxi/settings.toml|json)    │
├─────────────────────────────────────────────┤
│ 1. 빌트인 기본값 (Settings::default())        │ ← 최하위
└─────────────────────────────────────────────┘
```

**강점:**

- **완전한 5-레이어 설정 스택**을 구현한다. 각 레이어가 명확하게 문서화되어 있다.
- **이중 포맷 지원:** TOML과 JSON 모두 지원하며, 자동 감지 및 우선순위 처리가 있다.
- **마이그레이션 시스템:** `version` 필드로 설정 포맷 버전 관리. v0→v2, v1→v2 마이그레이션 지원.
- **12개 환경변수 지원:**

  | 환경변수                    | 설정 필드                 |
  |---------------------------|-------------------------|
  | `OXI_MODEL`               | `default_model`         |
  | `OXI_PROVIDER`            | `default_provider`      |
  | `OXI_THINKING`            | `thinking_level`        |
  | `OXI_THEME`               | `theme`                 |
  | `OXI_MAX_TOKENS`          | `max_tokens`            |
  | `OXI_TEMPERATURE`         | `default_temperature`   |
  | `OXI_SESSION_DIR`         | `session_dir`           |
  | `OXI_STREAM`              | `stream_responses`      |
  | `OXI_EXTENSIONS_ENABLED`  | `extensions_enabled`    |
  | `OXI_AUTO_COMPACTION`     | `auto_compaction`       |
  | `OXI_TOOL_TIMEOUT`        | `tool_timeout_seconds`  |

- **원자적 쓰기:** 임시 파일 → rename 패턴으로 설정 파일 손상 방지.
- **프로젝트 설정 탐지:** 현재 디렉토리에서 상위로 `.oxi/settings.*` 파일을 탐색(walk up).

**개선점:**

- **설정 검증(validation)이 미흡하다.** `temperature`의 범위 검사(0.0–2.0), `max_tokens`의 최소값, `model_id` 형식 검증 등이 없다. `Settings::validate(&self) -> Result<Vec<ValidationWarning>>` 메서드가 필요하다.
- **레이어 오버레이가 `serde_json::Value` 병합으로 구현**되어 있다. 타입 안전한 오버레이가 아니다:
  ```rust
  fn layer_file(base: &Settings, path: &Path) -> Result<Settings> {
      // JSON value로 병합 → 역직렬화
  }
  ```
- **환경변수 적용이 수동 파싱이다.** `apply_env()`에 12개의 `if let Ok(v) = env::var(...)` 블록이 있다. `envy`나 `figment` 같은 크레이트 활용을 고려할 수 있다.
- **`.env` 파일 지원이 없다.** 프로젝트별 `.env.oxi` 파일 로딩이 유용할 수 있다.

---

## 7. 상태 관리 (State Management)

**점수: 78/100**

### 상태 아키텍처

```
┌───────────────────────────────────────────────────┐
│ SharedState                                        │
│ ├── state: parking_lot::RwLock<AgentState>         │
│ │   ├── messages: Vec<Message>                     │
│ │   ├── iteration: usize                           │
│ │   ├── stop_reason: Option<StopReason>            │
│ │   ├── tool_results: Vec<ToolResult>              │
│ │   ├── total_tokens: usize                        │
│ │   ├── input_tokens: usize                        │
│ │   └── output_tokens: usize                       │
│                                                     │
│ AgentInner (RwLock-protected)                       │
│ ├── config: AgentConfig                            │
│ └── provider: Arc<dyn Provider>                    │
└───────────────────────────────────────────────────┘
```

**세션 관리 (`oxi-cli`):**

- JSONL 기반 append-only 트리 구조
- `SessionEntry`가 `id`/`parent_id`로 분기 트리를 형성
- `SessionMeta`로 분기 메타데이터 관리
- 버전 관리: `CURRENT_SESSION_VERSION = 3`

**강점:**

- **`SharedState` 패턴이 깔끔하다:**
  ```rust
  pub struct SharedState {
      state: RwLock<AgentState>,
  }
  impl SharedState {
      pub fn get_state(&self) -> AgentState { self.state.read().clone() }
      pub fn update<F>(&self, f: F) { f(&mut self.state.write()); }
  }
  ```
  스냅샷 기반 읽기 + 쓰기 락 업데이트로 일관성을 보장한다.

- **세션 트리 구조가 우수하다:** 대화 분기(fork), 되감기, 트리 탐색이 가능한 append-only JSONL 포맷은 프로덕션급 설계이다.

- **서킷 브레이커로 상태 복구:** lock-free 원자 연산으로 상태 전환(Closed → Open → HalfOpen)을 관리한다.

- **컴팩션으로 상태 크기 관리:** 4가지 전략으로 대화 히스토리 크기를 제어한다.

**개선점:**

- **`parking_lot::RwLock` 사용이 위험하다.** 비동기 컨텍스트에서 `.write()`를 잡은 채 `.await`를 호출하면 데드락이 발생할 수 있다. 현재 코드에서:
  ```rust
  // agent.rs의 switch_model()
  {
      let inner = self.config();   // read lock 획득
      let old_api = ...;           // lock 아래에서 읽기
  }                                // read lock 해제
  let mut inner = self.inner_mut(); // write lock 획득
  ```
  이 패턴은 TOCTOU(time-of-check-to-time-of-use) 경쟁 조건이 있다.

- **`oxi-cli`의 `InteractiveSession`이 `oxi-agent`의 `AgentState`와 동기화되지 않는다.** `App::run_prompt_with_events`에서 `InteractiveSession`과 `AgentState`를 수동으로 동기화해야 한다.

- **Agent와 AgentLoop의 상태 관리가 이원화되어 있다.** `Agent`는 `SharedState`를 가지고 있고, `AgentLoop`도 자체 `SharedState`를 가진다. 두 객체 간 상태 동기화 메커니즘이 없다.

- **세션 저장이 즉각적이지 않다.** 크래시 시 진행 중인 대화가 손실될 수 있다. 주기적 체크포인트나 WAL(write-ahead log) 패턴이 필요하다.

---

## 종합 평가

### 항목별 점수

| # | 항목                  | 점수  | 가중치 | 가중 점수 |
|---|-----------------------|------|--------|----------|
| 1 | 모듈 분리              | 85   | 15%    | 12.75    |
| 2 | 의존성 관리            | 92   | 15%    | 13.80    |
| 3 | API 설계               | 82   | 15%    | 12.30    |
| 4 | 비동기 아키텍처         | 80   | 15%    | 12.00    |
| 5 | 확장성                 | 88   | 15%    | 13.20    |
| 6 | 설정 관리              | 87   | 10%    | 8.70     |
| 7 | 상태 관리              | 78   | 15%    | 11.70    |
|   | **종합**              |      | **100%** | **84.45** |

### 🏗️ 종합 아키텍처 점수: **84/100**

### 요약

**강점 (做得好的 부분):**

1. **깔끔한 4-레이어 크레이트 분리** — 의존성 DAG가 순환 없이 깔끔하다
2. **프로바이더 추상화가 우수** — `Provider` 트레이트 하나로 13개 프로바이더를 통합
3. **확장 시스템이 포괄적** — 30개 라이프사이클 훅, 권한 시스템, 동적 로딩, 그레이스풀 디그레이데이션
4. **설정 관리가 프로덕션급** — 5-레이어 설정 스택, 마이그레이션, 원자적 쓰기
5. **세션 트리 구조** — append-only JSONL 기반 분기/탐색이 가능한 세션 관리
6. **높은 테스트 커버리지** — 1,652개 테스트, 전체 코드 대비 우수한 비율
7. **`#[non_exhaustive]` 적극 활용** — API 진화에 대한 하위 호환성 보장

**개선 필요 사항:**

1. **oxi-cli 과도한 비대** — 48K 라인, 70개 파일. 하위 크레이트로 분리 필요
2. **Agent/AgentLoop 이원화** — 두 에이전트 진입점의 책임이 중복/불명확
3. **비동기 안전성** — `parking_lot::RwLock`의 비동기 컨텍스트 사용, `LocalSet` 우회 등
4. **설정 검증 부재** — 값 범위, 형식 검증이 없음
5. **상태 동기화** — `InteractiveSession`과 `AgentState` 간 수동 동기화
6. **백프레셔 관리** — 하드코딩된 채널 버퍼 크기, 명시적 흐름 제어 없음
7. **Extension 트레이트 과대** — "god trait" 안티패턴, 분리 필요

### 개선 제안 (우선순위 순)

| 우선순위 | 제안                                            | 영향                  |
|---------|-------------------------------------------------|-----------------------|
| P0      | `oxi-cli`에서 `agent_session` 관련 코드를 `oxi-agent`로 이동 | 모듈 분리 개선        |
| P0      | `Agent`와 `AgentLoop`를 단일 진입점으로 통합        | 복잡도 감소           |
| P1      | `parking_lot::RwLock` → `tokio::sync::RwLock` 마이그레이션 | 비동기 안전성 확보    |
| P1      | `Settings::validate()` 구현                       | 설정 오류 사전 방지   |
| P2      | `Extension` 트레이트를 `ExtensionLifecycle` + `ExtensionHooks`로 분리 | 확장 작성 편의성      |
| P2      | `ProviderRegistry` 패턴 도입 (런타임 프로바이더 등록) | 확장성 향상           |
| P3      | 세션 주기적 체크포인트 / WAL 패턴                  | 크래시 복구           |
| P3      | `thiserror` 버전 통일 (v2)                         | 의존성 일관성         |
