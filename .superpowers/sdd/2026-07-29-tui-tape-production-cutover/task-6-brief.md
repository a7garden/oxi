# Task 6 Brief — PTY Cutover Acceptance and Old-Path Removal

Plan: `docs/superpowers/plans/2026-07-29-tui-tape-production-cutover.md`
Prerequisites: Tasks 1-5 complete and reviewed.

## Required acceptance
- Ordinary TUI launch/chat must not emit `\x1b[?1049h`.
- Main-screen transcript remains in captured terminal output after clean exit.
- Two streaming updates emit finalized prefix once and mutate only suffix; CSI 2026 pairs balance.
- Overlay emits exactly one alternate enter/leave pair and returns to usable main-screen chat.
- Resize and session replacement replay without duplicate finalized rows; ED3 only for destructive replacement/replay semantics.
- Cursor and all terminal modes restored on exit.

## Work
- Replace old alt-screen PTY expectation with main-screen assertions and first observe RED against old behavior if the prior cutover is not yet active in the test harness.
- Add deterministic local/mock provider or existing fixture use; no network.
- Add streaming differential, overlay, resize/session replacement PTY cases.
- Remove dead ChatView full-frame production callsites, transcript cursor bridge fields, standalone/not-wired comments, and OXICODE_TAPE_RENDER forward references. Keep reusable overlay and formatting code.

## Verification
- `cargo nextest run -p oxicode-cli --test pty_e2e`
- `cargo clippy -p oxicode-cli --all-targets -- -D warnings`
- `cargo clippy -p oxicode-tui --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

## Report
Write task-6-report.md in this plan workspace with RED/GREEN evidence, PTY byte assertions, deleted old paths, commits, self-review, concerns. Commit changes.
