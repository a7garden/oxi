---
description: Use match ergonomics instead of explicit ref/ref mut
condition: "ref mut|\\bref\\s"
scope: "text"
interruptMode: prose-only
---
# Use match ergonomics instead of explicit `ref` / `ref mut`

Rust 2018 introduced match ergonomics, which automatically binds references in patterns. Explicit `ref` and `ref mut` keywords are almost never needed in modern Rust.

## Examples

**Avoid:**
```rust
match &option {
    Some(ref value) => process(value),
    None => {}
}
```

**Prefer:**
```rust
match &option {
    Some(value) => process(value),
    None => {}
}
```

**Avoid:**
```rust
match &mut collection {
    items @ &mut ref mut vec => process_mut(vec),
}
```

**Prefer:**
```rust
match &mut collection {
    items => process_mut(items),
}
```

## Reasoning

- Reduces visual noise and cognitive load
- The compiler infers the correct binding mode automatically
- Consistent with modern Rust idioms used in the standard library and ecosystem
- Explicit `ref`/`ref mut` is only needed in rare cases where automatic inference is ambiguous
