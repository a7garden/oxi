# Code Deduplication Fixes — oxi project

## Summary

Four duplication issues were identified and fixed. All changes compile cleanly and pass existing tests.

---

## Fix 1: Unified CompactionReason

### Problem
Two separate `CompactionReason` enums:
- `oxi-cli/src/context/auto_compaction.rs` — `Manual`, `Automatic`, `Overflow`, `Iteration { current, every_n }`
- `oxi-cli/src/app/agent_session.rs` — `Manual`, `Threshold`, `Overflow`

### Solution
Unified the type in `auto_compaction.rs` as the canonical location, adding the `Threshold` variant:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    Manual,
    Threshold,    // was only in agent_session
    Automatic,
    Overflow,
    Iteration { current: usize, every_n: usize },
}
```

Removed the duplicate from `agent_session.rs`. Updated imports in:
- `oxi-cli/src/app/agent_session.rs` — now imports from `auto_compaction`
- `oxi-cli/src/app/mod.rs` — re-exports from `auto_compaction`
- `oxi-cli/src/tui/handlers.rs` — updated import + match arms for new variants
- `oxi-cli/src/tui/app.rs` — updated import

### Files Changed
- `oxi-cli/src/context/auto_compaction.rs` — added `Threshold` variant + Display match arm
- `oxi-cli/src/app/agent_session.rs` — removed duplicate enum, imported unified type
- `oxi-cli/src/app/mod.rs` — updated re-export
- `oxi-cli/src/tui/handlers.rs` — updated import + match arms
- `oxi-cli/src/tui/app.rs` — updated import

---

## Fix 2: Deduplicated normalize_tool_call_id

### Problem
Two implementations of tool call ID normalization:
- `oxi-ai/src/transform.rs:504` — public, sanitizes + truncates to 64 chars
- `oxi-ai/src/providers/google_shared.rs:94` — adds model_id guard before delegating

### Solution
Moved the core normalization logic to `oxi-ai/src/utils/mod.rs::normalize_tool_call_id(id: &str)`. Both `transform.rs` and `google_shared.rs` now delegate to this shared function:
- `transform.rs` — thin wrapper calling `crate::utils::normalize_tool_call_id(id)`
- `google_shared.rs` — guards with `requires_tool_call_id(model_id)`, then calls `crate::utils::normalize_tool_call_id(id)`

### Files Changed
- `oxi-ai/src/utils/mod.rs` — added `normalize_tool_call_id()` function
- `oxi-ai/src/transform.rs` — replaced body with delegation
- `oxi-ai/src/providers/google_shared.rs` — replaced inline logic with delegation

---

## Fix 3: Renamed tools::ValidationError → ToolValidationError

### Problem
Two `ValidationError` enums in the same crate:
- `oxi-ai/src/error.rs:51` — generic validation errors (InvalidJson, SchemaValidation, MissingRequiredField)
- `oxi-ai/src/tools.rs:144` — tool-specific validation errors (same variants)

### Solution
Renamed `tools.rs::ValidationError` to `ToolValidationError` to disambiguate. The `error.rs::ValidationError` remains the general-purpose one. Updated all internal references and the public re-export in `lib.rs`.

### Files Changed
- `oxi-ai/src/tools.rs` — renamed enum + all references
- `oxi-ai/src/lib.rs` — updated public re-export
- `oxi-ai/tests/error_handling.rs` — updated import alias

---

## Fix 4: Deduplicated truncate functions in oxi-tui

### Problem
Two identical `truncate_str`/`truncate_to_width` functions:
- `oxi-tui/src/widgets/chat.rs:62` — `truncate_str(s: &str, max_width: usize)`
- `oxi-tui/src/widgets/tool_renderer.rs:43` — `truncate_to_width(s: &str, max_width: usize)`

### Solution
Created `oxi-tui/src/text.rs` with the shared `truncate_to_width()` function. Exported from `lib.rs`. Both `chat.rs` and `tool_renderer.rs` now import from this shared module.

### Files Changed
- `oxi-tui/src/text.rs` — **new file**, contains `truncate_to_width()` with tests
- `oxi-tui/src/lib.rs` — added `pub mod text` + `pub use text::truncate_to_width`
- `oxi-tui/src/widgets/chat.rs` — removed local `truncate_str`, imports `truncate_to_width as truncate_str`
- `oxi-tui/src/widgets/tool_renderer.rs` — removed local `truncate_to_width`, imports from shared module

---

## Verification

- `cargo check --package oxi-ai` ✓
- `cargo check --package oxi-tui` ✓
- `cargo check --package oxi-cli` ✓
- `cargo test --package oxi-ai --test error_handling` ✓ (25 passed)
- `cargo test --package oxi-tui -- truncate` ✓ (5 passed)
- `cargo test --lib -p oxi-cli -- compaction_reason` ✓ (2 passed)
- Pre-existing failures in `provider_registry` tests are unrelated to these changes.
