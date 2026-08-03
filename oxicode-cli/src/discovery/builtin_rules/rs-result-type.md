---
description: Prefer Result<T, E> over panic for fallible operations
condition: "unwrap\\(\\)|expect\\("
scope: "text"
interruptMode: prose-only
---
# Prefer `Result<T, E>` over `unwrap()` / `expect()`

`unwrap()` and `expect()` cause panics on error, which is unacceptable in production code. Fallible operations should return `Result` and let callers handle errors gracefully.

## Examples

**Avoid:**
```rust
fn load_config(path: &str) -> Config {
    let data = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&data).expect("valid JSON")
}
```

**Prefer:**
```rust
fn load_config(path: &str) -> anyhow::Result<Config> {
    let data = std::fs::read_to_string(path)?;
    let config = serde_json::from_str(&data)?;
    Ok(config)
}
```

## When `unwrap()` / `expect()` are acceptable

- In tests (`#[cfg(test)]`) — panics are idiomatic for test assertions
- When the invariant is locally provable (e.g., `"known value".parse::<u32>().unwrap()`)
- In examples and documentation code
- During one-shot initialization (e.g., `LazyLock::new(|| …)`)

## Reasoning

- Panics crash the program with no recovery path
- `Result` + `?` propagates errors to a decision point
- Callers can choose how to handle failures (retry, fallback, report)
- Distinguishes truly unrecoverable bugs from expected failure modes
