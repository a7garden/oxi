---
description: Use Future directly instead of std::future::Future in type positions
condition: "std::future::Future"
scope: "text"
interruptMode: prose-only
---
# Prefer `Future` over `std::future::Future`

The `Future` trait is re-exported in the standard prelude. Writing `std::future::Future` in type positions is unnecessarily verbose and hurts readability.

## Examples

**Avoid:**
```rust
use std::future::Future;
use std::pin::Pin;

fn process(f: Pin<Box<dyn std::future::Future<Output = ()>>>) {}
```

**Prefer:**
```rust
use std::future::Future;
use std::pin::Pin;

fn process(f: Pin<Box<dyn Future<Output = ()>>>) {}
```

The prelude import makes `Future` available everywhere without qualification. Only use the full path when disambiguation is required (e.g., when a local `Future` name shadows the trait).

## Reasoning

- Reduces visual noise in type signatures
- Consistent with other prelude traits like `Send`, `Sync`, `Sized`
- Standard library convention: all stable `std` traits are available via prelude
