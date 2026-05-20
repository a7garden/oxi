# Fix: Make oxi-cli actually use oxi-sdk's OxiBuilder

## Summary
Refactored `App::new()` in `oxi-cli` to use `OxiBuilder` for provider/model resolution instead of calling global `get_model()`/`get_provider()` directly. This makes the CLI the first real consumer of the SDK, validating its API.

## Changes

### 1. `oxi-sdk/src/lib.rs` — Re-export core types
Added public re-exports so consumers can `use oxi_sdk::{Oxi, OxiBuilder}`:
```rust
pub use builder::{Oxi, OxiBuilder};
pub use agent_builder::AgentBuilder;
```

### 2. `oxi-cli/src/lib.rs` — Refactored App::new()

**Before:** Called `get_model()` and `get_provider()` (global functions from oxi-ai).
**After:** Creates an `Oxi` engine via `OxiBuilder::new().with_builtins().build()` and uses:
- `engine.resolve_model()` for model validation
- `engine.create_provider()` for provider instantiation

**Structural changes:**
- Added `engine: oxi_sdk::Oxi` field to `App` struct (with doc comment)
- Added `pub(crate) fn engine(&self)` accessor for future use
- Removed direct imports of `get_model`, `get_provider` from `oxi_ai`
- Added `use oxi_sdk::OxiBuilder` import

### 3. `oxi-cli/src/main.rs` — No changes (conservative approach)
`register_custom_providers()` continues to register custom providers into the global registry via `oxi_ai::register_provider()`. This works because `ProviderRegistry::get()` (used internally by `engine.create_provider()`) falls back to the global registry, so custom providers are automatically discovered.

## Verification Results
```
cargo check -p oxi-cli     → 0 errors
cargo test -p oxi-cli --lib → 307 passed; 0 failed
cargo build --release       → success
```

## How it works end-to-end
1. `main.rs` calls `register_custom_providers(&settings)` → registers custom providers into global state
2. `main.rs` calls `App::new(settings)` → creates `OxiBuilder::new().with_builtins().build()` 
3. `engine.create_provider("anthropic")` → checks instance registry (empty), falls back to global built-in providers
4. `engine.create_provider("custom-llm")` → checks instance registry (empty), falls back to global registry where custom provider was registered in step 1
5. `engine.resolve_model("anthropic/claude-sonnet-4-20250514")` → resolves from `ModelRegistry::from_static()` (built-in models)
