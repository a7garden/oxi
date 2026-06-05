# v0.27 Browser Observability Integration

> **Status:** Design (handoff spec — implementation is the next critical path)
> **Author:** oxibrowser team (in collaboration with oxi)
> **Scope:** oxi-agent + oxi-sdk
> **Estimated effort:** ~250 LoC + tests. About 1–2 days.

---

## 0. TL;DR

`oxibrowser-core 0.12.x` is shipped and exposes a stable observability API. The pieces that need to land **in oxi** to close the chain end-to-end:

1. **oxi-agent** — replace the single-slot `ProgressForwarder` with per-`tab_id` routing, and switch the `ProgressCallback` signature from `Fn(String)` to `Fn(oxibrowser_core::BrowserEvent)`. Then populate `tab_id` on `AgentEvent::ToolExecutionUpdate` (currently always `None`).
2. **oxi-sdk** — fix a pre-existing 5-error compile break (NOT introduced by this work) so v0.27.2 can ship.

Once these land, `oxios-kernel 1.0.4` and `oxios-web 1.0.4` (already implemented in oxios) will activate end-to-end automatically — no oxios code change required. Their tab-id badges will start rendering because `tab_id` will finally be non-`None`.

**oxibrowser itself is done** — this document is the handoff spec for the oxi side.

---

## 1. Wire contract from `oxibrowser-core` (frozen for 0.12.x)

This is the public surface we depend on. It is **stable** for the entire 0.12.x line. If you need to evolve it, that's a v0.13 RFC, not a v0.12.x change.

### 1.1 `BrowserEvent` (current, since `oxibrowser-core 0.12.1`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrowserEvent {
    NavigationStarted { tab_id: Uuid, url: String },
    WaitingForSelector { tab_id: Uuid, selector: String, timeout_ms: u64 },
    DocumentReady {
        tab_id: Uuid,
        final_url: String,
        title: String,
        status: u16,
        total_bytes: u64,
        js_script_count: usize,
        total_duration: Duration,
    },
    ScreenshotCaptured { tab_id: Uuid, bytes: usize, viewport_width: u32, duration: Duration },
}
```

**Rules for consumers:**

- The enum is `#[non_exhaustive]`. Match arms must have a wildcard.
- Adding a field to an existing variant is a breaking change for `oxibrowser-core`. Don't pin above 0.12.x speculatively.
- `tab_id` is `#[serde(default = "Uuid::nil")]` on each variant. JSON payloads from older 0.12.0 clients (which don't emit `tab_id`) deserialize with `tab_id = Uuid::nil()`. Use this as a sentinel: `Uuid::nil()` means "no tab info".

### 1.2 Subscription

```rust
let mut events: tokio::sync::broadcast::Receiver<BrowserEvent> = browser.subscribe_events();
```

- 32-slot buffer, oldest-dropped on overflow.
- Multiple subscribers supported.
- `RecvError::Lagged(n)` → log and continue. Don't treat as fatal.
- `RecvError::Closed` → the `Browser` was dropped; exit the drain loop.

### 1.3 Tab identity

`oxibrowser_core::Tab` now has a `tab_id(&self) -> Uuid` getter (since 0.12.1). The id is stable for the lifetime of a `Tab` and is shared across `Tab::clone()`. Use this to identify which tab an event came from.

### 1.4 What oxibrowser deliberately does NOT emit

Do not design oxi around these — they will not arrive:

- Sub-resource fetches (CSS, image, font)
- DNS / TLS / connection / redirect events
- Per-script execution events
- JS console output / errors
- Cookie / storage mutations

These are high-level state transitions only. The full list and rationale is in `oxibrowser/docs/designs/2026-06-04-oxibrowser-observability.md` §1.

---

## 2. oxi-agent — the per-tab routing refactor (this is the main work)

> **Files:** `oxi-agent/src/tools/browse/{engine.rs, oxibrowser_backend.rs, browse_tool.rs}`, `oxi-agent/src/tools.rs`, `oxi-agent/src/agent_loop/tool_exec.rs`
> **Bump:** `oxi-agent` 0.27.2 → 0.27.3, `oxi-sdk` 0.27.1 → 0.27.2

### 2.1 What shipped in v0.27.2 (and why it's not enough)

`09f0176 feat(agent): add tab_id to AgentEvent::ToolExecutionUpdate` added the field on the agent-loop event, but its construction site looks like:

```rust
emit_clone(AgentEvent::ToolExecutionUpdate {
    tool_call_id: tool_call_id_clone.clone(),
    tool_name: tool_name.clone(),
    partial_result: msg,
    tab_id: None,            // <-- always None
});
```

The reason: `oxi-agent/src/tools.rs:138` declares:

```rust
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;
```

The callback receives only the short label string, not the structured `BrowserEvent`. The agent loop has no way to recover `tab_id` at emit time.

To keep the simple `Fn(String)` API viable, `4c1c7e1 fix(agent): force BrowseTool to run sequentially` was added as defense-in-depth. This serialises tool calls but doesn't fix the underlying single-slot forwarder — it just hides the symptom.

### 2.2 The fix in five steps

#### Step A — change the callback signature

`oxi-agent/src/tools.rs:138`:

```rust
// OLD
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;

// NEW
pub type ProgressCallback = Arc<dyn Fn(oxibrowser_core::BrowserEvent) + Send + Sync>;
```

This is the only place in `oxi-agent` that updates the type alias. All `set` / `clear` / `invoke` calls go through `ProgressForwarder` (now `TabCallbackRegistry`).

#### Step B — replace `ProgressForwarder` with `TabCallbackRegistry`

`oxi-agent/src/tools/browse/engine.rs` — replace the single-slot `Mutex<Option<ProgressCallback>>` with a per-`tab_id` map:

```rust
pub struct TabCallbackRegistry {
    callbacks: parking_lot::Mutex<HashMap<Uuid, ProgressCallback>>,
}

impl TabCallbackRegistry {
    pub fn new() -> Self { Self { callbacks: Mutex::new(HashMap::new()) } }
    pub fn set(&self, tab_id: Uuid, cb: ProgressCallback) {
        self.callbacks.lock().insert(tab_id, cb);
    }
    pub fn clear(&self, tab_id: &Uuid) {
        self.callbacks.lock().remove(tab_id);
    }
    pub fn invoke(&self, tab_id: &Uuid, event: oxibrowser_core::BrowserEvent) {
        if let Some(cb) = self.callbacks.lock().get(tab_id).cloned() {
            cb(event);
        }
    }
    pub fn is_set(&self, tab_id: &Uuid) -> bool {
        self.callbacks.lock().contains_key(tab_id)
    }
    pub fn len(&self) -> usize { self.callbacks.lock().len() }
}
```

Update the `BrowserEngine` trait's default `progress_forwarder` to return a fresh empty registry.

Delete the old `ProgressForwarder` (or keep as a deprecated alias — your call; I'd delete for cleanliness).

#### Step C — route by `tab_id` in `OxiBrowserEngine`

`oxi-agent/src/tools/browse/oxibrowser_backend.rs`:

The background drain task currently does:

```rust
while let Ok(event) = events_rx.recv().await {
    registry_clone.invoke(event.short_label());  // WRONG: routes to the only callback
}
```

Change it to route by `event.tab_id`:

```rust
while let Ok(event) = events_rx.recv().await {
    let tab_id = match &event {
        BrowserEvent::NavigationStarted { tab_id, .. }
        | BrowserEvent::WaitingForSelector { tab_id, .. }
        | BrowserEvent::DocumentReady { tab_id, .. }
        | BrowserEvent::ScreenshotCaptured { tab_id, .. } => *tab_id,
    };
    registry_clone.invoke(&tab_id, event);
}
```

(Or extract the helper as `fn event_tab_id(event: &BrowserEvent) -> Uuid`. Match the style of the surrounding code.)

Add `tab_id: Uuid` and `registry: Arc<TabCallbackRegistry>` fields to `OxiTab`:

```rust
pub struct OxiTab {
    inner: oxibrowser_core::Tab,
    config: BrowseConfig,
    tab_id: Uuid,                              // from inner.tab_id()
    registry: Arc<TabCallbackRegistry>,        // shared with the engine
}

impl OxiTab {
    pub fn new(inner: oxibrowser_core::Tab, config: BrowseConfig, registry: Arc<TabCallbackRegistry>) -> Self {
        let tab_id = inner.tab_id();
        Self { inner, config, tab_id, registry }
    }

    pub fn set_progress_callback(&self, cb: ProgressCallback) {
        self.registry.set(self.tab_id, cb);
    }

    pub fn clear_progress_callback(&self) {
        self.registry.clear(&self.tab_id);
    }
}
```

In `BrowserEngine::new_tab` impl, pass the registry:

```rust
async fn new_tab(&self) -> Result<Box<dyn BrowserTabTrait>, BrowserError> {
    let tab = self.browser.new_tab().await?;
    Ok(Box::new(OxiTab::new(tab, self.config.clone(), Arc::clone(&self.registry))))
}
```

#### Step D — register per-tab from `BrowseTool`

`oxi-agent/src/tools/browse/browse_tool.rs`:

`on_progress` is called by the agent loop *before* `execute` — at that point the `OxiTab` doesn't exist yet. Store the callback on the tool; in `execute`, when the tab is opened, register the callback on the tab. `TabGuard` (or `guard.close().await`) clears the callback on close.

```rust
pub struct BrowseTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
    pending_callback: parking_lot::Mutex<Option<ProgressCallback>>,
}

impl AgentTool for BrowseTool {
    fn on_progress(&self, callback: ProgressCallback) {
        *self.pending_callback.lock() = Some(callback);
    }

    async fn execute(&self, ...) -> Result<AgentToolResult, ToolError> {
        let raw_tab = self.engine.new_tab().await?;
        if let Some(cb) = self.pending_callback.lock().take() {
            raw_tab.set_progress_callback(cb);
        }
        let guard = TabGuard::new(raw_tab);
        // ... existing body unchanged ...
        guard.close().await;   // TabGuard::close should call clear_progress_callback
    }
}
```

#### Step E — populate `tab_id` on the agent event

`oxi-agent/src/agent_loop/tool_exec.rs:441`:

The callback now receives the full `BrowserEvent`. Capture `tab_id` in the closure:

```rust
let progress_cb: Arc<dyn Fn(oxibrowser_core::BrowserEvent) + Send + Sync> =
    Arc::new(move |event: oxibrowser_core::BrowserEvent| {
        emit_clone(AgentEvent::ToolExecutionUpdate {
            tool_call_id: tool_call_id_clone.clone(),
            tool_name: tool_name.clone(),
            partial_result: event.short_label(),
            tab_id: Some(event.tab_id),
        });
    });
tool.on_progress(progress_cb);
```

### 2.3 Lift the `SequentialOnly` workaround?

After this lands, `BrowseTool::execution_mode` is *safe* to return `ParallelSafe` again — per-tab routing prevents the race. **But**: keep it as `SequentialOnly` for now unless there's a concrete need for parallel tabs. The right time to lift it is when a real use case (multi-tab scraping in one tool call) needs it. Document the decision either way.

### 2.4 Tests to add

Unit tests in `oxi-agent/src/tools/browse/engine.rs`:

1. `tab_callback_registry_set_and_invoke` — register cb for A, invoke for A → fires; invoke for B → no fire.
2. `tab_callback_registry_set_replaces_per_tab` — register cb_A for A and cb_B for B, invoke for A → only A fires.
3. `tab_callback_registry_clear` — clear A, invoke for A → no fire.
4. `tab_callback_registry_default_is_empty` — `new()` has 0 callbacks.

Integration test in `oxi-agent/src/tools/browse/oxibrowser_backend.rs`:

5. `engine_routes_events_by_tab_id_concurrent` — open two tabs in one engine, register two callbacks, navigate each. Assert each callback fires only for its own tab's events.

In `oxi-agent/src/agent_loop/tool_exec.rs` (or wherever `ToolExecutionUpdate` is tested):

6. Verify `tab_id` is populated when a callback fires.

### 2.5 Local test infrastructure

`oxibrowser-core 0.12.1` is not yet on crates.io at the time of writing. While testing locally, add a temporary `[patch.crates-io]` to the oxi workspace's root `Cargo.toml`:

```toml
[patch.crates-io]
oxibrowser-core = { path = "/path/to/oxibrowser/crates/oxibrowser-core" }
```

**Remove this before commit.** The commit should be clean.

### 2.6 Versioning

- `oxi-agent/Cargo.toml`: `0.27.2` → `0.27.3`
- `oxi-sdk/Cargo.toml`: `0.27.1` → `0.27.2`

The `ProgressCallback` signature change is a breaking change for any direct consumer. Bump the minor to signal it.

### 2.7 CHANGELOG

Add a new `## [0.27.3] - YYYY-MM-DD` section (or batch under `[Unreleased]`):

```markdown
### Added — oxi-agent

- Per-`tab_id` `TabCallbackRegistry` replaces the single-slot `ProgressForwarder`.
  Concurrent `BrowseTool` calls (each with their own tab) are now routed correctly.

### Changed — oxi-agent (BREAKING)

- `oxi_agent::tools::ProgressCallback` signature changed from
  `Arc<dyn Fn(String) + Send + Sync>` to
  `Arc<dyn Fn(oxibrowser_core::BrowserEvent) + Send + Sync>`.
  Direct consumers must update their callback type.

### Fixed — oxi-agent

- `AgentEvent::ToolExecutionUpdate.tab_id` is now populated (no longer always `None`).
  Required `oxibrowser-core >= 0.12.1` for the upstream `BrowserEvent::tab_id` field.
```

---

## 3. oxi-sdk — unblock the publish

> **Tracking:** pre-existing issue, NOT introduced by this work. Has been failing on `main` since before this initiative.

### 3.1 The breakage

`cargo publish --dry-run -p oxi-sdk` fails with 5 errors:

```
error[E0053]: method `resolve_provider` has an incompatible type for trait
error[E0053]: method `resolve_model` has an incompatible type for trait
error[E0053]: method `resolve_provider` has an incompatible type for trait
error[E0053]: method `resolve_model` has an incompatible type for trait
error[E0308]: mismatched types
```

`oxi-sdk` compiles fine via `cargo build --workspace` because the resolver uses the local path. The publish dry run uses the **tarball** (the `Cargo.toml` of the published crate + its declared `oxi-agent = "0.27.x"` dependency). The resolved version of `oxi-agent` from crates.io has a different `resolve_provider` / `resolve_model` signature than the local source.

### 3.2 Root cause (to be confirmed by the oxi team)

Two possibilities, in order of likelihood:

1. **`oxi-sdk` was published with `oxi-agent = "0.27"`** but the latest `oxi-agent` on crates.io has a different method signature than the local source. The local source has a `pub fn` while the published version had a different shape. Fix: align the local source to the published API (or vice versa).
2. **`oxi-sdk`'s `Cargo.toml` declares a more permissive version range** than what's actually used locally. Tightening the range to `"=0.27.3"` would force the resolver to pick the local version.

The oxi team should run `git log --oneline -- oxi-sdk/src/ports/` and `oxi-agent/src/agent/ports/` to find the last commit that touched these signatures and see which side is the source of truth.

### 3.3 Verification

Until this is fixed, `oxi-sdk 0.27.x` cannot be published, which blocks the oxios v1.0.4 publish chain (see §4). Resolve it before continuing the publish flow.

---

## 4. Cross-project publish order

After §2 and §3 land, the chain is:

```
1. oxibrowser-core 0.12.1      (already shipped to git, just needs publish)
2. oxi-sdk 0.27.2              (after §3 compile fix)
3. oxi-agent 0.27.3            (after §2 per-tab routing)
4. oxios — workspace dep bump to oxi-sdk 0.27    (oxios team)
5. oxios-kernel 1.0.4
6. oxios-web 1.0.4
```

### 4.1 Commands for the oxi side

```bash
# 1. oxibrowser-core 0.12.1 (oxibrowser repo — out of scope for oxi session)
cd /path/to/oxibrowser
cargo test -p oxibrowser-core
cargo publish -p oxibrowser-core

# 2. oxi-sdk 0.27.2 (oxi repo — publish BEFORE oxi-agent)
cd /path/to/oxi
cargo test -p oxi-sdk --features native-browser 2>/dev/null
cargo publish -p oxi-sdk

# 3. oxi-agent 0.27.3 (oxi repo)
cargo test -p oxi-agent --features native-browser
cargo publish -p oxi-agent
```

Steps 4-6 are the oxios team's work and live in `oxios/docs/designs/` (or wherever they track this).

### 4.2 Known pre-existing issues during local testing

- `oxi-sdk 0.27.2` publish dry run will fail (the §3 issue) until the compile break is fixed.
- `oxi-agent 0.27.3` publish needs the `oxibrowser-core` patch in place (§2.5). The patch is local-only.
- 10 pre-existing test failures in `browse_script_tool` (YAML parsing) on `oxi` main. They fail before and after this work — not blockers for the publish, but they'll show up in `cargo test -p oxi-agent` output. Note them; don't try to fix in this PR.

---

## 5. Open questions for the oxi team

These aren't blocking, but the answers shape the v0.13 RFC. Defer to follow-up issues:

1. **Lift `SequentialOnly` on `BrowseTool`?** Now that per-tab routing prevents the race, parallel `BrowseTool::execute` calls are safe. Don't lift speculatively — wait for a real multi-tab use case.

2. **Persist `ToolExecutionProgress` to the session log?** Currently ephemeral. The trajectory step schema would need a new `progress_fragments: Vec<String>` field. Probably defer to v0.13.

3. **oxi-sdk re-exports of `BrowserEvent` and `tab_id`-related types?** Currently `oxi-sdk` re-exports `AgentEvent` and `BrowserEvent`. If a downstream consumer wants to construct `BrowserEvent` themselves (e.g. a custom `BrowserEngine` impl), they may want access to the `tab_id` field's type. Currently they get it transitively via `oxibrowser_core`. Document this dependency clearly in the `oxi-sdk` README.

---

## 6. File / commit index for reference

| Repo | Branch | Commits added in this initiative |
|------|--------|---------------------------------|
| oxibrowser | `main` (at `b8ae314`) | `493f419` v0.12.0, `66a06c1` CHANGELOG, `2222a86` v0.12.1, `b8ae314` handoff doc |
| oxi | `main` (at `6d3f5ac`) | `4c1c7e1` SequentialOnly, `09f0176` tab_id field |
| oxios | `main` (at `b03baec`) | `7ef6d4c` kernel, `c66f764` web backend, `b03baec` web frontend + tsc, `56938a1` tsc fix (96→0) |

The `oxi-per-tab-forwarder` worktree (a partial refactor that hit a rate limit) was reset; the field addition in `09f0176` was preserved but the per-tab registry refactor is **not** committed yet. The full design is in §2 of this document.

---

## 7. Contact

- **oxibrowser maintainer**: see `oxibrowser/CODEOWNERS`
- **oxi maintainer**: a7garden (`oxi/CODEOWNERS`)
- **oxios maintainer**: see `oxios/CODEOWNERS`

The wire contract (§1) is frozen for the 0.12.x line. Any proposed change is a v0.13 RFC.

For context on the full design (and the rationale for why the architecture is `oxios → oxi-sdk → oxi-agent → oxibrowser-core` rather than `oxios → oxibrowser-core` directly), see the original v0.12 observability design in the oxibrowser repo:

> `oxibrowser/docs/designs/2026-06-04-oxibrowser-observability.md`

The short version: the agent owns the browser, oxios consumes agent events, and the `String` indirection that lost structured data was an implementation bug — not an architectural flaw. Fixing the callback signature (§2) keeps the architecture intact and unblocks full data flow.
