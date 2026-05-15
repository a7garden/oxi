# Progress

## Model Registry Security Fixes (`oxi-store/src/model_registry.rs`)

- [x] **Fix 1: Command injection** — Removed `!` shell command execution from `resolve_config_value()` and `resolve_config_value_or_throw()`. Replaced with `$VAR`/`${VAR}` env var expansion. `!` prefix now logs warning and returns None/Err.
- [x] **Fix 2: apiKey warning** — Added `tracing::warn!` in `load_custom_models()` when providers have plaintext `apiKey` in models.json.
- [x] **Fix 3: Ambiguity logging** — Added `tracing::warn!` in `resolve_model()` when multiple providers match the same model ID.
- [x] **Tests updated** — Updated existing tests for new `$VAR` syntax, added test for `!` rejection and `${VAR}` syntax.
- [x] **Compiles clean** — `cargo check -p oxi-store` passes with no new warnings.
- [x] **Findings written** — `fix_model_registry_security.md`

### Notes
- Pre-existing test compilation error in `session_navigation.rs` (unrelated) prevents `cargo test` from building the test binary. Lib compilation is clean.
