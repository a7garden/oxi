# Task 3 Brief — Main-Screen Terminal Host and Overlay Session

Plan: `docs/superpowers/plans/2026-07-29-tui-tape-production-cutover.md`
Prerequisites: Tasks 1-2 complete and reviewed.

## Files
- Create `oxi-cli/src/tui/terminal_host.rs`
- Modify `oxi-cli/src/tui/mod.rs`, `app.rs`, `render.rs`
- Test terminal_host module and `oxi-cli/tests/pty_e2e.rs` only where required for this lifecycle task

## Required interfaces
- `TerminalHost` owns `TapeEngine<io::Stdout>` in production and supports an injectable writer/backend in tests.
- `paint_tape(&mut self, frame: &[String], live: LiveRegion, size: (u16,u16))`.
- `draw_overlay<F>(&mut self, draw: F)` or an equivalent closure API that enters alternate screen only for overlay lifetime and uses the existing ratatui overlay renderer.
- Idempotent `restore()` used by normal exit and Drop; panic fallback emits terminal restoration sequences without borrowing invalid state.

## Behavior
- Ordinary entry: raw mode, bracketed paste, keyboard enhancement flags, mouse ?1000/?1006, hidden cursor as needed; NO EnterAlternateScreen and no full clear.
- Ordinary tape paint: main screen only.
- Overlay transition: flush tape, enter alternate screen once, render repeated overlay frames without repeated enter, leave once on close, return to main screen without ED3.
- Exit/panic: disable mouse, pop keyboard flags, disable paste, leave alternate only if active, show cursor, disable raw mode. Each cleanup attempt independent.
- Explicit terminal size each frame via crossterm; geometry passed to tape.
- Remove old Tui wrapper, normal full-frame terminal.draw, transcript CursorState bridge only when replacement compiles. Overlay-local cursor support remains.

## TDD
- Injectable writer lifecycle tests first; observe RED.
- Assertions: ordinary lifecycle lacks 1049h/1049l; flags balanced; overlay has exactly one pair; restore idempotent; panic byte fallback restores all modes.
- GREEN: `cargo nextest run -p oxi-cli terminal_host`.
- Lint/format: `cargo clippy -p oxi-cli --all-targets -- -D warnings`; `cargo fmt --all -- --check`.

## Report
Write `.superpowers/sdd/2026-07-29-tui-tape-production-cutover/task-3-report.md` with status, RED/GREEN evidence, lifecycle state machine, commits, self-review, concerns. Commit changes.
