# Final-review fix wave — feat/tui-omp-ideas

Scope: all 6 findings (2 Critical, 4 Important) from the whole-branch final
review, plus the flagged dead code. Verification gate at the bottom.

## Finding 1 — CRITICAL: first-frame CSI 3J wipes shell scrollback

**Root cause.** `RenderState::default()` seeds `viewport_width: 80`
(main_loop.rs). On the first draw the resize detector compared
`prev_w = 80` against the real terminal width, so any terminal wider than
80 columns looked like a resize and fired `ESC[3J` + `terminal.clear()` —
destroying the user's pre-TUI shell scrollback on every launch.

**Fix (main_loop.rs).**
1. `run_event_loop` seeds `viewport_width` / `last_viewport_height` from
   `terminal.size()` before the initial draw. On a size failure it parks
   the 0 sentinel instead of a fake width.
2. `should_rebuild_scrollback` treats `prev_w == 0` as "never measured"
   and refuses to wipe (`prev_w != 0 && prev_w != new_w`) — belt and
   suspenders for any path that starts at the sentinel.

**Regression test.** `scrollback_commit_tests::rebuild_only_on_width_change`
now asserts `should_rebuild_scrollback(0, 100, 24, 24) == false` and
`should_rebuild_scrollback(0, 80, 24, 24) == false`.

**Command.**
```
cargo nextest run -p oxicode-cli scrollback
→ tui_vt::main_loop::scrollback_commit_tests::rebuild_only_on_width_change PASS
```

## Finding 2 — CRITICAL: git TUI diff pane scroll/navigation inert

**Root cause.** `selected_hunk` was mutated by j/k, alt+↓/↑ and g/G, but
no renderer read it, and the diff `Paragraph` rendered from row 0 with no
`.scroll()` — long diffs truncated and hunk navigation did nothing visible.

**Fix (git_tui/render.rs).**
- New pure helpers: `hunk_header_row(hunks, selected)` (flat no-wrap row
  offset of a hunk's header; each hunk renders `1 + lines.len()` rows) and
  `hunk_scroll_offset(hunks, selected, pane_height)` =
  `min(header offset, total − pane_height)`.
- `render_diff_pane` scrolls the inline-diff `Paragraph` by the computed
  offset (Inline view and the Split→inline fallback both).
- `render_inline` takes `selected_hunk` and renders the selected hunk's
  header in reverse video (BOLD | REVERSED) — the visible cursor.
- Exported via `git_tui::hunk_scroll_offset`.

**Regression tests.**
- `render::tests::hunk_scroll_offset_pins_selected_header_into_view` —
  pure helper: offset 0 for hunk 0, header-at-top when it fits, clamped to
  the last window, 0 when the pane fits the diff, clamped selection, empty.
- `render::tests::tall_diff_scrolls_to_selected_hunk_header` — TestBackend
  render smoke: 5 hunks × 5 rows in a 10-row pane with `selected_hunk = 4`
  shows `@@ -21,4` (selected) and NOT `@@ -1,4` (hunk 0 scrolled out).

**Command.**
```
cargo nextest run -p oxicode-cli git_tui
→ 31 passed (incl. both new tests)
```

## Finding 3 — IMPORTANT: needs_refresh never drained

**Root cause.** The doc comment promised the render loop drains
`needs_refresh`; no consumer existed. After `c` commit the overlay kept
showing the pre-commit diff until a manual `r`.

**Fix (git_tui/mod.rs).** Inline-after-mutation (the preferred option):
`toggle_stage()` and `commit()` call `self.refresh(cwd)` directly at the
end when the git command succeeded. The `needs_refresh` field is kept for
batch-mutation callers and documented as consumed by `refresh()`; the
stale "render loop drains it" promise is gone from the docs.

**Regression tests.**
- `tests::toggle_stage_moves_path_between_staged_and_entries` — now
  asserts `entries`/`staged` reflect the flip immediately after
  `toggle_stage` (manual `refresh` calls and the `needs_refresh` assertion
  removed from the test).
- `tests::commit_clears_message_on_success` — now asserts the committed
  file is gone from both `entries` and `doc.files` right after `commit()`.

**Command.**
```
cargo nextest run -p oxicode-cli git_tui::tests
→ toggle_stage / externally_staged / commit tests PASS
```

## Finding 4 — IMPORTANT: staging mirror blind to external staging

**Root cause.** `staged: HashSet` tracked only overlay-issued adds, so for
a file staged outside the overlay `already_staged` was false and `u`
(unstage) silently skipped.

**Fix (git_tui/mod.rs).** Staging truth is derived from the porcelain XY
index column: new `GitTuiState::staged_from_entries()` collects paths with
`xy[0] != ' ' && xy[0] != '?'`, applied on `load()` and every `refresh()`.
`toggle_stage` keeps its contains-check against the derived set (now
correct for external staging) and its post-command `refresh()` re-syncs
the mirror; the manual insert/remove bookkeeping is deleted.

**Regression test.** `tests::externally_staged_files_are_operable` — temp
repo, `run_git(["add", …])` external staging, `GitTuiState::load` sees the
file in `staged`, `toggle_stage` unstages it and the porcelain index
column returns to `' '`.

**Command.** same git_tui run as finding 3.

## Finding 5 — IMPORTANT: allocator emits mid-range allocations

**Root cause.** The pressure branch distributed surplus as
`rows = 1 + extra`, which could land in `3..natural-1`
(e.g. rows = 3 for a height-4 block, pinned by the old test's
"folded card (2-row fold = 3)" label). `render_transcript` treats
`alloc.rows >= 3` as full natural render, so Σrendered rows could exceed
the budget.

**Fix (oxicode-vtui/presentation/allocation.rs).** The pressure loop now
quantizes each block's share to a renderable shape: full natural height
(when `remaining >= natural − 1`), the 2-row folded card, or the 1-row
glyph floor — never `3..natural-1`. Module docs updated to state the
quantized contract.

**Tests.**
- `pressure_folds_oldest_first` updated: `[5,4,3]` budget 7 → `[2,2,3]`
  (was the forbidden `[1,3,3]`).
- `single_block_taller_than_budget_folds` updated: `[10]` budget 4 → 2
  (was the forbidden 4).
- New `pressure_never_allocates_mid_range_rows`: pinned cases
  (`[10]`/4 → 2, `[10]`/9 → 2, `[10]`/10 → 10) plus an exhaustive sweep
  over budgets 0..=30 × heights 3..=8 asserting every allocation is
  `rows <= 2 || rows == natural` and the sum stays within budget.
- `roomy_all_full` and the emergency tests unchanged and still green.

**Command.**
```
cargo nextest run -p oxicode-vtui presentation
→ 96 passed, 2 skipped (platform-gated), incl. the new sweep test
```

## Finding 6 — IMPORTANT: keymap half-real for 4 actions + scalar yml rejected

**Root cause (a).** Submit / ScrollUp / ScrollDown / Help were only
consulted inside hardcoded arms (`KeyCode::Enter`, guarded
`KeyCode::PageUp`/`PageDown`, `KeyCode::Char`), so rebinding
`submit: alt+s` never fired — the capability was "disable at the original
key", not remap.

**Fix (main_loop.rs).** New `keymap_pre_match(keymap, key, multiline)` +
`KeymapDispatch` consulted right before the hardcoded `match key.code`
(and after the confirmation/overlay/file-search/git-modal handlers, so
modal keys keep priority). Submit/ScrollUp/ScrollDown/Help bound keys are
consumed there; unmatched keys fall through to the arms, which are now
pure fallback. Two muscle-memory carve-outs encoded and tested:
- plain Enter in multiline still falls through (the Enter arm inserts the
  newline; only the *send* path is remappable);
- printable Help bindings (default `?`) stay with the Char arm's
  empty-composer gate so typing `?` inside text inserts it;
  non-printable rebinds (e.g. `help: ctrl+pageup`) dispatch generically.
The duplicated harvest/popup/history submit body was extracted into
`harvest_and_clear_input` (shared with SubmitNow), the dead
`PageUp`/`PageDown` arms were removed, and the Char-arm Help check reuses
a shared `cheatsheet_overlay()`.

**Root cause (b).** `KeySpec`'s Deserialize was map-only
(`deny_unknown_fields`), so the plan-documented scalar form
(`submit_now: ctrl+enter`) failed the whole file → silent full revert.

**Fix (keymap.rs).** Overlay values deserialize into a new untagged
`StringOrKeySpec { Map(KeySpec), Str(String) }`; scalar strings are parsed
by the new `parse_scalar_combo` (`ctrl`/`control`, `alt`/`option`,
`shift` `+`-separated modifiers + a `parse_key_string` key token; garbage
errors are logged and skipped per-entry, not fatal to the file).

**Regression tests.**
- keymap.rs: `overlay_accepts_scalar_string_form`,
  `overlay_mixed_forms_parse_together`,
  `scalar_combo_parser_rejects_garbage`; existing
  `defaults_match_current_hardcoded_keys` still pins the defaults.
- main_loop.rs `keymap_dispatch_tests`:
  `rebound_submit_fires_through_generic_dispatch` (alt+s Submit fires,
  multiline included),
  `default_submit_dispatch_preserves_muscle_memory` (Enter/Shift+Enter
  multiline semantics unchanged),
  `scroll_and_help_dispatch_via_keymap` (PageUp/PageDown defaults,
  rebound `scroll_up: ctrl+u`, printable-vs-non-printable Help split).

**Command.**
```
cargo nextest run -p oxicode-cli keymap
→ 10 keymap tests + 3 dispatch tests PASS
```

## Dead code removed (final-review extras)

- `block_items_left` (main_loop.rs `render_transcript`): write-only —
  declaration, three assignments, and the decrement deleted; the ladder
  branch restructured to `if alloc.rows < natural { … }` with the roomy
  path as fall-through (behavior unchanged: the variable was never read).
- `_total_blocks`: `compute_block_allocations` now returns a 3-tuple
  (`total_blocks` stays as a local for `with_capacity`); call site and
  doc comment updated.
- `cut_here` (oxicode-vtui/tui/ui/clamp.rs): variable + `let _ =`
  discarded; the `break` (the actual behavior) stays.
- `let stop = n` alias (allocation.rs): gone with the pressure-loop
  rewrite (`for i in (0..n).rev()`).

## Verification

Run in the worktree on the final tree (all green, pristine):

```
cargo fmt --all -- --check                                   → clean
cargo clippy --workspace --all-targets -- -D warnings        → clean
cargo clippy -p oxicode-cli --all-targets -- -D warnings     → clean
cargo nextest run -p oxicode-cli -p oxicode-vtui
  → Summary: 1185 tests run: 1185 passed, 3 skipped (platform-gated)
```

New/changed tests: 18 across the two crates
(git_tui 31 total incl. 2 new + 2 extended; oxicode-vtui allocator 6
incl. 1 new sweep + 2 updated; keymap 10 incl. 3 new; main_loop dispatch
3 new + resize sentinel 1 extended).

## Addendum — re-review round 2

The re-review caught that the wave's mangled-edit repair had deleted the
`#[test]` attribute above `single_block_taller_than_budget_folds`
(oxicode-vtui/src/presentation/allocation.rs), silently retiring the test
(focused nextest matched 0 tests). Restored:

```
cargo nextest run -p oxicode-vtui single_block_taller
→ Summary: 1 test run: 1 passed   (single_block_taller_than_budget_folds)
```

Post-fix gates (all green, tree clean):

```
cargo fmt --all -- --check                            → clean
cargo clippy --workspace --all-targets -- -D warnings → clean
cargo nextest run -p oxicode-cli -p oxicode-vtui
  → Summary: 1186 tests run: 1186 passed, 3 skipped
```

The restore is folded into the amended fix-wave commit (same message);
new SHA `e7af32ad` (docs commit `2789c564` on top).
