# oxi v0.6.0 — 9점 달성 설계 문서

## 목표

| 크레이트 | 현재 (v0.5.0) | 목표 (v0.6.0) |
|----------|:-------------:|:------------:|
| oxi-ai   | 8.0           | ≥ 9.0        |
| oxi-agent| 7.4           | ≥ 9.0        |
| oxi-tui  | 8.5           | ≥ 9.0        |
| oxi-cli  | 7.6           | ≥ 9.0        |
| **전체** | **7.9**       | **≥ 9.0**    |

---

## Phase 1: 에러 핸들링 하드닝 (7.3 → 9.0)

> 전체 점수에 +0.4 영향. 가장 큰 ROI.

### 1.1 oxi-ai `unwrap()` 제거 (49개)

#### 🔴 Dangerous (2개) — 반드시 수정

| 파일:라인 | 현재 코드 | 수정 방안 |
|-----------|----------|----------|
| `providers/cloudflare.rs:54` | `self.account_id.as_ref().unwrap()` | `if let Some(ref aid) = self.account_id { ... } else { return Err(...) }` |
| `oauth.rs:270` | `Url::parse(&config.endpoint).expect(...)` | `Url::parse(&config.endpoint).map_err(|e| Error::Provider(ProviderError::InvalidConfig(...)))?` |

#### ⚠️ Risky (19개) — 수정 권장

**A. API 키 HeaderValue 파싱 (5개)**
```
providers/copilot.rs:129  api_key.parse().unwrap()
providers/azure.rs:108    api_key.parse().unwrap()
providers/codex.rs:289    api_key.parse().unwrap()
providers/anthropic.rs:98 api_key.parse().unwrap()
providers/bedrock.rs:348  token.parse().unwrap()
```

**공통 수정 방안** — 유틸 함수 생성:
```rust
// oxi-ai/src/providers/mod.rs
fn parse_header_value(value: &str, context: &str) -> Result<HeaderValue, ProviderError> {
    value.parse().map_err(|_| ProviderError::ConfigError(
        format!("Invalid {} value: contains non-visible ASCII characters", context)
    ))
}
```

**B. RwLock 포이즈닝 (9개)**
```
provider_registry.rs: 330, 336, 344, 349, 362, 382, 397, 456, 477
```

**공통 수정 방안** — 포이즈닝 복구 헬퍼:
```rust
// oxi-ai/src/provider_registry.rs
impl ProviderRegistry {
    fn read(&self) -> RwLockReadGuard<'_, HashMap<String, Arc<dyn Provider>>> {
        self.providers.read().unwrap_or_else(|e| e.into_inner())
    }
    fn write(&self) -> RwLockWriteGuard<'_, HashMap<String, Arc<dyn Provider>>> {
        self.providers.write().unwrap_or_else(|e| e.into_inner())
    }
}
```

**C. model_registry 문자열 split (10개)**
```
model_registry.rs: 64, 147, 213, 265, 339, 407, 441, 475, 562, 600
```

**공통 수정 방안** — 안전한 유틸:
```rust
// oxi-ai/src/model_registry.rs
fn extract_model_name(id: &str) -> &str {
    id.rsplit_once('/').map(|(_, name)| name).unwrap_or(id)
}
```

**D. Bedrock 헤더 (4개)**
```
providers/bedrock.rs: 120 (host), 149 (auth), 343, 348 (token)
```
→ `parse_header_value()` 유틸 사용

#### ✅ Infallible (35개) — 변경 불필요

정적 문자열 파싱, `serde_json::to_string(Value)` (항상 성공), `write!(Vec<u8>, ...)` (항상 성공).
대신 `const` 또는 `HeaderValue::from_static()` 로 마이그레이션:

```rust
// Before
"application/json".parse().unwrap()

// After
const HEADER_JSON: HeaderValue = HeaderValue::from_static("application/json");
```

---

### 1.2 oxi-cli `unwrap()` 제거 (20개 risky)

#### ⚠️ Risky — 수정 권장

| 파일 | 위치 | 현재 코드 | 수정 방안 |
|------|------|----------|----------|
| `lib.rs` | :302 | `load_from_dir("/nonexistent").unwrap()` | `SkillManager::new()` 로 대체 |
| `session.rs` | :743, :746, :1622 | `current_dir().unwrap()`, `parent().unwrap()` | `.unwrap_or_else(\|\| ".".into())` |
| `session.rs` | :1357-1358 | `entries_map.get(id).unwrap()` | `.ok_or_else(\|\| anyhow!("..."))?` |
| `session.rs` | :1648 | `file_stem().unwrap()` | `.unwrap_or_default()` |
| `export.rs` | :557, :564, :581, :614 | `lines.next().unwrap()` | `if let Some(l) = lines.next()` + 에러 처리 |
| `packages.rs` | :1275 | `target_dir.parent().unwrap()` | `let Some(parent) = ... else { bail!(...) }` |
| `main.rs` | :523 | `meta.parent_id.unwrap()[..8]` | `.map(\|id\| &id[..8.min(id.len())])` |
| `main.rs` | :558 | `info.parent_session_id.unwrap()` | `if let Some(pid) = info.parent_session_id` |
| `session_navigation.rs` | :381, :591, :665 | `old_leaf_id.unwrap()` | `if let Some(id) = ...` 패턴으로 변경 |
| `branch_summarization.rs` | :592 | `options.custom_instructions.as_ref().unwrap()` | `if let Some(ref instr) = ...` |

#### ✅ Infallible (73개) — 변경 불필요
RwLock (단일 스레드), 정적 Regex, serde_json 직렬화.

---

### 1.3 oxi-agent `AgentError` thiserror 마이그레이션

```rust
// Before (manual Display impl)
impl std::fmt::Display for AgentError { ... }

// After (thiserror derive)
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Tool execution failed: {0}")]
    Tool(String),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Agent configuration error: {0}")]
    Config(String),

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
```

---

## Phase 2: 아키텍처 정리 (8.3 → 8.6)

### 2.1 agent_loop.rs 분해 (1,660줄 → 7개 파일)

```
oxi-agent/src/agent_loop/
├── mod.rs           (~300줄) AgentLoop struct, new(), run(), run_messages(), continue_loop(), run_loop()
├── config.rs        (~80줄)  AgentLoopConfig, ToolExecutionMode, 상수
├── tool_exec.rs     (~280줄) execute_tool_calls_*, prepare_tool_call, 훅 디스패치
├── streaming.rs     (~120줄) stream_assistant_response()
├── retry.rs         (~200줄) stream_with_retry, is_retryable_error, handle_retryable_error, 서킷 브레이커
├── queues.rs        (~40줄)  steer, follow_up, drain_*, clear_*
└── helpers.rs       (~30줄)  resolve_model, should_stop_after_turn, extract_tool_calls
```

### 2.2 Agent/AgentLoop 중복 제거

**공유 유틸 모듈 생성:**
```
oxi-agent/src/shared/
├── mod.rs
├── model_utils.rs   resolve_model_from_id() — split('/') 통일 (==2 → >=2 버그 수정)
├── compaction.rs    create_compaction_manager() 공장 함수
├── context.rs       build_context() — 메시지 + 시스템 프롬프트 + 툴 → Context
└── retry.rs         RetryConfig, stream_with_retry_core() — Agent와 AgentLoop이 공유
```

**중복 제거 항목:**

| 중복 | 발생 횟수 | 공유 함수 |
|------|:--------:|----------|
| 모델 ID 파싱 (`split('/')`) | 7회 | `resolve_model_from_id()` |
| 컴팩션 초기화 | 2회 | `create_compaction_manager()` |
| Context 빌딩 | 2회 | `build_context()` |
| 재시도 로직 | 2회 | `stream_with_retry_core()` |
| 상수 (MAX_RETRIES, BACKOFF) | 2회 | `retry.rs`에 단일 정의 |

**Agent vs AgentLoop 전략:**
- `Agent`를 `AgentLoop`의 thin wrapper로 리팩터링
- `Agent::run_with_channel()`은 내부적으로 `AgentLoop::run()` 호출
- `Agent`의 fallback 모델 로직을 `AgentLoop`로 이식

### 2.3 extensions.rs 분해 (4,202줄 → 14개 파일)

```
oxi-cli/src/extensions/
├── mod.rs            (~50줄)   리익스포트
├── permission.rs     (~30줄)   ExtensionPermission
├── manifest.rs       (~75줄)   ExtensionManifest
├── error.rs          (~80줄)   ExtensionError
├── events.rs         (~200줄)  이벤트 타입 15개
├── emit_result.rs    (~100줄)  EmitResult 타입들
├── context.rs        (~230줄)  ExtensionContext + Builder
├── commands.rs       (~30줄)   Command
├── trait_def.rs      (~215줄)  Extension trait
├── registry.rs       (~740줄)  ExtensionRegistry
├── runner.rs         (~820줄)  ExtensionRunner
├── loading.rs        (~80줄)   로딩 유틸
├── state.rs          (~40줄)   ExtensionState
└── tests/            (~1,190줄) 테스트
```

---

## Phase 3: 문서화 (7.3 → 8.5)

### 3.1 `#![warn(missing_docs)]` 추가

각 크레이트의 `lib.rs` / `main.rs` 상단에:
```rust
#![warn(missing_docs)]
```

이후 빌드 시 undocumented 공개 아이템마다 경고가 발생하므로 자연스럽게 문서화가 진행됨.

### 3.2 우선 문서화 대상 (779개 미문서 항목 중)

#### Tier 1: 핵심 공개 API (최우선, ~200개)

| 크레이트 | 항목 | 현재 커버리지 | 목표 |
|----------|------|:----------:|:----:|
| oxi-ai | `Context`, `Message`, `ContentBlock`, `ToolCall`, `ToolResultMessage`, `ProviderEvent` | 부분 | 100% |
| oxi-agent | `AgentLoop` 공개 메서드 전체, `AgentTool` trait, `AgentEvent` | 55% | 95% |
| oxi-tui | `Component` trait, `Container`, `Surface` | 66% | 90% |
| oxi-cli | `SessionManager`, `PackageManager`, `CliArgs`, `AgentSession` | 75% | 90% |

#### Tier 2: `/// # Examples` 추가 (~50개)

| 크레이트 | 대상 메서드 |
|----------|-----------|
| oxi-ai | `Provider::stream()`, `Context::new()`, `Tool::new()`, `complete()`, `estimate_tokens()`, `transform_messages()` |
| oxi-agent | `AgentLoop::new()`, `AgentLoop::run()`, `AgentTool::execute()`, `ToolRegistry::with_builtins()` |
| oxi-tui | `Component::handle_event()`, `Component::render()`, `TUI::new()`, `Surface::write_string()` |
| oxi-cli | `CliArgs::parse()`, `SessionManager::new()`, `Settings::load()` |

예시:
```rust
/// Creates a new conversation context with the given system prompt and messages.
///
/// # Examples
///
/// ```
/// use oxi_ai::{Context, Message};
///
/// let ctx = Context::new(
///     Some("You are a helpful assistant.".into()),
///     vec![Message::user("Hello!")],
///     vec![],
/// );
/// ```
pub fn new(...) -> Self { ... }
```

#### Tier 3: 아키텍처 가이드 문서

| 문서 | 위치 | 내용 |
|------|------|------|
| oxi-ai ARCHITECTURE.md | `oxi-ai/ARCHITECTURE.md` | Provider trait 설계, 메시지 변환 흐름, 컴팩션 전략 |
| oxi-agent ARCHITECTURE.md | `oxi-agent/ARCHITECTURE.md` | AgentLoop 이벤트 흐름, 툴 실행 파이프라인, 재시도/복구 |
| oxi-tui GUIDE.md | `oxi-tui/GUIDE.md` | 컴포넌트 구현 가이드, 렌더링 파이프라인 설명 |
| oxi-cli ARCHITECTURE.md | `oxi-cli/ARCHITECTURE.md` | 세션 시스템, 확장 시스템, 설정 레이어 |

---

## Phase 4: 테스트 보강 (7.9 → 9.0+)

### 4.1 oxi-cli 통합 테스트 (현재 0개 → ~40개)

```
oxi-cli/tests/
├── cli_commands.rs     (~300줄) 서브커맨드 E2E 테스트
│   ├── test_sessions_list
│   ├── test_sessions_tree
│   ├── test_session_fork
│   ├── test_session_delete
│   ├── test_pkg_list
│   ├── test_config_show
│   ├── test_config_set_get
│   ├── test_single_prompt_mode
│   └── test_version_flag
├── session_persistence.rs (~200줄) 세션 JSONL 영속성
│   ├── test_create_and_load_session
│   ├── test_session_branching
│   ├── test_session_migration_v1_to_v3
│   └── test_corrupted_session_recovery
└── settings_layering.rs  (~200줄) 설정 레이어 병합
    ├── test_default_values
    ├── test_global_config_override
    ├── test_project_config_merge
    ├── test_env_var_override
    └── test_cli_args_override
```

사용 크레이트: `assert_cmd`, `predicates`, `tempfile`

### 4.2 oxi-ai Mock HTTP 테스트 (~30개)

```
oxi-ai/tests/
├── provider_mock.rs    (~400줄) mockito 기반 프로바이더 테스트
│   ├── test_openai_streaming_text
│   ├── test_openai_tool_call
│   ├── test_openai_error_response
│   ├── test_anthropic_thinking_blocks
│   ├── test_anthropic_cache_metrics
│   ├── test_google_streaming
│   ├── test_bedrock_sigv4_signing
│   ├── test_rate_limit_429
│   ├── test_server_error_500
│   └── test_malformed_response
```

`mockito`는 이미 `dev-dependencies`에 있음.

### 4.3 oxi-agent 에이전트 수준 통합 테스트 (~20개)

```
oxi-agent/tests/
├── agent_loop_integration.rs  (~300줄)
│   ├── test_single_turn_user_assistant
│   ├── test_multi_turn_tool_loop
│   ├── test_parallel_tool_execution
│   ├── test_sequential_tool_execution
│   ├── test_model_switching_mid_conversation
│   ├── test_compaction_trigger_and_recovery
│   ├── test_circuit_breaker_open_close
│   ├── test_fallback_model_on_failure
│   ├── test_steering_message_injection
│   ├── test_follow_up_queue_processing
│   ├── test_max_iterations_stop
│   └── test_concurrent_agent_runs
```

---

## Phase 5: 추가 개선 (9.0+ 달성 후)

### 5.1 프로바이더 파일 분리 (선택)

```
oxi-ai/src/providers/openai/
├── mod.rs        (~100줄) OpenAiProvider struct + stream()
├── request.rs    (~100줄) 요청 빌딩
├── response.rs   (~200줄) SSE 파싱
└── tests.rs      (~200줄) 단위 테스트
```

### 5.2 `#![deny(clippy::unwrap_used)]` 추가

모든 프로덕션 `unwrap()` 제거 후 추가. CI에서 새로운 `unwrap()` 추가를 자동 차단.

### 5.3 `parking_lot::RwLock` 마이그레이션

`oxi-ai/provider_registry.rs`, `oxi-cli/model_registry.rs`의 `std::sync::RwLock`을
`parking_lot::RwLock`으로 변경 (포이즈닝 불가, 성능 향상).

---

## 실행 계획

```
Week 1: Phase 1 — 에러 핸들링 하드닝
  ├── Day 1-2: oxi-ai unwrap 49개 제거 (병렬 4 에이전트)
  ├── Day 2-3: oxi-cli unwrap 20개 제거 (병렬 4 에이전트)
  ├── Day 3:   oxi-agent AgentError thiserror 마이그레이션
  └── Day 4:   전체 빌드/테스트 검증

Week 2: Phase 2 — 아키텍처 정리
  ├── Day 1-2: agent_loop.rs 분해 + shared/ 모듈 (병렬 4 에이전트)
  ├── Day 2-3: extensions.rs 분해 (병렬 4 에이전트)
  └── Day 4:   Agent → AgentLoop wrapper 리팩터링

Week 3: Phase 3 — 문서화
  ├── Day 1: #![warn(missing_docs)] 추가 + Tier 1 문서화 (병렬 4 에이전트)
  ├── Day 2: /// # Examples 추가 (병렬 4 에이전트)
  └── Day 3: ARCHITECTURE.md 작성 (병렬 4 에이전트)

Week 4: Phase 4 — 테스트 보강
  ├── Day 1-2: oxi-cli 통합 테스트 (병렬 4 에이전트)
  ├── Day 2-3: oxi-ai mock HTTP 테스트 + oxi-agent 통합 테스트 (병렬 8 에이전트)
  └── Day 4:   전체 검증 + 점수 재평가
```

---

## 점수 예측

| 크레이트 | Phase 1 후 | Phase 2 후 | Phase 3 후 | Phase 4 후 |
|----------|:----------:|:----------:|:----------:|:----------:|
| oxi-ai   | 8.5        | 8.5        | 9.0        | **9.2**    |
| oxi-agent| 8.0        | 9.0        | 9.2        | **9.3**    |
| oxi-tui  | 8.7        | 8.7        | 9.2        | **9.3**    |
| oxi-cli  | 8.2        | 8.5        | 9.0        | **9.1**    |
| **전체** | **8.4**    | **8.7**    | **9.1**    | **9.2**    |

---

## 파일 변경 규모 추정

| Phase | 파일 수 | 라인 수 (추정) |
|-------|:------:|:------------:|
| Phase 1 | ~25 | ~500 (수정) |
| Phase 2 | ~40 | ~3,000 (재구성) |
| Phase 3 | ~80 | ~2,000 (문서 추가) |
| Phase 4 | ~8 | ~1,500 (테스트 추가) |
| **총계** | **~100** | **~7,000** |
