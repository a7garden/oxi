# Fix 1+4: Oxi struct holds real registries + AgentBuilder.build()

## Summary

Rewrote `Oxi` to hold a real `Arc<ModelRegistry>` instance and fixed `AgentBuilder.build()` to use instance registries instead of global functions. Clean compile: 0 errors, 0 warnings.

## Key Findings from Source Analysis

### 1. No `ProviderRegistry` for instances
- `oxi_ai::providers::ProviderRegistry` does **not** exist
- `oxi_ai::provider_registry::ProviderAuthRegistry` exists but manages **API keys**, not provider instances
- Providers are stateless and created on-demand via `get_provider()` → `Box<dyn Provider>`
- **Decision**: Oxi does NOT hold a provider registry; `create_provider()` delegates to `get_provider()`

### 2. ModelRegistry API (oxi-ai/src/model_registry.rs)
- `ModelRegistry::new()` — empty registry
- `ModelRegistry::from_static()` — pre-populated with all built-in models
- `register(&self, Model)` — add dynamic model
- `lookup(&self, provider: &str, model_id: &str) -> Option<Model>` — lookup with dynamic priority

### 3. model_db::get_all_models() returns `Iterator<Item=&ModelEntry>`
- `ModelEntry` is a static struct with `&'static str` fields — NOT `Model`
- No `From<ModelEntry> for Model` conversion exists
- **Decision**: Use `ModelRegistry::from_static()` instead, which already has all models loaded

### 4. Provider trait has no clone method
- `Provider: Send + Sync + 'static` with only `stream()` and `name()`
- Cannot clone `dyn Provider`
- **Decision**: `create_provider()` returns `Box<dyn Provider>` by creating fresh instances

### 5. ToolRegistry API (oxi-agent/src/tools.rs)
- `names() -> Vec<String>` ✓
- `get(&str) -> Option<Arc<dyn AgentTool>>` ✓
- `register_arc(Arc<dyn AgentTool>)` ✓

### 6. Agent::new() and Agent::tools()
- `Agent::new(Arc<dyn Provider>, AgentConfig)` ✓
- `Agent::tools() -> Arc<ToolRegistry>` ✓

## Changes Made

### oxi-sdk/src/builder.rs
- `Oxi` now holds `Arc<ModelRegistry>` + `Arc<ToolRegistry>` (was just `Arc<ToolRegistry>`)
- `resolve_model()` uses instance `self.models.lookup()` (was global `oxi_lookup_model()`)
- `create_provider()` delegates to `oxi_ai::get_provider()` (works for custom + built-in)
- `OxiBuilder.with_builtins()` uses `ModelRegistry::from_static()` to load all built-in models
- Added `OxiBuilder.model(Model)` for custom model registration
- `build()` wraps both registries in `Arc`

### oxi-sdk/src/agent_builder.rs
- `build()` resolves model via `self.oxi.resolve_model()` (instance registry)
- Creates provider via `self.oxi.create_provider()` 
- Converts `Box<dyn Provider>` → `Arc<dyn Provider>` for Agent::new()
- Merges `workspace_dir` and `system_prompt` into config
- Registers builder's tools into agent's tool registry via `register_arc()`

### oxi-sdk/src/lib.rs
- Added `ModelRegistry` to oxi-ai re-exports

## Verification

```
cargo check --workspace --lib — 0 errors, 0 warnings
```
