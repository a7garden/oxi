# Handoff: `/providers` overlay-chain regression fix

**Date:** 2026-08-12
**Status:** Fixed and verified
**Affected:** `oxicode-cli/src/tui_vt/main_loop.rs`
**Branch:** `main` (local, ahead of `origin/main` by 8 commits)

## Symptom

Inside the TUI:

1. User types `/providers` and presses Enter.
2. The provider list overlay renders.
3. User picks a provider row (Down + Enter).
4. Expected: an action menu (multi-action chain) or a secure prompt (single-action chain) opens.
5. Actual: nothing appears. The provider list closes and the user is back at the composer with no panel to configure.

## Root cause

The `OverlayEvent::Submitted` arm in `handle_inline_event`
(`oxicode-cli/src/tui_vt/main_loop.rs`, around line 1689) ended with an
unconditional `handle.close_overlay()`:

```rust
state.overlay_catalog_models.clear();
state.overlay_providers.clear();
state.overlay_model_ids.clear();
handle.close_overlay();   // ← always fires
```

The `Submitted` arm also opens a fresh overlay in two branches:

- `ProviderRow` multi-action → `handle.show_list_modal("Pick an action", ...)`
- `ProviderRow` / `ProviderAction` single-action → `handle_auth_action(SetApiKey)` which `handle.show_modal(...)` for the secure prompt

`ShowOverlay` and `CloseOverlay` both enqueue onto the same `cmd_rx`
channel and are processed in submit order. The new overlay opens, then
the stale `close_overlay()` clears it — leaving the user with nothing.

The unconditional close was introduced in commit `646b2801` ("fix(tui):
restore overlay cleanup in Submitted arm") to clear stale
`state.overlay_providers` / `state.overlay_model_ids` /
`state.overlay_catalog_models` after dispatch. The added `close_overlay()`
was over-broad — it should have cleared the *state* but not the *overlay*
when the handler had already opened a replacement.

`git blame`:
- `state.overlay_*` clears: `92d693b3` (oxi, 2026-08-07)
- `state.overlay_model_ids.clear() + handle.close_overlay()`: `646b2801` (oxi, 2026-08-10)

## Fix

1. `handle_auth_action` now returns `bool` (`true` iff the dispatched
   action opened a new overlay via `handle.show_*`). Only `SetApiKey`
   opens a new overlay (the secure prompt modal). `StartOAuth` spawns
   an async task asynchronously, and `RemoveKey` sets
   `state.confirmation` (a separate field, not `state.overlay`), so both
   return `false`.

2. The `OverlayEvent::Submitted` arm tracks a local `opened_new_overlay`
   flag. The two relevant call sites use `opened_new_overlay |= …` to
   capture the result. The `ProviderRow` multi-action branch sets
   `opened_new_overlay = true` after `show_list_modal`.

3. The trailing `close_overlay()` is gated on `!opened_new_overlay`. The
   stale-state cleanup (clearing `state.overlay_*`) still runs
   unconditionally so the next overlay dispatch starts from a clean slate.

## Verification

- **Unit tests** (`provider_overlay_tests` module in `main_loop.rs`):
  - `provider_row_opens_action_menu_without_close` — openai (OAuth-capable, no key) → multi-action menu opens via `ShowOverlay`, no trailing `CloseOverlay`, `overlay_providers` cleared.
  - `provider_row_set_api_key_opens_secure_prompt_without_close` — cerebras (key-only) → `SetApiKey` opens secure prompt via `ShowOverlay`, no trailing `CloseOverlay`, `secure_input_target` stashed.
  - `catalog_model_selection_still_closes_overlay` — pins the working baseline; non-ProviderRow branches still close.

  Each test was independently verified to fail with the unconditional
  `close_overlay()` and pass with the gated version.

- **Full test suite:** `cargo nextest run --workspace` → 3351 passed, 4 skipped.
- **Clippy:** `cargo clippy -p oxicode-cli --all-targets -- -D warnings` → clean.
- **Formatter:** `cargo fmt -p oxicode-cli` → no changes.

## Why not a PTY e2e test?

I tried adding a PTY e2e test (`tests/pty_e2e.rs`) that spawns the
`oxicode` binary, opens `/providers`, presses Enter, and looks for the
action menu text in the captured PTY output. The test passed both
*with* and *without* the fix because the PTY output buffer captures the
brief render of the action menu (the render tick is ~50ms, the bug
closes the overlay within that window, but the rendered bytes are still
written to the PTY before the close takes effect). The unit test
inspects the command channel sequence directly, which is the correct
level for this bug.

## Files changed

- `oxicode-cli/src/tui_vt/main_loop.rs` — 3 blocks:
  1. `handle_auth_action` now returns `bool`.
  2. `OverlayEvent::Submitted` arm adds `opened_new_overlay` flag and gates the trailing `close_overlay()`.
  3. New `provider_overlay_tests` module with 3 tests.

## Commit plan (suggested)

```
fix(tui): keep newly-opened overlay after /providers row selection

The `OverlayEvent::Submitted` arm closed the current overlay
unconditionally after dispatch, even when the handler had just opened
a replacement (the action menu after `/providers` row selection, or
the secure prompt after `SetApiKey`). The cmd channel processes
`ShowOverlay` and `CloseOverlay` in submit order, so the trailing
`close` wiped the freshly-opened overlay.

Gate the trailing `close_overlay()` on a new `opened_new_overlay` flag
that the handler sets whenever it opens a new overlay via
`handle.show_*`. The stale-state cleanup that motivated the prior
"restore overlay cleanup" fix still runs unconditionally.

Tests (provider_overlay_tests in main_loop.rs) cover both the
multi-action and single-action paths and pin the working baseline
for catalog model selection.
```

## Follow-up considerations

- The provider-row `Submitted` arm runs through `handle_inline_event`
  which is also a caller of `session` (for `Model`, `CatalogModel`,
  `ConfigAction`). All `session.set_model` / `cycle_*` calls remain
  session-driven and unaffected by this change.
- If a future overlay type adds a new `Submitted` branch that opens a
  fresh overlay, it must also set `opened_new_overlay = true` (or use
  `|=` against a `handle_auth_action`-style returning helper). The
  inline comment above the flag documents this contract.
- `clippy::if_not_else` / `clippy::needless_bool` may flag the
  `if !opened_new_overlay { close_overlay() }` pattern. The current
  code keeps the explicit `if` for readability; if clippy starts
  complaining, prefer `if opened_new_overlay { /* skip */ } else { … }`.
