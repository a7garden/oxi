# SDK Issue Exposure — Design

Date: 2026-08-29
Status: Approved (user: "가장 우아한 방법으로 알아서 쭉 해")
Approach: **A — todo-pattern** (concrete move into `oxicode-agent`, SDK re-exports)

## Motivation

The issue feature (local `.oxicode/issues/` store + `issue` agent tool + `/issue`
TUI panel) shipped in 2026-08-27 entirely inside `oxicode-cli`. SDK consumers
(oxios, sister repo; headless agents) cannot use it: the store, the CAS layer,
liveness/ownership, and the agent tool are CLI-private. The todo feature
already provides the canonical pattern for this exposure.

## Goal

A pure-`oxicode-sdk` consumer can create an agent with a working `issue` tool
and manage project-local issues without depending on `oxicode-cli` (and ideally
without a direct `oxicode-agent` dep), following the single-dependency pattern.

## Non-Goals

- Port-trait abstraction over the store backend (`ports/issue.rs`). No second
  backend exists; a trait can be extracted later without breaking the API (the
  concrete types remain re-exportable).
- Moving the `/issue` TUI panel — TUI is the CLI's job (`tui_vt/issues_panel/`
  stays), exactly like the todo sticky panel.
- Any schema or on-disk format change (`IssueMeta`/`Issue`/`IssuePatch`,
  JSONL-per-issue layout, flock liveness files) — byte-for-byte compatible.

## Moves

| From (oxicode-cli) | To | Notes |
|---|---|---|
| `src/store/issues/{types,error,filter,serialize,liveness,store,mod}.rs` (2,041 LOC) | `oxicode-agent/src/issues/` | `git mv`; fix `crate::store::issues::` → `crate::issues::` |
| `src/tools/issue_tool.rs` (646 LOC) | `oxicode-agent/src/tools/issue.rs` | `git mv`; fix imports; `pub mod issue;` in `tools.rs` |

CLI-internal coupling measured: exactly one helper — `store::fs_util::atomic_write`
(store.rs L11, 2 call sites). Absorbed as a private helper in `oxicode-agent/src/issues/`
using the same temp+rename semantics (`<path>.tmp.<pid>.<uuid-simple>`,
best-effort temp cleanup on rename failure). `uuid` v4 and `tempfile` are
already deps of `oxicode-agent`.

## SDK Surface (oxicode-sdk/src/lib.rs)

Adjacent to the existing todo re-exports (L288–290):

```rust
// ── Issue tool + store (agent-scoped, observable by SDK consumers) ──
#[oxicode_stable(since = "0.77.0")]
pub use oxicode_agent::issues::{
    FileIssueStore, Issue, IssueError, IssueFilter, IssueMeta, IssuePatch,
    Priority, Status, liveness,
};
#[oxicode_stable(since = "0.77.0")]
pub use oxicode_agent::tools::issue::{IssueTool, cas_retry};
#[oxicode_stable(since = "0.77.0")]
pub use oxicode_agent::issues::{format_issue_full, format_issue_line};
```

(`format_issue_*` move with the tool since both CLI surface and consumers need
them; `pub use` in `oxicode-agent` lib.rs as needed.) Precise export list is
validated against every CLI callsite during migration.

## Registration Contract (unchanged shape, todo-consistent)

The `issue` tool is **not** auto-registered by `with_builtins_cwd`: the tool and
the TUI panel must share ONE `Arc<FileIssueStore>` instance (single flock
holder, coherent ownership), so registration stays a composition-root decision:

- CLI (`lib.rs::build_app`): unchanged — constructs the store and registers
  `IssueTool::new(store)`.
- SDK consumers: `IssueTool::new(Arc::new(FileIssueStore::open_from_cwd(&cwd)?))`
  registered via their tool wiring; ownership identity via
  `AgentConfig.session_id` (already threaded to `ToolContext.session_id`).

## CLI Migration (clean cutover, no aliases)

`oxicode-cli/src/store/issues/` and `src/tools/issue_tool.rs` are deleted.
Every CLI reference migrates to `oxicode_sdk::` paths (CLI already depends on
the SDK):

- `lib.rs` — `FileIssueStore`, `liveness::{AliveGuard, TUI_OWNERSHIP_ID, acquire}`
- `bootstrap.rs` — `TUI_OWNERSHIP_ID`
- `tui_vt/issues_panel/*` — types + `cas_retry` + `liveness`
- `tui_vt/main_loop.rs` — `FileIssueStore`, `TUI_OWNERSHIP_ID`
- `cli/commands/issue.rs` — `IssueFilter`/`Priority`/`Status`/`issues_dir`
- `tools/mod.rs` — `pub mod issue_tool;` removed

## Compatibility Guarantees

1. On-disk format, `.oxicode/issues/` layout, flock protocol: unchanged.
2. `oxicode issue …` CLI subcommands: unchanged behavior.
3. TUI panel: unchanged behavior; only import paths change.
4. Issue-tool JSON schema and actions: unchanged.

## Testing

- All existing store/tool/panel tests move or re-point verbatim (store unit
  tests travel with the store into `oxicode-agent`; panel tests stay in CLI).
- New: SDK-surface smoke test asserting the re-exports resolve
  (`oxicode-sdk/tests/issue_surface.rs` — construct `FileIssueStore` in a temp
  dir, create/list/update via `IssueTool`-level ops).
- Gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo nextest run --workspace`,
  `cargo build -p oxicode-agent --features native-browser`.

## Risks

- `pub` visibility gaps when moving (items reachable only via CLI-internal
  paths) — resolved case-by-case during migration; `cargo check` drives it.
- API-stability lint requires `#[oxicode_stable]` discipline on new exports.
- `oxicode-agent` gains no new external deps (uuid/tempfile already present).
