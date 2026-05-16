# Progress

## Status
In Progress

## Tasks
- [x] Refactor 1: Agent accepts pre-built ToolRegistry
- [x] Refactor 2: Oxi holds real ProviderRegistry + proper model loading
- [x] Refactor 3: oxi-cli workspace_dir flow + Agent::new() 3-arg migration

## Files Changed — Refactor 1
- `oxi-agent/src/agent.rs` — Changed `Agent::new()` to accept `Arc<ToolRegistry>`, added `new_empty()` convenience constructor
- `oxi-agent/src/tests.rs` — Updated all 8 call sites to pass `Arc::new(ToolRegistry::new())`
- `oxi-cli/src/lib.rs` — Updated 1 call site (App::new)
- `oxi-cli/src/app/agent_session.rs` — Updated 1 call site (test helper)
- `oxi-cli/src/app/agent_session_runtime.rs` — Updated 2 call sites (fallback path and main path)
- `oxi-sdk/src/agent_builder.rs` — KEY CHANGE: passes builder's `self.tools` via `Arc::new(self.tools)` directly to `Agent::new()`, eliminating the post-creation tool registration loop

## Files Changed — Refactor 2
- `oxi-ai/src/providers/mod.rs` — Added `ProviderRegistry` struct with `new()`, `register()`, `register_arc()`, `remove()`, `names()`, `get()` methods. `get()` checks local custom providers first, then falls back to built-in providers via `get_provider()`. Returns `Option<Arc<dyn Provider>>`. Preserved backward-compatible global functions.
- `oxi-ai/src/lib.rs` — Added `ProviderRegistry` to re-exports
- `oxi-sdk/src/builder.rs` — `Oxi` now holds `Arc<ProviderRegistry>`, `Arc<ModelRegistry>`, and `Arc<ToolRegistry>`. Added `providers()` accessor. `create_provider()` returns `Result<Arc<dyn Provider>>` using `ProviderRegistry::get()`. `OxiBuilder` has new `provider()` method.
- `oxi-sdk/src/agent_builder.rs` — Updated to use `Arc<dyn Provider>` directly from `Oxi::create_provider()` (no more Box-to-Arc conversion)
- `oxi-sdk/src/lib.rs` — Added `ProviderRegistry` and `ModelRegistry` to re-exports

## Files Changed — Refactor 3
- `oxi-cli/src/lib.rs` — Changed `workspace_dir: None` → `workspace_dir: Some(cwd)` in App::new() AgentConfig
- `oxi-cli/src/app/agent_session_runtime.rs` — Changed `workspace_dir: None` → `workspace_dir: Some(services.cwd.clone())` in both AgentConfig construction sites (fallback path and main path)

## Notes
- `cargo check --workspace --lib`: 0 errors
- `cargo check -p oxi-cli` (bin target): 0 errors
- `cargo test --workspace --lib`: 1209 tests passed, 0 failed (6 crates)
