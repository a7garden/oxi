# Refactor 3: oxi-cli workspace_dir flow + Agent::new() 3-arg migration

## Summary

This refactor ensured that all `Agent::new()` calls in oxi-cli use the new 3-argument signature (provider, config, tools) and that `workspace_dir` is properly set on `AgentConfig` so tools resolve the correct working directory.

## Findings

### Pre-existing state
Most of the 3-arg `Agent::new()` migration was already done in Refactor 1. All call sites in oxi-cli already passed `Arc::new(ToolRegistry::new())` as the third argument. The only remaining error was in `oxi-sdk/src/agent_builder.rs`, which was also already fixed (passes `Arc::new(self.tools)` directly).

### Changes made

#### 1. `oxi-cli/src/lib.rs` — workspace_dir in App::new()
- **Before:** `workspace_dir: None`
- **After:** `workspace_dir: Some(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))`
- **Why:** Tools that use `workspace_dir` (file operations, bash, etc.) now have the correct CWD from the moment the Agent is created, rather than relying on post-hoc registration.

#### 2. `oxi-cli/src/app/agent_session_runtime.rs` — workspace_dir in create_agent_session_from_services() (2 sites)
- **Fallback path** (no model configured): `workspace_dir: None` → `workspace_dir: Some(services.cwd.clone())`
- **Main path** (model resolved): `workspace_dir: None` → `workspace_dir: Some(services.cwd.clone())`
- **Why:** The `services.cwd` is the authoritative working directory for the session. Both code paths now propagate it to the Agent.

### What was NOT changed (conservative approach)
- Provider resolution in `App::new()` still uses `oxi_ai::get_model()` / `get_provider()` directly — not `OxiBuilder`. The task explicitly chose this conservative approach.
- Tool registration flow in `main.rs` is unchanged: Agent creates empty `ToolRegistry`, `main.rs` registers builtin tools after creation via `register_arc()`. This works because:
  - `register_builtin_tools()` creates tools with correct `cwd` via `ToolRegistry::with_builtins_cwd()`
  - Tools store their own `root_dir` from construction time
  - The Agent's shared `Arc<ToolRegistry>` receives them via `register_arc()`

### Verification
```
cargo check --workspace     → 0 errors
cargo check -p oxi-cli      → 0 errors (lib + bin)
cargo test --workspace --lib → 1209 passed, 0 failed (6 crates)
```

### Remaining `workspace_dir: None` sites (intentional)
- `oxi-agent/src/config.rs` — `Default` impl, correct
- `oxi-sdk/src/agent_builder.rs` — Builder initial state, overridden by `.workspace()` or config

## Architecture Notes

The tool registration flow for oxi-cli:
```
main.rs
  ├─ App::new(settings)
  │    └─ Agent::new(provider, config, Arc::new(ToolRegistry::new()))  // empty tools
  ├─ tools = app.agent_tools()  // Arc<ToolRegistry> from Agent
  ├─ register_builtin_tools(&tools, &cwd, ...)  // creates tools WITH cwd, registers into Agent's registry
  └─ load_wasm_extensions(...)  // registers WASM tools too
```

The `workspace_dir` on `AgentConfig` is primarily for SDK consumers who build their own tool registry before creating the Agent. For CLI mode, tools get their cwd from `ToolRegistry::with_builtins_cwd(cwd)`.
