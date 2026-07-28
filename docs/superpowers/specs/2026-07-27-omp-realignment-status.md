# omp-정렬 리팩토링 — 진행 상황 (STATUS)

- **날짜**: 2026-07-27 (자율 실행 세션)
- **브랜치**: `omp-realignment-p0`
- **상위 설계**: `docs/superpowers/specs/2026-07-27-omp-realignment-design.md`
- **실행 계획**: `docs/superpowers/plans/2026-07-27-p0-provider-redesign.md`
- **분석 증거**: `docs/superpowers/specs/2026-07-27-omp-realignment-analysis.md`

이 문서는 자율 실행 세션에서 **완료된 작업**과 **남은 작업**을 정확히 기록하여 다음 세션이 중단 없이 이어할 수 있게 합니다.

---

## 1. 완료된 작업 (5 커밋, 전부 green)

모든 커밋은 회귀 게이트를 통과했습니다: `cargo build --workspace`, `cargo clippy --workspace --all-targets -D warnings`, `cargo clippy -p oxi-sdk --features native-browser -D warnings`, `cargo fmt --all -- --check`, `cargo nextest run --workspace`.

| 커밋 | 단계 | 내용 |
|---|---|---|
| `4836a17b` | **P0.1** | `oxi-catalog` 별도 leaf 크레이트 추출. `Api` enum + `catalog/` 7파일 + `product_env` + `data/catalog/` 이관. oxi-ai는 재-내보내기로 62개 consumer 무변경. 의존성 방향 단방향(oxi-ai → oxi-catalog) 복원. |
| `7532a22e` | **P0.3** | **프로바이더 정체성 붕괴 수정 (사용자 핵심 pain)**. `NamedProvider` 래퍼로 `create_builtin_provider("deepseek").name() == "deepseek"` (was "openai"). transport builder / identity-wrapping public wrapper로 분리. 7개 테스트(붕괴를 올바른 것으로 단정하던) 수정. |
| `8410bc73` | **P0.4** | `ImageStart`/`ImageDelta`/`ImageEnd` 스트리밍 이벤트 추가. omp `AssistantMessageEvent` image family 대응 (Gemini 증분 이미지). |
| `f408672c` | **P0.4** | `Api`를 omp `KnownApi` 14개로 확장 + `MistralConversations` 제거(omp는 Mistral을 openai-completions 호환 취급). 7개 신규 dialect 추가(OpenRouter, OpenAiCodexResponses, GoogleGeminiCli, OllamaChat, CursorAgent, GitLabDuoAgent, DevinAgent). `mistral.rs` + 16 테스트 삭제. oxi-sdk `CatalogProtocol` mirror에서도 제거. |

### 사용자 pain 해결 상태
- **"프로바이더가 이상하다"** → 정체성 붕괴 **수정됨** (`7532a22e`). deepseek/minimax/togetherai/openrouter/cerebras 모두 올바른 catalog id 반환.
- **catalog/ai boundary** → **복원됨** (`4836a17b`).
- **API dialect 정렬** → **완료** (`f408672c`, 14 KnownApi).

---

## 2. 남은 P0 작업

### P0.4 (마무리) — AI 품질 층 [부분 완료]

**남은 항목 2개** (둘 다 품질 개선, 사용자 pain 아님):

- [ ] **SSE 파싱 중앙화**: scout가 "8중 복제"라고 했으나 실제로는 더 미묘함. `parse_sse_events(text,...)`는 3개 파일(azure/openai_responses/openai)만 있고 이는 **provider별 이벤트 해석**(핈수적으로 개별). 실제 중복은 **byte-stream 프레이밍 층** — 7개 provider가 각자 `.bytes_stream()` + pending-byte buffer + `\n\n` split + partial-UTF-8 처리. omp는 `readSseEvents()`로 중앙화.
  - **작업**: `oxi-ai/src/utils/` (또는 `providers/sse.rs`)에 공유 `SseFrame` 디코더 추가 (byte stream → SSE frames). 7개 provider의 stream loop(`openai.rs:277`, `anthropic.rs:484`, `google.rs:144`, `vertex.rs:183`, `azure.rs:219`, `openai_responses.rs:223`, `bedrock.rs:494`)가 이를 사용하도록 전환.
  - **위험**: 중간 (7개 provider stream loop 수정). 각 provider별로 별도 테스트 후 게이트.
  - **수락 기준**: 7개 provider가 공유 프레이머 사용, 기존 SSE 테스트 전부 통과.

- [ ] **per-provider 에러 계층**: omp `error/classes.ts`는 `AnthropicApiError`(request-id 파싱), `OpenAIHttpError`(body envelope), `BedrockApiError`, `GoogleApiError`, `OllamaApiError`, `DevinApiError`, `CodexProviderStreamError` 보유. oxi는 평면 `ProviderError::HttpError(u16, String)`.
  - **작업**: `oxi-ai/src/error.rs`에 per-provider 에러 subtypes 추가. 각 provider의 HTTP 에러 처리가 구조화된 subtype 사용.
  - **위험**: 중간 (모든 provider의 에러 생성 경로). 재시도 로직(`is_retryable()`)이 새 subtypes 인식하도록.

### P0.5 — provider 포팅 [미착수, 가장 큼]

omp의 14 KnownApi 중 7개 dialect에 transport가 없음 (`_ => None` arm). remote-AGENT 프로토콜 3개는 OpenAI-compat endpoint가 아니라 **고유 프로토콜**:

- [ ] **Ollama** (`ollama-chat`): omp `packages/ai/src/providers/ollama.ts` + `packages/catalog/src/provider-models/ollama.ts` 참조. 로컬 서버(`/api/chat`). production 필수. 중간 노력.
- [ ] **Cursor** (`cursor-agent`): remote-AGENT 프로토콜. omp `packages/ai/src/providers/cursor.ts`. 고유 stream function + 프로토콜. 높은 노력.
- [ ] **Devin** (`devin-agent`): remote-AGENT. omp `packages/ai/src/providers/devin.ts`. 높은 노력.
- [ ] **GitLab Duo** (`gitlab-duo-agent`): remote-AGENT. omp `packages/ai/src/providers/gitlab-duo.ts`. 높은 노력.
- (우선순위 낮: OpenRouter, OpenAiCodexResponses, GoogleGeminiCli — OpenAI-compat 또는 경량)

각 포팅은 omp 소스(`/tmp/omp`에 클론됨, 또는 github.com/can1357/oh-my-pi)를 Rust로 번역. 수락 기준: 해당 `Api` variant가 transport를 가져, 통합 테스트(mock server) 통과.

### P0.2 — complexity machinery 제거 [미착수, 위험]

omp에 대응 없는 oxi-original ~225KB: `multi_provider.rs`(45KB), `complexity_router.rs`(22KB), `circuit_breaker.rs`(32KB), `fallback_chain.rs`(19KB), `provider_pool.rs`(6KB), `router/`(~100KB). consumer가 넓음: `oxi-agent` (agent_loop/retry, recovery, tests), `oxi-cli` (bootstrap, settings, tui/handlers), `oxi-sdk` (builder, 자체 multi_provider.rs, prelude), `ProviderEvent`의 FallbackStart/FallbackExhausted 변형.
- **위험**: 높음 — agent 루프 retry/recovery가 이것에 의존. 전면 삭제는 agent의 fallback/retry 동작을 direct dispatch로 재연결해야.
- **권장**: 별도 세션에서 신중히. `ProviderEvent::FallbackStart/FallbackExhausted` 제거 + consumer를 `get_provider`/`stream()` direct dispatch로 전환.

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
