# omp-정렬 리팩토링 — 다음 세션 인계 (HANDOFF)

- **작성**: 2026-07-27 (자율 실행 세션 종료 시점)
- **브랜치**: `omp-realignment-p0` (base: `main`, 11 커밋, 전 green)
- **상태**: P0.1·P0.3·P0.4 **완료** + P0.2 정밀 분석 + P1/P2 상세 계획

> **이 문서를 가장 먼저 읽으세요.** 완료된 작업·남은 작업·실행 방법·존중할 결정이 한 곳에 있습니다. 상세는 링크된 문서로.

---

## 0. 한 줄 요약

사용자의 핵심 pain(프로바이더 정체성 붕괴: `create_builtin_provider("deepseek").name()=="openai"`)을 **수정했고**, omp-정렬 P0의 검증 가능한 고가치 작업을 완수했습니다. 남은 P0.2/P0.5/P3/P4/P1/P2는 각각 전용 세션 작업입니다.

---

## 1. 즉시 시작 (Quick Start)

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
git checkout omp-realignment-p0

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

## 2. 완료된 작업 (11 커밋, 전부 게이트 green)

| 커밋 | 단계 | 내용 |
|---|---|---|
| `4836a17b` | **P0.1** | `oxi-catalog` 별도 leaf 크레이트 추출. `Api` + `catalog/` + `product_env` + `data/catalog/` 이관, oxi-ai 재-내보내기(62 consumer 무변경). 의존성 단방향 복원. |
| `7532a22e` | **P0.3** | **정체성 붕괴 수정 (사용자 핵심 pain)**. `NamedProvider` 래퍼 → `deepseek.name()=="deepseek"`. transport builder / identity-wrapping public wrapper 분리. |
| `8410bc73` | P0.4 | `ImageStart`/`ImageDelta`/`ImageEnd` 스트리밍 이벤트 (omp image family). |
| `f408672c` | P0.4 | `Api` → omp `KnownApi` 14개 확장 + `MistralConversations` 제거(omp는 openai-completions 호환). `mistral.rs` 삭제, oxi-sdk `CatalogProtocol` mirror 동기화. |
| `2bd2ccdd` | P0.4 | `HttpErrorDetail{status,body,provider,request_id}` 구조화. Anthropic `request-id` 캡처. `http_status()` 헬퍼. |
| `1c57c08c` | P0.4 | SSE byte-stream framing 중앙화 (`providers/sse.rs`). |
| `01180496` | P0.4 | `Api::from_kebab_str` — stale `parse_api` landmine 수정(7 신규 dialect 누락). |
| `026457f4` + docs | 분석/계획 | STATUS, P1/P2 상세 계획, P0.2 정밀 분석. |

**검증**: 매 커밋 build + clippy + native-browser + fmt + nextest(3641 통과) green.

---

## 3. 다음에 할 작업 (권장 순서)

### ~~Step 1 — P0.2 opt-in 층 제거~~ ✅ 완료 (2026-07-28, main 병합됨)
complexity machinery 제거 완료 (−2730 lines, 3603 tests green):
- 삭제: `oxi-ai/src/{multi_provider.rs, complexity_router.rs, provider_pool.rs}`, `oxi-sdk/src/multi_provider.rs`, `OxiBuilder::enable_routing()`
- 제거: `ProviderEvent::FallbackStart/FallbackExhausted` + `FallbackReason`, `AgentEvent::Fallback`, `UiEvent::ModelChanged`
- **유지됨**: `circuit_breaker.rs`, `fallback_chain.rs` (agent 루프 retry에 live), **`router/`** (oxi-cli auto-routing + `/router` 명령 + overlay에서 live — 이 문서 초판의 "router/ 삭제" 지시는 오류였음)

### Step 2 — P0.2 CircuitBreaker 재연결 + 제거 (위험, 별도 집중)
`CircuitBreaker`는 `oxi-agent` retry/recovery에 live(`agent_loop/mod.rs:74`, `streaming.rs:333/435/529`, `retry.rs:41`). 제거 시 agent retry를 direct dispatch로 재연결. 신중한 회귀 필요.

### Step 3 — P0.5 Ollama 포팅 (가장 가치 있는 신규 provider)
omp `packages/ai/src/providers/ollama.ts`(750줄 + 유틸 의존) → Rust. 로컬 `/api/chat` (NDJSON). 최소 동작 포팅은 mock Ollama 서버 테스트 인프라 필요. → `Api::OllamaChat`에 transport 연결.

### Step 4 — P0.5 remote-AGENT (Cursor/Devin/GitLab Duo)
omp 고유 프로토콜. 각각 다-일 작업.

### Step 5+ — P3 / P4 / P1 / P2
`2026-07-27-omp-realignment-status.md` §3 + 각 phase 계획 문서 참조.

---

## 4. 존중할 핵심 결정 (재확인 불필요)

사용자가 승인한 방침 (design doc §2):

- **B: omp-정렬 Rust-native** — 견고한 oxi-original(`oxi-sdk` port system, `oxi-lsp`, issue 시스템)은 유지, 표류한 핵심 층을 omp 설계에 정렬.
- **T1: TUI legacy→omp tape 진화, v2 폐기** (현 `oxi-tui` v2는 grok-inspired, legacy가 omp에 더 가까움).
- **issue 시스템**: 유지하되 agent 루프/session 모델에서 격리.
- **package manager**: omp 플러그인 모델로 재정렬.
- **language policy**: 제거/단순화.

**이미 구현된 architectural 결정** (P0.1/P0.3):
- **Provider identity ≠ transport**: `NamedProvider` 래퍼가 catalog id 전달, transport(`build_builtin_transport`)는 identity 없음. **후속(P0.3 완성)**: `Provider::name()`을 trait에서 완전 제거 + `ProviderDefinition` registry를 identity 단일 소스로 + base_url/auth를 oxi-catalog `ProviderDescriptor`로 이동. 현재는 identity가 trait에 여전히 붙어있으나 붕괴는 수정됨.
- **oxi-catalog은 leaf, 단일 소스**: oxi-ai가 소비만. 역방향 의존 금지.
- **Api = omp KnownApi 14**: Mistral 없음.

---

## 5. 문서 지도

| 문서 | 용도 |
|---|---|
| **이 문서 (HANDOFF)** | 다음 세션 진입점 |
| `specs/2026-07-27-omp-realignment-status.md` | 진행 상황 + 남은 작업 상세 (이 문서의 확장판) |
| `specs/2026-07-27-omp-realignment-design.md` | 마스터 설계 (5 phase, 5 원칙) |
| `specs/2026-07-27-omp-realignment-analysis.md` | 6 도메인 omp↔oxi 분석 증거 (scout findings) |
| `plans/2026-07-27-p0-provider-redesign.md` | P0 실행 계획 |
| `plans/2026-07-27-p1-agent-loop-realignment.md` | P1 agent 루프 상세 계획 |
| `plans/2026-07-27-p2-tui-realignment.md` | P2 TUI 상세 계획 (다-월간) |

---

## 6. 주의사항

- **omp는 TS 중심**, Rust는 perf-critical natives만. 포팅은 Rust 관용구로 번역(Bun/N-API/npm 특산물 문자적 포팅 X).
- **비목표**: collab-web, swarm-extension, metaharness, wire protocol, stats dashboard, OS-integration natives — 별도 평가.
- **파일 포맷**: TOML 유지 (Rust 표준), omp의 YAML로 되돌리지 말 것.
- 각 phase는 **독립 배포 가능**. big-bang 교체 금지, callsite 점진적 마이그레이션.
- `cargo clippy -p oxi-sdk --features native-browser` 잊지 말 것 (edition-2024 lifetime 버그 catch).

---

## 7. 사용자 컨텍스트

- 사용자는 자러 가며 "모두 완료될 때까지 묻지 말고 끝까지 진행" 지시.
- 핵심 pain은 **프로바이더가 이상하다** → 정체성 붕괴로 진단·수정 완료.
- 자율 실행 중 사용자 승인 없이 진행; design doc §2의 승인된 방침이 진실의 소스.
- 진행 상황은 장기 기억에도 저장됨 (`recall("oxi omp-realignment")`).
