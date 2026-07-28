# P2.1 — V2 Retirement + Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retire the grok-inspired `oxi-tui` v2 crate, port its only unique feature (cursor dedup) to legacy, and rename `oxi-tui-legacy` → `oxi-tui` to establish a single TUI crate.

**Architecture:** v2's `draw_frame_closure` wraps legacy `render::draw` inside a v2 `RenderCtx`. Investigation shows legacy `DiffBackend` already has CSI 2026 sync, DECCARA, and row-level diffing — v2 adds only cursor dedup (~35 lines of `reconcile()` logic). Port that to legacy, switch the render path to `terminal.draw()` + `cursor_state.reconcile()`, then delete v2 and rename.

**Tech Stack:** Rust, ratatui 0.30, crossterm, oxi-tui-legacy (→ oxi-tui)

## Global Constraints

- Every task ends with `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all -- --check` green.
- `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` must pass.
- `cargo nextest run --workspace` must pass.
- Render output must be visually identical (cursor blink behavior preserved).
- No functional changes — pure refactoring (v2 retirement + rename).

---

### Task 1: Port CursorState to legacy DiffBackend

**Files:**
- Create: `oxi-tui-legacy/src/render/cursor.rs`
- Modify: `oxi-tui-legacy/src/render/mod.rs` (add `mod cursor; pub use cursor::CursorState;`)
- Modify: `oxi-tui-legacy/src/lib.rs` (re-export `CursorState`)

**Interfaces:**
- Produces: `oxi_tui_legacy::render::CursorState` — a struct with `new()` and `reconcile(want: Option<Position>, term: &mut Terminal<B>) -> Result<(), B::Error>`.

- [ ] **Step 1: Create `cursor.rs` in legacy**

Create `oxi-tui-legacy/src/render/cursor.rs` with the following content. This is a direct port of `oxi-tui/src/pipeline/cursor.rs` (lines 18-65), adapted to live in the legacy render module:

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

In `oxi-tui-legacy/src/lib.rs`, add `CursorState` to the re-exports from the render module. Check the existing `pub use` statements for the render module and add `CursorState` alongside `DiffBackend`.

- [ ] **Step 4: Run build + test**

```bash
cargo build -p oxi-tui-legacy
cargo nextest run -p oxi-tui-legacy
```
Expected: PASS. The 5 cursor tests pass. No existing tests broken.

- [ ] **Step 5: Commit**

```bash
git add oxi-tui-legacy/src/render/cursor.rs oxi-tui-legacy/src/render/mod.rs oxi-tui-legacy/src/lib.rs
git commit -m "feat(oxi-tui-legacy): port CursorState cursor dedup from v2

Port the cursor dedup logic (~35 lines) from oxi-tui v2's
pipeline/cursor.rs to legacy render module. This is the only feature
v2 provides that legacy DiffBackend lacks — the prerequisite for v2
retirement (P2.1)."
```

---

### Task 2: Migrate oxi-cli render path to legacy direct + remove all v2 code

**Files:**
- Modify: `oxi-cli/src/tui/app.rs` — remove v2 imports, remove v2 fields, switch render path
- Modify: `oxi-cli/src/tui/handlers.rs` — replace `V2MessageRole` with legacy `MessageRole`
- Modify: `oxi-cli/src/tui/mod.rs` — remove `pub mod v2_render`, `pub mod v2_bridge`, `pub mod v2_overlay_adapter`
- Delete: `oxi-cli/src/tui/v2_render.rs`
- Delete: `oxi-cli/src/tui/v2_bridge.rs`
- Delete: `oxi-cli/src/tui/v2_overlay_adapter.rs`

**Interfaces:**
- Consumes: `oxi_tui_legacy::CursorState` from Task 1.
- Produces: oxi-cli no longer imports `oxi_tui::*` (v2).

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
use oxi_tui::pipeline::CursorState as V2CursorState;
use oxi_tui::theme::{TerminalCaps, Theme as V2Theme};
```

Add the legacy cursor import:
```rust
use oxi_tui_legacy::render::CursorState;
```

- [ ] **Step 3: Remove v2 fields from AppState**

In the `AppState` struct definition:
- Delete `pub v2_chat: oxi_tui::content::ChatLog,` (line ~297)
- Delete `pub v2_chat_view: oxi_tui::widget::chat::ChatView,` (line ~303)
- Change `pub cursor_state: V2CursorState,` to `pub cursor_state: CursorState,`

In `AppState::new()`:
- Delete `v2_chat: oxi_tui::content::ChatLog::new(),` (line ~515)
- Delete `v2_chat_view: oxi_tui::widget::chat::ChatView::new(),` (line ~516)
- Change `cursor_state: V2CursorState::new(),` to `cursor_state: CursorState::new(),`

- [ ] **Step 4: Delete v2_theme_from_legacy function**

Delete the entire `fn v2_theme_from_legacy(...)` function (lines ~53-80 of app.rs).

- [ ] **Step 5: Fix handlers.rs V2MessageRole import**

Replace `use oxi_tui::content::MessageRole as V2MessageRole;` with the legacy type if it's actually used, or remove if unused. Search for `V2MessageRole` usage in the file. If it's used, map to the equivalent legacy type. If unused, delete the import.

- [ ] **Step 6: Remove v2 module declarations from tui/mod.rs**

Delete these lines from `oxi-cli/src/tui/mod.rs`:
```rust
pub mod v2_render;
pub mod v2_bridge;
pub mod v2_overlay_adapter;
```

- [ ] **Step 7: Delete v2 bridge files**

```bash
rm oxi-cli/src/tui/v2_render.rs
rm oxi-cli/src/tui/v2_bridge.rs
rm oxi-cli/src/tui/v2_overlay_adapter.rs
```

- [ ] **Step 8: Remove oxi-tui dependency from oxi-cli/Cargo.toml**

Delete the line:
```toml
oxi-tui = { version = "0.60.0", path = "../oxi-tui" }
```

- [ ] **Step 9: Run build + clippy + test**

```bash
cargo build -p oxi-cli
cargo clippy -p oxi-cli --all-targets -- -D warnings
cargo nextest run -p oxi-cli
```
Expected: PASS. All v2 references resolved. No `oxi_tui::` imports remain.

If clippy or build fails on remaining v2 references, grep and fix:
```bash
grep -rn 'oxi_tui[^_]' oxi-cli/src/
```

- [ ] **Step 10: Commit**

```bash
git add -A oxi-cli/
git commit -m "refactor(oxi-cli): remove v2 TUI dependency, switch to legacy direct render

- Replace draw_frame_closure (v2) with terminal.draw() + CursorState.reconcile()
- Remove v2_chat, v2_chat_view dead fields from AppState
- Delete v2_render.rs, v2_bridge.rs, v2_overlay_adapter.rs
- Remove oxi-tui (v2) dependency from Cargo.toml

DiffBackend already provides CSI 2026 sync, DECCARA, and row-level diffing.
CursorState (ported in previous commit) provides cursor dedup. The v2
pipeline added nothing else. P2.1."
```

---

### Task 3: Delete v2 crate from workspace

**Files:**
- Modify: `Cargo.toml` (workspace root — remove `oxi-tui` from members)
- Delete: `oxi-tui/` directory

**Prerequisites:** Task 2 complete (no workspace crate depends on `oxi-tui` v2).

- [ ] **Step 1: Verify no remaining dependencies**

```bash
grep -r 'oxi-tui' Cargo.toml oxi-cli/Cargo.toml oxi-agent/Cargo.toml oxi-sdk/Cargo.toml
```
Expected: Only `oxi-tui-legacy` references. No bare `oxi-tui` (v2) dependency.

- [ ] **Step 2: Remove oxi-tui from workspace members**

In root `Cargo.toml`, edit the `members` array to remove `"oxi-tui"`. Keep `"oxi-tui-legacy"`.

- [ ] **Step 3: Delete the v2 crate directory**

```bash
rm -rf oxi-tui/
```

- [ ] **Step 4: Run build to verify**

```bash
cargo build --workspace
```
Expected: PASS. Workspace compiles without `oxi-tui`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml
git rm -r oxi-tui/  # if not already removed
git commit -m "chore: delete oxi-tui v2 crate from workspace

v2 was a grok-inspired clean-room rewrite that added only cursor dedup
over legacy DiffBackend. With cursor dedup ported to legacy and all v2
references removed from oxi-cli, the crate is dead code. P2.1."
```

---

### Task 4: Rename oxi-tui-legacy → oxi-tui

**Files:**
- Rename: `oxi-tui-legacy/` → `oxi-tui/`
- Modify: `oxi-tui-legacy/Cargo.toml` → `oxi-tui/Cargo.toml` (package name change)
- Modify: `Cargo.toml` (workspace — rename member)
- Modify: `oxi-cli/Cargo.toml` (dependency name)
- Modify: ALL `.rs` files with `oxi_tui_legacy` → `oxi_tui` imports
- Modify: `oxi-agent/src/tools/ask.rs` if it has any reference
- Modify: `oxi-cli/tests/pty_e2e.rs` if it references the crate name

- [ ] **Step 1: Rename the directory**

```bash
mv oxi-tui-legacy/ oxi-tui/
```

- [ ] **Step 2: Update Cargo.toml package name**

In `oxi-tui/Cargo.toml`, change:
```toml
[package]
name = "oxi-tui-legacy"
```
to:
```toml
[package]
name = "oxi-tui"
```

- [ ] **Step 3: Update workspace Cargo.toml**

In root `Cargo.toml`, change:
```toml
members = [..., "oxi-tui-legacy", ...]
```
to:
```toml
members = [..., "oxi-tui", ...]
```

- [ ] **Step 4: Update oxi-cli/Cargo.toml**

Change:
```toml
oxi-tui-legacy = { version = "0.60.0", path = "../oxi-tui-legacy" }
```
to:
```toml
oxi-tui = { version = "0.60.0", path = "../oxi-tui" }
```

- [ ] **Step 5: Global import rename**

Replace all occurrences of `oxi_tui_legacy` with `oxi_tui` in all `.rs` files across the workspace:

```bash
find . -name '*.rs' -not -path '*/target/*' -not -path '*/.git/*' \
  -exec sed -i '' 's/oxi_tui_legacy/oxi_tui/g' {} +
```

This touches ~30 files in oxi-cli/ and potentially oxi-agent/.

- [ ] **Step 6: Check for remaining string references**

Search for any remaining `oxi-tui-legacy` or `oxi_tui_legacy` references in non-test source:

```bash
grep -rn 'oxi.tui.legacy' --include='*.rs' --include='*.toml' . | grep -v target | grep -v '.git' | grep -v docs/ | grep -v '.superpowers/'
```

Fix any remaining references manually.

- [ ] **Step 7: Run build + clippy + fmt + test**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace
```
Expected: ALL PASS. Single `oxi-tui` crate, no `oxi-tui-legacy` anywhere.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: rename oxi-tui-legacy → oxi-tui

With the v2 crate deleted, legacy becomes the sole TUI crate. Rename
completes the single-crate goal. All imports updated:
oxi_tui_legacy::* → oxi_tui::*. P2.1 complete."
```

---

### Task 5: Update documentation

**Files:**
- Modify: `AGENTS.md` — update workspace layout, dependency flow, crate references
- Modify: `CHANGELOG.md` — add P2.1 entry
- Modify: `oxi-tui/src/lib.rs` crate-level docs if they reference "legacy"

- [ ] **Step 1: Update AGENTS.md**

In the Workspace Layout section, remove `oxi-tui-legacy` as a separate entry. The `oxi-tui` entry should describe it as the unified TUI crate (legacy-based, omp tape model evolution planned).

Update the dependency flow diagram to show `oxi-tui` (not `oxi-tui-legacy`).

Update all references to `oxi-tui-legacy` throughout the file.

- [ ] **Step 2: Update CHANGELOG.md**

Add entry under the latest version:

```markdown
### Changed
- **TUI**: Retired the grok-inspired `oxi-tui` v2 crate. Ported cursor dedup
  to legacy `DiffBackend`. Renamed `oxi-tui-legacy` → `oxi-tui`. Single TUI
  crate. (P2.1)
```

- [ ] **Step 3: Update crate-level docs**

In `oxi-tui/src/lib.rs`, update the crate-level doc comment to remove any "legacy" references and describe the crate as the unified TUI.

- [ ] **Step 4: Commit**

```bash
git add AGENTS.md CHANGELOG.md oxi-tui/src/lib.rs
git commit -m "docs: update for oxi-tui single-crate rename (P2.1)"
```

---

## Verification Checklist (Final)

- [ ] `cargo build --workspace` — green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — green
- [ ] `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` — green
- [ ] `cargo fmt --all -- --check` — green
- [ ] `cargo nextest run --workspace` — all tests pass
- [ ] No `oxi-tui-legacy` references remain in source code
- [ ] No `oxi_tui::` (v2) imports remain in oxi-cli
- [ ] `oxi-tui/` directory exists as single TUI crate
- [ ] `oxi-tui-legacy/` directory does not exist
- [ ] Cursor blink behavior preserved (manual TUI test)

## Risks

- **handlers.rs V2MessageRole**: May be used in a type conversion. Check all usages before removing — map to legacy equivalent.
- **v2_overlay_adapter.rs tests**: The 341-line file has tests. If any test covers logic we want to keep (overlay rendering), extract it before deleting.
- **pty_e2e.rs**: Integration test may reference crate name. Update in Task 4 Step 6.
- **Pre-commit hooks**: `.pre-commit-config.yaml` may reference `oxi-tui` (v2) in clippy commands. Check and update.
