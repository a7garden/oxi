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

## Files Changed
- oxi-sdk/src/prelude.rs — expanded prelude re-exports

## Notes
- All tool `new()` methods still use no-argument signatures (default to current dir)
- `ToolRegistry::with_builtins_cwd()` exists and works
- oxi-sdk, oxi-cli, oxi-agent, oxi-ai all compile cleanly
- 1209 workspace tests pass across all crates
