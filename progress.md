# Progress

## Status
In Progress

## Tasks
- [x] Fix ModelRegistry naming confusion (rename oxi-store::ModelRegistry to CliModelRegistry)

## Files Changed
- `oxi-store/src/model_registry.rs` — Renamed `struct ModelRegistry` → `struct CliModelRegistry`, added `type ModelRegistry = CliModelRegistry;` alias, added doc comments
- `oxi-store/src/lib.rs` — Exports both `CliModelRegistry` and `ModelRegistry` (alias)
- `oxi-ai/src/model_registry.rs` — Added doc comment clarifying SDK/engine role

## Notes
- All workspace checks pass (0 errors)
- All 1209 tests pass across 6 test suites
- Backward compatible: `use oxi_store::ModelRegistry` still works via type alias
- `oxi-cli` needed no changes (uses the alias)
