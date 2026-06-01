# Progress

## oxios-kernel Issues

### #11 — Dead re-export cleanup ✅
- Removed 74 dead `pub use oxi_sdk::` re-exports from `lib.rs` (top-level block + sdk_exports module + CircuitBreaker alias)
- Removed dead `pub use oxi_sdk::{...}` block from `coordination.rs`
- Verified no external consumers via `rg`

### #7 — WasmSandbox feature-gate ✅
- Wrapped entire `wasm_sandbox.rs` module contents in `#[cfg(feature = "wasm-sandbox")]`
- Removed `#[cfg(not(...))]` stub types (no longer needed with module-level gate)
- `lib.rs` `pub mod` already had feature gate — confirmed

### #9 — Orchestrator session restore ✅
- Replaced empty `restore_sessions()` stub with full implementation
- Loads sessions from StateStore, filters by `active_seed_id`, reconstructs InterviewSession entries
- Restores conversation history for mid-orchestration session continuity on restart
