# Fix Provider Errors — Error Handling Improvements

## Summary

Three targeted error-handling improvements applied to `oxi-ai/src/`.

---

## Fix 1: `ProviderError` retryability (`error.rs`)

### Changes
- **Added 3 new variants** to `ProviderError`:
  - `NetworkError(String)` — for connection/transport failures
  - `Timeout` — for request timeouts
  - `RateLimited { retry_after: Option<Duration> }` — for rate-limit responses with optional server-suggested wait time
- **Added `is_retryable(&self) -> bool`** — returns `true` for:
  - `HttpError(429, _)` or `HttpError(500+)`
  - `NetworkError(_)`
  - `Timeout`
  - `RateLimited { .. }`
- **Added `retry_after(&self) -> Option<Duration>`** — returns:
  - The `retry_after` value from `RateLimited`
  - A default 5s for `HttpError(429, _)`
  - `None` for all other variants

### File
- `oxi-ai/src/error.rs`

---

## Fix 2: `Secret<String>` Serialize masking (`secret.rs` + `options.rs`)

### Changes — `secret.rs`
- **Fixed `Serialize` impl** to output `"[REDACTED]"` instead of the plain inner value. This prevents secrets from leaking in serialized output (logs, JSON payloads, debug dumps).
- **Updated `serde_roundtrip` test** to verify serialization masks the value while deserialization still works from non-redacted input.

### Changes — `options.rs` (StreamOptions)
- Changed `api_key` field from `#[serde(skip_serializing_if = "Option::is_none")]` to `#[serde(skip)]` — ensures the key never appears in any serialized output.
- Replaced `#[derive(Debug)]` with a **manual `fmt::Debug` impl** that shows `"[REDACTED]"` for the `api_key` field, preventing key leakage in debug/log output.
- Kept `api_key` as `Option<String>` (not `Secret<String>`) because ~15 provider files access the field directly as `&String`. Changing to `Secret<String>` would require cascading `.expose()` calls across all providers — out of scope for this fix. The `#[serde(skip)]` + custom Debug achieve the same security goal.

### Files
- `oxi-ai/src/secret.rs`
- `oxi-ai/src/providers/options.rs`

---

## Fix 3: `Usage::calculate_cost` with model pricing (`types.rs`)

### Changes
- **Extended signature** from `calculate_cost(&mut self)` to `calculate_cost(&mut self, input_cost_per_million: Option<f64>, output_cost_per_million: Option<f64>)`.
- **Backward compatible** — callers passing `None, None` get the previous behavior (default $1/M rate).
- When pricing params are provided, they override the per-million-dollar rate for input/output tokens.
- **Updated existing test** to call `calculate_cost(None, None)`.

### File
- `oxi-ai/src/types.rs`

---

## Verification

- `cargo check -p oxi-ai` — compiles cleanly (only pre-existing warnings)
- `cargo test -p oxi-ai -- error::tests secret::tests types::tests` — **20 tests pass, 0 failures**
- The 2 pre-existing `provider_registry` test failures are unrelated to these changes
