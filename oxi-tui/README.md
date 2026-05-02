# oxi-tui

Terminal UI library for Rust — component framework, differential rendering, and theme system.

## Overview

`oxi-tui` provides a lightweight, component-based terminal UI framework designed for building interactive applications. It features:

- **Component trait** — composable UI building blocks with focus management
- **Differential rendering** — only redraws changed lines for flicker-free updates
- **Theme system** — customizable colors, fonts, and spacing with hot-reload from TOML/JSON
- **Built-in components** — `Text`, `Input`, `Editor`, `Markdown`, `Completion`
- **Overlay system** — modal dialogs and popovers
- **Event abstraction** — unified keyboard, mouse, and resize events

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
oxi-tui = { path = "path/to/oxi-tui" }
```

Basic usage:

```rust
use oxi_tui::{TUI, Text};

fn main() -> anyhow::Result<()> {
    let mut tui = TUI::with_crossterm()?;

    let greeting = Text::new("Hello, terminal!");
    tui.add_child(greeting);

    tui.start()?; // Enters event loop, renders until stop() is called
    Ok(())
}
```

## Component Overview

### The Component Trait

All UI elements implement the `Component` trait:

```rust
pub trait Component: Send {
    fn name(&self) -> &str;
    fn request_render(&mut self);
    fn is_dirty(&self) -> bool;
    fn clear_dirty(&mut self);
    fn handle_event(&mut self, event: &Event) -> bool;
    fn render(&mut self, surface: &mut Surface, area: Rect);
    fn min_size(&self) -> Size;
    fn desired_size(&self) -> Option<Size> { None }

    // Focus management
    fn on_focus(&mut self) {}
    fn on_unfocus(&mut self) {}
    fn is_focused(&self) -> bool { false }
    fn focus(&mut self) { self.on_focus(); }
    fn unfocus(&mut self) { self.on_unfocus(); }
}
```

Components render into a `Surface` (a grid of `Cell`s), and the `TUI` event loop handles diffing and flushing to the terminal.

### Built-in Components

#### Text

Static text display:

```rust
use oxi_tui::Text;

let text = Text::new("Hello, world!");
```

#### Input

Single-line text input with configurable options:

```rust
use oxi_tui::{Input, InputOptions};

let input = Input::new(InputOptions {
    placeholder: Some("Type here...".into()),
    ..Default::default()
});
```

#### Editor

Multi-line text editor with mention support:

```rust
use oxi_tui::{Editor, EditorOptions, Mention};

let editor = Editor::new(EditorOptions {
    placeholder: Some("Write your message...".into()),
    ..Default::default()
});
```

#### Markdown

Renders Markdown content with syntax highlighting:

```rust
use oxi_tui::{Markdown, MarkdownTheme};

let md = Markdown::new("# Hello\n\nThis is **markdown**.");
```

#### Completion

Autocomplete popup for file paths and custom suggestions:

```rust
use oxi_tui::{Completion, FileCompleter};

let completer = FileCompleter::new();
let popup = Completion::new(completer);
```

### Custom Components

Implement the `Component` trait:

```rust
use oxi_tui::{Component, Surface, Rect, Event, Size};

struct Counter {
    count: u32,
    dirty: bool,
}

impl Counter {
    fn new() -> Self {
        Self { count: 0, dirty: true }
    }
}

impl Component for Counter {
    fn request_render(&mut self) { self.dirty = true; }
    fn is_dirty(&self) -> bool { self.dirty }
    fn clear_dirty(&mut self) { self.dirty = false; }

    fn handle_event(&mut self, event: &Event) -> bool {
        if let Event::Key(key) = event {
            if key.code == KeyCode::Char('+') {
                self.count += 1;
                self.request_render();
                return true;
            }
        }
        false
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let text = format!("Count: {}", self.count);
        for (i, ch) in text.chars().enumerate() {
            if i as u16 >= area.width { break; }
            surface.set(area.row, area.col + i as u16, Cell::new(ch));
        }
        self.clear_dirty();
    }

    fn min_size(&self) -> Size {
        Size { width: 10, height: 1 }
    }
}
```

## Theme System

### Built-in Themes

Two themes are included:

```rust
use oxi_tui::Theme;

let dark = Theme::dark();   // Catppuccin-inspired dark theme
let light = Theme::light(); // Catppuccin-inspired light theme
```

### Theme Structure

A theme consists of three parts:

```rust
pub struct Theme {
    pub name: String,
    pub colors: ColorScheme,   // Semantic color palette
    pub fonts: FontScheme,     // Text styles (bold, italic, etc.)
    pub spacing: Spacing,      // Padding, margin, border widths
}
```

### Color Scheme

The `ColorScheme` provides semantic colors:

| Field | Purpose |
|-------|---------|
| `foreground` | Default text color |
| `background` | Default background |
| `primary` | Primary accent |
| `secondary` | Secondary accent |
| `error` | Error messages |
| `warning` | Warning messages |
| `success` | Success indicators |
| `muted` | Dimmed/placeholder text |
| `accent` | Highlight color |
| `border` | Borders and separators |
| `cursor_fg` | Cursor text color |
| `cursor_bg` | Cursor background |
| `selection_bg` | Selected text background |

Colors support multiple formats:

```rust
use oxi_tui::Color;

Color::Rgb(220, 223, 228)     // True color: #dcdfe4
Color::Indexed(8)              // 256-color palette index
Color::Red                     // Named ANSI colors
Color::Default                 // Terminal default
```

### Loading Custom Themes

Themes can be loaded from TOML or JSON files:

**TOML** (`my-theme.toml`):
```toml
name = "midnight"
[colors]
foreground = "#cdd6f4"
background = "#1e1e2e"
primary = "#89b4fa"
error = "#f38ba8"
success = "#a6e3a1"
```

**JSON** (`my-theme.json`):
```json
{
  "name": "midnight",
  "colors": {
    "foreground": "#cdd6f4",
    "background": "#1e1e2e",
    "primary": "#89b4fa"
  }
}
```

Color values accept: hex (`#rrggbb`, `#rgb`), named (`red`, `bright-black`), or indexed (`i42`).

### Theme Manager with Hot-Reload

```rust
use oxi_tui::{ThemeManager, Theme};

// Start with dark theme
let mut manager = ThemeManager::dark();

// Watch a file for changes (auto-reloads on modification)
manager.watch_file("themes/my-theme.toml")?;

// In your event loop, check for reloads:
if manager.check_reload() {
    // Theme was reloaded — trigger a re-render
    tui.request_render();
}

// Get the current theme
let theme = manager.theme();

// Switch programmatically
manager.set_theme_by_name("light");
```

## Rendering

### Differential Rendering

The TUI uses a double-buffered, differential rendering approach:

1. Each component renders into a `Surface` (grid of `Cell`s)
2. The renderer compares the new surface against the previous frame
3. Only changed lines are written to the terminal
4. Synchronized output (`CSI 2026`) prevents flickering

```rust
pub enum RenderStrategy {
    Full,        // First render or terminal resize
    Incremental, // Only dirty lines
}
```

### Surface

A `Surface` is a 2D grid of `Cell`s:

```rust
use oxi_tui::{Surface, Cell, Rect};

let mut surface = Surface::new(80, 24);

// Set individual cells
surface.set(0, 0, Cell::new('H'));

// Get cell
let cell = surface.get(0, 0);

// Track dirty regions
surface.mark_dirty(0);           // Mark row 0 as dirty
let first = surface.first_dirty(); // First dirty row
let last = surface.last_dirty();   // Last dirty row
```

### Cells

```rust
use oxi_tui::{Cell, CellBuilder, Color, Attributes};

let cell = CellBuilder::new()
    .character('A')
    .foreground(Color::Rgb(255, 136, 0))
    .background(Color::Rgb(30, 30, 44))
    .attributes(Attributes::new().with_bold())
    .build();
```

### Overlay System

Overlays render on top of all other components and capture input first:

```rust
use oxi_tui::{OverlayOptions, OverlayContent};

// Add an overlay
let id = tui.add_overlay(my_modal, OverlayOptions::default());

// Remove an overlay
tui.remove_overlay(id);

// Remove all overlays
tui.clear_overlays();

// Escape key closes the top overlay by default
```

## Event Handling

### Event Types

```rust
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(ResizeEvent),
    FocusGained,
    FocusLost,
    None,
}
```

### Key Events

```rust
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

pub enum KeyCode {
    Char(char), Enter, Escape, Tab, Backspace, Delete,
    Up, Down, Left, Right, Home, End, PageUp, PageDown,
    Insert, F(u8),
}

pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}
```

### Mouse Events

```rust
pub struct MouseEvent {
    pub kind: MouseEventKind,  // Click, Drag, ScrollDown, ScrollUp
    pub button: MouseButton,   // Left, Middle, Right
    pub row: u16,
    pub col: u16,
}
```

### Focus Management

Tab cycles focus between child components. Shift+Tab cycles backwards.

```rust
// Set focus programmatically
tui.set_focus(2);

// Get current focus index
let idx = tui.focus_index();
```

### Custom Event Handler

```rust
tui.on_event(|event| {
    if let Event::Key(key) = event {
        if key.code == KeyCode::Char('q') && key.modifiers.ctrl {
            // Handle Ctrl+Q
        }
    }
});
```

## License

MIT
