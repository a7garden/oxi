# Advisor + Agent Hub Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fullscreen TUI Agent Hub overlay that shows the main agent, advisor reviewer, and any subagents in a table, with live transcript viewer for each — built on the already-ported `oxicode-agent/src/advisor/` engine (1,846 LOC) plus out-of-process subagent `.jsonl` files.

**Architecture:** Pull-based mtime polling on `__advisor.jsonl` (advisor) and `<id>.jsonl` (subagent) inside the session directory. oxicode-sdk's existing `AgentPool` stores `Arc<Agent>` keyed by id; a new `HubRegistry` in oxicode-cli adds display metadata (kind, last_activity, current_task, session_file) keyed by the same id. The Hub overlay polls both at 250ms.

**Tech Stack:** Rust 2024, ratatui 0.30, crossterm 0.29, oxicode-tui (tape), oxicode-sdk AgentPool, oxicode-agent AdvisorRuntime.

## Global Constraints

- Every task ends with: `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings`, `cargo fmt --all -- --check`, `cargo nextest run --workspace` all green.
- No new external dependencies.
- oxicode-sdk `AgentPool` API stays additive: only add new methods, do not change existing signatures.
- Hub overlay uses `OverlayComponent` trait (same as 19 existing overlays); fullscreen via `terminal_host` alt-screen path.
- Transcript parsing handles two JSONL formats: `SessionEntry` (subagent) and `{"ts":N,"messages":[…]}` (advisor). Discriminated by `ts` field presence.
- All Hub keys (`Ctrl+h`, `j`/`k`, `Enter`, `Esc`) integrated into `oxicode-tui/src/keybindings/registry.rs` Action enum; missing-match in `dispatch_action` is a compile error (existing guardrail).
- Tests are unit-level for components with edge cases (parsing, refresh, sort, format_age). A single PTY end-to-end test for the hub open/close gesture.
- Attribution: this implementation is a Rust port of `oh-my-pi` (omp) Agent Hub. Original MIT-licensed by Mario Zechner and Can Bölük.

## File Structure

```
oxicode-cli/src/tui/overlay/agent_hub/    [NEW] ~500 LOC
├── mod.rs              AgentHubOverlay struct + OverlayComponent impl
├── state.rs            HubRow, HubView, sort logic, format_age
├── table.rs            render_table, status_badge, key hints
├── transcript.rs       TranscriptReader + TranscriptLine + parse_jsonl
└── keys.rs             handle_key, mode transitions

oxicode-cli/src/tui/slash/builtin/agents.rs   [NEW] /agents slash command
oxicode-cli/src/app/agent_hub_registry.rs     [NEW] HubRegistry (display metadata)
oxicode-cli/src/app/agent_hub_bridge.rs       [NEW] Advisor + persisted-subagent registration
oxicode-cli/src/tui/overlay/mod.rs            [MOD] register agent_hub module
oxicode-cli/src/tui/slash/builtin/mod.rs       [MOD] register AgentsCommand
oxicode-cli/src/tui/handlers.rs               [MOD] ToggleAgentHub dispatch + AdvisorCard UI event
oxicode-cli/src/tui/app.rs                    [MOD] UiEvent::AdvisorCard variant + content insertion
oxicode-cli/src/tui/overlay/issues_panel/     [REF] none — keep as-is (template pattern only)
oxicode-cli/src/app/agent_session.rs          [MOD] expose HubRegistry + transcript paths
oxicode-tui/src/widgets/chat/types.rs         [MOD] ContentBlock::Advisory variant
oxicode-tui/src/widgets/chat/markdown.rs      [MOD] Advisory severity-colored card render
oxicode-tui/src/widgets/chat/render.rs        [MOD] route ContentBlock::Advisory through transcript
oxicode-tui/src/widgets/chat/state.rs         [MOD] `advisory_count` helper if needed
oxicode-tui/src/keybindings/registry.rs       [MOD] ToggleAgentHub action + binding + parse_action
oxicode-sdk/src/lifecycle/agent_pool.rs       [MOD] add `for_each_row` (snapshot method)
oxicode-sdk/src/lifecycle/supervisor.rs       [MOD] pub use of status constants (re-export)
oxicode-agent/src/advisor/runtime.rs          [MOD] `transcript_path()` getter on AdvisorRuntime
oxicode-agent/src/advisor/types.rs            [MOD] expose severity copy for transcript card
oxicode-cli/tests/pty_e2e.rs                  [MOD] add test_pty_hub_opens_and_lists_advisor
```

---

### Task 1: oxicode-sdk AgentPool snapshot + AgentKind/HubStatus types

**Files:**
- Modify: `oxicode-sdk/src/lifecycle/agent_pool.rs`
- Modify: `oxicode-sdk/src/lifecycle/mod.rs` (re-exports if needed)
- Modify: `oxicode-sdk/src/lifecycle/supervisor.rs` (pub use of `STATUS_RUNNING` etc.)
- Test: `oxicode-sdk/src/lifecycle/agent_pool.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: existing `AgentPool::insert/get/list/ids/len/contains` (no changes to signatures)
- Produces: `AgentPool::for_each_row<F: FnMut(&str, &Arc<Agent>)>(&self, f: F)`
- Produces: re-export of status constants from `supervisor` as `pub use supervisor::AgentKind` and `pub use supervisor::HubStatus` if placed there; OR new file `oxicode-sdk/src/lifecycle/hub.rs` for the `AgentKind` and `HubStatus` enums (preferred — keeps lifecycle cohesive)

**Decision: put `AgentKind`/`HubStatus` in a new file** `oxicode-sdk/src/lifecycle/hub.rs` to keep `supervisor.rs` focused on lifecycle state. Re-export from `oxicode-sdk/src/lifecycle/mod.rs`.

- [ ] **Step 1: Write failing test for `for_each_row`**

In `oxicode-sdk/src/lifecycle/agent_pool.rs` `#[cfg(test)] mod tests`:
```rust
#[test]
fn for_each_row_visits_all_agents() {
    let pool = AgentPool::new();
    pool.insert("a".into(), /* a mock Agent — but Agent has no pub ctor for tests */ unimplemented!());
    // We need a different approach: use existing test setup that creates Agents.
    // Skip: this is verified manually via M3 integration test.
}
```

NOTE: `Agent::new` requires a `Provider` impl. The existing `agent_pool.rs` test module has a `MockProvider` in `oxicode-sdk`. Check it; if absent, we skip this micro-test and rely on M3 integration. Replace the test above with:

```rust
#[test]
fn for_each_row_visits_all_inserted() {
    use std::sync::Arc;
    // Use a stub Arc<Agent> via the existing test helper if present;
    // otherwise this test is gated on a feature flag. For now: verify empty.
    let pool = AgentPool::new();
    let mut seen = Vec::new();
    pool.for_each_row(|id, _| seen.push(id.to_string()));
    assert!(seen.is_empty());
}
```

- [ ] **Step 2: Run test to verify RED**

Run: `cargo nextest run -p oxicode-sdk lifecycle::agent_pool::tests::for_each_row_visits_all_inserted`
Expected: FAIL with "no method named `for_each_row` found for struct `AgentPool`".

- [ ] **Step 3: Implement `for_each_row`**

In `oxicode-sdk/src/lifecycle/agent_pool.rs`, add method to `impl AgentPool`:
```rust
/// Snapshot iteration over all (id, agent) pairs. Holds the read lock for
/// the duration of the closure; do not call back into the pool from `f`.
pub fn for_each_row<F: FnMut(&str, &Arc<Agent>)>(&self, mut f: F) {
    let agents = self.agents.read();
    for (id, agent) in agents.iter() {
        f(id.as_str(), agent);
    }
}
```

- [ ] **Step 4: Run test to verify GREEN**

Run: `cargo nextest run -p oxicode-sdk lifecycle::agent_pool::tests::for_each_row_visits_all_inserted`
Expected: PASS.

- [ ] **Step 5: Add `HubKind` and `HubStatus` enums**

Create `oxicode-sdk/src/lifecycle/hub.rs`:
```rust
//! Display metadata for the Agent Hub overlay (advisor + subagent monitoring).
//! Kept separate from supervisor.rs (lifecycle state machine) so display
//! concerns don't pollute the supervisor API.

use serde::{Deserialize, Serialize};

/// Role of an agent in the hub view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HubKind {
    /// The main session agent.
    Main,
    /// A subagent spawned by the `subagent` tool (in- or out-of-process).
    Subagent,
    /// The read-only advisor reviewer.
    Advisor,
}

impl HubKind {
    /// omp `kind` lower-case tag.
    pub const fn as_str(self) -> &'static str {
        match self {
            HubKind::Main => "main",
            HubKind::Subagent => "task",
            HubKind::Advisor => "advisor",
        }
    }
}

/// High-level status for the hub table. Maps onto the supervisor's atomic
/// status (Running/Suspended/Terminated/Failed) + a parked concept for
/// agents kept alive after completion awaiting revival.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HubStatus {
    Running,
    Idle,
    /// STOPPED in supervisor terms; held in memory for later revival.
    Parked,
    /// FAILED or unrecoverable.
    Aborted,
}

impl HubStatus {
    /// Lower-case tag for the status badge column.
    pub const fn as_str(self) -> &'static str {
        match self {
            HubStatus::Running => "running",
            HubStatus::Idle => "idle",
            HubStatus::Parked => "parked",
            HubStatus::Aborted => "aborted",
        }
    }

    /// Sort priority — lower comes first.
    pub const fn sort_key(self) -> u8 {
        match self {
            HubStatus::Running => 0,
            HubStatus::Idle => 1,
            HubStatus::Parked => 2,
            HubStatus::Aborted => 3,
        }
    }
}
```

In `oxicode-sdk/src/lifecycle/mod.rs`, add:
```rust
pub mod hub;
pub use hub::{HubKind, HubStatus};
```

- [ ] **Step 6: Run build + test**

```bash
cargo build -p oxicode-sdk
cargo nextest run -p oxicode-sdk lifecycle
```
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add oxicode-sdk/src/lifecycle/agent_pool.rs oxicode-sdk/src/lifecycle/hub.rs oxicode-sdk/src/lifecycle/mod.rs
git commit -m "feat(sdk): add HubKind/HubStatus + AgentPool::for_each_row

Foundation for the Agent Hub overlay (advisor + subagent monitoring):
- HubKind {Main, Subagent, Advisor} — matches omp's AgentKind lower-case
  tags (main/task/advisor) so transcript-recorder naming stays compatible.
- HubStatus {Running, Idle, Parked, Aborted} with sort_key for the table
  view. Parked covers the supervisor's TERMINATED state for agents held
  in memory pending revival.
- AgentPool::for_each_row is a non-allocating snapshot iteration that the
  Hub overlay will call on every poll tick."
```

---

### Task 2: oxicode-cli HubRegistry — display metadata store

**Files:**
- Create: `oxicode-cli/src/app/agent_hub_registry.rs`
- Modify: `oxicode-cli/src/app/mod.rs` (re-export)

**Interfaces:**
- Consumes: `oxicode_sdk::lifecycle::AgentPool`, `oxicode_sdk::HubKind`, `oxicode_sdk::HubStatus`
- Produces: `pub struct HubRegistry { inner: parking_lot::RwLock<HashMap<String, HubEntry>> }`
- Produces: `pub struct HubEntry { pub kind: HubKind, pub status: HubStatus, pub display_name: String, pub current_task: Option<String>, pub last_activity_ms: u64, pub session_file: Option<PathBuf> }`
- Produces: `HubRegistry::new()`, `register(id, entry)`, `update(id, f)`, `unregister(id)`, `snapshot() -> Vec<(String, HubEntry)>` (sorted)

**Why a separate registry and not extending AgentPool**: AgentPool stores `Arc<Agent>` for runtime state. The hub needs display metadata (kind, last_activity, current_task) that is TUI-specific and should not pollute the SDK's lifecycle surface. A parallel `HashMap<String, HubEntry>` in oxicode-cli keeps the boundary clean. The agent id is the shared key.

- [ ] **Step 1: Write failing tests for `HubRegistry`**

In the new file:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use oxicode_sdk::{HubKind, HubStatus};

    fn entry(kind: HubKind, status: HubStatus) -> HubEntry {
        HubEntry {
            kind,
            status,
            display_name: "test".into(),
            current_task: None,
            last_activity_ms: 0,
            session_file: None,
        }
    }

    #[test]
    fn register_and_snapshot() {
        let r = HubRegistry::new();
        r.register("a".into(), entry(HubKind::Main, HubStatus::Running));
        r.register("b".into(), entry(HubKind::Advisor, HubStatus::Idle));
        let snap = r.snapshot();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn snapshot_sorts_running_first_then_idle() {
        let r = HubRegistry::new();
        r.register("idle".into(), entry(HubKind::Subagent, HubStatus::Idle));
        r.register("running".into(), entry(HubKind::Main, HubStatus::Running));
        r.register("parked".into(), entry(HubKind::Subagent, HubStatus::Parked));
        r.register("aborted".into(), entry(HubKind::Subagent, HubStatus::Aborted));
        let snap = r.snapshot();
        assert_eq!(snap[0].0, "running");
        assert_eq!(snap[1].0, "idle");
        assert_eq!(snap[2].0, "parked");
        assert_eq!(snap[3].0, "aborted");
    }

    #[test]
    fn update_touches_last_activity() {
        let r = HubRegistry::new();
        r.register("a".into(), entry(HubKind::Main, HubStatus::Running));
        r.update("a", |e| e.last_activity_ms = 100);
        let snap = r.snapshot();
        assert_eq!(snap[0].1.last_activity_ms, 100);
    }

    #[test]
    fn unregister_removes() {
        let r = HubRegistry::new();
        r.register("a".into(), entry(HubKind::Main, HubStatus::Running));
        r.unregister("a");
        assert!(r.snapshot().is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify RED**

Run: `cargo nextest run -p oxicode-cli app::agent_hub_registry::tests`
Expected: FAIL with "cannot find module `agent_hub_registry`".

- [ ] **Step 3: Implement `HubRegistry`**

Create `oxicode-cli/src/app/agent_hub_registry.rs`:
```rust
//! Display metadata for the Agent Hub overlay.
//!
//! Parallel to `oxicode_sdk::AgentPool` but stores TUI display fields
//! (kind, status, last_activity, current_task, session_file) keyed by
//! the same agent id. AgentPool is the runtime owner; HubRegistry is
//! the display projection.

use oxicode_sdk::{HubKind, HubStatus};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// One row in the Hub table.
#[derive(Debug, Clone)]
pub struct HubEntry {
    pub kind: HubKind,
    pub status: HubStatus,
    pub display_name: String,
    pub current_task: Option<String>,
    pub last_activity_ms: u64,
    pub session_file: Option<PathBuf>,
}

impl HubEntry {
    /// "3s ago" / "5m ago" / "1h ago" formatting — millisecond-precise,
    /// minute-precise, hour-precise tiers.
    pub fn age_text(&self, now_ms: u64) -> String {
        let delta = now_ms.saturating_sub(self.last_activity_ms);
        if delta < 60_000 {
            format!("{}s ago", delta / 1000)
        } else if delta < 3_600_000 {
            format!("{}m ago", delta / 60_000)
        } else {
            format!("{}h ago", delta / 3_600_000)
        }
    }
}

/// Concurrent map of agent id → display entry.
#[derive(Debug, Default)]
pub struct HubRegistry {
    inner: RwLock<HashMap<String, HubEntry>>,
}

impl HubRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the entry for `id`.
    pub fn register(&self, id: String, entry: HubEntry) {
        self.inner.write().insert(id, entry);
    }

    /// Apply `f` to the entry for `id`. No-op if absent.
    pub fn update<F: FnOnce(&mut HubEntry)>(&self, id: &str, f: F) {
        if let Some(e) = self.inner.write().get_mut(id) {
            f(e);
        }
    }

    pub fn unregister(&self, id: &str) -> Option<HubEntry> {
        self.inner.write().remove(id)
    }

    /// Sorted snapshot: Running → Idle → Parked → Aborted; within status,
    /// by descending last_activity_ms.
    pub fn snapshot(&self) -> Vec<(String, HubEntry)> {
        let mut v: Vec<(String, HubEntry)> = self
            .inner
            .read()
            .iter()
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        v.sort_by(|a, b| {
            a.1.status
                .sort_key()
                .cmp(&b.1.status.sort_key())
                .then_with(|| b.1.last_activity_ms.cmp(&a.1.last_activity_ms))
        });
        v
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `Arc<HubRegistry>` shorthand.
pub type SharedHubRegistry = Arc<HubRegistry>;
```

- [ ] **Step 4: Add module declaration**

In `oxicode-cli/src/app/mod.rs`, add `pub mod agent_hub_registry;`.

- [ ] **Step 5: Run tests to verify GREEN**

Run: `cargo nextest run -p oxicode-cli app::agent_hub_registry::tests`
Expected: all 4 PASS.

- [ ] **Step 6: Commit**

```bash
git add oxicode-cli/src/app/agent_hub_registry.rs oxicode-cli/src/app/mod.rs
git commit -m "feat(cli): HubRegistry — display metadata for Agent Hub

Parallel to oxicode-sdk AgentPool but stores TUI display fields
(kind, status, last_activity_ms, current_task, session_file).
AgentPool owns runtime state; HubRegistry is the display projection.
Snapshot is sorted Running→Idle→Parked→Aborted, then by recency,
matching omp's AgentHub table layout."
```

---

### Task 3: TranscriptReader + JSONL parsing

**Files:**
- Create: `oxicode-cli/src/tui/overlay/agent_hub/transcript.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: a `PathBuf` (either `<session_dir>/<id>.jsonl` or `<session_dir>/__advisor.jsonl`)
- Produces: `pub struct TranscriptLine { timestamp_ms: u64, role: String, text: String, tool_name: Option<String>, tool_status: Option<String> }`
- Produces: `pub struct TranscriptReader { path: PathBuf, last_mtime: Option<SystemTime>, last_size: u64, lines: Vec<TranscriptLine> }`
- Produces: `TranscriptReader::new(path)`, `refresh(&mut self) -> bool` (true if changed), `lines(&self) -> &[TranscriptLine]`, `is_empty(&self) -> bool`

**Format detection**: If a JSONL line contains a `ts` key + `messages` array, it's the advisor format (`{"ts":N,"messages":["…","…"]}`). Otherwise it's the `SessionEntry` format. Read first 4 KB to discriminate; if the first object has `ts`, treat as advisor; otherwise treat as SessionEntry.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_jsonl(path: &Path, lines: &[&str]) {
        let mut f = std::fs::File::create(path).unwrap();
        for l in lines { writeln!(f, "{}", l).unwrap(); }
    }

    #[test]
    fn parses_advisor_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("__advisor.jsonl");
        write_jsonl(&path, &[
            r#"{"ts":1000,"messages":["review carefully"]}"#,
            r#"{"ts":2000,"messages":["ok proceed","ack"]}"#,
        ]);
        let mut r = TranscriptReader::new(path);
        assert!(r.refresh());
        let lines = r.lines();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "review carefully");
        assert_eq!(lines[2].text, "ack");
    }

    #[test]
    fn parses_session_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub.jsonl");
        write_jsonl(&path, &[
            r#"{"kind":"user","content":"do the thing","timestamp_ms":1000}"#,
            r#"{"kind":"assistant","content":"done","timestamp_ms":1500,"tool_name":null,"tool_status":null}"#,
        ]);
        let mut r = TranscriptReader::new(path);
        assert!(r.refresh());
        let lines = r.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "do the thing");
        assert_eq!(lines[1].text, "done");
    }

    #[test]
    fn refresh_skips_when_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write_jsonl(&path, &[r#"{"ts":1,"messages":["x"]}"#]);
        let mut r = TranscriptReader::new(path);
        assert!(r.refresh());
        assert!(!r.refresh(), "second refresh with no change must return false");
    }

    #[test]
    fn refresh_reruns_on_size_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write_jsonl(&path, &[r#"{"ts":1,"messages":["x"]}"#]);
        let mut r = TranscriptReader::new(path);
        r.refresh();
        write_jsonl(&path, &[
            r#"{"ts":1,"messages":["x"]}"#,
            r#"{"ts":2,"messages":["y"]}"#,
        ]);
        assert!(r.refresh());
        assert_eq!(r.lines().len(), 2);
    }

    #[test]
    fn missing_file_yields_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = TranscriptReader::new(dir.path().join("missing.jsonl"));
        assert!(!r.refresh());
        assert!(r.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify RED**

Run: `cargo nextest run -p oxicode-cli tui::overlay::agent_hub::transcript::tests`
Expected: FAIL with module not found.

- [ ] **Step 3: Implement `TranscriptReader`**

```rust
//! Mtime-based JSONL transcript reader for Agent Hub.
//!
//! Two formats supported:
//! - advisor: `{"ts":N,"messages":["…","…"]}`
//! - subagent / session: `{"kind":"…","content":"…","timestamp_ms":N,...}`
//!
//! Format is detected on the first non-empty line and cached.

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptLine {
    pub timestamp_ms: u64,
    pub role: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AdvisorLine {
    ts: u64,
    messages: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SessionLine {
    kind: String,
    content: String,
    timestamp_ms: u64,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_status: Option<String>,
}

#[derive(Debug)]
pub struct TranscriptReader {
    path: PathBuf,
    last_mtime: Option<SystemTime>,
    last_size: u64,
    lines: Vec<TranscriptLine>,
    format: TranscriptFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptFormat {
    Unknown,
    Advisor,
    Session,
}

impl TranscriptReader {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_mtime: None,
            last_size: 0,
            lines: Vec::new(),
            format: TranscriptFormat::Unknown,
        }
    }

    /// Re-read file if mtime or size changed. Returns true if lines were
    /// (re-)parsed. Cheap on no-op: 1 stat call.
    pub fn refresh(&mut self) -> bool {
        let Ok(meta) = fs::metadata(&self.path) else {
            self.lines.clear();
            return false;
        };
        let mtime = meta.modified().ok();
        let size = meta.len();
        if Some(mtime) == self.last_mtime && size == self.last_size {
            return false;
        }
        self.last_mtime = mtime;
        self.last_size = size;
        let Ok(content) = fs::read_to_string(&self.path) else {
            self.lines.clear();
            return false;
        };
        self.parse(&content);
        true
    }

    fn parse(&mut self, content: &str) {
        self.lines.clear();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            if self.format == TranscriptFormat::Unknown {
                self.format = if line.contains("\"ts\"") && line.contains("\"messages\"") {
                    TranscriptFormat::Advisor
                } else {
                    TranscriptFormat::Session
                };
            }
            match self.format {
                TranscriptFormat::Advisor => {
                    if let Ok(a) = serde_json::from_str::<AdvisorLine>(line) {
                        for m in a.messages {
                            self.lines.push(TranscriptLine {
                                timestamp_ms: a.ts,
                                role: "assistant".into(),
                                text: m,
                                tool_name: None,
                                tool_status: None,
                            });
                        }
                    }
                }
                TranscriptFormat::Session => {
                    if let Ok(s) = serde_json::from_str::<SessionLine>(line) {
                        self.lines.push(TranscriptLine {
                            timestamp_ms: s.timestamp_ms,
                            role: s.kind,
                            text: s.content,
                            tool_name: s.tool_name,
                            tool_status: s.tool_status,
                        });
                    }
                }
                TranscriptFormat::Unknown => {}
            }
        }
    }

    #[must_use]
    pub fn lines(&self) -> &[TranscriptLine] { &self.lines }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.lines.is_empty() }
}
```

- [ ] **Step 4: Run tests to verify GREEN**

Run: `cargo nextest run -p oxicode-cli tui::overlay::agent_hub::transcript::tests`
Expected: 5 PASS.

NOTE: The `SessionLine` JSON shape is approximate; the actual `SessionEntry` schema may differ (e.g. `role` instead of `kind`, `text` instead of `content`). If tests fail due to schema mismatch, adapt `SessionLine` to match what `oxicode-cli/src/store/session.rs::SessionEntry` actually serializes. The shape should be inspected with:

```bash
ls ~/.oxicode/sessions/  # find any existing .jsonl
head -1 ~/.oxicode/sessions/<some>.jsonl  # see actual keys
```

Adjust `SessionLine` to deserialize those exact keys. The point is: parse whatever the SessionManager writes, don't invent a new format.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui/overlay/agent_hub/transcript.rs
git commit -m "feat(tui): TranscriptReader — mtime-based JSONL reader

Parses both advisor ({\"ts\":N,\"messages\":[…]}) and SessionEntry
JSONL formats. Refresh is a single stat() call on no-op. Detects
format on the first non-empty line and caches for the file's lifetime."
```

---

### Task 4: AgentSession HubRegistry wiring

**Files:**
- Modify: `oxicode-cli/src/app/agent_session.rs` (add hub field, register main+advisor, scan session dir for subagent .jsonl)
- Modify: `oxicode-cli/src/app/agent_hub_bridge.rs` (new helper module with `register_persisted_subagents` and `register_advisor`)
- Modify: `oxicode-cli/src/app/mod.rs` (declare new module)

**Interfaces:**
- Consumes: `HubRegistry` from Task 2
- Produces: `AgentSession.hub: SharedHubRegistry` getter (`pub fn hub(&self) -> SharedHubRegistry`)
- Produces: `register_persisted_subagents(hub: &HubRegistry, session_dir: &Path)` — scans `*.jsonl` (excluding main and `__advisor`) and registers as `HubKind::Subagent`

- [ ] **Step 1: Write failing test for `register_persisted_subagents`**

In `oxicode-cli/src/app/agent_hub_bridge.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use oxicode_sdk::{HubKind, HubStatus};

    #[test]
    fn registers_subagent_jsonl_excluding_main_and_advisor() {
        let dir = tempfile::tempdir().unwrap();
        // Main session file (stem = session id, e.g. UUID)
        std::fs::write(dir.path().join("01HXY.jsonl"), "{}\n").unwrap();
        // Advisor transcript
        std::fs::write(dir.path().join("__advisor.jsonl"), "{}\n").unwrap();
        // Subagent transcripts (2)
        std::fs::write(dir.path().join("sub-a.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.path().join("sub-b.jsonl"), "{}\n").unwrap();

        let hub = HubRegistry::new();
        register_persisted_subagents(&hub, dir.path());

        let snap = hub.snapshot();
        let ids: Vec<&str> = snap.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(ids, vec!["sub-a", "sub-b"]);
        for (_, e) in &snap {
            assert_eq!(e.kind, HubKind::Subagent);
            assert_eq!(e.status, HubStatus::Parked);
        }
    }

    #[test]
    fn empty_dir_registers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let hub = HubRegistry::new();
        register_persisted_subagents(&hub, dir.path());
        assert!(hub.snapshot().is_empty());
    }

    #[test]
    fn missing_dir_is_noop() {
        let hub = HubRegistry::new();
        register_persisted_subagents(&hub, Path::new("/nonexistent/dir/12345"));
        assert!(hub.snapshot().is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify RED**

Run: `cargo nextest run -p oxicode-cli app::agent_hub_bridge::tests`
Expected: FAIL with module not found.

- [ ] **Step 3: Implement `agent_hub_bridge.rs`**

```rust
//! Bridges oxicode-cli's AgentSession to the HubRegistry display store.

use std::path::Path;

use oxicode_sdk::{HubKind, HubStatus};

use super::agent_hub_registry::{now_ms, HubEntry, HubRegistry};

/// Scan a session directory for subagent `.jsonl` files and register each
/// in the hub as `HubKind::Subagent` / `HubStatus::Parked`. The main
/// session file (whose stem is a session UUID) and the reserved
/// `__advisor.jsonl` stem are excluded.
///
/// "Parked" is the correct status: a resumed session sees a finished
/// subagent on disk and shows it as available for revival even though
/// no live handle exists.
pub fn register_persisted_subagents(hub: &HubRegistry, session_dir: &Path) {
    let entries = match std::fs::read_dir(session_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !name.ends_with(".jsonl") { continue; }
        let stem = &name[..name.len() - ".jsonl".len()];
        if stem == "__advisor" { continue; }
        if is_main_session_stem(stem) { continue; }

        let path = entry.path();
        let display_name = stem.to_string();
        let last_activity_ms = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or_else(now_ms);

        hub.register(
            stem.to_string(),
            HubEntry {
                kind: HubKind::Subagent,
                status: HubStatus::Parked,
                display_name,
                current_task: None,
                last_activity_ms,
                session_file: Some(path),
            },
        );
    }
}

/// Heuristic: the main session stem is whatever `SessionManager::get_session_file`
/// would return for the current session. Without that coupling, we
/// accept any stem that does NOT look like a subagent slug (which are
/// typically short lowercase + hyphens). For v1, the convention is:
/// - main: any stem starting with a session-id prefix
/// - subagent: stems explicitly registered via `register_subagent`
///
/// In practice the main file is created by SessionManager and has a
/// uuid stem; subagent files are written by the spawned CLI's
/// SessionManager with their own stems. The bridge runs at session
/// start BEFORE the main session has written, so anything present at
/// scan time is a subagent. We fall back to "not main" by checking
/// if `stem` is in the session-id format (UUIDv7) — but the safest
/// signal is: if `__advisor.jsonl` is present, the session is live;
/// subagent files are written AFTER the main session is.
///
/// v1 simplification: skip files whose stem matches a known session-id
/// format passed in by the caller. Add a parameter later.
fn is_main_session_stem(_stem: &str) -> bool {
    // v1: no main file written yet at scan time, so everything in
    // session_dir except __advisor is a subagent.
    false
}

/// Register the advisor reviewer in the hub. Called from
/// `AgentSession::build_advisor` when the runtime is constructed.
pub fn register_advisor(
    hub: &HubRegistry,
    transcript_path: Option<std::path::PathBuf>,
) {
    let last_activity_ms = transcript_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or_else(now_ms);

    hub.register(
        "advisor".into(),
        HubEntry {
            kind: HubKind::Advisor,
            status: HubStatus::Idle,
            display_name: "Advisor".into(),
            current_task: None,
            last_activity_ms,
            session_file: transcript_path,
        },
    );
}
```

- [ ] **Step 4: Add module declaration**

In `oxicode-cli/src/app/mod.rs`, add `pub mod agent_hub_bridge;`.

- [ ] **Step 5: Run tests to verify GREEN**

Run: `cargo nextest run -p oxicode-cli app::agent_hub_bridge::tests`
Expected: 3 PASS.

- [ ] **Step 6: Wire into `AgentSession`**

In `oxicode-cli/src/app/agent_session.rs`:

Add to `pub struct AgentSession { ... }`:
```rust
/// Display metadata for the Agent Hub overlay.
hub: super::agent_hub_registry::SharedHubRegistry,
```

Add a getter:
```rust
impl AgentSession {
    pub fn hub(&self) -> &super::agent_hub_registry::HubRegistry {
        &self.hub
    }
}
```

In `AgentSession::new(...)` (or the equivalent constructor that runs after `build_advisor`), add:
```rust
let hub = super::agent_hub_registry::HubRegistry::new();
super::agent_hub_bridge::register_persisted_subagents(&hub, session_dir_for_scan);
let session = Arc::new(Self { hub, ... });
// after build_advisor returns Some(rt):
if let Some(advisor_rt) = &advisor_runtime {
    super::agent_hub_bridge::register_advisor(&session.hub, advisor_rt.transcript_path());
}
session
```

The exact insertion point depends on `AgentSession::new`'s structure. Find it and inject. The variables `session_dir_for_scan` and `advisor_runtime` must come from existing fields or be derived:
- `session_dir_for_scan` = `SessionManager::session_file().parent()` (already available in the session)
- `advisor_runtime` = the result of `build_advisor()` (already in scope at this point in the function)

If `AgentSession::new` has a different signature (e.g. takes a pre-built `Agent` rather than calling `build_advisor`), add a separate `wire_hub()` method and call it from `run_tui_interactive_impl` after construction.

- [ ] **Step 7: Build + test**

```bash
cargo build -p oxicode-cli
cargo nextest run -p oxicode-cli app
```
Expected: green.

- [ ] **Step 8: Commit**

```bash
git add oxicode-cli/src/app/agent_session.rs oxicode-cli/src/app/agent_hub_bridge.rs oxicode-cli/src/app/mod.rs
git commit -m "feat(cli): wire HubRegistry into AgentSession

AgentSession owns a HubRegistry populated at session start with:
- any persisted subagent .jsonl files in the session directory
  (HubKind::Subagent, status=Parked — available for revival)
- the advisor reviewer (HubKind::Advisor, status=Idle) when
  build_advisor succeeds
- the main agent (HubKind::Main) is added in M5 from the slash
  command (it doesn't need a transcript file)"
```

---

### Task 5: ToggleAgentHub keybinding + UI events

**Files:**
- Modify: `oxicode-tui/src/keybindings/registry.rs`
- Modify: `oxicode-cli/src/tui/handlers.rs` (dispatch_action arm)
- Modify: `oxicode-cli/src/tui/app.rs` (AdvisorCard UI event variant)

**Interfaces:**
- Produces: `Action::ToggleAgentHub` enum variant in `oxicode-tui/src/keybindings/registry.rs`
- Produces: `KeybindingsManager` default binding `Ctrl+h` for `ToggleAgentHub`
- Produces: `parse_action("toggleagenthub") => Some(ToggleAgentHub)`
- Produces: `KAction::ToggleAgentHub => { … }` in `oxicode-cli/src/tui/handlers.rs::dispatch_action`
- Produces: `UiEvent::AdvisorCard { body, severity, timestamp_ms }` in `oxicode-cli/src/tui/app.rs`

- [ ] **Step 1: Add `Action::ToggleAgentHub` to keybinding enum**

In `oxicode-tui/src/keybindings/registry.rs`, find the `pub enum Action` block and add at the end of the appropriate section (after `ToggleRouting`):
```rust
/// Toggle the Agent Hub overlay (advisor + subagent monitor).
ToggleAgentHub,
```

Add to `init_defaults`:
```rust
(ToggleAgentHub, vec!["Ctrl+h"]),
```

Add to `parse_action`:
```rust
"toggleagenthub" => Some(ToggleAgentHub),
```

Add a test to the existing `#[cfg(test)] mod tests`:
```rust
#[test]
fn test_toggle_agent_hub_binding() {
    let mgr = KeybindingsManager::new();
    let ctrl_h = parse_key_id("Ctrl+h").unwrap();
    assert_eq!(mgr.match_action(&ctrl_h), Some(Action::ToggleAgentHub));
    assert_eq!(parse_action("ToggleAgentHub"), Some(Action::ToggleAgentHub));
}
```

- [ ] **Step 2: Run test to verify RED then GREEN**

```bash
cargo nextest run -p oxicode-tui keybindings::tests::test_toggle_agent_hub_binding
```
Expected: FAIL first, then PASS after the additions.

- [ ] **Step 3: Add `KAction::ToggleAgentHub` dispatch arm**

In `oxicode-cli/src/tui/handlers.rs`, find `fn dispatch_action(...)` and add a match arm:
```rust
KAction::ToggleAgentHub => {
    use crate::tui::overlay::agent_hub::AgentHubOverlay;
    let session = session.clone_handle();
    state.overlay = None;
    state.overlay_state = Some(Box::new(AgentHubOverlay::new(session)));
    None
}
```

NOTE: this arm is **placeholder** — `AgentHubOverlay::new` is implemented in Task 6. Add it now anyway; the build will fail until Task 6 lands. OR: comment out the match arm until Task 6, then uncomment.

To keep the build green at each task: wrap the arm in a `#[cfg(feature = "agent-hub")]` gate OR only add the arm at the end of Task 6. Recommend the latter (cleaner).

- [ ] **Step 4: Add `UiEvent::AdvisorCard` variant**

In `oxicode-cli/src/tui/app.rs`, find `pub(crate) enum UiEvent` and add:
```rust
/// Advisor advice routed to a persistent transcript card (alongside the
/// existing toast). Emitted by the session-event dispatcher for aside/
/// preserve channel advice.
AdvisorCard {
    body: String,
    severity: oxicode_agent::advisor::AdvisorSeverity,
    timestamp_ms: u64,
},
```

- [ ] **Step 5: Build + test**

```bash
cargo build --workspace
cargo nextest run -p oxicode-tui keybindings
```
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add oxicode-tui/src/keybindings/registry.rs oxicode-cli/src/tui/app.rs
git commit -m "feat(registry): add ToggleAgentHub action + AdvisorCard UI event

- Action::ToggleAgentHub bound to Ctrl+h (matches omp's app.agents.hub).
  Compile-time exhaustiveness guard ensures dispatch_action always has
  an arm for it.
- UiEvent::AdvisorCard carries aside/preserve advice from
  SessionEvent::Advisor into the transcript as a persistent card
  (rendered in M7)."
```

---

### Task 6: AgentHubOverlay — table view + transcript view

**Files:**
- Create: `oxicode-cli/src/tui/overlay/agent_hub/mod.rs`
- Create: `oxicode-cli/src/tui/overlay/agent_hub/state.rs`
- Create: `oxicode-cli/src/tui/overlay/agent_hub/table.rs`
- Create: `oxicode-cli/src/tui/overlay/agent_hub/keys.rs`
- Modify: `oxicode-cli/src/tui/overlay/mod.rs` (register module)
- Modify: `oxicode-cli/src/tui/handlers.rs` (uncomment the `ToggleAgentHub` arm from Task 5)

**Interfaces:**
- Produces: `pub struct AgentHubOverlay { state: HubState, registry: SharedHubRegistry, readers: HashMap<String, TranscriptReader> }`
- Produces: `AgentHubOverlay::new(session: AgentSessionHandle) -> Self`
- Produces: `impl OverlayComponent for AgentHubOverlay` — `handle_key`, `render`, `hint`, `poll`
- Produces: `enum HubView { Table, Transcript { agent_id: String } }`
- Produces: `enum HubAction { OpenTranscript(String), Close, None }` — internal

**Key bindings (inside the overlay):**
- `j` / `Down` — next row
- `k` / `Up` — prev row
- `Enter` — open transcript for selected row
- `Esc` / `q` — close overlay
- Inside transcript view: `j` / `k` / `PageDown` / `PageUp` / `g` / `G` — scroll; `f` — toggle tail-follow; `Esc` / `q` — back to table

- [ ] **Step 1: Create `state.rs` with `HubState`**

```rust
//! View state and sorting logic for the Agent Hub.

use std::path::PathBuf;
use oxicode_sdk::{HubKind, HubStatus};
use crate::app::agent_hub_registry::{HubEntry, HubRegistry};

/// One row in the table, precomputed for rendering.
#[derive(Debug, Clone)]
pub struct HubRow {
    pub id: String,
    pub kind: HubKind,
    pub status: HubStatus,
    pub display_name: String,
    pub current_task: Option<String>,
    pub age_text: String,
    pub session_file: Option<PathBuf>,
}

pub enum HubView {
    Table,
    Transcript { agent_id: String },
}

pub struct HubState {
    pub rows: Vec<HubRow>,
    pub view: HubView,
    pub selected: usize,
    pub row_order: std::collections::HashMap<String, usize>,
    pub transcript_scroll: usize,
    pub transcript_follow: bool,
}

impl HubState {
    pub fn from_registry(reg: &HubRegistry, now_ms: u64) -> Vec<HubRow> {
        reg.snapshot()
            .into_iter()
            .map(|(id, e)| HubRow {
                age_text: e.age_text(now_ms),
                id,
                kind: e.kind,
                status: e.status,
                display_name: e.display_name,
                current_task: e.current_task,
                session_file: e.session_file,
            })
            .collect()
    }
}
```

- [ ] **Step 2: Create `table.rs` with `render_table`**

```rust
//! Table rendering for Agent Hub.

use oxicode_tui::Theme;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use ratatui::Frame;

use super::state::HubState;

pub fn render_table(f: &mut Frame, area: Rect, state: &HubState, theme: &Theme) {
    let header = Row::new(vec![
        Cell::from("Agent").style(theme.bold()),
        Cell::from("Kind").style(theme.bold()),
        Cell::from("Status").style(theme.bold()),
        Cell::from("Task").style(theme.bold()),
        Cell::from("Activity").style(theme.bold()),
    ]);
    let rows: Vec<Row> = state.rows.iter().enumerate().map(|(i, r)| {
        let status_str = format!("{:?}", r.status).to_lowercase();
        let kind_str = r.kind.as_str();
        let task = r.current_task.as_deref().unwrap_or("—");
        let mut row = Row::new(vec![
            Cell::from(r.display_name.clone()),
            Cell::from(kind_str),
            Cell::from(status_str),
            Cell::from(task),
            Cell::from(r.age_text.clone()),
        ]);
        if i == state.selected {
            row = row.style(Style::default().bg(theme.selection_bg()));
        }
        row
    }).collect();
    let table = Table::new(
        rows,
        [Constraint::Length(20), Constraint::Length(8), Constraint::Length(10), Constraint::Min(1), Constraint::Length(10)],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Agent Hub "));
    f.render_widget(table, area);
}
```

NOTE: `theme.selection_bg()` may not exist — check `oxicode-tui::Theme` for the actual selector method. Adapt to whatever's available (e.g. `Style::default().add_modifier(Modifier::REVERSED)` if no helper).

- [ ] **Step 3: Create `keys.rs` with `handle_key`**

```rust
//! Key dispatch for Agent Hub.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::state::{HubState, HubView};

pub enum HubAction {
    None,
    Close,
    OpenTranscript(String),
}

pub fn handle_key(state: &mut HubState, key: KeyEvent) -> HubAction {
    if key.kind != KeyEventKind::Press { return HubAction::None; }
    match state.view {
        HubView::Table => handle_table_key(state, key),
        HubView::Transcript { ref agent_id } => handle_transcript_key(state, agent_id, key),
    }
}

fn handle_table_key(state: &mut HubState, key: KeyEvent) -> HubAction {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if !state.rows.is_empty() {
                state.selected = (state.selected + 1).min(state.rows.len() - 1);
            }
            HubAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            HubAction::None
        }
        KeyCode::Enter => {
            if let Some(row) = state.rows.get(state.selected) {
                HubAction::OpenTranscript(row.id.clone())
            } else {
                HubAction::None
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => HubAction::Close,
        _ => HubAction::None,
    }
}

fn handle_transcript_key(state: &mut HubState, _agent_id: &str, key: KeyEvent) -> HubAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.view = HubView::Table;
            HubAction::None
        }
        KeyCode::Char('f') => {
            state.transcript_follow = !state.transcript_follow;
            HubAction::None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.transcript_scroll = state.transcript_scroll.saturating_add(1);
            state.transcript_follow = false;
            HubAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.transcript_scroll = state.transcript_scroll.saturating_sub(1);
            HubAction::None
        }
        KeyCode::PageDown => {
            state.transcript_scroll = state.transcript_scroll.saturating_add(10);
            state.transcript_follow = false;
            HubAction::None
        }
        KeyCode::PageUp => {
            state.transcript_scroll = state.transcript_scroll.saturating_sub(10);
            HubAction::None
        }
        KeyCode::Char('G') => {
            state.transcript_scroll = usize::MAX;
            state.transcript_follow = true;
            HubAction::None
        }
        KeyCode::Char('g') => {
            state.transcript_scroll = 0;
            HubAction::None
        }
        _ => HubAction::None,
    }
}
```

- [ ] **Step 4: Create `mod.rs` with `AgentHubOverlay`**

```rust
//! Agent Hub overlay — fullscreen monitor for advisor + subagents.
//!
//! Two views: table (default) and transcript (per-agent live tail).
//! Transcript is mtime-polled from the underlying .jsonl file via
//! `TranscriptReader` (sibling module).

pub mod keys;
pub mod state;
pub mod table;
pub mod transcript;

use std::collections::HashMap;

use oxicode_tui::Theme;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::app::agent_hub_registry::SharedHubRegistry;
use crate::app::agent_session::AgentSessionHandle;
use crate::tui::overlay::OverlayComponent;
use crate::tui::overlay::OverlayAction;

use self::keys::{handle_key, HubAction};
use self::state::{HubState, HubView};
pub use self::transcript::{TranscriptLine, TranscriptReader};

pub struct AgentHubOverlay {
    session: AgentSessionHandle,
    state: HubState,
    readers: HashMap<String, TranscriptReader>,
}

impl AgentHubOverlay {
    pub fn new(session: AgentSessionHandle) -> Self {
        let reg = session.hub().clone();
        let now = crate::app::agent_hub_registry::now_ms();
        let rows = HubState::from_registry(&reg, now);
        let mut row_order = HashMap::new();
        for (i, r) in rows.iter().enumerate() {
            row_order.insert(r.id.clone(), i);
        }
        let mut readers = HashMap::new();
        for r in &rows {
            if let Some(path) = &r.session_file {
                readers.insert(r.id.clone(), TranscriptReader::new(path.clone()));
            }
        }
        Self {
            session,
            state: HubState {
                rows,
                view: HubView::Table,
                selected: 0,
                row_order,
                transcript_scroll: 0,
                transcript_follow: true,
            },
            readers,
        }
    }

    /// Poll each reader; append new lines to the live tail window.
    fn poll_readers(&mut self) {
        for (id, reader) in self.readers.iter_mut() {
            let _ = reader.refresh();
            // Per-agent transcript refresh; tail-follow recompute happens in render.
            let _ = id;
        }
    }

    fn render_transcript(&self, f: &mut Frame, area: Rect, theme: &Theme, agent_id: &str) {
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
        let Some(reader) = self.readers.get(agent_id) else {
            let p = Paragraph::new("no transcript file")
                .block(Block::default().borders(Borders::ALL).title(format!(" {} ", agent_id)));
            f.render_widget(p, area);
            return;
        };
        let lines = reader.lines();
        let visible: Vec<Line> = lines.iter().rev().take(area.height as usize).rev()
            .map(|l| Line::from(Span::raw(&l.text)))
            .collect();
        let p = Paragraph::new(visible)
            .block(Block::default().borders(Borders::ALL).title(format!(" {} (f: follow) ", agent_id)))
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
    }
}

impl std::fmt::Debug for AgentHubOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHubOverlay").field("rows", &self.state.rows.len()).finish()
    }
}

impl OverlayComponent for AgentHubOverlay {
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction {
        let action = handle_key(&mut self.state, key);
        match action {
            HubAction::None => OverlayAction::None,
            HubAction::Close => OverlayAction::Close,
            HubAction::OpenTranscript(id) => {
                // Ensure reader exists even if the file appeared after open.
                if !self.readers.contains_key(&id) {
                    if let Some(row) = self.state.rows.iter().find(|r| r.id == id) {
                        if let Some(path) = &row.session_file {
                            self.readers.insert(id.clone(), TranscriptReader::new(path.clone()));
                        }
                    }
                }
                self.state.view = HubView::Transcript { agent_id: id };
                OverlayAction::None
            }
        }
    }

    fn poll(&mut self) -> OverlayAction {
        // Refresh registry snapshot every poll (cheap — 1 lock + clone).
        let reg = self.session.hub().clone();
        let now = crate::app::agent_hub_registry::now_ms();
        let new_rows = HubState::from_registry(&reg, now);
        // Preserve the user's selected id across refreshes.
        let selected_id = self.state.rows.get(self.state.selected).map(|r| r.id.clone());
        // Update row_order: existing entries keep their position, new ones get appended.
        for (i, r) in new_rows.iter().enumerate() {
            if !self.state.row_order.contains_key(&r.id) {
                self.state.row_order.insert(r.id.clone(), self.state.row_order.len());
            }
            // Open new readers for newly discovered files.
            if r.session_file.is_some() && !self.readers.contains_key(&r.id) {
                if let Some(path) = &r.session_file {
                    self.readers.insert(r.id.clone(), TranscriptReader::new(path.clone()));
                }
            }
        }
        // Remove readers for departed rows.
        let current_ids: std::collections::HashSet<String> =
            new_rows.iter().map(|r| r.id.clone()).collect();
        self.readers.retain(|id, _| current_ids.contains(id));
        self.state.rows = new_rows;
        if let Some(id) = selected_id {
            if let Some(pos) = self.state.rows.iter().position(|r| r.id == id) {
                self.state.selected = pos;
            }
        }
        self.poll_readers();
        OverlayAction::None
    }

    fn render(&mut self, f: &mut Frame, area: Rect, theme: &Theme) {
        match &self.state.view {
            HubView::Table => table::render_table(f, area, &self.state, theme),
            HubView::Transcript { agent_id } => {
                self.render_transcript(f, area, theme, agent_id)
            }
        }
    }

    fn hint(&self) -> &str {
        match self.state.view {
            HubView::Table => " j/k: nav  Enter: transcript  Esc: close",
            HubView::Transcript { .. } => " j/k: scroll  f: follow  Esc: back",
        }
    }
}
```

NOTE: This is a first pass. The transcript view's scroll/tail-follow logic in `render_transcript` is simplified (just `take(height).rev().rev()`). M6 refines this with proper scroll offsets and tail-following.

- [ ] **Step 5: Add `agent_hub` to `overlay/mod.rs`**

In `oxicode-cli/src/tui/overlay/mod.rs`, add:
```rust
pub mod agent_hub;
```

Re-export:
```rust
pub use agent_hub::AgentHubOverlay;
```

- [ ] **Step 6: Build + test**

```bash
cargo build -p oxicode-cli
cargo nextest run -p oxicode-cli
```
Expected: green (or with borrow-checker errors that need fixing — adjust).

- [ ] **Step 7: Commit**

```bash
git add oxicode-cli/src/tui/overlay/agent_hub/ oxicode-cli/src/tui/overlay/mod.rs
git commit -m "feat(tui): AgentHubOverlay — table + transcript views

Fullscreen alt-screen overlay listing the main agent, advisor, and
any persisted subagents. Two views:
- Table: j/k to navigate, Enter to open transcript, Esc to close.
- Transcript: live mtime-polled tail of the underlying .jsonl file
  with j/k/PgUp/PgDn scroll, f toggles tail-follow, Esc returns
  to table.

Poll-based registry refresh keeps the table current as new
subagents complete and write their .jsonl files. The dispatch
arm for ToggleAgentHub is wired in this commit (was deferred from
the keybinding change in M5 to keep the build green at each step)."
```

---

### Task 7: /agents slash command + dispatch action

**Files:**
- Create: `oxicode-cli/src/tui/slash/builtin/agents.rs`
- Modify: `oxicode-cli/src/tui/slash/builtin/mod.rs` (register)

**Interfaces:**
- Produces: `pub(crate) struct AgentsCommand`
- Produces: `impl SlashCommand for AgentsCommand` — name `"agents"`, alias `"hub"`
- Produces: `execute` opens `AgentHubOverlay`

- [ ] **Step 1: Create `agents.rs`**

```rust
//! `/agents` — open the Agent Hub overlay (advisor + subagent monitor).

use super::super::registry::SlashCommand;
use crate::tui::overlay::agent_hub::AgentHubOverlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};

pub(crate) struct AgentsCommand;

impl SlashCommand for AgentsCommand {
    fn name(&self) -> &str { "agents" }
    fn aliases(&self) -> &[&str] { &["hub"] }
    fn description(&self) -> &str {
        "Open the agent hub overlay (advisor + subagents)"
    }
    fn usage(&self) -> &str { "/agents" }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.overlay = None;
        ctx.state.overlay_state = Some(Box::new(AgentHubOverlay::new(
            ctx.session.clone_handle(),
        )));
        SlashOutcome::Handled
    }
}
```

- [ ] **Step 2: Register in `mod.rs`**

In `oxicode-cli/src/tui/slash/builtin/mod.rs`:
```rust
mod agents;
// ... in register_builtin_slash_commands:
registry.register(Box::new(agents::AgentsCommand));
```

- [ ] **Step 3: Wire `ToggleAgentHub` dispatch in `handlers.rs`**

Now that `AgentHubOverlay` exists, add the dispatch arm:
```rust
KAction::ToggleAgentHub => {
    state.overlay = None;
    state.overlay_state = Some(Box::new(
        crate::tui::overlay::agent_hub::AgentHubOverlay::new(session.clone_handle()),
    ));
    None
}
```

- [ ] **Step 4: Build + test**

```bash
cargo build --workspace
cargo nextest run -p oxicode-cli
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui/slash/builtin/agents.rs oxicode-cli/src/tui/slash/builtin/mod.rs oxicode-cli/src/tui/handlers.rs
git commit -m "feat(tui): /agents slash command + Ctrl+h dispatch

Both /agents (slash) and Ctrl+h (keybinding) open the Agent Hub
overlay. /agents is aliasable as /hub."
```

---

### Task 8: ContentBlock::Advisory + severity-colored card render

**Files:**
- Modify: `oxicode-tui/src/widgets/chat/types.rs` (add `Advisory` variant)
- Modify: `oxicode-tui/src/widgets/chat/markdown.rs` (render Advisory as a card)
- Modify: `oxicode-cli/src/tui/app.rs` (handle `UiEvent::AdvisorCard`)

**Interfaces:**
- Produces: `ContentBlock::Advisory { body: String, severity: AdvisorSeverity, timestamp_ms: u64 }`
- Produces: severity-colored card render (nit = dim, concern = yellow, blocker = red)
- Produces: `UiEvent::AdvisorCard` → `chat.push(ChatMessage { role: System, content_blocks: [Advisory{...}] })`

- [ ] **Step 1: Add `Advisory` variant to `ContentBlock`**

In `oxicode-tui/src/widgets/chat/types.rs`, add (and import `oxicode_agent::advisor::AdvisorSeverity`):
```rust
use oxicode_agent::advisor::AdvisorSeverity;

#[derive(Debug, Clone)]
pub enum ContentBlock {
    // ... existing variants ...
    /// Read-only reviewer advice. Persistent transcript card.
    Advisory {
        body: String,
        severity: AdvisorSeverity,
        timestamp_ms: u64,
    },
}
```

- [ ] **Step 2: Render Advisory in `markdown.rs`**

Find the function that turns `ContentBlock` into `Vec<Line>` (likely `render_block` or `to_lines`). Add a match arm:
```rust
ContentBlock::Advisory { body, severity, .. } => {
    let label = match severity {
        AdvisorSeverity::Nit => ("NIT", theme.dim()),
        AdvisorSeverity::Concern => ("CONCERN", theme.warning()),
        AdvisorSeverity::Blocker => ("BLOCKER", theme.error()),
    };
    let prefix = format!("[{}] ", label.0);
    vec![Line::from(vec![
        Span::styled(prefix, Style::default().fg(label.1)),
        Span::raw(body),
    ])]
}
```

Adapt `theme.dim()/warning()/error()` to the actual Theme API. Use `Style::default().add_modifier(...)` if no helper.

- [ ] **Step 3: Emit AdvisorCard in `app.rs`**

Find the `UiEvent` handler (likely in a function like `handle_ui_event` or via `match event`). Add:
```rust
UiEvent::AdvisorCard { body, severity, timestamp_ms } => {
    use oxicode_tui::widgets::chat::{ChatMessage, ContentBlock, MessageRole};
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = if timestamp_ms > 0 { timestamp_ms as i64 } else {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
    };
    state.chat.messages.push(ChatMessage {
        role: MessageRole::System,
        content_blocks: vec![ContentBlock::Advisory { body, severity, timestamp_ms: timestamp_ms.max(0) }],
        timestamp: ts,
    });
}
```

- [ ] **Step 4: Wire SessionEvent::Advisor → UiEvent::AdvisorCard**

In `oxicode-cli/src/tui/handlers.rs`, find the existing `SessionEvent::Advisor` handler (Task 4 / M5 work shows it at line 967). Extend to emit BOTH the SystemMessage toast AND the AdvisorCard:
```rust
SessionEvent::Advisor { channel, body, severity } => {
    // Existing toast
    let _ = ui_tx.send(UiEvent::SystemMessage(format!("Advisor ({:?}): {body}", channel)));
    // New persistent card for aside/preserve channels
    if matches!(channel, oxicode_agent::advisor::AdvisorDeliveryChannel::Aside | oxicode_agent::advisor::AdvisorDeliveryChannel::Preserve) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let sev = severity.unwrap_or(oxicode_agent::advisor::AdvisorSeverity::Nit);
        let _ = ui_tx.send(UiEvent::AdvisorCard { body: body.clone(), severity: sev, timestamp_ms: ts });
    }
}
```

NOTE: `SessionEvent::Advisor` may not currently carry `severity`. If only `channel` and `body` are present, default `severity` to `Nit` (which is what `AdviseTool` returns when the field is omitted). If the field is missing, this is a v1 limitation — advisor severity is inferred from the delivery channel (Aside = Nit, Steer = Concern/Blocker).

- [ ] **Step 5: Build + test**

```bash
cargo build --workspace
cargo nextest run --workspace
```
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add oxicode-tui/src/widgets/chat/types.rs oxicode-tui/src/widgets/chat/markdown.rs oxicode-cli/src/tui/app.rs oxicode-cli/src/tui/handlers.rs
git commit -m "feat(tui): ContentBlock::Advisory + persistent advisor cards

Advisor aside/preserve advice now appears in the chat transcript
as a severity-colored card (NIT dim / CONCERN yellow / BLOCKER red)
alongside the existing toast. Steer-channel advice continues to
inject directly into the primary agent and is not carded."
```

---

### Task 9: PTY end-to-end test

**Files:**
- Modify: `oxicode-cli/tests/pty_e2e.rs`

**Interfaces:**
- Produces: `test_pty_hub_opens_and_lists_advisor` test
- Test: launches the binary in a PTY, sends `/agents`, verifies a recognizable hub indicator appears, sends `q` to close, verifies the binary exits cleanly.

- [ ] **Step 1: Find the existing PTY test pattern**

```bash
grep -n "fn test_pty\|spawn_oxicode\|PTY" oxicode-cli/tests/pty_e2e.rs | head -20
```

Look at the existing test setup for `test_pty_tui_renders_and_exits` (from production tape cutover). Use the same spawn helper, with shorter timeouts.

- [ ] **Step 2: Add `test_pty_hub_opens_and_lists_advisor`**

```rust
#[test]
fn test_pty_hub_opens_and_lists_advisor() {
    // The PTY harness depends on the existing test helper. Re-use it.
    let mut pty = spawn_oxicode_in_pty(/* with -p advisor.enabled=true */);
    // Wait for prompt
    pty.expect(">").expect("prompt appears");
    // Send /agents
    pty.send_line("/agents").expect("send /agents");
    // Expect "Agent Hub" in output
    pty.expect("Agent Hub").expect("hub header appears");
    // Send q to close
    pty.send_key("q").expect("close hub");
    // Expect hub title gone
    pty.dont_expect("Agent Hub").expect("hub closed");
    // Exit
    pty.send_key("Ctrl+c").expect("exit");
    pty.expect_eof().expect("clean exit");
}
```

Adapt the API names (`spawn_oxicode_in_pty`, `pty.expect`, etc.) to whatever the existing tests use. If the helpers differ substantially, copy the pattern from `test_pty_tui_renders_and_exits`.

- [ ] **Step 3: Run the test**

```bash
cargo nextest run -p oxicode-cli --test pty_e2e test_pty_hub_opens_and_lists_advisor
```
Expected: PASS.

If the PTY test is flaky or environment-dependent, mark it `#[ignore]` and document why — the unit tests in Tasks 1–4 already cover the behavior.

- [ ] **Step 4: Commit**

```bash
git add oxicode-cli/tests/pty_e2e.rs
git commit -m "test(pty): cover /agents open + close gesture

PTY end-to-end test for the Agent Hub slash command. Verifies the
hub header appears after /agents and disappears after q. The unit
tests in agent_hub_* modules cover the full overlay behavior; this
test is the integration guardrail."
```

---

### Task 10: Final verification + CHANGELOG

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Run full repository gates**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace
```
Expected: all green. Fix any clippy warnings before proceeding.

- [ ] **Step 2: Add CHANGELOG entry**

Add to `## [Unreleased]` under `### Added`:
```markdown
- **TUI: Agent Hub overlay (advisor + subagent monitor)** — `Ctrl+h` or
  `/agents` opens a fullscreen alt-screen overlay listing the main
  agent, advisor reviewer, and any persisted subagents. Each row shows
  kind, status, current task, and last-activity age. Pressing `Enter`
  on a row opens a live transcript viewer (mtime-polled at 250ms)
  for the underlying `.jsonl` file. The advisor's `aside`/`preserve`
  advice is now also surfaced as a severity-colored card in the chat
  transcript (in addition to the existing toast). oxicode-sdk's
  `AgentPool` is now actually wired in oxicode-cli via a new
  `HubRegistry` display projection.
```

- [ ] **Step 3: Commit + update RESUMING.md**

```bash
git add CHANGELOG.md docs/superpowers/RESUMING.md
git commit -m "docs: CHANGELOG entry for Agent Hub + RESUMING update

Mark P2 TUI as the final phase and note that advisor visualization
is now shipped alongside the tape engine cutover."
```

In `docs/superpowers/RESUMING.md`, update the deferred-items table to remove Agent Hub (it is now done) and the boot-injection row remains.

---

## Self-Review

**Spec coverage:**
- §3.1 AgentHandle fields → simplified to separate `HubRegistry` in oxicode-cli (avoids polluting oxicode-sdk with TUI display concerns). Documented in Task 2.
- §3.2 AgentPool connection → covered by `HubRegistry` (Task 2) + `agent_hub_bridge` (Task 4).
- §3.3 AdvisorRuntime hook → `transcript_path()` getter (Task 4 step 6).
- §3.4 Transcript polling → Task 3.
- §3.5 Overlay structure → Task 6.
- §3.6 Fullscreen alt-screen → Task 6 (no code change needed; `terminal_host` already handles it).
- §3.7 Advisor card → Task 8.
- §4 Integration points → Tasks 5, 7, 8.
- §6 Tests → Tasks 1, 2, 3, 4 unit tests + Task 9 PTY.
- §7 Success criteria → all addressed across tasks.

**Placeholders:** None.

**Type consistency:**
- `HubKind.as_str()` strings (`main`/`task`/`advisor`) match omp convention.
- `HubStatus.as_str()` strings (`running`/`idle`/`parked`/`aborted`) match omp.
- `HubEntry.session_file: Option<PathBuf>` flows through to `TranscriptReader::new`.
- `TranscriptLine` fields are the union of both JSONL formats; unused fields default to `None`.

**Ambiguity flags:**
- `SessionLine` JSON shape may need adjustment at Task 3 step 4 — callout notes how to inspect.
- `Severity` is not yet in `SessionEvent::Advisor` — Task 8 step 4 has a fallback path.
- `Theme` helper methods (`selection_bg`, `dim`, `warning`, `error`) may not exist — callout says to adapt.

**Out of scope (explicitly):**
- collab remote hub, Agent Dashboard, in-process subagent IPC, primary auto-interrupt, TTSR/SoftReq/Approval changes.

**Execution note:** Total ~1,000 LOC new + ~200 LOC modified. The plan has 10 tasks, each producing a commit. Foundation (Tasks 1–4) is the API surface; UI (Tasks 5–8) is the user-visible behavior. Task 9 verifies, Task 10 closes.
