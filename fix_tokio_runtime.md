# Fix: Tokio Runtime Recreation in TUI Session Switching

## Problem
Every time a session switches, the TUI's agent worker thread created a brand-new multi-threaded Tokio runtime:

```rust
let rt = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(2)
    .enable_all()
    .build()
    .expect("Failed to build agent runtime");
```

This is expensive — `new_multi_thread()` spawns multiple OS threads, sets up the thread pool, and allocates internal structures on every session switch. For a CLI that switches sessions frequently, this wastes significant CPU and memory.

## Solution
Added a process-lifetime shared runtime via `OnceLock`:

```rust
fn get_agent_runtime() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to build agent runtime")
    })
}
```

Replaced the per-session runtime creation with `let rt = get_agent_runtime();`.

## Changes Made

### `oxi-cli/src/tui/app.rs`
1. **Added `get_agent_runtime()` function** (after imports, before `run_tui_interactive`) — a `OnceLock`-guarded process-lifetime multi-thread Tokio runtime.
2. **Replaced runtime creation** in the agent worker thread spawn block (line ~688): `tokio::runtime::Builder::new_multi_thread()...build()` → `get_agent_runtime()`.

## No Shutdown Calls to Remove
There were no `rt.shutdown_*()` calls in the code — the runtime was dropped implicitly when the thread ended. With this fix, the runtime lives for the entire process lifetime, so no shutdown management is needed.

## Verification
- `cargo check --package oxi-cli` compiles with zero errors (pre-existing warnings only)
- Logic unchanged — only the runtime lifecycle changed
- Thread safety: `OnceLock` guarantees the runtime is created exactly once, on first use
