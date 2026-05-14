# oxi 프로젝트 테스트 품질 분석 보고서

**분석 일시:** 2026-05-14  
**대상 프로젝트:** /Volumes/MERCURY/PROJECTS/oxi (Rust AI 코딩 어시스턴트)  
**크레이트 구성:** `oxi-agent`, `oxi-ai`, `oxi-cli`, `oxi-store`, `oxi-tui`

---

## 1. 테스트 현황 요약

### 1.1 전체 테스트 규모

| 지표 | 수치 |
|------|------|
| **총 테스트 함수 수** | **~1,934개** |
| 통합 테스트 파일 (`tests/`) | 8개 |
| 인라인 단위 테스트 (`#[cfg(test)]` 모듈) | ~90개 소스 파일 |
| 벤치마크 파일 | 2개 |
| 테스트가 없는 소스 파일 | **0개** (모든 `.rs` 파일에 최소 1개 이상의 테스트 또는 `#[cfg(test)]` 존재) |

### 1.2 크레이트별 테스트 분포

| 크레이트 | 통합 테스트 파일 | 인라인 테스트 파일 | 추정 테스트 수 |
|----------|-----------------|-------------------|---------------|
| **oxi-agent** | 4개 (retry_tests, tools, streaming, agent_loop_full) | ~15개 | ~250개 |
| **oxi-ai** | 2개 (error_handling, provider_mock) | ~35개 | ~500개 |
| **oxi-cli** | 2개 (cli_parsing, session_persistence) | ~40개 | ~600개 |
| **oxi-store** | 0개 | 7개 | ~310개 |
| **oxi-tui** | 0개 | 5개 | ~80개 |

### 1.3 벤치마크 현황

- `oxi-ai/benches/sse_parsing.rs` — OpenAI/Anthropic SSE 파싱 성능 벤치마크 (10~1,000청크)
- `oxi-ai/benches/token_estimation.rs` — 토큰 추정(`estimate`, `estimate_words`, `context_usage`) 성능 벤치마크 (영어 산문, 코드, CJK 혼합, JSON)

벤치마크는 criterion 기반으로, Throughput 측정 및 html 리포트 생성이 설정되어 있음.

---

## 2. 테스트 커버리지 분석

### 2.1 모듈별 커버리지 상태

#### ✅ 우수한 커버리지 (테스트 밀도 높음)
- **oxi-ai/providers/** — 모든 프로바이더(Anthropic, OpenAI, Azure, Bedrock, Cloudflare, Copilot, Codex, Google, Mistral, Vertex)에 대해 URL 빌더, 요청 바디 직렬화, 응답 파싱 테스트 존재
- **oxi-ai/transform.rs** — 크로스 프로바이더 메시지 변환 (Anthropic↔OpenAI) 15개 테스트
- **oxi-ai/messages.rs** — 메시지 빌더, 변환, 병합 21개 테스트
- **oxi-ai/compaction.rs** — 컨텍스트 압축 전략 27개 테스트
- **oxi-store/settings.rs** — 설정 직렬화/역직렬화 56개 테스트 (가장 많음)
- **oxi-store/model_resolver.rs** — 모델 해석 로직 39개 테스트
- **oxi-store/session.rs** — 세션 관리 33개 테스트
- **oxi-store/session_navigation.rs** — 세션 내비게이션 30개 테스트
- **oxi-cli/ui/keybindings.rs** — 키바인딩 50개 테스트
- **oxi-cli/rpc_mode/tests.rs** — JSON-RPC 프로토콜 50개 테스트
- **oxi-cli/storage/export.rs** — HTML/JSON 내보내기 46개 테스트
- **oxi-cli/storage/resource_loader.rs** — 리소스 로더 35개 테스트
- **oxi-cli/prompt/templates.rs** — 프롬프트 템플릿 46개 테스트
- **oxi-agent/tests/tools.rs** — 내장 도구 60개 테스트 (Read/Write/Edit/Bash/Grep/Find/Ls + Registry)
- **oxi-agent/tests/retry_tests.rs** — 재시도 로직 45개 테스트 (Circuit Breaker, Backoff, 에러 분류)

#### ⚠️ 보통 커버리지
- **oxi-store/auth_storage.rs** — 인증 저장소 33개 테스트 (직렬화/역직렬화는 충분하나, 실제 파일시스템 I/O 에러 경로 테스트 부족)
- **oxi-ai/provider_registry.rs** — 22개 테스트 (기본 CRUD는 커버, 동시성 시나리오 미비)
- **oxi-cli/media/** — 이미지 처리 관련 파일별 4~15개 테스트 (정상 경로는 커버, 손상된 이미지/비정상 입력에 대한 엣지케이스 부족)

#### ❌ 개선 필요
- **oxi-store** — 통합 테스트 파일이 0개. `oxi-store/tests/` 디렉토리 자체가 없음
- **oxi-tui** — 통합 테스트 파일이 0개. `oxi-tui/tests/` 디렉토리 자체가 없음
- **oxi-cli/tests/cli_parsing.rs** — 6개 테스트만 존재하며, 모두 `cargo run`을 호출하는 느린 E2E 테스트
- **oxi-cli/tests/session_persistence.rs** — 단 1개 테스트만 존재

### 2.2 테스트가 없는 소스 파일

분석 결과, 모든 `.rs` 소스 파일에 최소 1개 이상의 `#[cfg(test)]` 블록 또는 `#[test]` 함수가 포함되어 있어 **커버리지 빈틈(테스트가 전혀 없는 파일)은 사실상 없습니다.** 이는 매우 훌륭한 상태입니다.

다만, 테스트가 "존재"하더라도 테스트의 깊이와 질에는 큰 편차가 있습니다.

---

## 3. 테스트 품질 평가

### 3.1 엣지케이스 커버리지

#### 우수한 사례
- **retry_tests.rs** — 재시도 가능/불가능한 에러 분류에 대한 45개 세밀한 테스트. 케이스 인센서블 테스트, 빈 에러 메시지, 잘못된 `stop_reason` 등 엣지케이스 포함
- **tools.rs** — 경로 순회 공격 차단(`../../etc/passwd`), 빈 파일, 디렉토리 대신 파일 전달, 누락된 파라미터 등 보안/에러 엣지케이스 포괄
- **agent_loop_full.rs** — 빈 프롬프트, 특수문자(`"world" & 'test' < >`), 최대 반복 도달, 병렬 도구 실행 등 다양한 시나리오
- **overflow.rs** — 오버플로우 안전 연산에 22개 테스트 (경계값 포함)
- **sanitize_unicode.rs** — 제어 문자, null 바이트, 유효/무효 UTF-8 케이스

#### 개선 필요 사항
- **동시성 테스트 부족** — `ToolRegistry`, `SharedState`, `SessionManager` 등 `Arc<Mutex>` 기반 공유 상태에 대한 경쟁 상태(race condition) 테스트가 전무
- **대용량 입력 테스트** — 파일 도구(Read/Write/Edit)에 대해 대용량 파일(100MB+), 매우 긴 행(1M+ chars), 바이너리 파일 혼합 등에 대한 테스트 부족
- **네트워크 타임아웃/부분 실패** — SSE 스트림이 중간에 끊기는 경우, 타임아웃 발생 시나리오 등

### 3.2 에러 경로 테스트

| 에러 유형 | 테스트 상태 | 비고 |
|-----------|------------|------|
| 파일 없음 (ENOENT) | ✅ 충분 | ReadTool, EditTool, FindTool, LsTool 모두 커버 |
| 권한 거부 | ⚠️ 일부 | `path_security.rs`에 3개 테스트. 실제 권한 없는 파일에 대한 통합 테스트는 없음 |
| 잘못된 JSON 파라미터 | ✅ 충분 | `json_parse.rs`에 19개 테스트 |
| HTTP 4xx/5xx 에러 | ✅ 충분 | `provider_mock.rs`에서 429, 401, 500 상태 코드 테스트 |
| SSE 파싱 실패 | ⚠️ 일부 | 정상 파싱은 잘 테스트, 형식이 틀린 SSE 데이터에 대한 테스트 부족 |
| 디스크 공간 부족 | ❌ 없음 | WriteTool, SessionManager에서 디스크 Full 시나리오 테스트 없음 |
| API 키 누락/잘못됨 | ✅ 충분 | `ProviderError::MissingApiKey`, `InvalidApiKey` + `env_api_keys.rs`에 14개 테스트 |

### 3.3 해피 패스 vs 엣지케이스 비율

전체적으로 **해피 패스(정상 경로) 테스트가 압도적으로 많으며**, 엣지케이스와 에러 경로 테스트는 특정 모듈(retry, tools, error_handling)에 집중되어 있습니다.

추정 비율: **해피 패스 ~65%, 엣지케이스 ~25%, 에러 경로 ~10%**

---

## 4. Mock 사용 분석

### 4.1 Mock 패턴

| 패턴 | 사용처 | 평가 |
|------|--------|------|
| **수동 MockProvider** | `agent_loop_full.rs`, `tests.rs` | Provider 트레이트를 구현한 커스텀 Mock. 응답 시퀀스, 툴 콜 시뮬레이션 가능. **잘 설계됨** |
| **MultiTurnToolProvider** | `agent_loop_full.rs`, `tests.rs` | 다중 턴 툴 사용 루프 시뮬레이션. 툴 콜 → 결과 → 최종 응답 흐름 재현. **우수** |
| **ApiAwareMockProvider** | `tests.rs` | API 타입 추적이 가능한 Mock. 크로스 프로바이더 전환 테스트에 사용. **좋음** |
| **RetryableProvider** | `tests.rs` | N번 실패 후 성공하는 Mock. 재시도 시나리오 테스트. **좋음** |
| **mockito (HTTP Mock)** | `provider_mock.rs` | 실제 HTTP 서버 Mock. SSE 스트리밍 응답, 에러 상태 코드 테스트. **우수** |
| **임시 파일시스템** | `tools.rs`, `session_persistence.rs` | `/tmp/oxi_tool_test_*` + `tempfile::TempDir` 사용. **양호** |

### 4.2 Mock 개선 포인트

- **MockProvider 중복 정의** — `agent_loop_full.rs`와 `tests.rs`에 동일한 `MockProvider`가 각각 정의되어 있음. 공통 유틸리티로 추출 필요
- **스트리밍 Mock 단순화** — 현재 `MockStream`이 `Stream` 트레이트를 수동 구현. `futures::stream::iter`를 활용하면 더 간단해짐 (일부 테스트는 이미 사용 중)
- **MCP 클라이언트 Mock** — `mcp/client.rs`에 대한 Mock이 없어, MCP 프로토콜 테스트가 불가능한 상태
- **oxi-store Mock** — 파일시스템 의존 모듈에 대한 인메모리 Mock이 없음

---

## 5. 크로스 크레이트 통합 테스트

### 5.1 현재 상태

크로스 크레이트 상호작용 테스트는 주로 `oxi-agent/tests/`와 `oxi-agent/src/tests.rs`에 집중되어 있습니다:

- ✅ `oxi-agent` ↔ `oxi-ai` — MockProvider를 통한 에이전트 루프 테스트, 메시지 변환(`transform_for_provider`) 테스트
- ✅ `oxi-agent` → `oxi-store` — 세션 저장/로드 흐름 (session_persistence.rs)
- ✅ `oxi-ai` ↔ HTTP — mockito 기반 SSE 스트리밍 테스트
- ✅ `oxi-cli` → `oxi-agent` — CLI 파싱 → 에이전트 실행 흐름 (cli_parsing.rs)

### 5.2 부족한 크로스 크레이트 테스트

- ❌ **oxi-cli ↔ oxi-store** — 설정 변경이 스토어에 반영되는 전체 흐름 테스트 없음
- ❌ **oxi-tui ↔ oxi-agent** — TUI 위젯이 에이전트 이벤트를 올바르게 렌더링하는지 테스트 없음
- ❌ **oxi-cli ↔ oxi-ai** — 프롬프트 템플릿 → 실제 API 요청 변환 흐름 테스트 없음
- ❌ **oxi-store ↔ oxi-ai** — 모델 해석 결과가 실제 프로바이더와 매칭되는지 테스트 없음

---

## 6. Property-based 테스트 기회

현재 프로젝트에는 **proptest, quickcheck, arbtest 등 property-based 테스팅 라이브러리가 전혀 사용되지 않고 있습니다.** 다음 영역에 도입을 강력히 권장합니다:

### 6.1 추천 도입 영역

| 모듈 | Property | 이유 |
|------|----------|------|
| **json_parse.rs** | 임의의 JSON 문자열 → 파싱 → 재직렬화가 원본과 동일 | JSON 파서의 완전성 보장 |
| **transform.rs** | 메시지 → 변환 → 역변환 → 원본과 동일 | 크로스 프로바이더 변환의 무손실성 |
| **sanitize_unicode.rs** | 임의 바이트 시퀀스 → 새니타이즈 → 유효 UTF-8 | 모든 입력에 대한 안전성 보장 |
| **overflow.rs** | 임의의 u64/u128 값 → 안전 연산 → 오버플로우 없음 | 수학적 불변성 검증 |
| **messages.rs** | 임의의 메시지 시퀀스 → 직렬화 → 역직렬화 → 동일 | 직렬화 라운드트립 |
| **settings.rs** | 임의의 설정값 → JSON → 역직렬화 → 동일 | 설정 영속성 불변성 |
| **fuzzy.rs** | 임의의 문자열/패턴 → 퍼지 매칭 → 스코어 순서 일관성 | 퍼지 검색 불변성 |

### 6.2 권장 라이브러리

```toml
[dev-dependencies]
proptest = "1"        # 가장 널리 사용됨, 문서 풍부
```

---

## 7. `#[ignore]` 테스트

**프로젝트 전체에 `#[ignore]`된 테스트가 존재하지 않습니다.** 이는 모든 테스트가 항상 실행됨을 의미하며, 긍정적인 상태입니다.

다만, CI 파이프라인에서 특정 테스트만 선택적으로 실행하는 메커니즘도 보이지 않으므로, 느린 테스트(E2E, HTTP Mock)가 개발 피드백 루프를 지연시킬 가능성이 있습니다.

---

## 8. 테스트 실행 시간 및 병렬성

### 8.1 실행 시간 리스크

| 테스트 | 예상 리스크 | 원인 |
|--------|------------|------|
| **cli_parsing.rs** (6개) | 🔴 높음 | `cargo run`을 6번 호출. 빌드 캐시가 없으면 각각 수십 초 소요 |
| **retry_tests.rs** (sleep 포함) | 🟡 보통 | `std::thread::sleep(Duration::from_millis(60))` 사용. 4개 테스트가 각각 60ms 대기 |
| **tools.rs** (파일시스템) | 🟡 보통 | 임시 디렉토리 생성/삭제. 병렬 실행 시 I/O 경합 가능 |
| **streaming.rs** | 🟢 낮음 | 파일 쓰기 + 읽기. 빠름 |
| **provider_mock.rs** (mockito) | 🟡 보통 | 매 테스트마다 mock HTTP 서버 생성/소멸 |

### 8.2 병렬성

- Rust의 기본 테스트 러너는 스레드 풀 기반 병렬 실행
- `tools.rs`의 임시 디렉토리 경로에 `AtomicU64` 카운터를 사용하여 충돌 방지 — **잘 설계됨**
- `cli_parsing.rs`는 `cargo run` 호출로 인해 서로 간섭할 가능성 있음

### 8.3 권장사항

```bash
# 느린 테스트 분리 예시
cargo test --workspace --exclude oxi-cli  # 빠른 단위/통합 테스트
cargo test -p oxi-cli --test cli_parsing  # CI 전용 느린 E2E 테스트
```

---

## 9. 테스트 Flakiness 리스크

### 9.1 시간 의존 테스트

`retry_tests.rs`의 Circuit Breaker 테스트에서 `std::thread::sleep(Duration::from_millis(60))`을 사용합니다.

```rust
// retry_tests.rs:147
std::thread::sleep(Duration::from_millis(60)); // open_duration이 50ms
```

**리스크:** CI 환경에서 CPU 부하가 높으면 60ms 내에 코루틴이 스케줄되지 않을 수 있음.

**권장사항:** 시간 의존 테스트에 `tokio::time::pause()` / `tokio::time::advance()`를 사용하여 가상 시간으로 제어.

### 9.2 파일시스템 의존 테스트

- `tools.rs` — `/tmp`에 직접 파일 생성. **권한 문제** 또는 **디스크 Full** 시 실패 가능
- `session_persistence.rs` — `tempfile::TempDir` 사용 (안전)
- `streaming.rs` — `/tmp/oxi_test_*.txt`에 하드코딩된 경로. **병렬 실행 시 충돌 가능**

**권장사항:** `streaming.rs`도 `tempfile::TempDir` 사용. `tools.rs`의 `/tmp` 직접 사용은 이미 AtomicU64로 네임스페이스를 격리하고 있어 양호함.

### 9.3 네트워크 의존 테스트

- `cli_parsing.rs` — 실제 `cargo run` 실행. **네트워크/빌드 캐시 상태에 따라 결과가 달라질 수 있음**
- `provider_mock.rs` — mockito 사용. **네트워크 의존 없음 (안전)**

---

## 10. 테스트 명명 규칙 및 조직

### 10.1 명명 패턴 분석

| 패턴 | 사용 예 | 빈도 |
|------|---------|------|
| `test_<기능>_<시나리오>` | `test_read_file_basic`, `test_edit_dry_run` | 가장 많음 (~80%) |
| `test_<기능>_<에러상황>` | `test_grep_path_traversal`, `test_bash_exit_code` | 보통 |
| `test_<컴포넌트>_<동작>` | `test_circuit_breaker_opens_after_threshold` | 많음 |
| 한국어/비영어 명명 | 없음 | — |

**평가:** 일관된 `snake_case` 명명 규칙이 잘 유지되고 있으며, 테스트 이름만으로 테스트 의도를 파악할 수 있음.

### 10.2 조직 구조

```
oxi-agent/
  tests/                    # 통합 테스트 (외부 크레이트 관점)
    retry_tests.rs          # 재시도/서킷브레이커
    tools.rs                # 내장 도구
    streaming.rs            # 진행률 스트리밍
    agent_loop_full.rs      # 전체 에이전트 루프
  src/
    tests.rs                # 모듈 간 통합 테스트 (#[cfg(test)] mod)
    tools/*.rs              # 각 도구 모듈 내부에 #[cfg(test)]

oxi-ai/
  tests/                    # 통합 테스트
    error_handling.rs       # 에러 타입
    provider_mock.rs        # HTTP Mock 기반 프로바이더
  src/providers/*.rs        # 각 프로바이더 내부에 #[cfg(test)]
```

### 10.3 개선 포인트

- **공유 테스트 유틸리티 부족** — `tests/common/` 모듈이 없어 MockProvider가 여러 파일에 중복 정의됨
- **테스트 파일 내 그룹화** — `tools.rs`는 `═══` 구분자와 섹션 주석으로 잘 그룹화되어 있으나, `retry_tests.rs`도 유사하지만 일관성이 떨어짐

---

## 11. 주요 개선 권장사항

### 🔴 우선순위 높음 (Critical)

#### 11.1 공유 테스트 유틸리티 생성
```
oxi-agent/tests/common/mod.rs  — MockProvider, MultiTurnToolProvider, 테스트 헬퍼
oxi-ai/tests/common/mod.rs     — HTTP Mock 헬퍼, SSE 생성 유틸
```
현재 `MockProvider`가 `agent_loop_full.rs`와 `tests.rs`에 동일하게 2번 정의되어 있음.

#### 11.2 oxi-store 통합 테스트 추가
`oxi-store/tests/` 디렉토리가 아예 없음. 최소한 다음 테스트가 필요:
- 세션 생성 → 저장 → 로드 → 이어서 대화 → 재저장 (전체 라이프사이클)
- 설정 변경 → 영속화 → 재시작 후 복원
- 동시 세션 접근 (멀티스레드)

#### 11.3 동시성 테스트 추가
`SharedState`, `ToolRegistry`, `SessionManager` 등 `Arc<Mutex>` 기반 구조체에 대해:
- 다중 스레드에서 동시 읽기/쓰기
- 교착 상태(Deadlock) 가능성 검증
- 메시지 손실 없음 보장

### 🟡 우선순위 보통 (Important)

#### 11.4 Property-based 테스트 도입
특히 `json_parse.rs`, `transform.rs`, `sanitize_unicode.rs`에 proptest 도입.

#### 11.5 느린 E2E 테스트 분리
`cli_parsing.rs`를 CI 전용으로 분리하거나, `#[ignore]` + `cargo test -- --ignored` 패턴 도입.

#### 11.6 시간 의존 테스트 개선
`retry_tests.rs`의 `std::thread::sleep`을 `tokio::time::pause()`/`advance()`로 대체.

#### 11.7 SSE 파싱 에러 케이스 추가
- 잘린 SSE 데이터 (마지막 청크가 불완전)
- `data:` 없는 라인
- `data: {malformed json}`
- 중간에 연결이 끊기는 경우

### 🟢 우선순위 낮음 (Nice to have)

#### 11.8 oxi-tui 통합 테스트
렌더링 결과 스냅샷 테스트 (insta 라이브러리 등 활용).

#### 11.9 테스트 커버리지 측정 도구 도입
```bash
cargo tarpaulin --workspace --out Html
# 또는
cargo llvm-cov --workspace --html
```

#### 11.10 대용량 입력 스트레스 테스트
- 100MB+ 파일 ReadTool
- 1M 행 파일 GrepTool
- 수천 개의 동시 툴 콜

#### 11.11 fuzzing 도입
JSON 파서, SSE 파서, UTF-8 새니타이저에 대해 `cargo-fuzz` 적용.

---

## 12. 결론

### 강점
1. **테스트가 없는 파일이 0개** — 업계 최고 수준의 기본 커버리지
2. **~1,934개의 테스트** — 대규모 Rust 프로젝트로서 매우 우수한 볼륨
3. **보안 테스트 우수** — 경로 순회, 인젝션 공격 차단 테스트가 체계적
4. **Mock 설계 우수** — MultiTurnToolProvider 등 복잡한 시나리오를 재현하는 정교한 Mock
5. **Provider 커버리지 완벽** — 12개 프로바이더 모두에 URL 빌더/파서 테스트 존재
6. **벤치마크 운영** — SSE 파싱, 토큰 추정에 대한 성능 회귀 방지 체계 구축

### 약점
1. **oxi-store 통합 테스트 부재** — 가장 중요한 영속성 레이어에 통합 테스트가 없음
2. **Mock 코드 중복** — 공유 테스트 유틸리티가 없어 Mock이 여러 파일에 복제됨
3. **동시성 테스트 부재** — 멀티스레드 환경에서의 정확성이 검증되지 않음
4. **Property-based 테스트 부재** — 파서/변환기의 완전성이 임의 입력에 대해 보장되지 않음
5. **느린 E2E 테스트 관리 부족** — `cargo run` 호출 테스트가 개발 피드백 루프를 저해

### 종합 평가: **B+ (우수, 개선 여지 있음)**

테스트의 "넓이"는 업계 최고 수준이나, "깊이"(동시성, 에러 경로, property-based)에서 개선이 필요합니다. 특히 oxi-store 통합 테스트 추가와 공유 테스트 유틸리티 추출은 즉각적인 투자 가치가 높습니다.
