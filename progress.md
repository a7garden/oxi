# Progress

## Status
In Progress

## Tasks

### Phase 1.3: oxi-ai infallible unwraps — add expect() with reason ✅

Changed `.unwrap()` to `.expect("...")` with descriptive messages for all infallible static-string header parses in 5 provider files.

## Files Changed

- `oxi-ai/src/providers/openai.rs` — 2 changes (bearer + content-type headers)
- `oxi-ai/src/providers/mistral.rs` — 2 changes (bearer + content-type headers)
- `oxi-ai/src/providers/deepseek.rs` — 2 changes (bearer + content-type headers)
- `oxi-ai/src/providers/openai_completions.rs` — 2 changes (bearer + content-type headers)
- `oxi-ai/src/providers/openai_responses.rs` — 2 changes (bearer + content-type headers)
- `oxi-agent/src/error.rs` — thiserror migration of AgentError enum

## Notes

- `cargo check -p oxi-ai` passes clean after all changes.
- Total: 10 `.unwrap()` → `.expect()` replacements across 5 files.
- Did NOT touch files excluded by task (copilot, azure, codex, anthropic, bedrock, cloudflare).

### Phase 1.7: oxi-agent AgentError thiserror migration ✅

Migrated `AgentError` from manual `Display` impl to `#[derive(thiserror::Error)]` with `#[error("...")]` attributes.

- Added `#[derive(Debug, thiserror::Error)]` to `AgentError` enum
- Added `#[error("...")]` attributes to all 9 variants with named-field interpolation
- Removed manual `impl fmt::Display for AgentError` block
- Removed manual `impl std::error::Error for AgentError`
- Kept `is_retryable()` and `user_friendly()` methods unchanged
- Kept `From<anyhow::Error>` impl (wraps into `Stream` variant) unchanged
- `cargo check -p oxi-agent` passes (2 pre-existing warnings only)
- `cargo test -p oxi-agent` blocked by pre-existing `oxi-ai` compilation errors (unrelated)
