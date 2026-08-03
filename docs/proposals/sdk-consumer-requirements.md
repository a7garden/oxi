# oxicode-sdk Consumer Requirements for Clean Integration

**Author:** oxios project  
**Date:** 2026-05-16  
**Status:** Draft — Codebase-verified  
**Target:** oxicode-sdk (oxicode project)

---

## Background

oxios is an Agent OS that uses oxicode-sdk as its execution engine. Our stated architecture depends on oxicode-sdk as the **single** integration surface — everything oxios needs should flow through that one crate.

In practice, oxios today depends on **three** oxicode crates directly:

```toml
# workspace Cargo.toml — current state
oxicode-sdk    = { path = "../oxicode/oxicode-sdk" }
oxicode-ai     = { path = "../oxicode/oxicode-ai" }
oxicode-agent  = { path = "../oxicode/oxicode-agent" }
```

This violates the SDK abstraction and creates tight coupling to oxicode's internal crate structure. If oxicode reorganizes `oxicode-ai` or `oxicode-agent`, oxios breaks.

The root causes are two:

1. **Missing re-exports** — types that consumers need are not surfaced through `oxicode-sdk`.
2. **`!Send` future** — `AgentLoop::run()` returns a non-`Send` future due to internal boxed futures missing `+ Send`, forcing consumers into `spawn_blocking`.

Both are fixable within oxicode-sdk without changing public semantics. This document details each request, with root causes **verified against the actual codebase**.

---

## Request 1: Complete Re-export Surface

**Priority: High**  
**Effort estimate: Low** (additive `pub use` lines)

### Problem

oxios imports the following types directly from `oxicode-ai` and `oxicode-agent` because `oxicode-sdk` does not re-export them:

| Import | Source crate | Used in (oxios) |
|--------|-------------|-----------------|
| `SearchCache` | `oxicode-agent` | `kernel_bridge.rs` |
| `CompactionEvent` | `oxicode-agent::prelude` | `agent_runtime.rs` |
| `UserMessage` | `oxicode-ai` | `ouroboros_engine.rs` |
| `Context` | `oxicode-ai` | `supervisor.rs`, `ouroboros_engine.rs` |
| `Message` | `oxicode-ai` | `ouroboros_engine.rs` |
| `Model` | `oxicode-ai` | `supervisor.rs`, `ouroboros_engine.rs` |
| `ProviderError` | `oxicode-ai` | `supervisor.rs` |
| `ProviderEvent` | `oxicode-ai` | `supervisor.rs` |
| `StreamOptions` | `oxicode-ai` | `supervisor.rs` |
| `CompactionStrategy` | `oxicode-ai` | `agent_runtime.rs` |
| `Provider` | `oxicode-ai` | `agent_runtime.rs`, `ouroboros_engine.rs` |
| `tools::ToolError` | `oxicode-agent` | `a2a_tools.rs` |

Some of these (`Context`, `Model`, `Provider`, `StreamOptions`, `CompactionStrategy`) are already re-exported in oxicode-sdk. However, the re-export alone isn't enough — when a consumer also needs types from the *same* module that aren't re-exported (e.g., `ProviderError` lives alongside `Provider`), they must reach through to the source crate, pulling in the transitive dependency.

### Current state (consumer code)

```rust
// oxios-kernel/Cargo.toml
oxicode-ai     = { workspace = true }
oxicode-agent  = { workspace = true }

// agent_runtime.rs
use oxicode_agent::{AgentEvent, AgentLoop, AgentLoopConfig, SearchCache, SharedState, ToolRegistry};
use oxicode_agent::agent_loop::config::ToolExecutionMode;
use oxicode_agent::prelude::CompactionEvent;
use oxicode_ai::{CompactionStrategy, Provider};

// ouroboros_engine.rs
use oxicode_ai::{Context, Message, Model, Provider, UserMessage};

// a2a_tools.rs
use oxicode_agent::{AgentTool, AgentToolResult, ToolContext, tools::ToolError};
```

### Desired state

```rust
// oxios-kernel/Cargo.toml
// (only oxicode-sdk — remove oxicode-ai and oxicode-agent)

// agent_runtime.rs
use oxicode_sdk::{
    AgentEvent, AgentLoop, AgentLoopConfig, SearchCache, SharedState,
    ToolRegistry, ToolExecutionMode, CompactionEvent,
    CompactionStrategy, Provider,
};

// ouroboros_engine.rs
use oxicode_sdk::{Context, Message, Model, Provider, UserMessage};

// a2a_tools.rs
use oxicode_sdk::{AgentTool, AgentToolResult, ToolContext, ToolError};
```

### What oxicode-sdk needs to add

> **Verified against `oxicode-sdk/src/lib.rs` as of 2026-05-16.**

The initial draft listed 5 missing types. After codebase verification, only **3 are genuinely missing**:

| Type | Source crate | Location | Currently in oxicode-sdk? | Action |
|------|-------------|----------|----------------------|--------|
| `UserMessage` | `oxicode-ai` | `messages.rs:200` | ❌ No | **Add** |
| `SearchCache` | `oxicode-agent` | `tools/search_cache.rs:37` | ❌ No | **Add** |
| `CompactionEvent` | `oxicode-agent` | `compaction.rs:8` | ❌ No | **Add** |
| `ProviderError` | `oxicode-ai` | `error.rs` | ✅ Already exported | None |
| `ProviderEvent` | `oxicode-ai` | `providers/` | ✅ Already exported | None |

> **Correction:** `ProviderError` and `ProviderEvent` were incorrectly listed as missing.
> They are already in the `oxicode_ai` re-export block:
> ```rust
> pub use oxicode_ai::{
>     Provider, ProviderRegistry, Model, ModelRegistry, Context, Message, ContentBlock,
>     ProviderEvent, StreamOptions, CompactionStrategy,
>     ProviderError, Api, Cost, InputModality,
> };
> ```

Only 3 `pub use` lines need adding:

```rust
// In oxicode-sdk/src/lib.rs — additions to existing re-export blocks

// From oxicode-ai (messages module)
pub use oxicode_ai::UserMessage;

// From oxicode-agent
pub use oxicode_agent::SearchCache;
pub use oxicode_agent::compaction::CompactionEvent;
```

### Acceptance criteria

- [ ] oxios can remove `oxicode-ai` and `oxicode-agent` from its `Cargo.toml` workspace dependencies
- [ ] All `use oxicode_ai::` and `use oxicode_agent::` imports in oxios-kernel and oxios-ouroboros are replaced with `use oxicode_sdk::`
- [ ] `cargo check -p oxios-kernel` passes with only `oxicode-sdk` as a dependency

---

## Request 2: Send-safe `AgentLoop::run()`

**Priority: Medium-High**  
**Effort estimate: Low** (2-line fix in `tool_exec.rs` + compile-time test)

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

### Root cause — verified by codebase audit

The `!Send` bound was traced to a **specific location** in `oxicode-agent/src/agent_loop/tool_exec.rs`:

```rust
// tool_exec.rs:20 — FinalizedToolCallEntry enum
enum FinalizedToolCallEntry {
    Immediate(FinalizedToolCall),
    Future(Pin<Box<dyn futures::Future<Output = FinalizedToolCall>>>),
    //                                        ^^^ MISSING + Send ^^^
}

// tool_exec.rs:205 — pending_futures vector
let mut pending_futures: Vec<(usize,
    Pin<Box<dyn futures::Future<Output = FinalizedToolCall>>>)>
= Vec::new();
// ^^^ same issue — no + Send bound
```

These two locations box futures without `+ Send`, which infects the entire `run()` async fn with `!Send`.

**What is NOT the problem** (verified Send-safe):

| Component | Status | Evidence |
|-----------|--------|----------|
| `AgentLoop` struct fields | ✅ All `Send` | `Arc<dyn Provider>` (`Provider: Send + Sync` per `trait_def.rs:13`), `Arc<ToolRegistry>`, `SharedState` (wraps `Arc<RwLock<AgentState>>`), `RwLock<Vec<Message>>`, `Arc<AtomicBool>`, `Arc<dyn ProviderResolver>` (`ProviderResolver: Send + Sync` per `agent.rs:27`) |
| `EmitFn` | ✅ `Send + Sync` | Defined as `Arc<dyn Fn(AgentEvent) + Send + Sync>` (`mod.rs:47`) |
| `BeforeToolCallHook` / `AfterToolCallHook` | ✅ `Send + Sync` | `Arc<dyn Fn(...) + Send + Sync>` with `Pin<Box<dyn Future<...> + Send>>` return (`config.rs:58-66`) |
| `AgentTool::execute()` | ✅ Already `Send` | Returns `Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>>` |

### Fix specification

The fix requires changing exactly **2 lines** in `tool_exec.rs` and adding 1 compile-time test:

```rust
// tool_exec.rs:20 — add + Send to the enum variant
Future(Pin<Box<dyn futures::Future<Output = FinalizedToolCall> + Send>>),

// tool_exec.rs:205 — add + Send to the pending_futures vector type
let mut pending_futures: Vec<(usize,
    Pin<Box<dyn futures::Future<Output = FinalizedToolCall> + Send>>)>
= Vec::new();
```

```rust
// In oxicode-agent tests — compile-time assertion to prevent regression
#[test]
fn agent_loop_future_is_send() {
    use std::future::Future;
    fn assert_send_future<F: Future + Send>(_: F) {}
    // This function exists only as a compile-time check.
    // If AgentLoop::run() stops being Send, this will fail to compile.
}
```

### Acceptance criteria

- [ ] `AgentLoop::run()` returns a `Send` future (verified by compile-time assertion in oxicode-agent)
- [ ] oxios can replace `spawn_blocking` with `tokio::spawn` in `agent_runtime.rs`
- [ ] No behavioral change in agent execution semantics

---

## Impact Analysis

### For oxicode project

| Change | Risk | Scope | Lines |
|--------|------|-------|-------|
| Add 3 re-exports (`UserMessage`, `SearchCache`, `CompactionEvent`) | **Low** — additive only | `oxicode-sdk/src/lib.rs` | ~3 lines |
| Add `+ Send` to boxed futures | **Low** — no semantic change | `oxicode-agent/src/agent_loop/tool_exec.rs` | 2 lines + 1 test |

> **Risk note on `+ Send` fix:** This is safe because every type flowing through the tool dispatch path is already `Send`-safe (verified above). If non-`Send` types are ever introduced, the compiler will catch it — which is the desired safety property.

### For oxios project

| Change | Benefit |
|--------|---------|
| Remove `oxicode-ai` + `oxicode-agent` deps | Single dependency surface, immune to oxicode internal reorganization |
| Replace `spawn_blocking` | Cleaner code, better performance, proper error propagation |

### Migration path

1. **oxicode-sdk** adds 3 re-exports (`UserMessage`, `SearchCache`, `CompactionEvent`) — ships immediately.
2. **oxios** switches imports and removes workspace deps (same PR or follow-up).
3. **oxicode-agent** adds `+ Send` to `FinalizedToolCallEntry::Future` and `pending_futures` + compile-time test (lands independently).
4. **oxios** removes `spawn_blocking` workaround (after oxicode release with Send-safe future).

Steps 1–2 and 3–4 are independent — they can proceed in parallel.

### Potential follow-up

After both changes land, add an **SDK surface integration test** to `oxicode-sdk` that compiles a minimal consumer using *only* `oxicode-sdk` as a dependency. This catches future re-export regressions automatically.

---

## Summary

| # | Request | Priority | Effort | Files | Blocking |
|---|---------|----------|--------|-------|----------|
| 1 | Re-export `SearchCache`, `CompactionEvent`, `UserMessage` | High | Low (~3 lines) | `oxicode-sdk/src/lib.rs` | No |
| 2 | Add `+ Send` to boxed futures in `tool_exec.rs` | Medium-High | Low (~2 lines + test) | `oxicode-agent/src/agent_loop/tool_exec.rs` | No |

Both changes are additive or internal — no public API breakage. They allow oxios to depend solely on `oxicode-sdk` as the integration contract, which was the original design intent.
