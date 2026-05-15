# Progress: MCP Client Timeout & Agent Fixes

## 2026-05-15 — Fix MCP timeout + backoff + hook ownership

### Completed
- [x] **Fix 1 (CRITICAL):** Added `tokio::time::timeout` to `McpClient::read_message()` in `oxi-agent/src/mcp/client.rs` — prevents indefinite blocking on stalled MCP servers
- [x] **Fix 2:** Made `FAILURE_BACKOFF_SECS` configurable via `McpSettings.failure_backoff_secs` and reduced default from 60s to 30s — `oxi-agent/src/mcp/mod.rs` + `types.rs`
- [x] **Fix 3:** Changed `should_stop_after_turn` hook from `Box<dyn Fn>` to `Arc<dyn Fn>` so it can be cloned instead of consumed — `config.rs`, `agent.rs`, `tui/app.rs`
- [x] All changes compile clean (`cargo check` passes)

### Files Modified
| File | Change |
|------|--------|
| `oxi-agent/src/mcp/client.rs` | Timeout wrapper on `read_message()` |
| `oxi-agent/src/mcp/mod.rs` | Configurable backoff, helper method |
| `oxi-agent/src/mcp/types.rs` | `failure_backoff_secs` field on `McpSettings` |
| `oxi-agent/src/config.rs` | `Arc<dyn Fn>` type for `should_stop_after_turn` |
| `oxi-agent/src/agent.rs` | `clone()` instead of `take()` for hook |
| `oxi-cli/src/tui/app.rs` | `Arc::new()` instead of `Box::new()` |

### Details
See `fix_mcp_timeout.md` for full technical writeup.
