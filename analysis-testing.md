# oxi 프로젝트 테스트(Test Coverage & Quality) 분석

**분석 일자:** 2026-05-06  
**워크스페이스:** oxi-ai, oxi-agent, oxi-tui, oxi-cli  
**총 테스트 수:** 1,821개 (#[test] 1,646 + #[tokio::test] 175)

---

## 1. 테스트 커버리지 — **78/100**

### 1.1 전체 현황

| 크레이트 | src 파일 수 | 테스트 있는 파일 | 커버리지 | #[test] | #[tokio::test] | 총 테스트 |
|----------|-----------|---------------|---------|---------|----------------|----------|
| oxi-ai   | 38        | 30            | 78.9%   | 449     | 8              | 457      |
| oxi-agent| 35        | 17            | 48.6%   | 190     | 149            | 339      |
| oxi-tui  | 10        | 6             | 60.0%   | 60      | 0              | 60       |
| oxi-cli  | 70        | 53            | 75.7%   | 947     | 18             | 965      |
| **합계** | **153**   | **107**       | **69.9%** | **1,646** | **175**    | **1,821** |

### 1.2 소스 LOC 대비 테스트 비율

| 크레이트 | 소스 LOC | 테스트 수 | 비율(LOC/테스트) |
|----------|---------|----------|----------------|
| oxi-ai   | 27,310  | 457      | 1:60           |
| oxi-agent| 13,047  | 339      | 1:38 (우수)    |
| oxi-tui  | 3,257   | 60       | 1:54           |
| oxi-cli  | 48,224  | 965      | 1:50           |
| **합계** | **91,838** | **1,821** | **1:50**      |

### 1.3 모듈별 분포

**oxi-ai (457 테스트) — 포괄적 커버리지:**
- `providers/` 하위 14개 provider 각각에 테스트 내장 (anthropic 22개, openai 17개, bedrock 15개, vertex 6개 등)
- `utils/overflow.rs` — 23개 테스트, 모든 provider별 오버플로우 패턴 커버
- `utils/sanitize_unicode.rs` — 10개, `utils/json_parse.rs` — 19개
- `model_db.rs` — 10개, `compaction.rs` — 27개, `messages.rs` — 21개

**oxi-agent (339 테스트) — 도구 중심, 에이전트 루프 미흡:**
- `tools/` 하위 14개 파일에 테스트 (edit 10, bash 20, subagent 14, write 18 등)
- **인라인 통합 테스트**: `tests/tools.rs` (60테스트, 1144라인), `tests/agent_loop_full.rs` (20 테스트)
- **미흡 영역**: `agent_loop/mod.rs` (494라인), `agent.rs` (710라인) — 테스트 없음

**oxi-tui (60 테스트) — 제한적이나 적절:**
- `fuzzy.rs` — 20개 (가장 많음)
- `widgets/command_palette.rs` — 18개
- `event.rs` (248라인), `cell.rs` (123라인) — 테스트 없음 (TUI 렌더링 위주)

**oxi-cli (965 테스트) — 가장 방대:**
- `settings.rs` — 44개, `keybindings.rs` — 50개, `rpc_mode.rs` — 50개
- `session.rs` — 33개, `export.rs` — 46개, `templates.rs` — 46개
- **미흡 영역**: `main.rs` (676라인), `lib.rs` (592라인), `tui/` 전체 (1,462라인)

### 1.4 테스트 없는 주요 파일 (50라인 이상)

**oxi-agent** — 가장 취약:
- `agent.rs` (710라인) — 핵심 Agent 구조체, 테스트 없음
- `agent_loop/mod.rs` (494라인) — 메인 에이전트 루프, 테스트 없음
- `agent_loop/tool_exec.rs` (356라인) — 도구 실행 로직
- `events.rs` (317라인), `tools/grep.rs` (413라인), `state.rs` (153라인)

**oxi-cli** — TUI/확장 미흡:
- `extensions/registry.rs` (657라인), `extensions/types.rs` (506라인)
- `resource_loader_compat.rs` (537라인), `main.rs` (676라인)
- `tui/` 전체 (1,462라인) — 4개 파일 모두 테스트 없음

**oxi-ai** — 상대적으로 양호:
- `providers/deepseek.rs` (384라인), `context.rs` (172라인), `providers/event.rs` (149라인)

---

## 2. 테스트 품질 — **75/100**

### 2.1 어서션 밀도

| 지표 | 수치 |
|------|------|
| 총 assert! 호출 | 2,087 |
| 총 assert_eq! 호출 | 1,810 |
| 총 assert_ne! 호출 | 12 |
| **총 어서션** | **3,909** |
| **테스트당 평균 어서션** | **2.1** |

어서션 밀도 2.1은 **양호한 수준**이다. 단순 true/false 확인보다 assert_eq!로 구체적 값을 검증하는 패턴이 널리 사용됨.

```rust
// oxi-cli/src/rpc_mode.rs — 좋은 예: 다중 어서션으로 출력 검증
fn test_serialize_json_line() {
    let value = serde_json::json!({"type": "test", "data": 42});
    let line = serialize_json_line(&value);
    assert!(line.ends_with('\n'));
    assert!(!line.contains("\r\n"));
    let parsed: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["type"], "test");
    assert_eq!(parsed["data"], 42);
}
```

### 2.2 Edge Case 커버리지

**우수한 사례:**

`utils/overflow.rs` — 23개 테스트가 18개 provider별 오버플로우 패턴을 개별 검증:
- Anthropic `prompt is too long`, `request_too_large`
- OpenAI `exceeds the context window`
- Google, xAI, Groq, Mistral, Bedrock 등 고유 패턴
- **부정 케이스**: `test_non_overflow_rate_limit()`, `test_service_unavailable_not_overflow()`
- **무조건 오버플로우**: `test_silent_overflow()`, `test_length_stop_overflow()`

```rust
// oxi-ai/src/utils/overflow.rs — 다양한 edge case
fn test_non_overflow_rate_limit() { ... }  // rate limit은 overflow가 아님
fn test_silent_overflow() { ... }          // 에러 없이 조용히 overflow
fn test_length_stop_overflow() { ... }     // output=0으로 overflow 감지
fn test_no_error_no_overflow() { ... }     // 정상 케이스
```

`anthropic.rs` — SSE 파싱 edge case 포괄:
```rust
fn parse_malformed_json_is_skipped()     // 잘못된 JSON 무시
fn parse_empty_data_line_skipped()       // 빈 데이터 라인
fn parse_carriage_return_line_endings()  // CR 라인 엔딩 처리
fn parse_unknown_event_type_ignored()    // 알 수 없는 이벤트 무시
```

### 2.3 테스트 격리

**격리 품질: 높음**

- `tempfile::TempDir` 사용: oxi-cli 10개 파일, oxi-agent 3개 파일, oxi-ai 1개 파일
- `create_temp_dir()` 헬퍼 (oxi-agent/tests/tools.rs) — AtomicU64로 고유 디렉토리 보장
- `setup_temp_store()` (oxi-ai/src/oauth.rs) — 임시 OAuth 저장소

```rust
// oxi-agent/tests/tools.rs — 훌륭한 격리 패턴
async fn create_temp_dir(name: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = format!("/tmp/oxi_tool_test_{}_{}", name, id);
    // ...
}
```

### 2.4 `#[should_panic]` 부재

- **`#[should_panic]` 테스트: 0개** — panic 경로 테스트 누락
- `Result` 반환 패턴을 선호하여 panic 테스트 필요성이 낮으나, 입력 검증이 중요한 파서/유틸에는 필요할 수 있음

### 2.5 `#[ignore]` 테스트

- **14개 `#[ignore]` 테스트** — 모두 `// broken test` 주석
  - `frontmatter.rs` (3), `clipboard_image.rs` (3), `image_convert.rs` (6), `auto_compaction.rs` (1), `error_recovery.rs` (1)
- ⚠️ 깨진 테스트를 수정하지 않고 방치한 점은 품질 우려 사항

---

## 3. 통합 테스트 — **72/100**

### 3.1 통합 테스트 파일

| 파일 | 라인 | 테스트 수 | 내용 |
|------|------|----------|------|
| oxi-ai/tests/provider_mock.rs | 384 | 8 | mockito 기반 HTTP 모킹, SSE 스트리밍 |
| oxi-ai/tests/error_handling.rs | 255 | 25 | 에러 타입, Display, From, 체인 |
| oxi-agent/tests/tools.rs | 1,144 | 60 | 모든 도구 통합 테스트 |
| oxi-agent/tests/agent_loop_full.rs | 1,277 | 20 | AgentLoop 전체 플로우 |
| oxi-agent/tests/streaming.rs | 153 | 4 | 진행률 스트리밍 |
| oxi-agent/tests/retry_tests.rs | 526 | ~45 | 서킷 브레이커, 백오프 |
| oxi-cli/tests/session_persistence.rs | 38 | 1 | 세션 저장/로드 |
| oxi-cli/tests/cli_parsing.rs | 63 | 6 | CLI 인수 파싱 (assert_cmd) |

### 3.2 크레이트 간 통합

**우수:** `agent_loop_full.rs`는 `oxi-ai`의 Provider 트레이트와 `oxi-agent`의 AgentLoop를 결합하여 테스트:
```rust
// MockProvider가 oxi-ai::Provider를 구현 → AgentLoop에서 사용
struct MockProvider { responses: Vec<MockResponse>, ... }
impl Provider for MockProvider { async fn stream(...) ... }

// 테스트에서 두 크레이트 결합
let agent_loop = AgentLoop::new(provider, config, tools, state);
let result = agent_loop.run("Hi there".to_string(), callback).await;
```

**우수:** `provider_mock.rs`는 mockito로 실제 HTTP 서버를 모킹하여 OpenAI provider의 SSE 스트리밍을 엔드투엔드 테스트:
```rust
let mut server = Server::new_async().await;
let mock = server.mock("POST", "/chat/completions")
    .with_status(200)
    .with_body(r#"data: {"choices":[...]}..."#)
    .create_async().await;
// 실제 OpenAiProvider로 스트리밍 → 검증
```

### 3.3 E2E 테스트

**제한적:** `cli_parsing.rs`에서 `assert_cmd`로 CLI 바이너리를 실행하는 6개 E2E 테스트:
```rust
Command::new("cargo").args(&["run", "--", "--version"]).assert().success();
Command::new("cargo").args(&["run", "--", "config", "show"]).assert().success();
```

⚠️ 이 테스트들은 `cargo run`에 의존하여 빌드 시간이 길고 CI에서 비용이 높음.

---

## 4. 비동기 테스트 — **80/100**

### 4.1 분포

| 크레이트 | #[tokio::test] | 비율 |
|----------|----------------|------|
| oxi-ai   | 8              | 1.7% |
| oxi-agent| 149            | 43.9% |
| oxi-tui  | 0              | 0%   |
| oxi-cli  | 18             | 1.9% |
| **합계** | **175**        | **9.6%** |

### 4.2 비동기 테스트 패턴

**oxi-agent가 비동기 테스트의 핵심 담당** — 도구 실행, AgentLoop, 스트리밍이 모두 async:
```rust
#[tokio::test]
async fn test_multi_turn_tool_loop() {
    let provider = Arc::new(MultiTurnToolProvider::new(vec![...]));
    let agent_loop = AgentLoop::new(provider, config, tools, state);
    let result = agent_loop.run("Please echo".to_string(), callback).await;
    assert!(result.is_ok());
    // 다중 턴 검증, 이벤트 순서 검증
}
```

**MockStream 패턴** — Stream 트레이트를 직접 구현하여 제어 가능한 비동기 스트림 생성:
```rust
struct MockStream { text: String, done: bool }
impl Stream for MockStream {
    type Item = ProviderEvent;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        if self.done { return Poll::Ready(None); }
        self.done = true;
        Poll::Ready(Some(ProviderEvent::Done { ... }))
    }
}
```

### 4.3 모킹 패턴

| 패턴 | 사용처 | 품질 |
|------|--------|------|
| `MockProvider` (커스텀) | agent_loop_full.rs | ⭐⭐⭐ Provider 트레이트 구현 |
| `MultiTurnToolProvider` | agent_loop_full.rs | ⭐⭐⭐ 다중 턴 도구 호출 시뮬레이션 |
| `Mockito` HTTP 모킹 | provider_mock.rs | ⭐⭐⭐ 실제 HTTP 요청/응답 |
| `EchoTool` / `CountingTool` | agent_loop_full.rs | ⭐⭐ AgentTool 트레이트 구현 |
| `MockDefLike` | tool_definition_wrapper.rs | ⭐ 단순 구조체 모킹 |

### 4.4 비동기 테스트 격리

- `tokio::test`의 기본 런타임 격리 활용
- `Arc<Mutex<Vec<_>>>` 패턴으로 이벤트 수집 (thread-safe)
- `AtomicUsize`로 호출 카운트 추적
- ⚠️ `#[serial]` 테스트 속성 미사용 — 경쟁 상태 가능성 (현재까지는 문제 없어 보임)

---

## 5. 벤치마크 — **70/100**

### 5.1 존재 현황

| 크레이트 | 벤치마크 | 파일 |
|----------|---------|------|
| oxi-ai   | ✅ 2개 | `benches/token_estimation.rs`, `benches/sse_parsing.rs` |
| oxi-agent| ❌ 없음 | — |
| oxi-tui  | ❌ 없음 | — |
| oxi-cli  | ❌ 없음 | — |

### 5.2 벤치마크 품질

**token_estimation.rs** — ⭐⭐⭐ 우수:
- Criterion 프레임워크 사용 (HTML 리포트 생성)
- 3개 벤치마크 그룹: `estimate`, `estimate_words`, `context_usage`
- 다양한 입력 타입: prose, code, CJK 혼합, JSON
- 다양한 크기: 1K, 10K, 100K 바이트
- `Throughput::Bytes`로 처리량 측정
- `black_box` 사용으로 최적화 방지

**sse_parsing.rs** — ⭐⭐⭐ 우수:
- OpenAI/Anthropic SSE 파싱 벤치마크
- 10~1,000 청크 크기 변화
- 실제 SSE 포맷의 현실적 생성기
- 파서 로직이 벤치마크 파일에 복제되어 있음 (비공개 함수 접근 불가)

### 5.3 미흡 사항

- oxi-agent 도구 (edit, write, grep 등) 성능 벤치마크 없음
- oxi-cli 세션 직렬화/역직렬화 벤치마크 없음
- oxi-tui 렌더링 벤치마크 없음 (TUI 프레임 레이트)

---

## 6. 테스트 인프라 — **65/100**

### 6.1 dev-dependencies

| 크레이트 | dev-dependencies |
|----------|-----------------|
| oxi-ai   | tokio-test, mockito, tempfile, criterion |
| oxi-agent| tokio-test, async-stream, tempfile |
| oxi-tui  | **없음** |
| oxi-cli  | tempfile, assert_cmd, predicates |

### 6.2 테스트 유틸리티

**존재하는 헬퍼:**
```rust
// oxi-agent/tests/tools.rs
create_temp_dir(name) → String        // 고유 임시 디렉토리
cleanup(path)                          // 정리
execute_tool(tool, params) → Result    // 도구 실행 추상화

// oxi-cli/src/packages.rs
setup_temp_packages_dir() → (TempDir, PathBuf)
create_test_package(base, name, version) → PathBuf

// oxi-agent/tests/agent_loop_full.rs
make_config() → AgentLoopConfig       // 기본 설정
make_tools() → Arc<ToolRegistry>       // 기본 도구 등록
```

### 6.3 Fixture 파일

`oxi-ai/tests/fixtures/` — 4개 SSE fixture:
- `openai_stream.txt` — OpenAI Chat Completions SSE 스트림
- `openai_tool_call.txt` — 도구 호출 SSE 스트림
- `anthropic_stream.txt` — Anthropic Messages SSE 스트림
- `anthropic_thinking.txt` — thinking 블록 SSE 스트림

⚠️ fixture는 oxi-ai에만 존재. 다른 크레이트는 fixture 없음.

### 6.4 공유 테스트 유틸 없음

- 크레이트 간 공유 테스트 유틸리티 모듈 없음
- 각 파일에 로컬 헬퍼 함수 중복 정의
- `test_utils`, `test_helper`, 공유 `mock` 모듈 없음
- 테스트용 프로시저 매크로 없음

### 6.5 테스트 전용 `cfg(test)` 모듈

- 113개 파일에 `#[cfg(test)] mod tests {}` 패턴 사용 — Rust 관례 준수

---

## 7. 테스트 가독성 — **82/100**

### 7.1 네이밍 컨벤션

**일관된 `test_` 접두어 사용:**
```rust
test_parse_frontmatter_basic()
test_parse_frontmatter_with_blank_lines()
test_parse_frontmatter_no_frontmatter()
test_read_file_basic()
test_read_file_multiline()
test_read_file_not_found()
test_read_path_traversal_blocked()
```

**구체적이고 의미 있는 이름:**
```rust
test_circuit_breaker_opens_after_threshold()     // 명확한 동작 설명
test_circuit_breaker_default_threshold_is_five() // 기본값 검증
test_non_overflow_rate_limit()                   // 부정 케이스 표시
test_silent_overflow()                           // 특수 케이스
test_event_sequence_tool_loop()                  // 시퀀스 검증
test_all_tools_executed_before_continue()        // 보장 조건
```

### 7.2 구조화

**Section 주석으로 논리적 그룹화:**
```rust
// ═══════════════════════════════════════════════════════════════════
// Test 1: Single turn (no tools)
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// Test 3: Parallel tool execution
// ═══════════════════════════════════════════════════════════════════
```

**도구별 섹션 분리 (tools.rs):**
```rust
// ═══════════════════════════════════════════════════════════════════
// ReadTool Tests
// WriteTool Tests
// EditTool Tests
// GrepTool Tests
// BashTool Tests
// FindTool Tests
// ═══════════════════════════════════════════════════════════════════
```

### 7.3 파일 수준 문서화

**훌륭한 모듈 문서:**
```rust
//! AgentLoop full integration tests
//!
//! These tests verify the complete AgentLoop flow including:
//! - Single turn conversations (no tools)
//! - Multi-turn tool use loops
//! - Parallel tool execution
//! - Steering message injection
//! - Max iterations stopping
```

```rust
//! Tests for oxi-agent retry logic: circuit breaker, exponential backoff,
//! and retryable error classification.
```

### 7.4 테스트 함수 문서화

- ⚠️ 개별 테스트 함수에 `///` doc comment 없음 (0개 확인)
- 테스트 이름이 충분히 설명적이어서 큰 문제는 아니지만, 복잡한 테스트에는 문서화가 도움될 수 있음

### 7.5 Assertion 메시지

대부분의 assert에 커스텀 메시지가 없으나, 일부 좋은 예시:
```rust
assert!(agent_start_idx < agent_end_idx);
assert!(start < end, "TurnStart should come before TurnEnd");
assert!(msg.contains("503"), "should preserve status code: {msg}");
```

---

## 종합 평가

### 항목별 점수

| # | 항목 | 점수 | 가중치 | 가중 점수 |
|---|------|------|--------|----------|
| 1 | 테스트 커버리지 | 78 | 25% | 19.5 |
| 2 | 테스트 품질 | 75 | 20% | 15.0 |
| 3 | 통합 테스트 | 72 | 15% | 10.8 |
| 4 | 비동기 테스트 | 80 | 10% | 8.0 |
| 5 | 벤치마크 | 70 | 5% | 3.5 |
| 6 | 테스트 인프라 | 65 | 10% | 6.5 |
| 7 | 테스트 가독성 | 82 | 15% | 12.3 |
|   | **종합** | **—** | **100%** | **75.6** |

### 🏆 종합 테스트 품질 점수: **76/100**

---

## 강점 요약

1. **방대한 테스트 수량** — 1,821개 테스트는 Rust 프로젝트로서 매우 우수한 수준
2. **우수한 어서션 밀도** — 테스트당 평균 2.1개 어서션으로 의미 있는 검증
3. **포괄적 edge case 커버** — overflow 패턴(23테스트), SSE 파싱(20+테스트), 에러 처리(25테스트)
4. **모킹 품질** — Provider 트레이트 기반 MockProvider, mockito HTTP 모킹
5. **가독성** — 일관된 네이밍, 섹션 주석, 모듈 문서
6. **벤치마크** — Criterion 기반, 다양한 입력 타입/크기

## 개선 권장사항

1. **깨진 테스트 즉시 수정** — 14개 `#[ignore]` 테스트를 수정하거나 제거
2. **oxi-agent 핵심 로직 테스트 추가** — `agent.rs`(710라인), `agent_loop/mod.rs`(494라인)
3. **oxi-cli TUI 테스트 추가** — 1,462라인의 테스트 없는 TUI 코드
4. **`#[should_panic]` 테스트 추가** — 입력 검증이 중요한 파서/유틸
5. **공유 테스트 유틸리티 모듈** — 크레이트 간 중복 헬퍼 제거
6. **oxi-tui/oxi-cli 벤치마크** — 렌더링, 세션 직렬화 성능 추적
7. **oxi-tui dev-dependencies 추가** — 현재 테스트 인프라 전무
8. **CI에서 `#[ignore]` 테스트 추적** — 깨진 테스트의 누적 방지
