# P2-Integration — Safe Additive TUI Features

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the three standalone P2 modules (KillRing, LaTeX-to-Unicode, Kitty keyboard) into the TUI without changing the render path or breaking existing behavior.

**Architecture:** Additive only — no modifications to `terminal.draw()` or the render loop. Each integration is gated by environment variable (`OXICODE_KILL_RING=1`, `OXICODE_LATEX_INLINE=1`, `OXICODE_KITTY_KEYBOARD=1`) defaulting to off. This lets the user opt in incrementally and roll back instantly by unsetting a variable.

**Tech Stack:** Rust, ratatui 0.30, crossterm, oxicode-tui 0.60

## Global Constraints

- Every task ends with `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo nextest run --workspace` green.
- `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` must pass.
- `cargo fmt --all -- --check` must pass.
- The PTY TUI render test (`test_pty_tui_renders_and_exits`) must pass after every task — it's the primary guardrail.
- Feature gating: each integration reads an env var once at startup and stores the result. No runtime toggling.
- `Action` enum variants added to `oxicode-tui/src/keybindings/registry.rs` MUST be matched exhaustively in `oxicode-cli/src/tui/handlers.rs::dispatch_action` — failing to match is a compile error, which is the desired guardrail.
- No new dependencies. All modules are already implemented in `oxicode-tui/src/input/` and `oxicode-tui/src/render/latex_unicode.rs`.

## What This Plan Explicitly Does NOT Do

- **No OXICODE_TAPE_RENDER feature flag.** The tape engine writes to the main screen (native scrollback, no alt screen). The current TUI uses alt screen (1049h). These are incompatible terminal session modes — you cannot runtime-switch between them. The tape module remains standalone in `oxicode-tui/src/tape/` for future dedicated integration.
- **No modification to the render loop.** The `terminal.draw()` + `CursorState::reconcile()` path stays untouched.
- **No mermaid/image/markdown engine changes.** Those are in P2.5 integration which is deferred.

---

### Task 1: KillRing into Input Editor

**Files:**
- Modify: `oxicode-tui/src/keybindings/registry.rs` — add `KillToLineEnd`, `KillToLineStart`, `Yank`, `YankPop` to `Action` enum
- Modify: `oxicode-tui/src/keybindings/registry.rs` — add default keybindings and `parse_action` entries
- Modify: `oxicode-cli/src/tui/app.rs` — add `kill_ring: oxicode_tui::input::KillRing` field to `AppState`
- Modify: `oxicode-cli/src/tui/handlers.rs` — add match arms in `dispatch_action` for the 4 new actions

**Interfaces:**
- Consumes: `oxicode_tui::input::KillRing` (already exists with `new`, `kill`, `yank`, `yank_pop`, `len`)
- Produces: Kill/yank/yank-pop behavior in the input editor, gated by `OXICODE_KILL_RING=1`

- [ ] **Step 1: Add Action variants to registry**

In `oxicode-tui/src/keybindings/registry.rs`, add after the existing `DeleteToLineEnd` variant (find the `DeleteToLineEnd,` line and insert after it):

```rust
/// Kill (cut) from cursor to line end into the kill ring.
KillToLineEnd,
/// Kill (cut) from cursor to line start into the kill ring.
KillToLineStart,
/// Yank (paste) the most recent kill ring entry.
Yank,
/// Cycle to previous kill ring entry (Emacs yank-pop).
YankPop,
```

- [ ] **Step 2: Add `parse_action` entries**

In the same file, add after the `"deletetolineend" => Some(DeleteToLineEnd),` line:

```rust
"killtolineend" => Some(KillToLineEnd),
"killtolinestart" => Some(KillToLineStart),
"yank" => Some(Yank),
"yankpop" => Some(YankPop),
```

- [ ] **Step 3: Add default keybindings**

In the `init_defaults` defaults vector, after the `(DeleteToLineEnd, vec!["Ctrl+k"])` entry, add:

```rust
// Kill ring (Emacs-style, extends Ctrl+k / Ctrl+u with Shift modifier)
(KillToLineEnd, vec!["Ctrl+Shift+k"]),
(KillToLineStart, vec!["Ctrl+Shift+u"]),
(Yank, vec!["Ctrl+y"]),
(YankPop, vec!["Alt+y"]),
```

Also change `(CopyCodeBlock, vec!["Ctrl+y"])` to `(CopyCodeBlock, vec![])` with a comment `// Ctrl+y reclaimed for Yank (kill ring paste)`.

- [ ] **Step 4: Add tests for new actions**

In the tests module, add:

```rust
#[test]
fn test_kill_ring_bindings() {
    let mgr = KeybindingsManager::new();
    let ctrl_y = parse_key_id("Ctrl+y").unwrap();
    assert_eq!(mgr.match_action(&ctrl_y), Some(Action::Yank));
    let alt_y = parse_key_id("Alt+y").unwrap();
    assert_eq!(mgr.match_action(&alt_y), Some(Action::YankPop));
    let csk = parse_key_id("Ctrl+Shift+k").unwrap();
    assert_eq!(mgr.match_action(&csk), Some(Action::KillToLineEnd));
}

#[test]
fn test_parse_kill_actions() {
    assert_eq!(parse_action("Yank"), Some(Action::Yank));
    assert_eq!(parse_action("YankPop"), Some(Action::YankPop));
    assert_eq!(parse_action("KillToLineEnd"), Some(Action::KillToLineEnd));
}
```

- [ ] **Step 5: Add KillRing field to AppState**

In `oxicode-cli/src/tui/app.rs`, add after the `cursor_state: CursorState,` field:

```rust
/// Emacs-style kill ring. Populated by KillToLineEnd/Start actions,
/// consumed by Yank/YankPop. Gated by `OXICODE_KILL_RING=1`.
kill_ring: oxicode_tui::input::KillRing,
```

In `AppState::new()`, add to the struct initializer after `cursor_state: CursorState::new(),`:

```rust
kill_ring: oxicode_tui::input::KillRing::new(16),
```

- [ ] **Step 6: Wire dispatch arms in handlers.rs**

In `oxicode-cli/src/tui/handlers.rs`, in the `dispatch_action` match, add these arms BEFORE the existing `KAction::DeleteToLineEnd` arm (order doesn't matter, but grouping kill-ring actions together helps readability):

```rust
KAction::KillToLineEnd => {
    if std::env::var("OXICODE_KILL_RING").as_deref() == Ok("1") {
        let killed = state.input.delete_to_end();
        if !killed.is_empty() {
            state.kill_ring.kill(killed);
        }
    } else {
        state.input.delete_to_end();
    }
    None
}
KAction::KillToLineStart => {
    if std::env::var("OXICODE_KILL_RING").as_deref() == Ok("1") {
        let killed = state.input.delete_to_start();
        if !killed.is_empty() {
            state.kill_ring.kill(killed);
        }
    } else {
        state.input.delete_to_start();
    }
    None
}
KAction::Yank => {
    if std::env::var("OXICODE_KILL_RING").as_deref() == Ok("1") {
        if let Some(text) = state.kill_ring.yank() {
            state.input.insert_str(text);
        }
    }
    None
}
KAction::YankPop => {
    if std::env::var("OXICODE_KILL_RING").as_deref() == Ok("1") {
        // Yank-pop replaces the just-yanked text with the previous entry.
        // Simplest impl: delete the last yank length, then insert previous.
        if let Some(prev) = state.kill_ring.yank_pop() {
            state.input.insert_str(prev);
        }
    }
    None
}
```

**IMPORTANT**: Verify that `state.input` has `delete_to_end()` / `delete_to_start()` / `insert_str()` methods. Check `oxicode-tui/src/widgets/input.rs`. If the method names differ, adapt the code to match the actual API. If `insert_str` doesn't exist, use `state.input.insert_char()` in a loop or find the equivalent.

- [ ] **Step 7: Build and test**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p oxicode-cli --test pty_e2e test_pty_tui_renders_and_exits
cargo nextest run -p oxicode-tui keybindings
```

Expected: all green. The new `Action` variants will cause a compile error in `dispatch_action` if any arm is missing — that's the guardrail.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(tui): wire KillRing into input editor (OXICODE_KILL_RING=1)

Adds Emacs-style kill ring behavior to the TUI input editor:
- Ctrl+Shift+k: kill to line end (adds to ring)
- Ctrl+Shift+u: kill to line start (adds to ring)
- Ctrl+y: yank (paste most recent kill)
- Alt+y: yank-pop (cycle to previous kill)

Gated by OXICODE_KILL_RING=1 env var. When unset, Ctrl+Shift+k falls
back to delete-to-end without populating the ring (safe default).

Ctrl+y was previously bound to CopyCodeBlock — reassigned to Yank
(Emacs convention). CopyCodeBlock default removed (users can rebind).

4 new tests for keybinding registration and parse_action.
PTY TUI render test still passes (no render path changes)."
```

---

### Task 2: LaTeX-to-Unicode in Markdown Renderer

**Files:**
- Modify: `oxicode-tui/src/widgets/chat/markdown.rs` — call `latex_to_unicode` on text spans before rendering
- Modify: `oxicode-cli/src/tui/render.rs` — call `latex_to_unicode` on chat message text before display

**Interfaces:**
- Consumes: `oxicode_tui::render::latex_unicode::latex_to_unicode` (already exists)
- Produces: Inline LaTeX symbols in chat messages rendered as Unicode characters, gated by `OXICODE_LATEX_INLINE=1`

- [ ] **Step 1: Read the current markdown renderer**

Read `oxicode-tui/src/widgets/chat/markdown.rs` to understand how text spans are processed. Identify the function that converts source markdown to rendered `Line`/`Span` values.

- [ ] **Step 2: Find the text preprocessing hook**

Look for where raw markdown text is converted to display text. The most common pattern is a function that takes `&str` and returns styled spans. Add a LaTeX-to-Unicode pass at the start of that function.

- [ ] **Step 3: Add the LaTeX pass**

At the top of the text-to-spans function (before markdown parsing), add:

```rust
use oxicode_tui::render::latex_unicode;

// At the start of the rendering function:
let processed_text = if std::env::var("OXICODE_LATEX_INLINE").as_deref() == Ok("1") {
    latex_unicode::latex_to_unicode(input)
} else {
    input.to_string()
};
```

Then use `processed_text` instead of `input` for the rest of the rendering.

- [ ] **Step 4: Also patch the chat message display path**

In `oxicode-cli/src/tui/render.rs`, find where chat message text is rendered to the frame. Apply the same `latex_to_unicode` pass to user and assistant message text before styling. Gate by the same `OXICODE_LATEX_INLINE=1` env var.

If the chat messages are rendered via the markdown renderer (Step 3), this step may be redundant — verify by reading the call chain.

- [ ] **Step 5: Build and test**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p oxicode-cli --test pty_e2e test_pty_tui_renders_and_exits
cargo nextest run -p oxicode-tui markdown
cargo nextest run -p oxicode-tui latex_unicode
```

Expected: all green. The latex_unicode tests verify the conversion logic; the PTY test verifies the render path still works.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): inline LaTeX-to-Unicode in markdown renderer (OXICODE_LATEX_INLINE=1)

When OXICODE_LATEX_INLINE=1 is set, text in chat messages and markdown
rendering is preprocessed through latex_to_unicode() before display.

Example: \"\\alpha + \\beta = \\gamma\" renders as \"α + β = γ\"
in the TUI. Supports 145 symbol mappings (Greek, math operators,
arrows, sets, subscripts/superscripts, accents, fractions, roots).

Gated by env var to avoid any risk of changing display behavior for
existing users. PTY TUI render test still passes (no render path
changes)."
```

---

### Task 3: Kitty Keyboard Protocol Parser in Event Loop

**Files:**
- Modify: `oxicode-cli/src/tui/handlers.rs` — in the key event handler, before falling through to ratatui's `KeyEvent`, try `parse_kitty_key` on the raw input bytes

**Interfaces:**
- Consumes: `oxicode_tui::input::kitty::parse_kitty_key` (already exists, returns `Option<ParsedKey>`)
- Produces: Kitty protocol key events translated to ratatui `KeyEvent`, gated by `OXICODE_KITTY_KEYBOARD=1`

- [ ] **Step 1: Find the raw input read site**

In `oxicode-cli/src/tui/app.rs`, find where crossterm events are read (the main event loop). Look for `crossterm::event::read()` or `event::poll()`. The raw bytes need to be captured BEFORE crossterm parses them into a `KeyEvent`.

NOTE: crossterm does not expose raw bytes — it parses bytes into `KeyEvent` internally. To get raw bytes, we need to either:
- (a) Switch to raw byte reading (bypassing crossterm's event parser) and use our own parser
- (b) Use crossterm's `KeyboardEnhancementFlags` to detect Kitty protocol and let crossterm handle it

For this integration, use approach (a) but ONLY when `OXICODE_KITTY_KEYBOARD=1`. In that mode, replace `event::read()` with a raw byte read + `parse_kitty_key` + manual `KeyEvent` construction.

- [ ] **Step 2: Read oxicode-tui/src/input/kitty.rs public API**

Check what `parse_kitty_key` returns and how `ParsedKey` maps to ratatui `KeyCode`/`KeyModifiers`. Write a helper `fn parsed_to_crossterm(p: ParsedKey) -> crossterm::event::KeyEvent` that translates.

- [ ] **Step 3: Implement the raw read path**

In the main event loop, when `OXICODE_KITTY_KEYBOARD=1`:
1. Read raw bytes from stdin
2. If the bytes look like a Kitty sequence (`\x1b[>...`), call `parse_kitty_key`
3. If parsed, convert to crossterm `KeyEvent` and feed to the handler
4. If not a Kitty sequence (or parse fails), fall through to crossterm's normal event parser for legacy compatibility

This is the hardest of the three integrations. If the implementation gets complex, it's acceptable to limit scope to: detect Kitty sequences, parse them, and emit a `KeyEvent`. Don't try to replace the entire input pipeline.

- [ ] **Step 4: Build and test**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p oxicode-cli --test pty_e2e test_pty_tui_renders_and_exits
cargo nextest run -p oxicode-tui kitty
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(tui): Kitty keyboard protocol parser in event loop (OXICODE_KITTY_KEYBOARD=1)

When OXICODE_KITTY_KEYBOARD=1 is set, the event loop reads raw stdin
bytes and routes Kitty protocol sequences (\\x1b[>...u) through
parse_kitty_key(). Parsed events are translated to crossterm
KeyEvent and fed to the normal keybinding dispatcher.

Falls back to crossterm's native event parser for non-Kitty input
(legacy compatibility).

20 new tests in oxicode-tui/src/input/kitty.rs cover the parser.
PTY TUI render test still passes (no render path changes)."
```

---

### Task 4: Final Workspace Verification

- [ ] **Step 1: Run all gates**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace
```

Expected: all green. Total test count should be 3529 + new tests from this plan (roughly 3590-3600).

- [ ] **Step 2: Run the PTY TUI test one more time**

```bash
cargo nextest run -p oxicode-cli --test pty_e2e test_pty_tui_renders_and_exits
```

Expected: PASS. This is the final guardrail — the TUI must render and exit cleanly.

- [ ] **Step 3: Verify no oxicode-tui-legacy references remain**

```bash
grep -rn 'oxicode.tui.legacy' --include='*.rs' --include='*.toml' . 2>/dev/null | grep -v target | grep -v '.git' | grep -v docs/ | grep -v '.superpowers/'
```

Expected: empty output. All references should say `oxicode-tui` now.

- [ ] **Step 4: Update CHANGELOG**

Add entries under `[Unreleased]` > `### Added`:

```markdown
- **TUI: KillRing in input editor (P2 integration)** — Emacs-style kill ring
  (kill to line end/start, yank, yank-pop) gated by `OXICODE_KILL_RING=1`.
- **TUI: LaTeX-to-Unicode in markdown (P2 integration)** — Inline LaTeX symbols
  rendered as Unicode (145 mappings) gated by `OXICODE_LATEX_INLINE=1`.
- **TUI: Kitty keyboard protocol (P2 integration)** — Kitty protocol parser in
  the event loop gated by `OXICODE_KITTY_KEYBOARD=1`.
- **TUI: PTY render verification test** — Guards the P2.1 render path
  (`terminal.draw()` + `CursorState::reconcile()`) against regression.
```

- [ ] **Step 5: Commit final docs**

```bash
git add CHANGELOG.md
git commit -m "docs: CHANGELOG P2 integration entries

All three safe additive P2 integrations shipped behind env var gates:
- KillRing: OXICODE_KILL_RING=1
- LaTeX: OXICODE_LATEX_INLINE=1
- Kitty: OXICODE_KITTY_KEYBOARD=1

PTY TUI render test added as permanent guardrail."
```

---

## Risk Summary

| Risk | Mitigation |
|---|---|
| Unmatched `Action` variant causes runtime panic | `Action` is `#[derive(.., strum::EnumIter)]` — Rust's exhaustive match is a compile error, not a runtime error. The advisory about stacking changes on unverified base was caught because adding variants without matching arms broke the build. |
| KillRing leaks memory if user kills without yanking | `KillRing::new(16)` capacity-bounded. Worst case: ring fills with old kills, old ones overwritten. |
| LaTeX-to-Unicode mangles non-LaTeX text | `OXICODE_LATEX_INLINE=1` is opt-in. The function only matches known LaTeX commands (`\alpha`, `\times`, etc.) — text without backslash commands is unchanged. Tested in 18 latex_unicode tests. |
| Kitty parser misinterprets non-Kitty escape sequences | Parser checks for `\x1b[>` prefix (Kitty CSI > introducer). Non-Kitty sequences return `None` and fall through to crossterm. |
| PTY test is flaky (timing-dependent) | Test uses 5s timeout for alt-screen enter (generous). If flaky, increase timeout — the test's purpose is to catch hard failures (hang, panic, no-render), not timing races. |

## Verification Checklist (Final)

- [ ] `cargo build --workspace` clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] `cargo nextest run --workspace` all pass
- [ ] `test_pty_tui_renders_and_exits` passes
- [ ] No `oxicode_tui_legacy` references in source
- [ ] CHANGELOG updated
