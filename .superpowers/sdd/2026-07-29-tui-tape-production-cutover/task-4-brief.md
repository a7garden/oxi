# Task 4 Brief — Production Tape Frame and Event Wiring

Plan: `docs/superpowers/plans/2026-07-29-tui-tape-production-cutover.md`
Prerequisites: Tasks 1-3 complete and reviewed.

## Files
- Create `oxi-cli/src/tui/tape_render.rs`
- Modify `oxi-cli/src/tui/mod.rs`, `app.rs`, `handlers.rs`, `render.rs`
- Modify `oxi-tui/src/widgets/chat/state.rs` only for stable revision/introspection APIs needed by the projection

## Invariants
- `ChatViewState.messages` and `.streaming` remain the only conversation state. TapeRenderState is a renderer/projection, never a parallel message store.
- Completed messages are immutable. Active stream is mutable. Sticky rows never commit.
- Session replacement/branch switch requests one ED3 replay; append and ordinary stream updates never do.

## Required behavior
- `TapeRenderState::sync` projects messages and stream through Task 2 TranscriptRenderer.
- Compose transcript plus sticky queue/todo/status/input/footer/completion/notifications.
- Live boundary: first unstable streaming row. If no stream, first sticky row is pinned. If stream exists, live starts at stream and includes sticky suffix.
- Preserve UI event semantics: user message, MessageStart/Update/End, thinking, tool execution start/end/duration, error, image, cancellation, resume, session/branch replacement.
- Preserve wheel/page behavior as viewport-tail navigation without rewriting committed history.
- Main loop calls TerminalHost paint; ordinary `render::draw` no longer paints transcript.

## TDD
- Event-to-tape tests first using real handle_ui_event sequences; observe RED.
- Assert no duplicate finalized blocks, live-only changes, correct tool/thinking transitions, cancel marker, resume and replacement behavior.
- GREEN: `cargo nextest run -p oxi-cli tui::tape_render tui::handlers`.
- Lint/format: `cargo clippy -p oxi-cli --all-targets -- -D warnings`; `cargo fmt --all -- --check`.

## Report
Write `.superpowers/sdd/2026-07-29-tui-tape-production-cutover/task-4-report.md` with status, RED/GREEN, data-flow decisions, commits, self-review, concerns. Commit changes.
