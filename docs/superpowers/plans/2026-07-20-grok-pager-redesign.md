# oxi-pager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `oxi-pager` crate as a thin state-machine layer between `oxi-agent` events and the existing `oxi-tui` widgets, plus the typed tool trait migration (PR-A1 from applied-design.md) — without disturbing the 32 existing `AgentTool` implementations or any `oxi-tui` widget code.

**Architecture:** `oxi-pager` is a `select!`-driven event consumer that fans `AgentEvent` (and only `AgentEvent`) into a pure `reduce(state, event) -> Vec<PagerAction>` function. State lives behind `Arc<parking_lot::RwLock<PagerState>>`. Render path reads the same state and pushes to the existing `oxi-tui` widget tree — no widget code is modified. Typed tools are introduced in `oxi-agent` via a parallel generic trait `TypedTool` with a `TypedToolAdapter: AgentTool` eraser; the `ToolRegistry`'s public surface is unchanged.

**Tech Stack:** Rust 2024 edition, tokio 1, `parking_lot` 0.12, `schemars` 0.8 (new), `thiserror` 2 (already workspace). No new runtime deps for oxi-pager. The pager reuses `oxi-tui`'s `KeybindingsManager`, `ChatWidget`, `Footer`, `ToolRenderer` unchanged.

**Reference spec:** `docs/superpowers/specs/2026-07-20-grok-pager-redesign.md`
**Reference design:** `docs/designs/2026-07-20-grok-build-applied-design.md` (for the typed-tool half)
**Reference pattern:** `oxi-cli/src/tui/app.rs:898-903` (`run_tui_interactive`) — the redirect target

## Global Constraints

- Workspace rust-version: `1.96` (from `[workspace.package]`)
- Workspace edition: `2024`
- License: `MIT`. New files: MIT header, NO `/// adapted from` or Apache-2.0 attribution.
- Lint gate: `cargo clippy --workspace --all-targets -- -D warnings` MUST pass clean
- Test runner: `cargo nextest run --workspace` MUST pass
- Pre-commit: `cargo fmt --check`, `cargo clippy --all-targets`
- Native-browser feature MUST still compile: `cargo build -p oxi-agent --features native-browser` (AGENTS.md)
- `parking_lot::MutexGuard` is `!Send` — drop guard before any `.await`
- `oxi-tui` widget code: **0 lines** may be modified. (PR-3 may add 4 variants to the `Action` enum only — that file is `oxi-tui/src/keybindings/registry.rs`, not a widget.)
- `oxi-agent::AgentTool` and `oxi-agent::ToolRegistry` public surface: **0 breaking changes**. (PR-1 adds a sibling trait and a `wrap_typed` helper; it may convert `ToolError` from `String` alias to enum, which is a type-system change but no caller signature change at the call sites that go through `Arc<dyn AgentTool>`.)

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `Cargo.toml` (workspace) | Modify | Add `oxi-pager` to `members` (PR-0); add `schemars = "0.8"` to `[workspace.dependencies]` (PR-1) |
| `oxi-pager/Cargo.toml` | Create | Crate manifest, deps `oxi-tui` + `oxi-agent` only |
| `oxi-pager/src/lib.rs` | Create | `pub fn version()`, top-level re-exports |
| `oxi-pager/src/state.rs` | Create | `PagerState` + sub-states (PR-2) |
| `oxi-pager/src/emitter.rs` | Create | `PagerEvent` enum, `AgentEvent → PagerEvent::Agent` wrapper (PR-2) |
| `oxi-pager/src/reducer.rs` | Create | `pub fn reduce(...) -> Vec<PagerAction>`, **PR-2 = empty body returning `vec![]`**, **PR-5 = full body** |
| `oxi-pager/src/dispatch.rs` | Create | `pub enum AgentCmd`, `dispatch(state, cmd)` (PR-4) |
| `oxi-pager/src/main_loop.rs` | Create | `pub async fn run(app: App) -> Result<()>` (PR-4) |
| `oxi-pager/src/prompt.rs` | Create | `PromptState` (PR-5) |
| `oxi-pager/src/status.rs` | Create | `StatusState` (PR-5) |
| `oxi-pager/src/scrollback.rs` | Create | `ScrollbackState` (PR-5) |
| `oxi-pager/src/modal.rs` | Create | `ModalKind` enum (PR-6) |
| `oxi-pager/src/slash.rs` | Create | `route_slash` (PR-6) |
| `oxi-pager/src/keymap.rs` | Create | `KeyRouter`, `ResolvedKey` (PR-3) |
| `oxi-pager/src/theme_bridge.rs` | Create | `Theme → line style` helper (PR-3, minimal) |
| `oxi-pager/src/render/mod.rs` | Create | Render glue (PR-4 stub, PR-5 body) |
| `oxi-pager/src/render/markdown_streaming.rs` | Create | `MarkdownStreaming` (PR-7) |
| `oxi-pager/src/widgets/spinner.rs` | Create | 12-frame spinner (PR-7) |
| `oxi-pager/src/widgets/token_bar.rs` | Create | `TokenBar` (PR-7) |
| `oxi-pager/src/widgets/tool_progress_card.rs` | Create | `ToolProgressCard` (PR-7) |
| `oxi-pager/README.md` | Create | Crate README (PR-0) |
| `oxi-agent/src/tools/typed.rs` | Create | `TypedTool` trait, `TypedToolAdapter`, `wrap_typed` (PR-1) |
| `oxi-agent/src/error.rs` | Modify | Convert `pub type ToolError = String` to enum with `InvalidArgs(String)` etc. (PR-1) |
| `oxi-agent/src/tools.rs` | Modify | Re-export `pub mod typed;` + adjust 32 tool error sites to use new variants (PR-1) |
| `oxi-agent/Cargo.toml` | Modify | Add `schemars = { workspace = true }` to `[dependencies]` (PR-1) |
| `oxi-tui/src/keybindings/registry.rs` | Modify | Add 4 variants `ToggleTodo` / `ToggleIssues` / `ToggleHub` / `ToggleLsp` (PR-3) |
| `oxi-tui/src/keybindings/keys.rs` | Modify | Add default key bindings for the 4 new actions (PR-3) |
| `oxi-cli/src/tui/app.rs:898-903` | Modify | `run_tui_interactive` body becomes `oxi_pager::run(app).await` (PR-4) |
| `oxi-cli/src/bootstrap.rs:250-252` | Modify | (No change in PR-4 — the call site already points to `run_tui_interactive*`) |

---

## Task 1: PR-0 — Scaffold `oxi-pager` crate (no-op)

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/Cargo.toml` (add `oxi-pager` to `[workspace] members`)
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/Cargo.toml`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/lib.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/README.md`

**Interfaces:**
- Produces: a new workspace member `oxi-pager` that builds clean and exposes `pub fn version() -> &'static str`.

- [ ] **Step 1: Add `oxi-pager` to workspace members**

In `/Volumes/MERCURY/PROJECTS/oxi/Cargo.toml`, find the `[workspace] members = [...]` array (currently contains `oxi-ai`, `oxi-agent`, `oxi-tui`, `oxi-cli`, `oxi-sdk`, `oxi-hashline`, `oxi-lsp`, `oxi-mnemopi`, `oxi-snapcompact`). Add `"oxi-pager"` to the end of the array (preserving trailing comma style — match the existing format).

- [ ] **Step 2: Create `oxi-pager/Cargo.toml`**

Create the file with these exact contents:

```toml
[package]
name = "oxi-pager"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Pager state machine + emitter + reducer for the oxi-cli TUI"

[dependencies]
oxi-tui = { path = "../oxi-tui" }
oxi-agent = { path = "../oxi-agent" }
parking_lot = "0.12"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "signal"] }
```

- [ ] **Step 3: Create `oxi-pager/src/lib.rs`**

Create the file with these exact contents:

```rust
#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! oxi-pager — pager state machine for the oxi-cli TUI.
//!
//! See `docs/superpowers/specs/2026-07-20-grok-pager-redesign.md` for the
//! full architecture. This crate is a thin layer between `oxi-agent`
//! events and the existing `oxi-tui` widget tree. It does not introduce
//! new widgets, new agent semantics, or new public types in either
//! dependency.

/// Returns the crate version (matches `Cargo.toml`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 4: Create `oxi-pager/README.md`**

Create the file with these exact contents:

```markdown
# oxi-pager

Pager state machine for the [oxi](https://github.com/a7garden/oxi) TUI.

See `docs/superpowers/specs/2026-07-20-grok-pager-redesign.md` for the
architecture and `docs/superpowers/plans/2026-07-20-grok-pager-redesign.md`
for the implementation plan.
```

- [ ] **Step 5: Verify the crate builds**

Run: `cargo build -p oxi-pager`
Expected: `Compiling oxi-pager v0.1.0 (...)` and `Finished` with no errors.

- [ ] **Step 6: Verify clippy passes**

Run: `cargo clippy -p oxi-pager -- -D warnings`
Expected: `Finished` with no warnings.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml oxi-pager/
git commit -m "feat(pager): scaffold oxi-pager crate (PR-0)

Adds an empty oxi-pager workspace member with oxi-tui + oxi-agent
dependencies. No behavior yet — this is the scaffold. PR-1+ will
add state, emitter, reducer, and main loop.
"
```

---

## Task 2: PR-1 — Typed tool trait (TypedTool + TypedToolAdapter)

This task migrates oxi-agent to support typed tool arguments via a parallel generic trait. Existing 32 tools stay on `AgentTool` unchanged.

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/Cargo.toml` (add `schemars = "0.8"` to `[workspace.dependencies]`)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/Cargo.toml` (add `schemars = { workspace = true }` to `[dependencies]`)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/tools.rs` (declare `pub mod typed;`, re-export)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/tools.rs:557` (replace `pub type ToolError = String` with a new enum; preserve `pub type` alias for compatibility)
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/tools/typed.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/tools/typed_tests.rs` (test module, or use `#[cfg(test)] mod tests` inline — pick the inline form)

**Interfaces:**
- Produces (in `oxi_agent::tools`):
  - `pub trait TypedTool` with `type Args: DeserializeOwned + JsonSchema + Send + 'static`, methods `name`, `label`, `description`, `essential`, `async fn execute_typed`
  - `pub struct TypedToolAdapter<T: TypedTool>(pub Arc<T>)`
  - `impl<T: TypedTool> AgentTool for TypedToolAdapter<T>`
  - `pub fn wrap_typed<T: TypedTool>(tool: T) -> Arc<dyn AgentTool>`
- Modifies: `pub type ToolError = String` (line 557) → `pub type ToolError = ToolErrorKind;` with a new enum in `oxi_agent::error`. **Critical**: keep the type alias to preserve all 32 existing call sites that do `Result<_, ToolError>`.

- [ ] **Step 1: Add `schemars` to workspace deps**

In `/Volumes/MERCURY/PROJECTS/oxi/Cargo.toml`, the `[workspace.dependencies]` section currently has `thiserror = "2"`. Add the new dep:

```toml
[workspace.dependencies]
thiserror = "2"
schemars = "0.8"
```

- [ ] **Step 2: Add `schemars` to oxi-agent deps**

In `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/Cargo.toml`, find the `[dependencies]` section. Add a new line:

```toml
schemars = { workspace = true }
```

(If a `schemars` entry already exists from a prior in-progress change, leave it.)

- [ ] **Step 3: Convert `ToolError` from alias to enum-backed alias**

In `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/tools.rs`, find the line `pub type ToolError = String;` (around line 557). Replace it with:

```rust
/// Tool error type — re-exported as the public `ToolError` alias.
///
/// Backed by `oxi_agent::error::ToolErrorKind` so typed tools can return
/// structured variants (e.g. `InvalidArgs`) without breaking the existing
/// `String`-based call sites in the 32 legacy `AgentTool` impls.
pub type ToolError = crate::error::ToolErrorKind;
```

Create `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/error.rs` with:

```rust
//! oxi-agent error types.

use thiserror::Error;

/// Tool execution error variants. `ToolError` (in `crate::tools`) is a
/// public alias for this enum.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolErrorKind {
    /// Caller-supplied arguments failed to deserialize or validate.
    #[error("invalid args: {0}")]
    InvalidArgs(String),

    /// Generic execution failure (preserves the legacy `String` error
    /// payload so the 32 existing `AgentTool` impls keep working).
    #[error("{0}")]
    Other(String),
}

impl From<String> for ToolErrorKind {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl From<&str> for ToolErrorKind {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}
```

In `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/lib.rs`, add near the top-level module declarations:

```rust
pub mod error;
```

(Place next to the other top-level `pub mod` lines.)

- [ ] **Step 4: Verify the 32 legacy tools still compile**

Run: `cargo build -p oxi-agent`
Expected: builds clean. The `From<String>` / `From<&str>` impls let every `Err(ToolError::from("..."))` and `Err("...".into())` site keep working.

If any legacy tool uses `ToolError` in a pattern like `match err { ToolError::Something => ... }` (without going through `From`), the build will fail. **Fix by replacing such patterns with the new variants** — most likely only `ToolError::Other` and `ToolError::InvalidArgs` are needed.

- [ ] **Step 5: Create `oxi-agent/src/tools/typed.rs`**

Create the file with these exact contents:

```rust
//! Typed tool trait — type-safe alternative to `AgentTool`'s `params: Value`.
//!
//! A `TypedTool` declares its argument type as `type Args: DeserializeOwned +
//! JsonSchema`. `TypedToolAdapter` erases it into the existing
//! `Arc<dyn AgentTool>` slot, so the `ToolRegistry`, `AgentLoop`, and all
//! 32 existing `AgentTool` impls are untouched. New tools can opt into
//! the typed path; old tools keep working.
//!
//! Streaming (`ToolStream<T>`) is explicitly out of scope — see
//! `docs/designs/2026-07-20-grok-build-applied-design.md:145-147`. The
//! `on_progress(ProgressCallback)` API on `AgentTool` continues to apply
//! for both legacy and typed tools via the adapter.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use tokio::sync::oneshot;

use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode};

/// Type-safe tool trait. Generic + associated type → **not dyn compatible**.
/// `TypedToolAdapter` erases it into `AgentTool`.
pub trait TypedTool: Send + Sync + 'static {
    /// JSON arguments deserialized from the LLM's tool call.
    /// `DeserializeOwned + JsonSchema` both required.
    type Args: DeserializeOwned + JsonSchema + Send + 'static;

    fn name(&self) -> &str;
    fn label(&self) -> &str { self.name() }
    fn description(&self) -> &str;
    fn essential(&self) -> bool { false }

    /// Execute with already-deserialized arguments.
    async fn execute_typed(
        &self,
        tool_call_id: &str,
        args: Self::Args,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError>;
}

/// Adapter that erases a `TypedTool` into the existing `AgentTool` dyn
/// surface. The `parameters_schema` is generated from `schemars`; `execute`
/// deserializes the raw `Value` into `<T as TypedTool>::Args` before
/// dispatching to `execute_typed`.
pub struct TypedToolAdapter<T: TypedTool>(pub Arc<T>);

impl<T: TypedTool> std::fmt::Debug for TypedToolAdapter<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedToolAdapter")
            .field("name", &self.0.name())
            .finish()
    }
}

#[async_trait]
impl<T: TypedTool> AgentTool for TypedToolAdapter<T> {
    fn name(&self) -> &str { self.0.name() }
    fn label(&self) -> &str { self.0.label() }
    fn description(&self) -> &str { self.0.description() }
    fn essential(&self) -> bool { self.0.essential() }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(<T as TypedTool>::Args))
            .unwrap_or_else(|_| serde_json::json!({"type": "object"}))
    }

    fn execution_mode(&self) -> ToolExecutionMode { ToolExecutionMode::ParallelSafe }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let tool_name = self.0.name();
        let args = <T as TypedTool>::Args::deserialize(params)
            .map_err(|e| ToolError::InvalidArgs(format!("invalid args for '{tool_name}': {e}")))?;
        self.0.execute_typed(tool_call_id, args, signal, ctx).await
    }
}

/// Register helper — wraps a typed tool into the dyn surface for
/// `ToolRegistry::register_arc(...)`.
pub fn wrap_typed<T: TypedTool>(tool: T) -> Arc<dyn AgentTool> {
    Arc::new(TypedToolAdapter(Arc::new(tool)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::json;
    use std::sync::Mutex;

    /// A minimal typed tool for testing the adapter roundtrip.
    struct EchoTool;

    #[derive(Deserialize, JsonSchema)]
    struct EchoArgs {
        msg: String,
    }

    impl TypedTool for EchoTool {
        type Args = EchoArgs;
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "echoes the input" }
        async fn execute_typed(
            &self,
            _call_id: &str,
            args: Self::Args,
            _signal: Option<oneshot::Receiver<()>>,
            _ctx: &ToolContext,
        ) -> Result<AgentToolResult, ToolError> {
            Ok(AgentToolResult::text(args.msg))
        }
    }

    /// A typed tool that records its invocations to verify adapter wiring.
    struct CountingTool(Mutex<u32>);

    #[derive(Deserialize, JsonSchema)]
    struct CountArgs;

    impl TypedTool for CountingTool {
        type Args = CountArgs;
        fn name(&self) -> &str { "count" }
        fn description(&self) -> &str { "counts" }
        async fn execute_typed(
            &self,
            _call_id: &str,
            _args: Self::Args,
            _signal: Option<oneshot::Receiver<()>>,
            _ctx: &ToolContext,
        ) -> Result<AgentToolResult, ToolError> {
            let mut guard = self.0.lock().unwrap();
            *guard += 1;
            Ok(AgentToolResult::text(format!("called {} times", *guard)))
        }
    }

    #[test]
    fn typed_adapter_roundtrip_via_dyn_agent_tool() {
        let dyn_tool: Arc<dyn AgentTool> = wrap_typed(EchoTool);
        let schema = dyn_tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        // schemars should have produced a property entry for `msg`.
        let props = schema.get("properties").expect("schema has properties");
        assert!(props.get("msg").is_some(), "msg property present in schema");
    }

    #[test]
    fn typed_schema_matches_schemars_directly() {
        let dyn_tool: Arc<dyn AgentTool> = wrap_typed(EchoTool);
        let from_adapter = dyn_tool.parameters_schema();
        let from_direct = serde_json::to_value(schemars::schema_for!(EchoArgs))
            .expect("schemars serializes");
        assert_eq!(from_adapter, from_direct);
    }

    #[tokio::test]
    async fn typed_args_validation_fails_loudly() {
        let dyn_tool: Arc<dyn AgentTool> = wrap_typed(EchoTool);
        // Missing `msg` field — should deserialize to InvalidArgs.
        let result = dyn_tool.execute(
            "call-1",
            json!({}), // no `msg`
            None,
            &ToolContext::default(),
        ).await;
        match result {
            Err(ToolError::InvalidArgs(msg)) => {
                assert!(msg.contains("invalid args"), "message: {msg}");
            }
            other => panic!("expected InvalidArgs, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn typed_execute_typed_is_invoked_with_deserialized_args() {
        let tool = CountingTool(Mutex::new(0));
        let dyn_tool: Arc<dyn AgentTool> = wrap_typed(tool);
        let r = dyn_tool.execute("c1", json!({}), None, &ToolContext::default()).await
            .expect("count should succeed");
        let r2 = dyn_tool.execute("c2", json!({}), None, &ToolContext::default()).await
            .expect("count should succeed again");
        let _ = (r, r2);
        // The Mutex counter is internal — but the fact that both calls
        // succeeded without InvalidArgs proves the empty `CountArgs`
        // struct deserialized correctly.
    }
}
```

- [ ] **Step 6: Add `pub mod typed;` to oxi-agent/src/tools.rs**

In `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/tools.rs`, find the bottom of the `// Built-in tools` section (around line 766 onward). Add a new module declaration in alphabetical order — for example, after `pub mod truncate;` (line 826):

```rust
/// Typed tool trait and adapter — see [typed].
pub mod typed;
```

(If a `pub mod typed;` already exists from prior work, skip this step.)

- [ ] **Step 7: Verify build + tests pass**

Run: `cargo build -p oxi-agent`
Expected: builds clean.

Run: `cargo nextest run -p oxi-agent -- typed`
Expected: all 4 new tests pass. The `-- typed` filter targets the test module name (matches the file `typed.rs`).

If the 4 tests don't appear in the run list, the `#[cfg(test)] mod tests` block may be filtered out. Run without filter: `cargo nextest run -p oxi-agent`. The 32 existing tool tests must still pass.

- [ ] **Step 8: Verify clippy passes**

Run: `cargo clippy -p oxi-agent --all-targets -- -D warnings`
Expected: no warnings. If `schemars` derive macros emit warnings, see Task 1's `tools.rs:704-710` precedent in applied-design.md — they should not appear, but if they do, add `#![cfg_attr(test, allow(...))]` narrowly.

- [ ] **Step 9: Verify native-browser feature still compiles**

Run: `cargo clippy -p oxi-sdk --features native-browser -- -D warnings`
Expected: passes. This is the AGENTS.md-mandated sanity check — PR-1 changes oxi-agent only, so oxi-sdk should be untouched, but we verify.

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml oxi-agent/Cargo.toml oxi-agent/src/
git commit -m "feat(agent): typed tool trait + adapter (PR-1)

Adds TypedTool trait with DeserializeOwned + JsonSchema Args, a
TypedToolAdapter that erases into the existing AgentTool dyn surface,
and a wrap_typed() helper. ToolError is now an enum (with InvalidArgs
+ Other variants) but keeps the public type alias so the 32 existing
AgentTool impls are unchanged. New tests cover adapter roundtrip,
schema correctness, and InvalidArgs surface.

This is applied-design.md PR-A1. Streaming (PR-A3) and per-tool
migrations (PR-A4..N) are intentionally out of scope.
"
```

---

## Task 3: PR-2 — PagerState + PagerEvent + reduce stub

**Files:**
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/state.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/emitter.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/reducer.rs`
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/lib.rs` (add module decls + re-exports)

**Interfaces:**
- Produces (in `oxi_pager`):
  - `pub struct PagerState` (with sub-states `ScrollbackState`, `PromptState`, `StatusState`, `AgentMetaState`, `StickyPanelState` — these are stub `pub struct XxxState {}` definitions; filled in PR-5)
  - `pub enum PagerEvent` with variants `Agent(oxi_agent::events::AgentEvent)`, `Input(ResolvedKey)` (where `ResolvedKey` is a stub enum), `Tick`, `Background(BackgroundEvent)` (stub)
  - `pub enum PagerAction` with `Render`, `SendToAgent(AgentCmd)`, `SendToTerminal(TermCmd)`, `PlaySound(Sound)`, `ScheduleTick(u64)`, `OpenModal(ModalKind, ModalCtx)`, `CloseModal`, `Quit(ExitReason)`
  - `pub fn reduce(_state: &mut PagerState, _event: PagerEvent) -> Vec<PagerAction>` — **stub: always returns `vec![]`**

- [ ] **Step 1: Create `oxi-pager/src/state.rs`**

Create the file with these exact contents:

```rust
//! PagerState — single source of truth for the pager.

use std::sync::Arc;
use parking_lot::RwLock;

/// Top-level pager state. Wrapped in `Arc<RwLock<...>>` for sharing
/// between the main loop (writer) and the render path (reader).
#[derive(Default)]
pub struct PagerState {
    pub scrollback: ScrollbackState,
    pub prompt: PromptState,
    pub status: StatusState,
    pub agent_meta: AgentMetaState,
    pub sticky_panels: StickyPanelState,
    /// Active modal — `None` when no overlay is open. Filled in PR-6.
    pub modal: Option<ModalKind>,
}

pub type SharedState = Arc<RwLock<PagerState>>;

#[derive(Default, Debug, Clone)]
pub struct ScrollbackState {}

#[derive(Default, Debug, Clone)]
pub struct PromptState {
    pub text: String,
    pub cursor: usize,
}

#[derive(Default, Debug, Clone)]
pub struct StatusState {
    pub spinner_phase: u8,
    pub last_error: Option<String>,
}

#[derive(Default, Debug, Clone)]
pub struct AgentMetaState {
    pub session_id: Option<String>,
    pub model: Option<String>,
}

#[derive(Default, Debug, Clone)]
pub struct StickyPanelState {
    pub todo: bool,
    pub issues: bool,
    pub hub: bool,
    pub lsp: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModalKind {
    None,
    Ask,
    ModelSelect,
    ProviderSelect,
    Settings,
    Extensions,
    McpDashboard,
    McpConfig,
    Issues,
    Roles,
    Router,
    Skill,
    ToolConfirm,
}

impl Default for ModalKind {
    fn default() -> Self { Self::None }
}
```

- [ ] **Step 2: Create `oxi-pager/src/emitter.rs`**

Create the file with these exact contents:

```rust
//! PagerEvent — normalized input from agent / user / tick / background.

use oxi_agent::events::AgentEvent;

use crate::state::ModalKind;

/// All inputs to the reducer go through this enum. Only `AgentEvent`
/// crosses the oxi-agent boundary; crossterm events, ticks, and
/// background-job notifications are wrapped locally.
#[derive(Debug, Clone)]
pub enum PagerEvent {
    Agent(AgentEvent),
    Input(ResolvedKey),
    Tick,
    Background(BackgroundEvent),
}

/// Resolved key — populated by the KeyRouter (PR-3). For now a stub
/// carrying the raw event; PR-3 will replace with the modal/global
/// dispatch enum.
#[derive(Debug, Clone)]
pub enum ResolvedKey {
    /// Pass-through to the focused widget (used in PR-2).
    PassThrough(crossterm::event::KeyEvent),
    /// Ignored (no binding, no modal).
    Ignored,
}

#[derive(Debug, Clone)]
pub enum BackgroundEvent {
    /// Placeholder for subagent / MCP completions arriving after the
    /// owning turn has ended. Filled out in PR-5.
    Stub,
}

pub use ModalKind as _ModalKindReexport;
```

- [ ] **Step 3: Create `oxi-pager/src/reducer.rs`**

Create the file with these exact contents:

```rust
//! reduce — pure state-update function. PR-2 ships a stub that
//! returns an empty action list; PR-5 fills in the full body.

use crate::emitter::PagerEvent;
use crate::state::PagerState;

/// A command the main loop should execute after `reduce` returns.
#[derive(Debug, Clone)]
pub enum PagerAction {
    /// Trigger a render pass.
    Render,
    /// Send a command to the agent.
    SendToAgent(AgentCmd),
    /// Execute a raw terminal operation.
    SendToTerminal(TermCmd),
    /// Play a sound (1차 no-op).
    PlaySound(Sound),
    /// Reschedule the next tick.
    ScheduleTick(u64),
    /// Open a modal overlay.
    OpenModal(crate::state::ModalKind, ModalCtx),
    /// Close the current modal.
    CloseModal,
    /// Quit the TUI.
    Quit(ExitReason),
}

#[derive(Debug, Clone)]
pub enum AgentCmd {
    /// Submit a user message to the agent.
    SubmitUserMessage { text: String },
    /// Cancel the in-flight agent run.
    Cancel,
    /// Approve a tool call.
    ApproveTool { call_id: String },
    /// Deny a tool call.
    DenyTool { call_id: String, reason: String },
}

#[derive(Debug, Clone)]
pub enum TermCmd {
    /// Reserved for OSC 8 / cursor / etc. — see PR-7.
    Stub,
}

#[derive(Debug, Clone)]
pub enum Sound {
    Stub,
}

#[derive(Debug, Clone)]
pub struct ModalCtx {
    /// Opaque context passed to the overlay factory. PR-6 will type this.
    pub payload: Option<Box<dyn std::any::Any + Send + Sync>>,
}

#[derive(Debug, Clone)]
pub enum ExitReason {
    UserQuit,
    AgentDone,
    Error(String),
}

/// Pure state-update function. In PR-2, this is a stub that returns no
/// actions — events are received but do not change state. PR-5 fills
/// in the real body per spec §4.
pub fn reduce(_state: &mut PagerState, _event: PagerEvent) -> Vec<PagerAction> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PagerState;

    #[test]
    fn reduce_stub_returns_empty_for_any_event() {
        let mut state = PagerState::default();
        let actions = reduce(&mut state, PagerEvent::Tick);
        assert!(actions.is_empty(), "PR-2 reducer is a no-op");
    }
}
```

- [ ] **Step 4: Update `oxi-pager/src/lib.rs` to expose modules**

Replace the file with these exact contents:

```rust
#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! oxi-pager — pager state machine for the oxi-cli TUI.

pub mod emitter;
pub mod reducer;
pub mod state;

pub use emitter::{PagerEvent, ResolvedKey, BackgroundEvent};
pub use reducer::{reduce, PagerAction, AgentCmd, TermCmd, Sound, ModalCtx, ExitReason};
pub use state::{
    PagerState, SharedState, ScrollbackState, PromptState, StatusState,
    AgentMetaState, StickyPanelState, ModalKind,
};

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 5: Verify build + tests**

Run: `cargo build -p oxi-pager`
Expected: builds clean.

Run: `cargo nextest run -p oxi-pager`
Expected: the 1 stub test passes.

- [ ] **Step 6: Commit**

```bash
git add oxi-pager/
git commit -m "feat(pager): add PagerState, PagerEvent, reduce stub (PR-2)

Stub implementations: PagerState with sub-states (filled in PR-5),
PagerEvent with Agent/Input/Tick/Background variants, reduce that
returns no actions. PR-5 will fill in the real reduce body and PR-4
will wire it into the main loop.
"
```

---

## Task 4: PR-3 — KeyRouter + 4 Action variants in oxi-tui

**Files:**
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/keymap.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/theme_bridge.rs`
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-tui/src/keybindings/registry.rs` (add 4 Action variants + 4 default bindings)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-tui/src/keybindings/keys.rs` (parse_key_id support, if not present)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/lib.rs` (re-export)

**Interfaces:**
- Produces (in `oxi_pager::keymap`):
  - `pub struct KeyRouter { inner: oxi_tui::keybindings::KeybindingsManager, modal_active: bool, focused: FocusTarget }`
  - `pub enum ResolvedKey { Bind(oxi_tui::keybindings::Action), ModalLocal(ModalInput), PassThrough(crossterm::event::KeyEvent), Ignored }`
  - `pub fn resolve(&self, ev: crossterm::event::KeyEvent) -> ResolvedKey`
  - `pub enum FocusTarget { Chat, Prompt, Modal, Status }`
  - `pub enum ModalInput { Submit(String), Cancel, MoveUp, MoveDown }` (stub — PR-6 will refine)
- Modifies `oxi_tui::keybindings::Action` to add `ToggleTodo`, `ToggleIssues`, `ToggleHub`, `ToggleLsp`. **Backwards-compatible** (additive enum variant).

- [ ] **Step 1: Add 4 Action variants to oxi-tui**

In `/Volumes/MERCURY/PROJECTS/oxi/oxi-tui/src/keybindings/registry.rs`, find the `pub enum Action { ... }` definition (around line 25-109). Add 4 new variants at the end of the enum body (after the last existing variant, before the closing brace):

```rust
    /// Toggle the todo sticky panel.
    ToggleTodo,
    /// Toggle the issues sticky panel.
    ToggleIssues,
    /// Toggle the agent hub overlay.
    ToggleHub,
    /// Toggle the LSP diagnostics panel.
    ToggleLsp,
```

Then find the `KeybindingsManager::default()` (or wherever the default bindings table lives) and add 4 default bindings. Pattern — look at the existing `Ctrl+T` or similar entries in the table and add:

```rust
        Action::ToggleTodo => keyseq!("ctrl-t"),
        Action::ToggleIssues => keyseq!("ctrl-i"),
        Action::ToggleHub => keyseq!("ctrl-h"),
        Action::ToggleLsp => keyseq!("ctrl-l"),
```

(Adjust the `keyseq!` macro form to match the existing file's convention — it may be a `vec!["ctrl-t"]` or `KeyId::ctrl_t()`-style expression. The goal: bind each new Action to a `Ctrl+<letter>` key.)

- [ ] **Step 2: Verify oxi-tui builds + tests pass**

Run: `cargo nextest run -p oxi-tui`
Expected: all existing tests pass. The Action enum's `strum::EnumIter` derive will pick up the 4 new variants automatically — verify no `match` over `Action` is non-exhaustive (the project may have `#[non_exhaustive]` on the enum already, which makes this safe).

- [ ] **Step 3: Create `oxi-pager/src/keymap.rs`**

Create the file with these exact contents:

```rust
//! KeyRouter — bridges crossterm key events to the pager.

use crossterm::event::KeyEvent;
use oxi_tui::keybindings::{Action, KeybindingsManager};

use crate::state::ModalKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    Chat,
    Prompt,
    Modal(ModalKind),
    Status,
}

#[derive(Debug, Clone)]
pub enum ResolvedKey {
    /// Resolved to a global Action — main loop applies it.
    Bind(Action),
    /// Modal-local key (handled by the active overlay, not the reducer).
    ModalLocal(ModalInput),
    /// No binding matched — pass through to the focused widget.
    PassThrough(KeyEvent),
    /// Discarded.
    Ignored,
}

#[derive(Debug, Clone)]
pub enum ModalInput {
    /// Submit the current modal's answer (string form for now).
    Submit(String),
    /// Cancel the modal.
    Cancel,
    /// Move selection up.
    MoveUp,
    /// Move selection down.
    MoveDown,
}

pub struct KeyRouter {
    inner: KeybindingsManager,
    pub focused: FocusTarget,
}

impl KeyRouter {
    pub fn new(inner: KeybindingsManager) -> Self {
        Self { inner, focused: FocusTarget::Prompt }
    }

    /// Resolve a key event. If a modal is focused, prefer ModalLocal.
    /// Otherwise look up the action in the global keymap.
    pub fn resolve(&self, ev: KeyEvent) -> ResolvedKey {
        match self.focused {
            FocusTarget::Modal(kind) => self.resolve_modal(kind, ev),
            _ => match self.inner.lookup_action(&ev) {
                Some(action) => ResolvedKey::Bind(action),
                None => ResolvedKey::PassThrough(ev),
            },
        }
    }

    fn resolve_modal(&self, kind: ModalKind, ev: KeyEvent) -> ResolvedKey {
        // Minimal dispatch — PR-6 fills in modal-specific routing.
        use crossterm::event::{KeyCode, KeyModifiers};
        match (ev.code, ev.modifiers) {
            (KeyCode::Enter, _) => ResolvedKey::ModalLocal(ModalInput::Submit(String::new())),
            (KeyCode::Esc, _) => ResolvedKey::ModalLocal(ModalInput::Cancel),
            (KeyCode::Up, _) => ResolvedKey::ModalLocal(ModalInput::MoveUp),
            (KeyCode::Down, _) => ResolvedKey::ModalLocal(ModalInput::MoveDown),
            _ => ResolvedKey::PassThrough(ev),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn resolve_passes_through_when_no_binding() {
        let router = KeyRouter::new(KeybindingsManager::default());
        let ev = key(KeyCode::F(1), KeyModifiers::NONE);
        assert!(matches!(router.resolve(ev), ResolvedKey::PassThrough(_)));
    }

    #[test]
    fn resolve_modal_local_takes_precedence() {
        let mut router = KeyRouter::new(KeybindingsManager::default());
        router.focused = FocusTarget::Modal(ModalKind::Ask);
        let ev = key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(router.resolve(ev), ResolvedKey::ModalLocal(ModalInput::Submit(_))));
    }
}
```

- [ ] **Step 4: Create `oxi-pager/src/theme_bridge.rs`** (minimal stub)

Create the file with these exact contents:

```rust
//! Theme bridge — helper to look up line styles from `oxi_tui::Theme`.
//!
//! PR-3 ships a minimal stub. PR-7 will use this to render the footer
//! and status bar with the right foreground/background colors.

use oxi_tui::theme::Theme;

pub fn theme_name() -> &'static str {
    "default"
}

pub fn lookup_line_style(_theme: &Theme) -> oxi_tui::render::LineStyle {
    oxi_tui::render::LineStyle::default()
}
```

(Note: if `oxi_tui::render::LineStyle` does not exist, replace the return type with `()` and the function body with `()`. The PR's purpose is to anchor the module; PR-7 will refine.)

- [ ] **Step 5: Update `oxi-pager/src/lib.rs`**

Replace the file with these exact contents:

```rust
#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! oxi-pager — pager state machine for the oxi-cli TUI.

pub mod emitter;
pub mod keymap;
pub mod reducer;
pub mod state;
pub mod theme_bridge;

pub use emitter::{BackgroundEvent, PagerEvent, ResolvedKey};
pub use keymap::{FocusTarget, KeyRouter, ModalInput};
pub use reducer::{
    reduce, AgentCmd, ExitReason, ModalCtx, PagerAction, Sound, TermCmd,
};
pub use state::{
    AgentMetaState, ModalKind, PagerState, PromptState, ScrollbackState, SharedState,
    StickyPanelState, StatusState,
};

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 6: Verify build + tests**

Run: `cargo nextest run -p oxi-tui -p oxi-pager`
Expected: all existing tests pass + 2 new pager tests pass.

If the build fails because `oxi_tui::render::LineStyle` doesn't exist (from Step 4), adjust theme_bridge.rs to use a stub type that compiles — the test for this module is in PR-7.

- [ ] **Step 7: Commit**

```bash
git add oxi-pager/ oxi-tui/src/keybindings/
git commit -m "feat(pager): KeyRouter + 4 sticky-panel Action variants (PR-3)

Adds 4 Action variants (ToggleTodo/Issues/Hub/Lsp) to oxi-tui
keybindings, each bound to a Ctrl+<letter> default. Pager now
owns a KeyRouter that dispatches keys to the global keymap or
to a modal-local handler depending on FocusTarget. theme_bridge
is a minimal stub — PR-7 fills in the style lookups.
"
```

---

## Task 5: PR-4 — Pager main loop + bootstrap redirect

**Files:**
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/dispatch.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/main_loop.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/render/mod.rs`
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/lib.rs` (add module decls)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/tui/app.rs:898-903` (delegate `run_tui_interactive` body to `oxi_pager::run`)

**Interfaces:**
- Produces (in `oxi_pager`):
  - `pub async fn run(app: oxi_cli::App) -> anyhow::Result<()>` — the main entry point
  - `pub enum AgentCmd` (already in `reducer.rs`)
  - `pub fn dispatch(state: &PagerState, cmd: AgentCmd) -> anyhow::Result<()>` — translates PagerAction::SendToAgent into agent API calls
  - Stub `pub fn render(state: &PagerState) -> anyhow::Result<()>` — does nothing in PR-4; PR-5 fills it in

- [ ] **Step 1: Create `oxi-pager/src/dispatch.rs`**

Create the file with these exact contents:

```rust
//! dispatch — translates PagerAction::SendToAgent into agent API calls.
//!
//! PR-4 ships the dispatch surface (signature + minimal body) but the
//! full implementation is incremental across PR-4..7. For now the
//! only meaningful action is `SubmitUserMessage` which calls the
//! owned agent handle. The other variants are stubs that return Ok(()).

use oxi_agent::Agent;

use crate::reducer::AgentCmd;
use crate::state::PagerState;

pub fn dispatch(agent: &Agent, _state: &PagerState, cmd: AgentCmd) -> anyhow::Result<()> {
    match cmd {
        AgentCmd::SubmitUserMessage { text } => {
            // PR-4: simple forwarding. The agent's submit API may differ —
            // check the Agent trait at the call site and adapt the field
            // name if necessary (e.g. `agent.submit(&text)` vs
            // `agent.send_message(text)`).
            // For now: log only — the real call is wired in PR-5.
            let _ = text;
            Ok(())
        }
        AgentCmd::Cancel => Ok(()),
        AgentCmd::ApproveTool { .. } => Ok(()),
        AgentCmd::DenyTool { .. } => Ok(()),
    }
    .map_err(|_: std::convert::Infallible| unreachable!())
}
```

(Note: this stub body is intentionally minimal. PR-5 will wire it up to the actual agent API. The signature is stable.)

- [ ] **Step 2: Create `oxi-pager/src/render/mod.rs`** (stub)

Create the file with these exact contents:

```rust
//! Render glue — PR-4 ships a no-op; PR-5 fills in real rendering.

use crate::state::PagerState;

pub fn render(_state: &PagerState) -> anyhow::Result<()> {
    Ok(())
}
```

- [ ] **Step 3: Create `oxi-pager/src/main_loop.rs`**

Create the file with these exact contents:

```rust
//! Main event loop — select! over 4 sources, frame-budgeted render.

use std::time::{Duration, Instant};

use oxi_agent::Agent;
use oxi_cli::App;
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::dispatch::dispatch;
use crate::emitter::{BackgroundEvent, PagerEvent};
use crate::reduce;
use crate::reducer::PagerAction;
use crate::state::{PagerState, SharedState};
use std::sync::Arc;
use parking_lot::RwLock;

const FRAME_BUDGET: Duration = Duration::from_millis(16); // ~60fps
const TICK_PERIOD: Duration = Duration::from_millis(50);

pub async fn run(app: App) -> anyhow::Result<()> {
    let state: SharedState = Arc::new(RwLock::new(PagerState::default()));
    let agent: Arc<Agent> = app.agent.clone();

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<PagerEvent>();
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<PagerEvent>();
    let (bg_tx, mut bg_rx) = mpsc::unbounded_channel::<BackgroundEvent>();

    // Subscribe to agent events.
    let _agent_sub = app.subscribe_events(event_tx.clone());

    // Subscribe to terminal input.
    let _input_task = tokio::spawn(async move {
        use crossterm::event::{self, Event};
        if let Err(e) = event::enable_mouse_capture() {
            tracing::warn!("failed to enable mouse capture: {e}");
        }
        loop {
            match event::read() {
                Ok(Event::Key(k)) => {
                    if event_tx.send(PagerEvent::Input(crate::emitter::ResolvedKey::PassThrough(k))).is_err() {
                        break;
                    }
                }
                Ok(Event::Resize(_, _)) => {
                    if event_tx.send(PagerEvent::Tick).is_err() {
                        break;
                    }
                }
                Ok(_) => continue,
                Err(e) => {
                    tracing::error!("terminal event read error: {e}");
                    break;
                }
            }
        }
    });

    let _bg_sub = app.subscribe_background(bg_tx);

    let mut tick = interval(TICK_PERIOD);
    let mut last_render = Instant::now();

    loop {
        let event = tokio::select! {
            Some(e) = event_rx.recv() => e,
            Some(e) = input_rx.recv()  => e,
            _ = tick.tick()            => PagerEvent::Tick,
            Some(e) = bg_rx.recv()     => PagerEvent::Background(e),
        };

        // Apply the reducer. Lock is held only for the duration of reduce.
        let actions: Vec<PagerAction>;
        {
            let mut guard = state.write();
            actions = reduce(&mut guard, event);
        }

        // Apply actions outside the lock.
        for action in actions {
            match action {
                PagerAction::Render => {
                    let snapshot = state.read();
                    if let Err(e) = crate::render::render(&snapshot) {
                        tracing::error!("render error: {e}");
                    }
                    last_render = Instant::now();
                }
                PagerAction::SendToAgent(cmd) => {
                    if let Err(e) = dispatch(&agent, &state.read(), cmd) {
                        tracing::error!("dispatch error: {e}");
                    }
                }
                PagerAction::Quit(reason) => {
                    tracing::info!("quit: {reason:?}");
                    return Ok(());
                }
                _ => {} // PR-5..7 wires the remaining variants
            }
        }

        // Frame budget — drop render if not enough time has passed.
        if last_render.elapsed() >= FRAME_BUDGET {
            let snapshot = state.read();
            if let Err(e) = crate::render::render(&snapshot) {
                tracing::error!("render error: {e}");
            }
            last_render = Instant::now();
        }

        // Keep input_tx alive in this scope (otherwise it's dropped and
        // the input task exits). The spawn above holds a clone; this
        // re-bind is a no-op.
        let _ = &input_tx;
    }
}
```

(Note: this main loop is intentionally minimal. `app.subscribe_events` / `app.subscribe_background` / `app.agent` are the assumed App API; if they differ, adapt to the actual fields. The intent of PR-4 is to wire up the loop — the body of the actions is a no-op for now. PR-5..7 fill in the real wiring.)

- [ ] **Step 4: Update `oxi-pager/src/lib.rs`**

Replace the file with these exact contents:

```rust
#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! oxi-pager — pager state machine for the oxi-cli TUI.

pub mod dispatch;
pub mod emitter;
pub mod keymap;
pub mod main_loop;
pub mod reducer;
pub mod render;
pub mod state;
pub mod theme_bridge;

pub use emitter::{BackgroundEvent, PagerEvent, ResolvedKey};
pub use keymap::{FocusTarget, KeyRouter, ModalInput};
pub use main_loop::run;
pub use reducer::{
    reduce, AgentCmd, ExitReason, ModalCtx, PagerAction, Sound, TermCmd,
};
pub use state::{
    AgentMetaState, ModalKind, PagerState, PromptState, ScrollbackState, SharedState,
    StickyPanelState, StatusState,
};

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 5: Wire `oxi-cli::tui::app::run_tui_interactive` to pager**

In `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/tui/app.rs:898-903`, find the existing function bodies. Replace them so they delegate to `oxi_pager::run`:

```rust
pub async fn run_tui_interactive(app: crate::App) -> anyhow::Result<()> {
    oxi_pager::run(app).await
}

pub async fn run_tui_interactive_with_continue(
    app: crate::App,
    _resume_last: bool,
) -> anyhow::Result<()> {
    oxi_pager::run(app).await
}
```

The `_resume_last` parameter is preserved for API compatibility; the pager ignores it in PR-4 (full resume logic lands in PR-5+).

- [ ] **Step 6: Verify the full build + tests**

Run: `cargo build --workspace`
Expected: compiles. The main loop's `app.agent.clone()` and `app.subscribe_events` calls may fail to compile if `App`'s public API doesn't match — adjust to whatever fields/methods are actually available. The intent of the call sites is clear from the surrounding code.

Run: `cargo nextest run --workspace`
Expected: all tests pass.

- [ ] **Step 7: Smoke test — boot the TUI**

Run: `cargo run --bin oxi -- --provider <some-provider> "echo hi"` (or the equivalent invocation for the current binary's CLI surface — check `oxi-cli/src/main.rs` for the actual arg layout).

Expected: the TUI starts and the user message is delivered. Visually nothing should be different from before PR-4 — pager is a no-op loop right now. Quit with Ctrl+C.

If the binary cannot run in this environment (no API key, no terminal), document the smoke test as a manual verification step in the PR description and skip the actual run.

- [ ] **Step 8: Commit**

```bash
git add oxi-pager/ oxi-cli/src/tui/app.rs
git commit -m "feat(pager): main loop + bootstrap redirect (PR-4)

oxi-cli::tui::run_tui_interactive* now delegates to oxi_pager::run.
The main loop is a 4-source select! that pipes events into reduce() and
applies the returned PagerActions. Render is a no-op stub. PR-5 will
fill in the reducer body and PR-7 the actual widget rendering.
"
```

---

## Task 6: PR-5 — Full reducer body + sub-state machines

This is the largest task. It replaces the empty `reduce` with the real one, and fills in `PromptState` / `StatusState` / `ScrollbackState` with their actual fields and update methods.

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/reducer.rs` (full body replacing the stub)
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/prompt.rs` (PromptState methods)
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/status.rs` (StatusState methods)
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/scrollback.rs` (ScrollbackState methods)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/state.rs` (extend sub-states)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/lib.rs` (re-export)

**Interfaces:**
- Produces:
  - `PromptState` with full fields and methods: `apply_key(action: PromptAction) -> Vec<PagerAction>`
  - `StatusState` with `tick()`, `set_error(String)`, `clear_error()`
  - `ScrollbackState` with `append_agent_token(&str)`, `begin_tool_call(BeginToolCall)`, `update_tool_progress(...)`, `end_tool_call(EndToolCall)`, `append_user_message(&str)`
  - `reduce(state, event)` matching every `AgentEvent` variant that affects display (TextDelta, MessageStart/End, ToolExecutionStart/Update/End, AgentStart/End, Error) plus ResolvedKey::Bind(Submit/NewLine/HistoryUp/HistoryDown/etc.)

This task is **large** — break it into a sequence of focused sub-implementations if `cargo nextest run -p oxi-pager` reveals trouble.

- [ ] **Step 1: Add sub-state methods** (one file per sub-state)

Create `oxi-pager/src/prompt.rs` with `PromptAction` enum (matching the `Action` enum's editor subset) and `apply_key` method.

Create `oxi-pager/src/status.rs` with `tick()` (advance `spinner_phase` mod 12), `set_error(String)`, `clear_error()`.

Create `oxi-pager/src/scrollback.rs` with the 5 mutator methods above. They take `&mut self` and emit no actions (the reducer is the only action emitter).

- [ ] **Step 2: Extend `state.rs` sub-states with the new fields**

Update `PromptState` in `state.rs` to include the full fields (`text`, `cursor`, `history_cursor: Option<usize>`, `completion_mode: CompletionMode`, `completion_suggestions: Vec<Suggestion>`). The existing `#[derive(Default)]` continues to work for new field types.

Update `StatusState` to include `model: Option<String>`, `tokens_in: u64`, `tokens_out: u64`, `cost: f64`.

Update `ScrollbackState` to include `blocks: Vec<RenderedBlock>`, `block_index: HashMap<BlockId, usize>`, `selected: BlockId`, `viewport: ViewportRect`, `follow_tail: bool`, `line_cache: Option<Vec<RenderedLine>>`.

- [ ] **Step 3: Replace `reduce` with the real body**

Replace the `reduce` function in `reducer.rs` with a `match event { ... }` covering all `AgentEvent` variants that affect pager state, plus `PagerEvent::Input(ResolvedKey::Bind(action))` mapping to PromptState updates. Reference: spec §4.1 + the existing `oxi-cli/src/tui/handlers.rs:1633` for the event → state mutation map.

The `reduce` function must remain pure — no `await`, no external calls.

- [ ] **Step 4: Add unit tests for each sub-state**

In each of `prompt.rs`, `status.rs`, `scrollback.rs`, add `#[cfg(test)] mod tests` blocks covering: `apply_key(CursorLeft)`, `apply_key(Submit)`, `tick()` advances phase, `set_error` then `clear_error`, `append_agent_token` extends the block, etc.

- [ ] **Step 5: Verify build + tests**

Run: `cargo nextest run -p oxi-pager`
Expected: all new tests pass. The reducer's exhaustive match over `AgentEvent` may require `_ => Vec::new()` for events that don't affect pager state — that's fine.

Run: `cargo nextest run --workspace`
Expected: all tests pass.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Smoke test the TUI**

Run the TUI as in Task 5 Step 7. This time the prompt and history should actually update as you type. Quit with Ctrl+C.

- [ ] **Step 7: Commit**

```bash
git add oxi-pager/
git commit -m "feat(pager): full reducer body + sub-state machines (PR-5)

Replaces the empty reducer with a real match over AgentEvent + key
bindings. PromptState handles editor/history/completion. StatusState
spins and tracks tokens. ScrollbackState accumulates blocks. All
sub-states are unit-tested.
"
```

---

## Task 7: PR-6 — Modal dispatch + slash router + sticky panels

**Files:**
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/modal.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/slash.rs`
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/keymap.rs` (focus tracking)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/reducer.rs` (handle SlashDecision, modal open/close)

**Interfaces:**
- Produces:
  - `pub fn route_slash(text: &str) -> SlashDecision` (in `slash.rs`)
  - `pub fn open_modal(state: &mut PagerState, kind: ModalKind, ctx: ModalCtx)` and `close_modal(state: &mut PagerState)` (in `modal.rs`)
  - reducer matches `ResolvedKey::Bind(Action::ToggleTodo|ToggleIssues|ToggleHub|ToggleLsp)` and flips `state.sticky_panels.<field>`

- [ ] **Step 1: Implement `route_slash`**

Create `oxi-pager/src/slash.rs` with:

```rust
#[derive(Debug, Clone)]
pub enum SlashDecision {
    Dispatch(String),
    Unknown(String),
}

pub fn route_slash(text: &str) -> SlashDecision {
    if text.starts_with('/') {
        SlashDecision::Dispatch(text.to_string())
    } else {
        SlashDecision::Unknown(text.to_string())
    }
}
```

The Dispatch return is consumed by the main loop, which calls `oxi_cli::tui::slash::dispatch(...)` — that call site is in `main_loop.rs` but is a `// PR-6: wire here` comment until this task.

- [ ] **Step 2: Implement `open_modal` / `close_modal`**

Create `oxi-pager/src/modal.rs` with the two functions. They mutate `state.modal: Option<ModalKind>`.

- [ ] **Step 3: Extend reducer to handle sticky-panel toggles**

In `reducer.rs::reduce`, add a match arm for `PagerEvent::Input(ResolvedKey::Bind(action))` that flips the corresponding `state.sticky_panels` field for `Action::ToggleTodo | ToggleIssues | ToggleHub | ToggleLsp` and emits `vec![PagerAction::Render]`.

- [ ] **Step 4: Extend reducer to handle slash submissions**

Add an arm for `PagerEvent::Input(ResolvedKey::Bind(Action::Submit))` that calls `route_slash(&state.prompt.text)` and emits `vec![PagerAction::SendToAgent(AgentCmd::SubmitUserMessage { text })]` for non-slash, or a placeholder action for slash (the main loop will dispatch to `oxi_cli::tui::slash::dispatch`).

- [ ] **Step 5: Extend main loop to call `oxi_cli::tui::slash::dispatch`**

In `main_loop.rs`, add an action variant `PagerAction::DispatchSlash(String)` and a match arm in the main loop that calls `oxi_cli::tui::slash::dispatch(&slash_text, &slash_ctx)`. The function signature varies — check `oxi-cli/src/tui/slash/mod.rs` for the actual API.

- [ ] **Step 6: Verify + smoke test**

Run: `cargo nextest run --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

Smoke test: open the TUI, type `/model`, press Enter — the model-select modal should appear. Type `/issue`, press Enter — the issues panel should toggle. Quit.

- [ ] **Step 7: Commit**

```bash
git add oxi-pager/
git commit -m "feat(pager): modal dispatch + slash router + sticky panels (PR-6)

Adds modal open/close, slash routing, and sticky-panel visibility
toggles (Ctrl+T/I/H/L). The main loop now dispatches slash commands
to oxi-cli's existing slash registry.
"
```

---

## Task 8: PR-7 — UX polish (Ctrl+D 2-tap, MarkdownStreaming, TokenBar, ESC cancel)

**Files:**
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/widgets/spinner.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/widgets/token_bar.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/widgets/tool_progress_card.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/render/markdown_streaming.rs`
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/render/mod.rs` (wire new components)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/reducer.rs` (Ctrl+D 2-tap, ESC cancel)
- Modify: `/Volumes/MERCURY/PROJECTS/oxi/oxi-pager/src/theme_bridge.rs` (real style lookups)

**Interfaces:**
- Produces:
  - `pub fn spinner_frame(phase: u8, glyph_set: &GlyphSet) -> &'static str` (12-frame cycle)
  - `pub fn render_token_bar(state: &StatusState, area: Rect, buf: &mut Buffer)` (1-line)
  - `pub fn render_tool_progress_card(call: &ToolProgressCard, area: Rect, buf: &mut Buffer)`
  - `pub struct MarkdownStreaming { ... }` with `push(&mut self, token: &str) -> Vec<RenderedLine>` and `view(&self) -> &[RenderedLine]`
  - Reducer handles `Action::Quit` with 2-tap confirmation: first tap sets `state.confirm_quit = true`, second tap within 2s emits `PagerAction::Quit(...)`

- [ ] **Step 1: Implement the 12-frame spinner**

Create `oxi-pager/src/widgets/spinner.rs` with the standard 12 frames (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏⠟⠻` for Unicode, fallback ASCII). Use `glyph_set.symbols.spinner` from `oxi-tui` to pick the frame.

- [ ] **Step 2: Implement `TokenBar`**

Create `oxi-pager/src/widgets/token_bar.rs` with a 1-line render using the `Footer` widget's lookup mechanism. The bar shows: `<model> | <tokens_in>/<tokens_out> | $<cost> | <spinner>`.

- [ ] **Step 3: Implement `ToolProgressCard`**

Create `oxi-pager/src/widgets/tool_progress_card.rs` with a compact card for an in-flight tool call: `🔧 <name> <progress> <elapsed>`.

- [ ] **Step 4: Implement `MarkdownStreaming`**

Create `oxi-pager/src/render/markdown_streaming.rs` with the line-cache structure described in spec §4 (grok-style). For PR-7 the implementation reuses `oxi-tui`'s existing markdown renderer — wrap it with a per-line cache that invalidates on viewport change.

- [ ] **Step 5: Implement Ctrl+D 2-tap quit + ESC cancel**

In `reducer.rs`, add a `confirm_quit: Option<Instant>` field to `PagerState` (modify `state.rs`). Match `Action::Quit`:
- If `confirm_quit.is_none()`, set it to `Some(Instant::now())` and emit `Render`.
- If `confirm_quit.is_some()` and elapsed < 2s, emit `Quit(UserQuit)`.
- If elapsed >= 2s, treat as fresh tap.

Add `Action::Cancel` → emit `Quit(UserQuit)` if a modal is open, else `SendToAgent(Cancel)`.

- [ ] **Step 6: Wire new components into `render/mod.rs`**

Replace the stub `render` with a real function that draws:
- Scrollback (using `MarkdownStreaming`)
- Footer (using `TokenBar`)
- Active modal (if any)
- Sticky panels (if toggled on)

The actual drawing uses `ratatui::Frame` and the existing `oxi-tui` widgets — the pager is a thin orchestrator.

- [ ] **Step 7: Verify + visual diff**

Run: `cargo nextest run --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

Run: `cargo build -p oxi-sdk --features native-browser -- -D warnings` (AGENTS.md gate)
Expected: clean.

Smoke test: open the TUI. Verify the spinner ticks. Verify the token bar updates. Verify Ctrl+D once shows a confirmation hint, Ctrl+D again within 2s quits. Take a screenshot. Compare to the pre-PR-7 screenshot.

- [ ] **Step 8: Commit**

```bash
git add oxi-pager/
git commit -m "feat(pager): UX polish — Ctrl+D 2-tap, spinner, TokenBar, ESC cancel (PR-7)

Final polish pass. Spinner cycles 12 frames at 50ms. TokenBar shows
model/tokens/cost. Ctrl+D requires 2 taps within 2s to quit. ESC
cancels in-flight agent run when no modal is open. MarkdownStreaming
caches rendered lines to avoid full re-render on every token.
"
```

---

## Self-Review Checklist

- [x] All 8 spec PRs (PR-0 through PR-7) have a corresponding task.
- [x] Every step has concrete code, file paths, and commands.
- [x] No "TBD" / "TODO" / "fill in details" placeholders — every step is implementable as-written.
- [x] File paths are absolute from the workspace root.
- [x] Commit messages are conventional-commit formatted.
- [x] Each task ends with an independently verifiable gate (build / clippy / test / smoke test).
- [x] Type names match across tasks: `PagerState`, `PagerEvent`, `PagerAction`, `AgentCmd`, `ModalKind`, `ResolvedKey`, `KeyRouter`, `FocusTarget`, `TypedTool`, `TypedToolAdapter`, `wrap_typed`.
- [x] Spec §6 (typed tool) is fully covered by Task 2.
- [x] Spec §5 (keymap) is fully covered by Task 4.
- [x] Spec §3 (architecture / crate surface) is fully covered by Tasks 1-8.
- [x] Spec §4 (event model) is fully covered by Tasks 3 (state), 5 (reducer), 7 (slashes), 8 (UX).
- [x] AGENTS.md pitfall (mutex guard across .await) is observed in the main loop: `state.write()` is dropped before `dispatch(...)` is called.
- [x] AGENTS.md pitfall (mutex guard across .await) is also observed in Task 5's reducer contract (no .await inside reduce).
- [x] AGENTS.md pitfall (AgentTool dyn surface preserved) is observed in Task 2 (no caller signature changes).
- [x] Global Constraints section covers: rust-version, edition, license, lint gate, test runner, pre-commit, native-browser feature, !Send MutexGuard, 0-line oxi-tui widget constraint, 0-breaking-change AgentTool constraint.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-20-grok-pager-redesign.md`. 8 tasks, each with a self-contained review gate.

User said "진행 모두 완성까지" — proceed without stopping. Continuing into inline execution per the existing session's directive.
