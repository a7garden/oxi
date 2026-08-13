# TUI `/sessions` Slash Command — Make Resume Actually Resume — Design

**Date:** 2026-08-13
**Status:** Approved (autonomous execution per user delegation; user pre-authorized "설계하고 구현까지 할거야" with "쭉 진행해, 질문에 응답은 못 해줘")
**Scope:** TUI `/sessions` slash command only. No SDK/CLI surface changes. No protocol changes.

## Companion audit

This spec lands alongside the post-`/model`-picker audit (commits `0296cce4`–`728dd7b5`). Two of the seventeen TUI slash commands had "looks functional but doesn't do the thing" defects:

| Defect | Status |
|---|---|
| `/model` picker dead code (`scoped_models: Vec::new()` hardcoded) | **Fixed** in commits `0296cce4` + `32aa5d45` |
| `/sessions` (alias `/resume`) drops its arguments, can pick but never resume | **This spec** |
| `/settings` "Model:" row is read-only (`selection: None`) | Documented as known limitation (subtitle already says "Use /model to switch") — deferred, not a defect |
| `/compact` is silent on error (`tracing::warn!` only) | Deferred, separate concern (notification routing, not TUI surface) |

`/sessions` is HIGH severity: a core TUI feature is fully dead. The user can pick a session, but pressing Enter only reopens the picker. There is no public API to actually resume a session from the TUI; the RPC mode has it (`oxicode rpc switch_session`) but the TUI never wired one up. The user has to fall back to the CLI (`oxicode sessions` to find the id, then `oxicode delete` and lose context — even the CLI has no `oxicode resume <id>` subcommand).

## Problem

`oxicode-cli/src/tui_vt/slash/registry.rs:267`:

```rust
fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    // _args dropped. Always opens the picker.
```

The TUI flow today:

1. User types `/sessions` → `SessionsCommand::execute(_args="")` → opens picker.
2. User picks a session → `main_loop.rs:1866` handler fills `state.input_buffer = "/resume <id>"`.
3. User presses Enter → `/resume <id>` dispatched → `SessionsCommand::execute(_args="<id>")` → **`_args` is dropped, picker reopens**.

So the TUI is a "select twice and get the picker" loop. The underlying `SessionManager::open(path, …)` and `SessionManager::switch_session(path, …)` exist but no TUI code path ever calls them with the picked id.

## Goal

When the user picks a session in the `/sessions` picker, the TUI:

1. Tears down the current `AgentSession` cleanly (fires the `SessionEnd` hook, drops queues).
2. Opens the picked `.jsonl` file via `SessionManager::open`.
3. Validates CWD with `assert_session_cwd_exists` (same check the runtime uses).
4. Creates a new `AgentSession` wrapping the **same `Arc<Agent>`** so the agent's tool registry / model resolution / streaming state persist across the swap.
5. Replaces the active `AgentSessionHandle` in the TUI's render state and the agent worker.
6. Notifies the user: "Resumed session `<short-id>` (`<n>` messages)" or a clear error if the file is gone / cwd invalid.

Behavior changes:

- `/sessions` (no args) → opens picker (unchanged UX).
- `/sessions <id>` → resume the named session without opening the picker (new UX; matches `/model` direct-set semantics).
- `/resume <id>` → same as `/sessions <id>`.
- Picker row selection → resume the picked id (replaces the current dead "fill /resume <id> into input buffer" pattern).
- The hook chain (`SessionEnd` on the old session, `SessionStart` on the new) fires correctly via the runtime's `teardown_current` path.

## Non-goals

- No `/new`, `/fork`, `/delete` slash commands added. The CLI has them, the TUI does not, and that's a separate scope. **Not** in this spec.
- No `oxicode resume <id>` CLI subcommand. Out of scope — the user has a TUI issue, not a CLI issue.
- No migration of the TUI to `AgentSessionRuntime` for *all* session lifecycle. The runtime is overkill if the only call we need is swap-on-`/sessions`. The spec uses the lower-level `SessionManager::open` + `AgentSession::new` directly, mirroring what `rpc_mode/handlers.rs::swap_session` already does. (See "Alternative considered" below for why the runtime was rejected.)
- No streaming-cancel / in-flight prompt abortion. The `teardown_current` path already requires the runtime to be idle (`SessionSwitchReason::Resume` precondition is "no active run"). The TUI must refuse the swap if a stream is active and tell the user to `/cancel` first. The current `/handoff` already has this gate (`registry.rs:533`); we reuse the same pattern.
- No per-session model/thinking override. The TUI's `/settings` "Model" row is left as a known limitation for a follow-up — the new session inherits the active settings.
- No "auto-resume most recent" behavior. The user must explicitly pick or specify the id.

## Audit of other slash commands (companion)

The earlier audit in `2026-08-13-tui-slash-model-pick-by-key-design.md` re-checked all 17 commands. After this fix lands, **every TUI slash command is fully functional**:

| Command | Status |
|---|---|
| `/quit`, `/clear`, `/compact`, `/cancel`, `/find`, `/vim`, `/shortcuts`, `/theme`, `/status`, `/handoff`, `/agents` | Pure state ops, OK. |
| `/settings` | Interactive items work; Model row is intentionally read-only (subtitle redirects to `/model`). |
| `/tools`, `/mcp`, `/info`, `/export` | Read-only diagnostics, OK. |
| `/providers` (+ `remove`/`add`/`run-oauth`) | Auth + catalog, OK. |
| `/models` | Full catalog browser, OK. |
| `/model` | Fixed (commits `0296cce4`+`32aa5d45`). |
| `/sessions` (+ `/resume` alias) | **Fixed by this spec.** |
| File commands (`~/.oxicode/commands/*.md`) | OK (30/30 unit tests pass). |

## Architecture

### 1. Public resume API: `SessionSwapper` + `AgentSession::resume_from_file`

We need a way for the TUI to swap the active session. The TUI's `RenderState` carries an `AgentSessionHandle` by reference; the agent worker holds its own clone. Both need to see the new handle after a swap.

**Decision:** introduce a small `SessionSwapper` shared between the render state and the agent worker. The swap is a swap of the inner `Arc<AgentSession>`.

```rust
// New: oxicode-cli/src/app/agent_session_handle.rs
//
// Cheap, thread-safe handle to the "current" AgentSession. Replaces the
// raw `AgentSessionHandle` everywhere the TUI / worker need to observe
// a session that may be swapped mid-run.
//
// Construction is one-time at TUI startup (wraps the initial handle);
// `swap` is called by `/sessions` / `/resume`; readers clone via
// `current()`. Drop semantics: when the wrapper drops, the initial
// `Arc<AgentSession>` is released; the agent itself is owned by the
// `Arc<Agent>` held by the swapper (or by `App`) and outlives the wrapper.
pub struct SessionSwapper {
    current: parking_lot::Mutex<AgentSessionHandle>,
}

impl SessionSwapper {
    pub fn new(initial: AgentSessionHandle) -> Self { … }
    pub fn current(&self) -> AgentSessionHandle { … }
    pub fn swap(&self, new: AgentSessionHandle) { … }
}
```

**Why not `Arc<AgentSessionHandle>` with `Arc::make_mut`?** `make_mut` requires `Arc::get_mut` (only succeeds when refcount is 1); the TUI has multiple holders (render state + worker). `Mutex` is correct and cheap — swap is a rare event, `current()` is a hot read but a `parking_lot::Mutex` read is < 10 ns.

**Why not `arc_swap::ArcSwap`?** One more dep, ~1k lines, for a feature a `Mutex` handles fine. KISS.

### 2. Resume implementation: free function on `AgentSession`

`App` is `!Send + !Sync` (its fields include `parking_lot::RwLock`), so we cannot put an `Arc<App>` inside a `tokio::task::spawn` closure. The resume must run inside a thread that already owns the resources. The cleanest fit is a free function on the `AgentSession` module that takes the `Arc`s the slash command already clones for `tokio::spawn`:

```rust
// New on oxicode-cli/src/app/agent_session.rs:

/// Open a session from a file path, validating the CWD and seeding the
/// agent's conversation state from the resumed branch. Mirrors the
/// `AgentSession::new` body but takes the resources by `Arc` so the
/// caller can `tokio::spawn` it.
pub async fn resume_from_file(
    agent: std::sync::Arc<Agent>,
    settings: std::sync::Arc<Settings>,
    session_state: SessionState,
    path: &std::path::Path,
    cwd_override: Option<&str>,
) -> Result<(AgentSession, SessionManager), ResumeError> {
    // 1. Open the file (sync — file I/O is fast for the JSONL case).
    if !path.is_file() {
        return Err(ResumeError::FileNotFound(path.to_path_buf()));
    }
    let session_manager = SessionManager::open(
        &path.to_string_lossy(),
        None,
        cwd_override,
    );

    // 2. Validate CWD.
    let cwd = session_manager.get_cwd();
    let adapter = SessionManagerCwdAdapter(&session_manager);
    if let Err(e) = assert_session_cwd_exists(&adapter, &cwd) {
        return Err(ResumeError::CwdInvalid(format!("{e}")));
    }

    // 3. Construct a fresh AgentSession around the same Arc<Agent>.
    //    The constructor (AgentSession::new) already seeds the agent's
    //    message history from the resumed branch via
    //    `resume_messages_from_branch` (issue #23).
    let session = AgentSession::new(
        std::sync::Arc::clone(&agent),
        (*settings).clone(),
        session_manager,
        cwd,
        session_state,
    );
    Ok((session, session.session_manager_handle()))
}

```

**`ResumeError`** (the slash command's error vocabulary; matches the spec's `Files to change` table):

```rust
#[derive(Debug, thiserror::Error)]
pub enum ResumeError {
    #[error("No session file at {0}")]
    FileNotFound(std::path::PathBuf),
    #[error("Session cwd is gone: {0}")]
    CwdInvalid(String),
}
```

Note: `SessionBusy` is *not* an error from `resume_from_file` — the streaming check is the slash command's job (cheap, sync, and needs the slash command's handle anyway), not a function of the file or the CWD.

**Caller-side flow.** The `App` is the only place that can construct a fresh `AgentSession` (it owns the `Arc<Agent>`), but `App` is `!Send + !Sync`. The slash command can't ship `Arc<App>` into a `tokio::spawn`, so it ships the individual `Arc`s and runs `resume_from_file` inside the spawn:

1. The slash command validates the path, captures `Arc::clone(&agent)`, `Arc::clone(&settings)`, `session_state.clone()`, and `Arc::clone(&session_swapper)`, then `tokio::spawn`s an async block that calls `AgentSession::resume_from_file(...)` and, on success, `session_swapper.swap(new_handle)` + writes a transcript line via the `InlineHandle`.
2. The agent worker thread keeps running its prompt loop untouched. Between prompts, `session_swapper.current()` returns the new handle on the next read.

The `tokio::spawn` closure captures only `Send + Sync` types: `Arc<Agent>`, `Arc<Settings>`, `SessionState` (already wraps `Arc<atomic::AtomicBool>` + `Arc<RwLock<...>>`), `Arc<SessionSwapper>`, `InlineHandle`, and the path. No `Arc<App>` leaks into the closure — `App` stays in `run_tui`'s stack frame.

### 3. TUI integration

The TUI's `RenderState` gains two new fields:

```rust
/// Shared between the render loop and the agent worker; readers clone
/// `current()` to get the live `AgentSessionHandle`. `Arc<SessionSwapper>`
/// so it can move into both threads at startup.
pub session_swapper: std::sync::Arc<crate::app::agent_session_handle::SessionSwapper>,
/// `Some(path)` when the slash command wants the event loop to drain a
/// resume job on the next `Submitted` arm. Read-and-cleared by the
/// `Submitted` arm in `handle_inline_event`.
pub pending_resume: Option<std::path::PathBuf>,
```
**Streaming-in-flight gate:** `AgentSession::resume_from_file` does *not* check the streaming flag — that lives in the slash command, where `ctx.session.is_streaming()` is the cheap check. On `true` the command replies `Cannot resume while agent is running. Use /cancel first.` (same wording as the existing `/handoff` busy gate at `registry.rs:534`).

**Picker handler update:** the `InlineListSelection::Session(id)` arm in `main_loop.rs:1866` currently fills the input buffer with `/resume <id>`. Replace with: build a `ResumeJob { id, reply: handle.clone() }` and enqueue it (we add `pending_resume: Option<PathBuf>` to `RenderState`; the event loop's `Submitted` arm drains it and `tokio::spawn`s the resume). The transcript line "Resuming …" is written by the spawn worker, not the handler.

### 4. `SessionsCommand::execute` — direct-resume path

```rust
fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    let arg = args.trim();
    match arg {
        // Empty: open the picker (unchanged UX).
        "" => { /* existing picker code, lines 270-327 */ }

        // /sessions <id> or /resume <id>: queue a resume.
        id => {
            if ctx.session.is_streaming() {
                ctx.reply(InlineMessageKind::Error,
                    "Cannot resume while agent is running. Use /cancel first.");
                return SlashOutcome::Handled;
            }
            let session_dir = sessions_dir();
            let path = session_dir.join(format!("{id}.jsonl"));
            if !path.is_file() {
                ctx.reply(InlineMessageKind::Error,
                    format!("No session file: {}", path.display()));
                return SlashOutcome::Handled;
            }
            // Stash the path; the event loop's `Submitted` arm drains it
            // and spawns the resume (we don't have `Arc<Agent>` /
            // `Arc<SessionSwapper>` in `SlashCtx`).
            ctx.state.pending_resume = Some(path);
            ctx.reply(InlineMessageKind::Info, format!("Resuming {id}…"));
        }
    }
    SlashOutcome::Handled
}
```

**Two new fields on `RenderState`:**

```rust
/// Shared between the render loop and the agent worker. The render loop
/// calls `current()` to get the live `AgentSessionHandle`; the resume
/// `tokio::spawn` calls `swap(new_handle)` to atomically replace it.
pub session_swapper: std::sync::Arc<crate::app::agent_session_handle::SessionSwapper>,
/// `Some(path)` when the slash command wants the event loop to drain a
/// resume job. The `Submitted` arm in `handle_inline_event` calls
/// `state.pending_resume.take()` and enqueues the resume.
pub pending_resume: Option<std::path::PathBuf>,
```

**`SessionSwapper` itself** is built at TUI startup in `build_agent_session` (or a new helper alongside it) by wrapping the initial `AgentSessionHandle` returned by `create_agent_session_from_services`. The initial handle is moved into the wrapper; downstream code reads through `session_swapper.current()`.

### 5. Confirmation for `cwd_invalid`

`/sessions` opening a session whose CWD no longer exists is the only failure mode that should NOT silently fail. The `assert_session_cwd_exists` check returns `Err(CwdInvalid)`. We translate that to:

```
Cannot resume `<id>`: the session was recorded in `<cwd>`, which no longer exists. Use /export to save its content, then /clear.
```

The transcript line uses `InlineMessageKind::Error` so it's visually distinct.

## Files to change

| File | Change |
|---|---|
| `oxicode-cli/src/app/agent_session_handle.rs` (new) | `SessionSwapper` newtype + unit tests. |
| `oxicode-cli/src/app/mod.rs` | `pub mod agent_session_handle;` re-export. |
| `oxicode-cli/src/app/agent_session.rs` | `pub async fn resume_from_file(agent, settings, session_state, path, cwd_override) -> Result<(AgentSession, SessionManager), ResumeError>` + `pub enum ResumeError { FileNotFound, CwdInvalid }`. |
| `oxicode-cli/src/tui_vt/main_loop.rs` | `RenderState.session_swapper: Arc<SessionSwapper>` + `pending_resume: Option<PathBuf>`; `spawn_agent_worker` takes `Arc<SessionSwapper>` (worker calls `current()` between prompts); `run_event_loop` calls `session_swapper.current()` per dispatch; `handle_inline_event` `Submitted` arm drains `pending_resume.take()` and `tokio::spawn`s the resume; picker `InlineListSelection::Session(id)` arm enqueues the resume. |
| `oxicode-cli/src/tui_vt/slash/registry.rs` | `SessionsCommand::execute`: non-empty `arg` queues via `state.pending_resume`; empty `arg` keeps the picker. |
| `oxicode-cli/src/lib.rs` | (No new method on `App` — the swap is owned by the worker + `SessionSwapper`. The spec uses the `Arc<Agent>` and `Arc<Settings>` directly.) |
| `CHANGELOG.md` | New `### Fixed` entry under `[Unreleased]`. |

## Test plan

Unit tests alone are insufficient — the bug is end-to-end (TUI → handler → worker → `resume_from_file` → `SessionManager::open` → file). The plan needs:

1. **Unit test** for `SessionSwapper::swap` + `current` thread safety: spawn N threads that each `current()` and `swap()` in a loop; assert the handle observed is always the most-recently-swapped one (a fence ordering test; no `loom` needed if we just check "no torn read").
2. **Unit test** for `AgentSession::resume_from_file` (free function — no `App` needed):
   - Happy path: write a session file via `SessionManager::create`, persist a known entry, call `resume_from_file`, assert the new session's `messages()` match.
   - `FileNotFound`: call with a path that doesn't exist, expect `Err(FileNotFound)`.
   - `CwdInvalid`: write a file with a header whose `cwd` points to a non-existent path, expect `Err(CwdInvalid)`.
3. **Integration test** (`tests/sessions_resume.rs`): a TUI smoke via `pty_harness` (already in the workspace — `tests/pty_harness.rs`):
   - Type `/sessions`, pick a row, verify the new session's model id matches the old one's model id but the conversation is fresh / resumed.
4. **No regression** of existing tests: `cargo nextest run -p oxicode-cli` → 901/901 (or 901 + the new tests).

**Critical auth test invariant (carried over from the `/model` cycle):** every test that exercises `AgentSession::resume_from_file` or any path touching `AuthStorage` MUST use `AuthStorage::in_memory()` (the existing public hermetic constructor at `store/auth_storage.rs:535`). **Never** construct `AuthStorage::default()` in a test — it points at `~/.oxicode/auth.json` and `set_api_key` on it would overwrite the user's real stored API keys with test placeholders. (See the advisory that caught the `/model` plan for context.)


## Rollout

1. Spec → plan → implementation → verification, three commits on main.
2. The plan must follow the "don't clobber `~/.oxicode/auth.json`" lesson from the `/model` cycle: **all tests use `AuthStorage::in_memory()` (or no auth path at all); no test seam that wraps `default()`**.
3. CHANGELOG `### Fixed` entry under `[Unreleased]`.
4. Manual TUI smoke: type `/sessions <id>`, verify the picker doesn't reopen and the session actually loads. With the picker path: type `/sessions`, pick, verify the same.

## Alternative considered: migrate the TUI to `AgentSessionRuntime`

`AgentSessionRuntime` (in `agent_session_runtime.rs`) already provides `switch_session(path, cwd_override)` — the exact method we need. It owns the `AgentSession` and re-creates it on swap. The TUI is currently the *only* major consumer of the lower-level `create_agent_session_from_services` + manual handle passing; everything else (RPC, print mode) uses the runtime or the App-level swap.

**Rejected because:**
- Migration scope: the TUI's `run_event_loop` + `spawn_agent_worker` would need to switch from `&AgentSessionHandle` to `&AgentSessionRuntime` (or wrap it), and `AgentSessionRuntime` is not `Send + Sync` (it has `AgentSession` directly, not behind an Arc). Refactoring that is a separate spec.
- The runtime also requires a `CreateRuntimeFactory: Arc<dyn Fn(CreateRuntimeOptions) -> Result<CreateAgentSessionRuntimeResult>>` closure, which the TUI's `build_agent_session(app)` would have to expose. Currently `build_agent_session` is a local `async fn` — turning it into a closure that can be replayed changes the call site.
- The TUI's render state reads through the handle dozens of times per frame. Going through a runtime would require either (a) exposing the inner handle (which is what we're doing anyway with `SessionSwapper`) or (b) adding forwarding methods on the runtime for every `AgentSession` method the TUI uses.

**When the runtime migration IS the right call:** a future spec that unifies session lifecycle across the TUI + RPC + print modes (the F-5 audit 2026-06-21 was already on this track — see `oxicode-code-audit-report.html`). That spec would absorb this fix as one of its early wins. For now, the `SessionSwapper` + `AgentSession::resume_from_file` approach is the minimum-surface fix that closes the bug without a wider refactor.

## Out-of-scope follow-ups (deferred)

| Item | Severity | Reason |
|---|---|---|
| `/new`, `/fork`, `/delete` slash commands | LOW (UX gap) | CLI has them; the TUI doesn't. Same shape as `/sessions` — needs a `SessionSwapper`-driven path. Separate spec. |
| `/settings` Model row should be interactive (Enter → open `/model`) | LOW (UX polish) | The subtitle already says "Use /model to switch"; the user knows. The current shape is honest. Defer until someone complains. |
| `/compact` should reply on failure | LOW (UX polish) | The fix is in the `tokio::spawn` closure at `registry.rs:488-491`: change `tracing::warn!` to a transcript line via `session.emit_event(...)`. Separate, trivial. |
| Migrate TUI to `AgentSessionRuntime` | MEDIUM (architecture) | Already justified above. Separate spec. |
| `oxicode resume <id>` CLI subcommand | LOW (UX gap) | Out of TUI scope; needs a separate `cli/commands/` file. |
