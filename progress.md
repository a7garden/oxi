# Progress

## Status
In Progress

## Tasks

### Port pi-mono's model-registry.ts to Rust
- [x] Read source TypeScript file completely
- [x] Read existing Rust codebase (oxi-ai model_registry, model_db, provider_registry, auth_storage)
- [x] Create `oxi-cli/src/model_registry.rs` with full ModelRegistry port
- [x] Add `pub mod model_registry;` to lib.rs
- [x] `cargo check -p oxi-cli --lib` passes with zero errors/warnings in new file
- [ ] Tests pass (blocked by pre-existing compilation errors in other modules)

## Files Changed
- `oxi-cli/src/model_registry.rs` — NEW: Full port of pi-mono model-registry.ts (945 lines TS → ~1000 lines Rust)
- `oxi-cli/src/lib.rs` — Added `pub mod model_registry;`

## Notes
- Pre-existing compilation errors in session.rs, export.rs, compaction_utils.rs prevent full `cargo test -p oxi-cli` from running. These are unrelated to this port.
- The new model_registry.rs is a CLI-layer registry (separate from oxi-ai's model_registry.rs) that:
  - Uses `oxi_ai::model_db` for built-in model definitions
  - Integrates with `auth_storage.rs` for credential lookup
  - Supports loading custom models from `models.json` (same schema as pi-mono)
  - Supports dynamic provider registration (for extensions)
  - Provides API key resolution through auth storage, env vars, and models.json config
  - Provides model filtering by provider, capability (reasoning), and modality (vision)
  - Includes 20 unit tests covering core functionality
