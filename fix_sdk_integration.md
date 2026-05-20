# Fix 5: Ensure oxi-cli still works + integration test

## Summary

All steps completed successfully. The oxi-cli binary, SDK, and entire workspace compile and pass tests after the other agents' changes.

## Step-by-step Results

### Step 1: Verify oxi-cli lib compiles
```
cargo check -p oxi-cli --lib → 0 errors
```

### Step 2: Verify oxi-cli binary compiles
```
cargo check -p oxi-cli → 0 errors
```

### Step 3: Updated oxi-sdk/src/prelude.rs
Expanded the prelude to re-export useful types:

```rust
pub use crate::builder::{Oxi, OxiBuilder};
pub use crate::agent_builder::AgentBuilder;
pub use crate::tool_factory::{coding_tools, readonly_tools};
pub use oxi_agent::{
    Agent, AgentConfig, AgentEvent, AgentLoop,
    ToolRegistry, AgentTool, AgentToolResult, ToolError,
    SharedState, AgentState, ToolExecutionMode,
};
pub use oxi_ai::{Provider, Model, CompactionStrategy};
```

### Step 4: Workspace-wide compilation check
```
cargo check --workspace --lib → 0 errors
```

### Step 5: Workspace tests
```
232 passed (oxi-ai)
350 passed (oxi-agent)
307 passed (oxi-core)
0   passed (oxi-sdk)
241 passed (oxi-cli)
79  passed (oxi-extensions)
─────────────────────
1,209 total — 0 failures
```

### Step 6: Release build + smoke test
```
oxi 0.12.0 — release build succeeded
codesign — signed
oxi --print -p zai -m glm-5.1 "1+1은?" → "1 + 1 = 2 입니다! 😊"
```

## Files Changed
- `oxi-sdk/src/prelude.rs` — expanded prelude re-exports to include Agent, AgentConfig, AgentLoop, SharedState, AgentState, ToolExecutionMode, Provider, Model, CompactionStrategy

## Notes
- All tool `new()` methods (ReadTool, WriteTool, EditTool, LsTool, etc.) retain their no-argument signatures, defaulting to `current_dir()` — no changes needed in oxi-cli
- `ToolRegistry::with_builtins_cwd(cwd, disabled)` is available and functional
- The SDK layer cleanly wraps the underlying crates with a fluent builder API
