# Progress Tracker

## 2026-05-15: Error Handling Improvements (oxi-ai)

### Status: ✅ Complete

Three fixes applied to `oxi-ai/src/`:

1. **`error.rs`** — Added `NetworkError`, `Timeout`, `RateLimited` variants + `is_retryable()` + `retry_after()` methods to `ProviderError`
2. **`secret.rs`** — Fixed `Serialize` impl to mask value as `"[REDACTED]"` instead of exposing plain text
3. **`types.rs`** — Extended `Usage::calculate_cost()` to accept optional per-million pricing parameters (backward compatible)
4. **`options.rs`** — Protected `StreamOptions.api_key` from leaking via `#[serde(skip)]` + custom `Debug` impl showing `[REDACTED]`

All tests pass. Report: `fix_provider_errors.md`
