---
description: Use std::sync::LazyLock for lazy static initialization
condition: "lazy_static|once_cell"
scope: "text"
interruptMode: prose-only
---
# Use `std::sync::LazyLock` for lazy static initialization

Since Rust 1.80, the standard library provides `std::sync::LazyLock` as a replacement for the `lazy_static!` macro and `once_cell` crate. Use the standard library type for zero-dependency lazy initialization.

## Examples

**Avoid:**
```rust
use lazy_static::lazy_static;
use std::collections::HashMap;

lazy_static! {
    static ref CACHE: parking_lot::Mutex<HashMap<String, Vec<u8>>> =
        parking_lot::Mutex::new(HashMap::new());
}
```

**Avoid:**
```rust
use once_cell::sync::Lazy;
use std::collections::HashMap;

static CACHE: Lazy<parking_lot::Mutex<HashMap<String, Vec<u8>>>> =
    Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));
```

**Prefer:**
```rust
use std::sync::LazyLock;
use std::collections::HashMap;

static CACHE: LazyLock<parking_lot::Mutex<HashMap<String, Vec<u8>>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));
```

## Reasoning

- Zero external dependencies — `LazyLock` is in `std`
- Same API as `once_cell::sync::Lazy`
- Stabilized and ready for production use
- One fewer crate to audit, update, and compile
