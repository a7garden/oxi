# TUI Inline Provider Setup (API Key + OAuth)

**Date:** 2026-08-10
**Status:** Approved (design complete, awaiting plan)
**Scope:** `oxicode-cli/src/tui_vt/` (slash commands, overlays, secure prompt,
OAuth callback listener), `oxicode-ai/data/catalog/product-meta.toml` (OAuth
metadata), `oxicode-cli/src/store/auth_storage.rs` (no API change), new modules
`oxicode-cli/src/provider_oauth.rs`, `oxicode-cli/src/oauth_refresh.rs`.

## Problem

`/providers` (added in 92d693b3, 2026-08-07) lists every known provider with
credential status and supports **removing** an API key in-TUI. It cannot
**add** one: a provider row without a key falls through to the message
"`No key for '<name>'. Run `oxicode setup` to add one.`" (`main_loop.rs:1470-1485`).
The fix has been deferred twice with the same reason quoted verbatim:

> Key entry still routes through oxicode setup because the list overlay has
> no free-text input for secrets; removal is fully in-TUI.

`oxicode setup` is a separate 2029-line `ratatui` widget (`setup_wizard.rs`)
with its own input loop, state machine, and event handling. Running it inside
the TUI would mean a nested render + double input loop — a structural
collision, not a small one.

Worse, OAuth authentication for providers that *only* support OAuth at the
provider level (e.g., ChatGPT subscription via OpenAI, Claude Pro/Max via
Anthropic) has no path at all. The only OAuth code in the repo is the
**MCP** `client_credentials` provider (`mcp_credentials.rs`), which is
irrelevant for LLM provider login.

Users have to drop out of the TUI to set up providers — the only auth flow
in a code agent that breaks the in-session model.

## Design

### 1. Revive the existing `SecurePromptConfig` overlay channel

**Today.** `OverlayRequest::Modal { title, lines, secure_prompt: Option<SecurePromptConfig> }`
is fully defined in `oxicode-vtui-compat/src/ui_protocol/types.rs:41` and the
`InlineHandle::show_modal(title, lines, secure_prompt)` API exists at
`oxicode-vtui/src/tui/core_tui/types/protocol.rs:436-447`. **But the host's
`OverlayState` (`main_loop.rs:331`) drops `secure_prompt` on the floor.**
`materialize_overlay` (line 995-1003) constructs `OverlayState` without it.
Zero callers pass `Some(_)`. The protocol path is dead.

**Change.**
- Add `secure_input: Option<OverlaySecureInput>` to `OverlayState`.
- Define `OverlaySecureInput { config: SecurePromptConfig, value: String, cursor: usize }`.
- `materialize_overlay`'s `Modal` arm projects `req.secure_prompt` into
  `OverlaySecureInput` (empty value, cursor 0).
- New `OverlaySubmission::SecureInput(String)` variant.
- Input thread: when `overlay.secure_input.is_some()`, route printable keys
  to `value` accumulation (backspace pops, normal text appends at cursor),
  Enter submits as `SecureInput(value)`, Esc submits as `Cancelled`.
  Ignore non-text keys (`←`/`→` move cursor; copy/paste is `Cmd-V` → text
  insertion at cursor — best-effort).
- `render_overlay` draws a single-line input box below the modal text:
  `<label>: [value or * for mask_input]` with `▌` cursor indicator.
  `mask_input=true` displays `*` per character, never the value itself.
  Placeholder shown when `value.is_empty()`.

**Why not `WizardOverlayRequest`.** `WizardOverlayRequest` is defined but the
host does not render it natively (`main_loop.rs:1018` comment:
"Wizard overlays are multi-step flows that this TUI does not yet render
natively"). Building that whole machinery for two providers is YAGNI; the
existing `Modal` channel is sufficient.

### 2. Action matrix for `/providers` rows

Today's row selection at `main_loop.rs:1458-1485` has one branch: has key
or no key. We extend it to:

|              | has_key | no_key |
|--------------|---------|--------|
| key-only     | confirm remove | secure prompt → `set_api_key` |
| oauth-capable | menu: Update key / Re-login OAuth / Remove | menu: Set API key / Login with OAuth |

**Implementation.**
- New `ConfirmationAction::AuthProviderAction { provider, action: AuthAction }`
  variant.
- `AuthAction` enum: `SetApiKey`, `StartOAuth { spec: ProviderOAuthSpec }`,
  `RemoveKey`.
- For `SetApiKey`: push a `Modal` overlay with `secure_prompt`. On
  `SecureInput(text)` submission → `auth_storage.set_api_key(provider, text)`,
  append info message, refresh overlay if any.
- For `StartOAuth`: see §3.
- `RemoveKey`: today's behavior — confirm modal → `auth.remove(provider)`.
- For **menu** presentation (oauth-capable rows), reuse `show_list_modal`
  with two or three `InlineListItem`s whose `selection` is
  `InlineListSelection::ProviderAction(AuthAction)`. The host already
  handles list submissions; we add the variant.

**Detecting oauth-capable.** The catalog now exposes per-provider OAuth
metadata (§4). When `provider_oauth::spec_for(name)` returns `Some(_)`, the
row is oauth-capable. Otherwise it is key-only. Custom providers
(`Settings.custom_providers`) without OAuth metadata are key-only.

### 3. OAuth `authorization_code` flow with ephemeral localhost listener

**Provider metadata.** Extend `data/catalog/product-meta.toml` with one
section per OAuth-capable provider:

```toml
[providers.openai.oauth]
client_id     = "app-xxx"           # public client ID
auth_url      = "https://auth.openai.com/..."
token_url     = "https://auth.openai.com/token"
scopes        = ["openid", "offline_access"]
redirect_path = "/callback"        # path component; host:port is dynamic
use_pkce      = true

[providers.anthropic.oauth]
# same shape; claude.ai console
```

`client_secret` is **not** stored — public PKCE clients. `use_pkce = true`
is the default; future device_code providers can set `use_pkce = false`.

**New module `oxicode-cli/src/provider_oauth.rs`.**
- `ProviderOAuthSpec` (deserialized from the TOML block).
- `build_auth_url(spec, state: &str, code_challenge: &str) -> String` —
  emits exactly the query params the provider expects, with proper
  URL-encoding.
- `exchange_code(spec, code: &str, verifier: &str) -> anyhow::Result<OAuthTokens>`
  — POST to `token_url` with `application/x-www-form-urlencoded`,
  parse JSON `{ access_token, refresh_token?, expires_in?, scope? }`,
  compute absolute `expires_at`.
- `pkce_pair() -> (verifier: String, challenge: String)` — S256 method,
  per RFC 7634. Cryptographically strong random verifier (32 bytes, base64url).
- `spec_for(provider: &str) -> Option<ProviderOAuthSpec>` — loads
  `product-meta.toml` once at startup, caches in `OnceLock`.
- `open_browser(url: &str) -> Result<()>` — wraps `open::that` from the
  `open` crate. Returns `Err` on headless / no-display environments so the
  TUI can degrade gracefully.

**Listener.** New module `oxicode-cli/src/oauth_listener.rs`:
- `pub async fn await_callback(listener: TcpListener, expected_state: String, timeout: Duration) -> anyhow::Result<CallbackReceived>`
  where `CallbackReceived { code: String, state: String }`.
- Binds to `127.0.0.1:0` (kernel-assigned port).
- Single-shot: accepts one TCP connection, reads until CRLFCRLF (end of
  HTTP headers), parses `GET <path>?<query> HTTP/1.x`, validates
  `path == expected redirect_path`, validates `state == expected_state`,
  validates `code` is present and non-empty.
- Writes `200 OK` HTML response ("Login complete — return to oxicode.") then
  closes. Ignores anything beyond the headers (HTTP keep-alive is not
  honored; browser closes after the response).
- Errors → distinct variants: `Timeout`, `BadRequest(String)`, `StateMismatch`,
  `MissingCode(String)`.

**Use `std::net::TcpListener` synchronously wrapped in `spawn_blocking`**, or
`tokio::net::TcpListener` if it composes cleanly with the rest of the
runtime — both are acceptable. The handler is ~50 LOC; no `hyper`.

**TUI flow.**
1. User selects "Login with OAuth" on a `/providers` row.
2. TUI generates `state = random_base64url(16)` and `pkce_pair()`.
3. TUI binds listener on `127.0.0.1:0`, gets assigned port, builds
   `redirect_uri = "http://127.0.0.1:{port}/{redirect_path}"`,
   builds auth URL.
4. TUI calls `open_browser(&auth_url)`. On success: appends an info line
   "Opening browser for openai login (Ctrl-C to cancel)…" and spawns the
   listener task with a 120 s timeout (configurable later).
5. Listener resolves with `(code, state)`. TUI calls `exchange_code` →
   `OAuthTokens` → `auth_storage.set_oauth_full(provider, ...)` →
   appends "Logged in to openai (expires in N min)" info line. If the
   provider returns a `refresh_token`, mark `refreshable = true` (used by
   §5).
6. On error: append error line, leave provider keyless, no half-saved state.
7. Listener is dropped; the bound port is released.

**Headless fallback.** If `open_browser` returns `Err`, the TUI appends
"Could not open a browser. Open this URL manually within 5 minutes:" and the
URL, then waits up to 5 minutes on the listener. Same success path after.

### 4. Catalog OAuth metadata format

`data/catalog/product-meta.toml` is the existing place for oxicode-specific
provider metadata (extra HTTP headers, etc.). Add an optional
`[providers.<name>.oauth]` table per OAuth-capable provider. Initial entries:
`openai`, `anthropic`. Empty table means key-only.

Parsing is owned by `oxicode-cli/src/provider_oauth.rs`. The catalog code
in `oxicode-ai` is unaware of OAuth.

### 5. OAuth refresh + use

**Storage.** Existing `AuthCredential::OAuth { access_token, refresh_token,
expires_at, scopes }` is reused. `auth_storage.set_oauth_full` (line 771)
already takes every field. `update_oauth_tokens` (line 1749 in tests) handles
in-place refresh.

**Refresh path.** New module `oxicode-cli/src/oauth_refresh.rs`:
- `pub async fn refresh_if_expired(provider: &str) -> Result<(), OAuthError>`
- Looks up the stored `AuthCredential::OAuth`. If not expired, no-op.
- If expired and `refresh_token` is present: POST to the provider's
  `token_url` with `grant_type=refresh_token&refresh_token=...`, parse the
  new `access_token` (+ optional `refresh_token` rotation), call
  `auth_storage.update_oauth_tokens`. Same module as `exchange_code`.
- If no `refresh_token`: return `OAuthError::ReLoginRequired(provider)`.
  TUI surfaces this as "openai session expired — run `/providers` and
  re-login."

**Wiring.** `oxicode_sdk::ports::AuthProvider` (existing) returns
`get_api_key(provider)` which already prefers `OAuth { access_token }` over
`ApiKey` (`auth_storage.rs:664`). We hook the refresh check into the
existing `App::from_oxicode` (or the bootstrap that constructs the
`AuthProvider` impl) so it runs at most once per token lifetime. Token
refresh is **not** triggered per request — only when a request fails with
401 or when the bootstrap sees the token is within 60 s of expiry.

**Concurrency.** A single in-flight refresh per provider is enough. Use
`tokio::sync::Mutex<HashMap<String, Arc<OnceCell<()>>>>` per provider name
to coalesce concurrent calls. (Simpler than a full per-provider semaphore
and adequate for one TUI process.)

### 6. Slash command surface

| Command | Change |
|---------|--------|
| `/providers` | Row selection now branches on has_key × oauth-capable. New action menu for OAuth-capable rows. |
| `/status` | Show OAuth vs API-key badge per active provider; refresh-token presence. |
| `/info` | Add OAuth login timestamps if available. |
| `/shortcuts` | No change. |
| `/providers remove <name> [--yes]` | Unchanged. |

## Components and data flow

```
/providers
   │
   ▼
ProviderRow selection (main_loop.rs:1458)
   │
   ├── has_key & oauth-capable ──▶ action list modal
   │                                ├─ SetApiKey ──▶ SecurePrompt modal
   │                                │                  └▶ auth.set_api_key
   │                                ├─ StartOAuth ──▶ provider_oauth
   │                                │                  ├▶ open_browser(auth_url)
   │                                │                  ├▶ await_callback(...)
   │                                │                  └▶ exchange_code → set_oauth_full
   │                                └─ RemoveKey ──▶ confirm modal
   │                                                    └▶ auth.remove
   │
   ├── has_key & key-only ──▶ confirm remove ──▶ auth.remove
   │
   ├── no_key & oauth-capable ──▶ action list modal
   │                              ├─ SetApiKey ──▶ SecurePrompt modal
   │                              └─ StartOAuth ──▶ (same as above)
   │
   └── no_key & key-only ──▶ SecurePrompt modal ──▶ auth.set_api_key

On next Provider.stream call:
   AuthProvider.get_api_key(provider)
       │
       ▼
   refresh_if_expired(provider)
       │
       ├── not expired → return stored access_token
       └── expired + refresh_token → update_oauth_tokens → return new access_token
```

## Error handling

- `set_api_key` failure (disk write) → append error line, leave row state
  unchanged. Secure prompt re-opens with same label.
- `open_browser` failure → fall back to manual URL display, extend timeout
  to 5 min.
- `await_callback` Timeout → append error, abort flow, port released.
- `await_callback` StateMismatch → append error, abort flow, port released.
  (State is per-flow; mismatch means the browser sent a stale callback from
  a previous attempt or a different process.)
- `exchange_code` 4xx → append error with provider's `error_description`
  if present. Most common: invalid `code` (already redeemed or expired) →
  user retries.
- `exchange_code` 5xx or network → append error, suggest retry.
- Refresh failure → mark provider as needing re-login; next request
  surfaces this as a one-line inline error.

## Testing

**Unit.**
- `provider_oauth::build_auth_url`: query param order, URL-encoding, missing
  optional params, PKCE inclusion.
- `provider_oauth::pkce_pair`: verifier length, S256 challenge matches
  RFC 7634 test vector.
- `provider_oauth::exchange_code`: 200 happy path, 4xx with `error_description`,
  5xx, malformed JSON, missing `access_token`.
- `provider_oauth::spec_for`: known provider returns `Some`, unknown returns
  `None`, malformed TOML surfaces a clear error once.
- `oauth_listener::await_callback`: valid code+state, missing code, wrong
  state, wrong path, malformed request, timeout (short timeout in test).
- `OverlayState::secure_input`: empty → first char → backspace → paste
  50 chars → submit.
- `render_overlay` secure mode: `mask_input=true` shows `*` count equal to
  value length; `mask_input=false` shows value; placeholder visible when
  empty.
- `materialize_overlay`: `Modal { secure_prompt: Some(_) }` →
  `overlay.secure_input.is_some()`; `Modal { secure_prompt: None }` →
  `None`.
- `/providers` action matrix: 8 cells (has_key × oauth-capable × action)
  all routed correctly. Use a fake catalog + auth store.
- `oauth_refresh::refresh_if_expired`: unexpired no-op, expired+refresh
  → updated tokens, expired+no refresh → `ReLoginRequired`,
  refresh 401 → `ReLoginRequired`.

**Integration.**
- TCP loopback listener (real socket) with a fake provider HTTP server
  (one-shot `std::net::TcpListener` + `BufRead::read_until`); full flow.
- Refresh race: two concurrent `refresh_if_expired` calls coalesce to one
  network call.

**Manual.**
- TUI smoke: `/providers`, pick keyless openai, enter OAuth flow, complete
  in browser, see "Logged in" + updated `/status`. Re-run, re-login path.
- Headless smoke: same flow with `open_browser` patched to `Err`; verify
  manual-URL fallback message and 5-min timeout.

## Out of scope

- `oxicode setup` wizard changes (kept as fallback).
- Device code, AWS SigV4, gcloud ADC, Azure tenant flows — OAuth table
  schema is forward-compatible but only authorization_code is implemented.
- Browser app auto-detection (only `open::that` from the `open` crate).
- Token refresh backoff / rate limiting (next iteration if needed).
- OAuth provider additions beyond openai + anthropic (schema is ready;
  ship those two first).
- UI polish (loading spinners during OAuth flow) — existing message
  banner is enough for v1.

## Verification

- `cargo fmt --all`.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings`.
- `cargo nextest run -p oxicode-cli` — all existing tests plus the new
  unit/integration tests pass.
- Smoke: `cargo build --release -p oxicode-cli` builds.

## Risks

1. **Browser auto-open on SSH / CI** — degrades to manual-URL fallback,
   but UX surprise. Document in `/shortcuts`.
2. **`127.0.0.1` collisions with other listeners** — port 0 (kernel-assigned)
   sidesteps this; only one oxicode process holds the port at a time.
3. **PKCE state reuse** — `state` is per-flow random 16 bytes; never reused.
4. **Refresh race during shutdown** — `OnceCell` per provider; aborted
   tokio tasks clean up.
5. **Custom providers** — OAuth schema applies only to builtin providers
   loaded from `product-meta.toml`. Custom providers stay key-only.
