# oxi Progress

## 2026-05-04: Port google-shared.ts → google_shared.rs

### Completed
- **Created `oxi-ai/src/providers/google_shared.rs`** (782 lines)
  - `GoogleThinkingLevel` enum (THINKING_LEVEL_UNSPECIFIED, MINIMAL, LOW, MEDIUM, HIGH)
  - `is_thinking_part()` — detect thinking content in streaming parts via `thought: true`
  - `retain_thought_signature()` — preserve thought signatures across streaming deltas
  - `map_stop_reason()` — map all Google FinishReason strings to oxi StopReason
  - `convert_messages()` — transform Context messages to Google Content[] format
  - `convert_tools()` — transform Tool[] to Google functionDeclarations with optional OpenAPI mode
  - `blocks_to_google_parts()` — convert ContentBlock[] to Google parts JSON
  - `build_request_body()` — shared request body builder
  - `parse_google_events()` — unified SSE event parser for both providers
  - `create_error_message()` — shared error message factory
  - Shared response structs: GoogleResponse, GoogleCandidate, GoogleContent, GooglePart, GoogleFunctionCall, GoogleUsageMetadata
  - `requires_tool_call_id()` and `normalize_tool_call_id()` helpers
  - `sanitize_for_openapi()` — strip JSON Schema meta-declarations
  - 20 comprehensive unit tests

- **Updated `google.rs`** (365 → 190 lines)
  - Removed duplicate message conversion, tool conversion, SSE parsing, and response structs
  - Now imports all shared logic from `google_shared` module
  - Retains provider-specific: GoogleProvider struct, API key handling, stream() implementation
  - 5 provider-specific tests

- **Updated `vertex.rs`** (715 → 314 lines)
  - Removed duplicate message conversion, tool conversion, SSE parsing, and response structs
  - Now imports all shared logic from `google_shared` module
  - Retains provider-specific: VertexProvider struct, OAuth/JWT auth, gcloud token handling
  - 5 provider-specific tests

- **Updated `mod.rs`** — added `mod google_shared;` declaration

### Metrics
- Net code reduction: ~576 lines of duplicate code eliminated
- `cargo check -p oxi-ai` passes ✓
- `cargo test -p oxi-ai --lib` passes: 351 tests ✓ (was 305 before, +46 from shared module tests)

### Files Changed
- `oxi-ai/src/providers/google_shared.rs` — NEW: shared module (782 lines)
- `oxi-ai/src/providers/google.rs` — refactored to use shared module (190 lines)
- `oxi-ai/src/providers/vertex.rs` — refactored to use shared module (314 lines)
- `oxi-ai/src/providers/mod.rs` — added `mod google_shared` declaration
