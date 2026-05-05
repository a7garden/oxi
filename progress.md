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

## Files Changed

- `oxi-tui/src/components/editor.rs` — Fixed all byte/char index confusion, added 20 unicode tests

## Notes

- Pre-existing test failure `layout::tests::horizontal_split_with_percentage_constraints` is unrelated to this fix
- `accept_completion` had an additional bug: the insertion loop was reversing the completion text (inserting at same position each time). Fixed by using `replace_range()`
- The render method now properly advances `x` by unicode display width instead of always +1
