# HTTP Client Singleton Consolidation

## Summary

Consolidated scattered `reqwest::Client::new()` and `reqwest::Client::builder().build()` calls into shared singleton patterns across the oxi project, following the existing `oxi-ai::providers::shared_client()` pattern.

## Changes Made

### Fix 1: oxi-agent — New `http_client` module

**New file:** `oxi-agent/src/tools/http_client.rs`
- Provides `shared_http_client()` → `&'static reqwest::Client`
- Configured with: `pool_max_idle_per_host(4)`, `pool_idle_timeout(30s)`, `timeout(30s)`

**Updated files:**

| File | Before | After |
|------|--------|-------|
| `oxi-agent/src/tools.rs` | — | Added `pub mod http_client;` |
| `oxi-agent/src/tools/context7.rs` | Local `OnceLock<Client>` + `client()` fn | Uses `shared_http_client()` |
| `oxi-agent/src/tools/github_search.rs` | `reqwest::Client::new()` | `shared_http_client()` |
| `oxi-agent/src/proxy.rs` | `reqwest::Client::builder().timeout(120s).build()?` | Cached in local `OnceLock` (keeps 120s timeout for streaming) |

### Fix 2: oxi-cli — New `util::http_client` module

**New file:** `oxi-cli/src/util/http_client.rs`
- Provides `shared_http_client()` → `&'static reqwest::Client` (30s timeout)
- Provides `shared_http_client_with_timeout(duration)` → `&'static reqwest::Client` (custom timeout)

**Updated files:**

| File | Before | After |
|------|--------|-------|
| `oxi-cli/src/util/mod.rs` | — | Added `pub(crate) mod http_client;` |
| `oxi-cli/src/infra/tools_manager.rs` | Two `Client::builder()` calls | Shared client for API calls; cached OnceLock for 120s downloads |
| `oxi-cli/src/infra/version_check.rs` | `Client::builder().timeout(10s)` | `shared_http_client()` |
| `oxi-cli/src/storage/packages.rs` | Two `Client::builder().timeout(10s)` calls | `shared_http_client()` |
| `oxi-cli/src/extensions/ext_cli.rs` | Two `Client::new()` calls | `shared_http_client()` |

### Notes

- **proxy.rs**: Kept a separate OnceLock with 120s timeout (streaming proxy needs extended timeout)
- **tools_manager.rs downloads**: Kept a separate OnceLock with 120s timeout (large file downloads)
- User-Agent headers moved from client builder to per-request `.header()` calls where needed
- Removed now-unused constants: `NETWORK_TIMEOUT_SECS` in both tools_manager.rs and packages.rs, `Duration` import in version_check.rs and packages.rs

## Build Status

✅ `cargo check --workspace` passes cleanly (no new warnings or errors)
