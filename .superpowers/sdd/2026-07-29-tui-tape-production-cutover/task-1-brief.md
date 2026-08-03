# Task 1 Brief — Real Component Memoization and Engine Invariants

Plan: `docs/superpowers/plans/2026-07-29-tui-tape-production-cutover.md`

## Files
- Modify `oxicode-tui/src/tape/component.rs`
- Modify `oxicode-tui/src/tape/container.rs`
- Modify `oxicode-tui/src/tape/engine.rs`
- Modify `oxicode-tui/src/tape/components/text.rs`
- Modify `oxicode-tui/src/tape/components/streaming.rs`
- Modify `oxicode-tui/src/tape/components/tool_call.rs`

## Required behavior
1. Add `Component::revision(&self) -> u64`. Contract: O(1), stable between output-affecting mutations, changes for every output-affecting mutation; width is a separate cache key.
2. Immutable components may return stable content hash. Mutable components use a monotonic wrapping revision counter bumped by mutations and `invalidate()`.
3. Add real per-child cache to `Container`: `(revision, width, RenderResult, LiveRegion)`. A cache hit MUST NOT call child.render(). `compose` should expose a borrowed aggregate result or equivalent clone-free result for unchanged composition.
4. Cache must remain correct when child count/order changes, width changes, revision changes, invalidation occurs, and live-region offsets change.
5. Engine must distinguish finalized prefix, mutable live suffix, and pinned sticky suffix. Pinned rows never commit to scrollback.
6. Replay boundaries: resize and explicit session replacement reset prior committed-row assumptions. Unchanged frames produce zero bytes. CSI 2026 begin/end always pair.

## TDD requirements
- First add counting component tests proving first render, cache hit, width miss, revision miss, and live-region offset preservation. Run and record the expected RED failure.
- Add engine tests for resize replay, explicit ED3 replacement, idle zero-byte output, monotonic finalized commits between replay boundaries, and pinned rows not committed. Record RED before implementation where behavior is absent.
- Then implement minimally and run `cargo nextest run -p oxicode-tui tape`.
- Also run `cargo clippy -p oxicode-tui --all-targets -- -D warnings` and `cargo fmt --all -- --check`.

## Constraints
- No CLI integration in this task.
- Avoid avoidable allocations/copies. In particular, do not clone all finalized/live rows merely to calculate a revision.
- Preserve current public APIs where possible, but correctness and borrowed cache reuse govern.
- No hardcoded glyph/theme cleanup yet except adjustments forced by the revision interface.

## Report
Write `.superpowers/sdd/2026-07-29-tui-tape-production-cutover/task-1-report.md` containing:
- status (`DONE`, `DONE_WITH_CONCERNS`, `NEEDS_CONTEXT`, or `BLOCKED`)
- RED commands/output summary
- implementation summary and invariants
- GREEN/lint/format commands with pass counts
- commit(s)
- self-review findings and concerns
Commit the implementation.
