# Progress

## 2026-05-05 — Fix: oxi-cli remove dead code and fix warnings

- **packages.rs**: Removed 5 `#[allow(dead_code)]` annotations and the dead code:
  - `UPDATE_CHECK_CONCURRENCY` constant (unused)
  - `GIT_UPDATE_CONCURRENCY` constant (unused)
  - `RESOURCE_KINDS` constant (unused)
  - `with_progress()` method (unused)
  - `is_offline()` method (unused)
- **main.rs**: Documented 4 `#[allow(dead_code)]` annotations with reasons (readline-based interactive mode fallback)
- **extensions.rs**: Documented 3 `#[allow(dead_code)]` annotations (library handles, runner state for error broadcasting)
- **tui_interactive.rs**: Documented 6 `#[allow(dead_code)]` annotations (ToolCall fields, timestamp, theme colors)
- **settings.rs**: Removed unused `use std::collections::HashMap` in test
- **auto_compaction.rs**: Removed unused module-level `use futures::StreamExt` (inner scope usage kept)
- **fs_watch.rs**: Removed unused `use notify::EventKind` in test
- **model_resolver.rs**: Prefixed unused variable `has_auth` → `_has_auth`
- **packages.rs**: Fixed useless comparison `events.lock().is_empty() || events.lock().len() >= 0`
- **diagnostics.rs**: Fixed useless comparison `issues.len() >= 0`
- `cargo check -p oxi-cli` passes clean (0 warnings, 0 errors)

## 2026-05-05 — Fix: oxi-cli unify CLI parsers (main.rs + cli.rs)

- **cli.rs**: Replaced disconnected `CliArgs`/`Commands`/`ThinkingLevel`/`OutputMode`/`InstallArgs`/`RemoveArgs`/`UpdateArgs`/`ListArgs` with unified types that match main.rs's actual usage
- **cli.rs**: Removed duplicate `ThinkingLevel` enum (6 variants: Off/Minimal/Low/Medium/High/XHigh) — now re-exports the canonical `settings::ThinkingLevel` (4 variants: None/Minimal/Standard/Thorough)
- **main.rs**: Removed all local type definitions (`Args`, `Commands`, `PkgCommands`, `ConfigCommands`, `parse_thinking_level`) — now imports from `cli` module
- **main.rs**: Uses `CliArgs` instead of `Args`; references `cli::Commands`, `cli::PkgCommands`, `cli::ConfigCommands`
- All subcommands preserved: sessions, tree, fork, delete, pkg (install/list/uninstall/update), config (show/list/enable/disable/set/get)
- `cargo check -p oxi-cli` passes clean (0 errors)

## 2026-05-05 — Fix oxi-cli ignored tests

- Fixed `test_parse_version_invalid`: Changed `parse_version()` to reject non-exactly-3-component version strings (`parts.len() == 3` instead of `>= 3`)
- Fixed `test_calculate_versions_behind`: Rewrote to use per-component difference with weighting (major × 2, minor × 1, patch × 1) instead of raw `compare_versions` result
- Removed both `#[ignore]` attributes
- All 22 version_check tests pass (0 ignored)

## 2026-05-05 — Fix: oxi-tui add missing editor features + improve test coverage

### Task 1: Editor features

- **Undo/Redo**: Wired existing `undo_stack::UndoStack<String>` into the `Editor` component
  - Added `undo_stack` field, `snapshot()`, `undo()`, `redo()`, `restore_state()`, `can_undo()`, `can_redo()` methods
  - Snapshots are taken before each mutation (insert, backspace, delete, enter)
  - `Ctrl+Z` triggers undo, `Ctrl+Y` triggers redo (match arms placed before generic `Char(c)` to avoid unreachable patterns)
  - `clear()` also clears the undo stack

- **Word-wise movement**: Added `move_word_left()` and `move_word_right()` methods
  - Word chars defined as alphanumeric + underscore (consistent with `utils::find_word_boundaries`)
  - `Ctrl+Left` moves word-left, `Ctrl+Right` moves word-right
  - Added `is_word_char()` helper function

- Tests added: `test_undo_basic`, `test_undo_redo_cycle`, `test_undo_on_empty_editor`, `test_redo_on_empty_editor`, `test_undo_after_backspace`, `test_move_word_left_basic`, `test_move_word_right_basic`, `test_move_word_left_at_start`, `test_move_word_right_at_end`, `test_ctrl_left_right_events`, `test_move_word_with_underscores`

### Task 2: Test coverage improvements

- **loader.rs**: Added 20 tests covering all public API surface — creation, builder methods, tick, cancel, reset, set_done, focus, events (escape, Ctrl+C, other keys), rendering (active/cancelled/truncated), dirty flag, min_size
- **footer.rs**: Added 14 edge-case tests — token update methods, context window clamping, session duration, extension status CRUD, model-only rendering, thinking level filtering, empty token formatting, duration edge cases
- **image.rs**: Added 12 tests — empty data, builder chain, fallback narrow/tall render, base64 caching, protocol detection, JPEG Kitty format, default dimensions, file errors, events, desired size
- **fuzzy.rs**: Added 11 tests — start-of-text bonus, consecutive match bonus, rank empty cases, word boundary, single char, unicode, length bonus
- **utils.rs**: Added 22 tests — strip_ansi edge cases, visible_width mixed, wrap edge cases, segment_text, word_at edge cases, highlight variants, slice_by_column, boundary tests, background, truncation edge cases, classification

### Results

- `cargo check -p oxi-tui`: Clean (0 errors, 0 warnings)
- `cargo test -p oxi-tui`: 479 unit tests + 12 doc tests pass, 0 failures
