---
description: Never use Box::leak to satisfy a lifetime
condition: "Box::leak"
scope: "text"
interruptMode: prose-only
---
# Never use `Box::leak` to satisfy a lifetime

`Box::leak` intentionally leaks heap-allocated memory. Using it to work around lifetime errors is always wrong — it masks the underlying design problem and introduces a memory leak that will never be reclaimed.

## Examples

**Avoid:**
```rust
fn cache_data(data: Vec<u8>) -> &'static [u8] {
    Box::leak(data.into_boxed_slice())
}
```

**Prefer — redesign the API to own the data:**
```rust
use std::sync::Arc;

struct Cache {
    data: Arc<[u8]>,
}

fn cache_data(cache: &mut Cache, data: Vec<u8>) {
    cache.data = data.into();
}
```

**Or use a static container:**
```rust
use std::sync::LazyLock;

static CACHE: LazyLock<parking_lot::Mutex<Option<Vec<u8>>>> = LazyLock::new(Default::default);
```

## Reasoning

- `Box::leak` creates permanent memory leaks
- Leaked allocations are invisible to tooling (valgrind, sanitizers)
- There is always a better design: `Arc`, arena allocators, or owned containers
- The `'static` lifetime obtained from `Box::leak` is a lie — the data is not truly static
