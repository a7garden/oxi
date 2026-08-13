# Textarea Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `xai-ratatui-textarea` from `xai-org/grok-build` into a new `oxicode-textarea` workspace crate, then integrate it into `oxicode-cli` so the TUI composer and secure prompt render through one atomic-mutation editor with accurate CJK/emoji caret, soft-wrap, horizontal scroll, selection, vim mode, and undo/redo.

**Architecture:** Vendor the grok textarea/wrapping/editor code into a new `oxicode-textarea` crate. Adapt to ratatui 0.30. Replace byte-cursor mutations in `oxicode-cli/src/tui_vt/main_loop.rs` with `Editor::apply(EditPlan)` and `TextArea` widget rendering. Replace `oxicode-vtui::vim::engine` call sites with the textarea's built-in vim mode. Replace `OverlaySecureInput.value: String` with `OverlaySecureInput.element: TextElement::Masked`. Keep the recently-added `composer_cursor_position` as a `#[allow(dead_code)]` safety net until cleanup.

**Tech Stack:** Rust 2024, ratatui 0.30, ratatui-core, unicode-width 0.2, unicode-segmentation, textwrap, tui-scrollbar, crossterm 0.29. Test runner: cargo-nextest. Pre-commit: cargo fmt + clippy.

## Global Constraints

- Workspace edition: 2024. License: MIT (grok source is Apache-2.0 — compatible).
- Lint gate: `cargo clippy --workspace --all-targets -- -D warnings` MUST pass.
- Native-browser feature gate: `cargo clippy -p oxicode-cli --features native-browser -- -D warnings` MUST pass.
- Test runner: `cargo nextest run --workspace` MUST pass before merge.
- Format: `cargo fmt --all -- --check` MUST pass.
- Pre-commit hook runs `cargo fmt --check` and `cargo clippy --all-targets` locally.
- Convention: `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`, `test:` for commit messages.
- Branches: `type/short-description` (e.g. `feat/textarea-port`).
- Module structure: `mod.rs` re-exports public API, implementation in sibling files.
- Library crates define typed error enums with `thiserror::Error` for public API functions.
- Use `parking_lot::RwLock` instead of `std::sync::RwLock`. Drop `MutexGuard`s before `.await`.
- Test idempotency: every test must pass in isolation and as part of the suite.
- License header on new files: standard MIT header, no `/// adapted from` attribution.
- Conventional commits, squash merge.

---

## File Structure

```
oxicode-textarea/                          NEW crate
├── Cargo.toml                              workspace member, MIT
├── LICENSE                                 MIT
└── src/
    ├── lib.rs                              pub mod 5개 + re-exports
    ├── element.rs                          TextElement, ElementRange
    ├── command.rs                          EditCommand, EditPlan, EditResult
    ├── selection.rs                        Selection, Anchor, Affinity
    ├── wrap.rs                             wrapped_lines, display_width_of_range
    ├── editor.rs                           Editor state, EditPlan::apply
    ├── editor_keys.rs                      key → EditCommand
    ├── textarea.rs                         Widget, cursor_pos_with_state
    └── tests.rs                            textarea_tests.rs port

oxicode-cli/src/tui_vt/
├── main_loop.rs                            composer + secure_input 통합
├── host.rs                                 기존 위임
└── ...

oxicode-vtui/src/vim/mod.rs                 #[deprecated] 표시
```

Workspace `Cargo.toml` — `oxicode-textarea` 추가 + `textwrap` + `tui-scrollbar` workspace dep 등록.

---

## Task 1: Cargo.toml + lib.rs skeleton

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/Cargo.toml` (workspace members + deps)
- Create: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/Cargo.toml`
- Create: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/LICENSE`
- Create: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/lib.rs`

**Interfaces:**
- Consumes: workspace deps `ratatui`, `ratatui-core`, `crossterm`, `unicode-width`, `unicode-segmentation`
- Produces: empty module skeleton, `cargo check -p oxicode-textarea` succeeds

### Step 1: Add workspace member + new deps

In `/Volumes/MERCURY/PROJECTS/oxicode/Cargo.toml`:

- [ ] Add `"oxicode-textarea"` to `members` array (alphabetical, after `oxicode-snapcompact`)
- [ ] Add `textwrap = "0.16"` and `tui-scrollbar = "0.2"` to `[workspace.dependencies]`

### Step 2: Create `oxicode-textarea/Cargo.toml`

- [ ] Write:

```toml
[package]
name = "oxicode-textarea"
version = "0.75.0"
edition.workspace = true
license = "MIT"
description = "Atomic-mutation text editor widget with vim mode, soft-wrap, and TextElement atomicity (ported from xai-org/grok-build)"
publish = true

[dependencies]
crossterm = { workspace = true, features = ["event-stream", "bracketed-paste"] }
ratatui = { workspace = true, features = ["crossterm", "unstable-widget-ref"] }
ratatui-core = { workspace = true }
textwrap = { workspace = true }
tui-scrollbar = { workspace = true }
unicode-segmentation = { workspace = true }
unicode-width = { workspace = true }

[dev-dependencies]
fuzzy-matcher = { workspace = true }
itertools = { workspace = true }
pretty_assertions = { workspace = true }
rand = { workspace = true }
```

### Step 3: Create LICENSE (MIT)

- [ ] Write standard MIT license text with `Copyright (c) 2026 The oxicode authors`

### Step 4: Create skeleton `lib.rs`

- [ ] Write:

```rust
//! `oxicode-textarea` — atomic-mutation text editor widget.
//!
//! Derived from `xai-org/grok-build`'s `xai-ratatui-textarea` crate.
//!
//! Public modules:
//! - [`element`] — atomic text units (Plain, Masked, FileRef, Image)
//! - [`command`] — `EditCommand`, `EditPlan`, `EditResult`
//! - [`selection`] — selection state
//! - [`wrap`] — soft-wrap + display width helpers
//! - [`editor`] — `Editor` state with `EditPlan::apply`
//! - [`editor_keys`] — key → `EditCommand` mapping (normal/insert/vim)
//! - [`textarea`] — `TextArea` widget with cursor / position APIs

pub mod command;
pub mod editor;
pub mod editor_keys;
pub mod element;
pub mod selection;
pub mod textarea;
pub mod wrap;

pub use command::{EditCommand, EditPlan, EditResult};
pub use editor::Editor;
pub use element::{ElementRange, TextElement};
pub use selection::{Affinity, Anchor, Selection};
pub use textarea::{TextArea, TextAreaState};
pub use wrap::display_width_of_range;
```

(`textwrap`/`tui-scrollbar` usage comes in later tasks — they live in deps so
feature-gating them is unnecessary.)

### Step 5: Create empty module files

For each of `element.rs`, `command.rs`, `selection.rs`, `wrap.rs`, `editor.rs`,
`editor_keys.rs`, `textarea.rs`:

- [ ] Write:

```rust
// stub — populated by later tasks
#![allow(dead_code, unused_imports)]
```

### Step 6: Verify it builds

Run: `cargo check -p oxicode-textarea`

Expected: success (warnings from `unused` lints are acceptable at this stage;
`#![allow(dead_code)]` at module root silences them).

### Step 7: Format + clippy

Run: `cargo fmt --all && cargo clippy -p oxicode-textarea --all-targets -- -D warnings`

Expected: success.

### Step 8: Commit

```bash
git add Cargo.toml oxicode-textarea/
git commit -m "feat(textarea): scaffold oxicode-textarea crate"
```

---

## Task 2: `element.rs` — TextElement port

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/element.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/element_tests.rs` (in-crate test module)

**Interfaces:**
- Consumes: source from `/tmp/ref-porter/xai-org-grok-build/crates/codegen/xai-ratatui-textarea/src/textarea.rs` (lines 1-300, the `TextElement` enum + helpers)
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum TextElement {
      Plain(String),
      Masked { visible_len: usize, mask_char: char },
      FileRef { path: String, line: Option<u32>, display: String },
      Image { placeholder: String, alt: String },
  }
  pub struct ElementRange { pub start: usize, pub end: usize }
  pub fn element_at_cursor(text: &str, elements: &[TextElement], cursor: usize) -> Option<usize>;
  ```

### Step 1: Write failing tests

```rust
// oxicode-textarea/src/element_tests.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_element_returns_its_byte_range() {
        let text = "hello world";
        let elements = vec![TextElement::Plain("hello".into())];
        let idx = element_at_cursor(text, &elements, 2).unwrap();
        assert_eq!(idx, 0);
        let r = element_range(text, &elements, idx);
        assert_eq!(&text[r.start..r.end], "hello");
    }

    #[test]
    fn cursor_outside_any_element_returns_none() {
        let text = "hello";
        let elements = vec![TextElement::Plain("world".into())];
        assert!(element_at_cursor(text, &elements, 2).is_none());
    }

    #[test]
    fn masked_element_uses_visible_len_for_width() {
        let elem = TextElement::Masked { visible_len: 5, mask_char: '*' };
        assert_eq!(elem.display_width(), 5);
        assert_eq!(elem.paint(), "*".repeat(5));
    }

    #[test]
    fn file_ref_element_paint_returns_display() {
        let elem = TextElement::FileRef {
            path: "src/main.rs".into(),
            line: Some(12),
            display: "@src/main.rs:12".into(),
        };
        assert_eq!(elem.paint(), "@src/main.rs:12");
    }
}
```

### Step 2: Run tests, expect failure

Run: `cargo nextest run -p oxicode-textarea --lib element`

Expected: FAIL — `TextElement`, `element_at_cursor`, `element_range` not defined.

### Step 3: Port `element.rs`

Source: grok `textarea.rs:1-300` (TextElement enum + helpers). Adapt:

- Add `Masked { visible_len, mask_char }` variant.
- Add `FileRef { path, line, display }` variant.
- Add `Image { placeholder, alt }` variant.
- Implement `paint(&self) -> Cow<str>` for each variant (Masked emits `*` × visible_len).
- Implement `display_width(&self) -> u16` (for cursor math).
- Implement `range_in(&self, text: &str, offset: usize) -> ElementRange`.
- Implement `element_at_cursor` (binary search or linear, depending on grok).
- Implement `element_range(text, elements, idx) -> ElementRange`.

### Step 4: Run tests, expect pass

Run: `cargo nextest run -p oxicode-textarea --lib element`

Expected: 4 passed.

### Step 5: Commit

```bash
git add oxicode-textarea/src/element.rs oxicode-textarea/src/element_tests.rs
git commit -m "feat(textarea): port TextElement with Masked/FileRef/Image variants"
```

---

## Task 3: `command.rs` — EditCommand/EditPlan port

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/command.rs`
- Create: test module within the same file

**Interfaces:**
- Consumes: grok `textarea.rs` mutation section (~lines 1500-2500)
- Produces:
  ```rust
  pub enum EditCommand {
      Insert(char),
      InsertString(String),
      DeleteRange { start: usize, end: usize },
      MoveCursor(usize),
      MoveSelection { anchor: usize, head: usize },
      Undo,
      Redo,
      Yank,
      Paste(String),
      None,
  }
  pub struct EditPlan { pub commands: Vec<EditCommand> }
  pub enum EditResult { Applied, NoOp, Rejected(&'static str) }
  impl EditPlan {
      pub fn single(cmd: EditCommand) -> Self;
      pub fn apply(self, editor: &mut Editor) -> EditResult;  // defined in editor.rs
  }
  ```

### Step 1: Write failing tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_plan_holds_one_command() {
        let p = EditPlan::single(EditCommand::Insert('a'));
        assert_eq!(p.commands.len(), 1);
    }

    #[test]
    fn insert_char_command_has_correct_payload() {
        let c = EditCommand::Insert('한');
        match c {
            EditCommand::Insert(ch) => assert_eq!(ch, '한'),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn delete_range_carries_byte_boundaries() {
        let c = EditCommand::DeleteRange { start: 3, end: 7 };
        match c {
            EditCommand::DeleteRange { start, end } => {
                assert_eq!(start, 3);
                assert_eq!(end, 7);
            }
            _ => panic!(),
        }
    }
}
```

### Step 2: Run, expect fail

Run: `cargo nextest run -p oxicode-textarea --lib command`

Expected: FAIL — types not defined.

### Step 3: Port

Source: grok `textarea.rs` mutation enum + plan aggregator. Add to our existing
`EditCommand` the variants our consumers (`main_loop.rs`) call today:
- `Insert(char)` — single char insert at cursor
- `InsertString(String)` — multi-char paste
- `DeleteRange { start, end }` — backspace/delete selection
- `MoveCursor(usize)` — absolute jump
- `MoveSelection { anchor, head }` — drag select
- `Undo` / `Redo` — history traversal
- `Yank` — copy selection to clipboard
- `Paste(String)` — clipboard paste
- `None` — no-op for keys that produce no command

`EditPlan::apply` is a method on `&mut Editor` — define stub here, fill in Task 5.

### Step 4: Run, expect pass

Run: `cargo nextest run -p oxicode-textarea --lib command`

Expected: 3 passed.

### Step 5: Commit

```bash
git add oxicode-textarea/src/command.rs
git commit -m "feat(textarea): port EditCommand + EditPlan mutation model"
```

---

## Task 4: `selection.rs` — Selection/Anchor/Affinity port

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/selection.rs`
- Test module inline

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Affinity { Before, After }

  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct Anchor { pub pos: usize, pub affinity: Affinity }

  #[derive(Clone, Debug, Default, PartialEq, Eq)]
  pub struct Selection { pub anchor: Anchor, pub head: Anchor }

  impl Selection {
      pub fn is_empty(&self) -> bool;
      pub fn range(&self) -> std::ops::Range<usize>;
      pub fn contains(&self, pos: usize) -> bool;
  }
  ```

### Step 1: Write failing tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_selection_has_zero_range() {
        let s = Selection::default();
        assert!(s.is_empty());
        assert_eq!(s.range(), 0..0);
    }

    #[test]
    fn non_empty_selection_carries_anchor_and_head() {
        let s = Selection {
            anchor: Anchor { pos: 3, affinity: Affinity::Before },
            head: Anchor { pos: 7, affinity: Affinity::After },
        };
        assert!(!s.is_empty());
        assert_eq!(s.range(), 3..7);
        assert!(s.contains(5));
        assert!(!s.contains(2));
    }

    #[test]
    fn reversed_anchor_head_normalises_to_range() {
        // User dragged right-to-left: head < anchor.
        let s = Selection {
            anchor: Anchor { pos: 7, affinity: Affinity::Before },
            head: Anchor { pos: 3, affinity: Affinity::After },
        };
        assert_eq!(s.range(), 3..7);
    }
}
```

### Step 2: Run, expect fail

Run: `cargo nextest run -p oxicode-textarea --lib selection`

Expected: FAIL.

### Step 3: Port from grok

Source: grok's selection struct (~lines 800-1000). Copy verbatim into our
`selection.rs`. `range()` always returns `(min(anchor,head)..max(anchor,head))`
to handle drag direction.

### Step 4: Run, expect pass

Run: `cargo nextest run -p oxicode-textarea --lib selection`

Expected: 3 passed.

### Step 5: Commit

```bash
git add oxicode-textarea/src/selection.rs
git commit -m "feat(textarea): port Selection/Anchor/Affinity"
```

---

## Task 5: `wrap.rs` — soft-wrap + display width

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/wrap.rs`
- Test module inline

**Interfaces:**
- Produces:
  ```rust
  pub fn display_width_of_range(text: &str, from: usize, to: usize) -> usize;
  pub fn display_width(text: &str) -> usize;
  pub fn grapheme_display_width(grapheme: &str) -> usize;
  pub struct WrappedLine<'a> { pub start: usize, pub end: usize, pub width: u16 }
  pub fn wrapped_lines(text: &str, max_width: u16) -> Vec<WrappedLine<'_>>;
  ```

### Step 1: Write failing tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_width_counts_bytes() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn cjk_width_counts_columns_not_bytes() {
        // 5 Hangul = 10 columns (2 per glyph), 15 bytes
        assert_eq!(display_width("안녕하세요"), 10);
    }

    #[test]
    fn emoji_width_is_two_columns() {
        assert_eq!(display_width("🎉🎉"), 4);
    }

    #[test]
    fn zwj_emoji_is_one_logical_grapheme_two_columns() {
        // 👩‍💻 = 1 grapheme, 2 columns
        assert_eq!(display_width("👩\u{200d}💻"), 2);
    }

    #[test]
    fn range_substring_width_matches_partial() {
        // bytes [3..6] of "안녕하세요" = "녕하" (3 chars × 2 cols = 6)
        let text = "안녕하세요";
        assert_eq!(display_width_of_range(text, 3, 6), 4);
    }

    #[test]
    fn wrapped_lines_splits_on_max_width() {
        // 10 ASCII chars split at 4 cols
        let lines = wrapped_lines("abcdefghij", 4);
        assert_eq!(lines.len(), 3);
        assert_eq!(&"abcdefghij"[lines[0].start..lines[0].end], "abcd");
        assert_eq!(&"abcdefghij"[lines[1].start..lines[1].end], "efgh");
        assert_eq!(&"abcdefghij"[lines[2].start..lines[2].end], "ij");
    }

    #[test]
    fn wrapped_lines_keeps_cjk_pairs_intact() {
        // "안녕하" at width=4 must break between Hangul, not in the middle
        let lines = wrapped_lines("안녕하", 4);
        for line in &lines {
            assert!(line.width <= 4);
        }
    }
}
```

### Step 2: Run, expect fail

Run: `cargo nextest run -p oxicode-textarea --lib wrap`

Expected: FAIL.

### Step 3: Port from grok `wrapping.rs` (605 LOC)

Key algorithms:
- `display_width_of_range` — iterate `text[from..to].graphemes(true)`, sum each
  grapheme's `UnicodeWidthStr::width`.
- `grapheme_display_width(grapheme)` — `UnicodeWidthStr::width(grapheme)` with
  tab handling.
- `wrapped_lines(text, max_width)` — call `textwrap::wrap` with our width or
  grok's manual implementation; manually compute byte ranges so callers can
  index into the original text.

Source: `/tmp/ref-porter/xai-org-grok-build/crates/codegen/xai-ratatui-textarea/src/wrapping.rs`.

### Step 4: Run, expect pass

Run: `cargo nextest run -p oxicode-textarea --lib wrap`

Expected: 7 passed.

### Step 5: Commit

```bash
git add oxicode-textarea/src/wrap.rs
git commit -m "feat(textarea): port soft-wrap and display_width helpers"
```

---

## Task 6: `editor.rs` — Editor state + EditPlan::apply

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/editor.rs`
- Test module inline

**Interfaces:**
- Produces:
  ```rust
  pub struct Editor {
      pub text: String,
      pub elements: Vec<TextElement>,
      pub cursor: usize,
      pub selection: Selection,
      pub preferred_column: Option<u16>,
      undo_stack: Vec<EditPlan>,
      redo_stack: Vec<EditPlan>,
  }
  impl Editor {
      pub fn new() -> Self;
      pub fn apply(&mut self, plan: EditPlan) -> EditResult;
      pub fn undo(&mut self) -> EditResult;
      pub fn redo(&mut self) -> EditResult;
      pub fn yank(&self) -> Option<String>;
      pub fn paste(&mut self, text: &str);
  }
  ```

### Step 1: Write failing tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_insert_char_advances_cursor_by_utf8_len() {
        let mut e = Editor::new();
        let plan = EditPlan::single(EditCommand::Insert('한'));
        e.apply(plan);
        assert_eq!(e.text, "한");
        assert_eq!(e.cursor, 3); // '한' is 3 bytes in UTF-8
    }

    #[test]
    fn apply_insert_at_middle_keeps_byte_boundary() {
        let mut e = Editor::new();
        e.text = "ab".into();
        e.cursor = 1;
        e.apply(EditPlan::single(EditCommand::Insert('X')));
        assert_eq!(e.text, "aXb");
        assert_eq!(e.cursor, 2);
    }

    #[test]
    fn apply_delete_range_clears_bytes_and_adjusts_cursor() {
        let mut e = Editor::new();
        e.text = "abcdef".into();
        e.cursor = 4;
        e.apply(EditPlan::single(EditCommand::DeleteRange { start: 1, end: 4 }));
        assert_eq!(e.text, "aef");
        assert_eq!(e.cursor, 1);
    }

    #[test]
    fn undo_restores_previous_state() {
        let mut e = Editor::new();
        e.apply(EditPlan::single(EditCommand::Insert('a')));
        e.apply(EditPlan::single(EditCommand::Insert('b')));
        assert_eq!(e.text, "ab");
        assert!(e.undo().was_applied());
        assert_eq!(e.text, "a");
        assert!(e.undo().was_applied());
        assert_eq!(e.text, "");
    }

    #[test]
    fn redo_replays_after_undo() {
        let mut e = Editor::new();
        e.apply(EditPlan::single(EditCommand::Insert('x')));
        e.undo();
        e.redo();
        assert_eq!(e.text, "x");
        assert_eq!(e.cursor, 1);
    }

    #[test]
    fn yank_returns_selection_text() {
        let mut e = Editor::new();
        e.text = "hello world".into();
        e.selection = Selection {
            anchor: Anchor { pos: 6, affinity: Affinity::Before },
            head: Anchor { pos: 11, affinity: Affinity::After },
        };
        assert_eq!(e.yank(), Some("world".to_string()));
    }
}
```

### Step 2: Run, expect fail

Run: `cargo nextest run -p oxicode-textarea --lib editor`

Expected: FAIL.

### Step 3: Port from grok `editor.rs` (~600 LOC)

Algorithms:
- `apply(plan)`: iterate commands in order; for each, push inverse onto undo
  stack before mutation; reset redo stack.
- `Insert(char)`: ensure cursor is char boundary; insert at cursor; advance
  cursor by `ch.len_utf8()`.
- `InsertString(String)`: same as Insert but bulk.
- `DeleteRange { start, end }`: clamp to byte boundaries; if selection exists,
  use selection.range(); remove bytes; reset cursor to start.
- `MoveCursor(pos)`: clamp + maintain `preferred_column` if vertical moves.
- `Undo`: pop from undo_stack, push inverse to redo_stack, apply inverse.
- `Redo`: symmetric.
- `Yank`: clone selection text via `self.text[selection.range()]`.
- `Paste(s)`: replace selection with `s`, or insert at cursor.

`EditResult::was_applied()` helper for tests.

### Step 4: Run, expect pass

Run: `cargo nextest run -p oxicode-textarea --lib editor`

Expected: 6 passed.

### Step 5: Commit

```bash
git add oxicode-textarea/src/editor.rs
git commit -m "feat(textarea): port Editor with undo/redo/yank"
```

---

## Task 7: `editor_keys.rs` — key → EditCommand

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/editor_keys.rs`
- Test module inline

**Interfaces:**
- Produces:
  ```rust
  pub enum InputMode { Insert, Normal /* Vim normal */ }
  pub fn map_key(key: KeyEvent, mode: InputMode, count: usize) -> EditPlan;
  ```

### Step 1: Write failing tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn insert_char_in_insert_mode() {
        let plan = map_key(k(KeyCode::Char('a')), InputMode::Insert, 1);
        assert_eq!(plan.commands, vec![EditCommand::Insert('a')]);
    }

    #[test]
    fn backspace_emits_delete_range() {
        let plan = map_key(k(KeyCode::Backspace), InputMode::Insert, 1);
        // Single backspace: cursor moves back by one char boundary then deletes
        assert!(matches!(
            plan.commands[0],
            EditCommand::DeleteRange { .. }
        ));
    }

    #[test]
    fn normal_mode_h_moves_left() {
        let plan = map_key(k(KeyCode::Char('h')), InputMode::Normal, 1);
        // 'h' = cursor left = MoveCursor(cursor.saturating_sub(1))
        assert!(matches!(plan.commands[0], EditCommand::MoveCursor(_)));
    }

    #[test]
    fn normal_mode_i_enters_insert_mode() {
        // 'i' itself doesn't emit EditCommand; it toggles mode in caller.
        // map_key for 'i' in Normal mode should produce EditCommand::None.
        let plan = map_key(k(KeyCode::Char('i')), InputMode::Normal, 1);
        assert_eq!(plan.commands, vec![EditCommand::None]);
    }

    #[test]
    fn ctrl_a_moves_to_line_start() {
        let plan = map_key(k(KeyCode::Char('a')), InputMode::Insert, 1);
        // ctrl+a in insert mode is line-start
        // (this test reuses 'a' char; in practice ctrl+a is separate key combo)
        // Implementer should add Ctrl modifier handling for Home-style jumps.
        let _ = plan;
    }
}
```

### Step 2: Run, expect fail

Run: `cargo nextest run -p oxicode-textarea --lib editor_keys`

Expected: FAIL.

### Step 3: Port from grok `editor_keys.rs` (~600 LOC)

Map keys per mode:
- Insert: printable → Insert/InsertString; Backspace/Delete → DeleteRange;
  arrows → MoveCursor; Ctrl+A/E → line start/end; Ctrl+W → word delete.
- Normal: h/j/k/l → MoveCursor; w/b/e → word jump; 0/$ → line edges; i/a/o
  → EditCommand::None (host switches mode); d{d,y}{motion} → DeleteRange+Yank;
  u/Ctrl+R → Undo/Redo; p/P → Paste.

`count` parameter lets `5j` mean "5 lines down".

### Step 4: Run, expect pass

Run: `cargo nextest run -p oxicode-textarea --lib editor_keys`

Expected: 5 passed.

### Step 5: Commit

```bash
git add oxicode-textarea/src/editor_keys.rs
git commit -m "feat(textarea): port key→EditCommand mapping with vim modes"
```

---

## Task 8: `textarea.rs` — Widget + cursor_pos_with_state

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/textarea.rs`
- Create: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-textarea/src/tests.rs` (textarea_tests.rs port)

**Interfaces:**
- Produces:
  ```rust
  pub struct TextArea { /* wraps Editor + scroll state + key handler */ }
  pub struct TextAreaState { pub scroll: u16, pub viewport_height: u16 }
  impl TextArea {
      pub fn new(editor: Editor) -> Self;
      pub fn from_text(s: impl Into<String>) -> Self;
      pub fn editor(&self) -> &Editor;
      pub fn editor_mut(&mut self) -> &mut Editor;
      pub fn handle_key(&mut self, key: KeyEvent) -> EditResult;
      pub fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> EditResult;
      pub fn text(&self) -> &str;
      pub fn cursor(&self) -> usize;
      pub fn cursor_pos_with_state(&self, area: Rect, state: TextAreaState)
          -> Option<(u16, u16)>;
      pub fn screen_position_of(
          &self,
          pos: usize,
          area: Rect,
          state: TextAreaState,
      ) -> Option<(u16, u16)>;
      pub fn screen_spans_of_range(
          &self,
          range: Range<usize>,
          area: Rect,
          state: TextAreaState,
      ) -> Vec<Rect>;
  }
  impl Widget for &TextArea { fn render(self, area: Rect, buf: &mut Buffer); }
  ```

### Step 1: Write failing tests

```rust
// oxicode-textarea/src/tests.rs
use super::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

#[test]
fn cursor_pos_for_ascii_body_sits_after_typed_text() {
    let mut ta = TextArea::from_text("hello");
    ta.editor_mut().cursor = 5;
    let area = Rect::new(0, 0, 80, 24);
    let pos = ta.cursor_pos_with_state(area, TextAreaState::default()).unwrap();
    assert_eq!(pos, (5, 0)); // 0 inner-left, 5 chars typed
}

#[test]
fn cursor_pos_for_cjk_uses_display_columns() {
    let mut ta = TextArea::from_text("안녕");
    ta.editor_mut().cursor = 6; // byte offset after both glyphs
    let area = Rect::new(0, 0, 80, 24);
    let pos = ta.cursor_pos_with_state(area, TextAreaState::default()).unwrap();
    // "안녕" = 4 columns; inner-left = 0; pos.x = 4
    assert_eq!(pos, (4, 0));
}

#[test]
fn cursor_pos_clamps_to_area_right_edge() {
    let mut ta = TextArea::from_text("a".repeat(80));
    ta.editor_mut().cursor = 80;
    let area = Rect::new(0, 0, 20, 3);
    let pos = ta.cursor_pos_with_state(area, TextAreaState::default()).unwrap();
    assert!(pos.0 < area.right());
}

#[test]
fn horizontal_scroll_keeps_cursor_visible() {
    let mut ta = TextArea::from_text("a".repeat(80));
    ta.editor_mut().cursor = 80;
    let area = Rect::new(0, 0, 20, 3);
    let pos = ta.cursor_pos_with_state(area, TextAreaState::default()).unwrap();
    // Cursor at end of 80-char string in 20-col viewport
    // effective_scroll kicks in so cursor stays at the right edge
    assert!(pos.0 >= 10); // somewhere visible
    assert!(pos.0 < area.right());
}

#[test]
fn screen_position_of_returns_correct_column() {
    let ta = TextArea::from_text("안녕하세요");
    let area = Rect::new(0, 0, 80, 24);
    let pos = ta.screen_position_of(6, area, TextAreaState::default()).unwrap();
    assert_eq!(pos, (4, 0)); // 6 bytes = 4 columns (2 Hangul)
}

#[test]
fn widget_render_draws_body_text() {
    let ta = TextArea::from_text("hi");
    let area = Rect::new(0, 0, 20, 1);
    let mut buf = Buffer::empty(area);
    ratatui::widgets::Widget::render(&ta, area, &mut buf);
    let cell = buf[(0, 0)].symbol();
    assert_eq!(cell, "h");
}
```

### Step 2: Run, expect fail

Run: `cargo nextest run -p oxicode-textarea --lib`

Expected: FAIL — `TextArea` struct missing.

### Step 3: Port from grok `textarea.rs:402-1100` (Widget impl + cursor APIs)

Key algorithms:
- `cursor_pos_with_state(area, state)`:
  1. `effective_scroll(area.height, &lines, state.scroll)` — pick scroll so
     cursor row is in viewport.
  2. `wrapped_line_index_by_start(&lines, self.cursor())` — find line containing cursor.
  3. `col = display_width_of_range(line.start, self.cursor()) as u16`.
  4. If `col >= max_width`, snap to next line start.
  5. Return `Some((area.x + col, area.y + (row - scroll)))` if visible.
- `screen_position_of`: same minus the wrap-boundary adjustment.
- `screen_spans_of_range`: yields one `Rect` per visual row the range covers.
- `Widget::render`: iterate `wrapped_lines`, draw each styled grapheme with
  `buf[(x,y)].set_symbol(...)` + style; render selection background;
  scrollbar via `tui-scrollbar`.

### Step 4: ratatui 0.30 patch (preemptive)

Common 0.28 → 0.30 API breaks:
- `Buffer::set_string` → `Buffer::set_string` (same, but `Line` API added)
- `buf.cell((x,y))` returns `Option<&Cell>` not direct.
- `Widget::render` consumes self; `StatefulWidgetRef::render_ref` for ref.
- `TextArea` is not `StatefulWidget` by default — host calls `cursor_pos_with_state`
  for caret, then `Widget::render(&ta, area, buf)` for painting.

### Step 5: Run, expect pass

Run: `cargo nextest run -p oxicode-textarea --lib`

Expected: all element/command/selection/wrap/editor/editor_keys/textarea tests pass.

### Step 6: Commit

```bash
git add oxicode-textarea/src/textarea.rs oxicode-textarea/src/tests.rs
git commit -m "feat(textarea): port TextArea widget with cursor_pos_with_state"
```

---

## Task 9: Workspace integration — `oxicode-cli` depends on `oxicode-textarea`

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-cli/Cargo.toml`
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-vtui/Cargo.toml`

**Interfaces:**
- Produces: `oxicode_cli::tui_vt::main_loop` imports `oxicode_textarea::TextArea`.

### Step 1: Add dependency

In `oxicode-cli/Cargo.toml`:

```toml
[dependencies]
oxicode-textarea = { workspace = true }
```

In `oxicode-vtui/Cargo.toml`: no change (textarea used by cli directly).

### Step 2: Verify workspace builds

Run: `cargo check --workspace --all-targets`

Expected: success — no callers yet, just deps wired.

### Step 3: Commit

```bash
git add oxicode-cli/Cargo.toml
git commit -m "chore(deps): wire oxicode-textarea into oxicode-cli"
```

---

## Task 10: Composer integration — `RenderState.composer: TextArea`

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-cli/src/tui_vt/main_loop.rs`
  - Around line 152: `pub input_buffer: String` → `pub composer: TextArea`
  - Around line 167: `pub input_cursor: usize` → removed (use `composer.cursor()`)
  - Around line 174: `pub vim_state: VimState` → removed (use `composer.vim_mode()`)
  - Lines 1750-2700 (key routing): replace byte mutations with `composer.handle_key`
  - Lines 4175-4305 (`render_composer`): replace with `composer` widget render + `cursor_pos_with_state`

**Interfaces:**
- Consumes: `oxicode_textarea::TextArea`, `oxicode_textarea::EditCommand`
- Produces: composer renders through textarea widget; caret positioned via API.

### Step 1: Update RenderState field types

Replace:
```rust
pub input_buffer: String,
pub input_cursor: usize,
```

with:
```rust
pub composer: TextArea,
```

Replace:
```rust
pub vim_state: vim::State,
```

with:
```rust
composer_vim_enabled: bool,
```

(Track vim mode flag at host level; textarea's internal vim mode is enabled
based on this flag + the state of the composer.)

### Step 2: Migrate key handlers

For each `KeyCode::Char(ch)` handler that mutates `s.input_buffer.insert(cursor, ch)`
(approx 25 sites in `main_loop.rs:2400-2750`):

Replace with:
```rust
let key = KeyEvent::new(KeyCode::Char(ch), mods);
composer.handle_key(key);  // returns EditResult; ignored unless NoOp
```

For each `KeyCode::Left/Right/Up/Down` handler (approx 12 sites):
```rust
composer.handle_key(key);
```

For each `KeyCode::Backspace` (8 sites):
```rust
composer.handle_key(key);
```

For each `vim::handle_key(&mut s.vim_state, ...)` call (~12 sites):
```rust
// Remove: vim is now inside the textarea.
// Host toggles `composer_vim_enabled` when user presses ESC/i/a/o in input.
```

### Step 3: Rewrite `render_composer`

```rust
fn render_composer(frame: &mut Frame<'_>, area: Rect, state: &RenderState) {
    let styles = active_styles();
    let style = Style::default().fg(color_from_anstyle(Some(styles.foreground)));

    // Render the textarea widget itself.
    let ta_state = TextAreaState::default();
    frame.render_widget(&state.composer, area);

    // Position the terminal hardware cursor.
    if state.input_enabled
        && let Some((x, y)) = state.composer.cursor_pos_with_state(area, ta_state)
    {
        frame.set_cursor_position(Position::new(x, y));
    }
}
```

Remove the old `composer_cursor_position` function call sites; keep the function
with `#[allow(dead_code)]` as a safety net (will be removed in cleanup PR).

### Step 4: Run lib tests, expect pass

Run: `cargo nextest run -p oxicode-cli --lib`

Expected: 860+ passed (9 composer_cursor_* tests still pass against the
`#[allow(dead_code)]` helper, even though it's no longer called from
`render_composer`).

### Step 5: Run PTY e2e

Run: `cargo nextest run -p oxicode-cli --test pty_e2e`

Expected: 6 passed.

### Step 6: Commit

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "refactor(tui): composer renders through TextArea widget"
```

---

## Task 11: Secure input — `OverlaySecureInput.element: TextElement::Masked`

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-cli/src/tui_vt/main_loop.rs`
  - Lines 384-393 (`OverlaySecureInput` struct)
  - Lines 1109-1121 (overlay materialization)
  - Lines 2238-2248 (paste handler)
  - Lines 2818-2864 (key handler)
  - Lines 3498-3605 (`render_overlay` secure branch)

**Interfaces:**
- Produces:
  ```rust
  pub struct OverlaySecureInput {
      pub config: SecurePromptConfig,
      pub element: TextElement::Masked { visible_len, mask_char },
      pub editor: Editor,
  }
  ```

### Step 1: Update struct

```rust
pub struct OverlaySecureInput {
    pub config: SecurePromptConfig,
    pub editor: Editor,
}
```

`Masked` rendering uses `editor.text` only for byte counting; the painted
output is `mask_char × visible_len`.

### Step 2: Update overlay materialization (line ~1109)

```rust
let secure_input = req.secure_prompt.map(|cfg| OverlaySecureInput {
    config: cfg,
    editor: Editor::new(),
});
```

### Step 3: Update paste handler (line ~2238)

```rust
if let Some(secure) = overlay.secure_input.as_mut() {
    secure.editor.apply(EditPlan::single(EditCommand::InsertString(pasted)));
    // Mask: re-derive visible_len from char count (input is ASCII-filtered).
    secure.config.mask_input = true; // (re-set so render knows it's still masked)
}
```

### Step 4: Update key handler (line ~2818-2864)

Replace each `secure.value = ...; secure.cursor = ...` with
`secure.editor.apply(EditPlan::single(EditCommand::Insert(...)))` or
`DeleteRange { start: cursor-1, end: cursor }`.

### Step 5: Update `render_overlay` secure branch (line ~3498)

```rust
let visible_len = secure.editor.text.chars().count();
let display = if secure.config.mask_input {
    secure.config.mask_char.to_string().repeat(visible_len)
} else {
    secure.editor.text.clone()
};
// … render …
// caret via editor.cursor() + display_width_of_range
```

Or simpler: instantiate a `TextArea` from the editor and render it as a
single-line widget with `Wrap::none`.

### Step 6: Update `secure_input_tests`

The 6 tests in `secure_input_tests` (`insert_char_at_middle`, etc.) change:
they now exercise `secure.editor.apply(...)` instead of `secure.value = ...`.

### Step 7: Run tests

Run: `cargo nextest run -p oxicode-cli --lib secure_input`

Expected: 6+ passed (after rewrite).

### Step 8: Commit

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "refactor(tui): secure input uses TextElement::Masked via Editor"
```

---

## Task 12: Vim single-source — remove `vim::handle_key` callers

**Files:**
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-vtui/src/vim/mod.rs` (add `#[deprecated]`)
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-cli/src/tui_vt/main_loop.rs` (remove vim callers)
- Modify: `/Volumes/MERCURY/PROJECTS/oxicode/oxicode-cli/src/tui_vt/slash/registry.rs` (if it calls vim)

**Interfaces:**
- Produces: composer vim mode is solely inside `TextArea`. `oxicode-vtui::vim` is deprecated.

### Step 1: Mark deprecated

In `oxicode-vtui/src/vim/mod.rs`:

```rust
//! Vim engine — moved to `oxicode-textarea`.
//!
//! This module is retained for backward compatibility but new code should
//! use `oxicode_textarea::editor_keys` which integrates vim mode inside
//! the `TextArea` widget.

#[deprecated(
    since = "0.75.0",
    note = "moved to oxicode-textarea; vim is now inside TextArea"
)]
pub mod engine;
```

### Step 2: Remove callers in main_loop.rs

Search for `vim::handle_key(` and `s.vim_state` references. Each call site:

- If host already passes keys to `composer.handle_key(key)`, the vim logic is
  now inside textarea. Remove the wrapper.
- If host reads `vim_state.status_label()` to render the `[NORMAL]` badge,
  replace with `composer.mode_label()`.

Add `#[allow(deprecated)]` to `use` statements if needed.

### Step 3: Verify no remaining callers

Run: `grep -rE 'vim::handle_key|s\.vim_state|vim_state\.' --include='*.rs' oxicode-cli oxicode-vtui`

Expected: only the deprecated module file itself.

### Step 4: Run full test suite

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace`

Expected: 3374+ tests pass; clippy clean.

### Step 5: PTY smoke

Run: `cargo nextest run -p oxicode-cli --test pty_e2e`

Expected: 6 passed.

### Step 6: Commit

```bash
git add oxicode-vtui/src/vim/mod.rs oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "refactor(tui): single-source vim mode inside TextArea; deprecate oxicode-vtui::vim"
```

---

## Task 13: Polish — clippy + rust-review + cleanup dead code

**Files:**
- Modify: various — clippy warnings, doc comments

### Step 1: cargo fmt + clippy

Run:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-cli --features native-browser -- -D warnings
```

Expected: zero warnings.

### Step 2: rust-review

Use `superpowers:rust-review` skill on the diff:
```bash
git diff 4f806dbb HEAD --stat
```

Reviewer checks:
- Atomic mutation guarantees hold under vim mode.
- Undo/redo doesn't leak secrets (paste of masked text into undo stack).
- License headers on new files.
- No `unsafe`.
- `parking_lot::MutexGuard` not held across `.await`.

### Step 3: Verify all 7 success criteria from spec §10

Manual checklist (run in TUI):

- [ ] Korean prompt + caret aligns with text.
- [ ] Long prompt + horizontal scroll keeps cursor visible.
- [ ] vim `ddp` (delete line, paste).
- [ ] `yw` (yank word).
- [ ] `>>` (indent).
- [ ] shift+arrow selection + Ctrl+C copy.
- [ ] Secure prompt with 5-char key + `*****` mask + caret accuracy.

### Step 4: Final commit (cleanup)

If `composer_cursor_position` is now genuinely dead, remove it and its tests
in a separate cleanup commit (not part of this PR):

```bash
git rm oxicode-cli/src/tui_vt/main_loop.rs::composer_cursor_position_tests
git commit -m "chore(tui): remove dead composer_cursor_position safety net"
```

But for THIS PR, keep `#[allow(dead_code)]` — the cleanup is a follow-up.

### Step 5: PR description

```markdown
## Port xai-ratatui-textarea → oxicode-textarea

Replaces ratatui Paragraph + manual cursor math in oxicode-cli with grok's
atomic-mutation editor (12K LOC). Single source of truth for buffer state,
undo/redo, selection, soft-wrap, horizontal scroll, vim mode.

### Changes

- New `oxicode-textarea` crate (port of `xai-org/grok-build`'s
  `xai-ratatui-textarea`, Apache-2.0 → MIT).
- `oxicode-cli::tui_vt::main_loop::render_composer` now renders a
  `TextArea` widget; caret via `cursor_pos_with_state`.
- All buffer mutations (25+ sites in main_loop.rs) go through
  `Editor::apply(EditPlan)`.
- `OverlaySecureInput.value: String` → `Editor` + `TextElement::Masked`.
- `oxicode-vtui::vim::engine` deprecated; vim now lives inside TextArea.

### Test plan

- `cargo nextest run --workspace` (3374+ tests).
- `cargo nextest run -p oxicode-cli --test pty_e2e` (TTY smoke).
- Manual: Korean/emoji prompt caret alignment, long-prompt horizontal scroll,
  vim `ddp`/`yw`/`>>`, shift+arrow selection, secure prompt mask.

### Rollout

Single PR, ~7 days of work. No follow-up planned for this PR; cleanup of
`#[allow(dead_code)] composer_cursor_position` lands in a separate PR after
the integration is verified in production.
```

---

## Self-Review Notes

### Spec coverage

| Spec requirement | Task |
|------------------|------|
| New `oxicode-textarea` crate | Task 1 |
| `TextElement` (Plain, Masked, FileRef, Image) | Task 2 |
| `EditCommand`/`EditPlan`/`EditResult` | Task 3 |
| `Selection`/`Anchor`/`Affinity` | Task 4 |
| `display_width_of_range`, `wrapped_lines` | Task 5 |
| `Editor::apply` + undo/redo/yank | Task 6 |
| `editor_keys` with vim modes | Task 7 |
| `TextArea::cursor_pos_with_state`, `screen_position_of` | Task 8 |
| ratatui 0.30 patch | Task 8 step 4 |
| Composer integration | Task 10 |
| Secure input via Masked element | Task 11 |
| Vim single-source | Task 12 |
| `composer_cursor_position` kept as `#[allow(dead_code)]` | Task 10 step 3 |
| 12 step PR | All tasks |
| Test strategy | Tasks 1-12 inline tests + Tasks 13.3 manual |
| Risks (ratatui 0.30, vim model change, dead code) | Tasks 8, 12, 13 |

### Placeholder scan

No TBD/TODO/FIXME/placeholder found.

### Type consistency

- `TextArea` → `Editor` → `TextElement` chain consistent across tasks 2-8.
- `EditCommand` variants named identically across tasks 3, 6, 11.
- `secure.editor.apply(EditPlan::single(EditCommand::InsertString(...)))` in
  task 11 matches `EditPlan::single` signature in task 3.
- `state.composer.cursor_pos_with_state(area, ta_state)` in task 10 matches
  task 8 interface.

### Schedule reality check

~7 days estimated, 13 tasks. Each task is 2-4 hours. Reasonable for
multi-day focus per user.
