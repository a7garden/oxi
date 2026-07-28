# omp-정렬 리팩토링 — 진행 상황 (STATUS)

- **날짜**: 2026-07-27 (자율 실행 세션)
- **브랜치**: `omp-realignment-p0`
- **상위 설계**: `docs/superpowers/specs/2026-07-27-omp-realignment-design.md`
- **실행 계획**: `docs/superpowers/plans/2026-07-27-p0-provider-redesign.md`
- **분석 증거**: `docs/superpowers/specs/2026-07-27-omp-realignment-analysis.md`

이 문서는 자율 실행 세션에서 **완료된 작업**과 **남은 작업**을 정확히 기록하여 다음 세션이 중단 없이 이어할 수 있게 합니다.

---

## 1. 완료된 작업 (7 커밋, 전부 green)

모든 커밋은 회귀 게이트를 통과했습니다: `cargo build --workspace`, `cargo clippy --workspace --all-targets -D warnings`, `cargo clippy -p oxi-sdk --features native-browser -D warnings`, `cargo fmt --all -- --check`, `cargo nextest run --workspace`.

| 커밋 | 단계 | 내용 |
|---|---|---|
| `4836a17b` | **P0.1** | `oxi-catalog` 별도 leaf 크레이트 추출. `Api` enum + `catalog/` 7파일 + `product_env` + `data/catalog/` 이관. oxi-ai는 재-내보내기로 62개 consumer 무변경. 의존성 방향 단방향(oxi-ai → oxi-catalog) 복원. |
| `7532a22e` | **P0.3** | **프로바이더 정체성 붕괴 수정 (사용자 핵심 pain)**. `NamedProvider` 래퍼로 `create_builtin_provider("deepseek").name() == "deepseek"` (was "openai"). transport builder / identity-wrapping public wrapper로 분리. 7개 테스트(붕괴를 올바른 것으로 단정하던) 수정. |
| `8410bc73` | **P0.4** | `ImageStart`/`ImageDelta`/`ImageEnd` 스트리밍 이벤트 추가. omp `AssistantMessageEvent` image family 대응 (Gemini 증분 이미지). |
| `f408672c` | **P0.4** | `Api`를 omp `KnownApi` 14개로 확장 + `MistralConversations` 제거(omp는 Mistral을 openai-completions 호환 취급). 7개 신규 dialect 추가(OpenRouter, OpenAiCodexResponses, GoogleGeminiCli, OllamaChat, CursorAgent, GitLabDuoAgent, DevinAgent). `mistral.rs` + 16 테스트 삭제. oxi-sdk `CatalogProtocol` mirror에서도 제거. |
| `2bd2ccdd` | **P0.4** | **per-provider 에러 계층**. 평면 `HttpError(u16, String)` → 구조화 `HttpErrorDetail { status, body, provider, request_id }`. Anthropic이 `request-id` 헤더 캡처(omp `AnthropicApiError` 정렬). `http_status()` 헬퍼 추가. 8 provider + oxi-agent + 테스트 전부 마이그레이션. |
| `1c57c08c` | **P0.4** | **SSE byte-stream framing 중앙화**. `find_valid_utf8_prefix`/`split_complete_lines`를 `providers/sse.rs` 전용 모듈로 이관(omp `readSseEvents()` 정렬). 4개 provider가 `super::openai::` 대신 `super::sse::`로 import. |

### 사용자 pain 해결 상태
- **"프로바이더가 이상하다"** → 정체성 붕괴 **수정됨** (`7532a22e`). deepseek/minimax/togetherai/openrouter/cerebras 모두 올바른 catalog id 반환.
- **catalog/ai boundary** → **복원됨** (`4836a17b`).
- **API dialect 정렬** → **완료** (`f408672c`, 14 KnownApi).

---

## 2. 남은 P0 작업

### P0.4 — AI 품질 층 [완료]

4항목 전부 완료: ImageEnd 이벤트, KnownApi 14 + Mistral 제거, per-provider 에러 계층(`HttpErrorDetail` + Anthropic request-id), SSE byte-stream framing 중앙화(`providers/sse.rs`).

### P0.5 — provider 포팅 [미착수, 가장 큼]

omp의 14 KnownApi 중 7개 dialect에 transport가 없음 (`_ => None` arm). remote-AGENT 프로토콜 3개는 OpenAI-compat endpoint가 아니라 **고유 프로토콜**:

- [ ] **Ollama** (`ollama-chat`): omp `packages/ai/src/providers/ollama.ts`는 **750줄** + 다수 omp 유틸 의존(`parseStreamingJson`, `stream-markup-healing`, `vision-guard`, `empty-completion-retry`, `idle-iterator`, `schema` sanitization). 로컬 서버 `/api/chat` (NDJSON 스트리밍, SSE 아님). production 필수. faithful 포팅은 유틸 포팅까지 수반해 **다-세션 작업**; 최소 동작 포팅은 mock Ollama 서버 테스트 인프라 필요. omp `packages/catalog/src/provider-models/ollama.ts`도 참조.
- [ ] **Cursor** (`cursor-agent`): remote-AGENT 프로토콜. omp `packages/ai/src/providers/cursor.ts`. 고유 stream function + 프로토콜. 높은 노력.
- [ ] **Devin** (`devin-agent`): remote-AGENT. omp `packages/ai/src/providers/devin.ts`. 높은 노력.
- [ ] **GitLab Duo** (`gitlab-duo-agent`): remote-AGENT. omp `packages/ai/src/providers/gitlab-duo.ts`. 높은 노력.
- (우선순위 낮: OpenRouter, OpenAiCodexResponses, GoogleGeminiCli — OpenAI-compat 또는 경량)

각 포팅은 omp 소스(`/tmp/omp`에 클론됨, 또는 github.com/can1357/oh-my-pi)를 Rust로 번역. 수락 기준: 해당 `Api` variant가 transport를 가져, 통합 테스트(mock server) 통과.

### P0.2 — complexity machinery 제거 [미착수, 위험]

omp에 대응 없는 oxi-original ~225KB: `multi_provider.rs`(45KB), `complexity_router.rs`(22KB), `circuit_breaker.rs`(32KB), `fallback_chain.rs`(19KB), `provider_pool.rs`(6KB), `router/`(~100KB), `oxi-sdk/src/multi_provider.rs`(빌더).

**정밀 분석 (2026-07-27)**: 프로덕션에서 **한 번도 생성되지 않음** — `MultiProvider::new`/`ComplexityRouter::new`/`FallbackChain::new`는 doc 주석 + 자체 테스트에만. bootstrap은 표준 `oxi_sdk::register_provider` 사용. 하지만 **`CircuitBreaker`는 agent 루프에 live** (`agent_loop/mod.rs:74` 필드, `streaming.rs:333/435/529` record_failure/success, `retry.rs:41` allow_request). `FallbackChain`은 re-export만(생성 안 됨).

**이중 구조**:
- **opt-in (안전하게 제거 가능)**: `MultiProvider` + `ComplexityRouter` + `provider_pool` + `router/` + oxi-sdk `multi_provider` 빌더 — 기본 경로 아님, `with_multi_provider_routing` opt-in 빌더로만 생성.
- **live (재연결 필요)**: `circuit_breaker` — agent retry가 의존. 제거 시 agent 루프 retry 로직을 direct dispatch로 재연결해야.

- **권장 순서**: (1) opt-in 층(MultiProvider/ComplexityRouter/router/SDK builder) 먼저 제거 — 안전, ~170KB; (2) 별도 세션에서 CircuitBreaker 재연결 + fallback_chain/circuit_breaker 제거; (3) `ProviderEvent::FallbackStart/FallbackExhausted` 변형 제거(이들만 emit).

---

## 3. P1–P4 (별도 세션 필요)

각 단계는 design doc §4의 해당 Phase 참조.

- **P1 — Agent 루프 재정렬**: owned dialect system, intent tracing(`i` 필드), append-only context, approval/tier, soft tool requirements, Harmony leak 감지. 누락 도구 16개 포팅(`ast_grep`, `ast_edit`, `debug`, `eval`, `computer`, `checkpoint`, `rewind`, `hub`, `learn`, `manage_skill`, `inspect_image`, `yield`, `goal`, `review`, `tts`, `vibe`). omp `packages/agent/src/agent-loop.ts`(102KB) 기준.
- **P2 — TUI 재정렬** (가장 큼, 다-월간): `oxi-tui-legacy` → `oxi-tui` rename, 현 v2 폐기. omp 3-전략 차등 렌더링(component memo → native scrollback commit → ED3 replay) + append-only tape 계약 Rust 구현. 전체 입력 시스템(Kitty/bracketed paste/keybinding/mouse/kill ring). LaTeX/mermaid/image. glyph 단일화. omp `packages/tui/src/tui.ts`(173KB) 기준.
- **P3 — 프롬프트 & CLI**: `.md` 기반 시스템 프롬프트(`include_str!()`). personality 시스템. tool-specific prompt `.md`(~45개). 환경 정보 주입. 누락 CLI 명령 포팅. `bootstrap.rs`/`lib.rs` 경계 정리 + F-5(main.rs inline subcommand → `cli/commands/*.rs`).
- **P4 — oxi-original 처리**: issue 시스템 격리, package manager → omp 플러그인 모델 재정렬, language policy 제거/단순화.

---

## 4. 이어하기 가이드

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
git checkout omp-realignment-p0

# 회귀 게이트 (각 변경마다)
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace

# omp 소스 (포팅 참조용)
ls /tmp/omp   # 또는 git clone https://github.com/can1357/oh-my-pi.git
```

### 다음 세션 우선순위 권장
1. **P0.5 Ollama 포팅** (production 필수, 중간 노력, 사용자가 체감할 새 provider)
2. **P0.4 SSE 중앙화** (품질, 명확한 중복)
3. **P0.4 error hierarchy** (품질)
4. 그 후 P0.2 → P3 → P1 → P4 → P2 순

### 핵심 architectural 결정 (이미 확정, 존중할 것)
- **Provider identity ≠ transport**: `NamedProvider` 래퍨가 catalog id 전달. transport(`build_builtin_transport`)는 identity 없음. (P0.3 완료)
- **oxi-catalog은 leaf, 단일 소스**: oxi-ai가 소비만. 역방향 의존 금지. (P0.1 완료)
- **Api = omp KnownApi 14**: Mistral 없음(openai-completions 호환). (P0.4 완료)
- **아직 미구현 (P0.3 후속)**: transport trait에서 `name()` 완전 제거 + `ProviderDefinition` registry를 identity 단일 소스로. 현재는 `NamedProvider` 래퍼가 identity를 올바르게 전달(붕괴는 수정됨)하지만 trait에 여전히 붙어 있음. omp의 완전한 3-way 분리는 `name()` 제거 + base_url/auth 메타데이터를 oxi-catalog `ProviderDescriptor`로 이동으로 완성.

### 사용자 승인된 방침 (재확인 불필요)
- B: omp-정렬 Rust-native (port system·LSP·issue 시스템 유지, 핵심 층 재정렬)
- T1: legacy→omp tape, v2 폐기
- issue 시스템: 유지하되 격리 / package manager: omp 플러그인으로 재정렬 / language policy: 제거
