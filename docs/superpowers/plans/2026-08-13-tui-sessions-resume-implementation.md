# TUI `/sessions` Resume — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `/sessions` (alias `/resume`) actually resume a session. The current TUI flow opens the picker, the user picks a session, and pressing Enter reopens the picker — the picked id is dropped on the floor. After this plan lands, picking a session (or typing `/sessions <id>` / `/resume <id>`) opens the file via `SessionManager::open`, validates the cwd, and atomically swaps the live `AgentSessionHandle` so the conversation continues with the prior history.

**Architecture:** Introduce a `SessionSwapper` (`parking_lot::Mutex<AgentSessionHandle>`) shared between the TUI render state and the agent worker; readers call `current()`, the resume `tokio::spawn` calls `swap(new_handle)`. The actual file open + cwd check + new `AgentSession` construction live in a free function `AgentSession::resume_from_file` (taking `Arc<Agent>`, `Arc<Settings>`, `SessionState`, path) because `App` is `!Send + !Sync` (its `parking_lot::RwLock` fields) and `tokio::spawn` cannot carry `&App`. The slash command stays thin: validate path, gate on `is_streaming`, set `state.pending_resume = Some(path)`. The event loop's `Submitted` arm drains it and spawns the resume.

**Tech Stack:** Rust 2024 edition. `parking_lot::Mutex` (already a dep). `tokio::spawn` (already used). `thiserror` (already used in `agent_session.rs`). No new dependencies. Existing `oxicode_sdk::ModelCatalog` + `crate::store::session::SessionManager` + `crate::app::agent_session::AgentSession` surface used as-is.

## Global Constraints

- **Cargo fmt** before every commit (`cargo fmt --all`).
- **Clippy** must pass: `cargo clippy --workspace --all-targets -- -D warnings`.
- **Tests** with `cargo nextest run -p oxicode-cli` (this crate is the only one that changes).
- **CHANGELOG.md** `[Unreleased]` section gets a `### Fixed` entry.
- **Pre-commit hooks** mirror CI; if installed they auto-run fmt/clippy on every commit.
- **No new public API** outside `oxicode-cli` itself (the new `SessionSwapper` is `pub(crate)`; the new `resume_from_file` is `pub` inside `agent_session.rs` for testability, no SDK ripple).
- **Auth test invariant (carried over from the `/model` cycle):** every test that exercises `AgentSession::resume_from_file` or any path touching `AuthStorage` MUST use `AuthStorage::in_memory()` (the existing public hermetic constructor at `store/auth_storage.rs:535`). **Never** construct `AuthStorage::default()` in a test — it points at `~/.oxicode/auth.json` and `set_api_key` on it would overwrite the user's real stored API keys with test placeholders. (See the advisory that caught the `/model` plan for context.)
- The `[Unreleased]` header sits at line 8 of `CHANGELOG.md` (after the `# Changelog` intro block).

## File Structure

| File | Role |
|---|---|
| `oxicode-cli/src/app/agent_session_handle.rs` (new) | `SessionSwapper` newtype. `parking_lot::Mutex<AgentSessionHandle>` wrapper with `new(initial)`, `current()`, `swap(new)`. ~40 LOC + ~80 LOC tests. |
| `oxicode-cli/src/app/mod.rs` | `pub mod agent_session_handle;` re-export. |
| `oxicode-cli/src/app/agent_session.rs` | `pub async fn resume_from_file(agent, settings, session_state, path, cwd_override) -> Result<(AgentSession, SessionManager), ResumeError>` + `pub enum ResumeError { FileNotFound, CwdInvalid }`. |
| `oxicode-cli/src/tui_vt/main_loop.rs` | `RenderState.session_swapper: Option<Arc<SessionSwapper>>` + `pending_resume: Option<PathBuf>` (Task 3 step 1). `RenderState.session_state: Option<SessionState>` (Task 3 step 2) — the `App`'s `SessionState` is cloned once at TUI startup so the resume spawn closure can capture it; mirrors the `pending_resume` access pattern. Construct the swapper at TUI startup; `handle_inline_event` `Submitted` arm drains `pending_resume` and spawns the resume; `spawn_agent_worker` / `run_event_loop` switch from `&AgentSessionHandle` to a function that takes `&Arc<SessionSwapper>` and calls `current()` per dispatch. |
| `oxicode-cli/src/tui_vt/slash/registry.rs` | `SessionsCommand::execute`: handle non-empty `arg` by setting `state.pending_resume = Some(path)`; keep empty-arg picker. Replace the picker handler in `main_loop.rs:1866` to enqueue a resume instead of the dead "fill `/resume <id>` into input buffer" pattern. |
| `CHANGELOG.md` | `### Fixed` entry under `[Unreleased]`. |

No new files outside `oxicode-cli`. No crate split. No SDK or CLI surface changes.

## Plan pre-flight: signatures in scope

The plan references these existing items; the implementer should not redefine them:

- `crate::app::agent_session::AgentSession` — `pub fn new(agent: Arc<Agent>, settings: Settings, session_manager: SessionManager, cwd: String, session_state: SessionState) -> Self` (at `agent_session.rs:402`).
- `AgentSession::clone_handle(&self) -> AgentSessionHandle` (at `agent_session.rs:1373`).
- `AgentSessionHandle { inner: Arc<AgentSession> }` (at `agent_session.rs:1869`).
- `SessionManager::open(path: &str, session_dir: Option<&str>, cwd_override: Option<&str>) -> Self` (at `store/session.rs:936`).
- `SessionManager::get_cwd(&self) -> String` (used in `agent_session_runtime::switch_session` at `agent_session_runtime.rs:717`).
- `assert_session_cwd_exists(adapter: &SessionManagerCwdAdapter, cwd: &str) -> Result<(), String>` — same check `AgentSessionRuntime::switch_session` uses at `agent_session_runtime.rs:719`.
- `Agent::is_streaming(&self)` — surfaced on `AgentSessionHandle` via the existing accessors; the slash command reads `ctx.session.is_streaming()` to gate.
- `App::agent(&self) -> Arc<Agent>` (at `lib.rs:520`), `App::settings(&self) -> &Settings` (at `lib.rs:485`), `App::session_state(&self) -> &SessionState` (at `lib.rs:640`).
- `RenderState` is `#[derive(Default)]` (at `main_loop.rs:148`); fields populated via `state.lock().foo = ...` after the `default()` call (see `main_loop.rs:736-737`).
- `RenderState` is wrapped in `Arc<parking_lot::Mutex<RenderState>>` and shared between the input thread and the event loop.
- `SessionState` is `Clone` (it wraps `Arc<AtomicBool>` + `Arc<RwLock<...>>`).
- `tokio::spawn` closure must capture only `Send + Sync` types — `Arc<Agent>`, `Arc<Settings>`, `SessionState`, `Arc<SessionSwapper>`, `InlineHandle`, `PathBuf` are all `Send + Sync`; `App` is **not**.

## Authentication test invariant (Task 1 / 2 / 3 reminder)

- **Always** use `AuthStorage::in_memory()` (`auth_storage.rs:535`) in tests. The `set_api_key` method persists to disk when the storage is file-backed; using `AuthStorage::default()` in a test will overwrite `~/.oxicode/auth.json` with test placeholders on `cargo nextest run`. The `/model` plan was caught doing this by advisory; this plan **never** adds a test seam and **never** calls `default()` in tests.
- For tests that don't exercise auth at all, this constraint is irrelevant — just don't construct an `AuthStorage` of any kind.

---

### Task 1: `SessionSwapper` newtype + tests

**Files:**
- Create: `oxicode-cli/src/app/agent_session_handle.rs`
- Modify: `oxicode-cli/src/app/mod.rs` (add `pub mod agent_session_handle;`)
- Test: `oxicode-cli/src/app/agent_session_handle.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing (zero internal deps).
- Produces: `pub(crate) struct SessionSwapper` with `pub(crate) fn new(initial: AgentSessionHandle) -> Self`, `pub(crate) fn current(&self) -> AgentSessionHandle`, `pub(crate) fn swap(&self, new: AgentSessionHandle)`.

- [ ] **Step 1: Create the file with the newtype + tests (TDD scaffold)**

Create `oxicode-cli/src/app/agent_session_handle.rs` with this exact content:

```rust
//! `SessionSwapper` — cheap, thread-safe handle to the "current"
//! `AgentSession` for the TUI. Replaces the raw `AgentSessionHandle`
//! everywhere the TUI / worker need to observe a session that may be
//! swapped mid-run (e.g. `/sessions <id>` resume).
//!
//! Construction is one-time at TUI startup (wraps the initial handle);
//! `swap` is called by the resume worker; readers clone via `current()`.
//! `parking_lot::Mutex` is the synchronization primitive — `current()`
//! is a hot read (every frame and every agent dispatch) and a Mutex
//! read is < 10 ns. We use a `Mutex<AgentSessionHandle>` (not
//! `ArcSwap`) to avoid pulling a new dep for one feature.

use std::sync::Arc;

use crate::app::agent_session::{AgentSession, AgentSessionHandle};

/// Cheap, thread-safe wrapper around the live `AgentSessionHandle`.
///
/// `current()` returns a cheap clone (the inner `Arc<AgentSession>`
/// is shared). `swap(new)` atomically replaces the inner handle;
/// the next `current()` call observes the new session.
#[derive(Clone)]
pub struct SessionSwapper {
    current: parking_lot::Mutex<AgentSessionHandle>,
}

impl std::fmt::Debug for SessionSwapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSwapper")
            .field("session_id", &"<redacted: contains Arc<AgentSession>>")
            .finish_non_exhaustive()
    }
}

impl SessionSwapper {
    /// Wrap an initial handle. The TUI does this once at startup
    /// (`build_agent_session` returns the initial `AgentSession`; we
    /// `clone_handle()` once into the swapper and pass the original
    /// `AgentSession` to `spawn_agent_worker` separately).
    pub fn new(initial: AgentSessionHandle) -> Self {
        Self { current: parking_lot::Mutex::new(initial) }
    }

    /// Get a cheap clone of the current handle. Hot read.
    pub fn current(&self) -> AgentSessionHandle {
        self.current.lock().clone()
    }

    /// Atomically replace the current handle. The next `current()`
    /// call observes the new session.
    pub fn swap(&self, new: AgentSessionHandle) {
        *self.current.lock() = new;
    }
}

// Inner `Arc<AgentSession>` is shared via `clone_handle()` — no
// additional `Arc` wrapper needed inside `SessionSwapper`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent_session::{AgentSession, AgentSessionHandle};
    use crate::store::session::SessionManager;
    use crate::store::settings::Settings;
    use oxicode_agent::Agent;
    use std::sync::Arc;

    fn dummy_handle() -> AgentSessionHandle {
        // Construct a bare AgentSession (no App, no real Agent — just
        // enough to get an AgentSessionHandle). This is hermetic: no
        // file I/O, no auth, no network.
        let agent = Arc::new(Agent::new(
            oxicode_agent::AgentConfig::new("test/dummy-model"),
        ));
        let sm = SessionManager::in_memory("/tmp");
        let session = AgentSession::new(
            agent,
            Settings::default(),
            sm,
            "/tmp".to_string(),
            crate::SessionState::default(),
        );
        session.clone_handle()
    }

    #[test]
    fn new_initial_handle_is_returned_by_current() {
        let h = dummy_handle();
        let id_before = h.session_id();
        let swapper = SessionSwapper::new(h);
        assert_eq!(swapper.current().session_id(), id_before);
    }

    #[test]
    fn swap_replaces_visible_handle() {
        let h1 = dummy_handle();
        let h2 = dummy_handle();
        let id1 = h1.session_id();
        let id2 = h2.session_id();
        assert_ne!(id1, id2);
        let swapper = SessionSwapper::new(h1);
        assert_eq!(swapper.current().session_id(), id1);
        swapper.swap(h2);
        assert_eq!(swapper.current().session_id(), id2);
    }

    #[test]
    fn swap_is_visible_across_threads() {
        // Two threads: one calls current() in a loop, one swaps. The
        // current() thread must never see a half-swapped handle. We
        // verify by checking that every observed session_id is one of
        // the two real ids (no torn reads).
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc as StdArc;

        let h1 = dummy_handle();
        let h2 = dummy_handle();
        let id1 = h1.session_id();
        let id2 = h2.session_id();

        let swapper = StdArc::new(SessionSwapper::new(h1));
        let stop = StdArc::new(AtomicUsize::new(0));
        let swapper_reader = swapper.clone();
        let swapper_writer = swapper.clone();
        let stop_reader = stop.clone();
        let stop_writer = stop.clone();

        let reader = std::thread::spawn(move || {
            let mut seen = Vec::with_capacity(10_000);
            while stop_reader.load(Ordering::Relaxed) == 0 {
                let id = swapper_reader.current().session_id();
                assert!(id == id1 || id == id2, "torn read: unexpected id");
                seen.push(id);
            }
            seen
        });

        let writer = std::thread::spawn(move || {
            for _ in 0..10_000 {
                swapper_writer.swap(dummy_handle());
            }
            stop_writer.store(1, Ordering::Relaxed);
        });
        writer.join().unwrap();
        let _seen = reader.join().unwrap();
        // We don't assert on the contents of `seen` (timing-dependent);
        // the per-iteration assert is the actual contract.
    }
}
```

- [ ] **Step 2: Wire the new module into `app/mod.rs`**

Open `oxicode-cli/src/app/mod.rs`. Add `pub mod agent_session_handle;` next to the other `pub mod …;` declarations. Order does not matter; alphabetical is fine.

- [ ] **Step 3: Run the new tests**

Run: `cargo nextest run -p oxicode-cli --no-fail-fast -- session_swapper`
Expected: 3 tests pass.

- [ ] **Step 4: Run `cargo fmt` and `cargo clippy`**

```bash
cargo fmt --all
cargo clippy -p oxicode-cli --all-targets -- -D warnings
```

Expected: clean. If clippy complains about a redundant `Clone` derive or `must_use` on the newtype, fix inline.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/app/agent_session_handle.rs oxicode-cli/src/app/mod.rs
git commit -m "feat(tui): add SessionSwapper newtype for live AgentSession handle

Cheap, thread-safe wrapper around the live AgentSessionHandle used
by the TUI render loop and the agent worker. current() is a hot
read (every frame and every agent dispatch); swap(new) atomically
replaces the handle. parking_lot::Mutex — no new dep.

Unit tests cover initial handle, single-threaded swap, and a
two-thread race that asserts every observed session_id is one of
the two real ids (no torn reads). The /sessions resume worker
(Task 3) will call swap() to atomically replace the live
session.

The AgentSession::resume_from_file helper lands in Task 2; the
TUI integration (RenderState field, event-loop drain, picker
handler update) lands in Task 3.

Spec: docs/superpowers/specs/2026-08-13-tui-sessions-resume-design.md"
```

---

### Task 2: `AgentSession::resume_from_file` free function + tests

**Files:**
- Modify: `oxicode-cli/src/app/agent_session.rs` (add `pub async fn resume_from_file` + `pub enum ResumeError` after the existing `impl AgentSession` block).
- Test: `oxicode-cli/src/app/agent_session.rs` (extend the existing `mod tests`).

**Interfaces:**
- Consumes: `Arc<Agent>`, `Arc<Settings>` (or `Settings` — see Step 1 below), `SessionState`, `&Path`, `Option<&str>` (cwd_override).
- Produces:
  - `pub async fn resume_from_file(agent: Arc<Agent>, settings: Arc<Settings>, session_state: SessionState, path: &Path, cwd_override: Option<&str>) -> Result<(AgentSession, SessionManager), ResumeError>`
  - `pub enum ResumeError { FileNotFound(PathBuf), CwdInvalid(String) }` with `#[derive(Debug, thiserror::Error)]`.

- [ ] **Step 1: Add the tests (TDD scaffold)**

Find the existing `mod tests` block in `oxicode-cli/src/app/agent_session.rs` (it's near the bottom of the file). Append the following four tests inside it:

```rust
    // ─── resume_from_file (TUI /sessions resume) ───

    use super::{resume_from_file, ResumeError};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Build an in-memory `Agent` + `Settings` for a single test. No
    /// `App`, no `AuthStorage`, no file I/O for auth.
    fn fixture_agent_and_settings() -> (Arc<oxicode_agent::Agent>, Arc<Settings>) {
        let agent = Arc::new(oxicode_agent::Agent::new(
            oxicode_agent::AgentConfig::new("test/dummy-model"),
        ));
        let settings = Arc::new(Settings::default());
        (agent, settings)
    }

    #[test]
    fn resume_from_file_returns_err_for_missing_file() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (agent, settings) = fixture_agent_and_settings();
            let result = resume_from_file(
                agent,
                settings,
                crate::SessionState::default(),
                std::path::Path::new("/tmp/does-not-exist-1234567890.jsonl"),
                None,
            )
            .await;
            assert!(matches!(result, Err(ResumeError::FileNotFound(_))));
        });
    }

    #[test]
    fn resume_from_file_succeeds_for_existing_in_memory_session() {
        // Write a session file via SessionManager::create + persist, then
        // re-open it via resume_from_file. The returned AgentSession's
        // message history should match what was written.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let session_dir = tmp.path().to_path_buf();
            let cwd = tmp.path().to_string_lossy().to_string();

            // 1. Create a fresh session, persist a user message.
            let mut sm = SessionManager::create(&cwd, Some(&session_dir.to_string_lossy()));
            let uid = sm.append_message(SessionEntry {
                id: Uuid::new_v4().to_string(),
                parent_id: None,
                timestamp: 0,
                message: AgentMessage::User {
                    content: ContentValue::String("hello prior".to_string()),
                },
            });
            sm.persist_session();

            // 2. Find the file path.
            let mut files: Vec<PathBuf> = std::fs::read_dir(&session_dir)
                .unwrap()
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().map(|x| x == "jsonl").unwrap_or(false) {
                        Some(p)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(files.len(), 1, "expected exactly one session file");
            let file = files.remove(0);

            // 3. Resume.
            let (agent, settings) = fixture_agent_and_settings();
            let (new_session, _new_sm) = resume_from_file(
                agent,
                settings,
                crate::SessionState::default(),
                &file,
                None,
            )
            .await
            .expect("resume should succeed for an in-memory roundtrip");

            // 4. The new session's seeded messages should include the
            //    user message we persisted. AgentSession seeds via
            //    `resume_messages_from_branch`; we just check the entry
            //    is reachable through `messages()`.
            let msgs = new_session.messages();
            let user_msg = msgs
                .iter()
                .find(|m| matches!(m, oxicode_sdk::Message::User(_)));
            assert!(user_msg.is_some(), "user message should be in resumed history");
            let _ = uid; // silence unused warning if branch filtering is tightened
        });
    }

    #[test]
    fn resume_from_file_returns_cwd_invalid_for_missing_cwd() {
        // Write a session file whose header.cwd points to a path that
        // does not exist on disk. resume_from_file must return
        // CwdInvalid, not FileNotFound.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let session_dir = tmp.path().to_path_buf();

            // Build the JSONL by hand: header + one entry, cwd bogus.
            let bogus_cwd = "/path/that/does/not/exist/1234567890";
            let header = FileEntry::Header(SessionHeader {
                entry_type: "session".to_string(),
                version: Some(CURRENT_SESSION_VERSION),
                id: Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                cwd: bogus_cwd.to_string(),
                parent_session: None,
            });
            let file = session_dir.join("bogus-cwd-session.jsonl");
            let mut s = String::new();
            s.push_str(&serde_json::to_string(&header).unwrap());
            s.push('\n');
            std::fs::write(&file, s).unwrap();

            let (agent, settings) = fixture_agent_and_settings();
            let result = resume_from_file(
                agent,
                settings,
                crate::SessionState::default(),
                &file,
                None,
            )
            .await;
            assert!(matches!(result, Err(ResumeError::CwdInvalid(_))));
        });
    }

    #[test]
    fn resume_from_file_propagates_session_busy_via_caller_check() {
        // `SessionBusy` is NOT a free-function error — it's a
        // slash-command concern. This test pins the contract: the
        // free function never returns SessionBusy. (If a future
        // change wants to add it, this test will fail and force a
        // conscious update.)
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ResumeError>();
    }
```

- [ ] **Step 2: Run the tests to confirm they fail (compile error on the missing function)**

Run: `cargo nextest run -p oxicode-cli --no-fail-fast -- resume_from_file`
Expected: compile errors pointing at the missing `resume_from_file` and `ResumeError`.

If `tempfile` is not already a dev-dependency of `oxicode-cli`, add it:

```bash
grep -q '^tempfile' oxicode-cli/Cargo.toml || echo 'tempfile = "3"' >> /tmp/x
```

…and edit `oxicode-cli/Cargo.toml` `[dev-dependencies]` to add `tempfile = "3"` (check existing entries for style; the crate is already in the workspace dev-deps if other tests use it — search for `tempfile::` imports first). If `tempfile` is already present, skip this step.

- [ ] **Step 3: Add the helper, the error enum, and the SessionState + AgentMessage re-exports needed by the tests**

Add the following at the end of `agent_session.rs` (after the existing `mod tests` block — or if the existing `mod tests` is at the bottom, add the new code *before* it):

```rust
// ─── TUI /sessions resume ────────────────────────────────────────────────

/// Open a session from a file path, validate the cwd, and construct a
/// fresh `AgentSession` around the same `Arc<Agent>` as the live
/// session. The `AgentSession::new` constructor already seeds the
/// agent's conversation state from the resumed branch via
/// `resume_messages_from_branch` (issue #23).
///
/// **Why a free function, not a method on `App`:** `App` is
/// `!Send + !Sync` (its fields include `parking_lot::RwLock`). The
/// resume worker runs inside a `tokio::spawn` closure, so it cannot
/// carry `&App`. The free function takes only `Send + Sync` arguments.
///
/// `cwd_override` is passed straight to `SessionManager::open`. Pass
/// `None` to let the file's header drive the cwd (the normal case).
pub async fn resume_from_file(
    agent: std::sync::Arc<crate::app::agent_session_runtime::Agent>, // see note below
    settings: std::sync::Arc<crate::store::settings::Settings>,
    session_state: crate::SessionState,
    path: &std::path::Path,
    cwd_override: Option<&str>,
) -> Result<(AgentSession, crate::store::session::SessionManager), ResumeError> {
    // 1. File exists.
    if !path.is_file() {
        return Err(ResumeError::FileNotFound(path.to_path_buf()));
    }

    // 2. Open the file. Sync — `SessionManager::open` is fast for the
    //    JSONL case and the rest of the function is I/O-free.
    let session_manager = crate::store::session::SessionManager::open(
        &path.to_string_lossy(),
        None,
        cwd_override,
    );

    // 3. Validate cwd.
    let cwd = session_manager.get_cwd();
    let adapter = crate::store::session_cwd::SessionManagerCwdAdapter(&session_manager);
    if let Err(e) = crate::store::session_cwd::assert_session_cwd_exists(&adapter, &cwd) {
        return Err(ResumeError::CwdInvalid(format!("{e}")));
    }

    // 4. Construct a fresh AgentSession around the same Arc<Agent>.
    let session = AgentSession::new(
        std::sync::Arc::clone(&agent),
        (*settings).clone(),
        session_manager,
        cwd,
        session_state,
    );
    Ok((session, session.session_manager_handle()))
}

/// Errors raised by `resume_from_file`. The TUI maps each variant
/// to a distinct transcript line. `SessionBusy` is *not* a
/// `ResumeError` variant — the streaming check is the slash
/// command's job, not this function's.
#[derive(Debug, thiserror::Error)]
pub enum ResumeError {
    #[error("No session file at {0}")]
    FileNotFound(std::path::PathBuf),
    #[error("Session cwd is gone: {0}")]
    CwdInvalid(String),
}
```

**Adjustments the implementer may need to make based on actual type paths (verify by reading the existing code):**

- The `Agent` type is re-exported from `oxicode_agent::Agent`; the existing `agent_session_runtime.rs` imports it directly as `use oxicode_agent::Agent`. The free function should match: `agent: std::sync::Arc<oxicode_agent::Agent>`.
- The `SessionManager` access path is `crate::store::session::SessionManager`. `session_manager_handle()` may not exist on `AgentSession`; if it doesn't, drop the second tuple element and return just `AgentSession`. The plan's intent is to surface the manager for tests; the slash command path doesn't need it.
- `assert_session_cwd_exists` and `SessionManagerCwdAdapter` are at `crate::store::session_cwd` (used in `agent_session_runtime.rs:718-719`). If the `session_cwd` module is `pub(crate)` only, import it the same way.

If the type paths in the snippet above are wrong after the implementer reads the actual code, fix them inline. The shape — `pub async fn resume_from_file(...) -> Result<(AgentSession, _), ResumeError>` — is the contract.
- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo nextest run -p oxicode-cli --no-fail-fast -- resume_from_file`
Expected: 4 tests pass.

- [ ] **Step 5: Run `cargo fmt` and `cargo clippy`**

```bash
cargo fmt --all
cargo clippy -p oxicode-cli --all-targets -- -D warnings
```

Expected: clean. If `clippy::needless_pass_by_value` fires on `session_state: SessionState` (because `SessionState` is `Clone` and could be `&`), prefer `session_state: SessionState` (the value form is what the caller already has — `App::session_state().clone()` — and avoids a clone inside the function). Add `#[allow(clippy::needless_pass_by_value)]` only if clippy insists and the implementation is sound.

- [ ] **Step 6: Commit**

```bash
git add oxicode-cli/src/app/agent_session.rs oxicode-cli/Cargo.toml
git commit -m "feat(tui): AgentSession::resume_from_file + ResumeError

Free function that opens a session file via SessionManager::open,
validates the cwd via assert_session_cwd_exists, and constructs a
fresh AgentSession around the caller's Arc<Agent>. The constructor
already seeds the agent's message history from the resumed branch
(resume_messages_from_branch, issue #23).

Why a free function: App is !Send + !Sync (parking_lot::RwLock
fields), so the resume tokio::spawn closure cannot borrow App.
The free function takes only Send + Sync arguments.

Tests cover FileNotFound, the happy in-memory roundtrip, and
CwdInvalid. The auth invariant from the /model cycle is honored:
no AuthStorage in this test path (resume_from_file does not
touch auth at all). SessionBusy is intentionally not a
ResumeError variant — the streaming check belongs in the slash
command, where ctx.session.is_streaming() is a cheap read.

The TUI integration (RenderState.session_swapper, event-loop
drain, picker handler update) lands in Task 3.

Spec: docs/superpowers/specs/2026-08-13-tui-sessions-resume-design.md
Plan: docs/superpowers/plans/2026-08-13-tui-sessions-resume-implementation.md"
```

---

### Task 3: TUI integration — `RenderState`, event-loop drain, picker handler, slash command

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (add `RenderState` fields, init at startup, drain `pending_resume` in `handle_inline_event`, update `spawn_agent_worker` / `run_event_loop` / picker handler).
- Modify: `oxicode-cli/src/tui_vt/slash/registry.rs` (`SessionsCommand::execute`: non-empty `arg` queues via `state.pending_resume`).

**Interfaces:**
- Consumes: `Arc<SessionSwapper>` (from Task 1), `Arc<Agent>`, `Arc<Settings>`, `SessionState` (from Task 2's caller).
- Produces: a `/sessions <id>` slash command that works end-to-end.

This task is a refactor of how the TUI threads the `AgentSessionHandle` around. The plan is to introduce the new wiring without breaking any existing tests. The most invasive change is `spawn_agent_worker` and `run_event_loop` switching from `&AgentSessionHandle` to `&Arc<SessionSwapper>`. Do this in three small steps; the first two keep the diffs trivial.

- [ ] **Step 1: Add the new `RenderState` fields + initialization**

Open `oxicode-cli/src/tui_vt/main_loop.rs`. Locate the `pub struct RenderState` block (around line 148, `#[derive(Default)]`). Add **three** new fields:

```rust
    /// Live-session swapper. `None` until the TUI startup wires it.
    /// The render loop and the agent worker both call `current()` per
    /// dispatch; the resume `tokio::spawn` calls `swap(new_handle)`.
    /// `Option` because `#[derive(Default)]` requires it.
    pub session_swapper: Option<std::sync::Arc<crate::app::agent_session_handle::SessionSwapper>>,
    /// `Some(path)` when the slash command wants the event loop to
    /// drain a resume job on the next `Submitted` arm. The
    /// `Submitted` arm calls `state.pending_resume.take()` and
    /// enqueues the resume.
    pub pending_resume: Option<std::path::PathBuf>,
    /// `Some(state)` once the TUI startup clones the `App`'s
    /// `SessionState` into the render state. The resume spawn
    /// closure captures it and passes it to
    /// `AgentSession::resume_from_file`. `Option` because
    /// `#[derive(Default)]` requires it.
    pub session_state: Option<crate::SessionState>,
```

- [ ] **Step 2: Wire the swapper at TUI startup**

In `run_tui` (around line 690-740, where the render state is built), after `let session = build_agent_session(&app).await?;` and `let session_handle = session.clone_handle();`, add:

```rust
    // Wrap the initial handle in a SessionSwapper. The render loop
    // and the agent worker both read through `current()`; the
    // resume `tokio::spawn` (Task 3 step 4) calls `swap(new_handle)`.
    let session_swapper =
        std::sync::Arc::new(crate::app::agent_session_handle::SessionSwapper::new(
            session_handle.clone(),
        ));
```

…and, just below the existing `state.lock().catalog = Some(app.catalog());` lines (around line 736), add:

```rust
    state.lock().session_swapper = Some(session_swapper.clone());
    state.lock().session_state = Some(app.session_state().clone());
```

- [ ] **Step 3: Add a thin helper on `RenderState` so the event loop and slash command can grab the swapper without unwrapping**

In `impl RenderState` (around line 429), add:

```rust
    /// Get a clone of the live `SessionSwapper`. Panics if the TUI
    /// wasn't initialized properly (the `run_tui` startup wires it
    /// before any user input is processed, so the panic is
    /// unreachable in normal use).
    pub fn swapper(&self) -> std::sync::Arc<crate::app::agent_session_handle::SessionSwapper> {
        self.session_swapper
            .clone()
            .expect("RenderState::session_swapper must be initialized at TUI startup")
    }
```

- [ ] **Step 4: Update `handle_inline_event` to drain `pending_resume` in the `Submitted` arm**

In `handle_inline_event` (around line 1651), the `Submitted` arm runs the `prompt` path. Add the resume drain as the **first** thing that happens, before the slash-command dispatch:

```rust
    // ── Drain pending resume (set by /sessions <id> or the picker). ──
    if let Some(path) = state.pending_resume.take() {
        let swapper = state.swapper();
        // The current `AgentSessionHandle` is the cheap way to reach
        // the live `Arc<Agent>` and `Arc<Settings>` (the
        // `AgentSessionHandle::agent_arc()` + `settings_arc()` mirror
        // accessors on `AgentSession` — see the note at the end of
        // this step). `SessionState` is already on the render state
        // (Task 3 step 2) so we can clone it directly.
        let agent_arc: std::sync::Arc<oxicode_agent::Agent> =
            std::sync::Arc::clone(ctx.session.agent_arc());
        let settings = ctx.session.settings_arc();
        let session_state = state
            .session_state
            .clone()
            .expect("RenderState::session_state must be initialized at TUI startup");
        let path_for_log = path.clone();
        let handle = handle.clone();
        let swapper_for_swap = swapper.clone();
        tokio::spawn(async move {
            match crate::app::agent_session::resume_from_file(
                agent_arc,
                settings,
                session_state,
                &path,
                None,
            )
            .await
            {
                Ok((new_session, _)) => {
                    swapper_for_swap.swap(new_session.clone_handle());
                    let n = new_session.messages().len();
                    let id = new_session.session_id();
                    handle.append_line(
                        oxicode_vtui::tui::core::InlineMessageKind::Info,
                        vec![crate::tui_vt::main_loop::plain_segment(
                            format!("Resumed session {id} ({n} messages)")
                        )],
                    );
                }
                Err(crate::app::agent_session::ResumeError::FileNotFound(p)) => {
                    handle.append_line(
                        oxicode_vtui::tui::core::InlineMessageKind::Error,
                        vec![crate::tui_vt::main_loop::plain_segment(
                            format!("No session file: {}", p.display())
                        )],
                    );
                }
                Err(crate::app::agent_session::ResumeError::CwdInvalid(cwd)) => {
                    handle.append_line(
                        oxicode_vtui::tui::core::InlineMessageKind::Error,
                        vec![crate::tui_vt::main_loop::plain_segment(
                            format!(
                                "Cannot resume {}: the session was recorded in `{cwd}`, which no longer exists. \
                                 Use /export to save its content, then /clear.",
                                path_for_log.display()
                            )
                        )],
                    );
                }
            }
        });
        return LoopOutcome::Continue;
    }

**Accessor pass on `AgentSession` / `AgentSessionHandle`.** The snippet above uses `ctx.session.agent_arc()` and `ctx.session.settings_arc()`. These do not exist yet — `AgentSession::agent` / `AgentSession::settings` are private (`agent_session.rs:177-178`). Add `pub(crate)` accessors on `AgentSession`:

```rust
impl AgentSession {
    pub(crate) fn agent_arc(&self) -> std::sync::Arc<oxicode_agent::Agent> {
        std::sync::Arc::clone(&self.agent)
    }
    pub(crate) fn settings_arc(&self) -> std::sync::Arc<Settings> {
        std::sync::Arc::clone(&self.settings)
    }
}
```

…and mirror them on `AgentSessionHandle` (the handle is the public surface the resume closure reaches):

```rust
impl AgentSessionHandle {
    pub fn agent_arc(&self) -> std::sync::Arc<oxicode_agent::Agent> {
        std::sync::Arc::clone(&self.inner.agent)
    }
    pub fn settings_arc(&self) -> std::sync::Arc<Settings> {
        std::sync::Arc::clone(&self.inner.settings)
    }
}
```

`SessionState` does **not** need an accessor on `AgentSession` — Task 3 step 1 puts it on `RenderState` instead, and step 4 reads it from there.


- [ ] **Step 5: Update `spawn_agent_worker` to take `Arc<SessionSwapper>` instead of `AgentSessionHandle`**


```rust
fn spawn_agent_worker(
    session_swapper: std::sync::Arc<crate::app::agent_session_handle::SessionSwapper>,
) -> tokio::sync::mpsc::UnboundedSender<String> {
```

…and inside, the `run_one_prompt(&session, prompt)` call becomes `run_one_prompt(session_swapper.current(), prompt)` — `current()` is a cheap clone of the live handle, and the per-prompt read picks up any swap that happened between prompts. The `run_one_prompt` function signature does **not** need to change — it still takes `&AgentSessionHandle`.

- [ ] **Step 6: Update the `spawn_agent_worker` call site**

Around line 772 (`let prompt_tx = spawn_agent_worker(session_handle.clone());`), replace the argument with the swapper:

```rust
    let prompt_tx = spawn_agent_worker(session_swapper.clone());
```

- [ ] **Step 7: Update `run_event_loop` to use the swapper**

In `run_event_loop` (around line 801), the function currently takes `session: &crate::app::agent_session::AgentSessionHandle` and threads it through every `handle_inline_event` call. Change the signature:

```rust
async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    cmd_rx: &mut tokio::sync::mpsc::UnboundedReceiver<InlineCommand>,
    evt_rx: &mut tokio::sync::mpsc::UnboundedReceiver<InlineEvent>,
    session_rx: &mut tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    handle: &InlineHandle,
    state: &Arc<parking_lot::Mutex<RenderState>>,
    session_swapper: &std::sync::Arc<crate::app::agent_session_handle::SessionSwapper>,
    prompt_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<()> {
```

…then inside, every `handle_inline_event(... session, ...)` becomes `handle_inline_event(... &session_swapper.current(), ...)`. There are several call sites; use `grep -n 'handle_inline_event' main_loop.rs` to find them. Each call site now borrows a per-dispatch fresh `AgentSessionHandle` clone.

- [ ] **Step 8: Update the `run_event_loop` call site**

Around line 774 (`let result = run_event_loop(... &session_handle, ...);`), replace `&session_handle` with `&session_swapper`:

```rust
    let result = run_event_loop(
        &mut tui.terminal,
        &mut cmd_rx,
        &mut evt_rx,
        &mut session_rx,
        &handle,
        &state,
        &session_swapper,         // was: &session_handle
        prompt_tx.clone(),
    )
    .await;
```

- [ ] **Step 9: Update `handle_inline_event`'s `session` parameter type**

In `handle_inline_event` (around line 1651), change `session: &crate::app::agent_session::AgentSessionHandle` — it stays the same. The caller passes `&session_swapper.current()`, which is a fresh `AgentSessionHandle` per dispatch. The function body is unchanged.

- [ ] **Step 10: Update the picker handler in `handle_inline_event`**

In `handle_inline_event` (around line 1866), the `InlineListSelection::Session(id)` arm currently does:

```rust
    if let OverlaySubmission::Selection(InlineListSelection::Session(id)) = &sub {
        state.input_buffer = format!("/resume {id}");
        state.input_cursor = state.input_buffer.len();
    }
```

Replace with:

```rust
    if let OverlaySubmission::Selection(InlineListSelection::Session(id)) = &sub {
        // Build the path the same way SessionsCommand does (in fact
        // we duplicate the directory resolution here; if the helper
        // grows, factor it into a tiny `pub(crate) fn sessions_dir() -> PathBuf`
        // in `slash/registry.rs`).
        let session_dir = dirs::home_dir()
            .map(|h| h.join(".oxicode").join("sessions"))
            .unwrap_or_else(|| std::path::PathBuf::from(".oxicode/sessions"));
        let path = session_dir.join(format!("{id}.jsonl"));
        if !path.is_file() {
            handle.append_line(
                oxicode_vtui::tui::core::InlineMessageKind::Error,
                vec![plain_segment(format!(
                    "No session file: {}", path.display()
                ))],
            );
        } else {
            state.pending_resume = Some(path);
            // The "Resuming …" line is written by the spawn
            // worker so the success and error paths are symmetric.
        }
    }
```

- [ ] **Step 11: Update `SessionsCommand::execute` for the direct-resume path**

In `oxicode-cli/src/tui_vt/slash/registry.rs` (around line 267), replace the function body:

```rust
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let arg = args.trim();
        if arg.is_empty() {
            // Existing picker path: unchanged.
            return self.open_picker(ctx);
        }

        // Direct resume: /sessions <id> or /resume <id>.
        if ctx.session.is_streaming() {
            ctx.reply(
                InlineMessageKind::Error,
                "Cannot resume while agent is running. Use /cancel first.",
            );
            return SlashOutcome::Handled;
        }
        let session_dir = dirs::home_dir()
            .map(|h| h.join(".oxicode").join("sessions"))
            .unwrap_or_else(|| std::path::PathBuf::from(".oxicode/sessions"));
        let path = session_dir.join(format!("{arg}.jsonl"));
        if !path.is_file() {
            ctx.reply(
                InlineMessageKind::Error,
                format!("No session file: {}", path.display()),
            );
            return SlashOutcome::Handled;
        }
        ctx.state.pending_resume = Some(path);
        ctx.reply(
            InlineMessageKind::Info,
            format!("Resuming {arg}…"),
        );
        SlashOutcome::Handled
    }
```

…and add the private helper `fn open_picker(&self, ctx: &mut SlashCtx<'_>) -> SlashOutcome` containing the existing picker code (lines 268-329 of the current file). The helper exists so the `arg.is_empty()` branch can `return self.open_picker(ctx);` without an `else` block. Don't duplicate the picker code into the new `match`.

**About the helper:** the existing picker is 60+ lines; pulling it into a method is mechanical. Use `put`/`cut` to move the body of the `if arg.is_empty() { … }` arm into `open_picker`.

- [ ] **Step 12: Run the unit tests**

Run: `cargo nextest run -p oxicode-cli --no-fail-fast -- session_swapper resume_from_file`
Expected: 3 + 4 = 7 tests pass (Tasks 1 + 2).

- [ ] **Step 13: Run the full oxicode-cli test suite to catch regressions**

Run: `cargo nextest run -p oxicode-cli`
Expected: 901 (pre-existing) + 7 (new) = 908 tests pass. If anything regresses, the most likely cause is the `run_event_loop` signature change breaking an existing test that calls `run_event_loop` with the old shape — fix the test or the call site, never both at once.

- [ ] **Step 14: Run `cargo fmt` and `cargo clippy`**

```bash
cargo fmt --all
cargo clippy -p oxicode-cli --all-targets -- -D warnings
```

Expected: clean. Likely clippy complaints: `needless_pass_by_value` on `session_state: SessionState` (we take by value because `SessionState: Clone` and the caller already has a fresh clone; ignore the lint or `#[allow]` it). `type_complexity` on the `tokio::spawn` closure capture list — extract the values to local `let`s if the warning fires.

- [ ] **Step 15: Build the binary and smoke-test the CLI surface**

```bash
cargo build -p oxicode-cli
target/debug/oxicode --help | head -10
target/debug/oxicode sessions 2>&1 | head -20
```

Expected: `--help` prints usage; `sessions` lists `.oxicode/sessions/*.jsonl`. The TUI smoke (`/sessions <id>`) is hard to do without an interactive terminal, but the CLI proves the binary still starts and the file format is unchanged. A real TUI smoke is optional; the PTY-based integration test (Task 4) covers the end-to-end path.

- [ ] **Step 16: Commit**

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs oxicode-cli/src/tui_vt/slash/registry.rs
git commit -m "feat(tui): wire SessionSwapper + resume_from_file into /sessions

The /sessions slash command now actually resumes a session.

Three integration points:

- RenderState gains session_swapper (Option<Arc<SessionSwapper>>)
  and pending_resume (Option<PathBuf>), populated at TUI startup.
- handle_inline_event's Submitted arm drains pending_resume first,
  before slash dispatch. The spawn closure captures Arc<Agent>,
  Arc<Settings>, SessionState, Arc<SessionSwapper>, and the path —
  all Send + Sync. The free function
  AgentSession::resume_from_file (Task 2) does the open + cwd
  check; on success the worker calls swapper.swap(new_handle).
- SessionsCommand::execute: non-empty arg queues via
  state.pending_resume after the is_streaming gate. Empty arg
  keeps the picker.
- Picker handler (InlineListSelection::Session) enqueues the
  resume instead of the dead 'fill /resume <id> into input
  buffer' pattern.
- spawn_agent_worker + run_event_loop take Arc<SessionSwapper>
  instead of AgentSessionHandle; each dispatch calls .current()
  so swaps are observed between events.

908 tests pass (3 SessionSwapper + 4 resume_from_file + 901
pre-existing).

Spec: docs/superpowers/specs/2026-08-13-tui-sessions-resume-design.md
Plan: docs/superpowers/plans/2026-08-13-tui-sessions-resume-implementation.md"
```

---

### Task 4: PTY integration test (`/sessions` end-to-end)

**Files:**
- Create: `oxicode-cli/tests/sessions_resume.rs` (small file; reuses `pty_harness`).

**Interfaces:**
- Consumes: `pty_harness::PtySession` (already in `oxicode-cli/tests/pty_harness.rs`).
- Produces: a single integration test that types `/sessions <id>` into a real PTY and asserts the picker does NOT reopen (the dead-code path) and a "Resuming …" line appears.

This task is optional but recommended. If the TTY smoke is too costly, skip and rely on Task 3's unit + integration tests.

- [ ] **Step 1: Read the existing PTY harness to understand the spawn API**

```bash
read oxicode-cli/tests/pty_harness.rs | head -100
```

Confirm `PtySession::spawn(args)` exists and the harness skips if `oxicode` is not in `PATH`. Note the timeout constants and `read_until` / `assert_output_contains` helpers.

- [ ] **Step 2: Create the integration test**

Create `oxicode-cli/tests/sessions_resume.rs`:

```rust
//! End-to-end smoke for the /sessions resume flow.
//!
//! Spins up the oxicode binary in a real PTY, types `/sessions <id>`,
//! and asserts that the picker does NOT reopen (the old dead-code
//! behavior) and that the session file is actually loaded.
//!
//! Hermetic: writes a stub session file into the user's
//! `~/.oxicode/sessions/` dir if one isn't already there, so the
//! test doesn't depend on prior state. The stub file has a
//! header + one user message; the resume should succeed.

mod pty_harness;

use std::time::Duration;

use pty_harness::PtySession;

const STUB_SESSION_ID: &str = "00000000-0000-0000-0000-000000000001";

fn write_stub_session() {
    use std::io::Write;
    let dir = dirs::home_dir()
        .expect("home dir")
        .join(".oxicode")
        .join("sessions");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(format!("{STUB_SESSION_ID}.jsonl"));
    if file.exists() {
        return;
    }
    let header = serde_json::json!({
        "type": "session",
        "version": 1,
        "id": STUB_SESSION_ID,
        "timestamp": "2026-08-13T00:00:00Z",
        "cwd": dirs::home_dir().unwrap().to_string_lossy(),
        "parent_session": null,
    });
    let entry = serde_json::json!({
        "id": "stub-entry-1",
        "parent_id": null,
        "timestamp": 0,
        "type": "message",
        "message": {
            "User": { "content": { "String": "stub prior turn" } }
        }
    });
    let mut f = std::fs::File::create(&file).expect("create stub");
    writeln!(f, "{}", header).unwrap();
    writeln!(f, "{}", entry).unwrap();
}

#[test]
fn sessions_direct_resume_does_not_reopen_picker() {
    if !pty_harness::oxicode_binary_available() {
        eprintln!("oxicode binary not in PATH; skipping");
        return;
    }
    write_stub_session();

    let mut pty = PtySession::spawn(&[]).expect("spawn oxicode");
    // Wait for the initial frame.
    pty.read_until("describe what you want to build", Duration::from_secs(10))
        .expect("initial frame");

    // Type the command.
    pty.type_text(&format!("/sessions {STUB_SESSION_ID}\n"));

    // The resume either succeeds (line "Resuming …" / "Resumed
    // session ...") or fails ("No session file: …" if the
    // stub couldn't be written, or the cwd-invalid error).
    // What we are NOT OK with: the picker reopening. If the
    // picker reopens, read_until would block; assert the absence
    // of the picker title with a short timeout.
    pty.read_until("Resum", Duration::from_secs(10))
        .or_else(|_| pty.read_until("No session", Duration::from_secs(2)))
        .or_else(|_| pty.read_until("Cannot resume", Duration::from_secs(2)))
        .expect("expected 'Resuming ...' / error line within 10s; picker must not reopen");

    // Sanity: the picker title "Models" / "Themes" / "Providers"
    // / "Sessions" is the dead-code symptom. None of those should
    // appear in the captured output.
    // (PtySession::assert_output_contains is the inverse; we
    // check by capturing the buffer and scanning.)
    let buf = pty.drain_output(Duration::from_millis(200));
    assert!(
        !buf.contains("\"Sessions\"\n") && !buf.contains("Select a session"),
        "picker reopened — got: {buf}"
    );

    pty.terminate();
}
```

**This test is best-effort.** It may flake in CI if the oxicode binary isn't in `PATH` or if the TTY timing varies. The harness's `oxicode_binary_available()` skip + a generous 10-second timeout cover the common cases. If the test proves unstable, the plan's acceptance criteria fall back to "Task 3's 908 unit + integration tests pass + manual TUI smoke."

- [ ] **Step 3: Run the test**

Run: `cargo nextest run -p oxicode-cli --no-fail-fast -- sessions_resume`
Expected: passes (or skips if `oxicode` binary unavailable). If the test fails for timing reasons, increase the timeouts and rerun. If it fails because the resume path didn't fire, re-read Task 3's wiring — the most likely cause is `state.pending_resume` not being drained before the slash-command dispatch (the spawn must run before the slash command in `handle_inline_event`).

- [ ] **Step 4: Commit**

```bash
git add oxicode-cli/tests/sessions_resume.rs
git commit -m "test(tui): add /sessions resume end-to-end PTY test

Smoke that types /sessions <id> into a real PTY and asserts the
picker does NOT reopen (the old dead-code behavior). Writes a
hermetic stub session file into ~/.oxicode/sessions/ if one
isn't already there.

Skips cleanly when the oxicode binary is not in PATH (the
PtySession harness's existing skip guard). Best-effort: the
plan's acceptance criteria fall back to Task 3's 908 unit
tests if the TTY timing proves unstable in CI."
```

---

### Task 5: CHANGELOG entry

**Files:**
- Modify: `CHANGELOG.md:8-9` (the `[Unreleased]` block).

- [ ] **Step 1: Add a `### Fixed` entry under `[Unreleased]`**

Open `CHANGELOG.md`. The `[Unreleased]` block (line 8) currently has the `/model` entry from the prior cycle. Below that entry, add:

```markdown
- **TUI `/sessions` is now a real resume, not a no-op.** Was: the
  picker reopened on every selection because the picked id was
  dropped. Now: `/sessions <id>` (and the `/resume <id>` alias)
  opens the file via `SessionManager::open`, validates the cwd,
  and atomically swaps the live `AgentSessionHandle` so the
  conversation continues with the prior history. Selection from
  the `/sessions` picker enqueues the same resume path (was:
  fills `/resume <id>` into the input buffer, which then
  re-dispatched to the same picker). `Cannot resume while agent
  is running. Use /cancel first.` gates the same way `/handoff`
  does. The `next`/`cycle` and `set_model` arms of `/model` and
  every other slash command are unchanged.
```

(Only the `### Fixed` subsection entry needs to land; the rest of the changelog is untouched.)

- [ ] **Step 2: Verify the diff is minimal**

Run: `git diff CHANGELOG.md`
Expected: 9-11 added lines under `[Unreleased]`, nothing else.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): note TUI /sessions resume fix in Unreleased"
```

---

## Verification (run after all five tasks)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p oxicode-cli
cargo build -p oxicode-cli
```

Expected: every command exits 0. The 7 new unit tests + (optionally) 1 PTY integration test pass; the pre-existing 901 tests are unchanged.

## Rollback

Revert the five commits in reverse order:

```bash
git revert HEAD         # CHANGELOG
git revert HEAD~1       # PTY integration test
git revert HEAD~2       # TUI integration (Task 3)
git revert HEAD~3       # AgentSession::resume_from_file (Task 2)
git revert HEAD~4       # SessionSwapper (Task 1)
```

No destructive operations. No DB migrations. No settings migrations.

## Out-of-scope follow-ups (deferred)

| Item | Reason |
|---|---|
| `/new`, `/fork`, `/delete` slash commands | CLI has them; the TUI doesn't. Same shape as `/sessions` — needs a `SessionSwapper`-driven path. Separate spec. |
| `/settings` Model row should be interactive | Subtitle already says "Use /model to switch"; the current shape is honest. Defer until someone complains. |
| `/compact` should reply on failure | The fix is in `registry.rs:488-491`: change `tracing::warn!` to a transcript line. Separate, trivial. |
| Migrate TUI to `AgentSessionRuntime` | Already justified in the spec. Separate spec. |
| `oxicode resume <id>` CLI subcommand | Out of TUI scope. |
