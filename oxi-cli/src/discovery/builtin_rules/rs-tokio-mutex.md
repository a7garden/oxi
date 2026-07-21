---
description: Use tokio::sync::Mutex when holding across .await
condition: "std::sync::Mutex"
scope: "text"
interruptMode: prose-only
---
# Use `tokio::sync::Mutex` when holding across `.await`

`std::sync::Mutex` must not be held across `.await` points — doing so can cause deadlocks because the lock is not `Send`-aware in async contexts. Use `tokio::sync::Mutex` for locks held across await boundaries.

## Examples

**Avoid:**
```rust
use std::sync::Mutex;

async fn update(lock: &Mutex<State>) {
    let mut guard = lock.lock().unwrap();
    let data = fetch_data().await; // DEADLOCK RISK
    guard.update(data);
}
```

**Prefer:**
```rust
use tokio::sync::Mutex;

async fn update(lock: &Mutex<State>) {
    let mut guard = lock.lock().await;
    let data = fetch_data().await;
    guard.update(data);
}
```

**Even better — minimize the critical section:**
```rust
use std::sync::Mutex;

async fn update(lock: &Mutex<State>) {
    let data = fetch_data().await;
    let mut guard = lock.lock().unwrap();
    guard.update(data);
}
```

## When `std::sync::Mutex` is still correct

- Lock is never held across `.await`
- Lock is held for short, synchronous operations only
- The data protected is simple and contention is low

## Reasoning

- `std::sync::Mutex` blocks the thread, starving the async runtime
- `tokio::sync::Mutex` yields to the runtime, allowing other tasks to make progress
- In async code, thread blocking = throughput collapse
