# oxicode-tui v2 — Plan C: OSC8 + Input Textarea + oxicode-cli Cutover

**Goal:** Complete the remaining oxicode-tui v2 features (OSC8 hyperlink emission, input textarea) and cut over oxicode-cli from oxicode-tui-legacy to the new oxicode-tui.

**Tasks:**
1. PR-7: OSC8 row-write emission inside DiffBackend (inside CSI 2026 window)
2. PR-8: Input textarea wrapper (stock ratatui-textarea 0.9)
3. PR-9: oxicode-cli cutover — switch from legacy to new pipeline (large)

**Spec:** `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md` §9, §10

## Task C1: OSC8 row-write emission (PR-7)

Fill in the `set_links()` hook on DiffBackend to actually emit OSC8 escapes inside row writes, inside the CSI 2026 window. Currently `set_links` stores links but doesn't emit.

**Files:** `oxicode-tui/src/pipeline/diff_backend/mod.rs`
**LOC:** ~100

## Task C2: Input textarea wrapper (PR-8)

Thin wrapper around stock ratatui-textarea 0.9. Provides IME, paste, undo.

**Files:** `oxicode-tui/src/input/mod.rs`, `oxicode-tui/src/input/textarea.rs`
**LOC:** ~200

## Task C3: oxicode-cli cutover (PR-9)

Switch oxicode-cli's TUI from oxicode-tui-legacy to new oxicode-tui. THE big integration.

**Files:** `oxicode-cli/src/tui/app.rs` + 27+ oxicode-cli files
**LOC:** ~3,000

This is the largest single task. It requires:
- Replacing `terminal.draw(closure)` with `draw_frame()`
- Converting oxicode-cli's AppState to use new ChatView/ChatLog
- Converting all overlays to Renderable impls (or keeping them as overlay trait impls that call new primitives)
- Wiring theme + caps through to draw_frame

**Approach:** Multiple sub-tasks, dispatched sequentially.
