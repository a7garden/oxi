# Remaining Work

> 최종 갱신: 2026-07-29. 검증 기준: `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo nextest run --workspace` (3,584 passed).
>
> 각 항목은 **파일 위치 + 해결 방안 + 수용 기준**을 포함한다.

---

## ✅ 완료 (REMAINING에서 제거됨)

| 항목 | 상태 | 확인 위치 |
|---|---|---|
| P0.2 CommitTool LLM 주입 | ✅ **완료** | `oxi-cli/src/bootstrap.rs:419-427` — `resolve_role_to_model(ModelRole::Commit)` → `CommitTool::new(model)` |
| P1.3 `oxi commit` CLI 서브커맨드 | — | P1.2 (/commit slash)와 동일 설계에서 함께 처리 |

---

## P0 — 크리티컬 (사용자 가시성/안정성)

### P0.1 TodoPanel 위젯을 tape_render에 연결

| 항목 | 값 |
|---|---|
| **문제** | `oxi-tui/src/widgets/todo_panel.rs`에 `TodoPanel` StatefulWidget(ratatui render impl)이 완전히 구현되어 있지만, `tape_render.rs`는 compact `"X todos"` badge만 표시함 |
| **위치** | `oxi-cli/src/tui/tape_render.rs:47-49` (현재 badge) |
| **해결** | tape_render.rs에서 `TodoPanel` 위젯의 `render()`를 호출하도록 sticky 영역에 통합 |
| **수용 기준** | 화면 상단에 todo 패널이 접힌/펼친 상태로 표시됨. `/todo` 명령으로 phase/items 조작 가능 |

---

## P1 — 기능 완성 (integration gap)

### P1.1 `/compact snapcompact` 서브커맨드

| 항목 | 값 |
|---|---|
| **문제** | `CompactCommand`가 `/compact` slash 명령을 등록했지만, 설계 문서의 `soft\|snapcompact\|remote` subcommand 라우팅이 없음. snapcompact 모드를 TUI에서 선택할 방법 없음 |
| **위치** | `oxi-cli/src/tui/slash/builtin/tools_commands.rs` — `CompactCommand.execute()` |
| **해결** | `/compact snapcompact` argument를 파싱해 `CompactionStrategy::Snapcompact`로 session.compact() 호출 |
| **참조** | 설계: `09-compaction-modes.md` §4 |
| **수용 기준** | `/compact snapcompact` 입력 시 snapcompact PNG compaction 실행 |

### P1.2 `/commit` 슬래시 명령

| 항목 | 값 |
|---|---|
| **문제** | `CommitTool`은 구현 완료(1,798 LOC, 44 tests)되었으나 `/commit` slash 명령이 등록되지 않음 |
| **위치** | `oxi-cli/src/tui/slash/builtin/` — 신규 `commit.rs` 생성 필요 |
| **해결** | SlashCommand trait 구현체를 만들고 `builtin/mod.rs::register_all()`에 등록 |
| **참조** | 설계: `08-commit-tool.md` §5 |
| **수용 기준** | `/commit` 입력 시 commit 도구 실행 |

### P1.3 `oxi commit` CLI 서브커맨드

| 항목 | 값 |
|---|---|
| **문제** | `oxi commit [--push] [--dry-run]` CLI 명령이 TODO 스텁으로 남아있음 |
| **위치** | `oxi-cli/src/cli/commands/misc.rs:90` |
| **해결** | `CommitTool`을 직접 호출하는 CLI 핸들러 구현 (agent loop 없이 standalone) |
| **참조** | 설계: `08-commit-tool.md` §5 |
| **수용 기준** | `oxi commit --dry-run`으로 분석 결과 출력, `oxi commit`으로 실제 커밋 + push |

### P1.4 서브에이전트 자동 todo 매칭 + 스트라이크루

| 항목 | 값 |
|---|---|
| **문제** | `todo.rs`에 subagent matching을 위한 데이터 구조는 있으나(`SubagentMatch`), subagent spawn 시 자동으로 todo phase에 등록/완료되는 로직이 연결되지 않음. 스트라이크루(strikethrough) 애니메이션도 미구현 |
| **위치** | `oxi-agent/src/tools/todo.rs` — SubagentMatch 관련 코드 |
| **해결** | subagent spawn 시 현재 진행 중인 todo item 매칭, subagent 완료 시 해당 item을 done 마킹, TUI에서 strikethrough + slide 애니메이션 |
| **참조** | 설계: `06-todo-sticky-panel.md` |
| **수용 기준** | subagent가 spawn되면 todo 도구가 자동으로 phase 업데이트. 완료 시 strikethrough 표시 |

### P1.5 Hindsight: `session_reflect()` 세션 종료 훅 연결

| 항목 | 값 |
|---|---|
| **문제** | `oxi-cli/src/services.rs:387`에 `session_reflect()` 함수가 완전히 구현되어 있지만, 세션 종료 시 호출하는 코드가 어디에도 없음. 자동 mental-models(session-end summary → memory) 미동작 |
| **위치** | `oxi-cli/src/services.rs` + `oxi-cli/src/app/agent_session_runtime.rs` (종료 경로) |
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

**위치**: `oxi-cli/src/store/settings.rs` — `Settings` struct

**해결**: 필드 추가 + serde(default) + 각 게이트 지점에서 `settings.*_enabled` 확인

**수용 기준**: 각 feature flag를 `false`로 설정하면 해당 기능이 완전히 비활성화됨 (TUI에 표시 안 되고, 도구 등록 안 되고, agent loop에서 skip됨)

**참조**: `00-master-plan.md` §4, `00-design-revisions.md` §14

---

## P3 — 사소한 갭 (low priority)

### P3.1 Rename.apply / FileRename.apply preview-only

| 항목 | 값 |
|---|---|
| **문제** | `CliLspProvider`의 `rename`/`file_rename` 핸들러가 실제 LSP `textDocument/rename`/`workspace/willRenameFiles`를 호출하지만 `apply: false` (preview-only) |
| **위치** | `oxi-cli/src/lsp/provider.rs` |
| **해결** | `apply: true` 분기 추가 → LSP rename 요청 후 DiffBackend로 파일 내용 갱신 |
| **수용 기준** | `Rename.apply`로 실제 파일 rename이 발생하고 LSP 서버의 편집이 적용됨 |

### P3.2 `/memory` 서브커맨드 스텁 구현

| 항목 | 값 |
|---|---|
| **문제** | `/memory view`, `/memory stats`, `/memory diagnose`, `/memory clear`, `/memory enqueue` 5개 서브커맨드가 static info 반환 스텁 |
| **위치** | `oxi-cli/src/tui/slash/builtin/memory.rs` |
| **해결** | 각 서브커맨드를 MemoryBackend/Mnemopi 진단 실제 호출로 교체 |
| **수용 기준** | `/memory stats`가 실제 vector DB 통계 표시, `/memory diagnose`가 FTS5/vector 인덱스 상태 표시 |

### P3.3 `workspace/willRenameFiles` 통합

| 항목 | 값 |
|---|---|
| **문제** | 설계 문서가 `willRenameFiles`를 LSP 통합의 일부로 명시하지만, 현재 `oxi-lsp`/`oxi-agent/tools/lsp.rs`에 구현이 확인되지 않음 |
| **위치** | `oxi-lsp/src/lib.rs` + `oxi-agent/src/tools/lsp.rs` |
| **해결** | `willRenameFiles` 핸들러를 `LspClient`/`LspTool`에 추가 |
| **참조** | 설계: `10-lsp-integration.md` §5 |
| **수용 기준** | LSP `workspace/willRenameFiles` 요청 전송 및 결과 처리 |

---

## P4 — 실험적/후순위 (별도 결정 필요)

| 항목 | 설계 참조 | 비고 |
|---|---|---|
| Mnemopi MCP 서버 노출 | `11-mnemopi-backend.md` §4 | `oxi-mnemopi/src/mcp.rs` (843 LOC) — 파일은 있으나 기능 검증 안 됨 |
| Snapcompact inline imaging transform hook | `09-compaction-modes.md` §4 | `ContextTransformer` trait + hook은 있는데 `/compact` 연결이 안 되어 있어 end-to-end 미검증 |
| $\\cdots$ | | |

---

## 변경 이력

| 날짜 | 변경 |
|---|---|
| 2026-07-29 | 최초 작성. Audit 결과 기준 17개 항목 등록 |
