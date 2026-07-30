# omp-정렬 리팩토링 — 다음 세션 인계 (HANDOFF)

- **최종 갱신**: 2026-07-28
- **브랜치**: `main` (P0 + Step 2 + P1.1 owned dialect 완료 병합)
- **상태**: **SUPERSEDED (2026-07-30 current-status note below).** 이 줄의 P1/P2–P4 상태는 2026-07-28 snapshot입니다.

> **이 문서를 가장 먼저 읽으세요.** 완료된 작업·남은 작업·실행 방법·존중할 결정이 한 곳에 있습니다.

> **현재 상태 (2026-07-30):** 이 인계 문서의 미완료 목록은 superseded 되었습니다.
> P0/P0.5/P1/P3/P4 구조 작업과 P2 tape production cutover는 완료되었습니다.
> P2 rich-content는 부분 완료입니다. 모든 dialect transport는 explicit
> dispatch arm을 가지고 있고, backlog gap은 없습니다. Codex Responses는
> OpenAI Responses transport를 재사용하며, Gemini CLI는
> `GeminiCliProvider`(`ProviderError::NotImplemented`) 의도된 stub입니다.

---

## 0. 한 줄 요약

2026-07-28 snapshot: P0(Provider/AI 재설계), Step 2, P1.1 owned dialect foundation이 완료된 시점의 요약입니다. 이후 P0.5/P1/P3/P4 구조 작업과 P2 tape production cutover까지 완료되었습니다.

---

## 1. 즉시 시작 (Quick Start)

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
git checkout main

# 회귀 게이트 (각 변경마다 통과해야 함)
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace

# omp 소스 (포팅/참조용; 없으면 클론)
ls /tmp/omp 2>/dev/null || git clone https://github.com/can1357/oh-my-pi.git /tmp/omp
```

**읽기 순서**: 이 문서 → `2026-07-27-omp-realignment-status.md`(진행/남은 작업 상세) → 해당 phase 계획 문서.

---

## 2. 완료된 작업 (전부 main 병합, 게이트 green)

### Phase 0 — Provider/AI 재설계 [완료]

| 커밋 | 단계 | 내용 |
|---|---|---|
| `4836a17b` | **P0.1** | `oxi-catalog` 별도 leaf 크레이트 추출. `Api` + `catalog/` + `product_env` + `data/catalog/` 이관. 의존성 단방향 복원. |
| `7532a22e` | **P0.3** | **정체성 붕괴 수정 (당시 단계)**. `NamedProvider`로 identity를 transport와 분리; 이후 Step 2에서 `Provider::name()`과 래퍼 모두 제거. |
| `8410bc73` | P0.4 | `ImageStart`/`ImageDelta`/`ImageEnd` 스트리밍 이벤트. |
| `f408672c` | P0.4 | `Api` → omp `KnownApi` 14개 확장 + `Mistral` 제거. |
| `2bd2ccdd` | P0.4 | `HttpErrorDetail{status,body,provider,request_id}` 구조화. |
| `1c57c08c` | P0.4 | SSE byte-stream framing 중앙화 (`providers/sse.rs`). |
| `01180496` | P0.4 | `Api::from_kebab_str` — stale `parse_api` 수정. |
| `ae441c1f` | **P0.2** | opt-in routing 층 제거: `multi_provider.rs`, `complexity_router.rs`, `provider_pool.rs`, `OxiBuilder::enable_routing()`, `FallbackStart/FallbackExhausted` 이벤트, `AgentEvent::Fallback`, `UiEvent::ModelChanged`. **−2730 lines**. |
| `afe9cf04` | **P0.2b** | CircuitBreaker + FallbackChain 제거: `circuit_breaker.rs`(944 LOC), `fallback_chain.rs`(642 LOC), agent 루프 retry 재연결(독립 retry 로직 유지). **−2061 lines**. |
| `50d88302` | **P0.5** | `OllamaProvider` — NDJSON streaming, thinking/tool-call 지원, `sanitizeSchemaForOllama`, `Api::OllamaChat` transport 연결. **+693 lines, 9 tests**. |
| `72f1df92` | **Step 2** | `Provider::name()` trait에서 제거. `NamedProvider` 래퍼 폐기. identity = registry key / `Model.provider`. 24 files, −174 lines. 3556 tests green. |

**검증**: 매 커밋 build + clippy + native-browser + fmt + nextest green. 최종 3556 tests.

### 사용자 pain 해결
- **"프로바이더가 이상하다"** → 정체성 붕괴 수정됨.
- **catalog/ai boundary** → 복원됨.
- **API dialect 정렬** → 완료 (14 KnownApi).
- **dead code 제거** → −4791 lines (complexity machinery 전체).
- **Ollama 지원** → 로컬 LLM 사용 가능.

---

## 3. 2026-07-28 당시 남은 작업 (superseded)

### Step 1 — P0.5 remote-AGENT provider (현재 완료)
두 dialect 모두 explicit dispatch arm이 있습니다: Codex Responses는
`OpenAiResponsesProvider`를 재사용하고, Gemini CLI는 의도적으로
`ProviderError::NotImplemented`를 반환하는 stub입니다.

### Step 2 — P0.3 후속: Provider trait에서 `name()` 제거 [완료]
`Provider::name()`을 trait에서 제거하고 `NamedProvider` 래퍼를 폐기함. identity는 이제 registry key와 `Model.provider` 필드에만 존재. factory 함수(`create_builtin_provider*`)는 transport를 직접 반환. P0.3 정체성 회귀 테스트는 `is_some()` + registry-key 기반으로 마이그레이션됨.

### Step 3 — P1: Agent 루프 재정렬 [현재 완료]
omp `packages/agent/src/agent-loop.ts`(102KB) 기준. 상세: `plans/2026-07-27-p1-agent-loop-realignment.md`.

**P1.1 owned dialect — 완료** (`5584ee46` 엔진 + `414f2036` 루프 wiring):
- **엔진** (`oxi-ai/src/dialect/`): `Dialect` enum(11종) + `from_name`/`preferred_for_model`, 3-piece 계약(prompt 주입 / history 인코딩 / output 파싱), XML dialect 완전 구현(renderer + batch parser, render→parse round-trip 검증), coercion(string-arg 스키마 감지). 24 tests.
- **루프 wiring** (`AgentLoopConfig.dialect` opt-in, omp `config.dialect` 정렬): 요청 측 = 카탈로그 시스템 프롬프트 주입 + history 텍스트 재인코딩 + native tools 미전송 (`streaming.rs` context 빌드). 출력 측 = `Done`에서 `parse_assistant_message()`로 in-band 텍스트→native `ToolCall` 재물질화, clean Stop→ToolUse 전환으로 루프 지속.
- **수락 테스트**: `test_owned_dialect_inband_tool_calling` — native tool 블록 없이 텍스트만 내뱉는 mock이 루프를 구동, in-band call 파싱·실행·루프 지속 검증.
- **남은 강화(선택)**: 배치 파서 → 증분 streaming scanner(omp `owned-stream.ts`의 `InbandStreamProjector`, fabrication abort). 토큰 단위 UX 개선, 기능 필수 아님.


2026-07-28 당시 P1 잔여 목록(현재 완료): intent tracing, append-only
context, approval/tier, soft requirements, Harmony leak 감지, 누락 도구 포팅.

### Step 4–6 — P3/P4/P2 [현재 상태]
- P3 프롬프트/CLI 구조 작업 완료.
- P4 oxi-original 구조 작업 및 language policy 제거 완료.
- P2 production tape cutover 완료; rich-content는 부분 완료.
  위 P2 상태가 현재 기준입니다. 당시 세부 목표 목록은 historical plan에 보존되어 있습니다.

---

## 4. 존중할 핵심 결정 (재확인 불필요)

사용자가 승인한 방침 (design doc §2):

- **B: omp-정렬 Rust-native** — 견고한 oxi-original(`oxi-sdk` port system, `oxi-lsp`, issue 시스템)은 유지, 표류한 핵심 층을 omp 설계에 정렬.
- **T1 (historical direction)**: legacy→omp tape 진화, v2 폐기.
- **issue/package/language policy (historical direction)**: 격리·재정렬·제거.

이미 구현된 architectural 결정:
- **Provider identity ≠ transport**: `Provider::name()` trait 제거 완료. identity는 registry key / `Model.provider`에만 존재 (Step 2).
- **oxi-catalog은 leaf, 단일 소스**: oxi-ai가 소비만. 역방향 의존 금지. (P0.1)
- **Api = omp KnownApi 14**: Mistral 없음. (P0.4)
- **complexity machinery 제거 완료**: MultiProvider, ComplexityRouter, CircuitBreaker, FallbackChain, ProviderPool 전부 삭제. agent 루프 retry는 독립 로직(`stream_retry.rs`: 3 attempts, exponential backoff)으로 동작. (P0.2/P0.2b)
- **`router/` 모듈은 live**: oxi-cli auto-routing + `/router` slash command + overlay에서 사용 중. 삭제 대상 아님.

---

## 5. 문서 지도

| 문서 | 용도 |
|---|---|
| **이 문서 (HANDOFF)** | 다음 세션 진입점 |
| `specs/2026-07-27-omp-realignment-status.md` | 진행 상황 + 남은 작업 상세 |
| `specs/2026-07-27-omp-realignment-design.md` | 마스터 설계 (5 phase, 5 원칙) |
| `specs/2026-07-27-omp-realignment-analysis.md` | 6 도메인 omp↔oxi 분석 증거 |
| `plans/2026-07-27-p0-provider-redesign.md` | P0 실행 계획 (완료) |
| `plans/2026-07-27-p1-agent-loop-realignment.md` | P1 agent 루프 상세 계획 |
| `plans/2026-07-27-p2-tui-realignment.md` | P2 TUI 상세 계획 (다-월간) |
| `plans/2026-07-28-remaining-work.md` | **남은 구현 전부** — 작업별 omp 파일·예상 규모·의존성·수락 기준 |

---

## 6. 주의사항

- **omp는 TS 중심**, Rust는 perf-critical natives만. 포팅은 Rust 관용구로 번역(Bun/N-API/npm 특산물 문자적 포팅 X).
- **비목표**: collab-web, swarm-extension, metaharness, wire protocol, stats dashboard, OS-integration natives — 별도 평가.
- **파일 포맷**: TOML 유지 (Rust 표준), omp의 YAML로 되돌리지 말 것.
- 각 phase는 **독립 배포 가능**. big-bang 교체 금지, callsite 점진적 마이그레이션.
- `cargo clippy -p oxi-sdk --features native-browser` 잊지 말 것 (edition-2024 lifetime 버그 catch).
- **oxi-cli settings의 dead config 필드** (`circuit_breaker_failure_threshold`, `circuit_breaker_open_duration_secs`, `enable_routing`, `prefer_cost_efficient`, `fallback_chain`, `disable_fallback`)는 아직 struct에 남아있음. serde가 unknown key를 무시하므로 기존 settings.toml은 안전. P4에서 정리 권장.

---

## 7. 사용자 컨텍스트

- 핵심 pain은 **프로바이더가 이상하다** → 정체성 붕괴로 진단·수정 완료.
- 자율 실행 중 사용자 승인 없이 진행; design doc §2의 승인된 방침이 진실의 소스.
- 진행 상황은 장기 기억에도 저장됨 (`recall("oxi omp-realignment")`).
