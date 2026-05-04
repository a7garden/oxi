# Progress

## Status
Completed

## Tasks
- [x] Port `transform-messages.ts` (220 lines) — Message transformation between providers for model targeting
- [x] Port `register-builtins.ts` (403 lines) — Built-in provider registration with metadata
- [x] Port `openai-responses-shared.ts` (539 lines) — Shared Responses API code
- [x] Enhance `oxi-ai/src/transform.rs` with missing transformations
- [x] Create `oxi-ai/src/providers/openai_responses_shared.rs`
- [x] Create `oxi-ai/src/providers/register_builtins.rs`
- [x] Update `oxi-ai/src/providers/mod.rs` to register new modules
- [x] Update `oxi-ai/src/lib.rs` to re-export new APIs
- [x] Add `text_signature` field to `TextContent` struct
- [x] Fix `high_level.rs` struct literals for `TextContent` changes
- [x] `cargo check -p oxi-ai` passes (warnings only — unused new public APIs)
- [x] `cargo test -p oxi-ai` passes (335 tests, 0 failures)

## Files Changed
- `oxi-ai/src/transform.rs` — **ENHANCED**: Added ~280 lines of new functionality
  - `transform_messages_for_model()` — Main entry point matching `transformMessages` from TS
  - `normalize_tool_call_id()` — ID normalization for cross-provider compatibility
  - `downgrade_unsupported_images()` — Image → placeholder for non-vision models
  - `replace_images_with_placeholder()` — Consecutive placeholder deduplication
  - Synthetic tool result insertion for orphaned tool calls
  - Error/aborted assistant message filtering
  - Thinking block cross-model conversion (redacted → drop, signed → keep, empty → skip)
  - Tool call thought_signature stripping for cross-model
  - Added `Model`, `InputModality` to imports
  - Fixed all `TextContent` struct literals with `text_signature: None`

- `oxi-ai/src/providers/openai_responses_shared.rs` — **NEW**: ~530 lines
  - `encode_text_signature_v1()` / `parse_text_signature()` — Text signature handling
  - `short_hash()` — Deterministic short hash for ID generation
  - `convert_responses_messages()` — Messages → Responses API input format
  - `convert_responses_tools()` — Tools → Responses API tools format
  - `map_responses_stop_reason()` — Status → StopReason mapping
  - `sanitize_surrogates()` — Unicode sanitization
  - `parse_streaming_json()` — Best-effort partial JSON parsing
  - System prompt → developer/system role based on model capabilities
  - Tool call ID normalization for cross-provider compatibility
  - Full test suite (20 tests)

- `oxi-ai/src/providers/register_builtins.rs` — **NEW**: ~390 lines
  - `BuiltinProvider` struct with name, display_name, aliases, api, env_key, base_url
  - 18 built-in providers: openai, anthropic, google, vertex, mistral, azure, bedrock,
    deepseek, groq, cerebras, xai, openrouter, fireworks, cloudflare, copilot, codex,
    openai-responses, openai-completions
  - `get_builtin_provider()` — Lookup by name or alias
  - `get_provider_env_key()` / `get_provider_env_keys()` — API key env var lookup
  - `get_provider_api()` / `get_provider_base_url()` — Provider metadata
  - `resolve_provider_name()` — Alias → canonical name
  - `is_builtin_provider()` — Name validation
  - `get_all_provider_names()` / `get_all_provider_aliases()` — Listing
  - `get_api_mappings()` — API type to provider mappings
  - Full test suite (13 tests)

- `oxi-ai/src/providers/mod.rs` — Added `pub mod openai_responses_shared` and `pub mod register_builtins`
- `oxi-ai/src/messages.rs` — Added `text_signature: Option<String>` field to `TextContent`, added `with_signature()` constructor
- `oxi-ai/src/lib.rs` — Re-exported `normalize_tool_call_id` and `transform_messages_for_model`
- `oxi-ai/src/high_level.rs` — Fixed `TextContent` struct literals with `text_signature: None`
- `oxi-ai/src/providers/google.rs` — Restored to d30c4fd to fix pre-existing google_shared import
- `oxi-ai/src/providers/vertex.rs` — Restored to d30c4fd to fix pre-existing google_shared import

## Notes
- Pre-existing `google_shared` module references in google.rs and vertex.rs were broken in HEAD (commit 23318bf). Restored to working state from d30c4fd.
- All "unused" warnings are expected — these are newly created public APIs meant for consumers.
- The `openai_responses_shared.rs` module is designed to be used by both `openai_responses.rs` and `codex.rs` providers for shared message/tool conversion logic, matching the TS architecture.
