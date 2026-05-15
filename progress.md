# Progress: Streaming & Tool Exec Performance Fixes

## Status: ✅ COMPLETE

## Changes Made
- [x] streaming.rs: Clone-once for `messages.last()` in TextDelta, ThinkingDelta, ToolCallEnd, Done handlers
- [x] streaming.rs: Pre-sized tool definitions vector with `Vec::with_capacity`
- [x] streaming.rs: Reduced double-clone in Done handler
- [x] tool_exec.rs: Clone tool_call fields once upfront in sequential & parallel paths
- [x] tool_exec.rs: Single clone pattern for ToolResultMessage emit (MessageStart + MessageEnd)
- [x] tool_exec.rs: Replaced `"error".to_string()` / `"success".to_string()` with `String::from()`
- [x] helpers.rs: `should_stop_after_turn` takes `turn_number` param instead of counting messages
- [x] mod.rs: Updated call site to pass `turn_number as usize`

## Files Modified
- `oxi-agent/src/agent_loop/streaming.rs`
- `oxi-agent/src/agent_loop/tool_exec.rs`
- `oxi-agent/src/agent_loop/helpers.rs`
- `oxi-agent/src/agent_loop/mod.rs`

## Compilation
- All changed files compile cleanly (zero errors).
- Pre-existing errors in unrelated files (bash.rs, openai.rs, etc.) are unchanged.

---

# Progress: PathGuard Integration & input.rs Panic Fix

## Status: ✅ COMPLETE

## Changes Made

### Fix 1: input.rs text_mut() panic
- [x] Removed `text_mut()` method entirely — it called `unimplemented!()` which would panic at runtime
- [x] The method was never called anywhere in the codebase
- [x] Alternative APIs already exist: `set_text()`, `insert_str()`, `clear()`

### Fix 2: PathGuard applied to file tools
- [x] Added `validate_traversal()` method to PathGuard (traversal check + canonicalization, no workspace boundary)
- [x] Applied PathGuard to `read.rs` — replaced manual `..` component check with `PathGuard::validate_traversal()`
- [x] Applied PathGuard to `write.rs` — replaced manual `..` component check
- [x] Applied PathGuard to `edit.rs` — replaced manual `..` component check
- [x] Applied PathGuard to `ls.rs` — replaced manual `..` component check
- [x] Applied PathGuard to `find.rs` — replaced manual `..` component check
- [x] Applied PathGuard to `grep.rs` — replaced manual `..` component check

## Files Modified
- `oxi-tui/src/widgets/input.rs` — removed panicking `text_mut()` method
- `oxi-agent/src/tools/path_security.rs` — added `validate_traversal()` method
- `oxi-agent/src/tools/read.rs` — PathGuard integration
- `oxi-agent/src/tools/write.rs` — PathGuard integration
- `oxi-agent/src/tools/edit.rs` — PathGuard integration
- `oxi-agent/src/tools/ls.rs` — PathGuard integration
- `oxi-agent/src/tools/find.rs` — PathGuard integration
- `oxi-agent/src/tools/grep.rs` — PathGuard integration

## Testing
- All existing tests pass across all modified modules
- Path traversal tests continue to correctly reject `..` paths
- No workspace boundary regressions (uses `validate_traversal` not `validate`)

## Compilation
- All packages compile cleanly (zero errors)
