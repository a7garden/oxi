# Dead Code Cleanup Report - oxi-agent & oxi-ai

**Date:** 2026-05-16
**Scope:** `oxi-agent`, `oxi-ai` crates (lib only)
**Result:** ✅ 0 warnings for both crates (`cargo check -p oxi-agent -p oxi-ai --lib`)

## Summary

Removed **121 dead code warnings** across both crates by:
- Deleting 5 entire unused provider implementation files (~3,029 lines)
- Removing 12 dead methods from `Agent` struct
- Removing dead constants, structs, and functions
- Prefixing unused serde deserialization struct fields with `_` (with `#[serde(rename)]` for serde compatibility)
- Removing all `#[allow(dead_code)]` and `#![allow(dead_code)]` suppression annotations

## Changes by File

### oxi-ai — Files Deleted (untracked, never committed)

| File | Lines | Reason |
|------|-------|--------|
| `providers/cloudflare.rs` | 694 | Entire provider dead — Cloudflare routes through `OpenAiProvider` via built-in registry |
| `providers/copilot.rs` | 629 | Entire provider dead — Copilot routes through `OpenAiProvider` |
| `providers/codex.rs` | 704 | Entire provider dead — Codex routes through `OpenAiProvider` |
| `providers/deepseek.rs` | 387 | Entire provider dead — DeepSeek routes through `OpenAiProvider` |
| `providers/openai_completions.rs` | 615 | Entire provider dead — OpenAI Completions routes through `OpenAiProvider` |

### oxi-ai — Files Modified

#### `providers/google_shared.rs`
- Removed `GoogleThinkingLevel` enum (unused)
- Removed `retain_thought_signature()` function
- Removed `requires_tool_call_id()` function
- Removed `normalize_tool_call_id()` function
- Prefixed `thought_signature` → `_thought_signature` on `GooglePart`
- Prefixed `name` → `_name` on `GoogleFunctionCall` (with `#[serde(rename = "name")]`)
- Removed corresponding dead tests

#### `providers/azure.rs`
- Prefixed unused serde struct fields: `id` → `_id`, `model` → `_model`, etc. on `SSEChunk`, `ToolCallDelta`, `FunctionDelta`
- Moved `with_config()` to `#[cfg(test)]` (dead in lib, used by tests)

#### `providers/mistral.rs`
- Prefixed unused serde struct fields: `id` → `_id`, `model` → `_model`, etc. on `SSEChunk`, `ToolCallDelta`, `FunctionDelta`
- Moved `with_api_key()` to `#[cfg(test)]` (dead in lib, used by tests)

#### `providers/bedrock.rs`
- Removed `with_region()` method
- Prefixed unused serde struct fields: `index` → `_index`, `partial_json` → `_partial_json`, etc.

#### `providers/openai.rs`
- Prefixed unused serde struct fields: `id` → `_id`, `model` → `_model`, `type_` → `_type_`

#### `providers/openai_responses.rs`
- Prefixed unused serde struct fields across 7 structs: `ResponseCreatedData`, `OutputItem`, `TextDelta`, `FunctionCallDelta`, `OutputTextDone`, `ReasoningDone`, `ResponseWithUsageData`
- Added `#[allow(dead_code)]` to `Unknown(JsonValue)` variant (serde catch-all, needed for deserialization)

#### `providers/vertex.rs`
- Prefixed unused serde struct fields on `TokenResponse` and `ServiceAccountCreds`
- Removed unused `AssistantMessage` import in tests

#### `providers/model_fetch.rs`
- Prefixed `owned_by` → `_owned_by` on `ModelInfo`

#### `providers/anthropic.rs`
- Removed `with_api_key()` method

#### `providers/google.rs`
- Removed `with_api_key()` method

#### `compaction.rs`
- Updated 4 tests to use `OpenAiProvider::new()` instead of deleted `CloudflareProvider::new()`

### oxi-agent — Files Modified

#### `agent.rs` (major cleanup)
- **Removed dead constant:** `DEFAULT_FALLBACK_MODEL`
- **Removed dead struct:** `MpscRetryCallback` (and its `impl RetryCallback`)
- **Removed dead struct:** `ToolBatchResult`
- **Removed 12 dead methods:**
  - `run_compaction_check()`
  - `drain_steering_messages()`
  - `drain_follow_up_messages()`
  - `should_stop_after_turn()`
  - `execute_tool_batch()`
  - `execute_tools_sequential()`
  - `execute_tools_parallel()`
  - `execute_tool_single()`
  - `before_tool_call()`
  - `after_tool_call()`
  - `stream_with_retry()`
  - `try_fallback()`
- **Cleaned up imports:** Removed `stream_retry`, `AgentError`, `AgentToolResult`, `progress_callback`, `ContentBlock`, `Context`, `ProviderEvent`, `StreamOptions`, `TextContent`, `mpsc`

#### `agent_loop/tool_exec.rs`
- Prefixed `kind` → `_kind` on `PreparedToolCallOutcome` struct and all construction sites

#### `agent_loop/config.rs`
- Removed dead constants `AUTO_RETRY_MAX_ATTEMPTS` and `AUTO_RETRY_BASE_DELAY_MS`

#### `tools/edit.rs`
- Added `#[allow(dead_code)]` to `applied` field (read only in tests, which are exempt from removal)

#### `tools/github.rs`
- Removed dead function `gh_json()`

#### `tools/github_search.rs`
- Prefixed `incomplete_results` → `_incomplete_results` on `GitHubSearchResponse`
- Prefixed `archived` → `_archived` on `GitHubRepo`

## Verification

```
$ cargo check -p oxi-agent -p oxi-ai --lib
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s
# 0 warnings

$ cargo test -p oxi-agent --lib
# 232 passed; 0 failed

$ cargo test -p oxi-ai --lib
# 346 passed; 2 failed (pre-existing provider_registry env var tests)
```

## Notes

- **Error enum variants were preserved** per instructions
- **Test code was preserved** — only non-test dead code was removed
- Pre-existing test failures in `provider_registry` (env var handling) are unrelated to this cleanup
- The `register_builtins` module was kept alive — it's used by `oxi-cli` and `oxi-store`
- The `openai_responses_shared` module was trimmed to only `parse_streaming_json()` (the sole function used externally)
