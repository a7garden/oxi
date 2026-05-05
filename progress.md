# oxi Project Progress

## Completed Tasks

### Fix 4: Agent Loop — Parallel Tool Execution + Circuit Breaker (2026-05-05)

**Status: ✅ Complete (code changes)**

- **Parallel tool execution**: Fixed `execute_tool_calls_parallel` to use `futures::future::join_all` instead of sequential `.await` in a for-loop. Tool futures now run concurrently while preserving result order via indexed slots.
- **Circuit breaker integration**: Wired `CircuitBreaker` from `recovery.rs` into `AgentLoop`:
  - Added `circuit_breaker` field to `AgentLoop` struct
  - Initialized with `CircuitBreakerConfig::default()` (threshold: 5, open: 30s)
  - `stream_with_retry` checks `allow_request()` before each attempt, records success/failure
  - When circuit is open, returns error immediately without hitting the provider

**Files modified:**
- `oxi-agent/src/agent_loop.rs` (imports, struct, constructor, parallel execution, stream_with_retry)

**Blocked:** `cargo test -p oxi-agent` cannot run due to pre-existing `oxi-ai` compilation errors (broken `concat!` macros in test code).

**Output:** `/tmp/fix4-agent-loop.md`
