# Phase 3: oxios integration — Remove WORKSPACE_MUTEX

## Summary

Removed the `WORKSPACE_MUTEX` from `oxios-kernel/src/agent_runtime.rs`. The mutex was a process-global lock that serialized all agent executions to work around the fact that `std::env::set_current_dir()` is process-global. Now that `AgentLoopConfig.workspace_dir` carries the per-agent workspace path, the mutex is no longer needed.

## Changes Made

### File: `/Volumes/MERCURY/PROJECTS/oxios/crates/oxios-kernel/src/agent_runtime.rs`

1. **Removed `WORKSPACE_MUTEX` static** — The `static WORKSPACE_MUTEX: std::sync::Mutex<()>` and its entire doc comment (explaining the CWD race and the TODO for upstream `workspace_dir` support) were deleted.

2. **Removed mutex lock in `run_agent_loop()`** — The line `let _workspace_guard = WORKSPACE_MUTEX.lock().expect("WORKSPACE_MUTEX poisoned");` was removed. Tool registration and agent loop execution no longer hold a process-wide lock.

3. **No `set_current_dir()` removal needed** — The `set_current_dir()` call was already absent from the current code.

4. **`workspace_dir` already wired** — `AgentLoopConfig.workspace_dir` was already set to `config.project_paths.first().cloned()` in the existing code.

## Build Verification

```
cargo check -p oxios-kernel --lib  → 0 errors, 3 warnings (unrelated dead_code)
```

## Impact

- **Before**: All agent executions were serialized via a process-global mutex, meaning only one agent could run at a time across the entire oxios process.
- **After**: Agents can execute concurrently. Each agent's file operations use `workspace_dir` from `AgentLoopConfig` instead of relying on the process CWD.
