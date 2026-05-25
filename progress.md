# Progress

## Status
In Progress — Steps 1, 2 & 3 complete

## Tasks
- [x] 1. oxi-ai/src/router/mod.rs — all 7 sub-changes applied
- [x] 2. Router integration across 5 files (lib.rs, model_registry.rs, model_id.rs, settings.rs, main.rs)
- [x] 3. TUI router changes across 6 files (overlay/mod.rs, factories.rs, handlers.rs, slash.rs, app.rs, render.rs)
- [ ] 4. (next)

## Files Changed
- `oxi-ai/src/router/mod.rs` — Added RouterSnapshot struct + global static; added update_snapshot/get_snapshot methods to RouterProvider; fixed context truncation to preserve system prompt (remove(0)→remove(1)); replaced self-referential tier config fallback with explicit error; changed ThinkingLevel default from Medium to Off; added self.update_snapshot() call after record_decision; updated truncation comment
- `oxi-ai/src/lib.rs` — Added `dynamic_models` to model_registry re-exports
- `oxi-ai/src/model_registry.rs` — Added `dynamic_models()` method to ModelRegistry and convenience function
- `oxi-agent/src/model_id.rs` — Rewrote to check dynamic registry first (lookup_model) before static (get_model), enabling router/auto resolution
- `oxi-store/src/settings.rs` — Added `router_profile: Option<String>` field, Default impl entry, and `router_profile()` helper method
- `oxi-cli/src/main.rs` — Added `register_router_provider()` call after custom providers; added full `register_router_provider()` function that registers router/auto model, loads router config, converts store types to AI types, and calls `register_router()`
- `oxi-cli/src/tui/overlay/mod.rs` — Added `OpenRouterSetup` variant to `OverlayAction`; added `router_setup` and `router_integration` module declarations and re-exports
- `oxi-cli/src/tui/overlay/factories.rs` — Added router auto-setup guard in `ModelSelectOverlay::handle_key()` Enter handler: checks for router/* prefix and opens setup overlay if no config exists
- `oxi-cli/src/tui/overlay/router_setup.rs` — Added `router_setup()` factory function for creating boxed overlay component
- `oxi-cli/src/tui/handlers.rs` — Added `OverlayAction::OpenRouterSetup` dispatch arm with router setup overlay creation; updated Ctrl+R handler to pull real router snapshot data
- `oxi-cli/src/tui/slash.rs` — Added `/router` command (status/pin/disable subcommands + auto-setup); updated `/model` to include dynamic models; added `router_help()` function; updated `format_help()` with `/router` entry
- `oxi-cli/src/tui/app.rs` — Added `#[allow(dead_code)]` to `ProviderInfo` struct, `Setup::Done` variant, and `AppOverlay::Setup` variant
- `oxi-cli/src/tui/render.rs` — Added `#[allow(dead_code)]` to `render_provider_list`, `render_input_field`, and `render_setup_step` functions

## Notes
- Release build compiles clean: `cargo build --release` ✓
- Clippy clean: `cargo clippy -p oxi-cli -- -D warnings` ✓
- Fixed pre-existing error in `router_integration.rs` (removed non-existent `vision` field from ScoringWeights)
