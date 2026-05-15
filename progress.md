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

## HTTP Client Singleton Consolidation

- [x] **Fix 1: oxi-agent** — Created `oxi-agent/src/tools/http_client.rs` with `shared_http_client()`. Updated context7.rs, github_search.rs, and proxy.rs to use shared/cached clients.
- [x] **Fix 2: oxi-cli** — Created `oxi-cli/src/util/http_client.rs` with `shared_http_client()`. Updated tools_manager.rs, packages.rs, ext_cli.rs, and version_check.rs.
- [x] **Custom timeout cases** — proxy.rs (120s streaming) and tools_manager downloads (120s) use local OnceLock caches with their specific timeouts.
- [x] **Cleanup** — Removed unused constants and imports (NETWORK_TIMEOUT_SECS, Duration).
- [x] **Builds clean** — `cargo check --workspace` passes.
- [x] **Findings written** — `fix_http_client.md`
