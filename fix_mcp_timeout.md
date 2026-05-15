# MCP Client Timeout & Agent Fixes

**Date:** 2026-05-15
**Status:** ✅ All fixes applied and compiling

---

## Fix 1: MCP `read_message()` Timeout (CRITICAL)

**File:** `oxi-agent/src/mcp/client.rs`
**Problem:** `read_message()` had no timeout — if an MCP server stalled or silently disconnected, the `read_line()` call would block forever, hanging the agent.
**Fix:** Wrapped the entire header-parsing + body-read logic in `tokio::time::timeout(REQUEST_TIMEOUT_SECS)`. If the read exceeds 30s, a clear error `"MCP read_message timed out after 30s"` is returned.

```rust
async fn read_message(&mut self) -> Result<RawJsonRpcMessage> {
    tokio::time::timeout(
        std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS),
        async {
            // ... existing header parsing + body read ...
        },
    )
    .await
    .map_err(|_| anyhow::anyhow!("MCP read_message timed out after {}s", REQUEST_TIMEOUT_SECS))?
}
```

This also complements the existing timeout in `send_request()` which now has two layers:
1. Per-request outer timeout in `send_request()` (30s)
2. Per-read inner timeout in `read_message()` (30s) — prevents the drain loop from hanging too

---

## Fix 2: MCP Backoff Configuration

**Files:** `oxi-agent/src/mcp/mod.rs`, `oxi-agent/src/mcp/types.rs`
**Problem:** `FAILURE_BACKOFF_SECS = 60` was hardcoded, meaning after a server connection failure the agent waited a full minute before retrying.
**Fix:**
- Reduced default from 60s → 30s
- Made it configurable via `McpSettings.failure_backoff_secs` in the MCP config JSON
- Added `failure_backoff_secs()` helper method on `McpManager` that reads from config with fallback

Config example:
```json
{
  "settings": {
    "failure_backoff_secs": 15
  }
}
```

---

## Fix 3: `should_stop_after_turn` Hook Ownership

**Files:** `oxi-agent/src/config.rs`, `oxi-agent/src/agent.rs`, `oxi-cli/src/tui/app.rs`
**Problem:** `hooks.should_stop_after_turn.take()` consumed the `Box<dyn Fn>` on first use, so the hook only worked for one agent run. Subsequent runs had no stop hook.
**Fix:** Changed the type from `Option<Box<dyn Fn...>>` to `Option<Arc<dyn Fn...>>`. This allows `clone()` instead of `take()`, so the hook survives across multiple runs.

Changes:
- `config.rs`: Changed type to `Arc<dyn Fn(&ShouldStopAfterTurnContext) -> bool + Send + Sync>`
- `agent.rs`: Changed `hooks_w.should_stop_after_turn.take()` → `hooks_r.should_stop_after_turn.clone()`
- `tui/app.rs`: Changed `Box::new(...)` → `Arc::new(...)`

---

## Verification

All changes pass `cargo check --package oxi-agent --package oxi-cli` with zero errors.
