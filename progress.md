# Progress

## Status
In Progress

## Tasks
- [x] Refactor 1: Agent accepts pre-built ToolRegistry

## Files Changed
- `oxi-agent/src/agent.rs` — Changed `Agent::new()` to accept `Arc<ToolRegistry>`, added `new_empty()` convenience constructor
- `oxi-agent/src/tests.rs` — Updated all 8 call sites to pass `Arc::new(ToolRegistry::new())`
- `oxi-cli/src/lib.rs` — Updated 1 call site (App::new)
- `oxi-cli/src/app/agent_session.rs` — Updated 1 call site (test helper)
- `oxi-cli/src/app/agent_session_runtime.rs` — Updated 2 call sites (fallback path and main path)
- `oxi-sdk/src/agent_builder.rs` — KEY CHANGE: passes builder's `self.tools` via `Arc::new(self.tools)` directly to `Agent::new()`, eliminating the post-creation tool registration loop

## Notes
- `cargo check --workspace --lib`: 0 errors
- `cargo test --workspace --lib`: 1209 tests passed, 0 failed
