# TUI Tape Production Cutover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace oxicode-cli's alternate-screen, full-frame chat renderer with the existing OMP-aligned main-screen `TapeEngine`, while preserving transient ratatui overlays, current input behavior, rich content, themes, glyph presets, and session semantics.

**Architecture:** `ChatViewState` remains the canonical append-only conversation model. A memoized tape transcript renderer converts each `ChatMessage` and the active `StreamingState` into width-aware ANSI rows, then appends mutable sticky rows for todo/input/footer. `TapeEngine` owns normal main-screen output; transient overlays alone enter the alternate screen and render through ratatui, then return to the untouched main screen. There is no dual production path and no `OXICODE_TAPE_RENDER` flag.

**Tech Stack:** Rust 2024, crossterm 0.29, ratatui 0.30 for transient overlays and off-screen line reuse, oxicode-tui `TapeEngine`, cargo-nextest.

## Global Constraints

- Default transcript rendering MUST use the terminal main screen and native scrollback; it MUST NOT emit `EnterAlternateScreen` during ordinary chat rendering.
- Transient overlays MAY enter alternate screen at open and MUST leave it at close or panic without clearing main-screen scrollback.
- `ChatViewState.messages` and `ChatViewState.streaming` remain the single conversation state; no parallel message store.
- Completed messages are immutable after commit. Only the active streaming suffix and sticky rows may repaint.
- No runtime dual-render flag, deprecated bridge, compatibility alias, or dead alternate transcript path remains.
- Every glyph comes from `ThemeStyles.symbols`; tape components MUST NOT hardcode UI glyphs.
- Theme colors are serialized from ratatui `Style` to ANSI with terminal capability downgrading; no hardcoded SGR colors in components.
- Rich-content behavior already supported by the current chat renderer must survive: markdown, code, tables, LaTeX, Mermaid, tool call/results, thinking, errors, dashboard, and image protocol output.
- Input behavior must survive: Kitty enhancement flags, bracketed paste, SGR 1006 mouse, keybinding conflict resolution, undo, kill/yank/yank-pop, slash/file completion, queue, todo, notifications, and footer status.
- Significant behavioral work follows red-green-refactor: add a focused failing test, observe the expected failure, implement minimally, and rerun the focused test.
- Final gates: `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings`; `cargo fmt --all -- --check`; `cargo nextest run --workspace`.

---

### Task 1: Real Component Memoization and Engine Invariants

**Files:**
- Modify: `oxicode-tui/src/tape/component.rs`
- Modify: `oxicode-tui/src/tape/container.rs`
- Modify: `oxicode-tui/src/tape/engine.rs`
- Modify: `oxicode-tui/src/tape/components/text.rs`
- Modify: `oxicode-tui/src/tape/components/streaming.rs`
- Modify: `oxicode-tui/src/tape/components/tool_call.rs`

**Interfaces:**
- Produces: `Component::revision(&self) -> u64`.
- Produces: memoizing `Container::compose(&mut self, width: u16) -> (&RenderResult, LiveRegion)` or an equivalent borrowed-cache API that does not clone unchanged child rows.
- Produces: engine replay/reset behavior that is explicit on resize/session replacement and byte-tested.

- [ ] **Step 1: Write failing component-cache tests**

Add a counting component test proving: first compose renders once; a second compose at the same revision and width does not call `render`; a width change renders again; a revision change renders again; live-region offsets remain correct across cache hits.

- [ ] **Step 2: Run cache tests and verify RED**

Run: `cargo nextest run -p oxicode-tui tape::container`

Expected: FAIL because `Container::compose` currently calls every child's `render` every time.

- [ ] **Step 3: Add O(1) revision contracts**

Add `Component::revision`. Immutable components may return their stable content hash. Mutable components use a monotonic wrapping revision counter bumped by every output-affecting mutation and `invalidate()`.

- [ ] **Step 4: Implement per-child cache**

Cache `(revision, width, RenderResult, LiveRegion)` per child. On a hit, reuse the stored rows without calling the child. Aggregate rows and hashes only when a child or width changes. Cache invalidation must not require traversing rendered text.

- [ ] **Step 5: Add failing engine production-invariant tests**

Cover: resize triggers full replay without treating old committed row indices as valid; explicit session replacement emits ED3; unchanged frames emit zero bytes; finalized rows advance monotonically between replay boundaries; pinned sticky rows never enter committed scrollback.

- [ ] **Step 6: Implement minimal engine corrections**

Make replay boundaries explicit and teach the engine to distinguish finalized prefix, mutable live suffix, and pinned sticky suffix. Preserve CSI 2026 begin/end pairing on every successful output path.

- [ ] **Step 7: Verify Task 1**

Run: `cargo nextest run -p oxicode-tui tape`

Expected: PASS with cache and engine invariants covered.

---

### Task 2: ANSI Style Serialization and Width-Aware Rich Components

**Files:**
- Create: `oxicode-tui/src/tape/style.rs`
- Create: `oxicode-tui/src/tape/markdown.rs`
- Create: `oxicode-tui/src/tape/transcript.rs`
- Modify: `oxicode-tui/src/tape/mod.rs`
- Modify: `oxicode-tui/src/widgets/chat/markdown.rs`
- Modify: `oxicode-tui/src/widgets/chat/highlight.rs`
- Modify: `oxicode-tui/src/widgets/tool_renderer.rs`
- Modify: `oxicode-tui/src/render/mermaid.rs`
- Modify: `oxicode-tui/src/tape/components/tool_call.rs`

**Interfaces:**
- Consumes: existing `ChatMessage`, `ContentBlock`, `ThemeStyles`, `TerminalCapabilities`, markdown/LaTeX/Mermaid/tool formatting functions.
- Produces: `styled_line_to_ansi(line: &ratatui::text::Line<'_>, caps: &TerminalCapabilities) -> String`.
- Produces: `TranscriptRenderer::sync(&mut self, messages: &[ChatMessage], streaming: Option<&StreamingState>, theme: &Theme, caps: &TerminalCapabilities)`.
- Produces: `TranscriptRenderer::compose(&mut self, width: u16) -> (&RenderResult, LiveRegion)`.

- [ ] **Step 1: Write failing ANSI serialization tests**

Assert foreground/background/modifier transitions, reset behavior, Unicode width preservation, terminal color-level downgrade, OSC 8 closure, and absence of redundant full resets between equal adjacent spans.

- [ ] **Step 2: Verify ANSI tests RED**

Run: `cargo nextest run -p oxicode-tui tape::style`

Expected: FAIL because the serializer does not exist.

- [ ] **Step 3: Implement ANSI serializer**

Reuse `render::ansi::AnsiTracker` and capability color adaptation. Serialize ratatui `Line` spans to one terminal row and terminate styling at row boundaries.

- [ ] **Step 4: Expose pure width-aware formatting helpers**

Promote existing crate-private markdown/highlight helpers only as far as needed inside `oxicode-tui`. Do not duplicate markdown parsing, table layout, code highlighting, Mermaid parsing, or tool formatter logic.

- [ ] **Step 5: Write failing transcript component matrix tests**

For every `ContentBlock` variant, assert width-safe ANSI rows and role/theme/glyph behavior. Include CJK wrapping, tables, fenced code, inline/display LaTeX, Mermaid, executing/completed/error tool calls, collapsed/expanded thinking, retryable error, dashboard, and image placeholder/protocol metadata.

- [ ] **Step 6: Implement `TranscriptRenderer`**

Map each finalized `ChatMessage` to one memoized child. Map the active `StreamingState` to a mutable child whose live boundary begins at the first unstable rendered row. Use `ThemeStyles.symbols` for all icons/spinners/arrows and existing theme styles for all colors.

- [ ] **Step 7: Remove hardcoded tape glyphs/colors**

Replace `▸`, `⠋`, SGR 36, and SGR 90 in `ToolCallBlock` with `ThemeStyles`/`Symbols` inputs or remove the redundant component in favor of the shared formatter.

- [ ] **Step 8: Verify Task 2**

Run: `cargo nextest run -p oxicode-tui tape widgets::chat::markdown widgets::tool_renderer render::mermaid`

Expected: PASS.

---

### Task 3: Main-Screen Terminal Host and Overlay Session

**Files:**
- Create: `oxicode-cli/src/tui/terminal_host.rs`
- Modify: `oxicode-cli/src/tui/mod.rs`
- Modify: `oxicode-cli/src/tui/app.rs`
- Modify: `oxicode-cli/src/tui/render.rs`
- Test: `oxicode-cli/tests/pty_e2e.rs`

**Interfaces:**
- Produces: `TerminalHost` owning `TapeEngine<io::Stdout>` and terminal mode state.
- Produces: `TerminalHost::paint_tape(&mut self, frame: &[String], live: LiveRegion, size: (u16, u16))`.
- Produces: `TerminalHost::draw_overlay<F>(&mut self, draw: F)` using alternate screen only while an overlay is active.
- Produces: idempotent `restore()` used by normal exit, Drop, and panic fallback.

- [ ] **Step 1: Write failing lifecycle byte tests**

Using an injectable writer/backend, assert ordinary enter/paint/exit never emits `\x1b[?1049h`; raw mode adjunct sequences remain balanced; overlay open emits one enter-alt; overlay close emits one leave-alt; panic restoration shows cursor, disables mouse/paste/keyboard flags, and leaves alt if active.

- [ ] **Step 2: Verify lifecycle tests RED**

Run: `cargo nextest run -p oxicode-cli terminal_host`

Expected: FAIL because `TerminalHost` does not exist and current `Tui::enter` always enters alternate screen.

- [ ] **Step 3: Implement `TerminalHost`**

Move setup/cleanup from `app.rs`. Normal entry enables raw mode, bracketed paste, keyboard enhancement flags, and SGR mouse, but not alternate screen. Tape owns stdout. Overlay transition borrows the tape writer, enters alternate screen once, renders with a temporary ratatui `Terminal<DiffBackend<_>>`, and leaves once on transition back.

- [ ] **Step 4: Make resize explicit**

Read `crossterm::terminal::size()` each loop iteration. Pass geometry to tape. Geometry changes trigger engine replay; overlay geometry is handled by ratatui.

- [ ] **Step 5: Remove old terminal wrapper and cursor bridge**

Delete `Tui`, the full-frame `terminal.draw()` path, and transcript `CursorState` handling. Overlay-local cursor positioning remains in the overlay draw path only.

- [ ] **Step 6: Verify Task 3**

Run: `cargo nextest run -p oxicode-cli terminal_host`

Expected: PASS.

---

### Task 4: Production Tape Frame and Event Wiring

**Files:**
- Create: `oxicode-cli/src/tui/tape_render.rs`
- Modify: `oxicode-cli/src/tui/mod.rs`
- Modify: `oxicode-cli/src/tui/app.rs`
- Modify: `oxicode-cli/src/tui/handlers.rs`
- Modify: `oxicode-cli/src/tui/render.rs`
- Modify: `oxicode-tui/src/widgets/chat/state.rs`

**Interfaces:**
- Consumes: canonical `AppState.chat`, todo/input/footer/queue/notification state, `Theme`, terminal size.
- Produces: `TapeRenderState::sync(&mut self, app: &AppState, theme: &Theme, size: (u16, u16))`.
- Produces: `TapeRenderState::compose(&mut self) -> (&[String], LiveRegion, Option<CursorPosition>)`.

- [ ] **Step 1: Write failing event-to-tape tests**

Drive real `UiEvent` sequences through `handle_ui_event`: user message; MessageStart/Update/End; thinking; tool start/result/duration; error; image; cancellation; resumed branch; branch replacement. Assert the tape snapshot has one copy of each finalized block and only the active suffix changes during streaming.

- [ ] **Step 2: Verify event tests RED**

Run: `cargo nextest run -p oxicode-cli tui::tape_render`

Expected: FAIL because no production tape render state exists.

- [ ] **Step 3: Implement `TapeRenderState` as a projection**

Do not add another message store. Sync from `ChatViewState.messages` and `.streaming`, using message identity/content revisions to update transcript components. Session replacement requests ED3 replay; ordinary append does not.

- [ ] **Step 4: Compose sticky rows**

Render queue/todo/status/input/footer/completion/notification rows below transcript. Mark the first mutable streaming row as live; if no stream exists, mark the first sticky row pinned so input/footer never commit to scrollback.

- [ ] **Step 5: Wire the main loop**

Replace `render::draw` for ordinary chat with `TapeRenderState::sync/compose` and `TerminalHost::paint_tape`. Continue handling input events and agent events through existing handlers.

- [ ] **Step 6: Preserve scroll/session semantics**

Map wheel/page navigation to viewport-tail behavior without mutating committed history. Resume populates the transcript before first paint. Branch switch clears/replays once and never duplicates prior rows.

- [ ] **Step 7: Verify Task 4**

Run: `cargo nextest run -p oxicode-cli tui::tape_render tui::handlers`

Expected: PASS.

---

### Task 5: Overlay, Completion, Input, and Rich-Media Integration

**Files:**
- Modify: `oxicode-cli/src/tui/terminal_host.rs`
- Modify: `oxicode-cli/src/tui/tape_render.rs`
- Modify: `oxicode-cli/src/tui/app.rs`
- Modify: `oxicode-cli/src/tui/render.rs`
- Modify: `oxicode-tui/src/tape/transcript.rs`
- Modify: `oxicode-tui/src/tape/engine.rs`

**Interfaces:**
- Consumes: all existing overlay components and `render_overlay`.
- Produces: clean overlay transition state machine and protocol-safe image rows.

- [ ] **Step 1: Write failing overlay transition tests**

Open and close representative overlays (settings, model selector, ask, issues panel) around a populated tape. Assert one alternate-screen pair per overlay lifetime, correct input routing, and intact transcript after return.

- [ ] **Step 2: Write failing sticky interaction tests**

Cover multi-line input growth, slash and file completion, queue expand/collapse, todo updates, notification expiry, kill/yank/yank-pop, undo, paste, mouse scrolling, and Kitty-modified keys.

- [ ] **Step 3: Implement overlay state transitions**

Ordinary frames use tape. Any overlay draws exclusively in alternate screen. Closing restores main screen without ED3; the next tape frame may repaint the mutable/sticky window but must not duplicate finalized scrollback.

- [ ] **Step 4: Implement image-safe tape output**

Represent protocol image rows separately from ordinary terminated text rows, or add explicit raw-row metadata so `LINE_TERMINATOR` cannot corrupt Kitty/iTerm2 payloads. Maintain the current fallback text for unsupported terminals.

- [ ] **Step 5: Verify Task 5**

Run: `cargo nextest run -p oxicode-cli tui oxicode_tui`

Expected: PASS.

---

### Task 6: PTY Cutover Acceptance and Old-Path Removal

**Files:**
- Modify: `oxicode-cli/tests/pty_e2e.rs`
- Modify: `oxicode-cli/src/tui/app.rs`
- Modify: `oxicode-cli/src/tui/render.rs`
- Modify: `oxicode-tui/src/lib.rs`
- Modify: `oxicode-tui/src/tape/engine.rs`
- Remove: obsolete transcript-only ratatui render code and stale cursor bridge fields after references reach zero.

**Interfaces:**
- Produces: end-to-end proof that production TUI uses native scrollback tape.

- [ ] **Step 1: Replace the old PTY assertion and verify RED**

Change the test from expecting `\x1b[?1049h` on launch to asserting it is absent during ordinary chat. Submit a prompt through a deterministic local/mock path, observe rendered transcript output, exit, and assert the conversation remains in main-screen output.

Run: `cargo nextest run -p oxicode-cli --test pty_e2e test_pty_tui_renders_and_exits`

Expected: FAIL against the old alt-screen implementation before Task 3/4, and PASS after cutover.

- [ ] **Step 2: Add streaming differential PTY coverage**

Capture two streaming updates and assert the finalized prefix is emitted once while the live suffix changes in place. Verify balanced CSI 2026, cursor restoration, and clean exit.

- [ ] **Step 3: Add overlay PTY coverage**

Open one overlay, assert one alternate-screen enter/leave pair, close it, continue typing, and verify the main-screen transcript survives.

- [ ] **Step 4: Add resize/session-replace PTY coverage**

Resize the PTY and switch/resume a session. Assert ED3 replay occurs only for destructive replacement and output contains no duplicated finalized rows.

- [ ] **Step 5: Delete old production transcript path**

Remove dead `ChatView` full-frame callsites, stale `last_input_cursor`/transcript `CursorState`, standalone/not-wired comments, and any `OXICODE_TAPE_RENDER` forward references. Keep reusable ratatui formatting and overlay code.

- [ ] **Step 6: Verify Task 6**

Run: `cargo nextest run -p oxicode-cli --test pty_e2e`

Expected: all PTY tests pass with ordinary chat on main screen.

---

### Task 7: Final Verification and Documentation Cutover

**Files:**
- Modify: `AGENTS.md`
- Modify: `CHANGELOG.md`
- Modify: `oxicode-tui/README.md`
- Modify: `oxicode-tui/GUIDE.md`
- Modify: `docs/superpowers/specs/2026-07-29-p2-tui-tape-model-design.md`
- Modify: status/handoff documents that still describe v2, legacy dual-linking, or standalone tape.

**Interfaces:**
- Produces: one truthful description of the shipped architecture.

- [ ] **Step 1: Run focused TUI gates**

```bash
cargo build -p oxicode-tui -p oxicode-cli
cargo clippy -p oxicode-tui --all-targets -- -D warnings
cargo clippy -p oxicode-cli --all-targets -- -D warnings
cargo nextest run -p oxicode-tui
cargo nextest run -p oxicode-cli --test pty_e2e
```

- [ ] **Step 2: Run full repository gates**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace
```

- [ ] **Step 3: Update architecture documentation**

Document: main-screen tape is production; completed rows commit to native scrollback; mutable/sticky suffix diffing; overlays alone use alternate screen; single `oxicode-tui`; current test evidence. Remove the obsolete v2 pipeline and always-alt-screen claims.

- [ ] **Step 4: Verify documentation assertions against code**

Search production Rust for `EnterAlternateScreen`, `terminal.draw`, `CursorState`, `OXICODE_TAPE_RENDER`, and `not wired`. Every remaining occurrence must be overlay-only, test-only, or intentionally reusable and documented.

- [ ] **Step 5: Final differential review**

Review terminal restoration, output-byte correctness, cache invalidation, Unicode width, stream finalization, session replay, images, overlays, and input routing. Resolve all load-bearing findings before completion.
