# oxi-sdk Tests — Results

## Summary
Added 10 comprehensive unit tests to `oxi-sdk/src/lib.rs`. All pass.

## Test Results
```
running 10 tests
test tests::test_oxi_builder_new ... ok
test tests::test_oxi_builder_custom_model ... ok
test tests::test_tool_factory_readonly_tools ... ok
test tests::test_tool_factory_coding_tools ... ok
test tests::test_model_registry_isolation ... ok
test tests::test_oxi_builder_with_builtins ... ok
test tests::test_oxi_provider_resolution ... ok
test tests::test_agent_builder_workspace ... ok
test tests::test_agent_builder_coding_tools ... ok
test tests::test_agent_builder_readonly_tools ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## Workspace Check
```
cargo check --workspace --lib → 0 errors
```

## Tests Added

| # | Test Name | What It Validates |
|---|-----------|-------------------|
| 1 | `test_oxi_builder_new` | Empty builder has no models; `resolve_model` returns error |
| 2 | `test_oxi_builder_with_builtins` | `with_builtins()` populates known Anthropic and OpenAI models |
| 3 | `test_oxi_builder_custom_model` | Custom model registration via `.model()` and resolution via `provider/id` |
| 4 | `test_oxi_provider_resolution` | Built-in provider `anthropic` resolves; unknown provider errors |
| 5 | `test_agent_builder_workspace` | AgentBuilder with workspace doesn't panic |
| 6 | `test_agent_builder_coding_tools` | `.coding_tools()` registers read, write, edit, ls |
| 7 | `test_agent_builder_readonly_tools` | `.readonly_tools()` registers read, ls only (no write) |
| 8 | `test_model_registry_isolation` | Two Oxi instances don't share model state |
| 9 | `test_tool_factory_coding_tools` | `tool_factory::coding_tools()` returns exactly 4 tools |
| 10 | `test_tool_factory_readonly_tools` | `tool_factory::readonly_tools()` returns exactly 2 tools |

## File Changed
- `oxi-sdk/src/lib.rs` — Added `#[cfg(test)] mod tests` block at end of file

## Key Implementation Notes
- `Model` does **not** implement `Default` — tests use `Model::new()` with test defaults via a helper function
- `AgentConfig` implements `Default` — tests override only `model_id`, `max_iterations`, `timeout_seconds`
- Provider resolution falls back to built-in providers (not just custom-registered ones), so `create_provider("anthropic")` works without explicit registration
- Tool factories return `Arc<ToolRegistry>`; `AgentBuilder.coding_tools()`/`readonly_tools()` iterate and register each tool individually
