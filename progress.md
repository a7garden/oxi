# Progress

## Status
In Progress

## Tasks

### P1-1: Fix Editor Unicode handling in oxi-tui — ✅ DONE

All byte/char index confusion fixed. Cursor is consistently a byte position throughout.

**Changes made to `oxi-tui/src/components/editor.rs`:**

1. **`Line::insert()`** — Changed `cursor += 1` to `cursor += c.len_utf8()` so cursor advances by the correct byte count for multi-byte chars
2. **`Line::remove()`** — Changed `cursor -= 1` to `cursor -= c.len_utf8()` so cursor retreats by the correct byte count
3. **`Editor::move_left()`** — Now uses `char_indices().last()` to find the previous char boundary instead of `cursor -= 1`
4. **`Editor::move_right()`** — Now uses `char_indices().nth(1)` to find the next char boundary instead of `cursor += 1`
5. **`Editor::delete_back()`** — Now finds the byte position of the previous character using `char_indices()` before removing it, instead of blindly using `cursor - 1`
6. **`Editor::accept_completion()`** — Replaced broken char-by-char insert/remove loop with `replace_range()` which correctly handles byte ranges
7. **`Editor::render()`** — Changed `chars().enumerate()` to `char_indices()` so cursor byte position comparison is correct; added `UnicodeWidthChar::width()` for display column advancement
8. **`Editor::ensure_cursor_visible()`** — Added char boundary validation loop to snap cursor to a valid UTF-8 boundary
9. **Added `use unicode_width::UnicodeWidthChar`** import
10. **Added 20 new tests** covering Korean (3-byte), emoji (4-byte), and mixed ASCII+multi-byte scenarios

### P2-1: Fix ALL Clippy Warnings in oxi-tui — ✅ DONE

Fixed 101 clippy warnings down to 0. Used `cargo clippy --fix` for bulk auto-fixes (91), then manually fixed remaining 10.

**Auto-fixed (91 warnings):**
- `unnecessary_map_or` → `is_some_and` / direct comparison
- `clone_on_copy` → removed `.clone()` on `Copy` types
- `manual_range_contains` → `(x..=y).contains(&z)`
- `collapsible_if` → merged nested if statements
- `unnecessary_cast` → removed redundant casts
- `int_plus_one` → `row > y + total_height`
- `manual_div_ceil` → `.div_ceil()`
- `while_let_on_iterator` → `for x in iter.by_ref()`
- `manual_pattern_char_comparison` → char array pattern
- `manual_repeat_n` → `repeat_n()`
- `manual_find` → `.find()` on iterator
- `manual_split_once` → `.split_once()`
- `manual_contains` → `.contains()` instead of `.iter().any()`
- `implicit_saturating_sub` → `.saturating_sub()`
- `redundant_closure` → use function reference directly
- `derivable_impls` → `#[derive(Default)]` for Color and Event enums
- `unused_enumerate_index` → removed `.enumerate()` where unused

**Manually fixed (10 warnings):**
- `upper_case_acronyms` — renamed `SGR` → `Sgr` struct (7 references in renderer.rs)
- `dead_code` — added `#[allow(dead_code)]` to genuinely unused private methods `buf_write`/`write_str`
- `manual_strip` — refactored prefix stripping to use `strip_prefix()` in completion.rs
- `enum_variant_names` — renamed `CodeBlock` → `Code` variant in markdown.rs
- `field_reassign_with_default` — used struct update syntax in markdown heading attrs
- `too_many_arguments` — grouped (row, col) into tuple parameter in model_selector_overlay
- `manual_is_ascii_check` — used `.is_ascii_lowercase()` / `.is_ascii_digit()` in keys.rs
- `no_effect` / `unused_must_use` — removed dead comparison in layout.rs Min constraint
- `explicit_counter_loop` — used `.enumerate()` in utils.rs truncation loop
- `unused_attributes` — removed duplicate `#[inline]` in renderer.rs

## Files Changed

- `oxi-tui/src/components/editor.rs` — Fixed all byte/char index confusion, added 20 unicode tests
- `oxi-tui/src/autocomplete.rs` — Auto-fixed unnecessary_cast, unnecessary_map_or
- `oxi-tui/src/cell.rs` — Auto-fixed derivable_impls
- `oxi-tui/src/components/command_palette.rs` — Auto-fixed int_plus_one
- `oxi-tui/src/components/completion.rs` — Fixed manual_strip
- `oxi-tui/src/components/editor.rs` — Auto-fixed collapsible_if
- `oxi-tui/src/components/image.rs` — Auto-fixed manual_div_ceil
- `oxi-tui/src/components/markdown.rs` — Fixed enum_variant_names, field_reassign_with_default, auto-fixes
- `oxi-tui/src/components/model_selector_overlay.rs` — Fixed too_many_arguments, auto-fixed map_or/clone_on_copy
- `oxi-tui/src/components/theme_selector.rs` — Auto-fixed clone_on_copy
- `oxi-tui/src/event.rs` — Auto-fixed derivable_impls
- `oxi-tui/src/keybindings.rs` — Auto-fixed manual_range_contains, manual_find
- `oxi-tui/src/keys.rs` — Fixed manual_is_ascii_check, auto-fixed many range/contains/collapsible
- `oxi-tui/src/kill_ring.rs` — Auto-fixed unnecessary_map_or
- `oxi-tui/src/layout.rs` — Fixed no_effect, auto-fixed unused_enumerate_index/implicit_saturating_sub
- `oxi-tui/src/renderer.rs` — Fixed SGR→Sgr rename, dead_code, auto-fixed unnecessary_cast
- `oxi-tui/src/stdin_buffer.rs` — Auto-fixed collapsible_if
- `oxi-tui/src/tui.rs` — Auto-fixed collapsible_if
- `oxi-tui/src/utils.rs` — Fixed explicit_counter_loop, auto-fixed redundant_closure/manual_range_contains/unnecessary_map_or

## Notes

- Pre-existing test failure `layout::tests::horizontal_split_with_percentage_constraints` is unrelated to clippy fixes (existed before changes)
- No public API signatures were changed
- `dead_code` on `buf_write`/`write_str` uses `#[allow(dead_code)]` as instructed by the task for genuinely unused code
