# Mechanical Items — Just Implement

> **Tier 3 — No design needed.** Copy existing patterns.
> P2.1 (Settings flags) + P3.2 (/memory stubs) + P4 (experimental notes).

---

## P2.1: Settings Feature Flags

**파일:** `oxi-cli/src/store/settings.rs`

**패턴:** `memory_enabled` (line 241)을 그대로 복사:

```rust
// 기존: pub memory_enabled: bool,  // default false

// 추가할 5개 필드 — 적절한 위치에 삽입
#[serde(default = "default_true")]
pub todo_panel_enabled: bool,

#[serde(default = "default_true")]
pub agent_hub_enabled: bool,

#[serde(default = "default_false")]
pub snapcompact_enabled: bool,

#[serde(default = "default_true")]
pub mermaid_render_enabled: bool,

#[serde(default = "default_false")]
pub commit_tool_enabled: bool,
```

**기본값 함수** (기존 `default_false` / `default_true` 재사용):
```rust
// default_true는 이미 line 2136 근처에 존재
// default_false도 이미 존재
```

**게이트 포인트 (각 flag가 false일 때 무효화할 위치):**

| Flag | Gate 위치 | 동작 |
|---|---|---|
| `todo_panel_enabled` | `tape_render.rs:47` | badge만 표시, TodoPanel 위젯 생략 |
| `agent_hub_enabled` | `handlers.rs:553` + `agents.rs:execute` | `Ctrl+h`/`/agents` 무시 |
| `snapcompact_enabled` | `tools_commands.rs:CompactCommand` | `/compact snapcompact` 에러 |
| `mermaid_render_enabled` | `markdown.rs:419` | mermaid 감지 시 syntax highlight fallback |
| `commit_tool_enabled` | (이미 bootstrap.rs에서 role 기반으로 동작 — 이 플래그는 CLI/slash 게이트) | `/commit` slash에서 확인 |

**설정 overlay 노출:** `/settings` 화면에서 bool 토글 위젯으로 표시 (기존 패턴 참조).

---

## P3.2: /memory Subcommand Stubs

**파일:** `oxi-cli/src/tui/slash/builtin/memory.rs`

**현재 스텁 5개:**

```rust
// 현재: 각각 static string 반환
"view" => notify("📝 View: use /memory stats or /memory diagnose for details"),
"stats" => notify("📊 Stats: use /memory diagnose for details"),
"diagnose" => notify("🔍 Diagnose: not yet implemented"),
"clear" => notify("🗑️ Clear: not yet implemented"),
"enqueue" => notify("📨 Enqueue: not yet implemented"),
```

**구현 방향:**

| 서브커맨드 | 구현 |
|---|---|
| `view` | `memory_summary.md` 파일을 읽어 content 표시 (`read_path_block` 재사용) |
| `stats` | `MemoryBackend`에 `list()` 호출 → entry count, type별 분류, last consolidated time |
| `diagnose` | `oxi-mnemopi`의 `DiagnosticsReporter` 또는 `db.integrity_check()` 호출 |
| `clear` | 확인 메시지 후 `MemoryBackend.clear()` 호출 + consolidation trigger |
| `enqueue` | `start_memory_pipeline`의 join handle에 signal → 즉시 consolidation 실행 |

**각 구현은 3-10줄.** `MemoryBackend` trait의 기존 메서드 사용.

---

## P4: Experimental Items Notes

| 항목 | 상태 | 권장 |
|---|---|---|
| Mnemopi MCP 서버 노출 | `oxi-mnemopi/src/mcp.rs` (843 LOC) — 파일 존재, 기능 검증 안 됨 | 다음 PR에서 E2E 테스트 |
| Snapcompact inline imaging | `ContextTransformer` trait 존재. `/compact snapcompact` 완료 후 E2E 검증 | P1.1 완료 후 자동 해결 |
