# Brief 03: Code Quality — Production Expects and Unwraps

**Area:** `.expect()` and `.unwrap()` calls in non-test production code paths
**Severity:** 🟢 Normal
**Estimated scope:** ~80 production `unwrap()` calls (mostly in test-file edge cases), ~90 production `expect()` calls concentrated in providers and streaming

---

## Context

The project has **1165 total `.unwrap()` calls**, but analysis reveals nearly all are in `#[cfg(test)]` blocks or test helper files. The actual production unwrap count is ~80, with most in `tests.rs` files (which are technically module tests) and `examples/`. The real concern is **`.expect()` calls in production paths** — 7 in streaming, 10 in bedrock, and scattered elsewhere.

Most of these `expect()` calls are on **infallible operations** (e.g., `"application/json".parse().expect("valid header value")`, `HmacSha256::new_from_slice(key).expect("HMAC can take key of any size")`) and are safe. However, some in `streaming.rs` are on collection operations that could theoretically fail if invariants are violated.

Current state (production code only, excluding tests/examples):

| File | `.expect()` Count | Risk Assessment |
|------|-------------------|-----------------|
| `oxi-ai/src/providers/bedrock.rs` | 10 | Low — all parse/infallible crypto ops |
| `oxi-agent/src/agent_loop/streaming.rs` | 7 | **Medium** — `messages.last().expect("non-empty")` — could fail if invariant breaks |
| `oxi-cli/src/rpc_mode/handlers.rs` | 5 | Low — RPC handler setup |
| `oxi-ai/src/providers/openai.rs` | 3 | Low |
| `oxi-store/src/session.rs` | 2 | Low |
| `oxi-store/src/session_navigation.rs` | 2 | Low |
| `oxi-store/src/model_resolver.rs` | 2 | Low |
| Other files (1-2 each) | ~50 | Low |

**True production `unwrap()` in non-test, non-example code:**

| File | Count | Risk |
|------|-------|------|
| `oxi-sdk/src/multi_provider.rs` | 2 | Low — SDK builder |
| `oxi-ai/src/fallback_chain.rs` | 2 | Low — chain construction |
| `oxi-sdk/src/lib.rs` | 1 | Low |
| `oxi-sdk/src/middleware/builtins.rs` | 1 | Low |
| `oxi-sdk/src/coordination/work_queue.rs` | 1 | Low |
| `oxi-ai/src/model_db.rs` | 1 | Low |

---

## Objective

Audit every `.expect()` in `streaming.rs` and verify that the invariants they guard are either always true or should be replaced with proper error handling.

This does NOT mean:
- ❌ Removing all `unwrap()`/`expect()` from the codebase (many are correct and intentional)
- ❌ Replacing infallible operations like `"literal".parse()` with error handling
- ❌ Adding error types where none are needed
- ❌ Touching test code

It DOES mean:
- ✅ Every `.expect()` in production code has a justified reason for being there
- ✅ The 7 `expect("non-empty")` calls in `streaming.rs` are verified safe or replaced with proper error returns
- ✅ Document which expects are intentionally infallible

---

## Approach

### Phase 1: Audit (read-only)

1. Read `oxi-agent/src/agent_loop/streaming.rs` and trace the 7 `expect("non-empty")` calls:
   - `line 92`: `messages.last().expect("non-empty")`
   - `line 165`: `messages.last().expect("non-empty")`
   - `line 187`: `messages.last().expect("non-empty after push")`
   - `line 226`: `messages.last().expect("non-empty")`
   - `line 250`: `messages.last().expect("non-empty")`
   - `line 283`: `messages.last().expect("non-empty")`
   - `line 365`: `messages.last().expect("non-empty")`

2. For each, trace the code path to determine:
   - Is the collection guaranteed non-empty at this point by construction?
   - If yes, is the `expect` message sufficient to explain why?
   - If no, what error should be returned instead?

3. Read `oxi-ai/src/providers/bedrock.rs` lines 198-436 and verify the 10 `expect()` calls are all infallible header/crypto operations.

### Phase 2: Fix if needed

1. If any `expect()` in streaming.rs guards a **fallible** operation, replace with:
   ```rust
   let last_msg = messages.last().ok_or_else(|| anyhow::anyhow!("message list unexpectedly empty"))?;
   ```
2. If the operation is **provably infallible** (collection was just pushed to), consider adding a comment:
   ```rust
   // Invariant: messages is non-empty — we just pushed above
   let last_msg = messages.last().expect("non-empty after push");
   ```

### Phase 3: Verify

1. `cargo check --workspace` — compiles clean
2. `cargo nextest run --workspace` — all 2131 tests pass
3. `cargo clippy --workspace -- -D warnings` — no new warnings

---

## Constraints

- **Do not** add new error types — use existing `anyhow::Result` or the crate's error enum.
- **Do not** change the streaming architecture or message flow.
- **Preserve** the existing test suite.
- **Do not** touch `expect()` calls that are clearly infallible (parse literals, HMAC key size, etc.).

## Verification

1. `cargo nextest run --workspace` — 2131 tests pass
2. `cargo clippy --workspace -- -D warnings` — clean
3. Audit `streaming.rs` and verify every `expect()` either has an invariant comment or returns a proper error.
