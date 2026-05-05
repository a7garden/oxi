# Progress

## Status
In Progress

## Tasks

### P2-2: Add IME/Cursor Support to oxi-tui ✅
- Added `Event::CursorPosition(u16, u16)` variant to event.rs
- Added `Terminal::query_cursor_position()` and `Terminal::set_ime_cursor()` to terminal trait + CrosstermTerminal impl
- Added `cursor_position: Option<(u16, u16)>` field to Renderer with `set_cursor_position()` method
- Updated `Renderer::flush()` to emit cursor positioning + show cursor after content when IME position is set
- Added `cursor_marker_pending: bool` to TUI
- Added `TUI::request_cursor_position_query()`, `TUI::set_ime_cursor()`, `TUI::clear_ime_cursor()`
- Updated `TUI::poll_event()` to handle pending cursor position queries via terminal's cursor_pos()

## Files Changed
- `oxi-tui/src/event.rs` — Added `Event::CursorPosition(u16, u16)` variant
- `oxi-tui/src/terminal.rs` — Added `query_cursor_position()` and `set_ime_cursor()` to Terminal trait and CrosstermTerminal
- `oxi-tui/src/renderer.rs` — Added `cursor_position` field, `set_cursor_position()` method, updated flush to position cursor for IME
- `oxi-tui/src/tui.rs` — Added `cursor_marker_pending`, `request_cursor_position_query()`, `set_ime_cursor()`, `clear_ime_cursor()`, updated poll_event

## Notes
- Crossterm internally parses `ESC[row;colR` as `InternalEvent::CursorPosition` but its public EventFilter discards it. The implementation uses `Terminal::cursor_pos()` as a synchronous fallback when a pending cursor query has no regular event available.
- All 350 unit tests + 12 doc tests pass.
