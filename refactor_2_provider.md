# Refactor 2: Oxi holds real ProviderRegistry + proper model loading

## Summary

Created a proper `ProviderRegistry` struct and integrated it into the `Oxi` SDK instance, alongside `ModelRegistry`, for full test isolation and instance-based provider management.

## What was found (Step 1 & 5)

### ProviderRegistry API (did not exist before)
- There was no `ProviderRegistry` struct — providers were managed via global static functions: `register_provider()`, `unregister_provider()`, `custom_provider_names()`, `get_provider()`
- `get_provider()` returns `Option<Box<dyn Provider>>` — it checks the global `CUSTOM_PROVIDERS` hashmap, then falls back to built-in providers
- The global functions are still preserved for backward compatibility with `oxi-cli`

### ModelRegistry API
- `ModelRegistry::from_static()` exists — loads all static models into an instance
- `ModelRegistry::new()` — creates empty registry
- `ModelRegistry::register(model)`, `lookup(provider, model_id)`, `model_ids()` — all instance methods
- Dynamic models take priority over static in `lookup()`

### Exports
- `ProviderRegistry` is now exported from `oxi_ai` via `oxi-ai/src/lib.rs`
- `ModelRegistry` was already exported

## Changes Made

### 1. `oxi-ai/src/providers/mod.rs` — New `ProviderRegistry` struct

```rust
pub struct ProviderRegistry {
    custom: RwLock<HashMap<String, Arc<dyn Provider>>>,
}
```

Methods:
- `new()` — empty registry
- `register(name, impl Provider)` — add a custom provider
- `register_arc(name, Arc<dyn Provider>)` — add pre-Arced provider
- `remove(name)` — remove a custom provider
- `names()` — list registered custom provider names
- `get(name) -> Option<Arc<dyn Provider>>` — checks local custom map first, falls back to built-in providers via `get_provider()`, converting `Box` to `Arc`

The global `register_provider()`, `unregister_provider()`, `custom_provider_names()` functions are preserved for backward compatibility.

### 2. `oxi-ai/src/lib.rs` — Added `ProviderRegistry` export

### 3. `oxi-sdk/src/builder.rs` — Updated `Oxi` struct

```rust
pub struct Oxi {
    providers: Arc<ProviderRegistry>,
    models: Arc<ModelRegistry>,
    tools: Arc<ToolRegistry>,
}
```

- Added `providers()` accessor
- `create_provider()` now returns `Result<Arc<dyn Provider>>` via `ProviderRegistry::get()`
- `OxiBuilder` has new `provider(name, impl Provider)` method

### 4. `oxi-sdk/src/agent_builder.rs` — Simplified provider creation

Now uses `Arc<dyn Provider>` directly from `Oxi::create_provider()`, no Box-to-Arc conversion needed.

### 5. `oxi-sdk/src/lib.rs` — Added `ProviderRegistry`, `ModelRegistry` to re-exports

## Verification

```
$ cargo check --workspace --lib 2>&1 | grep '^error' | wc -l
       0

$ cargo test --workspace --lib
test result: ok. 79 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## Design Decisions

1. **ProviderRegistry::get() returns `Option<Arc<dyn Provider>>`** — not `Box`. This is the natural type for shared provider instances and avoids repeated Box→Arc conversions.

2. **Backward compatibility** — Global functions (`register_provider`, `get_provider`, etc.) are preserved. The CLI still uses them. Migration can happen incrementally.

3. **Builder-pattern isolation** — Each `OxiBuilder::build()` creates a fresh `ProviderRegistry` and `ModelRegistry` wrapped in `Arc`, ensuring test isolation without global state mutation.

4. **Fallback to builtins** — `ProviderRegistry::get()` delegates to `get_provider()` for built-in providers, so no provider enumeration is needed at construction time.
