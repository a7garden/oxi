# Refactor 1: Agent accepts pre-built ToolRegistry

## Summary

Successfully refactored `Agent::new()` to accept an `Arc<ToolRegistry>` parameter, enabling callers to control tool creation with correct cwd.

## Changes Made

### `oxi-agent/src/agent.rs`

1. **Changed `Agent::new()` signature** from:
   ```rust
   pub fn new(provider: Arc<dyn Provider>, config: AgentConfig) -> Self
   ```
   to:
   ```rust
   pub fn new(provider: Arc<dyn Provider>, config: AgentConfig, tools: Arc<ToolRegistry>) -> Self
   ```

2. **Updated constructor body** from:
   ```rust
   tools: Arc::new(ToolRegistry::new()),
   ```
   to:
   ```rust
   tools,
   ```

3. **Added convenience constructor**:
   ```rust
   /// Create an agent with an empty tool registry.
   pub fn new_empty(provider: Arc<dyn Provider>, config: AgentConfig) -> Self {
       Self::new(provider, config, Arc::new(ToolRegistry::new()))
   }
   ```

### `oxi-agent/src/tests.rs` (8 call sites updated)

All test files that call `Agent::new()` now pass `Arc::new(ToolRegistry::new())` as the third argument:
- Line ~179: `test_agent_with_mock_provider`
- Line ~198: `test_agent_events_sequence`
- Line ~337: `test_agent_model_id`
- Line ~347: `test_agent_switch_model_invalid_format`
- Line ~360: `test_agent_switch_model_unknown_model`
- Line ~372: `test_agent_switch_model_same_provider`
- Line ~446: `test_cross_provider_message_transformation`
- Line ~491: `test_cross_provider_message_transformation_roundtrip`

### `oxi-cli/src/lib.rs` (1 call site)

Line ~347: `App::new()` updated to pass `Arc::new(oxi_agent::ToolRegistry::new())`.

### `oxi-cli/src/app/agent_session.rs` (1 call site)

Line ~1418: `make_session()` test helper updated.

### `oxi-cli/src/app/agent_session_runtime.rs` (2 call sites)

- Line ~283: Fallback path for session creation (uses placeholder provider)
- Line ~323: Main agent creation path

### `oxi-sdk/src/agent_builder.rs` — **KEY CHANGE**

Updated `build()` to pass the builder's collected tools directly to `Agent::new()`:
```rust
// OLD: Create empty agent, then register tools post-creation
let agent = Agent::new(provider, config);
let agent_tools = agent.tools();
for name in self.tools.names() {
    if let Some(tool) = self.tools.get(&name) {
        agent_tools.register_arc(tool);
    }
}

// NEW: Pass tools directly
let agent = Agent::new(provider, config, Arc::new(self.tools));
```

This is the KEY change — `AgentBuilder` collects tools during `.tool()` calls, then passes them all at once to `Agent`.

## Verification

```
cargo check --workspace --lib 2>&1 | grep '^error' | wc -l
=> 0 errors

cargo test --workspace --lib 2>&1 | grep 'test result:'
=> test result: ok. 232 passed; 0 failed (oxi-agent)
=> test result: ok. 350 passed; 0 failed (oxi-cli)
=> test result: ok. 307 passed; 0 failed (oxi-sdk)
=> test result: ok. 0 passed; 0 failed (oxi-store)
=> test result: ok. 241 passed; 0 failed (oxi-ai)
=> test result: ok. 79 passed; 0 failed (oxi-ui)
Total: 1209 tests passed, 0 failed
```

## Impact

- Callers now fully control `ToolRegistry` creation, enabling correct cwd setup before agent creation
- Cleaner architecture: tools are built externally and passed in, rather than added after construction
- Maintains backward compatibility through `new_empty()` convenience constructor