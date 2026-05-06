# oxi-tui Crate Review

**Crate:** `oxi-tui` v0.5.0  
**Date:** 2026-05-06  
**Total Lines:** 3,270 (10 source files)  
**Build:** Clean (0 warnings)  
**Tests:** 60 passed, 0 failed, 0 doc-tests  

---

## Summary Scores

| Dimension       | Score | Notes |
|-----------------|:-----:|-------|
| **Architecture**| **B+**| Clean module structure; cell/event layers add redundancy |
| **Quality**     | **B** | Correct widgets; cell-by-cell rendering instead of ratatui idioms; good test coverage for state logic |
| **Performance** | **B-**| O(n) background fills; no virtualized layout caching; char-by-char rendering |
| **Security**    | **A-**| No unsafe; no panics in non-test code; one `unwrap_or_default` on SystemTime (safe) |
| **Maintainability** | **B+** | Excellent docs; minor dead code warning; consistent style |

---

## File-by-File Analysis

### `lib.rs` — 19 lines
**Purpose:** Crate root. Re-exports public API from all modules.  
**Quality:** Clean. `#![warn(missing_docs)]` enforces documentation. Re-exports are well-chosen.  
**Concerns:** None.

---

### `cell.rs` — 123 lines  
**Purpose:** Foundational `Color` and `Attributes` types with ratatui conversion.  
**Quality:** Good. Builder-pattern API (`with_bold()`, `with_underline()`). `to_modifier()` and `to_ratatui()` conversions are correct.  
**Concerns:**
- **Redundancy risk:** `Color` is a near-clone of `ratatui::style::Color` with `Default` mapping to `Reset`. This is only justified if downstream crates need a stable serialization/ABI boundary independent of ratatui version changes. If not, this is unnecessary indirection.
- `strikethrough` and `reversed` fields on `Attributes` have no builder methods (`with_strikethrough()`, `with_reversed()` are missing).
- No tests for `Color::to_style()` or `Attributes::to_modifier()`.
- `Attributes::new()` is redundant with `Default::default()` but harmless.

**Verdict:** Justified as a theme-layer abstraction, but could be thinned.

---

### `event.rs` — 254 lines  
**Purpose:** Self-contained input event types (KeyCode, KeyEvent, MouseEvent, etc.) decoupled from crossterm/termion.  
**Quality:** Well-structured. Builder methods on `KeyModifiers`. `Event::Default` variant is a sensible no-op sentinel.  
**Concerns:**
- **Significant redundancy with crossterm/termwiz.** Every downstream crate that converts from actual terminal events must manually map `crossterm::event::KeyCode` → `oxi_tui::event::KeyCode`. This is ~100 lines of boilerplate for every backend.
- **Line 39-40:** Doc comment noise — `/// Variant.` orphan comment before `Down` variant. Appears to be a leftover from a merge.
- **`Number(u8)` variant is `#[allow(dead_code)]`** — if it's never used, it should be removed. If it's intended for future use, document that.
- **No `PartialOrd`/`Hash` on `KeyEvent`** — limits use in keybinding maps.
- **`CursorPosition(u16, u16)`** is documented but esoteric; no downstream usage visible in this crate.
- No tests for any event types (stateless, so low risk, but `as_char()` and `as_upper()` should be tested).

**Verdict:** Clean abstraction layer but creates a mapping tax on every backend. The `/// Variant.` noise and dead `Number` variant should be fixed.

---

### `fuzzy.rs` — 297 lines (106 lines tests)  
**Purpose:** Fuzzy string matching with scoring, gap penalties, word-boundary bonuses.  
**Quality:** **Excellent.** This is the highest-quality module in the crate.
- Scoring algorithm is well-tuned: consecutive bonus, gap penalty, start-of-text bonus, word-boundary bonus, length bonus.
- `fuzzy_rank()` correctly sorts descending by score.
- 20 tests covering exact match, case insensitivity, no-match, empty pattern, subsequence, gap penalty, word boundary, ranking, Unicode, single chars, length bonus, edge cases.
- No `unwrap()` in non-test code.
- Uses `partial_cmp` with `unwrap_or(Equal)` fallback for NaN safety — correct.

**Concerns:**
- Single-pass greedy matching (no backtracking). This means `"ab"` won't match `"ba"` (correct) but also means `"sr"` matches `"s_r_c"` at positions [0,2] which may not be the optimal assignment. For a command palette, this is fine.
- No ASCII-fast-path optimization — always lowercases the full text. Fine for command-palette scale (<10k candidates).

**Verdict:** Production-ready. Best-tested module.

---

### `theme.rs` — 736 lines (92 lines tests)  
**Purpose:** Complete theme system with ColorScheme, FontScheme, Spacing, file loading (TOML/JSON), hot-reload via `ThemeManager`.  
**Quality:** Very good. The most substantial module.

**Strengths:**
- Two built-in themes (Tokyo Night dark + Catppuccin Latte light).
- `ThemeFile` with serde deserialization — clean TOML/JSON loading.
- `parse_color()` supports hex (`#rrggbb`, `#rgb`), named, bright-named, indexed (`i42`), and `default`.
- `ThemeManager` with `Arc<RwLock<Theme>>` for shared access across threads.
- Hot-reload with mtime polling (configurable interval).
- `to_styles()` pre-computes all `Style` objects — good for render performance.
- 8 tests including file round-trip.

**Concerns:**
- **`into_theme()` is deeply repetitive** (~60 lines of identical `self.colors.X.as_deref().and_then(parse_color).unwrap_or(defaults.X)`). A macro or helper would cut this to ~15 lines.
- **`ThemeFile` doesn't support FontScheme or Spacing overrides** — they always use defaults. This is a limitation, not a bug.
- **`ThemeManager` is not `Send`/`Sync` safe** — `parking_lot::RwLock<Theme>` is Send+Sync, but `ThemeManager` itself has `Option<Instant>` and `Option<SystemTime>` which are fine. Actually, it IS Send+Sync. No issue here.
- **`check_reload()` takes `&mut self`** — prevents calling from multiple threads. Intended to be called from the main event loop only. Fine.
- **`ThemeFile::load()` silently falls back to dark defaults** for missing fields — this is correct but should be documented more prominently so users aren't confused when partial themes look like dark mode.
- **No `From<ColorScheme> for ThemeStyles>`** — `to_styles()` is the only path. Could implement `From` for ergonomics.
- `parse_hex` doesn't handle 8-digit (`#rrggbbaa`) or 4-digit (`#rgba`) colors — fine for terminals which don't support alpha.

**Verdict:** Solid. The repetitive `into_theme()` is the main code-smell.

---

### `widgets/mod.rs` — 22 lines  
**Purpose:** Module declarations + architecture doc comment.  
**Quality:** Good documentation of widget pattern (Widget struct + State struct).  
**Concerns:** The comment references `../components/` as legacy — remove the reference now that legacy code is gone.

---

### `widgets/chat.rs` — 451 lines (72 lines tests)  
**Purpose:** `ChatView` StatefulWidget — scrollable message list with streaming support, multiple content block types.  
**Quality:** Good data model. Rendering has issues.

**Strengths:**
- Rich content model: `ContentBlock` supports Text, Thinking (collapsible), ToolCall, ToolResult, Error.
- Streaming API is clean: `start_streaming()`, `stream_text_delta()`, `stream_tool_call()`, `finish_streaming()`.
- Proper `scroll_to_bottom()`, `scroll_up()`, `scroll_down()`.
- 5 state-level tests.

**Concerns:**
- **Cell-by-cell rendering** instead of using ratatui `Paragraph`, `Line`, `Span`. The render method manually iterates every character with `buf[(col, row)].set_char(c)`. This is ~200 lines of manual buffer manipulation that ratatui's `Paragraph` widget handles in ~5 lines. Benefits of ratatui Paragraph: automatic line wrapping, proper wide-char handling, text alignment, built-in scrolling.
- **Background fill is O(area)** — iterates every cell in the area to set `' '` with style. Could use `buf.set_style(area, style)` or `Block::default().style(style).render(area, buf)`.
- **Wide character handling is buggy:** The code checks `unicode_width::UnicodeWidthChar::width()` and writes continuation cells with `'\u{0}'`, but the column advancement (`cell_col + i`) doesn't account for the fact that the continuation cell itself takes a column position. If a wide char starts at col 10, it writes the char at col 10 and `'\u{0}'` at col 11. But the next character after the wide char should start at col 12, not col 11. The current code uses `text.chars().take(max_text).enumerate()` which counts chars, not columns, so wide chars will overflow into neighboring cells.
- **No line wrapping:** Long lines are silently truncated. For a chat view, this means long code blocks or URLs are invisible.
- **Scrollbar calculation has off-by-one risk:** `thumb_pos` calculation uses `f32` division which can round down, causing the thumb to not reach the bottom of the area.
- **`content_height` is private but `scroll_offset` is public** — inconsistent encapsulation. Users can set `scroll_offset` to values > `content_height` without validation.
- **Streaming only renders `ContentBlock::Text`** — ToolCall/Thinking/Error in streaming are ignored in the render path.
- **`test_theme()` function in tests is unused** — compiler warning confirms this.
- **No render tests** (smoke test only exists for command_palette).
- **Timestamp is stored but never displayed** — the `is_timestamp_row` field in the line tuple is always `false` and never used.

**Verdict:** Good data model, but the rendering should be rewritten to use ratatui `Paragraph`/`Line`/`Span`. The wide-char bug is a correctness issue.

---

### `widgets/command_palette.rs` — 763 lines (247 lines tests)  
**Purpose:** `CommandPalette` StatefulWidget — centered modal overlay with fuzzy-filtered command list.  
**Quality:** Most complete widget. Extensive test coverage.

**Strengths:**
- Full lifecycle: show/hide, filter, navigate, select.
- Fuzzy integration via `fuzzy_match()` with score-based sorting.
- 17 tests covering filter, selection, visibility, keyboard handling, and 3 render smoke tests (normal, no-matches, tiny area).
- Handles edge cases: empty results, tiny terminal, selected item scrolling.
- Right-aligned shortcuts with category prefixes.

**Concerns:**
- **Cell-by-cell rendering** — ~250 lines of manual buffer manipulation for borders, text, cursor. Could use ratatui `Block`, `Paragraph`, `Line` for borders and text. The `Clear` widget + `Block::bordered()` + `Paragraph` would reduce this to ~50 lines.
- **Hardcoded overlay background** `Color::Rgb(30, 30, 44)` — not theme-aware. Should use a theme color or a configurable overlay color.
- **Hardcoded backdrop** `Color::Rgb(20, 20, 30)` — same issue.
- **`_styles` computed but never used** — `let _styles = self.theme.to_styles();` on line 285 is dead code.
- **Backdrop fills entire area** — O(width × height) cell iteration. This causes visible flicker on large terminals. Consider using ratatui's `Clear` widget.
- **`handle_key()` doesn't handle Ctrl+Backspace, Ctrl+Delete, Ctrl+A (select all), etc.** — limited editing.
- **No word-wise movement** (Ctrl+Left/Right), no Home/End in the query input.
- **`selected` index is not clamped after filter** — if you select item 5, then filter reduces to 2 items, `selected` could be > `filtered_indices.len()`. The `handle_key` path resets to 0 on filter, but external callers modifying `query` directly could hit this.

**Verdict:** Feature-complete but render code is verbose. The hardcoded colors break theme consistency.

---

### `widgets/footer.rs` — 302 lines (44 lines tests)  
**Purpose:** Status bar with model info, tokens, cost, git branch, PWD.  
**Quality:** Good data formatting. Rendering has style issues.

**Strengths:**
- `FooterData` with helper methods: `format_tokens()`, `format_duration()`, `left_status()`, `right_status()`.
- Compact token formatting (1.5k, 2.5M).
- Duration formatting (30s, 5m, 1h30m).
- Home directory substitution for PWD display.
- 4 tests for formatting functions.

**Concerns:**
- **Implements both `Widget` and `StatefulWidget`** — the `Widget` impl creates a default `FooterState` and delegates. This means the stateless render path shows an empty footer. This is misleading — callers should always use the stateful path.
- **Cell-by-cell rendering** — same pattern as other widgets.
- **`left_status()` and `right_status()` methods exist but are NOT used in rendering** — the render method builds its own left/right text independently. These are dead methods for rendering purposes.
- **Git branch position calculation can overlap with right-side text** — no collision detection between left (pwd + branch) and right (model) sections.
- **PWD truncation reverses the string** (`chars().rev().take(n)`) which is O(n) and allocates twice. For a status bar, this is fine, but it's inelegant.
- **`styles` is computed but only `normal` and `accent` are used** — the rest is wasted allocation.
- **No render tests.**

**Verdict:** Useful but the dual Widget/StatefulWidget pattern is confusing. The dead `left_status()`/`right_status()` methods suggest incomplete refactoring.

---

### `widgets/input.rs` — 303 lines (43 lines tests)  
**Purpose:** Text input field with cursor, placeholder, horizontal scrolling, completion stubs.  
**Quality:** Adequate for single-line input.

**Strengths:**
- `InputState` with full cursor movement: left, right, home, end, insert, backspace, delete.
- Horizontal scrolling when text exceeds visible area.
- Completion API (`next_completion`, `prev_completion`, `accept_completion`) — wired up but no completion provider.
- 4 tests for basic operations including Unicode (`한`).

**Concerns:**
- **Single-line only** — no multi-line support. For a chat input, users frequently paste multi-line text or write multi-paragraph messages. This is a significant feature gap.
- **No selection support** — no Shift+Arrow for text selection, no Ctrl+A select all. Users can't select and delete portions of text.
- **No clipboard integration** — no Ctrl+V paste handling. The `Event::Paste` variant exists in event.rs but is never handled here.
- **No undo/redo** — expected for any non-trivial text input.
- **No Ctrl+Left/Right word movement** — standard in every terminal input.
- **Completion system is incomplete** — `completions` field, `completion_index`, `completion_active` are all private with no setter. The `accept_completion()` method exists but there's no way to populate completions from outside (no `set_completions()` method). This is dead code.
- **Cell-by-cell rendering** — 50+ lines for what `Paragraph` + cursor styling handles in ratatui.
- **`char_to_byte()` is O(n)** — called on every insert/delete. For short inputs this is fine, but a Rope or gap-buffer would be needed for large inputs.
- **Placeholder is rendered as part of the text area** — the placeholder uses `text_fg` which is `muted` style. When the user types their first character, the cursor is at position 0 and the placeholder disappears. This is correct UX but the implementation conflates display state with input state.
- **No render tests.**

**Verdict:** Minimally functional for single-line input. Missing multi-line, selection, clipboard, word movement — all critical for a production chat input. Completion API is half-implemented.

---

## Cross-Cutting Analysis

### Widget Rendering Pattern
All four widgets render **cell-by-cell** (manual `buf[(col, row)].set_char()`) instead of using ratatui's high-level widget API. This is the biggest architectural concern:

| Widget | Lines of render code | Could be with ratatui idioms |
|--------|---------------------|------------------------------|
| ChatView | ~150 | ~30 (Paragraph + Line wrapping) |
| CommandPalette | ~250 | ~60 (Block + Paragraph + List) |
| Footer | ~80 | ~20 (Paragraph with Spans) |
| Input | ~50 | ~15 (Paragraph with cursor) |
| **Total** | **~530** | **~125** |

Benefits of switching to ratatui idioms:
- Automatic line wrapping (chat.rs currently truncates)
- Proper wide-char handling (chat.rs has a bug)
- Built-in scrolling (Paragraph supports scroll offset)
- Border rendering via `Block::bordered()`
- Less code = fewer bugs

### Test Coverage Summary

| Module | State Tests | Render Tests | Total | Coverage |
|--------|:-----------:|:------------:|:-----:|:--------:|
| fuzzy | 20 | N/A | 20 | Excellent |
| theme | 8 | 0 | 8 | Good |
| chat | 5 | 0 | 5 | Fair |
| command_palette | 14 | 3 (smoke) | 17 | Good |
| footer | 4 | 0 | 4 | Fair |
| input | 4 | 0 | 4 | Fair |
| cell | 0 | 0 | 0 | Gap |
| event | 0 | 0 | 0 | Gap |
| **Total** | | | **60** | |

**Gap analysis:**
- `cell.rs`: No tests for `to_modifier()`, `to_ratatui()`, `to_style()`.
- `event.rs`: No tests for `as_char()`, `as_upper()`, builder methods.
- No render tests for chat, footer, input (only command_palette has smoke tests).
- No integration tests (multi-widget layout scenarios).

### Dependency Analysis
```
ratatui = "0.30"      # Core TUI framework
serde + serde_json     # Theme file loading
toml = "0.8"          # Theme file loading
anyhow = "1"          # Error handling
parking_lot = "0.12"  # RwLock for theme hot-reload
tracing = "0.1"       # Logging (theme reload)
unicode-width = "0.1" # Wide char handling (chat only)
tempfile = "3"        # Dev only (but not actually used in tests)
```

`tempfile` is declared as a dev-dependency but never used (tests use `std::env::temp_dir()` directly). Can be removed.

---

## Specific Findings

### Bugs
1. **chat.rs: Wide character overflow** — `ContentBlock::Text` rendering advances column position by character count, not display width. A string like `"ab한cd"` will render `한` (2 columns wide) at col 2, but the next char `c` will render at col 3, overlapping the wide char's continuation cell.
2. **command_palette.rs: `selected` not clamped after external query changes** — if `state.query` is modified directly (not via `handle_key`), `selected` can exceed `filtered_indices.len()`.
3. **footer.rs: Left/right text collision** — git branch and right-aligned model text can overlap without detection.

### Dead Code
1. `command_palette.rs:285` — `let _styles = self.theme.to_styles()` computed but never used.
2. `chat.rs` — `is_timestamp_row` field in line tuples is always `false`, never read.
3. `chat.rs:396` — `test_theme()` function in tests never called (compiler warning).
4. `input.rs` — Completion fields (`completions`, `completion_active`, `completion_index`) have no public setters.
5. `footer.rs` — `left_status()` and `right_status()` are never called from render code.
6. `event.rs:48` — `Number(u8)` variant is dead code.
7. `Cargo.toml` — `tempfile` dev-dependency is unused.

### Style/Quality
1. `event.rs:39-40` — Orphan doc comment `/// Variant.` before `Down`.
2. `event.rs` — `/// Backspace` appears twice (line 15-16 and line 17).
3. `theme.rs` — `into_theme()` has 13 identical blocks of `self.colors.X.as_deref().and_then(parse_color).unwrap_or(defaults.X)`. Should use a macro.
4. `widgets/mod.rs:6` — Comment references `../components/` which no longer exists.

### Missing Features (Priority Order)
1. **Multi-line input** — Critical for chat application.
2. **Markdown rendering** in ChatView — Currently renders raw text. No bold, code blocks, lists, or links.
3. **Text selection** in Input widget — Standard expectation.
4. **Clipboard paste** — `Event::Paste` exists but is never handled.
5. **Word movement** (Ctrl+Left/Right) in Input.
6. **Line wrapping** in ChatView — Currently truncates.
7. **Completion provider API** in Input — Scaffolding exists but is incomplete.
8. **Theme-aware overlay colors** in CommandPalette — Currently hardcoded.
9. **Virtualized rendering** in ChatView — Currently materializes all lines every frame.

---

## Recommendations

### High Priority
1. **Rewrite widget rendering to use ratatui idioms** (Paragraph, Line, Span, Block). This fixes the wide-char bug, adds line wrapping, and cuts ~400 lines of code.
2. **Add multi-line support to Input widget.** This is the #1 user-facing limitation.
3. **Fix dead code** — Remove `Number` variant, unused completion fields, orphan comments, unused `tempfile` dependency.

### Medium Priority
4. **Refactor `into_theme()`** to use a macro or helper function to eliminate repetition.
5. **Make CommandPalette overlay colors theme-aware.**
6. **Add render smoke tests** for all widgets (not just command_palette).
7. **Add tests for `cell.rs` and `event.rs`.**

### Low Priority
8. **Remove the `Widget` impl from Footer** — only `StatefulWidget` makes sense.
9. **Implement `From<ColorScheme> for ThemeStyles`.**
10. **Consider removing `cell.rs`/`event.rs` re-exports** if downstream crates always depend on ratatui directly anyway.
