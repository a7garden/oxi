# oxi v0.6.0 — 9점 달성 설계 문서 (수정본)

> **리뷰 반영일**: 2026-05-05  
> **원본 대비 변경**: 점수 예측 현실화, 기술적 부정확 수정, 위험 관리 추가, 일정 재조정

## 목표

| 크레이트 | 현재 (v0.5.0) | Phase 4 후 | Phase 5 후 |
|----------|:-------------:|:----------:|:----------:|
| oxi-ai   | 8.0           | 8.9        | **9.0**    |
| oxi-agent| 7.4           | 9.0        | **9.2**    |
| oxi-tui  | 8.5           | 9.0        | **9.0**    |
| oxi-cli  | 7.6           | 8.8        | **9.0**    |
| **전체** | **7.9**       | **8.9**    | **9.1**    |

### 점수 철학 (로그 스케일)

```
7→8: 비교적 쉬움 (버그 수정, 코드 정리)        ← v0.5.0이 이 단계 완료
8→9: 상당히 어려움 (구조적 개선, 포괄 문서화)   ← v0.6.0 목표
9→9.5: 매우 어려움 (세계적 수준, 근본적 한계 돌파)
```

---

## Phase 1: 에러 핸들링 하드닝

> 예상 효과: 전체 7.9 → 8.3

### 1.1 oxi-ai `unwrap()` 제거

#### 🔴 Dangerous (2개)

| 파일:라인 | 현재 코드 | 수정 |
|-----------|----------|------|
| `cloudflare.rs:54` | `self.account_id.as_ref().unwrap()` | `if let Some(ref aid) = self.account_id { ... } else { return Err(ProviderError::InvalidResponse("Cloudflare account_id required".into())) }` |
| `oauth.rs:270` | `Url::parse(...).expect(...)` | `Url::parse(...).map_err(|e| ProviderError::InvalidResponse(format!("Invalid OAuth endpoint: {e}")))?` |

> 참고: `ProviderError::ConfigError`는 존재하지 않음. 기존 `InvalidResponse` 또는 `InvalidApiKey` variant 재활용.

#### ⚠️ Risky — API 키 HeaderValue 파싱 (5개)

`copilot.rs:129`, `azure.rs:108`, `codex.rs:289`, `anthropic.rs:98`, `bedrock.rs:348`

**공통 유틸 함수 (새로운 variant 추가):**

```rust
// oxi-ai/src/error.rs — ProviderError에 새 variant 추가
#[error("Invalid API key header value")]
InvalidApiKeyHeader,

// oxi-ai/src/providers/mod.rs
fn parse_api_key_header(value: &str) -> Result<HeaderValue, ProviderError> {
    value.parse().map_err(|_| ProviderError::InvalidApiKeyHeader)
}
```

> `InvalidApiKey`와 구분: `InvalidApiKey` = "키가 없음", `InvalidApiKeyHeader` = "키에 HTTP 헤더로 사용 불가능한 문자 포함"

#### ⚠️ Risky — RwLock 포이즈닝 (9개)

`provider_registry.rs`: 330, 336, 344, 349, 362, 382, 397, 456, 477

**Phase 1에서 바로 `parking_lot::RwLock`으로 마이그레이션** (Phase 5로 미루면 같은 코드를 두 번 수정):

```rust
// Before
use std::sync::RwLock;

// After  
use parking_lot::RwLock;  // 이미 oxi-agent에서 사용 중

// 포이즈닝 불가. .read() / .write()가 항상 성공.
// .unwrap() 제거 불필요 — parking_lot은 반환 타입이 Guard 자체
```

> `parking_lot`은 이미 workspace 의존성에 있음 (`oxi-agent`가 사용 중).

#### ⚠️ Risky — model_registry 문자열 split (10개)

`model_registry.rs`: 64, 147, 213, 265, 339, 407, 441, 475, 562, 600

```rust
/// Safely extracts model name from a "provider/model" or "provider/org/model" ID.
/// Returns the last segment after '/', or the full ID if no '/' present.
fn extract_model_name(id: &str) -> &str {
    id.rsplit_once('/').map(|(_, name)| name).unwrap_or(id)
}
```

#### ⚠️ Risky — Bedrock 헤더 (4개)

`bedrock.rs`: 120, 149, 343, 348

`parse_api_key_header()`와 동일한 패턴의 헬퍼 사용:
```rust
fn parse_header(value: &str, label: &str) -> Result<HeaderValue, ProviderError> {
    value.parse().map_err(|_| {
        ProviderError::InvalidResponse(format!("Invalid {label} header value"))
    })
}
```

#### ✅ Infallible (35개) — 변경 불필요

정적 문자열 `.parse().unwrap()`은 `HeaderValue::from_static()`으로 바꿀 수 있으나,
**`from_static()`은 `const fn`이 아님** (reqwest 0.12). 따라서 함수 스코프에서 캐싱:

```rust
// Before
"application/json".parse().unwrap()

// After — 매직 상수는 그대로 두되, 의도를 명시하는 주석 추가
// Infallible: "application/json" is a valid HeaderValue
"application/json".parse().unwrap()
```

**실제로는 infallible unwrap에 `expect()`로 의도를 명시하는 것이 가장 실용적:**
```rust
"application/json".parse().expect("valid header value")
```

---

### 1.2 oxi-cli `unwrap()` 제거 (20개 risky)

| 파일 | 위치 | 수정 |
|------|------|------|
| `lib.rs` | :302 | `SkillManager::new()` 로 대체 (빈 매니저 반환) |
| `session.rs` | :743, :746, :1622 | `.unwrap_or_else(\|_| ".".to_string())` |
| `session.rs` | :1357-1358 | `.ok_or_else(\|_| anyhow!("Corrupted session: entry {} not found", id))?` |
| `session.rs` | :1648 | `.unwrap_or_default()` |
| `export.rs` | :557, :564, :581, :614 | `let Some(l) = lines.next() else { break }` + graceful 종료 |
| `packages.rs` | :1275 | `let Some(parent) = target_dir.parent() else { bail!("Invalid install path") }` |
| `main.rs` | :523 | `.map(\|id\| id.get(..8).unwrap_or("????????").to_string())` |
| `main.rs` | :558 | `if let Some(pid) = info.parent_session_id { pid } else { "unknown".into() }` |
| `session_navigation.rs` | :381, :591, :665 | `if let Some(id) = ...` 패턴으로 변경 |
| `branch_summarization.rs` | :592 | `if let Some(ref instr) = options.custom_instructions` |

#### oxi-cli RwLock (73개) — parking_lot 마이그레이션

`model_registry.rs` (31개), `bash_executor.rs` (16개), `rpc_mode.rs` (4개), `theme.rs` (3개)

모두 `std::sync::RwLock` → `parking_lot::RwLock`으로 변경. `.unwrap()` 자동 제거.

---

### 1.3 oxi-agent `AgentError` thiserror 마이그레이션

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Tool execution failed: {0}")]
    Tool(String),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("State error: {0}")]
    State(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Model not found: {0}")]
    Model(String),

    #[error("Max iterations ({0}) reached")]
    MaxIterations(usize),

    #[error("Rate limited: {0}")]
    RateLimited(String),

    #[error("Retries exhausted after {attempts} attempts")]
    RetriesExhausted { attempts: usize },

    #[error("All fallback models failed")]
    FallbackFailed,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// is_retryable()은 수동 구현 유지 (thiserror가 지원 안 함)
impl AgentError {
    pub fn is_retryable(&self) -> bool { ... }
    pub fn user_friendly(&self) -> String { ... }
}
```

---

## Phase 2: 아키텍처 정리

> 예상 효과: 전체 8.3 → 8.6

### 2.1 agent_loop.rs 분해 (1,660줄 → 7개 파일)

**기존 API 호환성 유지:** `pub mod agent_loop` 경로 변경 없음 (file → directory).

```
oxi-agent/src/agent_loop/
├── mod.rs           (~300줄) AgentLoop struct, new(), run(), run_messages(), continue_loop(), run_loop()
├── config.rs        (~80줄)  AgentLoopConfig, ToolExecutionMode, 상수
├── tool_exec.rs     (~280줄) execute_tool_calls_*, prepare_tool_call, 훅 디스패치
├── streaming.rs     (~120줄) stream_assistant_response()
├── retry.rs         (~200줄) stream_with_retry, is_retryable_error, handle_retryable_error
├── queues.rs        (~40줄)  steer, follow_up, drain_*, clear_*
└── helpers.rs       (~30줄)  resolve_model, should_stop_after_turn, extract_tool_calls
```

**선행 작업:** 내부 타입들의 visibility를 `pub(crate)`로 변경:
- `FinalizedToolCall`, `ExecutedToolCallBatch`, `FinalizedToolCallEntry`
- `ExecutedToolCallOutcome`, `PreparedToolCallKind`, `PreparedToolCallOutcome`

### 2.2 Agent/AgentLoop 중복 제거 (안전한 범위만)

**Phase 2에서 수행:**
1. ✅ `resolve_model_from_id()` 공유 유틸 추출 — **`parts.len() >= 2`로 통일** (버그 수정)
2. ✅ `create_compaction_manager()` 공유 함수 추출
3. ✅ `build_context()` 공유 함수 추출
4. ✅ 상수 `MAX_RETRIES`, `BACKOFF_BASE_SECS` 단일 정의

**Phase 2에서 수행하지 않음 (v0.7으로 연기):**
- ❌ Agent → AgentLoop wrapper 리팩토링 (oxi-cli가 Agent에 강결합, 영향 범위 과대)
- ❌ stream_with_retry 통합 (Agent는 mpsc::Sender, AgentLoop는 EmitFn — 시그니처가 근본적으로 다름)

**공유 모듈 위치:** `shared/` 대신 명확한 이름 사용:
```
oxi-agent/src/
├── agent.rs          (그대로)
├── agent_loop/       (분해)
├── model_id.rs       (NEW: resolve_model_from_id)
├── compaction_init.rs (NEW: create_compaction_manager)
├── context_builder.rs (NEW: build_context)
├── retry.rs           (NEW: 상수 + is_retryable_error)
```

### 2.3 extensions.rs 분해 (4,202줄 → 5개 파일)

**리뷰 반영:** 14개 대신 **5개**로 먼저 분해. 강결합된 타입은 하나의 types.rs에 유지:

```
oxi-cli/src/extensions/
├── mod.rs            (~100줄) 리익스포트 + Extension trait 정의
├── types.rs          (~500줄) Permission, Manifest, Error, Events, Commands, EmitResult
├── context.rs        (~230줄) ExtensionContext + Builder
├── registry.rs       (~1,560줄) ExtensionRegistry + ExtensionRunner (아직 강결합)
└── loading.rs        (~80줄)  로딩 유틸 (free functions)
```

테스트는 별도 `tests/extensions.rs`로 이동.

---

## Phase 3: 문서화

> 예상 효과: 전체 8.6 → 8.9

### 3.1 문서화 먼저, lint 나중에

**수정된 순서:** 문서화 완료 → `#![warn(missing_docs)]` 추가

`#![warn(missing_docs)]`를 먼저 추가하면 779개 경고가 발생하여 개발이 불가능해짐.

### 3.2 문서화 작업 (현실적 일정)

#### Tier 1: 핵심 공개 API — 4-5일

하루 ~50개. 총 ~200개.

| 크레이트 | 대상 | 항목 수 |
|----------|------|:------:|
| oxi-ai | `Context`, `Message`, `ContentBlock`, `ToolCall`, `ToolResultMessage`, `ProviderEvent` variant 전체 | ~70 |
| oxi-agent | `AgentLoop` 공개 메서드, `AgentTool` trait, `AgentEvent` variant | ~60 |
| oxi-tui | `Component` trait, `Container`, `Surface`, `Theme` | ~40 |
| oxi-cli | `SessionManager`, `CliArgs`, `AgentSession` | ~30 |

#### Tier 2: `/// # Examples` — 3-4일

하루 ~15개. 총 ~40개.

**doctest 컴파일 검증 전략:**
- 복잡한 API는 ```` ```ignore ```` (검증 안 함, 예시만 제공)
- 단순 생성자/BUILDER만 ```` ``` ```` (검증됨)

```rust
// 검증 가능한 예시 (간단한 생성자)
/// Creates a new tool definition.
///
/// # Examples
/// ```
/// use oxi_ai::Tool;
/// let tool = Tool::new("my_tool", "A tool");
/// ```
pub fn new(name: &str, description: &str) -> Self { ... }

// 검증 불가 예시 (복잡한 API)  
/// Streams a response from the provider.
///
/// # Examples
/// ```ignore
/// let stream = provider.stream(context, options).await?;
/// while let Some(event) = stream.next().await {
///     match event {
///         ProviderEvent::TextDelta { delta, .. } => print!("{delta}"),
///         _ => {}
///     }
/// }
/// ```
async fn stream(...) -> ...;
```

#### Tier 3: 아키텍처 가이드 — 2일

| 문서 | 내용 |
|------|------|
| `oxi-ai/ARCHITECTURE.md` | Provider trait 설계, 메시지 변환 흐름도, 컴팩션 전략 |
| `oxi-agent/ARCHITECTURE.md` | AgentLoop 이벤트 흐름도, 툴 실행 파이프라인, 재시도/복구 |
| `oxi-tui/GUIDE.md` | 컴포넌트 구현 가이드 (trait 구현 → render → handle_event) |
| `oxi-cli/ARCHITECTURE.md` | 세션 JSONL 구조, 확장 라이프사이클, 설정 레이어 병합 |

### 3.3 `#![warn(missing_docs)]` 적용

**Tier 1 완료 후에** 각 크레이트에 추가:

```rust
// lib.rs 또는 main.rs 상단
#![warn(missing_docs)]
```

이때 이미 90%+ 항목이 문서화되어 있으므로, 남은 경고만 해결하면 됨.

---

## Phase 4: 테스트 보강

> 예상 효과: 전체 8.9 유지 (점수 상승보다 방어적 — 회귀 방지)

### 4.1 oxi-cli 통합 테스트

**선행 작업:** `Cargo.toml`에 dev-dependencies 추가:
```toml
[dev-dependencies]
assert_cmd = "2"
predicates = "3"
```

**테스트 구조 (API 키 불필요한 것만):**

```
oxi-cli/tests/
├── cli_parsing.rs         (~200줄) 인수 파싱 E2E
│   ├── test_version_flag
│   ├── test_help_flag
│   ├── test_model_flag
│   ├── test_provider_flag
│   ├── test_thinking_level
│   ├── test_sessions_subcommand
│   └── test_config_subcommand
├── session_persistence.rs (~200줄) 세션 파일 I/O (단위 테스트 레벨)
│   ├── test_create_and_load_session
│   ├── test_session_branching
│   ├── test_session_migration
│   └── test_corrupted_session_graceful
└── settings_merge.rs      (~150줄) 설정 레이어 병합
    ├── test_default_values
    ├── test_global_override
    ├── test_project_config_merge
    └── test_env_var_override
```

### 4.2 oxi-ai Mock HTTP 테스트 (~25개)

**mockito 사용 가능:** 이미 `dev-dependencies`에 있음.
**base_url 주입 가능:** 모든 프로바이더가 `model.base_url`을 사용하므로 mockito 서버 URL로 교체 가능.

```rust
// 테스트 패턴
#[tokio::test]
async fn test_openai_streaming_text() {
    let mut server = mockito::Server::new_async().await;
    let mock = server.mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body_from_file("tests/fixtures/openai_stream.txt")
        .create_async()
        .await;

    let mut model = get_model("openai", "gpt-4o").unwrap();
    model.base_url = server.url(); // ← base_url 오버라이드

    let provider = OpenAiProvider::new(api_key);
    let stream = provider.stream(model, context, options).await;
    // ... 검증
}
```

**테스트 케이스:**
- OpenAI: 텍스트 스트리밍, 툴 콜, 에러 응답(429, 500)
- Anthropic: thinking block, cache metrics
- Google: 기본 스트리밍
- 에러: 잘못된 JSON, 연결 끊김, 타임아웃

**필요: SSE fixture 파일** (`tests/fixtures/` 디렉토리 생성)

### 4.3 oxi-agent 에이전트 수준 통합 테스트 (~15개)

기존 `MockProvider` + `EchoTool` 패턴 확장:

```
oxi-agent/tests/
├── agent_loop_full.rs  (~300줄)
│   ├── test_single_turn
│   ├── test_multi_turn_tool_loop
│   ├── test_parallel_vs_sequential
│   ├── test_compaction_trigger
│   ├── test_circuit_breaker_lifecycle
│   ├── test_steering_injection
│   ├── test_follow_up_processing
│   ├── test_max_iterations_stop
│   └── test_model_switch_preserves_context
```

### 4.4 oxi-tui ignored 테스트 해결 (4개)

현재 4개의 `#[ignore]` doc-test가 있는데, 이들은 터미널이 필요한 테스트.
해결: `#[cfg(feature = "tui-tests")]` 기능 게이트로 전환하거나,
`CI`에서 `TERM=dumb`으로 실행 가능한지 확인 후 수정.

---

## Phase 5: 마지막 마일리지 (9.0 확실 달성)

> 예상 효과: 전체 8.9 → 9.1

### 5.1 `#![deny(clippy::unwrap_used)]` 추가

Phase 1에서 모든 프로덕션 `unwrap()`을 제거했으므로,
이제 CI에서 새로운 `unwrap()` 추가를 자동 차단:

```rust
// 각 크레이트 lib.rs/main.rs
#![deny(clippy::unwrap_used)]
#![allow(clippy::unwrap_used_in_tests)]  // 테스트는 허용
```

### 5.2 Agent → AgentLoop wrapper 리팩토링 (v0.7에서 연기했던 작업)

```
oxi-agent/src/agent.rs (753줄 → ~150줄 thin wrapper)
  - 내부적으로 AgentLoop::new() 생성
  - run_with_channel()은 AgentLoop::run() + mpsc 어댑터
  - switch_model(), try_fallback()을 AgentLoop으로 이식
```

### 5.3 oxi-cli 커스텀 에러 타입

현재 모든 것이 `anyhow`인데, 핵심 모듈에 typed error 도입:

```rust
// oxi-cli/src/error.rs (NEW)
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found: {0}")]
    NotFound(String),
    #[error("Corrupted session file: {0}")]
    Corrupted(String),
    #[error("Migration failed: v{from} → v{to}")]
    MigrationFailed { from: u32, to: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("Package not found: {0}")]
    NotFound(String),
    #[error("Install failed: {0}")]
    InstallFailed(String),
    #[error("Network error: {0}")]
    Network(String),
}
```

### 5.4 쉘 탭 완성 생성

```rust
// oxi-cli/src/cli.rs에 추가
pub fn generate_completion(shell: clap_complete::Shell) -> String { ... }
```

`clap_complete` 크레이트로 bash/zsh/fish completion 스크립트 생성.

---

## 실행 계획 (현실적)

```
Week 1-2: Phase 1 — 에러 핸들링 하드닝
  ├── Day 1-3:   oxi-ai unwrap 제거 + parking_lot 마이그레이션 (병렬 4 에이전트)
  ├── Day 3-5:   oxi-cli unwrap 제거 + parking_lot 마이그레이션 (병렬 4 에이전트)
  ├── Day 5-6:   oxi-agent AgentError thiserror 마이그레이션
  └── Day 7-8:   전체 빌드/테스트 검증 + 위험 회귀 테스트

Week 3-4: Phase 2 — 아키텍처 정리
  ├── Day 1-4:   agent_loop.rs 분해 + 내부 visibility 조정 (병렬 4 에이전트)
  ├── Day 5-7:   공유 유틸 추출 (model_id.rs, compaction_init.rs, context_builder.rs)
  ├── Day 8-10:  extensions.rs 5파일 분해 (병렬 4 에이전트)
  └── Day 11-12: 전체 빌드/테스트 검증

Week 5-7: Phase 3 — 문서화
  ├── Day 1-5:   Tier 1: 핵심 공개 API 200개 문서화 (병렬 4 에이전트, 하루 50개)
  ├── Day 6-9:   Tier 2: # Examples 40개 + #![warn(missing_docs)] (병렬 4 에이전트)
  ├── Day 10-11: #![warn(missing_docs)] 추가 + 남은 경고 해결
  └── Day 12-13: ARCHITECTURE.md 4개 작성

Week 8-9: Phase 4 — 테스트 보강
  ├── Day 1-2:   dev-dependencies 추가 + fixture 파일 준비
  ├── Day 2-4:   oxi-cli 통합 테스트 ~25개 (병렬 4 에이전트)
  ├── Day 4-6:   oxi-ai mock HTTP 테스트 ~25개 (병렬 4 에이전트)
  ├── Day 6-8:   oxi-agent 통합 테스트 ~15개 + oxi-tui ignored 해결 (병렬 4 에이전트)
  └── Day 9-10:  전체 검증 + 점수 재평가

Week 10-11: Phase 5 — 마지막 마일리지
  ├── Day 1:     #![deny(clippy::unwrap_used)] 추가
  ├── Day 2-4:   Agent → AgentLoop wrapper 리팩토링
  ├── Day 5-6:   oxi-cli 커스텀 에러 타입
  ├── Day 7:     쉘 탭 완성
  └── Day 8:     최종 검증 + 점수 확정
```

**총 일정: 11주 (원본 4주에서 현실화)**

---

## 관련 설계 문서

이 설계와 **직교하는** 설계 문서들 (겹침 없음, 실행 순서만 조정 필요):

| 문서 | 관심사 | 관계 |
|------|--------|------|
| `docs/designs/subagent-improvements.md` | Subagent 도구 기능 개선 (프로세스 스폰, abort, usage, ToolRegistry 소유권) | main.rs, agent_session_runtime.rs 공통 변경 → **본 설계 먼저 실행 권장** |
| `docs/oxi-architecture.md` | 전체 아키텍처 개요 | 참고 문서 |
| `docs/oxi-design.md` | 초기 설계 문서 | 참고 문서 |

**실행 순서:** v0.6 (본 설계) → subagent-improvements 순으로 진행하면 파일 충돌을 최소화할 수 있습니다.

---

## 위험 관리

### Breaking Changes (semver v0.x에서 허용)

| Phase | 변경 | 영향 |
|-------|------|------|
| Phase 1 | `ProviderError`에 `InvalidApiKeyHeader` variant 추가 | 패턴 매칭에 `_` 없으면 컴파일 에러. `#[non_exhaustive]` 이미 적용됨 ✅ |
| Phase 1 | `std::sync::RwLock` → `parking_lot::RwLock` | API 동일. 내부 구현 변경. |
| Phase 2 | `agent_loop.rs` → `agent_loop/` 디렉토리 | `pub mod agent_loop` 경로 유지. 비호환 변경 아님. |
| Phase 2 | 공유 유틸 파일 4개 추가 | 순수 추가. 기존 API 변경 없음. |

### 회귀 위험

| 위험 | 완화 방안 |
|------|----------|
| parking_lot 마이그레이션 후 동작 변경 | parking_lot은 표준 RwLock과 동일 API. 테스트로 검증. |
| agent_loop 분해 후 import 누락 | `cargo check --workspace`로 컴파일 검증. |
| extensions.rs 분해 후 순환 의존 | 5개 파일 구성으로 강결합 유지 (14개 분해는 위험). |
| 문서화 중 코드 실수 | `/// ` 주석만 추가, 코드 변경 최소화. |

### CI/CD

Phase 3 이후 CI에 추가:
```yaml
- run: cargo clippy --workspace -- -D warnings
- run: cargo test --workspace
- run: cargo doc --workspace --no-deps  # doc 빌드 검증
```

---

## 파일 변경 규모 추정 (수정)

| Phase | 파일 수 | 라인 수 (추정) | 작업 유형 |
|-------|:------:|:------------:|----------|
| Phase 1 | ~30 | ~600 | 수정 (unwrap → Result, RwLock 교체) |
| Phase 2 | ~45 | ~4,000 | 재구성 (모듈 분해, 파일 이동) |
| Phase 3 | ~80 | ~2,500 | 추가 (doc comments, ARCHITECTURE.md) |
| Phase 4 | ~12 | ~1,800 | 추가 (테스트, fixture) |
| Phase 5 | ~15 | ~1,200 | 수정 (Agent wrapper, 에러 타입, completion) |
| **총계** | **~180** | **~10,100** | |

---

## 점수 예측 (현실적)

| 크레이트 | 현재 | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 |
|----------|:----:|:-------:|:-------:|:-------:|:-------:|:-------:|
| oxi-ai   | 8.0  | 8.5     | 8.5     | 8.8     | 8.9     | **9.0** |
| oxi-agent| 7.4  | 7.8     | 8.8     | 9.0     | 9.0     | **9.2** |
| oxi-tui  | 8.5  | 8.7     | 8.7     | 9.0     | 9.0     | **9.0** |
| oxi-cli  | 7.6  | 8.2     | 8.3     | 8.7     | 8.8     | **9.0** |
| **전체** | **7.9** | **8.3** | **8.6** | **8.9** | **8.9** | **9.1** |

> **Phase 4 완료 시 ~8.9, Phase 5 완료 시 9.1.**
> Phase 4만으로 9.0에 도달하려면 oxi-agent(9.0)와 oxi-tui(9.0)가 끌어올려주므로 근접하지만,
> oxi-cli(8.8)가 발목을 잡음. Phase 5에서 커스텀 에러 + Agent wrapper로 9.0 돌파.
