# Phase 1: Complete AgentBuilder.build() and workspace_dir flow

## Summary

Successfully wired `workspace_dir` from `AgentConfig` → `Agent` → `AgentLoopConfig` → file tools, and updated `AgentBuilder.build()` to properly merge config and register tools.

## Changes Made

### 1. `oxi-agent/src/config.rs`
- Added `pub workspace_dir: Option<std::path::PathBuf>` field to `AgentConfig` (after `api_key`)
- Added `workspace_dir: None` to `Default` impl

### 2. `oxi-agent/src/agent.rs`
- Read `workspace_dir` from inner config **before** dropping the read lock
- Changed `workspace_dir: None` → `workspace_dir` in `AgentLoopConfig` construction

### 3. `oxi-cli/src/lib.rs`
- Added `workspace_dir: None` to `AgentConfig` struct literal in `App::new()`

### 4. `oxi-cli/src/app/agent_session_runtime.rs`
- Added `workspace_dir: None` to both `AgentConfig` struct literals (empty-model placeholder and main config)

### 5. `oxi-sdk/src/agent_builder.rs`
- Rewrote `build()` to:
  - Merge `self.workspace_dir` into config (with `.or()` fallback)
  - Merge `self.system_prompt` into config if set
  - Create `Agent` with merged config
  - Register all tools from the builder's `ToolRegistry` into the Agent's registry via `register_arc()`

## Verification

- `cargo check --workspace --lib` → **0 errors**
- `cargo test --workspace --lib` → **79 passed, 0 failed**

## Key Design Decisions

- Used `register_arc()` on the Agent's `Arc<ToolRegistry>` to register builder tools after Agent creation (Option B from the task spec)
- `workspace_dir` flows through the `AgentConfig` struct, read at `run()` time, and passed to `AgentLoopConfig`
- The `workspace_dir` field is `Option<PathBuf>` — `None` means "use current directory" (matching existing behavior)
