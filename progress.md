# Progress

## Status
In Progress — Deep Verification Complete

## Tasks

### Deep Verification Items (7 items)
- [x] 1. `get_provider()` new instance per call — 🟡 Confirmed. Main path OK (`Arc<dyn Provider>`), `stream()` free function wastes connection pools.
- [x] 2. `should_terminate_batch()` logic — 🔴 **Critical bug**. Wrong termination logic: checks `success` instead of explicit `terminate` flag. Missing `terminate` field on `AgentToolResult`. Breaks multi-turn tool use.
- [x] 3. `_persist()` user message loss — 🟢 Same deferred-write pattern as pi-mono. Deliberate design.
- [x] 4. `get_tree()` O(n²) — 🟢 Actually O(n log n). Unused `_id` parameter.
- [x] 5. `prompt_streaming()` functionality — 🟢 Sound architecture. `spawn_blocking` + `LocalSet` pattern correct.
- [x] 6. Public exports — ⚪ No issue. All types correctly exported.
- [x] 7. Test coverage vs pi-mono — 🟡 ~24 oxi tests vs 90+ pi-mono test files. Missing persistence, migration, integration tests.

### Critical Findings
1. **[P0] `should_terminate_batch()` in `oxi-agent/src/agent_loop.rs:552-554`**: Terminates agent loop on ALL successful tool calls instead of requiring explicit opt-in `terminate: true`. `AgentToolResult` lacks the `terminate` field entirely.
2. **[P1] HTTP client reuse in `oxi-ai/src/providers/mod.rs:52-69`**: Each `get_provider()` call creates a new `reqwest::Client`, discarding connection pools.

## Files Changed
- `/tmp/oxi-deep-review.md` — Full review findings

## Notes
- Project compiles cleanly (`cargo check --workspace` passes)
- The `should_terminate_batch()` bug is the most critical — it effectively breaks multi-turn tool use
- Test coverage is functional for basic flows but far below pi-mono's comprehensive suite
