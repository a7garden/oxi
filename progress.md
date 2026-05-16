# Progress

## Status
In Progress

## Tasks
- [x] Fix 5: Ensure oxi-cli still works + integration test
  - [x] Step 1: cargo check -p oxi-cli --lib — PASS (0 errors)
  - [x] Step 2: cargo check -p oxi-cli (bin) — PASS (0 errors)
  - [x] Step 3: Updated oxi-sdk/src/prelude.rs with full type re-exports
  - [x] Step 4: cargo check --workspace --lib — PASS (0 errors)
  - [x] Step 5: cargo test --workspace — PASS (1209 tests, 0 failures)
  - [x] Step 6: Release build + smoke test — PASS (oxi 0.12.0, GLM-5.1 responds)

- [x] Fix 1+4: Oxi struct holds real ModelRegistry + AgentBuilder.build() works
  - [x] Read oxi-ai/src/providers/mod.rs — no ProviderRegistry for instances; providers are stateless, created via get_provider()
  - [x] Read oxi-ai/src/model_registry.rs — ModelRegistry::new(), from_static(), register(Model), lookup(&str, &str) -> Option<Model>
  - [x] Read oxi-ai/src/model_db.rs — get_all_models() returns Iterator<Item=&ModelEntry>, NOT Model; used from_static() instead
  - [x] Read oxi-ai/src/providers/trait_def.rs — Provider has no clone_provider(); can't clone dyn Provider
  - [x] Rewrote oxi-sdk/src/builder.rs — Oxi holds Arc<ModelRegistry> + Arc<ToolRegistry>, OxiBuilder.with_builtins() uses ModelRegistry::from_static()
  - [x] Rewrote oxi-sdk/src/agent_builder.rs — build() resolves model from instance registry, creates provider, merges config, registers tools
  - [x] Updated oxi-sdk/src/lib.rs — added ModelRegistry re-export
  - [x] cargo check --workspace — 0 errors, 0 warnings

- [x] Fix 2+3: File tools store and use cwd, coding_tools() accepts cwd
  - [x] Updated ReadTool, WriteTool, EditTool, LsTool, GrepTool, FindTool, BashTool — added root_dir: PathBuf field, with_cwd() constructor
  - [x] All PathGuard::new() calls use self.root_dir instead of std::env::current_dir()
  - [x] BashTool validates cwd within root_dir workspace, defaults to root_dir when no cwd param
  - [x] Updated ToolRegistry::with_builtins_cwd() to pass cwd.clone() to all tool constructors
  - [x] Updated tool_factory.rs: coding_tools(&Path) and readonly_tools(&Path) accept cwd
  - [x] Updated oxi-sdk/src/lib.rs: removed global function re-exports (lookup_model, get_models, get_providers, get_provider)
  - [x] Updated test calls to pass root_dir parameter where needed
  - [x] cargo check --workspace --lib — 0 errors, 0 warnings
  - [x] cargo test --workspace --lib — 1309 tests, 0 failures

## Files Changed
- oxi-sdk/src/builder.rs — Oxi now holds Arc<ModelRegistry> instead of just tools; OxiBuilder.with_builtins() loads from static
- oxi-sdk/src/agent_builder.rs — build() uses instance model registry, proper tool registration
- oxi-sdk/src/lib.rs — added ModelRegistry re-export
- oxi-sdk/src/prelude.rs — expanded prelude re-exports (from prior fix)

## Notes
- ProviderRegistry does NOT exist as an instance registry — providers are stateless singletons created via get_provider(). The task's ProviderRegistry was actually ProviderAuthRegistry (auth, not provider instances).
- model_db::get_all_models() returns &ModelEntry not &Model, so we use ModelRegistry::from_static() which already has all static models loaded
- Provider trait has no clone method, so create_provider() returns Box<dyn Provider> by creating fresh instances each time
- All tool `new()` methods still use no-argument signatures (default to current dir)
- `ToolRegistry::with_builtins_cwd()` exists and works
- oxi-sdk, oxi-cli, oxi-agent, oxi-ai all compile cleanly
- 1209 workspace tests pass across all crates
