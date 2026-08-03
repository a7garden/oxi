# oxicode-tui Guide

This guide covers the production tape rendering model and the retained ratatui widget layer.

## Production Rendering Model

Ordinary chat rendering uses the main screen:

1. `oxicode-cli` projects `ChatViewState` into tape rows via `TranscriptRenderer`.
2. `TapeEngine` commits finalized rows to native scrollback and repaints only the mutable/sticky suffix.
3. Transient overlays alone enter the alternate screen with ratatui.

The ratatui widgets remain the source of truth for line formatting. Tape components convert ratatui `Line` output into ANSI rows.

## Key Modules

- `tape/engine.rs` — append-only native-scrollback engine.
- `tape/container.rs` — memoized child composition keyed by revision and width.
- `tape/transcript.rs` — chat message projection into tape rows.
- `tape/style.rs` — ratatui style to ANSI conversion with capability downgrade.
- `widgets/` — ratatui widgets and formatting helpers used by overlays and transcript projection.

## Adding a Tape Component

1. Reuse existing widget formatting when possible.
2. Convert styled ratatui lines through `styled_line_to_ansi`.
3. Read glyphs from `ThemeStyles.symbols`; never hardcode UI glyphs.
4. Return a stable `revision()` that changes only when rendered output changes.
