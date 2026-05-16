# Progress

## Status
In Progress

## Tasks

- [x] Phase 2 Step 1: Add oxi-sdk as dependency of oxi-cli
- [x] Phase 1: Complete AgentBuilder.build() and workspace_dir flow

## Files Changed

- `oxi-cli/Cargo.toml` — added `oxi-sdk = { version = "0.12.0", path = "../oxi-sdk" }` dependency
- `oxi-agent/src/config.rs` — added `workspace_dir: Option<PathBuf>` field to `AgentConfig` and `Default` impl
- `oxi-agent/src/agent.rs` — read `workspace_dir` before dropping read lock; pass it through to `AgentLoopConfig`
- `oxi-cli/src/lib.rs` — added `workspace_dir: None` to `AgentConfig` construction in `App::new()`
- `oxi-cli/src/app/agent_session_runtime.rs` — added `workspace_dir: None` to both `AgentConfig` constructions
- `oxi-sdk/src/agent_builder.rs` — rewrote `build()` to merge `workspace_dir`/`system_prompt` into config, register tools from builder's registry via `register_arc`

## Notes

- `cargo check --workspace --lib` passes with 0 errors
- All 79 workspace tests pass
- App::new() refactor deferred to Phase 3 as instructed
