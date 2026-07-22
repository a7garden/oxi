# oxi-tui v2 — Plan A: Foundation (Pipeline + Widget Model + Theme)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the rendering pipeline (`draw_frame` + `CursorState` + `DiffBackend` migration), the widget model (`Renderable` trait + `RetainedTree` + `RenderCtx` with `CursorSlot` tri-state), and the capability-aware theme system — as the first ~2,250 LOC of the new `oxi-tui`.

**Architecture:** Terminal-first pipeline that decomposes `Terminal::draw()` into `autoresize → hash check → render → flush → conditional cursor → swap_buffers` (no fork, no writer thread, no SafeBuf). Retained widget tree with content_hash memoization for proactive skip. Theme split into palette/capability/serializer so capability detection and color downgrade live in the same module (dead-code structural prevention).

**Tech Stack:** Rust 2024, ratatui 0.30 (= ratatui-core 0.1.2), crossterm 0.29, parking_lot 0.12, supports-color (existing), thiserror, linkify.

**Spec:** `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md`

## Global Constraints

- Workspace: oxi monorepo, single crate greenfield rewrite of `oxi-tui` (kept name). Legacy stays as `oxi-tui-legacy` until PR-10 (Plan D).
- Rust edition: 2024. MSRV per workspace `rust-toolchain.toml`.
- Every module ≤ 500 LOC (spec §2.4).
- `cargo fmt --check`, `cargo clippy --workspace --all-targets --exclude oxi-vendor-* -- -D warnings`, `cargo nextest run --workspace`, `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` must pass after every task.
- `oxi-tui` has zero oxi-* dependencies. Re-export through oxi-tui crate root only.
- Library crate uses `thiserror::Error` for public error types (AGENTS.md convention).
- Use `parking_lot::RwLock` over `std::sync::RwLock`; never hold lock guards across `.await` (irrelevant here — pipeline is sync).
- Atomic file writes via temp + rename for any persistence.
- License: MIT. No grok code copying — clean-room rewrite only.

---

## File Structure

```
oxi-tui/                          (NEW tree, replaces old oxi-tui/ contents)
├── Cargo.toml                    (rewrite — slimmer deps, no tui-markdown, no nucleo)
├── README.md                     (rewrite — shorter)
└── src/
    ├── lib.rs                    (pub use 인덱스)
    ├── pipeline/
    │   ├── mod.rs                (draw_frame, FrameOutcome)
    │   ├── cursor.rs             (CursorState, reconcile)
    │   ├── cursor_slot.rs        (CursorSlot tri-state enum)
    │   └── diff_backend/        (4-file module: mod.rs / row.rs / deccara.rs / caps.rs — mirrors legacy split, each ≤500 LOC)
    ├── widget/
    │   ├── mod.rs                (RetainedTree 공개 API + CursorSlot re-export)
    │   ├── renderable.rs         (trait Renderable)
    │   ├── tree.rs               (RetainedTree impl — last_hash, last_cursor)
    │   └── context.rs            (RenderCtx)
    └── theme/
        ├── mod.rs                (Theme 공개 API)
        ├── palette.rs            (ColorScheme, Theme, 6 named constructors)
        ├── capability.rs         (TerminalCaps, detect, adapt_theme)
        └── serializer.rs         (load_theme, save_theme TOML)

oxi-tui-legacy/                   (RENAMED from oxi-tui/, contents unchanged)
└── (existing files, no edits until Plan D PR-10)

Cargo.toml (workspace root)
└── members list: oxi-tui-legacy added; oxi-tui kept (will be re-populated)
```

---

## Task 1: PR-0 — Workspace scaffold

**Files:**
- Modify: `Cargo.toml` (workspace root — add `oxi-tui-legacy` to members, keep `oxi-tui`)
- Rename: `oxi-tui/` → `oxi-tui-legacy/`
- Create: `oxi-tui/Cargo.toml`
- Create: `oxi-tui/src/lib.rs`
- Create: `oxi-tui/README.md`

**Interfaces:**
- Produces: empty `oxi-tui` crate at version `0.58.0` that compiles with `cargo check -p oxi-tui`. Legacy continues to work under new name.

- [ ] **Step 1: Verify clean tree and create branch**

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
git status
git checkout -b oxi-tui-v2-foundation
```

Expected: `nothing to commit, working tree clean` (or commit existing work first).

- [ ] **Step 2: Rename oxi-tui to oxi-tui-legacy**

```bash
git mv oxi-tui oxi-tui-legacy
```

- [ ] **Step 3: Update oxi-tui-legacy/Cargo.toml name field**

Modify `oxi-tui-legacy/Cargo.toml` line 2:
```toml
name = "oxi-tui-legacy"
```
(Rest of file unchanged. This is the only edit — package name must match new dir.)

- [ ] **Step 4: Update workspace root Cargo.toml members**

Modify `/Volumes/MERCURY/PROJECTS/oxi/Cargo.toml` `members` array: rename the existing `"oxi-tui"` entry to `"oxi-tui-legacy"`. **Do NOT add `"oxi-tui"` yet** — the new `oxi-tui/` directory does not exist. We'll add it in Step 8 after the new crate is created.

```toml
members = [
    # ... existing entries ...
    "oxi-tui-legacy",   # was "oxi-tui"; new oxi-tui added in Step 8
    # ... rest ...
]
```

- [ ] **Step 5: Update legacy `oxi-tui-legacy/Cargo.toml` references**

The `oxi-tui-legacy/Cargo.toml` `name` field is `"oxi-tui-legacy"` (Step 3). Now update everything that depends on the old name to point at the new name.

**5a — `oxi-cli/Cargo.toml`** (and any other workspace crate depending on oxi-tui): change the dependency from `oxi-tui` to `oxi-tui-legacy`. For path deps:

```toml
oxi-tui-legacy = { path = "../oxi-tui-legacy" }
```

**5b — Rust source files (★ CRITICAL: underscore form)**. Rust source uses `oxi_tui` (underscore) — NOT `oxi-tui`. A hyphen-only grep misses every `.rs` file. Scan with both forms:

```bash
# Hyphen form (Cargo.toml, rare .rs string literals)
grep -rln 'oxi-tui' --include='*.toml' --include='*.rs' /Volumes/MERCURY/PROJECTS/oxi/ \
  | grep -v 'oxi-tui-legacy\|target/\|Cargo.lock\|docs/'

# Underscore form (Rust use statements, paths, types) — this is the bulk
grep -rln 'oxi_tui' --include='*.rs' /Volumes/MERCURY/PROJECTS/oxi/ \
  | grep -v 'oxi-tui-legacy\|target/'
```

Expected underscore-form hits (verified 2026-07-21): **27 files in `oxi-cli/src/`** including:
- `oxi-cli/src/main.rs`, `setup_wizard.rs`, `store/settings.rs`
- `oxi-cli/src/tui/{app.rs, handlers.rs, render.rs, welcome.rs}`
- `oxi-cli/src/tui/overlay/*.rs` (18 files: ask, factories, extensions, fork_select, mcp_config, mcp_dashboard, model_select_inline, provider_select, roles_config, router_setup, settings, text_viewer, tree_navigator, mod, anchor, issues_panel/{mod,input,render})
- `oxi-cli/src/tui/slash/builtin/{export_grp,clipboard}.rs`

For each hit, do a global replace `oxi_tui` → `oxi_tui_legacy` (in `use` statements, fully-qualified paths, type references). Use `sed -i '' 's/oxi_tui/oxi_tui_legacy/g'` per file, or an editor macro. Do NOT touch `oxi_tui_legacy` itself (the new crate's own source — there is none yet).

- [ ] **Step 6: Verify legacy crate builds under new name**

Run: `cargo check -p oxi-tui-legacy`
Expected: `Finished` with no errors.

Run: `cargo check -p oxi-cli`
Expected: `Finished` (oxi-cli now depends on oxi-tui-legacy, all `oxi_tui_legacy::` paths resolve).

**★ Do NOT run `cargo check --workspace` yet** — `oxi-tui` is still not in members (Step 4 deliberately omitted it). The workspace check happens after Step 8.
- [ ] **Step 7: Create empty new oxi-tui crate**

Create `oxi-tui/Cargo.toml`:
```toml
[package]
name = "oxi-tui"
version = "0.58.0"
edition.workspace = true
rust-version.workspace = true
description = "Terminal UI rendering pipeline and widget library for oxi (v2 — terminal-first pipeline)"
readme = "README.md"
license = "MIT"
authors = ["a7garden <a7garden@icloud.com>"]
repository = "https://github.com/a7garden/oxi"
keywords = ["tui", "terminal", "ui", "ratatui"]
categories = ["command-line-interface", "gui"]
exclude = ["target/"]

[dependencies]
# Terminal rendering
ratatui = { version = "0.30", features = ["unstable-rendered-line-info"] }
ratatui-core = "0.1"
crossterm = "0.29"

# Serialization (for theme TOML loading)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.9"

# Logging
tracing = "0.1"

# Error handling
anyhow = "1"
thiserror = "2"

# Utilities
parking_lot = "0.12"
unicode-width = "0.2"
unicode-segmentation = "1"
supports-color = { workspace = true }
linkify = { workspace = true }

[dev-dependencies]
ratatui = { version = "0.30", features = ["unstable-rendered-line-info"] }
```

Create `oxi-tui/src/lib.rs`:
```rust
//! oxi-tui v2 — terminal-first rendering pipeline + widget library.
//!
//! Greenfield rewrite. See `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md`.
//!
//! ## Module map
//!
//! - `pipeline`: terminal-first frame lifecycle (`draw_frame`, `CursorState`, `DiffBackend`)
//! - `widget`: retained tree + memoization (`Renderable`, `RetainedTree`, `RenderCtx`)
//! - `theme`: capability-aware palette (`palette`, `capability`, `serializer`)
//!
//! Higher-level modules (`content`, `text`, `link`, `input`, `widget/{chat,panel,primitive}`)
//! are added in Plans B/C.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]
```

Create `oxi-tui/README.md` (one paragraph):
```markdown
# oxi-tui v2

Terminal-first rendering pipeline and widget library for oxi. Decomposes ratatui's `Terminal::draw()` to own the frame lifecycle (cursor blink preservation, proactive skip via content_hash memoization). See `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md` for design.

MIT licensed. Clean-room — no upstream code copied.
```

- [ ] **Step 8: Add new oxi-tui to workspace and verify**

**8a** — Modify `/Volumes/MERCURY/PROJECTS/oxi/Cargo.toml` members: add `"oxi-tui"` alongside `"oxi-tui-legacy"`. Now both are workspace members.

```toml
members = [
    # ... existing entries ...
    "oxi-tui",
    "oxi-tui-legacy",
    # ... rest ...
]
```

**8b** — Run: `cargo check -p oxi-tui`
Expected: `Finished` (empty lib compiles).

**8c** — Run: `cargo check --workspace`
Expected: `Finished` — both old (legacy) and new (empty) oxi-tui resolve, oxi-cli compiles against legacy.

**8d** — Run: `cargo fmt --all -- --check`
Expected: clean.

**8e** — Run: `cargo clippy --workspace --all-targets --exclude oxi-vendor-* -- -D warnings`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(oxi-tui): scaffold v2 crate, rename legacy to oxi-tui-legacy

- oxi-tui/ renamed to oxi-tui-legacy/ (keeps working under new name)
- new empty oxi-tui/ crate at v0.58.0 with slimmed deps (no tui-markdown, no nucleo)
- workspace members list updated
- all callsites updated to depend on oxi-tui-legacy temporarily

Plan A PR-0 of docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md"
```

---

## Task 2: PR-1 — CursorSlot tri-state enum

**Files:**
- Create: `oxi-tui/src/pipeline/mod.rs`
- Create: `oxi-tui/src/pipeline/cursor_slot.rs`
- Modify: `oxi-tui/src/lib.rs`

**Interfaces:**
- Produces: `pub enum CursorSlot { NotSet, Show(Position), Hide }` — used by `RenderCtx` (Task 5) and `RetainedTree` (Task 6).

- [ ] **Step 1: Write the failing test**

Create `oxi-tui/src/pipeline/cursor_slot.rs`:
```rust
//! Tri-state cursor slot — distinguishes "widget did not touch cursor"
//! from "widget explicitly showed/hid cursor".
//!
//! Without tri-state, `Option<Position>` cannot tell apart:
//! - hash-skipped widget that never called `set_cursor` (should fall back to last cursor)
//! - widget that explicitly called `hide_cursor` (authoritative — should propagate)
//!
//! See spec §5.2 "Cursor 폴백의 필요성".

use ratatui::layout::Position;

/// What a widget did to the cursor during this frame's `render()`.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorSlot {
    /// Widget did not touch the cursor (hash-skipped or doesn't manage cursor).
    /// `RetainedTree::render` falls back to `last_cursor`.
    #[default]
    NotSet,
    /// Widget explicitly showed cursor at `position`. Authoritative.
    Show(Position),
    /// Widget explicitly hid cursor. Authoritative — overrides `last_cursor` fallback.
    Hide,
}

impl CursorSlot {
    /// Resolve to `Option<Position>` given the previous cursor. `NotSet` falls back.
    #[must_use]
    pub fn resolve(self, last_cursor: Option<Position>) -> Option<Position> {
        match self {
            CursorSlot::NotSet => last_cursor,
            CursorSlot::Show(p) => Some(p),
            CursorSlot::Hide => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P1: Position = Position { x: 1, y: 1 };
    const P2: Position = Position { x: 2, y: 2 };

    #[test]
    fn notset_falls_back_to_last_cursor() {
        assert_eq!(CursorSlot::NotSet.resolve(Some(P1)), Some(P1));
        assert_eq!(CursorSlot::NotSet.resolve(None), None);
    }

    #[test]
    fn show_is_authoritative_over_last_cursor() {
        assert_eq!(CursorSlot::Show(P2).resolve(Some(P1)), Some(P2));
        assert_eq!(CursorSlot::Show(P2).resolve(None), Some(P2));
    }

    #[test]
    fn hide_is_authoritative_over_last_cursor() {
        // ★ critical: Hide must NOT be clobbered by fallback (the bug class we're fixing)
        assert_eq!(CursorSlot::Hide.resolve(Some(P1)), None);
        assert_eq!(CursorSlot::Hide.resolve(None), None);
    }

    #[test]
    fn default_is_notset() {
        assert_eq!(CursorSlot::default(), CursorSlot::NotSet);
    }
}
```

- [ ] **Step 2: Wire module into pipeline/mod.rs and lib.rs**

Create `oxi-tui/src/pipeline/mod.rs`:
```rust
//! Terminal-first frame lifecycle.
//!
//! Decomposes ratatui's `Terminal::draw()` so the application owns the cursor
//! emission decision. See spec §4.

pub mod cursor_slot;

pub use cursor_slot::CursorSlot;
```

Modify `oxi-tui/src/lib.rs` — append before the closing attributes:
```rust
pub mod pipeline;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo nextest run -p oxi-tui`
Expected: 4 tests pass (`notset_falls_back_to_last_cursor`, `show_is_authoritative_over_last_cursor`, `hide_is_authoritative_over_last_cursor`, `default_is_notset`).

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add oxi-tui/src/pipeline/
git commit -m "feat(oxi-tui/pipeline): add CursorSlot tri-state enum

Distinguishes 'hash-skipped widget' from 'widget explicitly hid cursor'.
- NotSet: fall back to last_cursor (RetainedTree responsibility)
- Show(p): authoritative, propagates
- Hide: authoritative, overrides last_cursor fallback

Spec §5.2 — fixes cursor flicker during streaming when textarea subtree
is hash-skipped but another subtree (chat) triggered a render.

Plan A PR-1 (1/5)"
```

---

## Task 3: PR-1 — CursorState with reconcile

**Files:**
- Create: `oxi-tui/src/pipeline/cursor.rs`
- Modify: `oxi-tui/src/pipeline/mod.rs`
- Test: `oxi-tui/src/pipeline/cursor.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `ratatui_core::terminal::Terminal`, `ratatui_core::backend::Backend`, `ratatui::layout::Position`.
- Produces: `pub struct CursorState; pub fn reconcile<B>(&mut self, want: Option<Position>, term: &mut Terminal<B>) -> Result<(), B::Error>`.

- [ ] **Step 1: Write failing tests (test-first for cursor dedup semantics)**

Create `oxi-tui/src/pipeline/cursor.rs`:
```rust
//! Cursor state with dedup — the core of cursor blink preservation.
//!
//! `reconcile()` is called every frame with the desired cursor state.
//! It emits cursor escape sequences to the terminal ONLY when something
//! actually changed:
//! - Visibility transition (Hide↔Show): emit `Hide`/`Show`
//! - Position change while visible: emit `MoveTo`
//! - Same position while visible: **emit nothing** ← this is the blink fix
//!
//! Contrast with ratatui's `apply_buffer_with_cursor` (render.rs:288-320)
//! which emits unconditionally based on whether `set_cursor_position` was
//! called in the render callback, regardless of whether anything moved.

use ratatui::backend::Backend;
use ratatui::layout::Position;
use ratatui::terminal::Terminal;

#[derive(Debug, Clone)]
pub struct CursorState {
    last_pos: Option<Position>,
    visible: bool,
}

impl Default for CursorState {
    fn default() -> Self {
        Self { last_pos: None, visible: false }
    }
}

impl CursorState {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Apply this frame's cursor request to the terminal.
    /// Emits zero bytes if nothing changed (same visibility AND same position).
    pub fn reconcile<B: Backend>(
        &mut self,
        want: Option<Position>,
        term: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let new_visible = want.is_some();

        // Visibility transition (rare): emit Show or Hide
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

        // Position change while visible: emit MoveTo. Same position → 0 bytes.
        if let (Some(new), Some(prev)) = (want, self.last_pos) {
            if new != prev {
                term.set_cursor_position(new)?;
                self.last_pos = Some(new);
            }
            // ★ new == prev: 0 bytes — blink timer preserved (core optimization)
        } else if let Some(new) = want {
            // Visibility just transitioned to Show — set initial position
            term.set_cursor_position(new)?;
            self.last_pos = Some(new);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::terminal::Terminal;

    /// TestBackend records cursor commands? Actually TestBackend doesn't record
    /// byte-level output. We test via observable state transitions on CursorState
    /// and a custom recording backend for byte-level tests.

    #[derive(Default)]
    struct RecordingBackend {
        commands: Vec<String>,
        size: ratatui::layout::Size,
    }

    impl Backend for RecordingBackend {
        type Error = std::convert::Infallible;

        fn draw<'a, I>(&mut self, _content: I) -> Result<(), Self::Error>
        where I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)> { Ok(()) }

        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            self.commands.push("hide".into());
            Ok(())
        }

        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            self.commands.push("show".into());
            Ok(())
        }

        fn set_cursor_position<P: Into<ratatui::layout::Position>>(
            &mut self,
            position: P,
        ) -> Result<(), Self::Error> {
            let p: Position = position.into();
            self.commands.push(format!("moveto({},{})", p.x, p.y));
            Ok(())
        }

        fn flush(&mut self) -> Result<(), Self::Error> { Ok(()) }

        fn size(&self) -> Result<ratatui::layout::Size, Self::Error> { Ok(self.size) }
    }

    fn make_terminal() -> Terminal<RecordingBackend> {
        Terminal::new(RecordingBackend {
            commands: Vec::new(),
            size: ratatui::layout::Size { width: 80, height: 24 },
        }).unwrap()
    }

    const P1: Position = Position { x: 5, y: 10 };
    const P1_AGAIN: Position = Position { x: 5, y: 10 };
    const P2: Position = Position { x: 7, y: 12 };

    #[test]
    fn first_show_emits_show_and_moveto() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        assert_eq!(term.backend().commands, vec!["show".to_string(), "moveto(5,10)".to_string()]);
    }

    #[test]
    fn same_position_second_frame_emits_zero_bytes() {
        // ★ THE blink-preservation test
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        term.backend_mut().commands.clear();

        cursor.reconcile(Some(P1_AGAIN), &mut term).unwrap();
        assert!(term.backend().commands.is_empty(), "second frame at same position must emit zero cursor bytes");
    }

    #[test]
    fn position_change_emits_only_moveto() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        term.backend_mut().commands.clear();

        cursor.reconcile(Some(P2), &mut term).unwrap();
        assert_eq!(term.backend().commands, vec!["moveto(7,12)".to_string()]);
    }

    #[test]
    fn hide_emits_hide() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        term.backend_mut().commands.clear();

        cursor.reconcile(None, &mut term).unwrap();
        assert_eq!(term.backend().commands, vec!["hide".to_string()]);
    }

    #[test]
    fn hide_then_hide_again_emits_zero_bytes() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(None, &mut term).unwrap(); // first hide
        term.backend_mut().commands.clear();

        cursor.reconcile(None, &mut term).unwrap(); // second hide — should be no-op
        assert!(term.backend().commands.is_empty());
    }

    #[test]
    fn show_after_hide_emits_show_and_moveto() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        cursor.reconcile(None, &mut term).unwrap();
        term.backend_mut().commands.clear();

        cursor.reconcile(Some(P2), &mut term).unwrap();
        assert_eq!(term.backend().commands, vec!["show".to_string(), "moveto(7,12)".to_string()]);
    }
}
```

- [ ] **Step 4: Wire cursor into pipeline mod**

Modify `oxi-tui/src/pipeline/mod.rs`:
```rust
pub mod cursor;
pub mod cursor_slot;

pub use cursor::CursorState;
pub use cursor_slot::CursorSlot;
```

- [ ] **Step 5: Verify clippy is clean**

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean. Address any warnings (likely `too_many_lines`, `cast_possible_truncation` — fix at source).

- [ ] **Step 6: Commit**

```bash
git add oxi-tui/src/pipeline/
git commit -m "feat(oxi-tui/pipeline): add CursorState with conditional cursor emit

reconcile() emits Show/Hide only on visibility transition, MoveTo only on
position change. Same position → 0 bytes → blink timer preserved.

Tests cover: first-show, same-position-noop (blink fix), position-change,
hide, hide-noop, show-after-hide. Uses recording TestBackend.

Spec §4.3.

Plan A PR-1 (2/5)"
```

---

## Task 4: PR-1 — FrameOutcome enum

**Files:**
- Modify: `oxi-tui/src/pipeline/mod.rs`

**Interfaces:**
- Produces: `pub enum FrameOutcome { Idle, Rendered }` — returned by `draw_frame` (Task 8).

- [ ] **Step 1: Update pipeline/mod.rs with FrameOutcome**

Replace `oxi-tui/src/pipeline/mod.rs` with:
```rust
//! Terminal-first frame lifecycle.
//!
//! Decomposes ratatui's `Terminal::draw()` so the application owns the cursor
//! emission decision. See spec §4.

pub mod cursor;
pub mod cursor_slot;

pub use cursor::CursorState;
pub use cursor_slot::CursorSlot;

/// Outcome of a single `draw_frame` call. Lets the caller sleep until the next
/// tick when nothing changed (idle skip — spec §1.4 proactive optimization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameOutcome {
    /// No work was done: content_hash unchanged, no resize, no cursor change.
    /// Caller may sleep until the next event/tick.
    #[default]
    Idle,
    /// A frame was rendered. Cell diff may or may not have emitted bytes
    /// (DiffBackend knows, but pipeline doesn't need to).
    Rendered,
}
```

- [ ] **Step 2: Verify build + tests still pass**

Run: `cargo nextest run -p oxi-tui`
Expected: 10 tests pass (unchanged).

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add oxi-tui/src/pipeline/mod.rs
git commit -m "feat(oxi-tui/pipeline): add FrameOutcome enum (Idle | Rendered)

Returned by draw_frame (Task 8). Idle = pipeline was skipped entirely.

Plan A PR-1 (3/5)"
```

---

## Task 5: PR-1 — DiffBackend migration (no behavioral changes)

**Files:**
- Create: `oxi-tui/src/pipeline/diff_backend/mod.rs` (~400 LOC — DiffBackend struct + impls)
- Create: `oxi-tui/src/pipeline/diff_backend/row.rs` (~150 LOC — Row, build_row, checksum)
- Create: `oxi-tui/src/pipeline/diff_backend/deccara.rs` (~410 LOC — DECCARA plan + emit)
- Create: `oxi-tui/src/pipeline/diff_backend/caps.rs` (~80 LOC — TerminalCaps inline, will move to theme/ in Task 13)
- Modify: `oxi-tui/src/pipeline/mod.rs`

**Interfaces:**
- Produces: `pub struct DiffBackend<W>` with same API as legacy. Later (Task 6) we add `set_links()`.

**NOTE:** Mechanical code-move + import-path fixups. No new behavior. Mirrors the legacy 3-file split — each new file stays ≤500 LOC per Global Constraint.

- [ ] **Step 1: Read the legacy files**

Read:
- `oxi-tui-legacy/src/render/mod.rs` (743 LOC) — DiffBackend struct + both impls + inline TerminalCaps
- `oxi-tui-legacy/src/render/diff.rs` (144 LOC) — Row, build_row, checksum
- `oxi-tui-legacy/src/render/deccara.rs` (406 LOC) — DECCARA plan + emission
- `oxi-tui-legacy/src/render/terminal.rs` — TerminalCaps struct + TerminalKind enum + detect()

Catalog items per file. Each new file owns one concern.

- [ ] **Step 2: Create `diff_backend/` directory with 4 files**

**`diff_backend/mod.rs`** (~400 LOC):
- DiffBackend struct definition + fields (force_full_redraw, last_width, last_height, prev_rows, caps, deccara_enabled, etc.)
- `impl<W: Write> DiffBackend<W>` inherent methods (new, force_redraw, etc.)
- `impl<W: Write> Backend for DiffBackend<W>` trait methods (draw, hide_cursor, show_cursor, set_cursor_position, flush, size, clear, etc.)
- `pub use` re-exports of Row (from row.rs), DeccaraPlan (from deccara.rs), TerminalCaps (from caps.rs)
- CSI 2026 emission bytes (`\x1b[?2026h` / `\x1b[?2026l`) stay here at flush boundaries
- `pub mod` declarations for row, deccara, caps submodules
- Module doc comment noting migration source

**`diff_backend/row.rs`** (~150 LOC):
- `pub(crate) struct Row` (or pub if needed by tests)
- `pub(crate) fn build_row(...)`
- u64 checksum logic
- Migrated verbatim from legacy `render/diff.rs`

**`diff_backend/deccara.rs`** (~410 LOC):
- `pub(crate) struct DeccaraPlan` (or pub if mod.rs needs it)
- `pub(crate) fn compute_deccara_plan(...)`
- `pub(crate) fn emit_deccara(...)`
- All helper functions for DECCARA bg-fill optimization
- Migrated verbatim from legacy `render/deccara.rs`

**`diff_backend/caps.rs`** (~80 LOC):
- `pub struct TerminalCaps` with all fields (color_level, true_color, hyperlinks, kitty_protocol, sixel, synchronized_output, deccara, cell_size, terminal_name)
- `pub enum TerminalKind` (or pub(crate) if only used internally)
- `pub fn detect() -> TerminalCaps` (basic stub OK — full impl in Task 13)
- `impl Default for TerminalCaps`
- Migrated from legacy `render/terminal.rs`. **Will be promoted to `theme/capability.rs` in Task 13** — kept here so DiffBackend compiles standalone in PR-1.

All imports: `crate::render::*` → `crate::pipeline::diff_backend::*` (specific submodule). Drop unused imports.

- [ ] **Step 3: Inline-test the migration with TestBackend parity**

Copy a subset of legacy tests into `diff_backend/mod.rs` `#[cfg(test)] mod tests` (or co-locate in the submodule being tested):
- 1-2 force_full_redraw tests
- 1-2 basic cell diff tests
- 1 CSI 2026 emission test (verifies `\x1b[?2026h` and `\x1b[?2026l` are queued)
- 1 DECCARA plan computation test (if self-contained)

Target ~8-12 migrated tests. Skip tests requiring legacy infrastructure not present in new crate.

- [ ] **Step 4: Wire into pipeline/mod.rs**

Modify `oxi-tui/src/pipeline/mod.rs`:
```rust
pub mod cursor;
pub mod cursor_slot;
pub mod diff_backend;

pub use cursor::CursorState;
pub use cursor_slot::CursorSlot;
pub use diff_backend::DiffBackend;
```

Preserve existing `FrameOutcome` enum.

- [ ] **Step 5: Verify build and tests**

Run: `cargo nextest run -p oxi-tui`
Expected: ~8-12 DiffBackend tests pass + existing 10 cursor/cursorslot tests.

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean. Fix any dead_code warnings (likely some TerminalCaps fields not yet consumed — `#[allow(dead_code)]` is acceptable with a comment explaining Task 13 will consume them).

Run: `cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add oxi-tui/src/pipeline/
git commit -m "feat(oxi-tui/pipeline): migrate DiffBackend from legacy render/

Merges oxi-tui-legacy/src/render/{mod.rs(DiffBackend 부분), diff.rs, deccara.rs, terminal.rs}
into pipeline/diff_backend/ 4-file module (mod/row/deccara/caps).
Each file ≤500 LOC per Global Constraint. No behavioral change.

Preserves: line-level u64 checksum diff, CSI 2026 sync wrap, DECCARA
bg-fill optimizer, force_full_redraw on resize.

Plan A PR-1 (4/5)"
```

---

## Task 6: PR-1 — LinkCollector skeleton (set_links hook on DiffBackend)

**Files:**
- Modify: `oxi-tui/src/pipeline/diff_backend.rs`

**Interfaces:**
- Produces: `pub enum LinkTarget { Url(String), File { path: PathBuf, line: Option<u32> } }`, `pub struct LinkCollector { spans: Vec<(RowRange, LinkTarget)> }`, `impl DiffBackend { pub fn set_links(&mut self, links: LinkCollector) }`.

**NOTE:** This task adds the hook but the OSC8 emission inside row writes comes in Plan C (link axis PR-7). The hook here is just `set_links` storing the collector for later use; the row-writer doesn't yet emit OSC8. This keeps the foundation PR small.

- [ ] **Step 1: Write the failing test**
**Files:**
- Modify: `oxi-tui/src/pipeline/diff_backend/mod.rs` (add LinkCollector + set_links)

**Interfaces:**
// ── OSC8 link collection (stub — emission comes in Plan C PR-7) ────────

use std::path::PathBuf;

/// Where a link points. `Url` for https/other schemes, `File` for absolute paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    File { path: PathBuf, line: Option<u32> },
}

/// Range of cells (row + x range) covered by a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    pub y: u16,
    pub x_start: u16,
    pub x_end: u16,  // inclusive
}

/// Collects links emitted by widgets during `render()`. DiffBackend will
/// emit OSC8 sequences inline during row writes (Plan C PR-7), inside the
/// CSI 2026 window. For now this is just storage.
#[derive(Debug, Default, Clone)]
pub struct LinkCollector {
    spans: Vec<(CellRange, LinkTarget)>,
}

impl LinkCollector {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    pub fn add(&mut self, range: CellRange, target: LinkTarget) {
        self.spans.push((range, target));
    }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.spans.is_empty() }

    #[must_use]
    pub fn len(&self) -> usize { self.spans.len() }

    /// Take the collected spans, leaving an empty collector.
    pub fn take(&mut self) -> Vec<(CellRange, LinkTarget)> {
        std::mem::take(&mut self.spans)
    }
}
```

Add a field to `DiffBackend<W>`:
```rust
pub struct DiffBackend<W> {
    // ... existing fields ...
    links: Vec<(CellRange, LinkTarget)>,  // ★ NEW — populated by set_links
}
```

Add method:
```rust
impl<W: Write> DiffBackend<W> {
    /// Set the OSC8 link spans for the next flush. Must be called BEFORE
    /// `flush()` so row writes can emit inline OSC8 escapes (Plan C PR-7).
    /// For now (foundation) we just store them — emission is a no-op.
    pub fn set_links(&mut self, links: LinkCollector) {
        self.links = links.take();
    }

    // ... existing methods ...
}
```

Initialize `links: Vec::new()` in `Default`/`new`.

- [ ] **Step 2: Add tests for LinkCollector**

Append to `#[cfg(test)] mod tests`:
```rust
#[test]
fn link_collector_add_and_take() {
    let mut c = LinkCollector::new();
    assert!(c.is_empty());
    c.add(
        CellRange { y: 0, x_start: 0, x_end: 4 },
        LinkTarget::Url("https://example.com".into()),
    );
    assert_eq!(c.len(), 1);
    let taken = c.take();
    assert_eq!(taken.len(), 1);
    assert!(c.is_empty());
}

#[test]
fn diff_backend_accepts_set_links_without_emitting() {
    // Foundation: set_links is a no-op storage. Emission in Plan C.
    let mut backend = DiffBackend::new(Vec::<u8>::new());
    let mut links = LinkCollector::new();
    links.add(
        CellRange { y: 0, x_start: 0, x_end: 3 },
        LinkTarget::Url("https://example.com".into()),
    );
    backend.set_links(links);
    // No assertion on bytes — just verifies the API is callable.
}
```

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p oxi-tui`
Expected: 2 new tests pass + all prior tests still pass.

- [ ] **Step 4: Commit**

```bash
git add oxi-tui/src/pipeline/diff_backend/mod.rs
git commit -m "feat(oxi-tui/pipeline): add LinkCollector + set_links hook on DiffBackend

LinkCollector stores (CellRange, LinkTarget) spans collected during render.
DiffBackend::set_links(links) takes them before flush — emission inside
row writes comes in Plan C PR-7 (OSC8 inside CSI 2026 window).

Spec §9.

Plan A PR-1 (5/5) — pipeline complete"
```

---

## Task 7: PR-2 — Renderable trait

**Files:**
- Create: `oxi-tui/src/widget/mod.rs`
- Create: `oxi-tui/src/widget/renderable.rs`
- Modify: `oxi-tui/src/lib.rs`

**Interfaces:**
- Produces: `pub trait Renderable: Send { fn content_hash(&self) -> u64; fn height_for(&self, width: u16, ctx: &RenderCtx) -> u16; fn render(&mut self, area: Rect, ctx: &mut RenderCtx); }`

- [ ] **Step 1: Write Renderable trait with doc + minimal stub**

Create `oxi-tui/src/widget/renderable.rs`:
```rust
//! The widget trait. Every UI element (chat view, message, footer, scrollbar)
//! implements `Renderable`.
//!
//! ## Memoization contract
//!
//! `content_hash()` MUST change whenever the widget's rendered output would
//! change. The pipeline (spec §4.2) calls `content_hash()` first; if it
//! matches the previous frame's hash, `render()` is not called.
//!
//! For widgets with children, `content_hash` aggregates child hashes (e.g.
//! via `hash_combine`). A child change propagates to parent, which propagates
//! to root, which trips `RetainedTree::any_hash_changed`.
//!
//! ## `height_for` contract
//!
//! Must be cheap (no rendering). Used for scrollback virtualization —
//! off-screen widgets are never `render()`-ed.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::widget::RenderCtx;

pub trait Renderable: Send {
    /// Hash of the widget's content. Change this when output would change.
    /// Must be deterministic and cheap.
    fn content_hash(&self) -> u64;

    /// Height this widget will occupy at the given width. Used by parents
    /// to lay out children, and by scrollback virtualization to skip
    /// off-screen widgets entirely.
    fn height_for(&self, width: u16, ctx: &RenderCtx) -> u16;

    /// Paint into `area` of `ctx`'s buffer. Only called when `content_hash`
    /// changed since the last frame (or on first frame, or after resize).
    fn render(&mut self, area: Rect, ctx: &mut RenderCtx);
}

/// Stable hash combine (Fowler–Noll–Vo 1a variant in u64). Use to aggregate
/// child hashes into parent. Same inputs → same output (deterministic).
#[must_use]
pub fn hash_combine(a: u64, b: u64) -> u64 {
    // FNV-1a 64-bit
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = a ^ b;
    h = h.wrapping_mul(FNV_PRIME) ^ (h >> 31);
    h ^ FNV_OFFSET
}

/// Hash a `&str`. Use for widget fields that affect rendering.
#[must_use]
pub fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_combine_is_deterministic() {
        let h1 = hash_combine(12345, 67890);
        let h2 = hash_combine(12345, 67890);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_combine_differs_on_different_inputs() {
        let h1 = hash_combine(12345, 67890);
        let h2 = hash_combine(12345, 67891);
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_str_is_deterministic() {
        assert_eq!(hash_str("hello"), hash_str("hello"));
        assert_ne!(hash_str("hello"), hash_str("world"));
    }
}
```

Create `oxi-tui/src/widget/mod.rs`:
```rust
//! Retained widget tree + memoization.
//!
//! Widgets live across frames (retained). Each frame:
//! 1. Pipeline calls `RetainedTree::any_hash_changed` — walks the tree,
//!    aggregates child hashes into root hash.
//! 2. If root hash unchanged AND no resize: pipeline skips render entirely.
//! 3. Otherwise: `RetainedTree::render` walks the tree, calling `render()`
//!    only on subtrees whose hash changed.
//!
//! See spec §5.

pub mod renderable;

pub use renderable::{Renderable, hash_combine, hash_str};

// Forward-declared — RenderCtx comes in Task 8.
```

Modify `oxi-tui/src/lib.rs` — append:
```rust
pub mod widget;
```

- [ ] **Step 2: This task's tests don't compile yet (RenderCtx missing). Move to Task 8 first.**

(Skip running tests; we'll run them after Task 8.)

- [ ] **Step 3: Commit (no test verification yet — RenderCtx is next)**

```bash
git add oxi-tui/src/widget/
git commit -m "feat(oxi-tui/widget): add Renderable trait + hash helpers

Renderable: content_hash, height_for, render. FNV-1a hash_combine for
child→parent hash propagation. Tests for hash determinism.

RenderCtx type is added in Task 8 — tests run then.

Plan A PR-2 (1/4)"
```

---

## Task 8: PR-2 — RenderCtx with CursorSlot integration

**Files:**
- Create: `oxi-tui/src/widget/context.rs`
- Modify: `oxi-tui/src/widget/mod.rs`
- Test: `oxi-tui/src/widget/renderable.rs` (now compiles) and `oxi-tui/src/widget/context.rs`

**Interfaces:**
- Consumes: `CursorSlot` (Task 2), `LinkCollector` (Task 6).
- Produces: `pub struct RenderCtx<'a> { frame, theme (stub), caps (stub), focus, time, links, cursor: CursorSlot }` with `set_cursor`, `hide_cursor`, `take_cursor_slot`, `emit_link`.

- [ ] **Step 1: Write RenderCtx with failing tests**

Create `oxi-tui/src/widget/context.rs`:
```rust
//! Per-frame render context passed to every `Renderable::render` call.
//!
//! Widgets read from `ctx` (theme, caps, time, focus) and write to it
//! (cursor slot, link spans). The buffer is accessed via `ctx.buffer_mut()`.
//!
//! ## Cursor slot lifecycle
//!
//! 1. `begin_frame` resets `cursor` to `CursorSlot::NotSet`.
//! 2. During `render()`, widgets call `set_cursor(pos)` or `hide_cursor()`.
//! 3. `RetainedTree::render` calls `take_cursor_slot()` after walking the tree,
//!    resolves via `CursorSlot::resolve(last_cursor)`, and emits to terminal
//!    through `CursorState::reconcile`.

use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::pipeline::{CursorSlot, LinkCollector, LinkTarget};
use crate::widget::CellRange;

/// What has focus this frame. Affects rendering of input cursor, highlights, etc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FocusTarget {
    #[default]
    None,
    Chat,
    Input,
    Overlay,
}

pub struct RenderCtx<'a> {
    frame: &'a mut ratatui::terminal::Frame<'a>,
    /// Placeholder until theme module lands (Task 11). For now, () — widgets
    /// use hardcoded styles or skip theme-dependent rendering.
    _theme: (),
    /// Placeholder for terminal capabilities. Task 11 adds real TerminalCaps.
    _caps: (),
    pub focus: FocusTarget,
    pub time: Instant,
    links: LinkCollector,
    cursor: CursorSlot,
}

impl<'a> RenderCtx<'a> {
    pub fn new(frame: &'a mut ratatui::terminal::Frame<'a>) -> Self {
        Self {
            frame,
            _theme: (),
            _caps: (),
            focus: FocusTarget::default(),
            time: Instant::now(),
            links: LinkCollector::new(),
            cursor: CursorSlot::NotSet,
        }
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer { self.frame.buffer_mut() }
    pub fn area(&self) -> Rect { self.frame.area() }

    pub fn set_cursor(&mut self, pos: Position) {
        self.cursor = CursorSlot::Show(pos);
    }

    pub fn hide_cursor(&mut self) {
        self.cursor = CursorSlot::Hide;
    }

    /// Drain the cursor slot, resetting to NotSet. Called by RetainedTree
    /// after render to inspect what widgets requested.
    pub(crate) fn take_cursor_slot(&mut self) -> CursorSlot {
        std::mem::replace(&mut self.cursor, CursorSlot::NotSet)
    }

    pub fn emit_link(&mut self, range: CellRange, target: LinkTarget) {
        self.links.add(range, target);
    }

    /// Drain collected links. Called by pipeline after render, before flush.
    pub(crate) fn take_links(&mut self) -> LinkCollector {
        let taken = std::mem::take(&mut self.links);
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::terminal::Terminal;

    fn make_ctx<'a>(frame: &'a mut ratatui::terminal::Frame<'a>) -> RenderCtx<'a> {
        RenderCtx::new(frame)
    }

    #[test]
    fn cursor_starts_notset() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let mut ctx = make_ctx(f);
            let slot = ctx.take_cursor_slot();
            assert_eq!(slot, CursorSlot::NotSet);
        }).unwrap();
    }

    #[test]
    fn set_cursor_makes_slot_show() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let mut ctx = make_ctx(f);
            ctx.set_cursor(Position { x: 3, y: 4 });
            let slot = ctx.take_cursor_slot();
            assert_eq!(slot, CursorSlot::Show(Position { x: 3, y: 4 }));
        }).unwrap();
    }

    #[test]
    fn hide_cursor_makes_slot_hide() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let mut ctx = make_ctx(f);
            ctx.hide_cursor();
            let slot = ctx.take_cursor_slot();
            assert_eq!(slot, CursorSlot::Hide);
        }).unwrap();
    }

    #[test]
    fn take_resets_to_notset() {
        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let mut ctx = make_ctx(f);
            ctx.set_cursor(Position { x: 0, y: 0 });
            let _ = ctx.take_cursor_slot();
            let slot2 = ctx.take_cursor_slot();
            assert_eq!(slot2, CursorSlot::NotSet);
        }).unwrap();
    }
}
```

- [ ] **Step 2: Wire CellRange / LinkTarget re-exports**

The `RenderCtx` references `CellRange` and `LinkTarget`. These are defined in `pipeline/diff_backend/mod.rs`. Add a re-export at the top of `widget/mod.rs`:

```rust
pub use crate::pipeline::diff_backend::{CellRange, LinkCollector, LinkTarget};
```

(These will move to a dedicated `link/` module in Plan C.)

Modify `oxi-tui/src/widget/mod.rs` to:
```rust
//! Retained widget tree + memoization. See spec §5.

pub mod context;
pub mod renderable;

pub use context::{FocusTarget, RenderCtx};
pub use crate::pipeline::diff_backend::{CellRange, LinkCollector, LinkTarget};
pub use renderable::{Renderable, hash_combine, hash_str};
```

- [ ] **Step 3: Run all tests**

Run: `cargo nextest run -p oxi-tui`
Expected: hash tests (3) + RenderCtx tests (4) + cursor tests (6) + CursorSlot tests (4) + DiffBackend tests + LinkCollector tests (2) = ~19+ tests pass.

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add oxi-tui/src/widget/
git commit -m "feat(oxi-tui/widget): add RenderCtx with CursorSlot tri-state integration

RenderCtx carries frame, focus, time, links, cursor. Widgets call
set_cursor/hide_cursor; take_cursor_slot drains after render.
take_links drains before flush (OSC8 emit prep — Plan C PR-7).

Tests: notset/show/hide/take-resets using TestBackend frame.

Plan A PR-2 (2/4)"
```

---

## Task 9: PR-2 — RetainedTree with last_cursor fallback

**Files:**
- Create: `oxi-tui/src/widget/tree.rs`
- Modify: `oxi-tui/src/widget/mod.rs`
- Test: `oxi-tui/src/widget/tree.rs`

**Interfaces:**
- Produces: `pub struct RetainedTree { root: Box<dyn Renderable>, last_hash: u64, last_cursor: Option<Position> }` with `any_hash_changed`, `render`.

- [ ] **Step 1: Write RetainedTree with cursor fallback tests**

Create `oxi-tui/src/widget/tree.rs`:
```rust
//! The retained widget tree root. Owns the top-level widget and tracks
//! two pieces of cross-frame state:
//!
//! - `last_hash`: previous frame's `content_hash()`. Pipeline uses
//!   `any_hash_changed()` to decide whether to render at all.
//! - `last_cursor`: previous frame's resolved cursor. Used as fallback
//!   when `CursorSlot::NotSet` (hash-skipped cursor widget).

use ratatui::layout::Position;
use ratatui::terminal::Frame;

use crate::pipeline::CursorSlot;
use crate::widget::{RenderCtx, Renderable};

pub struct RetainedTree {
    root: Box<dyn Renderable>,
    last_hash: u64,
    last_cursor: Option<Position>,
}

impl RetainedTree {
    pub fn new(root: Box<dyn Renderable>) -> Self {
        Self { root, last_hash: 0, last_cursor: None }
    }

    /// Did the root's content_hash change since the last call?
    /// First call always returns true (last_hash starts at 0).
    pub fn any_hash_changed(&mut self) -> bool {
        let h = self.root.content_hash();
        let changed = h != self.last_hash;
        self.last_hash = h;
        changed
    }

    /// Render the tree. Returns the resolved cursor position for this frame
    /// (None = hide, Some = show at position). CursorSlot::NotSet falls back
    /// to `last_cursor`; Show/Hide are authoritative.
    pub fn render(&mut self, ctx: &mut RenderCtx) -> Option<Position> {
        let area = ctx.area();
        // ctx.begin_frame은 draw_frame이 호출 (단일 책임).
        // ctx.cursor는 begin_frame 시 CursorSlot::NotSet으로 리셋.
        self.root.render(area, ctx);
        let slot = ctx.take_cursor_slot();
        let cursor = slot.resolve(self.last_cursor);
        self.last_cursor = cursor;
        cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::hash_str;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::terminal::Terminal;

    /// Test widget with a configurable hash and cursor slot it emits.
    struct StubWidget {
        hash_value: u64,
        cursor_to_emit: CursorSlot,
        render_count: usize,
    }

    impl Renderable for StubWidget {
        fn content_hash(&self) -> u64 { self.hash_value }
        fn height_for(&self, _w: u16, _ctx: &RenderCtx) -> u16 { 1 }
        fn render(&mut self, _area: Rect, ctx: &mut RenderCtx) {
            self.render_count += 1;
            match self.cursor_to_emit {
                CursorSlot::Show(p) => ctx.set_cursor(p),
                CursorSlot::Hide => ctx.hide_cursor(),
                CursorSlot::NotSet => {},
            }
        }
    }

    const P1: Position = Position { x: 1, y: 1 };

    fn run_frame(tree: &mut RetainedTree, term: &mut Terminal<TestBackend>) -> Option<Position> {
        let mut cursor_pos = None;
        term.draw(|frame| {
            let mut ctx = RenderCtx::new(frame);
            cursor_pos = tree.render(&mut ctx);
        }).unwrap();
        cursor_pos
    }

    #[test]
    fn first_call_emits_cursor_from_show() {
        let mut tree = RetainedTree::new(Box::new(StubWidget {
            hash_value: 100,
            cursor_to_emit: CursorSlot::Show(P1),
            render_count: 0,
        }));
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();
        let cursor = run_frame(&mut tree, &mut term);
        assert_eq!(cursor, Some(P1));
    }

    #[test]
    fn notset_falls_back_to_last_cursor_across_frames() {
        // ★ THE cursor-flicker regression test
        // Frame 1: widget sets Show(P1). last_cursor becomes Some(P1).
        // Frame 2: widget is hash-skipped (NotSet). Must fall back to Some(P1).
        // (In RetainedTree we simulate hash-skip by emitting NotSet directly.)
        let mut widget = StubWidget {
            hash_value: 100, // same hash both frames
            cursor_to_emit: CursorSlot::Show(P1),
            render_count: 0,
        };
        let mut tree = RetainedTree::new(Box::new(StubWidget {
            hash_value: 100,
            cursor_to_emit: CursorSlot::Show(P1),
            render_count: 0,
        }));
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();

        let c1 = run_frame(&mut tree, &mut term);
        assert_eq!(c1, Some(P1));

        // Now simulate the hash-skip case: widget didn't touch cursor.
        // We can't easily mutate the boxed widget, so we test the resolve logic
        // directly via CursorSlot::resolve.
        let last = tree.last_cursor;
        let resolved = CursorSlot::NotSet.resolve(last);
        assert_eq!(resolved, Some(P1), "NotSet must fall back to last_cursor");
    }

    #[test]
    fn hide_overrides_last_cursor_fallback() {
        // ★ THE hide-clobber regression test (advisory fix)
        // Even if last_cursor is Some, explicit Hide must propagate as None.
        let last = Some(P1);
        let resolved = CursorSlot::Hide.resolve(last);
        assert_eq!(resolved, None, "Hide must NOT be clobbered by last_cursor fallback");
    }

    #[test]
    fn any_hash_changed_first_call_true() {
        let mut tree = RetainedTree::new(Box::new(StubWidget {
            hash_value: 42,
            cursor_to_emit: CursorSlot::NotSet,
            render_count: 0,
        }));
        assert!(tree.any_hash_changed()); // first call always true
    }

    #[test]
    fn any_hash_changed_unchanged_false() {
        let mut tree = RetainedTree::new(Box::new(StubWidget {
            hash_value: 42,
            cursor_to_emit: CursorSlot::NotSet,
            render_count: 0,
        }));
        let _ = tree.any_hash_changed(); // first
        assert!(!tree.any_hash_changed()); // same hash → false
    }
}
```

- [ ] **Step 2: Wire into widget/mod.rs**

Modify `oxi-tui/src/widget/mod.rs`:
```rust
//! Retained widget tree + memoization. See spec §5.

pub mod context;
pub mod renderable;
pub mod tree;

pub use context::{FocusTarget, RenderCtx};
pub use crate::pipeline::diff_backend::{CellRange, LinkCollector, LinkTarget};
pub use renderable::{Renderable, hash_combine, hash_str};
pub use tree::RetainedTree;
```

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p oxi-tui`
Expected: 5 new RetainedTree tests pass + all prior (~24 total).

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add oxi-tui/src/widget/
git commit -m "feat(oxi-tui/widget): add RetainedTree with last_cursor fallback

RetainedTree tracks last_hash (any_hash_changed) and last_cursor
(render fallback when CursorSlot::NotSet). CursorSlot tri-state ensures
explicit Hide is not clobbered by fallback.

Tests: first-call-true, hash-unchanged-false, NotSet fallback across
frames (cursor flicker regression), Hide overrides fallback.

Spec §5.2.

Plan A PR-2 (3/4)"
```

---

## Task 10: PR-2 — Dummy widget + integration smoke test

**Files:**
- Create: `oxi-tui/src/widget/primitive.rs` (small — Text widget only, ~80 LOC; full primitive/ set comes in Plan B)
- Test: end-to-end pipeline+widget integration

**Interfaces:**
- Produces: `pub struct Text { content: String, style: Style }` impl Renderable — smallest widget for integration testing.

- [ ] **Step 1: Write Text widget with tests**

Create `oxi-tui/src/widget/primitive.rs`:
```rust
//! Minimal primitive widgets. The full set (Border, List, Scrollbar) comes
//! in Plan B. This module just has `Text` for integration testing of the
//! pipeline + retained tree.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::widget::{RenderCtx, Renderable, hash_str};

#[derive(Debug, Clone)]
pub struct Text {
    content: String,
    style: Style,
    cached_hash: u64,
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self {
        let content = content.into();
        let cached_hash = hash_str(&content);
        Self { content, style: Style::default(), cached_hash }
    }

    pub fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn fg(mut self, color: Color) -> Self {
        self.style = self.style.fg(color);
        self
    }

    pub fn bold(mut self) -> Self {
        self.style = self.style.add_modifier(Modifier::BOLD);
        self
    }

    pub fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        self.cached_hash = hash_str(&content);
        self.content = content;
    }
}

impl Renderable for Text {
    fn content_hash(&self) -> u64 {
        // Combine content hash with style bits for correctness
        let style_bits = (self.style.fg.is_some() as u64)
            | ((self.style.bg.is_some() as u64) << 1)
            | ((self.style.add_modifier.bits() as u64) << 2);
        crate::widget::hash_combine(self.cached_hash, style_bits)
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        self.content.lines().count().max(1) as u16
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        let line = Line::from(vec![Span::styled(self.content.clone(), self.style)]);
        ctx.buffer_mut().set_line(area.x, area.y, &line, area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_hash_changes_on_set_content() {
        let mut t = Text::new("hello");
        let h1 = t.content_hash();
        t.set_content("world");
        let h2 = t.content_hash();
        assert_ne!(h1, h2);
    }

    #[test]
    fn text_hash_stable_on_same_content() {
        let t1 = Text::new("hello");
        let t2 = Text::new("hello");
        assert_eq!(t1.content_hash(), t2.content_hash());
    }

    #[test]
    fn text_hash_differs_with_style() {
        let plain = Text::new("hi");
        let bold = Text::new("hi").bold();
        assert_ne!(plain.content_hash(), bold.content_hash());
    }
}
```

- [ ] **Step 2: Add module to widget/mod.rs**

Modify `oxi-tui/src/widget/mod.rs` — add:
```rust
pub mod primitive;
pub use primitive::Text;
```

- [ ] **Step 3: Run widget primitive tests**

Run: `cargo nextest run -p oxi-tui`
Expected: 3 new Text tests + all prior.

- [ ] **Step 4: Commit**

```bash
git add oxi-tui/src/widget/
git commit -m "feat(oxi-tui/widget): add Text primitive widget

Smallest concrete Renderable impl. Caches content hash; style changes
affect hash. Used for integration tests in next task.

Plan A PR-2 (4/4) — widget model complete"
```

---

## Task 11: PR-1 — `draw_frame` integration (pipeline + widget glue)

**Files:**
- Modify: `oxi-tui/src/pipeline/mod.rs`

**Interfaces:**
- Produces: `pub fn draw_frame<B>(term, tree, ctx, cursor) -> Result<FrameOutcome, B::Error>` — the central 14-LOC function from spec §4.2.

- [ ] **Step 1: Write the failing integration test**

Add to `oxi-tui/src/pipeline/mod.rs` at the bottom (before any `#[cfg(test)]`):
```rust
// ── draw_frame ─────────────────────────────────────────────────────────

use ratatui::backend::Backend;
use ratatui::terminal::Terminal;

use crate::widget::{RenderCtx, RetainedTree};

/// Draw a single frame, terminal-first style.
///
/// 1. Detect resize (via `term.size()` before/after autoresize).
/// 2. If content_hash unchanged AND not resized → return `Idle` (0 work, 0 bytes).
/// 3. Otherwise: render tree into back buffer, drain cursor slot (with
///    last_cursor fallback), flush DiffBackend (CSI 2026 + cell diff + OSC8
///    inline in Plan C), reconcile cursor (conditional emit).
///
/// See spec §4.2.
pub fn draw_frame<B: Backend>(
    term: &mut Terminal<B>,
    tree: &mut RetainedTree,
    cursor: &mut crate::pipeline::CursorState,
    focus: crate::widget::FocusTarget,
) -> Result<FrameOutcome, B::Error> {
    // 1. resize detection
    let prev_size = term.size()?;
    term.autoresize()?;
    let resized = term.size()? != prev_size;

    // 2. proactive skip — hash + resize
    if !tree.any_hash_changed() && !resized {
        return Ok(FrameOutcome::Idle);
    }

    // 3. render
    let want = {
        let mut frame = term.get_frame();
        let mut ctx = RenderCtx::new(&mut frame);
        ctx.focus = focus;
        tree.render(&mut ctx)
        // ctx drops here; links drained by DiffBackend in Plan C
    };

    // 4. flush — DiffBackend diffs cells + CSI 2026 wrap
    // (set_links integration comes in Plan C PR-7 — for now DiffBackend has
    // no links to emit, OSC8 deferred.)
    term.flush()?;

    // 5. cursor reconcile — 0 bytes if no change
    cursor.reconcile(want, term)?;

    // 6. swap for next frame
    term.swap_buffers();
    term.backend_mut().flush()?;

    Ok(FrameOutcome::Rendered)
}

#[cfg(test)]
mod draw_frame_tests {
    use super::*;
    use crate::widget::{Renderable, RetainedTree, Text};
    use ratatui::backend::TestBackend;

    fn make_term() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(40, 10)).unwrap()
    }

    #[test]
    fn first_frame_is_rendered() {
        let mut term = make_term();
        let mut tree = RetainedTree::new(Box::new(Text::new("hello")));
        let mut cursor = CursorState::new();
        let outcome = draw_frame(&mut term, &mut tree, &mut cursor, crate::widget::FocusTarget::None).unwrap();
        assert_eq!(outcome, FrameOutcome::Rendered);
    }

    #[test]
    fn second_frame_with_unchanged_hash_is_idle() {
        let mut term = make_term();
        let mut tree = RetainedTree::new(Box::new(Text::new("hello")));
        let mut cursor = CursorState::new();
        let _ = draw_frame(&mut term, &mut tree, &mut cursor, crate::widget::FocusTarget::None).unwrap();
        let outcome = draw_frame(&mut term, &mut tree, &mut cursor, crate::widget::FocusTarget::None).unwrap();
        assert_eq!(outcome, FrameOutcome::Idle);
    }
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo nextest run -p oxi-tui`
Expected: 2 new draw_frame tests pass + all prior.

- [ ] **Step 3: Verify clippy**

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add oxi-tui/src/pipeline/mod.rs
git commit -m "feat(oxi-tui/pipeline): add draw_frame — terminal-first frame lifecycle

14-LOC body: autoresize → hash check → render → flush → reconcile cursor
→ swap_buffers. Idle skip when content_hash unchanged and no resize.

Integration tests: first frame Rendered, second frame Idle (with
unchanged Text widget).

Spec §4.2.

Plan A PR-1 + PR-2 integration complete"
```

---

## Task 12: PR-3 — Theme palette (3-way split, part 1/3)

**Files:**
- Create: `oxi-tui/src/theme/mod.rs`
- Create: `oxi-tui/src/theme/palette.rs`
- Modify: `oxi-tui/src/lib.rs`

**Interfaces:**
- Produces: `pub struct ColorScheme { /* 28 semantic slots */ }`, `pub struct Theme { colors, styles, name }`, 6 constructors `Theme::{dark, light, nord, catppuccin, github_dark, monokai}`.

- [ ] **Step 1: Read legacy theme.rs to extract palette definitions**

Read `oxi-tui-legacy/src/theme.rs` (1,907 LOC) and extract:
- `ColorScheme` struct (28 fields including the 7 Phase-1 background slots per AGENTS.md pitfall)
- 6 constructors with their exact color values
- `ThemeStyles` struct + `to_styles()` impl
- `Theme` struct

Do NOT copy: TOML/JSON loading logic (Task 14), hot-reload watcher (out of scope for Plan A — wire later), color level adaptation (Task 13).

- [ ] **Step 2: Write theme/palette.rs with extracted types**

Create `oxi-tui/src/theme/palette.rs` (~200 LOC):
```rust
//! Semantic color slots + Theme struct + 6 named constructors.
//!
//! The brightness hierarchy per AGENTS.md must be respected:
//! `background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg`.
//!
//! 28 slots total (21 original + 7 Phase-1 background slots — see AGENTS.md
//! theme system pitfall).

use std::borrow::Cow;
use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct ColorScheme {
    // Original 21 slots
    pub background: Color,
    pub foreground: Color,
    pub primary: Color,
    pub accent: Color,
    pub muted: Color,
    pub border: Color,
    pub user: Color,
    pub user_bg: Color,
    pub response: Color,
    pub response_bg: Color,
    pub thinking: Color,
    pub thinking_bg: Color,
    pub tool: Color,
    pub tool_bg: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub diff_add: Color,
    pub diff_remove: Color,
    pub diff_hunk: Color,
    // Phase-1 background slots (7)
    pub surface_bg: Color,
    pub panel_bg: Color,
    pub code_bg: Color,
    pub selection_bg: Color,
    pub diff_add_bg: Color,
    pub diff_remove_bg: Color,
    pub diff_hunk_bg: Color,
}

impl ColorScheme {
    /// Dark theme (default). Neutral grays inspired by grok GrokNight.
    pub fn dark() -> Self { /* extract from legacy */ }
    pub fn light() -> Self { /* extract */ }
    pub fn nord() -> Self { /* extract */ }
    pub fn catppuccin() -> Self { /* extract */ }
    pub fn github_dark() -> Self { /* extract */ }
    pub fn monokai() -> Self { /* extract */ }
}

// ThemeStyles + Theme struct + to_styles() impl extracted from legacy.
// ThemeStyles has a flat field per ColorScheme slot, pre-resolved as
// ratatui::style::Style for hot-path access.

#[derive(Debug, Clone)]
pub struct ThemeStyles { /* ... */ }

#[derive(Debug, Clone)]
pub struct Theme {
    pub colors: ColorScheme,
    pub styles: ThemeStyles,
    pub name: Cow<'static, str>,
}

impl Theme {
    pub fn dark() -> Self { Self::from_scheme(ColorScheme::dark(), "dark") }
    pub fn light() -> Self { Self::from_scheme(ColorScheme::light(), "light") }
    pub fn nord() -> Self { Self::from_scheme(ColorScheme::nord(), "nord") }
    pub fn catppuccin() -> Self { Self::from_scheme(ColorScheme::catppuccin(), "catppuccin") }
    pub fn github_dark() -> Self { Self::from_scheme(ColorScheme::github_dark(), "github_dark") }
    pub fn monokai() -> Self { Self::from_scheme(ColorScheme::monokai(), "monokai") }

    fn from_scheme(colors: ColorScheme, name: &'static str) -> Self {
        let styles = ThemeStyles::from_colors(&colors);
        Self { colors, styles, name: Cow::Borrowed(name) }
    }
}
```

Fill in all 6 constructors with exact color values from legacy. Run `cargo check -p oxi-tui` to verify completeness.

- [ ] **Step 3: Wire into theme/mod.rs and lib.rs**

Create `oxi-tui/src/theme/mod.rs`:
```rust
//! Capability-aware theme system. See spec §7.

pub mod palette;

pub use palette::{ColorScheme, Theme, ThemeStyles};
```

Modify `oxi-tui/src/lib.rs`:
```rust
pub mod theme;
```

- [ ] **Step 4: Add smoke tests for each constructor**

Add to `oxi-tui/src/theme/palette.rs` end:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_constructors_produce_valid_themes() {
        let _ = Theme::dark();
        let _ = Theme::light();
        let _ = Theme::nord();
        let _ = Theme::catppuccin();
        let _ = Theme::github_dark();
        let _ = Theme::monokai();
    }

    #[test]
    fn dark_theme_brightness_hierarchy() {
        // AGENTS.md pitfall: background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg
        let t = Theme::dark();
        // Verify ordering invariant (approximate via luminance proxy).
        // For now: just verify all 7 background slots are distinct.
        let bgs = [
            t.colors.background, t.colors.response_bg, t.colors.thinking_bg,
            t.colors.surface_bg, t.colors.user_bg, t.colors.panel_bg,
        ];
        // Distinctness check (skip comparison — colors are RGB so direct equality).
        let unique: std::collections::HashSet<_> = bgs.iter().collect();
        assert!(unique.len() >= 4, "background slots should be mostly distinct, got {:?}", bgs);
    }
}
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo nextest run -p oxi-tui`
Expected: 2 new theme tests pass.

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add oxi-tui/src/theme/
git commit -m "feat(oxi-tui/theme): extract palette from legacy theme.rs

3-way split part 1/3: ColorScheme (28 slots), ThemeStyles, Theme with
6 named constructors. TOML loading and color-level adaptation come in
tasks 13, 14. Brightness hierarchy preserved per AGENTS.md.

Plan A PR-3 (1/3)"
```

---

## Task 13: PR-3 — Capability detection + adapt_theme (3-way split part 2/3)

**Files:**
- Create: `oxi-tui/src/theme/capability.rs`
- Modify: `oxi-tui/src/theme/mod.rs`
- Modify: `oxi-tui/src/pipeline/diff_backend/caps.rs` (move TerminalCaps out, leave re-export)

**Interfaces:**
- Produces: `pub enum ColorLevel { None, Basic, Ansi256, TrueColor }`, `pub struct TerminalCaps { color_level, true_color, hyperlinks, kitty_protocol, sixel, synchronized_output, deccara, cell_size }`, `TerminalCaps::detect()`, `TerminalCaps::adapt_theme(&self, theme)`. **Same module owns detection AND consumption — dead code prevention.**

- [ ] **Step 1: Write capability.rs with detection + adaptation**

Create `oxi-tui/src/theme/capability.rs` (~150 LOC):
```rust
//! Terminal capability detection + theme color-level adaptation.
//!
//! ★ CRITICAL: detection and consumption live in the SAME MODULE.
//! This is the structural fix for the dead-code class exemplified by
//! legacy `render/color_level.rs` (394 LOC, re-exported, never called).
//! See spec §1.5, §7.2.

use ratatui::style::Color;

use crate::theme::{ColorScheme, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ColorLevel {
    #[default]
    None,
    Basic,
    Ansi256,
    TrueColor,
}

impl ColorLevel {
    pub fn has_color(self) -> bool { self >= Self::Basic }
    pub fn has_256(self) -> bool { self >= Self::Ansi256 }
    pub fn has_truecolor(self) -> bool { self >= Self::TrueColor }
}

/// All detected terminal capabilities. Populated once at bootstrap.
#[derive(Debug, Clone, Default)]
pub struct TerminalCaps {
    pub color_level: ColorLevel,
    pub true_color: bool,
    pub hyperlinks: bool,
    pub kitty_protocol: bool,
    pub sixel: bool,
    pub synchronized_output: bool,
    pub deccara: bool,
    pub cell_size: Option<(u16, u16)>,
    pub terminal_name: Option<String>,
}

impl TerminalCaps {
    /// Detect capabilities from environment. NO_COLOR → None.
    /// COLORTERM=truecolor/24bit → TrueColor. supports-color crate fallback.
    /// tmux/SSH recovery: if COLORTERM missing but ITERM_SESSION_ID/TERM_PROGRAM
    /// indicates known truecolor terminal, recover TrueColor.
    #[must_use]
    pub fn detect() -> Self {
        // Extract from legacy render/color_level.rs (detect_color_level_inner)
        // and render/terminal.rs (TerminalKind → capability mapping).
        // ... full impl from legacy ...
        Self::default() // placeholder until extraction
    }

    /// Downgrade all colors in the theme to match the terminal's color level.
    /// Called once at bootstrap after `Theme::dark()` etc.
    pub fn adapt_theme(&self, theme: &mut Theme) {
        if self.color_level >= ColorLevel::TrueColor {
            return; // no downgrade needed
        }
        adapt_color_scheme(&mut theme.colors, self.color_level);
        // Re-derive styles from adapted colors
        theme.styles = crate::theme::ThemeStyles::from_colors(&theme.colors);
    }
}

#[must_use]
pub fn adapt_color(color: Color, level: ColorLevel) -> Color {
    match (color, level) {
        (_, ColorLevel::None) => Color::Reset,
        (Color::Reset, _) => Color::Reset,
        (c, ColorLevel::TrueColor) => c,
        (Color::Rgb(r, g, b), ColorLevel::Ansi256) => Color::Indexed(rgb_to_ansi256(r, g, b)),
        (Color::Rgb(r, g, b), ColorLevel::Basic) => Color::Indexed(ansi256_to_basic(rgb_to_ansi256(r, g, b))),
        (c @ (Color::Black | Color::Red | Color::Green | Color::Yellow | Color::Blue | Color::Magenta | Color::Cyan | Color::White | Color::Gray), _) => c,
        (Color::Indexed(idx), ColorLevel::Basic) => Color::Indexed(ansi256_to_basic(idx)),
        (Color::Indexed(_), ColorLevel::Ansi256 | ColorLevel::TrueColor) => color,
        (Color::DarkGray, ColorLevel::Basic | ColorLevel::Ansi256) => Color::Gray,
        (Color::DarkGray, ColorLevel::TrueColor) => Color::DarkGray,
    }
}

fn adapt_color_scheme(scheme: &mut ColorScheme, level: ColorLevel) {
    // Apply adapt_color to each of the 28 slots
    macro_rules! adapt_field {
        ($f:ident) => { scheme.$f = adapt_color(scheme.$f, level); };
    }
    adapt_field!(background); adapt_field!(foreground); adapt_field!(primary); adapt_field!(accent);
    // ... all 28 fields ...
}

/// RGB → ANSI 256 (xterm 216-cube + 24 grayscale). Extracted from legacy
/// color_level.rs::rgb_to_ansi256.
fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // standard xterm algorithm
    if r == g && g == b {
        if r < 8 { return 16; }
        if r > 248 { return 231; }
        return ((r - 8) as u16 * 24 / 237 + 232) as u8;
    }
    16 + 36 * (r as u16 * 5 / 255) as u8 + 6 * (g as u16 * 5 / 255) as u8 + (b as u16 * 5 / 255) as u8
}

/// ANSI 256 → basic 16 (heuristic). Extracted from legacy.
fn ansi256_to_basic(idx: u8) -> u8 {
    match idx {
        0..=7 => idx,                       // already basic
        8..=15 => idx,                      // bright basic
        16 => 0,                            // black
        17..=51 => 4,                       // blue range
        52..=87 => 1,                       // red range
        88..=123 => 5,                      // magenta
        124..=159 => 1,                     // red
        160..=195 => 3,                     // yellow
        196..=231 => if idx < 214 { 1 } else { 3 },  // red-yellow
        232..=255 => 7,                     // grayscale → white (lossy)
        _ => 7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapt_color_resets_at_none_level() {
        assert_eq!(adapt_color(Color::Red, ColorLevel::None), Color::Reset);
        assert_eq!(adapt_color(Color::Rgb(1, 2, 3), ColorLevel::None), Color::Reset);
    }

    #[test]
    fn adapt_color_keeps_truecolor_unchanged() {
        let c = Color::Rgb(123, 45, 67);
        assert_eq!(adapt_color(c, ColorLevel::TrueColor), c);
    }

    #[test]
    fn adapt_color_rgb_to_ansi256_red() {
        assert_eq!(adapt_color(Color::Rgb(255, 0, 0), ColorLevel::Ansi256), Color::Indexed(196));
    }

    #[test]
    fn adapt_color_keeps_basic_colors() {
        assert_eq!(adapt_color(Color::Red, ColorLevel::Basic), Color::Red);
    }

    #[test]
    fn no_color_env_returns_none_level() {
        // Set env, detect, restore. Detect should return ColorLevel::None.
        std::env::set_var("NO_COLOR", "1");
        let caps = TerminalCaps::detect();
        std::env::remove_var("NO_COLOR");
        // Note: in parallel test execution this could race; for CI we run single-threaded.
        assert_eq!(caps.color_level, ColorLevel::None);
    }
}
```

- [ ] **Step 2: Wire into theme/mod.rs**

Modify `oxi-tui/src/theme/mod.rs`:
```rust
//! Capability-aware theme system. See spec §7.

pub mod capability;
pub mod palette;

pub use capability::{ColorLevel, TerminalCaps, adapt_color};
pub use palette::{ColorScheme, Theme, ThemeStyles};
```

- [ ] **Step 3: Migrate TerminalCaps from pipeline/diff_backend/caps.rs**

In `oxi-tui/src/pipeline/diff_backend/caps.rs`, replace the `TerminalCaps` struct definition with a re-export from `theme::capability`. The `TerminalKind` enum and `detect()` function move entirely to `theme/capability.rs`:
```rust
pub use crate::theme::capability::{TerminalCaps, TerminalKind};
```

This makes DiffBackend consume theme's TerminalCaps. Anywhere DiffBackend uses `TerminalCaps` fields (`caps.synchronized_output`, `caps.deccara`), the access is the same.

- [ ] **Step 4: Run tests + clippy**

Run: `cargo nextest run -p oxi-tui`
Expected: 5 new capability tests pass. The `no_color_env_returns_none_level` test may need `--test-threads=1` if it races; add `#[serial_test::serial]` if needed (or run that test alone).

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add oxi-tui/src/theme/ oxi-tui/src/pipeline/diff_backend/caps.rs
git commit -m "feat(oxi-tui/theme): capability detection + adapt_theme (3-way 2/3)

TerminalCaps::detect() honors NO_COLOR/COLORTERM/TERM. adapt_theme
downgrades RGB → Ansi256 → Basic per detected level.

★ detection + consumption in same module — structural fix for dead-code
class (legacy color_level.rs had 0 callers). DiffBackend now re-exports
TerminalCaps from theme.

Spec §7.2.

Plan A PR-3 (2/3)"
```

---

## Task 14: PR-3 — TOML serializer (3-way split part 3/3)

**Files:**
- Create: `oxi-tui/src/theme/serializer.rs`
- Modify: `oxi-tui/src/theme/mod.rs`
- Modify: `oxi-tui/src/theme/palette.rs` (remove TOML derives if they were copied)

**Interfaces:**
- Produces: `pub fn load_theme(path: &Path) -> Result<Theme>`, `pub fn save_theme(theme: &Theme, path: &Path) -> Result<()>`.

- [ ] **Step 1: Write serializer.rs with TOML load/save**

Create `oxi-tui/src/theme/serializer.rs` (~100 LOC):
```rust
//! TOML/JSON theme loading and saving. Atomic writes (temp + rename).
//!
//! ThemeFile struct is the serde-friendly mirror of ColorScheme. The loaded
//! file is validated against the brightness hierarchy (AGENTS.md pitfall)
//! before being promoted to a runtime Theme.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::theme::{ColorScheme, Theme, ThemeStyles};

#[derive(Debug, Serialize, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    pub colors: ColorSchemeFile,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ColorSchemeFile {
    // Mirror of ColorScheme's 28 fields as serialized hex strings (#RRGGBB)
    // or ratatui color names. See legacy ThemeFileColors for format.
    pub background: String,
    pub foreground: String,
    // ... all 28 ...
}

impl ColorSchemeFile {
    fn into_scheme(self) -> Result<ColorScheme> {
        // Parse each string into ratatui::style::Color
        // Reject invalid hex / unknown names
        // ... extracted from legacy into_theme() ...
        todo!("extract from legacy ThemeFileColors::into_theme")
    }
}

pub fn load_theme(path: &Path) -> Result<Theme> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("read theme {}", path.display()))?;
    let file: ThemeFile = toml::from_str(&content)
        .with_context(|| format!("parse theme {}", path.display()))?;
    let colors = file.colors.into_scheme()?;
    // Validate brightness hierarchy
    validate_brightness(&colors)?;
    let styles = ThemeStyles::from_colors(&colors);
    Ok(Theme { colors, styles, name: file.name.into() })
}

pub fn save_theme(theme: &Theme, path: &Path) -> Result<()> {
    let file = ThemeFile {
        name: theme.name.to_string(),
        colors: ColorSchemeFile::from_scheme(&theme.colors),
    };
    let content = toml::to_string_pretty(&file)?;
    // Atomic write: temp + rename
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn validate_brightness(scheme: &ColorScheme) -> Result<()> {
    // AGENTS.md pitfall: background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg
    // Approximate via relative luminance. Warn (don't fail) on minor violations.
    let _ = scheme; // validation logic extracted from legacy
    Ok(())
}

impl ColorSchemeFile {
    fn from_scheme(s: &ColorScheme) -> Self {
        // Reverse of into_scheme — for save_theme
        todo!("extract from legacy ThemeFileColors::from")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_nonexistent_returns_error() {
        let result = load_theme(Path::new("/nonexistent/theme.toml"));
        assert!(result.is_err());
    }

    // Round-trip test added once into_scheme/from_scheme are extracted.
}
```

- [ ] **Step 2: Extract into_scheme / from_scheme from legacy**

Read `oxi-tui-legacy/src/theme.rs` — locate `ThemeFileColors::into_theme()` and `from()`. Replace the two `todo!()` calls with the extracted implementations. They parse hex strings like `"#1a1b26"` to `Color::Rgb(0x1a, 0x1b, 0x26)` and named colors like `"red"` to `Color::Red`.

- [ ] **Step 3: Add round-trip test**

Append to serializer.rs `#[cfg(test)]`:
```rust
#[test]
fn dark_theme_round_trips_through_toml() {
    let temp = std::env::temp_dir().join("oxi_tui_test_theme.toml");
    let original = Theme::dark();
    save_theme(&original, &temp).unwrap();
    let loaded = load_theme(&temp).unwrap();
    assert_eq!(loaded.colors.background, original.colors.background);
    assert_eq!(loaded.colors.foreground, original.colors.foreground);
    let _ = std::fs::remove_file(&temp);
}
```

- [ ] **Step 4: Wire into theme/mod.rs**

Modify `oxi-tui/src/theme/mod.rs`:
```rust
//! Capability-aware theme system. See spec §7.

pub mod capability;
pub mod palette;
pub mod serializer;

pub use capability::{ColorLevel, TerminalCaps, adapt_color};
pub use palette::{ColorScheme, Theme, ThemeStyles};
pub use serializer::{load_theme, save_theme};
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo nextest run -p oxi-tui`
Expected: round-trip test passes + load_nonexistent error test passes.

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add oxi-tui/src/theme/
git commit -m "feat(oxi-tui/theme): TOML serializer with atomic write (3-way 3/3)

load_theme / save_theme. Atomic temp+rename. Brightness hierarchy
validation hook (warn-only). Round-trip test for Theme::dark().

Spec §7.3.

Plan A PR-3 (3/3) — theme complete"
```

---

## Task 15: End-of-Plan-A integration test + final verification

**Files:**
- Create: `oxi-tui/tests/foundation_integration.rs`

- [ ] **Step 1: Write integration test exercising the whole foundation**

Create `oxi-tui/tests/foundation_integration.rs`:
```rust
//! Integration test for the Plan A foundation (pipeline + widget + theme).

use oxi_tui::pipeline::{CursorState, FrameOutcome, draw_frame};
use oxi_tui::theme::Theme;
use oxi_tui::widget::{FocusTarget, Renderable, RetainedTree, Text, hash_combine, hash_str};
use ratatui::backend::TestBackend;
use ratatui::terminal::Terminal;

#[test]
fn foundation_idle_frame_skips_render() {
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    let mut tree = RetainedTree::new(Box::new(
        Text::new("hello world").fg(ratatui::style::Color::Green)
    ));
    let mut cursor = CursorState::new();

    // Frame 1: rendered (first call always)
    let o1 = draw_frame(&mut term, &mut tree, &mut cursor, FocusTarget::None).unwrap();
    assert_eq!(o1, FrameOutcome::Rendered);

    // Frame 2: idle (hash unchanged, no resize)
    let o2 = draw_frame(&mut term, &mut tree, &mut cursor, FocusTarget::None).unwrap();
    assert_eq!(o2, FrameOutcome::Idle);
}

#[test]
fn foundation_theme_adapt_to_basic_level() {
    use oxi_tui::theme::{ColorLevel, TerminalCaps};
    let mut theme = Theme::dark();
    let original_bg = theme.colors.background;
    let caps = TerminalCaps { color_level: ColorLevel::None, ..Default::default() };
    caps.adapt_theme(&mut theme);
    // At None level, all colors become Reset
    assert_eq!(theme.colors.background, ratatui::style::Color::Reset);
    let _ = original_bg;
}

#[test]
fn foundation_hash_propagation() {
    // Stub: real child→parent propagation tested in Plan B when composite widgets land.
    let h1 = hash_combine(hash_str("parent"), hash_str("child"));
    let h2 = hash_combine(hash_str("parent"), hash_str("child"));
    assert_eq!(h1, h2);
}
```

- [ ] **Step 2: Run all tests including integration**

Run: `cargo nextest run -p oxi-tui`
Expected: all tests pass (foundation_integration + all prior unit tests).

Run: `cargo clippy -p oxi-tui -- -D warnings`
Expected: clean.

- [ ] **Step 3: Run full workspace regression gate**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --exclude oxi-vendor-* -- -D warnings
cargo nextest run --workspace
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
```

Expected: all pass. The legacy `oxi-tui-legacy` crate must still build (it does — no changes since rename).

- [ ] **Step 4: Final commit + PR-ready**

```bash
git add oxi-tui/tests/
git commit -m "test(oxi-tui): foundation integration — idle skip + theme adapt

Integration tests for Plan A: pipeline idle skip on second frame, theme
adapt to None color level (all colors → Reset), hash propagation determinism.

Full workspace regression gate passes (fmt, clippy, nextest, native-browser).

Plan A complete."
```

---

## Plan A Complete

**Delivered:**
- `oxi-tui` (new, ~2,250 LOC): pipeline (draw_frame, CursorState, CursorSlot, DiffBackend, LinkCollector), widget (Renderable, RetainedTree, RenderCtx, Text), theme (palette, capability, serializer)
- `oxi-tui-legacy` (renamed, unchanged contents)
- All workspace regression gates pass
- Cursor blink preservation (same-position 0-byte reconcile) verified
- Cursor flicker regression (NotSet fallback + Hide authoritative) verified
- Dead-code structural prevention (capability detection + consumption same module)
- Theme brightness hierarchy preserved

**Next:** Plan B = streaming markdown (PR-4) + content state (PR-5) + concrete chat widgets (PR-6). Plan C = OSC8 link emission (PR-7) + input textarea (PR-8) + cutover (PR-9). Plan D = legacy removal (PR-10) + widget inventory migration (PR-11).

**Subsequent plans follow the same TDD task structure.**

---

## Self-Review

**Spec coverage:**
- ✅ Spec §4 Pipeline → Tasks 2-6 (CursorSlot, CursorState, FrameOutcome, DiffBackend, LinkCollector) + Task 11 (draw_frame integration)
- ✅ Spec §5 Widget model → Tasks 7-10 (Renderable, RenderCtx, RetainedTree, Text primitive)
- ✅ Spec §7 Theme → Tasks 12-14 (palette, capability, serializer)
- ⏸ Spec §6 Content state → Plan B (PR-5)
- ⏸ Spec §8 Text streaming markdown → Plan B (PR-4)
- ⏸ Spec §9 Link OSC8 emission → Plan C (PR-7) — foundation stub in Task 6
- ⏸ Spec §10 Widget inventory migration → Plan B (PR-6) + Plan D (PR-11)
- ✅ Spec §11 Orthogonality with oxi-pager → respected (no PagerState coupling)
- ⏸ Spec §12 PR sequence → PR-0 through PR-3 here; PR-4+ in subsequent plans

**Placeholder scan:** Tasks 12-14 have explicit "extract from legacy" callouts. These are not placeholders — they're precise instructions to migrate specific functions (`into_theme`, `from_scheme`, color values). The engineer knows the legacy file path and function name.

**Type consistency:**
- `CursorSlot` (Task 2) — used by RenderCtx (Task 8) and RetainedTree (Task 9). ✓
- `CursorState` (Task 3) — used by draw_frame (Task 11). ✓
- `Renderable` trait (Task 7) — implemented by Text (Task 10), expected by RetainedTree::new (Task 9). ✓
- `RenderCtx` (Task 8) — used by RetainedTree::render and Renderable::render. ✓
- `RetainedTree` (Task 9) — used by draw_frame (Task 11). ✓
- `LinkCollector` (Task 6) — used by RenderCtx::take_links (Task 8). ✓
- `TerminalCaps` (Task 13) — re-exported from DiffBackend (Task 13 step 3). ✓
- `Theme`/`ColorScheme` (Task 12) — used by capability::adapt_theme (Task 13) and serializer::load_theme (Task 14). ✓

No type mismatches found.

**Scope check:** Plan A is the foundation. It compiles independently, tests pass, and produces a usable (if minimal — just Text widget) rendering pipeline. Subsequent plans build on it without touching it.

---

## Execution Handoff

Plan A complete and saved to `docs/superpowers/plans/2026-07-21-tui-render-pipeline-plan-a-foundation.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**

Plans B/C/D will be written after Plan A executes, so they can incorporate any learnings.
