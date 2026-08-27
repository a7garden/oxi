# TUI Issues Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a full-screen TUI panel (`/issue` slash command) that provides complete CRUD (list/filter/detail/create/edit/close/reopen) over the existing `.oxicode/issues/` backend, with zero schema changes.

**Architecture:** A new `oxicode-cli/src/tui_vt/issues_panel/` module owns panel state, rendering, and key handling. Read-only store calls (`list`/`read`/`create`) run synchronously on the input thread. CAS-guarded mutations (`apply_patch`/`close`/`reopen`) are requested via a new `IssueActionRequest` enum sent over a dedicated `tokio::sync::mpsc::UnboundedSender<IssueActionRequest>` channel (kept entirely inside `oxicode-cli` — `oxicode-vtui`'s `InlineEvent` enum is NOT touched, preserving its zero-`oxicode-*`-dependency invariant) into `run_event_loop`'s `select!`, which `tokio::spawn`s the async store call and writes the result back into `RenderState` under `state.lock()`.

**Tech Stack:** Rust, tokio, parking_lot, ratatui (via oxicode-vtui), `oxicode_textarea::TextArea`, existing `crate::store::issues::*` backend (unchanged).

## Global Constraints

- Design doc: `docs/superpowers/specs/2026-08-27-tui-issues-panel-design.md` — every task below implements one section of it.
- No schema changes to `IssueMeta`/`Issue`/`IssuePatch`/`Status`/`Priority` (design §1 non-goals).
- No new dedicated keybinding — entry is `/issue` slash command only (design §3).
- `oxicode-vtui` and `oxicode-textarea` get zero source changes — reuse only (design header).
- `cargo fmt --all -- --check`, `cargo clippy -p oxicode-cli --all-targets -- -D warnings`, `cargo nextest run -p oxicode-cli` must pass at the end of every task.
- `parking_lot::MutexGuard` is `!Send` — never hold `state.lock()` across an `.await` (AGENTS.md pitfall).

---

### Task 1: `IssueActionRequest` channel + panel state skeleton

**Files:**
- Create: `oxicode-cli/src/tui_vt/issues_panel/mod.rs`
- Modify: `oxicode-cli/src/tui_vt/mod.rs` (add `pub(crate) mod issues_panel;`)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (add `RenderState` field, channel creation, `select!` arm)
- Modify: `oxicode-cli/src/tools/issue_tool.rs:397` (`async fn cas_retry` → `pub(crate) async fn cas_retry`)
- Test: `oxicode-cli/src/tui_vt/issues_panel/mod.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `pub(crate) struct IssuesPanelState { pub mode: IssuesPanelMode, pub status_filter: crate::store::issues::Status, pub extra_filter: crate::store::issues::IssueFilter, pub rows: Vec<IssueRow>, pub selected: usize, pub pending: bool, pub error: Option<String> }`
- Produces: `pub(crate) enum IssuesPanelMode { List, Detail { id: u32, scroll: usize }, Form(IssueFormState), FilterInput(String) }` (variants filled in by later tasks; `Form`/`Detail` payload types stubbed with real fields now, no `TODO`s)
- Produces: `pub(crate) struct IssueRow { pub id: u32, pub title: String, pub status: crate::store::issues::Status, pub priority: crate::store::issues::Priority, pub labels: Vec<String>, pub assignee_badge: Option<AssigneeBadge> }`
- Produces: `pub(crate) enum AssigneeBadge { Live(String), Stale(String) }`
- Produces: `pub(crate) enum IssueActionRequest { Close { id: u32, caller: String, hash: Option<String> }, Reopen { id: u32, hash: Option<String> }, ApplyPatch { id: u32, patch: crate::store::issues::IssuePatch, caller: Option<String>, hash: Option<String> } }`
- Produces: `crate::tools::issue_tool::cas_retry` now visible as `pub(crate)` for reuse by the panel dispatcher (Task 9).
- Consumes (from existing code): `RenderState` struct (`main_loop.rs:287`), `spawn_input_thread` signature (`main_loop.rs:3199`), `run_event_loop`'s `select!` (`main_loop.rs:~1280-1345`).

- [ ] **Step 1: Define the panel state types**

```rust
// oxicode-cli/src/tui_vt/issues_panel/mod.rs
//! State, rendering, and input handling for the `/issue` TUI panel.
//! See docs/superpowers/specs/2026-08-27-tui-issues-panel-design.md.

use crate::store::issues::{IssueFilter, IssueMeta, IssuePatch, Priority, Status};

#[derive(Clone, Debug, Default)]
pub(crate) struct IssuesPanelState {
    pub mode: IssuesPanelMode,
    pub status_filter: Status,
    pub extra_filter: IssueFilter,
    pub rows: Vec<IssueRow>,
    pub selected: usize,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum IssuesPanelMode {
    #[default]
    List,
    Detail {
        id: u32,
        scroll: usize,
    },
    Form(IssueFormState),
    FilterInput(String),
}

#[derive(Clone, Debug)]
pub(crate) struct IssueRow {
    pub id: u32,
    pub title: String,
    pub status: Status,
    pub priority: Priority,
    pub labels: Vec<String>,
    pub assignee_badge: Option<AssigneeBadge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AssigneeBadge {
    Live(String),
    Stale(String),
}

#[derive(Clone, Debug)]
pub(crate) struct IssueFormState {
    pub editing_id: Option<u32>,
    pub content_hash: Option<String>,
    pub title: String,
    pub priority: Priority,
    pub labels_input: String,
    pub body: oxicode_textarea::TextArea,
    pub focus: FormField,
}

impl Default for IssueFormState {
    fn default() -> Self {
        Self {
            editing_id: None,
            content_hash: None,
            title: String::new(),
            priority: Priority::default(),
            labels_input: String::new(),
            body: oxicode_textarea::TextArea::new(),
            focus: FormField::Title,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum FormField {
    #[default]
    Title,
    Priority,
    Labels,
    Body,
}

/// Requests the panel's synchronous key-handling code cannot satisfy itself
/// (CAS-guarded async store writes). Sent over a dedicated channel into
/// `run_event_loop`'s `select!` — kept out of `oxicode_vtui::InlineEvent` so
/// the framework crate stays free of `oxicode-*` dependencies.
#[derive(Clone, Debug)]
pub(crate) enum IssueActionRequest {
    Close {
        id: u32,
        caller: String,
        hash: Option<String>,
    },
    Reopen {
        id: u32,
        hash: Option<String>,
    },
    ApplyPatch {
        id: u32,
        patch: IssuePatch,
        caller: Option<String>,
        hash: Option<String>,
    },
}
```

- [ ] **Step 2: Write a test for `IssuesPanelState::default()`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_opens_in_list_mode_with_open_filter() {
        let state = IssuesPanelState::default();
        assert!(matches!(state.mode, IssuesPanelMode::List));
        assert_eq!(state.status_filter, Status::Open);
        assert!(!state.pending);
        assert!(state.error.is_none());
        assert!(state.rows.is_empty());
    }
}
```

- [ ] **Step 3: Run test to verify it fails to compile (module not wired yet)**

Run: `cargo test -p oxicode-cli issues_panel::tests::default_state_opens_in_list_mode_with_open_filter`
Expected: FAIL — `issues_panel` module not found.

- [ ] **Step 4: Wire the module and fix the failing pieces**

In `oxicode-cli/src/tui_vt/mod.rs`, add near the other `pub(crate) mod` declarations:

```rust
pub(crate) mod issues_panel;
```

In `oxicode-cli/src/tui_vt/main_loop.rs`, add a field to `RenderState` (near `pub queue_panel_open: bool,` at line ~372):

```rust
    /// `/issue` panel state. `None` = panel closed.
    pub issues_panel: Option<crate::tui_vt::issues_panel::IssuesPanelState>,
```

and initialize it in the `impl Default for RenderState` block (line ~456) — since `Option<T>` derives `Default` as `None`, no explicit initializer needed if `RenderState` uses `#[derive(Default)]`; confirm by checking the struct's derive attribute and add `issues_panel: None,` explicitly only if the `Default` impl is hand-written field-by-field (it is, per `main_loop.rs:456-459` using `Self { ... }`) — add the field there.

In `oxicode-cli/src/tools/issue_tool.rs:397`, change:

```rust
async fn cas_retry<T, F, Fut>(
```

to:

```rust
pub(crate) async fn cas_retry<T, F, Fut>(
```

and confirm `FileIssueStore` is already `pub(crate)`-visible from `tui_vt` (it's `pub` on the `crate::store::issues` module already — check `oxicode-cli/src/store/issues/mod.rs:58-59` re-exports `FileIssueStore`).

- [ ] **Step 5: Add the dedicated action channel to the TUI startup path**

Find where `spawn_input_thread` is called (`main_loop.rs:1118`, inside the function that builds `evt_tx`/`prompt_queue`). Add just above it:

```rust
    let (issue_action_tx, mut issue_action_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::tui_vt::issues_panel::IssueActionRequest>();
```

Add `issue_action_tx.clone()` as a new parameter to `spawn_input_thread` (signature at `main_loop.rs:3199`):

```rust
fn spawn_input_thread(
    state: Arc<parking_lot::Mutex<RenderState>>,
    evt_tx: tokio::sync::mpsc::UnboundedSender<InlineEvent>,
    mode_handle: Option<std::sync::Arc<std::sync::atomic::AtomicU8>>,
    prompt_queue: Arc<PromptQueue>,
    issue_action_tx: tokio::sync::mpsc::UnboundedSender<crate::tui_vt::issues_panel::IssueActionRequest>,
) {
```

update its call site to pass `issue_action_tx.clone()`, and add a `select!` arm in `run_event_loop`'s main loop (next to the `Some(ev) = evt_rx.recv()` /agent/tick arms around `main_loop.rs:1280-1345`):

```rust
            Some(req) = issue_action_rx.recv() => {
                crate::tui_vt::issues_panel::dispatch_action(req, state.clone());
            }
```

(`dispatch_action` is implemented in Task 9 — for this task, define a temporary real implementation that only sets `pending = false` and logs, so the crate compiles; Task 9 replaces the body with the actual `tokio::spawn` + store call.)

```rust
// oxicode-cli/src/tui_vt/issues_panel/mod.rs — temporary until Task 9
pub(crate) fn dispatch_action(
    req: IssueActionRequest,
    state: std::sync::Arc<parking_lot::Mutex<crate::tui_vt::main_loop::RenderState>>,
) {
    tracing::debug!(?req, "issue action received (dispatch not yet implemented)");
    if let Some(panel) = state.lock().issues_panel.as_mut() {
        panel.pending = false;
    }
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p oxicode-cli issues_panel::tests::default_state_opens_in_list_mode_with_open_filter`
Expected: PASS

- [ ] **Step 7: Full workspace gate**

Run: `cargo fmt --all -- --check && cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo nextest run -p oxicode-cli`
Expected: all pass (no other code paths touched yet).

- [ ] **Step 8: Commit**

```bash
git add oxicode-cli/src/tui_vt/issues_panel/mod.rs oxicode-cli/src/tui_vt/mod.rs oxicode-cli/src/tui_vt/main_loop.rs oxicode-cli/src/tools/issue_tool.rs
git commit -m "feat(tui): scaffold issues panel state and action channel"
```

---

### Task 2: `parse_issue_filter`

**Files:**
- Create: `oxicode-cli/src/tui_vt/issues_panel/filter_parse.rs`
- Modify: `oxicode-cli/src/tui_vt/issues_panel/mod.rs` (add `mod filter_parse; pub(crate) use filter_parse::parse_issue_filter;`)
- Test: inline `#[cfg(test)]` in `filter_parse.rs`

**Interfaces:**
- Consumes: `crate::store::issues::{IssueFilter, Priority, Status}` (unchanged).
- Produces: `pub(crate) fn parse_issue_filter(input: &str, status_filter: Status) -> IssueFilter` — later tasks (List/FilterInput rendering) call this on `Enter`.

- [ ] **Step 1: Write failing tests**

```rust
// oxicode-cli/src/tui_vt/issues_panel/filter_parse.rs
//! Parses the `/` filter-modal free-text buffer into an `IssueFilter`.
//! Syntax: space-separated `key=value` tokens (`priority=`, `label=`); any
//! remaining unrecognized tokens are joined back into `text` (title substring
//! match). Unknown `priority=` values are ignored (filter falls back to no
//! priority constraint) rather than erroring — this is a live-typing buffer.

use crate::store::issues::{IssueFilter, Priority, Status};

pub(crate) fn parse_issue_filter(input: &str, status_filter: Status) -> IssueFilter {
    let mut priority = None;
    let mut label = None;
    let mut text_tokens = Vec::new();

    for token in input.split_whitespace() {
        if let Some(v) = token.strip_prefix("priority=") {
            priority = parse_priority(v);
        } else if let Some(v) = token.strip_prefix("label=") {
            label = Some(v.to_string());
        } else {
            text_tokens.push(token);
        }
    }

    IssueFilter {
        status: Some(status_filter),
        priority,
        label,
        assigned_to_session: None,
        text: if text_tokens.is_empty() {
            None
        } else {
            Some(text_tokens.join(" "))
        },
    }
}

fn parse_priority(v: &str) -> Option<Priority> {
    match v.to_ascii_lowercase().as_str() {
        "low" => Some(Priority::Low),
        "medium" | "med" => Some(Priority::Medium),
        "high" => Some(Priority::High),
        "critical" | "crit" => Some(Priority::Critical),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_keeps_only_status() {
        let f = parse_issue_filter("", Status::Open);
        assert_eq!(f.status, Some(Status::Open));
        assert!(f.priority.is_none());
        assert!(f.label.is_none());
        assert!(f.text.is_none());
    }

    #[test]
    fn parses_priority_and_label() {
        let f = parse_issue_filter("priority=critical label=auth", Status::Open);
        assert_eq!(f.priority, Some(Priority::Critical));
        assert_eq!(f.label.as_deref(), Some("auth"));
        assert!(f.text.is_none());
    }

    #[test]
    fn unrecognized_priority_value_is_ignored() {
        let f = parse_issue_filter("priority=urgent", Status::Open);
        assert!(f.priority.is_none());
    }

    #[test]
    fn leftover_tokens_become_text_filter() {
        let f = parse_issue_filter("priority=high login bug", Status::All_placeholder_unused(), );
        // placeholder removed below in Step 4 — replaced with Status::Open
    }

    #[test]
    fn leftover_tokens_join_into_text() {
        let f = parse_issue_filter("priority=high login bug", Status::Open);
        assert_eq!(f.priority, Some(Priority::High));
        assert_eq!(f.text.as_deref(), Some("login bug"));
    }
}
```

- [ ] **Step 2: Remove the invalid placeholder test**

The `leftover_tokens_become_text_filter` test above references a nonexistent `Status::All_placeholder_unused()` — delete that entire test function, keeping only `leftover_tokens_join_into_text`. (Left in Step 1 only to make the intent obvious; the final file must not contain it.) Final test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_keeps_only_status() {
        let f = parse_issue_filter("", Status::Open);
        assert_eq!(f.status, Some(Status::Open));
        assert!(f.priority.is_none());
        assert!(f.label.is_none());
        assert!(f.text.is_none());
    }

    #[test]
    fn parses_priority_and_label() {
        let f = parse_issue_filter("priority=critical label=auth", Status::Open);
        assert_eq!(f.priority, Some(Priority::Critical));
        assert_eq!(f.label.as_deref(), Some("auth"));
        assert!(f.text.is_none());
    }

    #[test]
    fn unrecognized_priority_value_is_ignored() {
        let f = parse_issue_filter("priority=urgent", Status::Open);
        assert!(f.priority.is_none());
    }

    #[test]
    fn leftover_tokens_join_into_text() {
        let f = parse_issue_filter("priority=high login bug", Status::Open);
        assert_eq!(f.priority, Some(Priority::High));
        assert_eq!(f.text.as_deref(), Some("login bug"));
    }
}
```

- [ ] **Step 3: Wire the module**

```rust
// oxicode-cli/src/tui_vt/issues_panel/mod.rs — add near the top
mod filter_parse;
pub(crate) use filter_parse::parse_issue_filter;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p oxicode-cli issues_panel::filter_parse::tests::`
Expected: all 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/issues_panel/filter_parse.rs oxicode-cli/src/tui_vt/issues_panel/mod.rs
git commit -m "feat(tui): add issue filter-modal text parser"
```

---

### Task 3: `refresh()` — list fetch + liveness badges

**Files:**
- Modify: `oxicode-cli/src/tui_vt/issues_panel/mod.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `crate::store::issues::{FileIssueStore, IssueFilter}`, `crate::store::issues::liveness::is_session_alive`.
- Produces: `impl IssuesPanelState { pub fn refresh(&mut self, store: &FileIssueStore, issues_dir: &std::path::Path) }` — called on panel open, after `f` toggle, after filter apply, after any mutation completes.

- [ ] **Step 1: Write failing test using a tempdir-backed store**

```rust
#[cfg(test)]
mod refresh_tests {
    use super::*;
    use crate::store::issues::{FileIssueStore, Priority};

    fn tmp_store() -> (tempfile::TempDir, FileIssueStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileIssueStore::open(tmp.path().to_path_buf()).unwrap();
        (tmp, store)
    }

    #[test]
    fn refresh_populates_rows_sorted_by_recency() {
        let (tmp, store) = tmp_store();
        store
            .create("first".into(), "body".into(), Priority::Low, vec![], None)
            .unwrap();
        store
            .create("second".into(), "body".into(), Priority::High, vec![], None)
            .unwrap();

        let mut panel = IssuesPanelState::default();
        panel.refresh(&store, tmp.path());

        assert_eq!(panel.rows.len(), 2);
        // FileIssueStore::list sorts by updated_at desc — "second" was
        // created after "first" so it sorts first.
        assert_eq!(panel.rows[0].title, "second");
        assert_eq!(panel.rows[1].title, "first");
    }

    #[test]
    fn refresh_marks_unassigned_issues_with_no_badge() {
        let (tmp, store) = tmp_store();
        store
            .create("solo".into(), "body".into(), Priority::Medium, vec![], None)
            .unwrap();
        let mut panel = IssuesPanelState::default();
        panel.refresh(&store, tmp.path());
        assert!(panel.rows[0].assignee_badge.is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oxicode-cli issues_panel::refresh_tests`
Expected: FAIL — `refresh` method not defined.

- [ ] **Step 3: Implement `refresh`**

```rust
// oxicode-cli/src/tui_vt/issues_panel/mod.rs
use std::path::Path;

use crate::store::issues::{FileIssueStore, liveness};

impl IssuesPanelState {
    pub fn refresh(&mut self, store: &FileIssueStore, issues_dir: &Path) {
        let filter = self.effective_filter();
        let issues = match store.list(&filter) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(e.to_string());
                self.rows.clear();
                return;
            }
        };
        self.rows = issues
            .into_iter()
            .map(|issue| IssueRow {
                id: issue.meta.id,
                title: issue.meta.title,
                status: issue.meta.status,
                priority: issue.meta.priority,
                labels: issue.meta.labels,
                assignee_badge: issue.meta.assigned_to.map(|a| {
                    if liveness::is_session_alive(issues_dir, &a.session) {
                        AssigneeBadge::Live(a.session)
                    } else {
                        AssigneeBadge::Stale(a.session)
                    }
                }),
            })
            .collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// Union of the status toggle and the `/` filter-modal fields (design §4).
    fn effective_filter(&self) -> crate::store::issues::IssueFilter {
        crate::store::issues::IssueFilter {
            status: Some(self.status_filter),
            ..self.extra_filter.clone()
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p oxicode-cli issues_panel::refresh_tests`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/issues_panel/mod.rs
git commit -m "feat(tui): implement issues panel list refresh with liveness badges"
```

---

### Task 4: `/issue` slash command opens the panel (List mode only, read-only)

**Files:**
- Create: `oxicode-cli/src/tui_vt/issues_panel/store_handle.rs`
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (add `pub issue_store: Option<Arc<FileIssueStore>>` field to `RenderState`)
- Modify: `oxicode-cli/src/tui_vt/slash/commands.rs` (register `IssueCommand`, call `register_extra`)
- Test: manual smoke test (documented below) + one unit test for lazy store init

**Interfaces:**
- Produces: `pub(crate) fn get_or_open_store(state: &mut RenderState) -> Arc<FileIssueStore>` — lazily opens `FileIssueStore` rooted at `state.cwd` and caches it in `RenderState.issue_store`.
- Consumes: `crate::store::issues::issues_dir(&Path) -> PathBuf`, `FileIssueStore::open(PathBuf) -> Result<Self>`, `state.cwd` (existing `RenderState` field, confirmed set at `main_loop.rs:1077`).

- [ ] **Step 1: Add the lazy store accessor**

```rust
// oxicode-cli/src/tui_vt/issues_panel/store_handle.rs
//! Lazily opens the project's `FileIssueStore`, cached on `RenderState` so
//! every panel action reuses the same in-memory cache instead of re-reading
//! the issues directory from scratch.

use std::sync::Arc;

use crate::store::issues::FileIssueStore;
use crate::tui_vt::main_loop::RenderState;

pub(crate) fn get_or_open_store(state: &mut RenderState) -> anyhow::Result<Arc<FileIssueStore>> {
    if let Some(store) = &state.issue_store {
        return Ok(store.clone());
    }
    let dir = crate::store::issues::issues_dir(&state.cwd);
    let store = Arc::new(FileIssueStore::open(dir)?);
    state.issue_store = Some(store.clone());
    Ok(store)
}
```

Add `mod store_handle; pub(crate) use store_handle::get_or_open_store;` to `issues_panel/mod.rs`.

- [ ] **Step 2: Add the cached field to `RenderState`**

In `main_loop.rs`, near the new `issues_panel` field from Task 1:

```rust
    /// Cached issue store handle, opened lazily on first `/issue` use.
    pub issue_store: Option<std::sync::Arc<crate::store::issues::FileIssueStore>>,
```

and `issue_store: None,` in the hand-written `Default` impl body.

- [ ] **Step 3: Register the `/issue` slash command**

```rust
// oxicode-cli/src/tui_vt/slash/commands.rs — add near the other command structs
pub(crate) struct IssueCommand;

impl SlashCommand for IssueCommand {
    fn name(&self) -> &'static str {
        "issue"
    }
    fn description(&self) -> &'static str {
        "Open the issues panel (list, create, edit, close/reopen local issues)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let store = match crate::tui_vt::issues_panel::get_or_open_store(ctx.state) {
            Ok(s) => s,
            Err(e) => {
                ctx.reply(
                    InlineMessageKind::Error,
                    format!("Could not open issue store: {e}"),
                );
                return SlashOutcome::Handled;
            }
        };
        let mut panel = crate::tui_vt::issues_panel::IssuesPanelState::default();
        panel.refresh(&store, &store.issues_dir());
        ctx.state.issues_panel = Some(panel);
        SlashOutcome::Handled
    }
}
```

Register it in `register_extra` (`commands.rs:20`):

```rust
    registry.register(Box::new(IssueCommand));
```

- [ ] **Step 4: Compile and gate**

Run: `cargo fmt --all -- --check && cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo nextest run -p oxicode-cli`
Expected: all pass. (Rendering the panel is Task 5 — right now `issues_panel: Some(..)` has no visible effect since `render_frame` doesn't branch on it yet; that's fine, this task only proves the data plumbing compiles and the command registers.)

- [ ] **Step 5: Write a unit test for the lazy accessor**

```rust
// oxicode-cli/src/tui_vt/issues_panel/store_handle.rs — append
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_call_reuses_the_cached_arc() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = RenderState {
            cwd: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let first = get_or_open_store(&mut state).unwrap();
        let second = get_or_open_store(&mut state).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }
}
```

Run: `cargo test -p oxicode-cli issues_panel::store_handle::tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add oxicode-cli/src/tui_vt/issues_panel/store_handle.rs oxicode-cli/src/tui_vt/main_loop.rs oxicode-cli/src/tui_vt/slash/commands.rs oxicode-cli/src/tui_vt/issues_panel/mod.rs
git commit -m "feat(tui): register /issue slash command with lazy store handle"
```

---

### Task 5: List view rendering + navigation input

**Files:**
- Create: `oxicode-cli/src/tui_vt/issues_panel/render.rs`
- Create: `oxicode-cli/src/tui_vt/issues_panel/input.rs`
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (`render_frame` branch, key-gating hook)

**Interfaces:**
- Produces: `pub(crate) fn render_issues_panel(frame: &mut Frame<'_>, area: Rect, panel: &IssuesPanelState)`
- Produces: `pub(crate) fn handle_issues_panel_key(state: &Arc<parking_lot::Mutex<RenderState>>, issue_action_tx: &UnboundedSender<IssueActionRequest>, code: KeyCode) -> bool` (returns `true` if the key was consumed)
- Consumes: `IssuesPanelState`/`IssueRow`/`AssigneeBadge` (Task 1/3), `ratatui::{Frame, layout::Rect, widgets::{Block, List, ListItem}}`.

- [ ] **Step 1: Implement the List-mode renderer**

```rust
// oxicode-cli/src/tui_vt/issues_panel/render.rs
//! Rendering for the `/issue` panel. Full-screen overlay — called from
//! `render_frame` when `state.issues_panel.is_some()`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use super::{AssigneeBadge, IssueRow, IssuesPanelMode, IssuesPanelState};

pub(crate) fn render_issues_panel(frame: &mut Frame<'_>, area: Rect, panel: &IssuesPanelState) {
    match &panel.mode {
        IssuesPanelMode::List => render_list(frame, area, panel),
        IssuesPanelMode::FilterInput(buf) => {
            render_list(frame, area, panel);
            render_filter_hint(frame, area, buf);
        }
        IssuesPanelMode::Detail { .. } => render_list(frame, area, panel), // Task 7 replaces this arm
        IssuesPanelMode::Form(_) => render_list(frame, area, panel),      // Task 8 replaces this arm
    }
}

fn render_list(frame: &mut Frame<'_>, area: Rect, panel: &IssuesPanelState) {
    let title = format!(
        "Issues — {} ({})",
        if panel.status_filter == crate::store::issues::Status::Open {
            "open"
        } else {
            "all"
        },
        panel.rows.len()
    );
    let items: Vec<ListItem> = panel
        .rows
        .iter()
        .map(|row| ListItem::new(Line::from(row_spans(row))))
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(panel.selected.min(panel.rows.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut list_state);

    if let Some(err) = &panel.error {
        let footer = Rect {
            y: area.y + area.height.saturating_sub(1),
            height: 1,
            ..area
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                err.clone(),
                Style::default().fg(ratatui::style::Color::Red),
            ))),
            footer,
        );
    }
}

fn row_spans(row: &IssueRow) -> Vec<Span<'static>> {
    let badge = match &row.assignee_badge {
        Some(AssigneeBadge::Live(s)) => format!(" [working: {s}]"),
        Some(AssigneeBadge::Stale(s)) => format!(" [stale claim: {s}]"),
        None => String::new(),
    };
    vec![Span::raw(format!(
        "#{} [{}] {}  {}  {}{}",
        row.id,
        row.priority,
        row.title,
        row.status,
        row.labels.join(","),
        badge
    ))]
}

fn render_filter_hint(frame: &mut Frame<'_>, area: Rect, buf: &str) {
    let hint = Rect {
        y: area.y + area.height.saturating_sub(3),
        height: 3,
        ..area
    };
    let text = format!(
        "filter: {buf}\nEnter: apply · Esc: cancel · Ctrl+U: clear · syntax: priority=critical label=auth text"
    );
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL)),
        hint,
    );
}
```

- [ ] **Step 2: Hook `render_issues_panel` into `render_frame`**

In `main_loop.rs`'s `render_frame` (line ~4345), add, right after the area/background setup and before the normal chat layout is drawn:

```rust
    if let Some(panel) = &state.issues_panel {
        crate::tui_vt::issues_panel::render_issues_panel(frame, area, panel);
        return;
    }
```

(Full-screen overlay per design §5 — short-circuits the rest of the chat render, mirroring how other full-screen overlay branches already return early. Verify against the actual early-return pattern used for `state.overlay` in the same function and match its style exactly.)

- [ ] **Step 3: Implement navigation input handling**

```rust
// oxicode-cli/src/tui_vt/issues_panel/input.rs
//! Key handling for the `/issue` panel — checked before all other key
//! handlers whenever `state.issues_panel.is_some()`, mirroring
//! `handle_overlay_key` / `handle_confirmation_key` / `handle_file_search_key`.

use std::sync::Arc;

use crossterm::event::KeyCode;
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;

use super::{IssueActionRequest, IssuesPanelMode};
use crate::tui_vt::main_loop::RenderState;

/// Returns `true` if the key was consumed by the panel (caller must not fall
/// through to composer/global key handling).
pub(crate) fn handle_issues_panel_key(
    state: &Arc<Mutex<RenderState>>,
    _issue_action_tx: &UnboundedSender<IssueActionRequest>,
    code: KeyCode,
) -> bool {
    let mut s = state.lock();
    let Some(panel) = s.issues_panel.as_mut() else {
        return false;
    };
    if !matches!(panel.mode, IssuesPanelMode::List) {
        return false; // Tasks 6-8 add FilterInput/Detail/Form handling here.
    }
    match code {
        KeyCode::Char('j') | KeyCode::Down => {
            if !panel.rows.is_empty() {
                panel.selected = (panel.selected + 1).min(panel.rows.len() - 1);
            }
            true
        }
        KeyCode::Char('k') | KeyCode::Up => {
            panel.selected = panel.selected.saturating_sub(1);
            true
        }
        KeyCode::Char('f') => {
            panel.status_filter = match panel.status_filter {
                crate::store::issues::Status::Open => crate::store::issues::Status::Closed,
                crate::store::issues::Status::Closed => crate::store::issues::Status::Open,
            };
            let store = s.issue_store.clone();
            if let (Some(store), Some(panel)) = (store, s.issues_panel.as_mut()) {
                panel.refresh(&store, &store.issues_dir());
            }
            true
        }
        KeyCode::Esc => {
            s.issues_panel = None;
            true
        }
        _ => true, // consume everything else in List mode for now; Tasks 6-8 add n/e/c/r/Enter//
    }
}
```

Note: the `_` arm intentionally returns `true` (consumes the key) rather than `false`, because once the panel is open it should behave as a modal — falling through to composer text-entry on an unhandled key would leak keystrokes into the chat prompt underneath. Tasks 6-8 replace specific `KeyCode` arms (`n`, `e`, `c`, `r`, `Enter`, `/`) with real behavior; until then they're safely absorbed as no-ops.

- [ ] **Step 4: Wire the gate into `spawn_input_thread`**

Next to the existing `if s.file_search.is_some() { ... }` gate (`main_loop.rs:3374-3392`), add, checked in the same position (before slash-popup/composer key handling, after confirmation/overlay/file-search gates so those retain priority if ever combined — though in practice `issues_panel` and `overlay` are mutually exclusive modals):

```rust
            {
                let s = state.lock();
                if s.issues_panel.is_some() {
                    drop(s);
                    if crate::tui_vt::issues_panel::handle_issues_panel_key(
                        &state,
                        &issue_action_tx,
                        key.code,
                    ) {
                        continue;
                    }
                }
            }
```

(`issue_action_tx` is the parameter threaded into `spawn_input_thread` in Task 1 Step 5.)

- [ ] **Step 5: Manual smoke test**

Run: `cargo run -p oxicode-cli`. Type `/issue` and press Enter. Expected: full-screen panel opens showing "Issues — open (0)" (or however many open issues exist in the current repo's `.oxicode/issues/`, if any). Press `f` — title switches to "Issues — all (...)". Press `Esc` — panel closes, chat is visible again.

- [ ] **Step 6: Gate and commit**

Run: `cargo fmt --all -- --check && cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo nextest run -p oxicode-cli`

```bash
git add oxicode-cli/src/tui_vt/issues_panel/render.rs oxicode-cli/src/tui_vt/issues_panel/input.rs oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(tui): render issues list and wire navigation/status-toggle keys"
```

---

### Task 6: Filter modal (`/` key)

**Files:**
- Modify: `oxicode-cli/src/tui_vt/issues_panel/input.rs`

**Interfaces:**
- Consumes: `parse_issue_filter` (Task 2).
- Modifies: `IssuesPanelMode::List` arm to add `/`, and adds a new match block for `IssuesPanelMode::FilterInput`.

- [ ] **Step 1: Add the `/` entry and `FilterInput` handling**

Replace the `handle_issues_panel_key` body's mode check (`if !matches!(panel.mode, IssuesPanelMode::List) { return false; }`) with a full match, and add the `/` key to the List arm:

```rust
    match &mut panel.mode {
        IssuesPanelMode::List => match code {
            KeyCode::Char('j') | KeyCode::Down => { /* unchanged from Task 5 */
                true
            }
            KeyCode::Char('k') | KeyCode::Up => { /* unchanged from Task 5 */
                true
            }
            KeyCode::Char('f') => { /* unchanged from Task 5 */
                true
            }
            KeyCode::Char('/') => {
                panel.mode = IssuesPanelMode::FilterInput(String::new());
                true
            }
            KeyCode::Esc => {
                s.issues_panel = None;
                true
            }
            _ => true,
        },
        IssuesPanelMode::FilterInput(buf) => match code {
            KeyCode::Char(c) => {
                buf.push(c);
                true
            }
            KeyCode::Backspace => {
                buf.pop();
                true
            }
            KeyCode::Enter => {
                let buf = buf.clone();
                let extra = super::parse_issue_filter(&buf, panel.status_filter);
                panel.extra_filter = extra;
                panel.mode = IssuesPanelMode::List;
                let store = s.issue_store.clone();
                if let (Some(store), Some(panel)) = (store, s.issues_panel.as_mut()) {
                    panel.refresh(&store, &store.issues_dir());
                }
                true
            }
            KeyCode::Esc => {
                panel.mode = IssuesPanelMode::List;
                true
            }
            _ => true,
        },
        IssuesPanelMode::Detail { .. } | IssuesPanelMode::Form(_) => true, // Tasks 7-8
    }
```

Note: `parse_issue_filter` was declared `pub(crate)` inside `filter_parse.rs` and re-exported as `pub(crate) use filter_parse::parse_issue_filter;` in `mod.rs` (Task 2) — reference it here as `super::parse_issue_filter` from `input.rs`.

Restructure needed: because `code == KeyCode::Char('/')` must not fire while the composer or slash-popup is capturing `/` normally elsewhere — this handler only runs once `state.issues_panel.is_some()`, so there's no ambiguity with the global slash-command `/` (that only applies to the composer, which is hidden behind the full-screen panel).

Handle `Ctrl+U` clearing the buffer — add before the `KeyCode::Char(c)` arm in `FilterInput`, checked via the key's modifiers at the call site. Since `handle_issues_panel_key` currently only receives `KeyCode` (not full `KeyEvent`/modifiers), change its signature to accept the full `crossterm::event::KeyEvent`:

```rust
pub(crate) fn handle_issues_panel_key(
    state: &Arc<Mutex<RenderState>>,
    _issue_action_tx: &UnboundedSender<IssueActionRequest>,
    key: crossterm::event::KeyEvent,
) -> bool {
```

and update the call site in Task 5 Step 4 to pass `key` instead of `key.code`. Inside `FilterInput`, add:

```rust
            KeyCode::Char('u')
                if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                buf.clear();
                true
            }
```

placed before the plain `KeyCode::Char(c)` arm (Rust match arms are order-sensitive; the guarded arm must come first or it will never be reached since `Char('u')` would otherwise match the plain arm).

- [ ] **Step 2: Update the render hint to reflect Ctrl+U**

Already present in Task 5's `render_filter_hint` — no change needed.

- [ ] **Step 3: Manual smoke test**

`cargo run -p oxicode-cli`, `/issue`, press `/`, type `priority=high`, press Enter. Expected: list narrows to high-priority open issues only, panel returns to List mode. Press `/`, type garbage, `Ctrl+U`, confirm buffer clears, `Esc` cancels back to List with the previous filter untouched.

- [ ] **Step 4: Gate and commit**

Run: `cargo fmt --all -- --check && cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo nextest run -p oxicode-cli`

```bash
git add oxicode-cli/src/tui_vt/issues_panel/input.rs oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(tui): add issue filter modal (/ key)"
```

---

### Task 7: Detail view + close/reopen actions

**Files:**
- Modify: `oxicode-cli/src/tui_vt/issues_panel/render.rs` (`render_detail`)
- Modify: `oxicode-cli/src/tui_vt/issues_panel/input.rs` (`Enter`/`c`/`r` in List, full key handling in `Detail`)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (`ConfirmationAction::CloseIssue(u32)` variant + dispatch arm)

**Interfaces:**
- Consumes: `oxicode_vtui::tui::ui::markdown::render_markdown(text: &str, width: usize) -> Vec<Vec<InlineSegment>>`, `FileIssueStore::read(id) -> Result<(Issue, String)>`, `ConfirmationAction`/`ModalConfirmation` (existing types, `main_loop.rs:614-625`).
- Produces: `IssueActionRequest::Close`/`Reopen` are now actually sent (not just logged) — Task 9 implements the receiving side; this task only needs the *sending* half to compile and the confirmation dialog to appear correctly. Until Task 9 lands, the temporary `dispatch_action` from Task 1 will mark `pending = false` without performing the write — call this out in the manual smoke test as an expected interim limitation.

- [ ] **Step 1: Add `ConfirmationAction::CloseIssue`**

In `main_loop.rs` near `pub enum ConfirmationAction` (line ~625):

```rust
    /// Close the given issue id (from the issues panel's `c` key).
    CloseIssue(u32),
```

In the confirmation dispatch match (`main_loop.rs:3759` area, alongside `ConfirmationAction::Quit`/`ClearConversation`):

```rust
                ConfirmationAction::CloseIssue(id) => {
                    let (caller, hash, cwd) = {
                        let s = state.lock();
                        let hash = s
                            .issue_store
                            .as_ref()
                            .and_then(|store| store.read(id).ok())
                            .map(|(_, h)| h);
                        (
                            crate::store::issues::liveness::TUI_OWNERSHIP_ID.to_string(),
                            hash,
                            s.cwd.clone(),
                        )
                    };
                    let _ = cwd; // store is already rooted; kept for clarity/future use
                    let _ = issue_action_tx.send(crate::tui_vt::issues_panel::IssueActionRequest::Close {
                        id,
                        caller,
                        hash,
                    });
                    let mut s = state.lock();
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.pending = true;
                    }
                }
```

(`issue_action_tx` must be reachable at this call site — `handle_confirmation_key` runs on the plain OS input thread (`spawn_input_thread`, called at `main_loop.rs:3351`), not inside the async `run_event_loop`. That's fine: `UnboundedSender::send` is a synchronous, non-blocking call that works from any thread — only the *receiving* side (`issue_action_rx.recv()` in the `select!`, Task 1 Step 5) needs async context, and it already has it. Thread `issue_action_tx` into `handle_confirmation_key`'s signature (`main_loop.rs:3746`) the same way `evt_tx` already is, and update its one call site at `main_loop.rs:3351`.)

- [ ] **Step 2: Add Detail-mode rendering**

```rust
// render.rs — replace the `IssuesPanelMode::Detail { .. } => render_list(...)` arm
IssuesPanelMode::Detail { id, scroll } => render_detail(frame, area, panel, *id, *scroll),
```

```rust
fn render_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    panel: &IssuesPanelState,
    id: u32,
    scroll: usize,
) {
    let Some(row) = panel.rows.iter().find(|r| r.id == id) else {
        render_list(frame, area, panel);
        return;
    };
    let header = format!(
        "#{} {}  [{}] {}  labels: {}",
        row.id, row.status, row.priority, row.title, row.labels.join(",")
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Issue #{id} — Esc: back, e: edit, c: close, r: reopen"));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_area = Rect {
        height: 1,
        ..inner
    };
    frame.render_widget(Paragraph::new(header), header_area);

    let body_area = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let body_text = panel
        .detail_body_cache
        .as_deref()
        .unwrap_or("(loading…)");
    let lines = oxicode_vtui::tui::ui::markdown::render_markdown(body_text, body_area.width as usize);
    let ratatui_lines: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .map(|segs| {
            Line::from(
                segs.into_iter()
                    .map(|seg| Span::raw(seg.text))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(ratatui_lines), body_area);
}
```

This introduces a new field `detail_body_cache: Option<String>` on `IssuesPanelState` (populated synchronously via `store.read(id)` when entering Detail mode, since `read` is a sync call — see Step 3). Add it to the struct (Task 1's definition) and its `Default` derive already covers `None`.

```rust
// mod.rs — IssuesPanelState gains:
pub detail_body_cache: Option<String>,
```

- [ ] **Step 3: Add List→Detail transition and Detail key handling**

In `input.rs`'s List arm, add:

```rust
            KeyCode::Enter => {
                if let Some(row) = panel.rows.get(panel.selected) {
                    let id = row.id;
                    let store = s.issue_store.clone();
                    if let Some(store) = store {
                        if let Ok((issue, _hash)) = store.read(id) {
                            if let Some(panel) = s.issues_panel.as_mut() {
                                panel.detail_body_cache = Some(issue.body);
                                panel.mode = IssuesPanelMode::Detail { id, scroll: 0 };
                            }
                        }
                    }
                }
                true
            }
```

and a `Detail` match arm:

```rust
        IssuesPanelMode::Detail { id, scroll } => {
            let id = *id;
            match code {
                KeyCode::Esc => {
                    panel.mode = IssuesPanelMode::List;
                    true
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    *scroll += 1;
                    true
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *scroll = scroll.saturating_sub(1);
                    true
                }
                KeyCode::Char('c') => {
                    s.confirmation = Some(crate::tui_vt::main_loop::ModalConfirmation {
                        title: "Close issue".into(),
                        message: format!("  y \u{2014} close #{id}     n / x \u{2014} cancel"),
                        action: crate::tui_vt::main_loop::ConfirmationAction::CloseIssue(id),
                    });
                    true
                }
                KeyCode::Char('r') => {
                    let hash = s
                        .issue_store
                        .as_ref()
                        .and_then(|store| store.read(id).ok())
                        .map(|(_, h)| h);
                    let _ = _issue_action_tx.send(IssueActionRequest::Reopen { id, hash });
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.pending = true;
                    }
                    true
                }
                _ => true, // 'e' wired in Task 8
            }
        }
```

- [ ] **Step 4: Manual smoke test**

`cargo run -p oxicode-cli`, `/issue`, create at least one issue beforehand via `oxicode issue new --title "test" --body "hello"` (existing CLI). Open panel, `Enter` on the row → Detail view shows header + rendered body. `r` reopens instantly (if it was closed) — confirm no crash even though Task 9's real dispatch isn't wired yet (interim: `pending` flips true/false with no actual write, documented limitation). `c` opens a Yes/No confirmation dialog; confirm it renders and `Esc`/`n` cancels it.

- [ ] **Step 5: Gate and commit**

Run: `cargo fmt --all -- --check && cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo nextest run -p oxicode-cli`

```bash
git add oxicode-cli/src/tui_vt/issues_panel/render.rs oxicode-cli/src/tui_vt/issues_panel/input.rs oxicode-cli/src/tui_vt/issues_panel/mod.rs oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(tui): add issue detail view with close/reopen actions"
```

---

### Task 8: Create/Edit form

**Files:**
- Modify: `oxicode-cli/src/tui_vt/issues_panel/render.rs` (`render_form`)
- Modify: `oxicode-cli/src/tui_vt/issues_panel/input.rs` (`n`/`e` entry points, `Form` mode key handling, `Tab` focus cycling, `Ctrl+Enter` submit)
- Test: unit tests for label parsing and priority cycling (pure functions, no TUI needed)

**Interfaces:**
- Consumes: `oxicode_textarea::TextArea::{new, set_text, text, input}`, `FileIssueStore::create(title, body, priority, labels, caller) -> Result<Issue>` (sync), `IssuePatch` (Task 1).
- Produces: `pub(crate) fn cycle_priority(p: Priority, forward: bool) -> Priority`, `pub(crate) fn parse_labels(input: &str) -> Vec<String>` — pure, independently testable per design §5's Form spec.

- [ ] **Step 1: Write failing tests for the pure helpers**

```rust
// oxicode-cli/src/tui_vt/issues_panel/mod.rs — new module
mod form;
pub(crate) use form::{cycle_priority, parse_labels};
```

```rust
// oxicode-cli/src/tui_vt/issues_panel/form.rs
use crate::store::issues::Priority;

pub(crate) fn cycle_priority(p: Priority, forward: bool) -> Priority {
    use Priority::*;
    match (p, forward) {
        (Low, true) => Medium,
        (Medium, true) => High,
        (High, true) => Critical,
        (Critical, true) => Low,
        (Low, false) => Critical,
        (Medium, false) => Low,
        (High, false) => Medium,
        (Critical, false) => High,
    }
}

pub(crate) fn parse_labels(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_priority_forward_wraps_at_critical() {
        assert_eq!(cycle_priority(Priority::Critical, true), Priority::Low);
    }

    #[test]
    fn cycle_priority_backward_wraps_at_low() {
        assert_eq!(cycle_priority(Priority::Low, false), Priority::Critical);
    }

    #[test]
    fn cycle_priority_forward_steps_through_all() {
        assert_eq!(cycle_priority(Priority::Low, true), Priority::Medium);
        assert_eq!(cycle_priority(Priority::Medium, true), Priority::High);
        assert_eq!(cycle_priority(Priority::High, true), Priority::Critical);
    }

    #[test]
    fn parse_labels_splits_trims_and_drops_empty() {
        assert_eq!(
            parse_labels(" auth, bug ,,ui "),
            vec!["auth".to_string(), "bug".to_string(), "ui".to_string()]
        );
    }

    #[test]
    fn parse_labels_empty_string_yields_empty_vec() {
        assert!(parse_labels("").is_empty());
        assert!(parse_labels("   ").is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail, then pass**

Run: `cargo test -p oxicode-cli issues_panel::form::tests`
Expected: FAIL (module doesn't exist) → after adding the `mod form;` line and the file above → PASS (5/5).

- [ ] **Step 3: Add form rendering**

```rust
// render.rs — replace the `IssuesPanelMode::Form(_) => render_list(...)` arm
IssuesPanelMode::Form(form) => render_form(frame, area, form),
```

```rust
fn render_form(frame: &mut Frame<'_>, area: Rect, form: &super::IssueFormState) {
    use super::FormField;

    let title = if form.editing_id.is_some() {
        "Edit issue — Tab: next field, Ctrl+Enter: save, Esc: cancel"
    } else {
        "New issue — Tab: next field, Ctrl+Enter: create, Esc: cancel"
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let focus_marker = |f: FormField| if form.focus == f { "> " } else { "  " };

    let title_line = format!("{}Title: {}", focus_marker(FormField::Title), form.title);
    let priority_line = format!(
        "{}Priority (\u{2190}/\u{2192}): {}",
        focus_marker(FormField::Priority),
        form.priority
    );
    let labels_line = format!(
        "{}Labels (comma-separated): {}",
        focus_marker(FormField::Labels),
        form.labels_input
    );

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(title_line),
            Line::from(priority_line),
            Line::from(labels_line),
            Line::from(format!("{}Body:", focus_marker(FormField::Body))),
        ]),
        Rect {
            height: 4,
            ..inner
        },
    );

    let body_area = Rect {
        y: inner.y + 4,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    form.body.render(frame, body_area); // TextArea's own render method — confirm exact
                                         // method name/signature against
                                         // oxicode-textarea's public API before
                                         // implementing (it is used identically
                                         // for the composer elsewhere in
                                         // main_loop.rs — mirror that call site).
}
```

- [ ] **Step 4: Add `n`/`e` entry points and Form key handling**

In `input.rs`'s List arm, add:

```rust
            KeyCode::Char('n') => {
                panel.mode = IssuesPanelMode::Form(super::IssueFormState::default());
                true
            }
            KeyCode::Char('e') => {
                if let Some(row) = panel.rows.get(panel.selected) {
                    let id = row.id;
                    if let Some(store) = s.issue_store.clone() {
                        if let Ok((issue, hash)) = store.read(id) {
                            if let Some(a) = &issue.meta.assigned_to {
                                if a.session != crate::store::issues::liveness::TUI_OWNERSHIP_ID
                                    && crate::store::issues::liveness::is_session_alive(
                                        &store.issues_dir(),
                                        &a.session,
                                    )
                                {
                                    if let Some(panel) = s.issues_panel.as_mut() {
                                        panel.error = Some(format!(
                                            "issue #{id} is being worked on by session {}",
                                            a.session
                                        ));
                                    }
                                    return true;
                                }
                            }
                            let mut form = super::IssueFormState {
                                editing_id: Some(id),
                                content_hash: Some(hash),
                                title: issue.meta.title,
                                priority: issue.meta.priority,
                                labels_input: issue.meta.labels.join(", "),
                                ..super::IssueFormState::default()
                            };
                            form.body.set_text(&issue.body);
                            if let Some(panel) = s.issues_panel.as_mut() {
                                panel.mode = IssuesPanelMode::Form(form);
                            }
                        }
                    }
                }
                true
            }
```

and a `Form` match arm using the full `KeyEvent` (for `Ctrl+Enter` detection and `Shift+Tab`):

```rust
        IssuesPanelMode::Form(form) => match code {
            KeyCode::Esc => {
                panel.mode = IssuesPanelMode::List;
                true
            }
            KeyCode::Tab => {
                form.focus = match form.focus {
                    super::FormField::Title => super::FormField::Priority,
                    super::FormField::Priority => super::FormField::Labels,
                    super::FormField::Labels => super::FormField::Body,
                    super::FormField::Body => super::FormField::Title,
                };
                true
            }
            KeyCode::Left if form.focus == super::FormField::Priority => {
                form.priority = super::cycle_priority(form.priority, false);
                true
            }
            KeyCode::Right if form.focus == super::FormField::Priority => {
                form.priority = super::cycle_priority(form.priority, true);
                true
            }
            KeyCode::Char(c) if form.focus == super::FormField::Title => {
                form.title.push(c);
                true
            }
            KeyCode::Backspace if form.focus == super::FormField::Title => {
                form.title.pop();
                true
            }
            KeyCode::Char(c) if form.focus == super::FormField::Labels => {
                form.labels_input.push(c);
                true
            }
            KeyCode::Backspace if form.focus == super::FormField::Labels => {
                form.labels_input.pop();
                true
            }
            KeyCode::Enter
                if form.focus == super::FormField::Body
                    && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                submit_form(&mut s, _issue_action_tx);
                true
            }
            _ if form.focus == super::FormField::Body => {
                form.body.input(key);
                true
            }
            _ => true,
        },
```

(`code` and `key` both need to be in scope — since Task 6 already changed the function signature to take the full `KeyEvent`, destructure `let code = key.code;` at the top of `handle_issues_panel_key` for the existing `match code` arms to keep compiling, and reference `key` directly where modifiers are needed.)

- [ ] **Step 5: Implement `submit_form`**

```rust
// input.rs
fn submit_form(
    s: &mut parking_lot::MutexGuard<'_, RenderState>,
    issue_action_tx: &UnboundedSender<IssueActionRequest>,
) {
    let Some(IssuesPanelMode::Form(form)) = s.issues_panel.as_ref().map(|p| p.mode.clone())
    else {
        return;
    };
    let labels = super::parse_labels(&form.labels_input);
    let body = form.body.text().to_string();

    match form.editing_id {
        None => {
            // Synchronous — `create` is not CAS-guarded.
            let Some(store) = s.issue_store.clone() else {
                return;
            };
            match store.create(
                form.title.clone(),
                body,
                form.priority,
                labels,
                Some(crate::store::issues::liveness::TUI_OWNERSHIP_ID),
            ) {
                Ok(_issue) => {
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.mode = IssuesPanelMode::List;
                        panel.error = None;
                    }
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.refresh(&store, &store.issues_dir());
                    }
                }
                Err(e) => {
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.error = Some(e.to_string());
                    }
                }
            }
        }
        Some(id) => {
            let patch = crate::store::issues::IssuePatch {
                title: Some(form.title.clone()),
                body: Some(body),
                priority: Some(form.priority),
                labels: Some(labels),
                ..Default::default()
            };
            let _ = issue_action_tx.send(IssueActionRequest::ApplyPatch {
                id,
                patch,
                caller: Some(crate::store::issues::liveness::TUI_OWNERSHIP_ID.to_string()),
                hash: form.content_hash.clone(),
            });
            if let Some(panel) = s.issues_panel.as_mut() {
                panel.pending = true;
                panel.mode = IssuesPanelMode::List;
            }
        }
    }
}
```

Note: `IssuesPanelMode` must derive `Clone` for the `.map(|p| p.mode.clone())` above — already declared `#[derive(Clone, Debug, Default)]` in Task 1.

- [ ] **Step 6: Manual smoke test**

`cargo run -p oxicode-cli`, `/issue`, `n`, type a title, `Tab` to Priority, `→` twice, `Tab` to Labels, type `a, b`, `Tab` to Body, type some markdown, `Ctrl+Enter`. Expected: returns to List, new row appears with correct priority/labels. `e` on that row, confirm fields are pre-filled, edit title, `Ctrl+Enter` — panel returns to List with `pending=true` (actual persistence lands in Task 9).

- [ ] **Step 7: Gate and commit**

Run: `cargo fmt --all -- --check && cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo nextest run -p oxicode-cli`

```bash
git add oxicode-cli/src/tui_vt/issues_panel/form.rs oxicode-cli/src/tui_vt/issues_panel/render.rs oxicode-cli/src/tui_vt/issues_panel/input.rs oxicode-cli/src/tui_vt/issues_panel/mod.rs
git commit -m "feat(tui): add issue create/edit form"
```

---

### Task 9: Real async dispatch (`IssueActionRequest` → store writes)

**Files:**
- Modify: `oxicode-cli/src/tui_vt/issues_panel/mod.rs` (`dispatch_action` real implementation)
- Test: integration test with a tempdir-backed store and concurrent `apply_patch` calls (CAS-retry smoke test)

**Interfaces:**
- Consumes: `crate::tools::issue_tool::cas_retry` (made `pub(crate)` in Task 1), `FileIssueStore::{close, reopen, apply_patch}` (all `async fn`).
- Replaces: the placeholder `dispatch_action` from Task 1 Step 5.

- [ ] **Step 1: Replace the placeholder with the real dispatcher**

```rust
// oxicode-cli/src/tui_vt/issues_panel/mod.rs
pub(crate) fn dispatch_action(
    req: IssueActionRequest,
    state: std::sync::Arc<parking_lot::Mutex<crate::tui_vt::main_loop::RenderState>>,
) {
    let store = { state.lock().issue_store.clone() };
    let Some(store) = store else {
        let mut s = state.lock();
        if let Some(panel) = s.issues_panel.as_mut() {
            panel.pending = false;
            panel.error = Some("issue store not initialized".into());
        }
        return;
    };
    tokio::spawn(async move {
        let result = match req {
            IssueActionRequest::Close { id, caller, hash } => {
                crate::tools::issue_tool::cas_retry(&store, id, hash, |h| {
                    let store = store.clone();
                    let caller = caller.clone();
                    async move { store.close(id, &caller, h).await }
                })
                .await
            }
            IssueActionRequest::Reopen { id, hash } => {
                crate::tools::issue_tool::cas_retry(&store, id, hash, |h| {
                    let store = store.clone();
                    async move { store.reopen(id, h).await }
                })
                .await
            }
            IssueActionRequest::ApplyPatch {
                id,
                patch,
                caller,
                hash,
            } => {
                crate::tools::issue_tool::cas_retry(&store, id, hash, |h| {
                    let store = store.clone();
                    let patch = patch.clone();
                    let caller = caller.clone();
                    async move { store.apply_patch(id, patch, caller, h).await }
                })
                .await
            }
        };

        let mut s = state.lock();
        if let Some(panel) = s.issues_panel.as_mut() {
            panel.pending = false;
            match result {
                Ok(_) => panel.error = None,
                Err(e) => panel.error = Some(e.to_string()),
            }
        }
        // Refresh regardless of outcome — a failed write may still reflect
        // a concurrent change made by another session.
        let store2 = s.issue_store.clone();
        if let (Some(store2), Some(panel)) = (store2, s.issues_panel.as_mut()) {
            panel.refresh(&store2, &store2.issues_dir());
        }
    });
}
```

- [ ] **Step 2: Write the CAS-retry integration smoke test**

```rust
#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::store::issues::{FileIssueStore, IssuePatch, Priority};
    use std::sync::Arc;

    #[tokio::test]
    async fn concurrent_apply_patch_via_dispatch_action_both_eventually_succeed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(FileIssueStore::open(tmp.path().to_path_buf()).unwrap());
        let issue = store
            .create("t".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (_issue, hash) = store.read(issue.meta.id).unwrap();

        let state = Arc::new(parking_lot::Mutex::new(
            crate::tui_vt::main_loop::RenderState {
                issue_store: Some(store.clone()),
                issues_panel: Some(IssuesPanelState::default()),
                ..Default::default()
            },
        ));

        // Both requests carry the SAME (now-stale-after-the-first-write) hash,
        // forcing the second one through cas_retry's re-read-and-retry path.
        dispatch_action(
            IssueActionRequest::ApplyPatch {
                id: issue.meta.id,
                patch: IssuePatch {
                    title: Some("first".into()),
                    ..Default::default()
                },
                caller: None,
                hash: Some(hash.clone()),
            },
            state.clone(),
        );
        dispatch_action(
            IssueActionRequest::ApplyPatch {
                id: issue.meta.id,
                patch: IssuePatch {
                    priority: Some(Priority::High),
                    ..Default::default()
                },
                caller: None,
                hash: Some(hash),
            },
            state.clone(),
        );

        // Give both spawned tasks a chance to run.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let (final_issue, _) = store.read(issue.meta.id).unwrap();
        assert_eq!(final_issue.meta.title, "first");
        assert_eq!(final_issue.meta.priority, Priority::High);
        let s = state.lock();
        assert!(s.issues_panel.as_ref().unwrap().error.is_none());
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p oxicode-cli issues_panel::dispatch_tests -- --test-threads=1`
Expected: PASS. (`--test-threads=1` avoids flakiness from the 200ms sleep racing other tests' CPU contention; the plain `cargo nextest run` gate in Step 4 already isolates tests into separate processes so this flag isn't needed there.)

- [ ] **Step 4: Full gate**

Run: `cargo fmt --all -- --check && cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo nextest run -p oxicode-cli`
Expected: all pass.

- [ ] **Step 5: Manual end-to-end smoke test**

`cargo run -p oxicode-cli`. `/issue`, `n`, create an issue, `Ctrl+Enter`. `Enter` on it to open Detail, `c`, confirm — issue disappears from the default Open-filtered List. `f` to show All — closed issue reappears with status `closed`. `Enter` → Detail → `r` — issue reopens, reappears under the Open filter. `e` an issue, change its title, `Ctrl+Enter` — reopen Detail and confirm the title changed on disk (`cat .oxicode/issues/<id>-*.md`).

- [ ] **Step 6: Commit**

```bash
git add oxicode-cli/src/tui_vt/issues_panel/mod.rs
git commit -m "feat(tui): wire real CAS-guarded async dispatch for issue mutations"
```

### Task 10: Automated render snapshot tests (List/Detail/Form)

**Files:**
- Modify: `oxicode-cli/src/tui_vt/issues_panel/render.rs` (add `#[cfg(test)]` module)

**Interfaces:**
- Consumes: `ratatui::backend::TestBackend`, `ratatui::Terminal` (already a dev-dependency — used by the existing `render_frame` snapshot tests in `main_loop.rs`, e.g. `commit_plan_sheds_oldest_blocks_and_keeps_the_tail` at `main_loop.rs:9482`). Mirror that exact setup: `Terminal::new(TestBackend::new(w, h))`, `terminal.draw(|frame| ...)`.

This closes a design §7 requirement not yet covered by Tasks 5–8: "List/Detail/Form 세 모드가 패닉 없이 렌더되는지" as an automated (not just manual) check.

- [ ] **Step 1: Write the three panic-freedom tests**

```rust
// oxicode-cli/src/tui_vt/issues_panel/render.rs — append
#[cfg(test)]
mod render_tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::store::issues::{Priority, Status};

    fn sample_row() -> IssueRow {
        IssueRow {
            id: 1,
            title: "sample issue".into(),
            status: Status::Open,
            priority: Priority::High,
            labels: vec!["auth".into()],
            assignee_badge: Some(AssigneeBadge::Live("tui".into())),
        }
    }

    #[test]
    fn list_mode_renders_without_panicking() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_issues_panel(frame, frame.area(), &panel))
            .unwrap();
    }

    #[test]
    fn detail_mode_renders_without_panicking() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            mode: IssuesPanelMode::Detail { id: 1, scroll: 0 },
            detail_body_cache: Some("# Heading\n\nSome **body** text.".into()),
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_issues_panel(frame, frame.area(), &panel))
            .unwrap();
    }

    #[test]
    fn form_mode_renders_without_panicking() {
        let panel = IssuesPanelState {
            mode: IssuesPanelMode::Form(super::super::IssueFormState::default()),
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_issues_panel(frame, frame.area(), &panel))
            .unwrap();
    }

    #[test]
    fn filter_input_mode_renders_without_panicking() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            mode: IssuesPanelMode::FilterInput("priority=high".into()),
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_issues_panel(frame, frame.area(), &panel))
            .unwrap();
    }

    #[test]
    fn empty_rows_and_tiny_viewport_do_not_panic() {
        // Regression guard for the `Rect` arithmetic in render_detail/
        // render_filter_hint (`area.height.saturating_sub(...)`) — a 1-row
        // viewport is the smallest a terminal can realistically report.
        let panel = IssuesPanelState::default();
        let mut terminal = Terminal::new(TestBackend::new(20, 1)).unwrap();
        terminal
            .draw(|frame| render_issues_panel(frame, frame.area(), &panel))
            .unwrap();
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p oxicode-cli issues_panel::render_tests`
Expected: all 5 PASS. If `empty_rows_and_tiny_viewport_do_not_panic` fails on a `Rect` underflow/overflow panic, fix the offending arithmetic in `render.rs` (every `Rect` field computed from `area.height`/`area.width` must use `saturating_sub`/`.min(...)`, not plain subtraction) — this is exactly the kind of bug this test exists to catch.

- [ ] **Step 3: Full gate**

Run: `cargo fmt --all -- --check && cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo nextest run -p oxicode-cli`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add oxicode-cli/src/tui_vt/issues_panel/render.rs
git commit -m "test(tui): add render panic-freedom tests for issues panel modes"
```

---

## Post-plan verification

```bash
cargo fmt --all -- --check
cargo clippy -p oxicode-cli --all-targets -- -D warnings
cargo nextest run -p oxicode-cli
```

All three must pass with zero warnings/failures before considering the feature complete.
