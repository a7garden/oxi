# oxi-sdk Consumer Requirements for Clean Integration

**Author:** oxios project  
**Date:** 2026-05-16  
**Status:** Draft  
**Target:** oxi-sdk (oxi project)

---

## Background

oxios is an Agent OS that uses oxi-sdk as its execution engine. Our stated architecture depends on oxi-sdk as the **single** integration surface — everything oxios needs should flow through that one crate.

In practice, oxios today depends on **three** oxi crates directly:

```toml
# workspace Cargo.toml — current state
oxi-sdk    = { path = "../oxi/oxi-sdk" }
oxi-ai     = { path = "../oxi/oxi-ai" }
oxi-agent  = { path = "../oxi/oxi-agent" }
```

This violates the SDK abstraction and creates tight coupling to oxi's internal crate structure. If oxi reorganizes `oxi-ai` or `oxi-agent`, oxios breaks.

The root causes are two:

1. **Missing re-exports** — types that consumers need are not surfaced through `oxi-sdk`.
2. **`!Send` future** — `AgentLoop::run()` returns a non-`Send` future, forcing consumers into `spawn_blocking`.

Both are fixable within oxi-sdk without changing public semantics. This document details each request.

---

## Request 1: Complete Re-export Surface

**Priority: High**  
**Effort estimate: Low** (additive `pub use` lines)

### Problem

oxios imports the following types directly from `oxi-ai` and `oxi-agent` because `oxi-sdk` does not re-export them:

| Import | Source crate | Used in (oxios) |
|--------|-------------|-----------------|
| `SearchCache` | `oxi-agent` | `kernel_bridge.rs` |
| `CompactionEvent` | `oxi-agent::prelude` | `agent_runtime.rs` |
| `UserMessage` | `oxi-ai` | `ouroboros_engine.rs` |
| `Context` | `oxi-ai` | `supervisor.rs`, `ouroboros_engine.rs` |
| `Message` | `oxi-ai` | `ouroboros_engine.rs` |
| `Model` | `oxi-ai` | `supervisor.rs`, `ouroboros_engine.rs` |
| `ProviderError` | `oxi-ai` | `supervisor.rs` |
| `ProviderEvent` | `oxi-ai` | `supervisor.rs` |
| `StreamOptions` | `oxi-ai` | `supervisor.rs` |
| `CompactionStrategy` | `oxi-ai` | `agent_runtime.rs` |
| `Provider` | `oxi-ai` | `agent_runtime.rs`, `ouroboros_engine.rs` |
| `tools::ToolError` | `oxi-agent` | `a2a_tools.rs` |

Some of these (`Context`, `Model`, `Provider`, `StreamOptions`, `CompactionStrategy`) are already re-exported in oxi-sdk. However, the re-export alone isn't enough — when a consumer also needs types from the *same* module that aren't re-exported (e.g., `ProviderError` lives alongside `Provider`), they must reach through to the source crate, pulling in the transitive dependency.

### Current state (consumer code)

```rust
// oxios-kernel/Cargo.toml
oxi-ai     = { workspace = true }
oxi-agent  = { workspace = true }

// agent_runtime.rs
use oxi_agent::{AgentEvent, AgentLoop, AgentLoopConfig, SearchCache, SharedState, ToolRegistry};
use oxi_agent::agent_loop::config::ToolExecutionMode;
use oxi_agent::prelude::CompactionEvent;
use oxi_ai::{CompactionStrategy, Provider};

// ouroboros_engine.rs
use oxi_ai::{Context, Message, Model, Provider, UserMessage};

// a2a_tools.rs
use oxi_agent::{AgentTool, AgentToolResult, ToolContext, tools::ToolError};
```

### Desired state

```rust
// oxios-kernel/Cargo.toml
// (only oxi-sdk — remove oxi-ai and oxi-agent)

// agent_runtime.rs
use oxi_sdk::{
    AgentEvent, AgentLoop, AgentLoopConfig, SearchCache, SharedState,
    ToolRegistry, ToolExecutionMode, CompactionEvent,
    CompactionStrategy, Provider,
};

// ouroboros_engine.rs
use oxi_sdk::{Context, Message, Model, Provider, UserMessage};

// a2a_tools.rs
use oxi_sdk::{AgentTool, AgentToolResult, ToolContext, ToolError};
```

### What oxi-sdk needs to add

The following are **not currently re-exported** and need to be added:

```rust
// In oxi-sdk/src/lib.rs — additions to existing re-export block

// From oxi-agent
pub use oxi_agent::SearchCache;
pub use oxi_agent::prelude::CompactionEvent;

// From oxi-ai
pub use oxi_ai::UserMessage;
pub use oxi_ai::ProviderError;  // critical for error handling downstream
pub use oxi_ai::ProviderEvent;  // critical for streaming consumers
```

Additionally, verify that these existing re-exports are accessible at the top level (not buried in submodules):

- `ToolError` — currently re-exported as `oxi_sdk::ToolError` ✓
- `ToolExecutionMode` — currently re-exported ✓
- `Context`, `Message`, `Model` — currently re-exported ✓

### Acceptance criteria

- [ ] oxios can remove `oxi-ai` and `oxi-agent` from its `Cargo.toml` workspace dependencies
- [ ] All `use oxi_ai::` and `use oxi_agent::` imports in oxios-kernel and oxios-ouroboros are replaced with `use oxi_sdk::`
- [ ] `cargo check -p oxios-kernel` passes with only `oxi-sdk` as a dependency

---

## Request 2: Send-safe `AgentLoop::run()`

**Priority: Medium-High**  
**Effort estimate: Medium** (internal refactor in oxi-agent)

### Problem

`AgentLoop::run()` returns a `!Send` future. The internal tool execution machinery stores `Box<dyn Future>` (without `+ Send`) and uses closures that capture non-`Send` state. This forces any consumer that needs to spawn the agent loop on a separate task to use `tokio::task::spawn_blocking`, which:

1. **Wastes a blocking thread** — `AgentLoop::run()` is fundamentally async (it awaits LLM API calls). It doesn't belong on a blocking thread.
2. **Complicates error propagation** — `spawn_blocking` adds an extra `JoinError` layer.
3. **Prevents structured concurrency** — consumers can't use `tokio::spawn` + `JoinSet` patterns for managing multiple concurrent agents.

### Current state (consumer code)

```rust
// oxios agent_runtime.rs — current workaround

// Must clone everything because spawn_blocking moves to a blocking thread.
let ctx = AgentLoopContext {
    provider: Arc::clone(&self.provider),
    config: self.config.clone(),
    system_prompt,
    prompt,
    seed_id,
    kernel_handle: Arc::clone(&self.kernel_handle),
    // ... more fields
};

let (final_content, steps_completed, success) =
    tokio::task::spawn_blocking(move || {
        run_agent_loop(ctx)  // runs an ASYNC loop on a BLOCKING thread
    })
    .await??;  // double-? for JoinError + inner error
```

### Desired state

```rust
// After fix — clean tokio::spawn

let handle = tokio::spawn(async move {
    let agent_loop = AgentLoop::new(provider, loop_config, tools, state);
    agent_loop.run(system_prompt, prompt).await
});

let result = handle.await??;  // or just .await? if we don't need JoinError separately
```

### Root cause

The `!Send` bound originates in oxi-agent's internal tool dispatch, where futures are boxed without `Send`:

```rust
// Likely somewhere in oxi-agent internals:
Box<dyn Future<Output = Result<AgentToolResult>>>
//                                    needs to be:
Box<dyn Future<Output = Result<AgentToolResult>> + Send>
```

The fix is to ensure all captured state in the tool execution path is `Send`, and update the boxed future bounds accordingly. This is an internal change — the public `AgentTool` trait already returns `Pin<Box<dyn Future<Output = AgentToolResult> + Send>>` in most cases.

### Implementation notes for oxi

1. Audit `AgentLoop::run()` internals for `Box<dyn Future>` without `+ Send`.
2. Ensure `AgentTool::call()` returns `Send` futures (add `+ Send` bound if missing).
3. Verify `SharedState` and `ToolRegistry` are `Send + Sync` (they likely already are — they use `Arc` internally).
4. Add `#[test] fn agent_loop_future_is_send()` compile-time assertion.

```rust
// Compile-time assertion to prevent regression
fn _assert_send() {
    fn assert_send<T: Send>() {}
    assert_send::<AgentLoop>();  // if AgentLoop itself needs Send
    // or check the future:
    fn check_future<F: Future + Send>(_: F) {}
    // check_future(agent_loop.run("sys".into(), "user".into()));
}
```

### Acceptance criteria

- [ ] `AgentLoop::run()` returns a `Send` future (verified by compile-time assertion in oxi-agent)
- [ ] oxios can replace `spawn_blocking` with `tokio::spawn` in `agent_runtime.rs`
- [ ] No behavioral change in agent execution semantics

---

## Impact Analysis

### For oxi project

| Change | Risk | Scope |
|--------|------|-------|
| Additional re-exports | **Low** — additive only, no breaking changes | `oxi-sdk/src/lib.rs` (~5 lines) |
| Send-safe AgentLoop | **Medium** — may require internal refactoring of tool dispatch | `oxi-agent` internals |

### For oxios project

| Change | Benefit |
|--------|---------|
| Remove `oxi-ai` + `oxi-agent` deps | Single dependency surface, immune to oxi internal reorganization |
| Replace `spawn_blocking` | Cleaner code, better performance, proper error propagation |

### Migration path

1. oxi-sdk adds re-exports (can ship immediately).
2. oxios switches imports and removes workspace deps (same PR or follow-up).
3. oxi-agent refactors for `Send` future (can land independently, on oxi's timeline).
4. oxios removes `spawn_blocking` workaround (after oxi release with Send-safe future).

Steps 1–2 and 3–4 are independent — they can proceed in parallel.

---

## Summary

| # | Request | Priority | Effort | Blocking |
|---|---------|----------|--------|----------|
| 1 | Re-export `SearchCache`, `CompactionEvent`, `UserMessage`, `ProviderError`, `ProviderEvent` | High | Low | No |
| 2 | Make `AgentLoop::run()` return `Send` future | Medium-High | Medium | No |

Both changes are additive or internal — no public API breakage. They allow oxios to depend solely on `oxi-sdk` as the integration contract, which was the original design intent.
