# omp-정렬 리팩토링 — 진행 상황 (STATUS)

- **최종 갱신**: 2026-07-28
- **브랜치**: `main` (모든 P0 + Step 2 병합 완료)
- **상위 설계**: `docs/superpowers/specs/2026-07-27-omp-realignment-design.md`
- **실행 계획**: `docs/superpowers/plans/2026-07-27-p0-provider-redesign.md`
- **분석 증거**: `docs/superpowers/specs/2026-07-27-omp-realignment-analysis.md`

> **현재 상태 (2026-07-30):** 아래 내용은 2026-07-28 당시의 진행 기록이며,
> 남은 작업 목록은 superseded 되었습니다. P0/P0.5/P1/P3/P4 구조 작업과
> P2 tape production cutover는 완료되었습니다. P2 rich-content는 부분 완료입니다.
> 모든 `Api` dialect는 explicit dispatch arm을 가지고 있습니다. Codex Responses는
> OpenAI Responses transport를 재사용하고, Gemini CLI는 의도적으로
> `ProviderError::NotImplemented`를 반환하는 stub (`GeminiCliProvider`) 입니다 —
> backlog가 아닙니다.

---

## 1. 완료된 작업

모든 커밋은 회귀 게이트를 통과했습니다: `cargo build --workspace`, `cargo clippy --workspace --all-targets -D warnings`, `cargo clippy -p oxicode-sdk --features native-browser -D warnings`, `cargo fmt --all -- --check`, `cargo nextest run --workspace`.

### Phase 0 — Provider/AI 재설계 [완료]

| 커밋 | 단계 | 내용 |
|---|---|---|
| `4836a17b` | **P0.1** | `oxicode-catalog` 별도 leaf 크레이트 추출. `Api` enum + `catalog/` 7파일 + `product_env` + `data/catalog/` 이관. oxicode-ai는 재-내보내기로 62개 consumer 무변경. 의존성 방향 단방향(oxicode-ai → oxicode-catalog) 복원. |
| `7532a22e` | **P0.3** | **프로바이더 정체성 붕괴 수정 (사용자 핵심 pain)**. 당시 `NamedProvider` 래퍼로 registry identity를 transport 이름과 분리. 이후 Step 2에서 `Provider::name()`과 래퍼를 제거해 registry key / `Model.provider`만 정체성 소스로 남김. |
| `8410bc73` | P0.4 | `ImageStart`/`ImageDelta`/`ImageEnd` 스트리밍 이벤트 추가. |
| `f408672c` | P0.4 | `Api`를 omp `KnownApi` 14개로 확장 + `Mistral` 제거. `mistral.rs` + 16 테스트 삭제. |
| `2bd2ccdd` | P0.4 | `HttpErrorDetail { status, body, provider, request_id }` 구조화. Anthropic `request-id` 캡처. |
| `1c57c08c` | P0.4 | SSE byte-stream framing 중앙화 (`providers/sse.rs`). |
| `01180496` | P0.4 | `Api::from_kebab_str` — stale `parse_api` landmine 수정. |
| `ae441c1f` | **P0.2** | opt-in routing 층 제거 (−2730 lines): `multi_provider.rs`, `complexity_router.rs`, `provider_pool.rs`, `oxicode-sdk/multi_provider.rs`, `OxicodeBuilder::enable_routing()`, `ProviderEvent::FallbackStart/FallbackExhausted`, `FallbackReason`, `AgentEvent::Fallback`, `UiEvent::ModelChanged`. |
| `afe9cf04` | **P0.2b** | CircuitBreaker + FallbackChain 제거 (−2061 lines): `circuit_breaker.rs`(944 LOC), `fallback_chain.rs`(642 LOC), agent 루프 CB 필드/gate/recording 제거, `stream_with_retry_core` on_success/on_failure 훅 제거. 독립 retry 로직(`stream_retry.rs`) 유지. 51 테스트 제거. |
| `50d88302` | **P0.5** | `OllamaProvider` (+693 lines, 9 tests): NDJSON streaming (`POST /api/chat`), thinking/text delta + content_index, complete tool calls, `sanitizeSchemaForOllama`, Bearer auth (Ollama Cloud), `Api::OllamaChat` transport 연결 (양쪽 factory 경로). |

### 사용자 pain 해결 상태
- **"프로바이더가 이상하다"** → 정체성 붕괴 **수정됨** (`7532a22e`).
- **catalog/ai boundary** → **복원됨** (`4836a17b`).
- **API dialect 정렬** → **완료** (`f408672c`, 14 KnownApi).
- **dead code** → **제거됨** (−4791 lines, P0.2/P0.2b).
- **Ollama 지원** → **추가됨** (`50d88302`).

---

## 2. 2026-07-28 당시 남아 있던 P0 작업 (superseded)

### P0.5 — remote-AGENT provider 포팅 [현재 완료]

2026-07-28 당시에는 Cursor, Devin, GitLab Duo, GitLab Duo Agent transport가
없었지만, 이후 모두 연결되었습니다. Codex Responses와 Gemini CLI도 explicit
dispatch arm이 있으며 Codex는 `OpenAiResponsesProvider`를 재사용하고 Gemini CLI는
의도된 `NotImplemented` stub입니다.

### P0.3 후속 — Provider trait `name()` 제거 [완료]

`Provider::name()`을 trait에서 제거하고 `NamedProvider` 래퍼를 폐기함 (21 files, −175 lines). 완전한 3-way 분리 달성:
- `Provider::name()` trait에서 제거 — transport 및 mock 구현에서 삭제
- `NamedProvider` 래퍼 폐기 — factory 함수(`create_builtin_provider*`)는 transport를 직접 반환
- identity는 이제 registry key와 `Model.provider` 필드에만 존재
- P0.3 정체성 회귀 테스트는 `is_some()` + registry-key 기반으로 마이그레이션

---

## 3. P1–P4의 2026-07-28 계획 (superseded)

각 단계는 design doc §4의 해당 Phase 참조.

> 현재 P1/P3/P4 구조 작업은 완료되었습니다. P2 production tape cutover도
> 완료되었고 rich-content 범위만 부분 완료 상태입니다. 아래 항목은 당시 계획 증거입니다.

- **P1 — Agent 루프 재정렬** [진행 중]: **P1.1 owned dialect 완료**(`5584ee46` 엔진 + `414f2036` 루프 wiring + 수락 테스트). `AgentLoopConfig.dialect` opt-in으로 native tool 미지원 모델이 in-band 텍스트로 tool calling 구동. 남은 P1: streaming scanner(선택 강화), intent tracing(`i` 필드), append-only context, approval/tier, soft tool requirements, Harmony leak 감지. 누락 도구 16개 포팅(`ast_grep`, `ast_edit`, `debug`, `eval`, `computer`, `checkpoint`, `rewind`, `hub`, `learn`, `manage_skill`, `inspect_image`, `yield`, `goal`, `review`, `tts`, `vibe`). 상세: `plans/2026-07-27-p1-agent-loop-realignment.md`.
- **P2 — TUI 재정렬** (가장 큼, 다-월간): `oxicode-tui-legacy` → `oxicode-tui` rename, 현 v2 폐기. omp 3-전략 차등 렌더링 + append-only tape 계약. 전체 입력 시스템. LaTeX/mermaid/image. glyph 단일화. omp `packages/tui/src/tui.ts`(173KB) 기준. 상세: `plans/2026-07-27-p2-tui-realignment.md`.
- **P3 — 프롬프트 & CLI**: `.md` 기반 시스템 프롬프트(`include_str!()`). personality 시스템. tool-specific prompt `.md`(~45개). 환경 정보 주입. 누락 CLI 명령 포팅. `bootstrap.rs`/`lib.rs` 경계 정리 + F-5(main.rs inline subcommand → `cli/commands/*.rs`).
- **P4 — oxicode-original 처리**: issue 시스템 격리, package manager → omp 플러그인 모델 재정렬, language policy 제거/단순화. oxicode-cli settings dead config 필드 정리 (`circuit_breaker_*`, `enable_routing`, `prefer_cost_efficient`, `fallback_chain`, `disable_fallback`).

---

## 4. 이어하기 가이드

```bash
cd /Volumes/MERCURY/PROJECTS/oxicode
git checkout main

# 회귀 게이트 (각 변경마다)
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace

# omp 소스 (포팅 참조용)
ls /tmp/omp   # 또는 git clone https://github.com/can1357/oh-my-pi.git
```

### 다음 세션 우선순위 권장
1. **P1 Agent 루프 재정렬** (가장 높은 사용자 체감 가치)
2. **P3 프롬프트 & CLI** (P1의 프롬프트 기반)
3. **P4 oxicode-original 처리** (독립적, 빠른 승리)
4. **P0.5 remote-AGENT** (요청 시)
5. **P2 TUI** (다-월간, 마지막)

### 핵심 architectural 결정 (확정, 존중할 것)
- **Provider identity ≠ transport**: `Provider::name()` trait 제거 완료(Step 2, `72f1df92`). identity = registry key / `Model.provider`. `NamedProvider` 래퍼 폐기.
- **oxicode-catalog은 leaf, 단일 소스**: oxicode-ai가 소비만. 역방향 의존 금지. (P0.1)
- **Api = omp KnownApi 14**: Mistral 없음. (P0.4)
- **complexity machinery 제거 완료**: agent 루프 retry는 `stream_retry.rs`(3 attempts, exponential backoff)로 독립 동작. (P0.2/P0.2b)
- **`router/` 모듈은 live**: oxicode-cli auto-routing + `/router` 명령 + overlay. 삭제 대상 아님.

### 사용자 승인된 방침 (재확인 불필요)
- B: omp-정렬 Rust-native (port system·LSP·issue 시스템 유지, 핵심 층 재정렬)
- T1: legacy→omp tape, v2 폐기
- issue 시스템: 유지하되 격리 / package manager: omp 플러그인으로 재정렬 / language policy: 제거
