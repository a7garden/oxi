# OAuth Security Fixes

## File Modified
`oxi-cli/src/oauth_server.rs`

## Issues Fixed

### 1. CSRF State Validation (Critical)
**Problem:** No CSRF protection — the OAuth callback accepted any state parameter, allowing CSRF attacks where an attacker could inject their own authorization code.

**Fix:**
- Added `csrf_state: Option<String>` field to `OAuthCallbackServer`
- Added `CsrfMismatch` variant to `OAuthError` enum
- Added `start_with_csrf()` method that accepts an expected state parameter
- In `run_server()`, the callback's returned state is validated against the expected CSRF state
- If mismatch, returns HTTP 403 and `OAuthError::CsrfMismatch`
- `authorize_with_browser()` generates a random UUID v4 state, includes it in the auth URL, and passes it to the server for validation

### 2. URL Decoding (Bug/Security)
**Problem:** Manual `%3D`/`%26` replacement only handled two of many possible percent-encoded characters, causing malformed authorization codes or state values.

**Fix:**
- Replaced `.replace("%3D", "=").replace("%26", "&")` with `urlencoding::decode()` which properly handles all percent-encoded characters
- The `urlencoding` crate was already in `Cargo.toml`

### 3. Redirect URI Port Mismatch (Critical)
**Problem:** `authorize_with_browser()` opened the browser with the auth URL *before* creating the callback server. The server port was random, so the `redirect_uri` in the auth URL (set externally) didn't match the actual server port. This caused the OAuth provider to redirect to the wrong port.

**Fix:**
- Reordered: create `OAuthCallbackServer` first, then get its port via `server.redirect_uri()`
- Inject the correct `redirect_uri` and `state` into the auth URL using `urlencoding::encode()`
- Then open the browser with the corrected URL

## API Changes
- `authorize_with_browser(auth_url: &str)` → `authorize_with_browser(auth_url_base: &str)` — now expects a base URL without `redirect_uri` or `state`; those are appended automatically
- New method: `OAuthCallbackServer::start_with_csrf(csrf_state: Option<String>)`
- `run_server()` now takes an additional `csrf_state: Option<String>` parameter

## Compilation
- No new compilation errors introduced (verified with `cargo check -p oxi-cli`)
- Pre-existing errors in `oxi-agent` are unrelated
