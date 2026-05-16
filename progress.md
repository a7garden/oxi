# Progress

## Status
In Progress

## Tasks
- [x] Make oxi-cli use oxi-sdk's OxiBuilder for provider/model resolution

## Files Changed
- `oxi-sdk/src/lib.rs` — Added `pub use` re-exports for `Oxi`, `OxiBuilder`, `AgentBuilder`
- `oxi-cli/src/lib.rs` — Refactored `App::new()` to use `OxiBuilder` instead of global `get_model()`/`get_provider()`
  - Added `engine: oxi_sdk::Oxi` field to `App` struct
  - Added `pub(crate) fn engine()` accessor
  - Provider resolution now goes through `engine.create_provider()`
  - Model validation now goes through `engine.resolve_model()`
- `oxi-cli/src/main.rs` — No changes needed (conservative approach)

## Notes
- `register_custom_providers()` in main.rs is unchanged — it registers into global state via `oxi_ai::register_provider()`. The `ProviderRegistry::get()` method already falls back to the global registry (including custom providers), so everything works seamlessly.
- Engine field stored in App for future use (e.g. model switching, dynamic provider creation).
- All checks pass: `cargo check` (0 errors), `cargo test -p oxi-cli --lib` (307 passed), `cargo build --release` (success).
