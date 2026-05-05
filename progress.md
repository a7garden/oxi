# Progress

## Status
In Progress

## Tasks
- [x] Phase 1.2: oxi-ai provider_registry RwLock migration + model_registry split

## Files Changed
- `oxi-ai/src/provider_registry.rs` — Migrated std::sync::RwLock → parking_lot::RwLock, removed all `.unwrap()` on read/write guards
- `oxi-ai/src/model_registry.rs` — Added `extract_model_name()` helper, replaced 11 occurrences of `id.split('/').last().unwrap()`

## Notes
- parking_lot was already a dependency in oxi-ai/Cargo.toml (v0.12)
- `cargo check -p oxi-ai` passes clean
