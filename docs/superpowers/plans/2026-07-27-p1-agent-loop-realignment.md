# P1 — Agent 루프 omp 재정렬 구현 계획

> **상위 설계:** `docs/superpowers/specs/2026-07-27-omp-realignment-design.md` (Phase 1)
> **omp 소스:** `/tmp/omp/packages/agent/src/` (agent.ts 56KB, agent-loop.ts 102KB, types.ts 35KB) + `/tmp/omp/packages/coding-agent/src/tools/`
> **대상 크레이트:** `oxi-agent/`
> **의존:** P0 완료 (Provider 3-way 분리, KnownApi 14)

**Goal:** oxi-agent를 omp의 agent 루프 설계에 정렬 — owned dialect system, intent tracing, append-only context, approval tiers, soft tool requirements, Harmony leak 감지 + 누락 도구 16개 포팅.

**Architecture:** omp는 agent 런타임을 `packages/agent`(코어 루프)와 `packages/coding-agent`(도구)로 분리. oxi는 `oxi-agent` 하나. 루프 충실도 ~40-50%, 도구 parity ~30% (ScoutAgent 2026-07-27).

## Global Constraints
- 회귀 게이트: `cargo nextest run --workspace` + `cargo clippy --workspace --all-targets -D warnings` + `cargo clippy -p oxi-sdk --features native-browser -D warnings` + `cargo fmt --all -- --check`.
- oxi-original(`ProviderResolver`, `AgentPoolProvider`, `SubagentRunner`, `LspProvider`)은 SDK 격리 층으로 **유지** (정당한 drift).
- 각 task는 독립 커밋, green 게이트.

## 작업 분해 (우선순위순)

### P1.1 — Owned dialect system (HIGH, 루프 핵심)
- **문제**: oxi는 non-native-tool 모델(툴 지원 안 하는 모델)에 in-band tool calling 불가. omp는 owned dialect로 시스템 프롬프트에 툴 스키마 주입 + 응답에서 툴콜 파싱.
- **omp 참조**: `agent-loop.ts`의 owned-dialect 분기 + `utils/`의 tool-call markup 파싱.
- **작업**: `oxi-agent/src/agent_loop/`에 owned-dialect 경로 추가. 모델이 네이티브 툴 미지원 시(`Model`의 capability 플래그) 시스템 프롬프트에 툴 스키마 주입, assistant 응답에서 XML/JSON 툴콜 파싱.
- **수락 기준**: 툴 미지원 모델로 툴콜 루프가 동작하는 통합 테스트.

### P1.2 — Intent tracing (`i` 필드) (HIGH)
- **문제**: omp `AgentTool`에 `i` 필드(intent trace)가 있어 루프가 도구 호출 의도를 추적. oxi trait은 ~10 메서드, intent 필드 없음.
- **omp 참조**: `packages/agent/src/types.ts` AgentTool 인터페이스(18+ 필드)의 `i` 필드 + 루프에서의 주입/추출.
- **작업**: `oxi-agent/src/tools.rs` AgentTool trait에 intent 추적 추가. 루프에서 `i` 주입/추출.
- **수락 기준**: tool_execution 이벤트에 intent 필드.

### P1.3 — Append-only context (prefix caching) (HIGH)
- **문제**: omp는 컨텍스트를 append-only로 유지해 안정적 prefix caching. oxi는 메시지를 재구성할 수 있어 캐싱이 깨짐.
- **작업**: `agent_loop/`의 컨텍스트 빌드를 append-only로 전환. 메시지 시퀀스가 안정적 접두사 유지.
- **수락 기준**: 연속 턴에서 prefix가 바뀌지 않음을 검증.

### P1.4 — Approval/tier 시스템 (MED)
- **문제**: omp는 사용자 확인 게이트(approval tiers). oxi는 `AccessGate` port(oxi-sdk)가 있지만 루프 통합이 다름.
- **작업**: `agent_loop/`에 approval gate 통합. 위험도별(읽기/쓰기/실행) 사용자 확인.
- **수락 기준**: 쓰기/실행 도구 전 사용자 확인 프롬프트.

### P1.5 — Soft tool requirements + Harmony leak (MED)
- **문제**: omp는 remind-then-escalate 패턴(soft requirements) + GPT-5 프로토콜 누수(Harmony leak) 감지. oxi 둘 다 없음.
- **작업**: 루프에 soft-requirement 추적 + Harmony leak 감지 추가.

### P1.6 — 누락 도구 16개 포팅 (LARGE, 병렬 가능)
omp `packages/coding-agent/src/tools/` 기준. 우선순위:
- **핵심 (P1.6a)**: `ast_grep`, `ast_edit`, `debug`, `eval` — 코드 조작/실행 도구.
- **컨텍스트 (P1.6b)**: `checkpoint`, `rewind`, `hub`, `yield`, `goal`, `review`.
- **메타 (P1.6c)**: `learn`, `manage_skill`, `inspect_image`, `computer`, `tts`, `vibe`.
- 각 도구: `oxi-agent/src/tools/<name>.rs` 생성, AgentTool trait 구현, `ToolRegistry::with_builtins_cwd()`에 등록.
- omp 소스(`packages/coding-agent/src/tools/<name>.ts`)를 Rust로 번역. 각 도구 단위 커밋 + 테스트.
- **병렬화**: 독립적인 도구는 subagent로 분산 가능 (dispatching-parallel-agents).

## oxi-유지 항목 (drift 정당)
- `ProviderResolver` trait — omp는 global catalog import, oxi는 주입형(격리용).
- `AgentPoolProvider`, `SubagentRunner`, `LspProvider` — SDK 격리.
- 런타임 토글 auto-retry, deferred model switching, `TokenSource`, tool-result truncation, 정교한 MCP(McpManager lifecycle).

## 위험
- owned dialect + intent tracing은 루프 코어 변경 → 회귀 위험. 기존 agent 루프 테스트(`oxi-agent/tests/agent_loop_full.rs`) 전부 green 유지가 gate.
- 16개 도구는 양이 많음; 핵심 4개(ast_grep/ast_edit/debug/eval) 우선, 나머지는 후속.

## 수락 기준 (P1 전체)
- owned dialect로 non-native 모델 툴콜 동작.
- intent tracing, append-only context, approval gate 통합.
- 핵심 도구 4개 이상 포팅.
- `cargo nextest run --workspace` green.
