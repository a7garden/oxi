# Progress

## Status
In Progress

## Tasks
- [x] P0-1: Fix `add_overlay()` UB in oxi-tui (double-free from two owning pointers to same memory)

## Files Changed
- `oxi-tui/src/overlay.rs` — Added Component passthrough methods to `OverlayHandle` trait; fixed `OverlayBox::render()` backdrop overfill
- `oxi-tui/src/tui.rs` — Simplified `OverlayHandleWrapper` to single `Box<dyn OverlayHandle>`; removed all `unsafe` code

## Notes
- All 313 unit tests + 12 doc tests pass
- Backdrop rendering intentionally removed from `OverlayBox::render()` — needs proper area management to be correct
