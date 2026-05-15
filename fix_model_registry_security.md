# Model Registry Security Fixes

**File:** `oxi-store/src/model_registry.rs`
**Status:** All three fixes applied and compiles cleanly.

---

## Fix 1: Command Injection in `resolve_config_value()` ✅

**Problem:** Both `resolve_config_value()` and `resolve_config_value_or_throw()` executed arbitrary shell commands via `sh -c` when a config value started with `!`. This allowed command injection through `models.json` or any caller that passed user-controlled strings.

**Fix:**
- Removed all `std::process::Command` execution from both functions.
- Replaced with environment variable expansion: `$VAR` or `${VAR}` syntax.
- `!` prefix now logs a `tracing::warn!` and returns `None` / `Err`.
- Updated `get_provider_auth_status()` to recognize `$` prefix as `models_json_env_var` source and warn on deprecated `!` prefix.
- Updated tests: replaced `!echo $VAR` test with `$VAR` and `${VAR}` tests, added test for `!` rejection.

**Functions changed:**
- `resolve_config_value()` — lines ~260-285
- `resolve_config_value_or_throw()` — lines ~287-310
- `get_provider_auth_status()` — lines ~580-600

---

## Fix 2: Warn about apiKey in models.json ✅

**Problem:** Plaintext API keys in `models.json` are a security risk (file may be checked into version control, shared, or logged).

**Fix:** Added a loop in `load_custom_models()` (after parsing the config, before returning) that checks each provider's `api_key` field and logs a `tracing::warn!` recommending `$ENV_VAR` references.

**Location:** `load_custom_models()`, after `validate_config()` call.

---

## Fix 3: Ambiguity warning in `resolve_model()` ✅

**Problem:** When a model ID (without provider prefix) matched multiple providers, the function silently returned the first match with no indication of ambiguity.

**Fix:** Added a `tracing::warn!` when `matches.len() > 1` that lists all matching provider names before returning the first match.

**Location:** `resolve_model()`, the "Multiple matches" branch.

---

## Test Impact

- **Updated tests:** `test_resolve_config_value_env` (now tests `$VAR`), added `test_resolve_config_value_env_braces` (`${VAR}`), added `test_resolve_config_value_command_rejected` (`!` → `None`).
- **Removed test:** `test_resolve_config_value_missing_env` (replaced by `test_resolve_config_value_command_rejected`).
- **Pre-existing issue:** Test compilation for `oxi-store` has a pre-existing error in `session_navigation.rs` (unrelated trait impl mismatch). `cargo check -p oxi-store` passes clean.
