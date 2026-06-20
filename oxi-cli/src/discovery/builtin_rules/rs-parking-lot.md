---
description: Use parking_lot::RwLock instead of std::sync::RwLock
condition: "std::sync::RwLock"
scope: "text"
interruptMode: prose-only
---
# Prefer `parking_lot::RwLock` over `std::sync::RwLock`

`parking_lot::RwLock` is faster, smaller, and more ergonomic than `std::sync::RwLock`. It avoids poisoning, supports downgradable read guards, and has a more efficient implementation.

## Examples

**Avoid:**
```rust
use std::sync::RwLock;

let lock = RwLock::new(42);
let value = *lock.read().unwrap();
```

**Prefer:**
```rust
use parking_lot::RwLock;

let lock = RwLock::new(42);
let value = *lock.read();
```

## Key differences

| Feature | `std::sync::RwLock` | `parking_lot::RwLock` |
|---|---|---|
| Poisoning | Yes (mutex poisoned on panic) | No |
| `read()` return | `LockResult<RwLockReadGuard>` | `RwLockReadGuard` (no unwrap needed) |
| Downgradable guards | No | Yes |
| Fairness | OS-dependent | Fair by default |
| Contention | Slower | Faster (uses futex-like primitives) |

## Reasoning

- No `.unwrap()` needed — eliminates an entire class of error-handling boilerplate
- Better performance under contention
- Downgrade from write to read without releasing the lock
- Widely adopted in the Rust ecosystem (tokio, rayon, ripgrep)
