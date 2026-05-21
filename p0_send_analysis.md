# P0 Send Analysis: `AgentLoop::run()` Future is `!Send`

## Executive Summary

**Confirmed:** `AgentLoop::run()` returns a `!Send` future. The `spawn_blocking` workaround in `run_tokio_stream()` (agent.rs:688) is **necessary and correct** given the current code.

**Root cause:** `RetryCallback` trait in `stream_retry.rs` is `Send` but **not `Sync`**. This causes `&dyn RetryCallback` to be `!Send`, and it is held across an `.await` point in `stream_with_retry_core()`.

---

## Finding #1: `AgentLoop` struct IS `Send + Sync`

The `AgentLoop` struct itself is fully `Send + Sync`. All fields use thread-safe primitives:

| Field | Type | Send | Sync |
|-------|------|------|------|
| `provider` | `Arc<dyn Provider>` | ✅ | ✅ |
| `config` | `AgentLoopConfig` | ✅ | ✅ |
| `tools` | `Arc<ToolRegistry>` | ✅ | ✅ |
| `state` | `SharedState` (Arc<parking_lot::RwLock>) | ✅ | ✅ |
| `compaction_manager` | `OxCompactionManager` | ✅ | ✅ |
| `before_tool_call` | `Option<BeforeToolCallHook>` (Arc) | ✅ | ✅ |
| `after_tool_call` | `Option<AfterToolCallHook>` (Arc) | ✅ | ✅ |
| `steering_queue` | `RwLock<Vec<Message>>` (parking_lot) | ✅ | ✅ |
| `follow_up_queue` | `RwLock<Vec<Message>>` (parking_lot) | ✅ | ✅ |
| `circuit_breaker` | `CircuitBreaker` (atomics + parking_lot::Mutex) | ✅ | ✅ |
| `external_stop` | `Arc<AtomicBool>` | ✅ | ✅ |
| `resolver` | `Arc<dyn ProviderResolver>` | ✅ | ✅ |
| `auto_retry_attempt` | `AtomicUsize` | ✅ | ✅ |
| `auto_retry_cancel` | `AtomicBool` | ✅ | ✅ |

**No `RefCell`, `Rc`, `UnsafeCell`, or `std::cell` types found anywhere** in `agent_loop/`.

A compile-time static assertion confirms this:
```rust
fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}
assert_send::<AgentLoop>();  // ✅ compiles
assert_sync::<AgentLoop>();  // ✅ compiles
```

---

## Finding #2: The `run()` future is `!Send` — Exact Cause

### The !Send type chain

```
AgentLoop::run()
  → run_messages()
    → run_loop()
      → stream_assistant_response()
        → stream_with_retry()
          → stream_with_retry_core()  ← HERE
```

### File: `oxi-agent/src/stream_retry.rs`

```rust
// Line 21: RetryCallback is Send but NOT Sync
pub trait RetryCallback: Send {
    fn on_retry(&self, attempt: usize, max_retries: usize, delay_secs: u64, reason: String);
}

// Line 41-54: Takes &dyn RetryCallback which is !Send
pub async fn stream_with_retry_core(
    provider: &dyn oxi_ai::Provider,
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
    retry_cb: &dyn RetryCallback,  // ← &dyn !Sync = !Send
    max_delay: Option<u64>,
    on_success: impl Fn(),
    on_failure: impl Fn(),
) -> Result<BoxStream<'static, ProviderEvent>, AgentError> {
    // ...
    for attempt in 0..=MAX_RETRIES {
        match provider.stream(model, context, options.clone()).await {
            //              ^^^^^ await point with `retry_cb` still live ^^^^^
```

### Compiler error (reproduced):

```
error: future created by async block is not `Send`
  = help: the trait `Sync` is not implemented for `dyn RetryCallback`
note: future is not `Send` as this value is used across an await
  --> oxi-agent/src/stream_retry.rs:54:64
   |
46 |     retry_cb: &dyn RetryCallback,
   |     -------- has type `&dyn RetryCallback` which is not `Send`
...
54 |         match provider.stream(model, context, options.clone()).await {
   |                                                                ^^^^^ await occurs here, with `retry_cb` maybe used later
```

### Why `&dyn RetryCallback` is `!Send`

A shared reference `&T` is `Send` if `T: Sync`. Since `RetryCallback: Send` but `!Sync`, `&dyn RetryCallback` is `!Send`. When this reference is held across the `.await` at line 54, the entire future becomes `!Send`.

---

## Finding #3: The `spawn_blocking` workaround IS necessary

### File: `oxi-agent/src/agent.rs` lines 686-688

```rust
let handle = tokio::task::spawn(async move {
    // AgentLoop internals are !Send (dyn Future without Send bound),
    // so we use spawn_blocking to run on a blocking thread.
    let result = tokio::task::spawn_blocking(move || {
        // Create a new tokio runtime for the blocking thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime");
        rt.block_on(async move {
            agent_loop.run(prompt, |event| { ... }).await
        })
    })
    .await;
```

**This workaround IS currently necessary.** The comment on line 686 says "dyn Future without Send bound" which is slightly imprecise — the actual cause is `RetryCallback: !Sync` — but the conclusion is correct: the future from `run()` is `!Send`.

The pattern of `spawn_blocking` + nested `new_current_thread()` runtime is a valid (if ugly) escape hatch for `!Send` futures.

---

## Finding #4: Other `!Send` risk points (none found)

All other async code in `agent_loop/` was checked:

- **`tool_exec.rs`**: `FinalizedToolCallEntry::Future` has `Pin<Box<dyn Future<...> + Send>>` — correctly Send-bounded.
- **`streaming.rs`**: Uses `BoxStream<'static, ProviderEvent>` from `stream_with_retry` — Send.
- **`helpers.rs`**: Pure synchronous code, no async.
- **`queues.rs`**: Pure synchronous code, no async.
- **`config.rs`**: `BeforeToolCallHook` / `AfterToolCallHook` return `Pin<Box<dyn Future<...> + Send>>` — correctly Send-bounded.
- **`retry.rs`**: `handle_retryable_error` uses `tokio::time::sleep` and atomics — all Send.

**The ONLY source of `!Send` is `RetryCallback: !Sync` in `stream_retry.rs`.**

---

## Recommended Fix

### Option A: Add `Sync` to `RetryCallback` (minimal change)

```rust
// stream_retry.rs line 21
pub trait RetryCallback: Send + Sync {  // ← add Sync
    fn on_retry(&self, attempt: usize, max_retries: usize, delay_secs: u64, reason: String);
}
```

This is a one-line fix. `EmitRetryCallback` already satisfies `Sync` (its fields are `&EmitFn` which is `Sync` and `Option<String>` which is `Sync`).

**After this fix, the entire `run()` future chain would become `Send`,** and `run_tokio_stream()` could be simplified to:

```rust
let handle = tokio::task::spawn(async move {
    agent_loop.run(prompt, |event| { ... }).await
});
```

### Option B: Use `Arc<dyn RetryCallback + Send + Sync>` instead of `&dyn`

This avoids the `&dyn` reference entirely but requires more refactoring.

### Option A is recommended.

---

## Impact Assessment

| Aspect | Current (with bug) | After fix |
|--------|-------------------|-----------|
| `AgentLoop` struct | `Send + Sync` ✅ | `Send + Sync` ✅ |
| `run()` future | `!Send` ❌ | `Send` ✅ |
| `spawn_blocking` workaround | Required | Can be removed |
| Nested runtime overhead | Yes (create + destroy per run) | Eliminated |
| Thread pool pressure | Consumes blocking thread | Uses normal tokio task |
| State sync back | Complex (SharedState across runtimes) | Simplified |

---

## Verification Commands

```bash
# 1. Verify AgentLoop is Send + Sync (passes today)
cargo test -p oxi-agent --lib agent_loop_is_send_and_sync

# 2. Verify run() future is !Send (compile error today)
# Attempt to tokio::spawn the future — will fail with:
#   "future is not Send" / "Sync is not implemented for dyn RetryCallback"

# 3. After fix: verify run() future is Send
# The same tokio::spawn test should compile and pass
```

---

## Files Analyzed

| File | Status |
|------|--------|
| `oxi-agent/src/agent.rs` | ✅ All types Send. `spawn_blocking` workaround at line 688. |
| `oxi-agent/src/agent_loop/mod.rs` | ✅ All types Send. `AgentLoop` struct is `Send + Sync`. |
| `oxi-agent/src/agent_loop/tool_exec.rs` | ✅ `FinalizedToolCallEntry::Future` correctly `+ Send`. |
| `oxi-agent/src/agent_loop/config.rs` | ✅ Hook types correctly `+ Send`. |
| `oxi-agent/src/agent_loop/streaming.rs` | ✅ Uses `BoxStream<'static, ...>` (Send). |
| `oxi-agent/src/agent_loop/retry.rs` | ✅ `EmitRetryCallback` is `Send + Sync`. |
| `oxi-agent/src/agent_loop/helpers.rs` | ✅ Synchronous only. |
| `oxi-agent/src/agent_loop/queues.rs` | ✅ Synchronous only. |
| **`oxi-agent/src/stream_retry.rs`** | **❌ `RetryCallback: !Sync` → `&dyn RetryCallback` is `!Send`** |
| `oxi-ai/src/providers/trait_def.rs` | ✅ `Provider::stream()` returns `+ Send` stream. |
| `oxi-ai/src/compaction.rs` | ✅ `Compactor: Send + Sync`, `LlmCompactor` fields all Send. |
