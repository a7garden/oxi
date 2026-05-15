# Progress Tracker

## 2026-05-15: RPC Bash Injection + CLI Thinking + OAuth + Prompt Dedup + AuthStorage Singleton

### Status: ✅ Complete

Five fixes applied across `oxi-cli/src/` and `oxi-store/src/`:

1. **RPC Bash injection** — Added `is_dangerous_rpc_command()` with warning log for dangerous patterns in `handlers.rs`
2. **CLI --thinking error** — Fixed wrong valid values in error messages (`none, minimal, standard, thorough` → `off, minimal, low, medium, high, xhigh`)
3. **OAuth URL decoding** — Already fixed (verified `urlencoding::decode()` in use)
4. **System prompt dedup** — Added TODO comments to both `build_system_prompt` instances
5. **AuthStorage singleton** — Added `shared_auth_storage()` returning `Arc<AuthStorage>`, updated 17 call sites

Report: `fix_rpc_misc.md`

---

## 2026-05-15: Error Handling Improvements (oxi-ai)

### Status: ✅ Complete

Three fixes applied to `oxi-ai/src/`:

1. **`error.rs`** — Added `NetworkError`, `Timeout`, `RateLimited` variants + `is_retryable()` + `retry_after()` methods to `ProviderError`
2. **`secret.rs`** — Fixed `Serialize` impl to mask value as `"[REDACTED]"` instead of exposing plain text
3. **`types.rs`** — Extended `Usage::calculate_cost()` to accept optional per-million pricing parameters (backward compatible)
4. **`options.rs`** — Protected `StreamOptions.api_key` from leaking via `#[serde(skip)]` + custom `Debug` impl showing `[REDACTED]`

All tests pass. Report: `fix_provider_errors.md`
