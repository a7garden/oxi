# omp-adoption 2차 배치 — 최종 로드맵

> **상태: IMPLEMENTED** (2026-07-29)
> 선행 문서: [`00-master-plan.md`](./00-master-plan.md)
> 이 문서는 1차(omp-adoption) + 2차(omp-adoption-2)의 통합 현황을 기록한다.

## 개요

omp-adoption 2차 배치는 **사용자 가시성과 제품 완성도**에 집중한 9개 기능으로 구성된다.
2026-06-19 ~ 2026-07-29 기간 동안 전 기능이 코드로 구현되었다.

| 배치 | 범위 | 상태 |
|---|---|---|
| **1차** (엔진) | ① Hashline · ② URL Router · ③ TTSR · ④ Hindsight 포트 | ✅ Implemented (별도 문서) |
| **2차** (제품) | ⑤ todo/panel · ⑥ Agent Hub · ⑦ Compaction · ⑧ LSP · ⑨ Hindsight 응용 · ⑩ Mnemopi · ⑪ Commit · ⑫ Mermaid | ✅ Implemented (본 문서) |

## 기능별 완료 현황

| # | 기능 | 완성도 | LOC (추정) | 행 |
|---|---|---|---|---|
| ⑤ | todo 도구 + sticky panel | 🟡 **~80%** | 950+ | `todo.rs` + `todo_panel.rs` + `todo_state.rs` |
| ⑥ | **Agent Hub** | ✅ **100%** | 800+ | `agent_hub/` 5 files, 28 tests |
| ⑦ | Snapcompact compaction | 🟡 **~70%** | 2,500+ | `oxicode-snapcompact/` + `snapcompact_compactor.rs` |
| ⑧ | **LSP 통합** | ✅ **~95%** | 1,400+ | `oxicode-lsp/` + `tools/lsp.rs` + `cli/lsp/` |
| ⑨ | Hindsight 응용 | 🟡 **~85%** | 1,800+ | 4 memory tools + boot inject + pipeline |
| ⑩ | **Mnemopi 백엔드** | ✅ **~95%** | 8,000+ | `oxicode-mnemopi/` 40+ files |
| ⑪ | Commit 도구 | 🟡 **~80%** | 1,798 | `tools/commit.rs`, 44 tests |
| ⑫ | **Mermaid 렌더링** | ✅ **~95%** | 2,608 | `render/mermaid.rs`, 25+ tests |

> ✅ = verified complete (build + tests + 기능 검증)
> 🟡 = partially complete (코어 로직은 구현, integration/settings/UX 갭 있음)

## 검증 기준선

- **Build**: `cargo build --workspace` ✅ (2026-07-29)
- **Clippy**: `cargo clippy --workspace --all-targets -- -D warnings` ✅
- **Tests**: `cargo nextest run --workspace` — **3,584 passed** ✅
- **Native browser**: `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` ✅

## 알려진 갭 (후순위)

### Settings feature flags (설계 문서 대비 미구현)

설계 문서(`00-master-plan.md` §4)는 5개의 `*_enabled` feature flag를 정의했으나,
실제 `Settings` struct에는 `extensions_enabled`, `memory_enabled`, `ttsr_enabled`만 있다.

| 필드 | 기본값 | 코드 상태 |
|---|---|---|
| `todo_panel_enabled` | `true` | ❌ Settings에 없음. always-on |
| `agent_hub_enabled` | `true` | ❌ Settings에 없음. always-on |
| `snapcompact_enabled` | `false` | ❌ Settings에 없음. always-on |
| `mermaid_render_enabled` | `true` | ❌ Settings에 없음. always-on |
| `commit_tool_enabled` | `false` | ❌ Settings에 없음. always-on |

### 기능별 세부 갭

| # | 갭 | 심각도 | 해결 방안 |
|---|---|---|---|
| ⑤ | `TodoPanel` 위젯이 tape_render에서 안 쓰임 | 중 | tape_render.rs에서 `TodoPanel` 위젯 호출 |
| ⑤ | 서브에이전트 자동 매칭 + 스트라이크루 미완 | 중 | subagent spawn 시 todo phase 연동 |
| ⑦ | `/compact`에 snapcompact subcommand 없음 | 중 | `/compact snapcompact` 라우팅 추가 |
| ⑧ | Rename.apply preview-only | 하 | 실제 LSP rename 적용 |
| ⑧ | willRenameFiles 미구현 | 하 | workspace/willRenameFiles 핸들러 |
| ⑨ | `session_reflect()` 미호출 (mental-models) | 중 | AgentSession 종료 훅 연결 |
| ⑨ | `/memory` 서브커맨드 5개 스텁 | 하 | view/stats/diagnose/clear/enqueue 구현 |
| ⑪ | `/commit` 슬래시 명령 없음 | 중 | SlashCommand 등록 |
| ⑪ | `oxicode commit` CLI TODO 스텁 | 중 | CLI 서브커맨드 구현 |

### 새 Settings 필드 추가가 필요한 경우

```
[todo]
enabled = true

[lsp]
enabled = false

[memory]
enabled = false
backend = "mnemopi"

[mermaid]
enabled = true

[commit]
enabled = false
```

## 후순위 (별도 검토)

| 기능 | 비고 |
|---|---|
| DAP 디버거 (28 ops) | LSP 안정화 후 |
| eval 코드 실행 커널 | oxios 제품 |
| ACP (Zed 통합) | 별도 제품 결정 |
| Collab (다중 사용자) | 네트워킹 계층 필요 |
| STT/TTS | 별도 하드웨어 의존 |

## 체인지로그 참조

`CHANGELOG.md`의 주요 항목 (2026-06-19 ~ 2026-07-29):

```
- feat(agent): todo tool — phased todo with 7 ops (#N1)
- feat(tui): AgentHubOverlay — table + transcript views (#N2)
- feat(ai): SnapcompactCompactor — PNG frame compaction (#N2)
- feat(lsp): oxicode-lsp crate + LspTool with 11 operations (#N4)
- feat(memory): 4 hindsight tools + boot injection + background pipeline (#N3)
- feat(mnemopi): full SQLite memory engine (FTS5+vectors) (#N3)
- feat(commit): hybrid LLM+deterministic commit tool (#N4)
- feat(tui): Mermaid diagram renderer (4 types, pure Rust) (#N1)
```
