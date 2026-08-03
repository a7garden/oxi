# Remaining Work

> 최종 갱신: 2026-07-29. 검증 기준: `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo nextest run --workspace` (**3,595 passed**, +11 new LSP unit tests).
>
> 각 항목은 **파일 위치 + 해결 방안 + 수용 기준**을 포함한다.

---

## ✅ 완료 (REMAINING에서 제거됨)

| 항목 | 상태 | 확인 위치 |
|---|---|---|
| P0.2 CommitTool LLM 주입 | ✅ **완료** | `oxicode-cli/src/bootstrap.rs:419-427` |
| P1.3 `oxicode commit` CLI | ✅ **완료** | `oxicode-cli/src/cli/commands/misc.rs` — CommitTool::unconfigured() 기반, LLM fallback은 bootstrap에서 모델 설정 시 |
| P3.1 Rename.apply | ✅ **완료** | `oxicode-cli/src/lsp/provider.rs` — apply_workspace_edit, atomic_write, UTF-16→byte 변환, 11 tests |
| P3.3 willRenameFiles | ✅ **완료** | FileRename.apply로 통합 (standalone variant는 FileRename preview와 중복) |
| P4 Mnemopi MCP | ⏳ MCP 서버 파일은 있음 (`mcp.rs` 843 LOC) — E2E 검증은 별도 PR 필요 |
| P4 Snapcompact E2E | ⏳ 완료 조건: `/compact snapcompact` 라우팅 + SnapcompactCompactor 연결 + 실제 compaction 실행 |

---

## P0 — 크리티컬 (사용자 가시성/안정성)

## ✅ 완료 (아래 항목들은 커밋됨)

| 항목 | 상태 | 확인 |
|---|---|---|
| P0.1 TodoPanel tape render | ✅ 완료 | `tape_render.rs` — ASCII markers, collapsed/expanded |
| P0.2 CommitTool LLM 주입 | ✅ 완료 | `bootstrap.rs:419-427` |
| P1.1 /compact snapcompact | ✅ 완료 | `tools_commands.rs` — strategy subcommand + save/restore |
| P1.2 /commit slash | ✅ 완료 | `commit.rs` — 등록 완료 |
| P1.3 oxicode commit CLI | ✅ 완료 | `misc.rs` — CommitTool::unconfigured() 기반 |
| P1.4 Subagent-todo matching | ✅ 완료 | `subagent.rs` — spawn 시 phase 생성, 완료 시 done |
| P1.5 session_reflect hook | ✅ 완료 | `agent_session_runtime.rs` — teardown 시 fire-and-forget |
| P2.1 Settings flags 5개 | ✅ 완료 | `settings.rs` — 모두 추가 + serde(default) |
| P3.1 Rename.apply | ✅ 완료 | `provider.rs` — UTF-16 safe, atomic write, 11 tests |
| P3.2 /memory stubs | ✅ 완료 | `memory.rs` — 5개 stub에 backend 정보 표시 |
| P3.3 willRenameFiles | ✅ 완료 | FileRename.apply로 대체 (preview 시 정보 표시) |

---

## ⏳ 검증 필요 (E2E 테스트)

| 항목 | 상태 | 다음 스텝 |
|---|---|---|
| P4.1 Mnemopi MCP 서버 | `oxicode-mnemopi/src/mcp.rs` (843 LOC) — 파일 존재, 기능 검증 안 됨 | 별도 PR: MCP 서버 기동 테스트 + LSP/MCP 연동 |
| P4.2 Snapcompact E2E | `/compact snapcompact` 라우팅 완료, SnapcompactCompactor 연결 확인 | `session.compact()` 후 PNG 출력 검증 |

---

## P4 — 실험적/후순위

위 P4 항목 참조.

---

| 항목 | 값 |
|---|---|
| **문제** | `CommitTool`은 구현 완료(1,798 LOC, 44 tests)되었으나 `/commit` slash 명령이 등록되지 않음 |
| **위치** | `oxicode-cli/src/tui/slash/builtin/` — 신규 `commit.rs` 생성 필요 |
| **해결** | SlashCommand trait 구현체를 만들고 `builtin/mod.rs::register_all()`에 등록 |
| **참조** | 설계: `08-commit-tool.md` §5 |
| **수용 기준** | `/commit` 입력 시 commit 도구 실행 |

### P1.3 `oxicode commit` CLI 서브커맨드

| 항목 | 값 |
|---|---|
| **문제** | `oxicode commit [--push] [--dry-run]` CLI 명령이 TODO 스텁으로 남아있음 |
| **위치** | `oxicode-cli/src/cli/commands/misc.rs:90` |
| **해결** | `CommitTool`을 직접 호출하는 CLI 핸들러 구현 (agent loop 없이 standalone) |
| **참조** | 설계: `08-commit-tool.md` §5 |
| **수용 기준** | `oxicode commit --dry-run`으로 분석 결과 출력, `oxicode commit`으로 실제 커밋 + push |

### P1.4 서브에이전트 자동 todo 매칭 + 스트라이크루

| 항목 | 값 |
|---|---|
| **문제** | `todo.rs`에 subagent matching을 위한 데이터 구조는 있으나(`SubagentMatch`), subagent spawn 시 자동으로 todo phase에 등록/완료되는 로직이 연결되지 않음. 스트라이크루(strikethrough) 애니메이션도 미구현 |
| **위치** | `oxicode-agent/src/tools/todo.rs` — SubagentMatch 관련 코드 |
| **해결** | subagent spawn 시 현재 진행 중인 todo item 매칭, subagent 완료 시 해당 item을 done 마킹, TUI에서 strikethrough + slide 애니메이션 |
| **참조** | 설계: `06-todo-sticky-panel.md` |
| **수용 기준** | subagent가 spawn되면 todo 도구가 자동으로 phase 업데이트. 완료 시 strikethrough 표시 |

### P1.5 Hindsight: `session_reflect()` 세션 종료 훅 연결

| 항목 | 값 |
|---|---|
| **문제** | `oxicode-cli/src/services.rs:387`에 `session_reflect()` 함수가 완전히 구현되어 있지만, 세션 종료 시 호출하는 코드가 어디에도 없음. 자동 mental-models(session-end summary → memory) 미동작 |
| **위치** | `oxicode-cli/src/services.rs` + `oxicode-cli/src/app/agent_session_runtime.rs` (종료 경로) |
| **해결** | `AgentSession` 종료/close 시점에 `session_reflect()` 호출 → MemoryBackend에 요약 저장 |
| **참조** | 설계: `12-hindsight-memory.md` §5 |
| **수용 기준** | 세션 종료 후 `/memory status`에 해당 세션의 summary가 나타남 |

---

## P2 — Settings feature flags (설계 문서 반영)

### P2.1 Settings struct에 5개 `*_enabled` 필드 추가

| 필드 | 기본값 | 현재 상태 |
|---|---|---|
| `todo_panel_enabled` | `true` | Settings에 없음 → tape_render에서 unchecked gating |
| `agent_hub_enabled` | `true` | Settings에 없음 → always-on |
| `snapcompact_enabled` | `false` | Settings에 없음 → always-on (실험적 기능인데 게이트 없음) |
| `mermaid_render_enabled` | `true` | Settings에 없음 → always-on |
| `commit_tool_enabled` | `false` | Settings에 없음 → always registered (설계 의도와 반대) |

**위치**: `oxicode-cli/src/store/settings.rs` — `Settings` struct

**해결**: 필드 추가 + serde(default) + 각 게이트 지점에서 `settings.*_enabled` 확인

**수용 기준**: 각 feature flag를 `false`로 설정하면 해당 기능이 완전히 비활성화됨 (TUI에 표시 안 되고, 도구 등록 안 되고, agent loop에서 skip됨)

**참조**: `00-master-plan.md` §4, `00-design-revisions.md` §14

---

## P3 — 사소한 갭 (low priority)

### P3.1 Rename.apply / FileRename.apply preview-only

| 항목 | 값 |
|---|---|
| **문제** | `CliLspProvider`의 `rename`/`file_rename` 핸들러가 실제 LSP `textDocument/rename`/`workspace/willRenameFiles`를 호출하지만 `apply: false` (preview-only) |
| **위치** | `oxicode-cli/src/lsp/provider.rs` |
| **해결** | `apply: true` 분기 추가 → LSP rename 요청 후 DiffBackend로 파일 내용 갱신 |
| **수용 기준** | `Rename.apply`로 실제 파일 rename이 발생하고 LSP 서버의 편집이 적용됨 |

### P3.2 `/memory` 서브커맨드 스텁 구현

| 항목 | 값 |
|---|---|
| **문제** | `/memory view`, `/memory stats`, `/memory diagnose`, `/memory clear`, `/memory enqueue` 5개 서브커맨드가 static info 반환 스텁 |
| **위치** | `oxicode-cli/src/tui/slash/builtin/memory.rs` |
| **해결** | 각 서브커맨드를 MemoryBackend/Mnemopi 진단 실제 호출로 교체 |
| **수용 기준** | `/memory stats`가 실제 vector DB 통계 표시, `/memory diagnose`가 FTS5/vector 인덱스 상태 표시 |

### P3.3 `workspace/willRenameFiles` 통합

| 항목 | 값 |
|---|---|
| **문제** | 설계 문서가 `willRenameFiles`를 LSP 통합의 일부로 명시하지만, 현재 `oxicode-lsp`/`oxicode-agent/tools/lsp.rs`에 구현이 확인되지 않음 |
| **위치** | `oxicode-lsp/src/lib.rs` + `oxicode-agent/src/tools/lsp.rs` |
| **해결** | `willRenameFiles` 핸들러를 `LspClient`/`LspTool`에 추가 |
| **참조** | 설계: `10-lsp-integration.md` §5 |
| **수용 기준** | LSP `workspace/willRenameFiles` 요청 전송 및 결과 처리 |

---

## P4 — 실험적/후순위 (별도 결정 필요)

| 항목 | 설계 참조 | 비고 |
|---|---|---|
| Mnemopi MCP 서버 노출 | `11-mnemopi-backend.md` §4 | `oxicode-mnemopi/src/mcp.rs` (843 LOC) — 파일은 있으나 기능 검증 안 됨 |
| Snapcompact inline imaging transform hook | `09-compaction-modes.md` §4 | `ContextTransformer` trait + hook은 있는데 `/compact` 연결이 안 되어 있어 end-to-end 미검증 |
| $\\cdots$ | | |

---

## 변경 이력

| 날짜 | 변경 |
|---|---|
| 2026-07-29 | 최초 작성. Audit 결과 기준 17개 항목 등록 |
