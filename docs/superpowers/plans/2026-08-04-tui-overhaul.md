# TUI Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire every unused vtui design-system capability into the live TUI — wrapping, scrollbar, overlays, reasoning indicator, vim mode, thinking treatment, todo pane, queue display, compact mode, theme accessibility, welcome layout, follow-up chips.

**Architecture:** All changes are in two crates: `oxicode-vtui` (API exposure — making `pub(crate)` items public) and `oxicode-cli/src/tui_vt/` (wiring — `main_loop.rs`, `frame_layout.rs`, `slash/registry.rs`). The inline protocol (`InlineCommand`/`InlineEvent`/`InlineHandle`) is the bridge between agent events and the render loop. No new crates.

**Tech Stack:** Rust 2024, ratatui, crossterm, tokio, parking_lot, pulldown-cmark, syntect.

## Global Constraints

- `cargo fmt --all -- --check` must pass after every task.
- `cargo clippy -p oxicode-cli --all-targets -- -D warnings` must pass.
- `cargo nextest run -p oxicode-cli` must pass (all tests, no regressions).
- `#![allow(dead_code)]` in `oxicode-cli/src/lib.rs` suppresses unused-fn lints → every new render path MUST have a TestBackend render test.
- `parking_lot::MutexGuard` is `!Send` → drop the guard before any `.await`.
- `oxicode-vtui` has `#![allow(dead_code)]` too → same testing rule applies.
- Commit messages: Conventional Commits, English (`feat(tui):`, `fix(tui):`, etc.).
- Commit after each task.

## File Map

| File | Responsibility |
|---|---|
| `oxicode-cli/src/tui_vt/main_loop.rs` | Event loop, render_frame, apply_command, input thread, all render_* fns |
| `oxicode-cli/src/tui_vt/frame_layout.rs` | Chrome rendering: StatusBar, ShortcutsBar, layout config |
| `oxicode-cli/src/tui_vt/slash/registry.rs` | Slash command definitions and dispatch |
| `oxicode-vtui/src/vim/mod.rs` | Vim module re-exports (make public) |
| `oxicode-vtui/src/tui/core_tui/types/protocol.rs` | InlineCommand/InlineEvent/InlineHandle |

---

## Phase 1: Rendering Pipeline Fixes

### Task 1: Transcript line wrapping + scrollbar space reclaim

**Problem:** `render_transcript` uses ratatui `List` which clips lines wider than the viewport. Also, `ScrollbarConfig::default()` has `enabled: true` but no scrollbar is ever drawn, wasting 2 columns.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — `render_transcript` (currently uses `List`)
- Modify: `oxicode-cli/src/tui_vt/frame_layout.rs` — pass `ScrollbarConfig { enabled: false, .. }` to layout

**Approach:** Replace `List` with a manual paragraph-based renderer that wraps long lines. Each `TranscriptLine` becomes a `Paragraph` with `.wrap(Wrap { trim: false })` rendered at the correct y-offset. The scroll offset logic stays byte/line-based (existing `effective_scroll_offset`).

Since wrapping changes how many visual rows a single transcript line occupies, compute visible lines by rendering top-down and counting wrapped rows until the viewport is full.

- [ ] **Step 1:** Disable the phantom scrollbar — change `frame_layout.rs:141` from `&ScrollbarConfig::default()` to `&ScrollbarConfig { enabled: false, ..Default::default() }`.

- [ ] **Step 2:** Rewrite `render_transcript` to use wrapped paragraph rendering:

```rust
fn render_transcript(frame: &mut Frame<'_>, area: Rect, state: &RenderState) {
    if state.transcript.is_empty() {
        render_welcome(frame, area);
        return;
    }
    let styles = active_styles();
    let viewport = area.height as usize;

    // Build ratatui Lines from transcript items.
    let lines: Vec<Line<'_>> = state.transcript.iter()
        .map(|tl| transcript_line(tl, &styles))
        .collect();

    // Render top-down, wrapping each line, until the viewport is full.
    let total = lines.len();
    let start = effective_scroll_offset(state.scroll_offset, total, viewport);

    let mut y = area.top();
    for line in lines.into_iter().skip(start) {
        if y >= area.bottom() { break; }
        let para = Paragraph::new(line).wrap(Wrap { trim: false });
        let height = para.line_count(area.width) as u16;
        let row = Rect { x: area.x, y, width: area.width, height: 1 };
        frame.render_widget(para, row);
        y += height;
    }
}
```

Extract a `transcript_line` helper (returns `Line`) from the existing `transcript_item` (returns `ListItem`). The glyph prefix logic stays the same.

- [ ] **Step 3:** Add a TestBackend test `transcript_wraps_long_lines` — create a state with a transcript line longer than the terminal width, render at 40 cols, assert the text appears on multiple rows.

- [ ] **Step 4:** Run `cargo nextest run -p oxicode-cli` — all tests must pass.

- [ ] **Step 5:** Commit: `feat(tui): wrap transcript lines + reclaim scrollbar space`

---

### Task 2: PendingHint for two-press quit + compact mode for ShortcutsBar

**Files:**
- Modify: `oxicode-cli/src/tui_vt/frame_layout.rs` — `render_chrome` + `shortcut_hints`
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — pass `pending_quit` to chrome

**Approach:**
- When `state.pending_quit` is true, pass a `PendingHint { key: "Ctrl+C", label: "quit" }` to the ShortcutsBar via `.pending()`.
- Pass a `CompactConfig` to the ShortcutsBar when the terminal is short (≤20 rows).

- [ ] **Step 1:** Change `render_chrome` signature to take `&RenderState` (already does) and construct the ShortcutsBar with pending hint:

```rust
let mut bar = ShortcutsBar::new(&hints, &shortcut_styles);
if state.pending_quit {
    bar = bar.pending(PendingHint { key: "Ctrl+C", label: "quit" });
}
if compact {
    bar = bar.compact(CompactConfig::default());
}
frame.render_widget(bar, layout.shortcuts);
```

- [ ] **Step 2:** Add TestBackend test: set `state.pending_quit = true`, render, assert "press" and "again" appear in the output.

- [ ] **Step 3:** Run tests, commit: `feat(tui): show pending-quit hint + compact shortcuts on short terminals`

---

### Task 3: Reasoning stage indicator (turn-status line)

**Problem:** `SetReasoningStage(Some("tool: read"))` is called by agent events but `apply_command` drops it in the catch-all. The layout has a `turn_status` pane but it's never used.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — `RenderState` (add `reasoning_stage` field), `apply_command` (handle `SetReasoningStage`), `render_frame` (render turn-status if present)

**Approach:**
- Add `pub reasoning_stage: Option<String>` to `RenderState`.
- Handle `InlineCommand::SetReasoningStage(stage) => { state.reasoning_stage = stage; }` in `apply_command`.
- In `render_frame`, if `state.reasoning_stage` is Some, render a 1-row indicator above the composer (using the `turn_status` layout rect). This requires passing `turn_status_height: 1` in `LayoutInput` when reasoning is active, and `0` otherwise.

- [ ] **Step 1:** Add field + apply_command handler.
- [ ] **Step 2:** Update `render_frame` to set `turn_status_height` conditionally and render the indicator.
- [ ] **Step 3:** TestBackend test: set `reasoning_stage`, render, assert text appears.
- [ ] **Step 4:** Commit: `feat(tui): show reasoning/tool stage indicator`

---

### Task 4: Thinking text visual treatment

**Problem:** `StreamDelta::Thinking(text)` is rendered identically to `StreamDelta::Text` — both use `InlineMessageKind::Agent`. Thinking blocks have no visual distinction.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — `map_agent_event`

**Approach:** The simplest no-new-type approach: render thinking deltas with a dimmed/italic prefix. In `map_agent_event`, when the delta is `Thinking`, prefix the segment with a dim style and a `✻ thinking` marker, using `InlineMessageKind::Info` (which renders in a different color). At `MessageEnd`, the markdown render replaces everything with `InlineMessageKind::Agent`, so thinking text naturally disappears from the final output if it was part of the message buffer.

Actually, the cleanest approach: add a `thinking: bool` flag to RenderState. When thinking deltas arrive, accumulate into a separate buffer. At Sync/MessageEnd, clear the thinking display. Render the thinking buffer as a dimmed block above the current agent message.

- [ ] **Step 1:** Add `pub thinking_buffer: String` to RenderState.
- [ ] **Step 2:** In `map_agent_event`, route `StreamDelta::Thinking` to the thinking buffer and display it as `InlineMessageKind::Info` with a dim prefix.
- [ ] **Step 3:** Clear the thinking display when thinking ends (next Text delta or MessageEnd).
- [ ] **Step 4:** TestBackend test: simulate thinking delta, assert dim indicator appears.
- [ ] **Step 5:** Commit: `feat(tui): distinct visual treatment for thinking blocks`

---

## Phase 2: Overlay System

### Task 5: Core overlay rendering + keyboard navigation

**Problem:** `ShowOverlay`/`CloseOverlay` InlineCommands are dropped by `apply_command`'s catch-all. The overlay types (Modal, List, Wizard) exist but are never rendered.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — RenderState, apply_command, render_frame, input thread

**Approach:**
- Add `pub overlay: Option<OverlayState>` to RenderState, where:

```rust
pub struct OverlayState {
    pub title: String,
    pub items: Vec<InlineListItem>,
    pub selected: usize,
    pub search: Option<String>,
    pub footer_hint: Option<String>,
}
```

- Handle `InlineCommand::ShowOverlay { request }` — extract List variant into OverlayState.
- Handle `InlineCommand::CloseOverlay` — clear overlay.
- Add `render_overlay(frame, area, state)` — centered Clear + bordered List with search filter + selection highlight.
- In `render_frame`, render overlay last (on top of everything).
- In the input thread, when overlay is open: intercept Up/Down/Enter/Esc/typing for overlay navigation instead of normal input.

- [ ] **Step 1:** Define `OverlayState` and add to RenderState.
- [ ] **Step 2:** Handle ShowOverlay/CloseOverlay in apply_command.
- [ ] **Step 3:** Write `render_overlay` function.
- [ ] **Step 4:** Wire overlay keyboard navigation in input thread.
- [ ] **Step 5:** Add `OverlayEvent::Submitted` / `OverlayEvent::Cancelled` routing — send as `InlineEvent::Overlay(...)`.
- [ ] **Step 6:** TestBackend tests: overlay renders title, items, selection; keyboard nav moves selection.
- [ ] **Step 7:** Commit: `feat(tui): wire overlay system — rendering + keyboard navigation`

---

### Task 6: Slash commands → overlay pickers (/model, /help)

**Problem:** `/model` only shows text. `/help` dumps a text list. Neither uses the new overlay system.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/registry.rs` — ModelCommand, help dispatch

**Approach:**
- `/model` (no args) → open a list overlay with available models from the session. On selection, set the model.
- `/help` (or `/commands`) → open a list overlay showing all commands with descriptions.

- [ ] **Step 1:** Add `handle.show_list_modal(...)` call in `/model` command when no args.
- [ ] **Step 2:** Add overlay-based `/help` command.
- [ ] **Step 3:** Test: slash dispatch opens overlay state.
- [ ] **Step 4:** Commit: `feat(tui): /model and /help use overlay pickers`

---

## Phase 3: Agent UX

### Task 7: Tool call structured rendering

**Problem:** Tool calls show `→ read` + a 200-char truncated flat preview. No status icons, no structure.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — `map_agent_event` (ToolStart, ToolComplete, ToolError)

**Approach:**
- `ToolStart` → render as `⚙ read` (tool color, with gear glyph instead of arrow).
- `ToolComplete` → render result on the same visual block, dimmed, with a `✓` prefix for success.
- `ToolError` → render with `✗` prefix, error color.
- Increase preview from 200 to 500 chars, and indent continuation lines.

- [ ] **Step 1:** Update ToolStart/ToolComplete/ToolError rendering in map_agent_event.
- [ ] **Step 2:** Update `preview_tool_result` MAX to 500 and add indent for multi-line.
- [ ] **Step 3:** TestBackend test: simulate tool events, assert glyphs appear.
- [ ] **Step 4:** Commit: `feat(tui): structured tool call rendering with status glyphs`

---

### Task 8: Input queue display

**Problem:** `SetQueuedInputs` is dropped by apply_command. When prompts are queued, the user can't see them.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — RenderState, apply_command, render_frame

**Approach:**
- Add `pub queued_inputs: Vec<String>` to RenderState.
- Handle `InlineCommand::SetQueuedInputs { entries }`.
- In `render_frame`, if queued inputs exist, set `queue_height` in LayoutInput and render the queue pane.

- [ ] **Step 1:** Add field + apply_command handler.
- [ ] **Step 2:** Render queue pane in render_frame.
- [ ] **Step 3:** TestBackend test.
- [ ] **Step 4:** Commit: `feat(tui): display queued input prompts`

---

### Task 9: Todo pane + follow-up chips

**Problem:** The layout supports a todo side-pane and follow-up suggestion chips, but neither is wired.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — RenderState, render_frame
- Modify: `oxicode-cli/src/tui_vt/frame_layout.rs` — LayoutInput

**Approach:**
- Todo pane: if `state.todo_items` is non-empty, set `todo_height` and render a compact checklist.
- Follow-up chips: if `state.follow_ups` is non-empty, set `follow_ups_height` and render suggestion chips as `▸ suggestion1  ▸ suggestion2`.

- [ ] **Step 1:** Add `todo_items: Vec<(String, bool)>` and `follow_ups: Vec<String>` to RenderState.
- [ ] **Step 2:** Render todo pane (compact list with ☐/☑ markers).
- [ ] **Step 3:** Render follow-up chips row.
- [ ] **Step 4:** TestBackend tests for both.
- [ ] **Step 5:** Commit: `feat(tui): todo pane + follow-up suggestion chips`

---

## Phase 4: vtui Integration

### Task 10: Vim mode

**Problem:** The vim engine (`oxicode-vtui/src/vim/`) is `pub(crate)` — not accessible from `oxicode-cli`. The input thread has no vim integration.

**Files:**
- Modify: `oxicode-vtui/src/vim/mod.rs` — change `pub(crate) use` to `pub use`
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — input thread, RenderState, render_composer (mode indicator)

**Approach:**
- Make `VimState`, `Editor`, `HandleKeyOutcome`, `handle_key` public in `oxicode-vtui/src/vim/mod.rs`.
- Implement the `Editor` trait for a struct backed by `RenderState`'s input_buffer + input_cursor.
- Add `vim_state: VimState` and `vim_clipboard: String` to RenderState.
- In the input thread, when vim is enabled, route keys through `handle_key` first. If `outcome.handled`, skip normal key processing.
- Handle `InlineCommand::SetVimModeEnabled(bool)` in apply_command → `state.vim_state.set_enabled(enabled)`.
- Show vim mode indicator (INSERT/NORMAL) in the composer or status bar.
- Add `/vim` slash command to toggle.

- [ ] **Step 1:** Make vim API public in oxicode-vtui.
- [ ] **Step 2:** Implement `Editor` trait for input buffer in main_loop.rs.
- [ ] **Step 3:** Wire vim into input thread key dispatch.
- [ ] **Step 4:** Handle SetVimModeEnabled in apply_command.
- [ ] **Step 5:** Render mode indicator in composer.
- [ ] **Step 6:** Add `/vim` slash command.
- [ ] **Step 7:** TestBackend tests.
- [ ] **Step 8:** Commit: `feat(tui): vim mode for prompt editing`

---

### Task 11: WelcomeLayout integration

**Problem:** The CLI renders a hand-rolled welcome banner. The vtui crate has a full `WelcomeLayout` with stacked/hero-box variants.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` — `render_welcome`

**Approach:** Replace the hand-rolled welcome with `WelcomeLayout::compute`. Render a simple ASCII/Unicode logo, a "Type a message to begin" hint, and the version string into the computed areas. The existing prompt composer stays in place (it's rendered separately by render_composer).

- [ ] **Step 1:** Use WelcomeLayout::compute for the scrollback area when transcript is empty.
- [ ] **Step 2:** Render logo text + hint into the computed rects.
- [ ] **Step 3:** TestBackend test.
- [ ] **Step 4:** Commit: `feat(tui): use vtui WelcomeLayout for empty state`

---

### Task 12: Theme accessibility validation

**Problem:** `validate_theme_contrast` and `set_color_accessibility_config` exist in the vtui theme runtime but are never called by the CLI.

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` or `oxicode-cli/src/bootstrap.rs` — call validation on startup

**Approach:**
- On TUI startup (in `run_tui` or `build_agent_session`), call `validate_theme_contrast` for the active theme. If warnings are found, log them as startup warnings (which could feed into the `startup_warnings` layout pane in a future task).
- This is a logging-only change for now — no visual impact unless we wire the warnings pane.

- [ ] **Step 1:** Call `validate_theme_contrast` on startup and `tracing::warn!` any findings.
- [ ] **Step 2:** Commit: `feat(tui): validate theme contrast on startup`

---

## Execution Order

Phases MUST be sequential (each builds on the prior). Within a phase, tasks are sequential when they touch the same function (render_transcript, render_frame, apply_command) and parallelizable otherwise.

```
Phase 1: T1 → T2 → T3 → T4
Phase 2: T5 → T6
Phase 3: T7, T8, T9 (parallelizable after Phase 1)
Phase 4: T10, T11, T12 (parallelizable)
```

**Dependency note:** Phase 3 and 4 tasks that don't touch `render_transcript` (T8, T9, T10, T11, T12) can run in parallel with Phase 1. But to keep the plan simple, execute sequentially unless using subagent-driven development with explicit dependency tracking.
