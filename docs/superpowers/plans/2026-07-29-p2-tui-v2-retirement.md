# P2.1 — V2 Retirement + Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retire the grok-inspired `oxicode-tui` v2 crate, port its only unique feature (cursor dedup) to legacy, and rename `oxicode-tui-legacy` → `oxicode-tui` to establish a single TUI crate.

**Architecture:** v2's `draw_frame_closure` wraps legacy `render::draw` inside a v2 `RenderCtx`. Investigation shows legacy `DiffBackend` already has CSI 2026 sync, DECCARA, and row-level diffing — v2 adds only cursor dedup (~35 lines of `reconcile()` logic). Port that to legacy, switch the render path to `terminal.draw()` + `cursor_state.reconcile()`, then delete v2 and rename.

**Tech Stack:** Rust, ratatui 0.30, crossterm, oxicode-tui-legacy (→ oxicode-tui)

## Global Constraints

- Every task ends with `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all -- --check` green.
- `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` must pass.
- `cargo nextest run --workspace` must pass.
- Render output must be visually identical (cursor blink behavior preserved).
- No functional changes — pure refactoring (v2 retirement + rename).

---

### Task 1: Port CursorState to legacy DiffBackend

**Files:**
- Create: `oxicode-tui-legacy/src/render/cursor.rs`
- Modify: `oxicode-tui-legacy/src/render/mod.rs` (add `mod cursor; pub use cursor::CursorState;`)
- Modify: `oxicode-tui-legacy/src/lib.rs` (re-export `CursorState`)

**Interfaces:**
- Produces: `oxicode_tui_legacy::render::CursorState` — a struct with `new()` and `reconcile(want: Option<Position>, term: &mut Terminal<B>) -> Result<(), B::Error>`.

- [ ] **Step 1: Create `cursor.rs` in legacy**

Create `oxicode-tui-legacy/src/render/cursor.rs` with the following content. This is a direct port of `oxicode-tui/src/pipeline/cursor.rs` (lines 18-65), adapted to live in the legacy render module:

```rust
//! Cursor state with dedup — the core of cursor blink preservation.
//!
//! `reconcile()` is called every frame with the desired cursor state.
//! It emits cursor escape sequences to the terminal ONLY when something
//! actually changed:
//! - Visibility transition (Hide↔Show): emit `Hide`/`Show`
//! - Position change while visible: emit `MoveTo`
//! - Same position while visible: **emit nothing** ← this is the blink fix

use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Position;

#[derive(Debug, Clone, Default)]
pub struct CursorState {
    last_pos: Option<Position>,
    visible: bool,
}

impl CursorState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply this frame's cursor request to the terminal.
    /// Emits zero bytes if nothing changed (same visibility AND same position).
    pub fn reconcile<B: Backend>(
        &mut self,
        want: Option<Position>,
        term: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let new_visible = want.is_some();

        if new_visible != self.visible {
            if new_visible {
                term.show_cursor()?;
                self.visible = true;
            } else {
                term.hide_cursor()?;
                self.visible = false;
                self.last_pos = None;
            }
        }

        if let (Some(new), Some(prev)) = (want, self.last_pos) {
            if new != prev {
                term.set_cursor_position(new)?;
                self.last_pos = Some(new);
            }
        } else if let Some(new) = want {
            term.set_cursor_position(new)?;
            self.last_pos = Some(new);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingBackend {
        commands: std::cell::RefCell<Vec<String>>,
        size: ratatui::layout::Size,
    }

    impl RecordingBackend {
        fn new(w: u16, h: u16) -> Self {
            Self { commands: std::cell::RefCell::new(Vec::new()), size: ratatui::layout::Size { width: w, height: h } }
        }
    }

    impl Backend for RecordingBackend {
        fn draw<'a, I>(&mut self, _content: I) -> std::io::Result<()>
        where I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)> { Ok(()) }
        fn hide_cursor(&mut self) -> std::io::Result<()> { self.commands.borrow_mut().push("hide".into()); Ok(()) }
        fn show_cursor(&mut self) -> std::io::Result<()> { self.commands.borrow_mut().push("show".into()); Ok(()) }
        fn set_cursor_position<P: Into<ratatui::layout::Position>>(&mut self, p: P) -> std::io::Result<()> {
            let pos = p.into(); self.commands.borrow_mut().push(format!("move({},{})", pos.x, pos.y)); Ok(()) }
        fn get_cursor_position(&mut self) -> std::io::Result<ratatui::layout::Position> { Ok(Position { x: 0, y: 0 }) }
        fn clear(&mut self) -> std::io::Result<()> { Ok(()) }
        fn size(&self) -> std::io::Result<ratatui::layout::Size> { Ok(self.size) }
        fn window_size(&mut self) -> std::io::Result<ratatui::backend::WindowSize> {
            Ok(ratatui::backend::WindowSize { columns_rows: ratatui::layout::Size { width: 80, height: 24 }, width_height: ratatui::backend::WindowSizePixelLogic { width: 0, height: 0 } })
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn make_terminal() -> Terminal<RecordingBackend> {
        Terminal::new(RecordingBackend::new(80, 24)).unwrap()
    }

    const P1: Position = Position { x: 5, y: 10 };
    const P2: Position = Position { x: 7, y: 12 };

    #[test]
    fn first_show_emits_show_and_moveto() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        let cmds = term.backend().commands.borrow();
        assert!(cmds.iter().any(|c| c == "show"));
        assert!(cmds.iter().any(|c| c == "move(5,10)"));
    }

    #[test]
    fn same_position_second_frame_emits_zero_bytes() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        let before = term.backend().commands.borrow().len();
        cs.reconcile(Some(P1), &mut term).unwrap();
        let after = term.backend().commands.borrow().len();
        assert_eq!(before, after, "same position should emit zero commands");
    }

    #[test]
    fn hide_emits_hide() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        cs.reconcile(None, &mut term).unwrap();
        assert!(term.backend().commands.borrow().iter().any(|c| c == "hide"));
    }

    #[test]
    fn hide_then_hide_again_emits_zero_bytes() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        cs.reconcile(None, &mut term).unwrap();
        let before = term.backend().commands.borrow().len();
        cs.reconcile(None, &mut term).unwrap();
        let after = term.backend().commands.borrow().len();
        assert_eq!(before, after);
    }

    #[test]
    fn show_after_hide_emits_show_and_moveto() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        cs.reconcile(None, &mut term).unwrap();
        cs.reconcile(Some(P2), &mut term).unwrap();
        let cmds = term.backend().commands.borrow();
        assert!(cmds.iter().any(|c| c == "show"));
        assert!(cmds.iter().any(|c| c == "move(7,12)"));
    }
}
```

- [ ] **Step 2: Register the module in legacy render/mod.rs**

Add after the existing module declarations (around line 24):

```rust
pub mod cursor;
```

And add to the re-exports section:

```rust
pub use cursor::CursorState;
```

- [ ] **Step 3: Re-export from lib.rs**

In `oxicode-tui-legacy/src/lib.rs`, add `CursorState` to the re-exports from the render module. Check the existing `pub use` statements for the render module and add `CursorState` alongside `DiffBackend`.

- [ ] **Step 4: Run build + test**

```bash
cargo build -p oxicode-tui-legacy
cargo nextest run -p oxicode-tui-legacy
```
Expected: PASS. The 5 cursor tests pass. No existing tests broken.

- [ ] **Step 5: Commit**

```bash
git add oxicode-tui-legacy/src/render/cursor.rs oxicode-tui-legacy/src/render/mod.rs oxicode-tui-legacy/src/lib.rs
git commit -m "feat(oxicode-tui-legacy): port CursorState cursor dedup from v2

Port the cursor dedup logic (~35 lines) from oxicode-tui v2's
pipeline/cursor.rs to legacy render module. This is the only feature
v2 provides that legacy DiffBackend lacks — the prerequisite for v2
retirement (P2.1)."
```

---

### Task 2: Migrate oxicode-cli render path to legacy direct + remove all v2 code

**Files:**
- Modify: `oxicode-cli/src/tui/app.rs` — remove v2 imports, remove v2 fields, switch render path
- Modify: `oxicode-cli/src/tui/handlers.rs` — replace `V2MessageRole` with legacy `MessageRole`
- Modify: `oxicode-cli/src/tui/mod.rs` — remove `pub mod v2_render`, `pub mod v2_bridge`, `pub mod v2_overlay_adapter`
- Delete: `oxicode-cli/src/tui/v2_render.rs`
- Delete: `oxicode-cli/src/tui/v2_bridge.rs`
- Delete: `oxicode-cli/src/tui/v2_overlay_adapter.rs`

**Interfaces:**
- Consumes: `oxicode_tui_legacy::CursorState` from Task 1.
- Produces: oxicode-cli no longer imports `oxicode_tui::*` (v2).

- [ ] **Step 1: Switch render path in app.rs**

Replace the entire v2 `draw_frame_closure` block (lines ~1607-1646) with legacy direct rendering. The replacement:

```rust
// Legacy direct render path. DiffBackend already provides CSI 2026 sync,
// DECCARA background fills, and row-level diffing. CursorState provides
// cursor dedup (same position → 0 bytes).
let want_cursor = {
    state.last_input_cursor = None;
    tui.terminal.draw(|frame| {
        render::draw(frame, &mut state, &theme);
    })?;
    state.last_input_cursor
};
state.cursor_state.reconcile(want_cursor, &mut tui.terminal)?;
```

Remove the `use_v2_render` env var check (lines ~1615-1616) — there is only one render path now.

Remove `let v2_theme = v2_theme_from_legacy(&theme);` and `let caps = TerminalCaps::detect();`.

- [ ] **Step 2: Remove v2 imports from app.rs**

Delete these import lines:
```rust
use oxicode_tui::pipeline::CursorState as V2CursorState;
use oxicode_tui::theme::{TerminalCaps, Theme as V2Theme};
```

Add the legacy cursor import:
```rust
use oxicode_tui_legacy::render::CursorState;
```

- [ ] **Step 3: Remove v2 fields from AppState**

In the `AppState` struct definition:
- Delete `pub v2_chat: oxicode_tui::content::ChatLog,` (line ~297)
- Delete `pub v2_chat_view: oxicode_tui::widget::chat::ChatView,` (line ~303)
- Change `pub cursor_state: V2CursorState,` to `pub cursor_state: CursorState,`

In `AppState::new()`:
- Delete `v2_chat: oxicode_tui::content::ChatLog::new(),` (line ~515)
- Delete `v2_chat_view: oxicode_tui::widget::chat::ChatView::new(),` (line ~516)
- Change `cursor_state: V2CursorState::new(),` to `cursor_state: CursorState::new(),`

- [ ] **Step 4: Delete v2_theme_from_legacy function**

Delete the entire `fn v2_theme_from_legacy(...)` function (lines ~53-80 of app.rs).

- [ ] **Step 5: Fix handlers.rs V2MessageRole import**

Replace `use oxicode_tui::content::MessageRole as V2MessageRole;` with the legacy type if it's actually used, or remove if unused. Search for `V2MessageRole` usage in the file. If it's used, map to the equivalent legacy type. If unused, delete the import.

- [ ] **Step 6: Remove v2 module declarations from tui/mod.rs**

Delete these lines from `oxicode-cli/src/tui/mod.rs`:
```rust
pub mod v2_render;
pub mod v2_bridge;
pub mod v2_overlay_adapter;
```

- [ ] **Step 7: Delete v2 bridge files**

```bash
rm oxicode-cli/src/tui/v2_render.rs
rm oxicode-cli/src/tui/v2_bridge.rs
rm oxicode-cli/src/tui/v2_overlay_adapter.rs
```

- [ ] **Step 8: Remove oxicode-tui dependency from oxicode-cli/Cargo.toml**

Delete the line:
```toml
oxicode-tui = { version = "0.60.0", path = "../oxicode-tui" }
```

- [ ] **Step 9: Run build + clippy + test**

```bash
cargo build -p oxicode-cli
cargo clippy -p oxicode-cli --all-targets -- -D warnings
cargo nextest run -p oxicode-cli
```
Expected: PASS. All v2 references resolved. No `oxicode_tui::` imports remain.

If clippy or build fails on remaining v2 references, grep and fix:
```bash
grep -rn 'oxicode_tui[^_]' oxicode-cli/src/
```

- [ ] **Step 10: Commit**

```bash
git add -A oxicode-cli/
git commit -m "refactor(oxicode-cli): remove v2 TUI dependency, switch to legacy direct render

- Replace draw_frame_closure (v2) with terminal.draw() + CursorState.reconcile()
- Remove v2_chat, v2_chat_view dead fields from AppState
- Delete v2_render.rs, v2_bridge.rs, v2_overlay_adapter.rs
- Remove oxicode-tui (v2) dependency from Cargo.toml

DiffBackend already provides CSI 2026 sync, DECCARA, and row-level diffing.
CursorState (ported in previous commit) provides cursor dedup. The v2
pipeline added nothing else. P2.1."
```

---

### Task 3: Delete v2 crate from workspace

**Files:**
- Modify: `Cargo.toml` (workspace root — remove `oxicode-tui` from members)
- Delete: `oxicode-tui/` directory

**Prerequisites:** Task 2 complete (no workspace crate depends on `oxicode-tui` v2).

- [ ] **Step 1: Verify no remaining dependencies**

```bash
grep -r 'oxicode-tui' Cargo.toml oxicode-cli/Cargo.toml oxicode-agent/Cargo.toml oxicode-sdk/Cargo.toml
```
Expected: Only `oxicode-tui-legacy` references. No bare `oxicode-tui` (v2) dependency.

- [ ] **Step 2: Remove oxicode-tui from workspace members**

In root `Cargo.toml`, edit the `members` array to remove `"oxicode-tui"`. Keep `"oxicode-tui-legacy"`.

- [ ] **Step 3: Delete the v2 crate directory**

```bash
rm -rf oxicode-tui/
```

- [ ] **Step 4: Run build to verify**

```bash
cargo build --workspace
```
Expected: PASS. Workspace compiles without `oxicode-tui`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml
git rm -r oxicode-tui/  # if not already removed
git commit -m "chore: delete oxicode-tui v2 crate from workspace

v2 was a grok-inspired clean-room rewrite that added only cursor dedup
over legacy DiffBackend. With cursor dedup ported to legacy and all v2
references removed from oxicode-cli, the crate is dead code. P2.1."
```

---

### Task 4: Rename oxicode-tui-legacy → oxicode-tui

**Files:**
- Rename: `oxicode-tui-legacy/` → `oxicode-tui/`
- Modify: `oxicode-tui-legacy/Cargo.toml` → `oxicode-tui/Cargo.toml` (package name change)
- Modify: `Cargo.toml` (workspace — rename member)
- Modify: `oxicode-cli/Cargo.toml` (dependency name)
- Modify: ALL `.rs` files with `oxicode_tui_legacy` → `oxicode_tui` imports
- Modify: `oxicode-agent/src/tools/ask.rs` if it has any reference
- Modify: `oxicode-cli/tests/pty_e2e.rs` if it references the crate name

- [ ] **Step 1: Rename the directory**

```bash
mv oxicode-tui-legacy/ oxicode-tui/
```

- [ ] **Step 2: Update Cargo.toml package name**

In `oxicode-tui/Cargo.toml`, change:
```toml
[package]
name = "oxicode-tui-legacy"
```
to:
```toml
[package]
name = "oxicode-tui"
```

- [ ] **Step 3: Update workspace Cargo.toml**

In root `Cargo.toml`, change:
```toml
members = [..., "oxicode-tui-legacy", ...]
```
to:
```toml
members = [..., "oxicode-tui", ...]
```

- [ ] **Step 4: Update oxicode-cli/Cargo.toml**

Change:
```toml
oxicode-tui-legacy = { version = "0.60.0", path = "../oxicode-tui-legacy" }
```
to:
```toml
oxicode-tui = { version = "0.60.0", path = "../oxicode-tui" }
```

- [ ] **Step 5: Global import rename**

Replace all occurrences of `oxicode_tui_legacy` with `oxicode_tui` in all `.rs` files across the workspace:

```bash
find . -name '*.rs' -not -path '*/target/*' -not -path '*/.git/*' \
  -exec sed -i '' 's/oxicode_tui_legacy/oxicode_tui/g' {} +
```

This touches ~30 files in oxicode-cli/ and potentially oxicode-agent/.

- [ ] **Step 6: Check for remaining string references**

Search for any remaining `oxicode-tui-legacy` or `oxicode_tui_legacy` references in non-test source:

```bash
grep -rn 'oxicode.tui.legacy' --include='*.rs' --include='*.toml' . | grep -v target | grep -v '.git' | grep -v docs/ | grep -v '.superpowers/'
```

Fix any remaining references manually.

- [ ] **Step 7: Run build + clippy + fmt + test**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace
```
Expected: ALL PASS. Single `oxicode-tui` crate, no `oxicode-tui-legacy` anywhere.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: rename oxicode-tui-legacy → oxicode-tui

With the v2 crate deleted, legacy becomes the sole TUI crate. Rename
completes the single-crate goal. All imports updated:
oxicode_tui_legacy::* → oxicode_tui::*. P2.1 complete."
```

---

### Task 5: Update documentation

**Files:**
- Modify: `AGENTS.md` — update workspace layout, dependency flow, crate references
- Modify: `CHANGELOG.md` — add P2.1 entry
- Modify: `oxicode-tui/src/lib.rs` crate-level docs if they reference "legacy"

- [ ] **Step 1: Update AGENTS.md**

In the Workspace Layout section, remove `oxicode-tui-legacy` as a separate entry. The `oxicode-tui` entry should describe it as the unified TUI crate (legacy-based, omp tape model evolution planned).

Update the dependency flow diagram to show `oxicode-tui` (not `oxicode-tui-legacy`).

Update all references to `oxicode-tui-legacy` throughout the file.

- [ ] **Step 2: Update CHANGELOG.md**

Add entry under the latest version:

```markdown
### Changed
- **TUI**: Retired the grok-inspired `oxicode-tui` v2 crate. Ported cursor dedup
  to legacy `DiffBackend`. Renamed `oxicode-tui-legacy` → `oxicode-tui`. Single TUI
  crate. (P2.1)
```

- [ ] **Step 3: Update crate-level docs**

In `oxicode-tui/src/lib.rs`, update the crate-level doc comment to remove any "legacy" references and describe the crate as the unified TUI.

- [ ] **Step 4: Commit**

```bash
git add AGENTS.md CHANGELOG.md oxicode-tui/src/lib.rs
git commit -m "docs: update for oxicode-tui single-crate rename (P2.1)"
```

---

## Verification Checklist (Final)

- [ ] `cargo build --workspace` — green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — green
- [ ] `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` — green
- [ ] `cargo fmt --all -- --check` — green
- [ ] `cargo nextest run --workspace` — all tests pass
- [ ] No `oxicode-tui-legacy` references remain in source code
- [ ] No `oxicode_tui::` (v2) imports remain in oxicode-cli
- [ ] `oxicode-tui/` directory exists as single TUI crate
- [ ] `oxicode-tui-legacy/` directory does not exist
- [ ] Cursor blink behavior preserved (manual TUI test)

## Risks

- **handlers.rs V2MessageRole**: May be used in a type conversion. Check all usages before removing — map to legacy equivalent.
- **v2_overlay_adapter.rs tests**: The 341-line file has tests. If any test covers logic we want to keep (overlay rendering), extract it before deleting.
- **pty_e2e.rs**: Integration test may reference crate name. Update in Task 4 Step 6.
- **Pre-commit hooks**: `.pre-commit-config.yaml` may reference `oxicode-tui` (v2) in clippy commands. Check and update.
