# Progress

## Status
In Progress

## Tasks
- [x] Fix ModelRegistry naming confusion (rename oxi-store::ModelRegistry to CliModelRegistry)

## Files Changed
- `oxi-store/src/model_registry.rs` — Renamed `struct ModelRegistry` → `struct CliModelRegistry`, added `type ModelRegistry = CliModelRegistry;` alias, added doc comments
- `oxi-store/src/lib.rs` — Exports both `CliModelRegistry` and `ModelRegistry` (alias)
- `oxi-ai/src/model_registry.rs` — Added doc comment clarifying SDK/engine role

## Tasks (completed)
- [x] Add comprehensive tests to oxi-sdk (10 tests)

## Files Changed
- `oxi-sdk/src/lib.rs` — Added `#[cfg(test)] mod tests` with 10 tests

## Notes
- All workspace checks pass (0 errors)
- All 10 oxi-sdk tests pass
- Backward compatible: `use oxi_store::ModelRegistry` still works via type alias
- `oxi-cli` needed no changes (uses the alias)

## oxi-sdk Tests Added
1. `test_oxi_builder_new` — Empty builder has no models
2. `test_oxi_builder_with_builtins` — with_builtins populates known models
3. `test_oxi_builder_custom_model` — Custom model registration and resolution
4. `test_oxi_provider_resolution` — Built-in provider fallback + unknown provider error
5. `test_agent_builder_workspace` — AgentBuilder with workspace doesn't panic
6. `test_agent_builder_coding_tools` — coding_tools registers read/write/edit/ls
7. `test_agent_builder_readonly_tools` — readonly_tools registers read/ls only
8. `test_model_registry_isolation` — Two Oxi instances don't share state
9. `test_tool_factory_coding_tools` — Tool factory creates exactly 4 coding tools
10. `test_tool_factory_readonly_tools` — Tool factory creates exactly 2 readonly tools
