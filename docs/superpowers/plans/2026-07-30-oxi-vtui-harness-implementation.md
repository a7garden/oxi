# oxi-vtui TUI Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Replace oxi's broken TUI with a working ratatui-based harness using the vendored vtcode-ui (design/theme/protocol).

**Architecture:** oxi-cli → spawn_core_session() → InlineSession (channel) → event loop rendering ratatui widgets against oxi-vtui theme. AgentSession subscribes SessionEvent, maps to InlineCommand. Keyboard events map to InlineEvent.

**Tech Stack:** Rust 2024, ratatui 0.30, crossterm 0.29, oxi-vtui (vendored), oxi-vtui-compat (stubs)

## Global Constraints

- Ship clean: 0 new clippy warnings, 0 test regressions.
- oxi-tui crate and oxi-cli/src/tui/ are deleted only after the new harness works end-to-end.

---

### Task 1: Boot Entry (mod.rs + bootstrap.rs wiring)

**Files:**
- Create: `oxi-cli/src/tui_vt/mod.rs`
- Modify: `oxi-cli/src/bootstrap.rs` (dispatch to new TUI)

**Interfaces:**
- Consumes: `crate::App` from bootstrap, `oxi_vtui::tui::core::*`
- Produces: `pub async fn run_tui(app: App) -> Result<()>` entry point

**Content needed:**
- `fn run_tui(app: App)` — setup terminal, create channels, spawn agent session, run event loop, teardown
- Wire into `dispatch_run_mode` alongside old path (new TUI behind CLI flag `--new-tui` initially, default path after verification)

- [ ] **Step 1: Create tui_vt/mod.rs entry point**

```rust
pub mod host;
pub mod main_loop;

pub use main_loop::run_tui;
```

- [ ] **Step 2: Wire into bootstrap.rs dispatch_run_mode**

Add `--new-tui` flag to `CliArgs`. In `dispatch_run_mode`, route to `tui_vt::run_tui(app).await` when flag is set (default: old path remains until verification).

---

### Task 2: Terminal Lifecycle (RAII + panic safety)

**Files:**
- Modify: `oxi-cli/src/tui_vt/main_loop.rs`

**Content needed:**
- `struct Tui { terminal, tty_ok }` — crossterm setup/teardown
- `Tui::enter()` — enable_raw_mode, EnterAlternateScreen, bracketed paste, mouse tracking
- `Tui::exit()` — restore terminal fully (even on partial failures)
- Panic hook that restores terminal before panic print

- [ ] **Step 1: Implement Tui struct with enter/exit**

```rust
struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    tty_ok: bool,
}
```

- [ ] **Step 2: Wire Teardown: Drop handler restores terminal**

---

### Task 3: Main Event Loop (biased tokio::select!)

**Files:**
- Modify: `oxi-cli/src/tui_vt/main_loop.rs`

**Content needed:**

```rust
loop {
    tokio::select! {
        biased;
        // 1. Agent session events
        Ok(agent_event) = session_rx.recv() => {
            match agent_event { ... /* map to InlineCommand */ }
        }
        // 2. Keyboard events (from spawned thread)
        Some(evt) = inline_rx.recv() => {
            match evt {
                InlineEvent::Submit(text) => agent.submit(text),
                InlineEvent::Interrupt => agent.interrupt(),
                InlineEvent::Cancel => agent.cancel(),
                InlineEvent::Exit => break,
                InlineEvent::ScrollLineUp => scroll_offset = scroll_offset.saturating_sub(1),
                InlineEvent::ScrollLineDown => scroll_offset += 1,
                _ => {}
            }
        }
        // 3. Ctrl+C signal
        _ = tokio::signal::ctrl_c() => { agent.interrupt(); }
    }
    // Render frame
    terminal.draw(|f| render_frame(f, &transcript, &input_text, &scroll_offset))?;
}
```

- [ ] **Step 1: Write the 3-arm biased select loop**
- [ ] **Step 2: Draft render_frame (empty — will fill in Task 4)**

---

### Task 4: AentEvent → InlineCommand Mapping

**Files:**
- Modify: `oxi-cli/src/tui_vt/main_loop.rs`

**Allocation budget:** Streaming tokens MUST NOT allocate per-character. Pre-allocate InlineSegment buffers.

**Mapping table:**

| AgentEvent | InlineCommand | ratatui render effect |
|---|---|---|
| `TokenDelta { delta }` | `Inline(msg, segment)` | Append segment to last assistant message |
| `ToolCall { name, .. }` | `AppendLine(Tool, [name])` + header update | New tool block with spinner |
| `ToolResult { content }` | `AppendLine(Tool, [content])` | Tool output visible |
| `ResponseEnd` | SetInputEnabled(true) + header update | Enable input, clear thinking label |
| `Error { message }` | `AppendLine(Error, [message])` | Red error card |
| `CompactionStart` | SetHeaderContext(stage="Compacting") | Header shows compacting |
| `CompactionEnd` | SetHeaderContext(restore) | Header restored |
| `ThinkingLevelChanged` | SetReasoningStage(text) | Header badge updates |
| `QueueUpdate` | SetQueuedInputs(entries) | Input area shows queue |
| `Advisor { channel, body }` | `AppendLine(Info, [body])` | Info card |

- [ ] **Step 1: Map all AgentEvent variants to InlineCommand calls on handle**

---

### Task 5: Ratatui Rendering (transcript, composer, footer)

**Files:**
- Modify: `oxi-cli/src/tui_vt/main_loop.rs`

**Layout (vertical split):**
```
┌─────────────────────────────────────┐
│ Header (model, branch, status)      │  ← 2 lines
├─────────────────────────────────────┤
│                                     │
│   Transcript (scrollable)           │  ← remainder
│   ┌───────────────────────────────┐ │
│   │ Agent: ...msg...              │ │
│   │ Tool: ⚙ reading file...      │ │
│   │ User: ⚡ large prompt sent    │ │
│   └───────────────────────────────┘ │
├─────────────────────────────────────┤
│ > input text here                   │  ← 1-3 lines (composer)
├─────────────────────────────────────┤
│ Status: Ready  Model: gpt-4o  ↑↓3  │  ← 1 line (footer)
└─────────────────────────────────────┘
```

**Transcript messages:** Each `(InlineMessageKind, Vec<InlineSegment>)` rendered as:
- Agent messages → styled with theme primary/agent style
- Tool messages → styled with theme tool/tool_detail style
- Error messages → red with error prefix
- User messages (submitted) → theme user style

**Scrolling:** scroll_offset tracks viewport. Render from `transcript.len() - scroll_offset - viewport_height`.

**Composer:** Single-line input area with cursor. Prefix "> " colored with theme accent.

**Footer:** Model name, git branch, scroll position.

- [ ] **Step 1: Implement render_frame() with 4-area layout**
- [ ] **Step 2: Wire InlineCommand changes into render state**
- [ ] **Step 3: Test: visible render on terminal**

---

### Task 6: Keyboard Input → InlineEvent Mapping

**Files:**
- Modify: `oxi-cli/src/tui_vt/main_loop.rs`

**Input thread:** Spawned at startup. Polls crossterm::event::poll(50ms) in loop. Sends InlineEvent over channel.

**Mapping:**

| Keyboard input | InlineEvent |
|---|---|
| Enter (no modifier) | `Submit(input_buffer)` + clear buffer |
| Ctrl+C | `Interrupt` |
| Escape | `Cancel` |
| Ctrl+D | `Exit` |
| Ctrl+U | Clear input buffer |
| Char/Backspace | Modify input buffer (no event — just redraw) |
| Up (while input empty) | `ScrollLineUp` |
| Down (while input empty) | `ScrollLineDown` |
| Ctrl+P | `CyclePrimaryAgent` |
| Tab | `CyclePrimaryAgent` |
| Alt+Enter | Submit with newline in buffer |

- [ ] **Step 1: Spawn input polling thread**
- [ ] **Step 2: Implement InputBuffer: text editing, history, suggestions**
- [ ] **Step 3: Wire InlineEvent channel to AgentSession actions**

---

### Task 7: Theme Integration

**Files:**
- Modify: `oxi-cli/src/tui_vt/main_loop.rs`

**Theme activation:** On startup call `oxi_vtui::theme::runtime::set_active_theme(theme_id)`. On `/theme` command, call same. Active styles from `active_styles()`.

**Style references:**
```rust
let style = match kind {
    InlineMessageKind::Agent => styles.agent,
    InlineMessageKind::Tool => styles.tool,
    InlineMessageKind::Error => styles.error,
    InlineMessageKind::User => styles.user,
    InlineMessageKind::Info => styles.info,
    _ => styles.foreground,
};
```

- [ ] **Step 1: Call set_active_theme on startup and /theme command**

---

### Task 8: Cleanup — Remove Old TUI

**Files:**
- Delete: `oxi-cli/src/tui/` directory
- Delete: `oxi-tui/` crate directory
- Modify: `Cargo.toml` workspace members (remove oxi-tui)
- Modify: `oxi-cli/Cargo.toml` (remove oxi-tui dep, make oxi-vtui primary)

- [ ] **Step 1: Remove oxi-tui from workspace members**
- [ ] **Step 2: Remove oxi-cli/src/tui/ and add oxi-tui crate deletion**
- [ ] **Step 3: Update all oxi_tui:: imports in oxi-cli to oxi_vtui::**
- [ ] **Step 4: Full workspace build pass: `cargo check --workspace`**
- [ ] **Step 5: `cargo clippy --workspace -- -D warnings`**
- [ ] **Step 6: `cargo nextest run --workspace`**

---

### Task 9: Verification

**Test scenarios (manual, since TUI requires a terminal):**
1. `cargo run -- --new-tui` — opens terminal, renders empty transcript + prompt
2. Type text + Enter — agent stream starts, tokens appear in transcript
3. Ctrl+C during stream — agent interrupts, status returns to ready
4. Up/Down arrow — scroll transcript
5. Ctrl+P — cycle model (if available)
6. ESC — cancel pending
7. `/theme catppuccin-mocha` — theme switches immediately
8. `cargo run -- --new-tui --continue` — resumes last session
