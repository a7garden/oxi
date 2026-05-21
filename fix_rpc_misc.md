# Fix Report: RPC Bash Injection, CLI Thinking, OAuth, Prompt Dedup, AuthStorage Singleton

**Date:** 2026-05-15

## Fix 1: RPC Bash Command Injection — DONE

**File:** `oxi-cli/src/rpc_mode/handlers.rs`

- Added `is_dangerous_rpc_command()` function that detects dangerous patterns:
  - `/etc/passwd`, `id_rsa`, `curl | nc`, `/dev/tcp/`, `rm -rf /`, `> /etc/`, `mkfifo`
- Added `tracing::warn!` log when a dangerous RPC bash command is detected
- The check runs before `sh -c` execution — it logs but does not block (consistent with the task's "at minimum, add a warning log" requirement)

## Fix 2: CLI --thinking Error Message — DONE

**File:** `oxi-cli/src/main.rs`

The error messages listed wrong valid values (`none, minimal, standard, thorough`). Fixed to match the actual `ThinkingLevel` enum and `parse_thinking_level()` function:

- **Before:** `"Valid options: none, minimal, standard, thorough"`
- **After:** `"Valid options: off, minimal, low, medium, high, xhigh"`

Fixed in two locations:
1. `--thinking` CLI argument error (line ~225)
2. `config set thinking_level` error (line ~400)

The actual valid aliases from `parse_thinking_level()` are: `off|none`, `minimal`, `low`, `medium|standard`, `high|thorough`, `xhigh`. The error message now lists the primary variant names.

## Fix 3: OAuth URL Decoding — ALREADY FIXED (SKIP)

**File:** `oxi-cli/src/oauth_server.rs`

The `parse_oauth_callback()` function already uses `urlencoding::decode()` properly:
```rust
let value = parts.next()
    .map(urlencoding::decode)
    .transpose()
    .ok()
    .flatten()
    .unwrap_or_default();
```
No changes needed.

## Fix 4: System Prompt Builder Dedup — TODO ADDED

**Files:** `oxi-cli/src/lib.rs`, `oxi-cli/src/app/agent_session_runtime.rs`

Both files have `build_system_prompt()` functions that:
- Share identical `ThinkingLevel` → custom_prompt matching logic
- Both delegate to `prompt::system_prompt::build_system_prompt()`
- Differ in options: `lib.rs` passes `skills`, `agent_session_runtime.rs` passes `tool_snippets` and `selected_tools`

Added TODO comments to both noting they should be unified into a single shared utility. Full unification would require changing function signatures and is best done as a focused refactoring task.

## Fix 5: AuthStorage Singleton — DONE

**File:** `oxi-store/src/auth_storage.rs`

Added `shared_auth_storage()` function:
```rust
pub fn shared_auth_storage() -> Arc<AuthStorage> {
    static STORAGE: OnceLock<Arc<AuthStorage>> = OnceLock::new();
    STORAGE.get_or_init(|| Arc::new(AuthStorage::new())).clone()
}
```

Returns `Arc<AuthStorage>` so callers can share a single instance without cloning the underlying `AuthStorage` (which doesn't implement `Clone`).

**Updated call sites (17 total):**

| File | Count | Change |
|------|-------|--------|
| `main.rs` | 2 | `AuthStorage::new()` → `shared_auth_storage()` |
| `lib.rs` | 1 | `AuthStorage::new()` → `shared_auth_storage()` |
| `app/agent_session_runtime.rs` | 2 | Service factory uses singleton |
| `tui/handlers.rs` | 4 | All auth lookups use singleton |
| `tui/slash.rs` | 4 | `/auth`, `/logout`, `/login` commands |
| `tui/app.rs` | 1 | App auth lookup |
| `tui/overlay/factories.rs` | 1 | Provider removal |
| `setup_wizard.rs` | 2 | Setup wizard auth |
| `oxi-store/model_resolver.rs` | 1 | Auth check |

**Not changed:**
- `agent_session_runtime.rs` `ModelRegistry::create()` — still uses `AuthStorage::new()` because `ModelRegistry` stores `AuthStorage` by value, not `Arc<AuthStorage>`. This would require changing the `ModelRegistry` API.
