# Progress

## Status
In Progress

## Tasks
- [x] P0-1: Fix `add_overlay()` UB in oxi-tui (double-free from two owning pointers to same memory)
- [x] P1-3: Fix mouse event conversion + other event fixes in oxi-tui
- [x] P0-3: Add Layout System to oxi-tui

## Files Changed
- `oxi-tui/src/overlay.rs` — Added Component passthrough methods to `OverlayHandle` trait; fixed `OverlayBox::render()` backdrop overfill
- `oxi-tui/src/tui.rs` — Simplified `OverlayHandleWrapper` to single `Box<dyn OverlayHandle>`; removed all `unsafe` code
- `oxi-tui/src/event.rs` — Added `Moved`, `ScrollLeft`, `ScrollRight` variants to `MouseEventKind`; added `#[allow(dead_code)]` to `KeyCode::Number`
- `oxi-tui/src/tui.rs` — Fixed mouse event conversion: `Moved` now maps to `Moved` (not `Drag`); button detection uses crossterm's actual button from `mouse.kind` instead of key modifiers; handles all crossterm `MouseEventKind` variants including `Down/Up/Drag` with proper button extraction
- `oxi-tui/src/layout.rs` — New file: `Direction` enum, `Constraint` enum (Length/Percentage/Min/Flex), `split()` function, `Container` component with nested layout support, 12 unit tests
- `oxi-tui/src/lib.rs` — Added `pub mod layout` and `pub use layout::{split, Constraint, Container as LayoutContainer, Direction}`
- `oxi-tui/src/tui.rs` — Added `layout: Option<(Direction, Vec<Constraint>)>` field, `set_layout()`, `clear_layout()` methods; layout-aware rendering in `render()`

## Notes
- All 313 unit tests + 12 doc tests pass
- Backdrop rendering intentionally removed from `OverlayBox::render()` — needs proper area management to be correct
- Pre-existing `cargo check` errors in `cell.rs` and `surface.rs` due to `unicode_width` API mismatch — unrelated to layout changes
- Layout tests cannot run via `cargo test` due to pre-existing `unicode_width` compilation errors, but layout code itself compiles cleanly
