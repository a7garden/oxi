<div align="center">

# oxi-tui

**ratatui-based widgets, theming, and a differential-rendering backend for oxi.**

[![Version](https://img.shields.io/badge/Version-0.39.0-blue?style=flat-square)](https://crates.io/search?q=oxi)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](../LICENSE.md)
[![Docs](https://img.shields.io/docsrs/oxi-tui/latest?style=flat-square)](https://docs.rs/oxi-tui)

</div>

---

## Overview

`oxi-tui` is a **widget and theming library** built on top of
[`ratatui`](https://docs.rs/ratatui) 0.30 + [`crossterm`](https://docs.rs/crossterm)
0.29. It does **not** ship its own event loop or terminal driver — the host
application owns its `ratatui::Terminal` (typically wrapped in our
[`DiffBackend`](#differential-rendering)) and renders our widgets into it.

It provides:

- **A differential-rendering backend** (`DiffBackend`) that wraps
  `CrosstermBackend`, diffs frame buffers line-by-line, and flushes inside a
  CSI 2026 synchronized-update window — flicker-free streaming without the
  per-frame I/O cost of a full repaint.
- **A typed theme system** (`Theme` / `ThemeManager` / `ThemeStyles`) with
  hot-reload from TOML or JSON, plus Catppuccin-inspired dark/light presets.
- **A glyph-set system** (`GlyphSet`: Unicode / ASCII / Nerd Font) so the entire
  UI degrades cleanly from emoji-rich terminals down to 7-bit serial consoles.
- **Production widgets**: a streaming `ChatView`, multi-line `Input`, fuzzy
  `CompletionPopup`, `DashboardWidget`, `Footer`, `RoutingStatus`, `TodoPanel`,
  and generic `StatefulList` / `TableList`.
- **Rendering helpers**: terminal capability detection, inline image (Kitty /
  iTerm2) decoding, ANSI-width text utilities, unified-diff coloring, and
  Mermaid → ASCII diagram rendering.
- **CJK-aware line wrapping** and wide-character-safe cell writes
  (`Buffer::set_line`) — ratatui's built-in `WordWrapper` does not CJK-break.

> oxi-tui has **no `oxi-*` dependencies**. It is a pure widget library any
> ratatui application can adopt.

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
oxi-tui = { path = "path/to/oxi-tui" }
ratatui = "0.30"
crossterm = "0.29"
```

The host owns the terminal; `oxi-tui` contributes the backend, the theme, and
widgets:

```rust
use oxi_tui::{Theme, ThemeManager};
use oxi_tui::render::DiffBackend;
use oxi_tui::widgets::chat::{ChatView, ChatViewState};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::stdout;

fn main() -> anyhow::Result<()> {
    let backend = DiffBackend::new(CrosstermBackend::new(stdout()));
    let mut terminal = Terminal::new(backend)?;

    let manager = ThemeManager::dark();   // hot-reloadable theme manager
    let mut state = ChatViewState::default();

    loop {
        // `manager.theme()` returns an owned `Theme` clone; re-fetch after a
        // hot-reload (`manager.check_reload()`) to pick up the new colors.
        let theme = manager.theme();
        let mut frame = terminal.draw(|f| {
            f.render_stateful_widget(ChatView::new(&theme), f.area(), &mut state);
        })?;
        // … read crossterm events, mutate `state`, break on quit …
        # break;
    }
    Ok(())
}
```

## Architecture

```
oxi-tui
├── render/      DiffBackend (line-diff + CSI 2026), capability probe,
│                inline image, ANSI/diff coloring, Mermaid ASCII
├── theme.rs     Theme, ColorScheme, Spacing, ThemeStyles, ThemeManager
├── symbols.rs   GlyphSet (Unicode/ASCII/Nerd) → Symbols table
├── widgets/     ratatui widgets (chat, input, completion, dashboard, …)
├── keybindings/ declarative key registry + conflict detection
└── lib.rs       re-exports the public surface
```

Each widget is a ratatui `StatefulWidget` (or `Widget`) with a sibling
`*State`. The widget layer defines its own domain types
(`ChatMessage`, `MessageRole`, `ContentBlock`, `ToolCallStatus`) so it can be
reused by any product; products implement the conversion in their own
composition root.

## Differential Rendering

`DiffBackend<W>` implements `ratatui::backend::Backend`. On each `draw()`:

1. Ratatui builds the frame buffer as usual.
2. The backend collects cells into per-row byte rows and checksums each.
3. Only rows whose checksum differs from the previous frame are written.
4. The whole repaint is wrapped in `\x1b[?2026h … \x1b[?2026l` (synchronized
   output) to eliminate mid-frame tearing.

```rust
use oxi_tui::render::DiffBackend;
use ratatui::{Terminal, backend::CrosstermBackend};

let backend = DiffBackend::new(CrosstermBackend::new(stdout));
let mut terminal = Terminal::new(backend)?;

// Force a full repaint on the next frame (e.g. after a resize you handled):
backend_ref.invalidate();
```

This matters most for streaming AI chat, where the overwhelming majority of the
screen is static between frames. The `ChatView` widget additionally caches its
computed layout and only recomputes when messages / width / spinner change.

## Theme System

```rust
use oxi_tui::{Theme, ThemeManager};

let mut manager = ThemeManager::dark();        // or ::light(), ::new(theme)

// Hot-reload a file: applies it immediately, then polls for changes.
manager.watch_file("themes/my-theme.toml")?;

// In your event loop:
if manager.check_reload() {
    terminal.draw(...)?;                       // theme changed — repaint
}

manager.set_theme_by_name("light");            // switch programmatically
let theme = manager.theme();                   // &Theme for widget constructors
```

A `Theme` is a typed `ColorScheme` + `Spacing` + active `Symbols`. Widgets
consume `theme.to_styles()` → `ThemeStyles` (a `Copy` bundle of `ratatui::Style`s),
so there are zero allocations per frame for styling.

### Color formats (theme files)

Hex, named, or indexed — accepted in both TOML and JSON:

```toml
name = "midnight"
[colors]
foreground = "#cdd6f4"
background = "#1e1e2e"
primary    = "#89b4fa"
error      = "#f38ba8"
success    = "#a6e3a1"
```

```json
{
  "name": "midnight",
  "colors": { "foreground": "#cdd6f4", "background": "#1e1e2e" }
}
```

`Color` accepts `#rrggbb` / `#rgb`, named ANSI (`red`, `bright-black`), or
indexed (`i42`).

## Glyph Sets

One setting switches every box-drawing, status, and spinner glyph across the
whole UI, so the app renders correctly from a Nerd-Font terminal down to a
7-bit serial console:

```rust
use oxi_tui::{GlyphSet, Symbols};

let symbols = Symbols::unicode();   // default — works on any UTF-8 terminal
let symbols = Symbols::ascii();     // 7-bit fallback for CI logs / serial
let symbols = Symbols::nerd();      // richest icons; needs a patched font
```

`GlyphSet` is stored in `settings.toml` as `glyph_set = "unicode"`. `Symbols`
is `Copy` and rides inside `ThemeStyles` with no allocation.

## Widgets

| Widget | State type | Notes |
|--------|------------|-------|
| `chat::ChatView` | `ChatViewState` | Streaming message list, tool-call boxes, markdown, layout cache |
| `Input` | `InputState` | Multi-line editor (`ratatui-textarea`), undo/redo, history |
| `CompletionPopup` | `CompletionState` | Fuzzy file/slash-command autocomplete |
| `DashboardWidget` | `DashboardState` | Sectioned status dashboard |
| `Footer` | `FooterState` | Model / provider / context status line |
| `RoutingStatus` | `RoutingStatusState` | Auto-routing & fallback panel |
| `TodoPanel` | `TodoPanelState` | Phased todo list |
| `StatefulList<T>` | — | Generic filterable list with ratatui list state |
| `TableList<T>` | — | Generic multi-column table with selection |

Plus `tool_renderer` (state-aware formatting of tool calls / results / diffs)
and `table_renderer` (compact table → line rendering).

## Keybindings

A declarative layer over crossterm key events, with normalized `KeyId`,
a default `Action` registry, user rebinding, and conflict detection:

```rust
use oxi_tui::keybindings::{KeybindingsManager, keys::KeyId, registry::Action};

let mgr = KeybindingsManager::new();
let key_id = KeyId::from(crossterm_event);
match mgr.match_action(&key_id) {
    Some(Action::Submit) => { /* ... */ }
    Some(Action::Quit)   => { /* ... */ }
    _ => {}
}
```

## Rendering Helpers

- **Terminal capabilities** (`render::terminal`): detects image protocol
  (Kitty / iTerm2), true color, OSC 8 hyperlinks, Kitty keyboard protocol, and
  cell size.
- **Inline images** (`render::image`): PNG / JPEG / GIF / WebP decode and
  Kitty / iTerm2 inline emission.
- **ANSI utilities** (`text`, `render::ansi`): display-width-aware truncation,
  wrapping, and string-width that ignore SGR escapes.
- **Diffs** (`render::diff`): colored unified-diff rendering with `+`/`-`
  stat counting.
- **Mermaid** (`render::mermaid`): pure-Rust Mermaid → ASCII diagram renderer
  (flowchart, sequence, state, class) with a process-level render cache. No
  external `mmdc` / Node dependency — single-binary self-contained.

## Conventions

- `parking_lot::RwLock` over `std::sync::RwLock`; never hold a guard across an
  `.await`.
- Atomic file writes use the temp + rename pattern.
- Shipped (non-test) code `warn`s on `clippy::unwrap_used`; tests relax that one
  lint via `#![cfg_attr(test, allow(...))]`.

## License

[MIT](../LICENSE.md)
