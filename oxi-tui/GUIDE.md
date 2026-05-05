# oxi-tui Guide

This guide covers the component lifecycle, rendering pipeline, and key systems in `oxi-tui`.

## Component Lifecycle

Every UI component follows this lifecycle:

```
new()  ──────►  render()  ──────►  handle_event()  ──────►  render()  ──────►  ...
              (draw to          (process input)          (re-draw with
               surface)                                   new state)
```

### Methods

```rust
pub trait Component: Send {
    /// Create and configure the component
    fn new() -> Self;
    
    /// Request a render on the next frame
    fn request_render(&mut self);
    
    /// Check if component needs re-rendering
    fn is_dirty(&self) -> bool;
    
    /// Clear dirty flag after render
    fn clear_dirty(&mut self);
    
    /// Handle user input, return true if consumed
    fn handle_event(&mut self, event: &Event) -> bool;
    
    /// Draw to the surface
    fn render(&mut self, surface: &mut Surface, area: Rect);
    
    /// Minimum size this component needs
    fn min_size(&self) -> Size;
    
    /// Desired size (optional override)
    fn desired_size(&self) -> Option<Size> { None }
    
    /// Called when component gains focus
    fn on_focus(&mut self) {}
    
    /// Called when component loses focus
    fn on_unfocus(&mut self) {}
}
```

### Lifecycle Example

```rust
struct MyComponent {
    text: String,
    dirty: bool,
}

impl Component for MyComponent {
    fn new() -> Self {
        Self { text: String::new(), dirty: true }
    }
    
    fn request_render(&mut self) {
        self.dirty = true;
    }
    
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    
    fn clear_dirty(&mut self) {
        self.dirty = false;
    }
    
    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(KeyEvent { code: KeyCode::Char(c), .. }) => {
                self.text.push(*c);
                self.request_render();  // Re-render after change
                true
            }
            _ => false,
        }
    }
    
    fn render(&mut self, surface: &mut Surface, area: Rect) {
        surface.write_string(area.y, area.x, &self.text);
        self.clear_dirty();
    }
    
    fn min_size(&self) -> Size {
        Size { width: 10, height: 1 }
    }
}
```

## Rendering Pipeline

### Surface System

`Surface` is a 2D grid of `Cell`s representing the terminal:

```rust
pub struct Surface {
    width: u16,
    height: u16,
    cells: Vec<Vec<Cell>>,
    dirty_cells: Vec<Vec<bool>>,
    first_dirty_row: Option<u16>,
    last_dirty_row: Option<u16>,
}
```

### Rendering Flow

```
┌─────────────┐
│ Components  │
│ (dirty?)    │
└──────┬──────┘
       │
       ▼
┌─────────────────────────┐
│ Render to Surface       │
│ (clear dirty cells)    │
└──────┬──────────────────┘
       │
       ▼
┌─────────────────────────┐
│ Diff Detection          │ ──► Compute SGR (escape codes)
│ (mark changed cells)    │     Only update changed cells
└──────┬──────────────────┘
       │
       ▼
┌─────────────────────────┐
│ Flush to Terminal       │
│ (SGR + cursor moves)    │
└─────────────────────────┘
```

### Dirty Tracking

```rust
// Mark cells as dirty when changing
surface.set(row, col, cell);  // Marks cell as dirty

// Query dirty range for optimal updates
if let Some(first) = surface.first_dirty() {
    let last = surface.last_dirty().unwrap();
    // Only redraw rows first..=last
}

// Clear after rendering
surface.clear_dirty();
```

### Cell Structure

```rust
pub struct Cell {
    pub char: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attributes,
}

pub struct Attributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub reverse: bool,
}
```

### SGR Escape Codes

Only changed cells are sent to the terminal:

```rust
// SGR sequence builder
fn cell_to_sgr(cell: &Cell) -> String {
    let mut parts = vec![];
    parts.push("38;2;{};{};{}".format(r, g, b)); // fg
    parts.push("48;2;{};{};{}".format(r, g, b)); // bg
    if attrs.bold { parts.push("1"); }
    if attrs.italic { parts.push("3"); }
    format!("\x1b[{}m", parts.join(";"))
}
```

### Wide Character Support

CJK characters occupy 2 columns:

```rust
surface.write_string(0, 0, "Hello 世界");

// Cells:
// [H][e][l][l][o][ ][세][게]
//        │           └── wide continuation
//        └─────────────── wide continuation
```

### Flush Pattern

```rust
fn flush_surface(surface: &Surface) {
    let first_dirty = surface.first_dirty().unwrap_or(0);
    let last_dirty = surface.last_dirty().unwrap_or(surface.height - 1);
    
    for row in first_dirty..=last_dirty {
        // Move cursor
        print!("\x1b[{};{}H", row + 1, 1);
        
        // Build row content with SGR codes
        for col in 0..surface.width() {
            let cell = surface.get(row, col).unwrap();
            // Emit SGR + char
        }
        
        // Clear to end of line
        print!("\x1b[0m\x1b[K");
    }
}
```

## Event System

### Event Types

```rust
pub enum Event {
    /// Keyboard input
    Key(KeyEvent),
    
    /// Mouse input
    Mouse(MouseEvent),
    
    /// Terminal resize
    Resize(ResizeEvent),
    
    /// Focus gained/lost
    Focus(FocusEvent),
}
```

### KeyEvent Structure

```rust
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

pub enum KeyCode {
    Char(char),
    Enter,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Tab,
    Backspace,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

pub struct KeyModifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
}
```

### Key Input Handling

#### Kitty Keyboard Protocol

Modern terminals use the Kitty keyboard protocol for extended keys:

```
\x1b[...;A u  (modifier + key with unicode codepoint)
```

The parser handles:
- Modifier flags in escape sequence
- Unicode codepoint for special keys

#### xterm Extensions

xterm provides additional sequences:
- F1-F4 via application mode
- Bracketed paste mode
- Focus in/out events

### Mouse Input

```rust
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub button: MouseButton,
    pub modifiers: KeyModifiers,
    pub position: Position,
}

pub enum MouseEventKind {
    Press,
    Release,
    Drag,
    ScrollUp,
    ScrollDown,
}
```

## Theme System

### Theme Structure

```rust
pub struct Theme {
    pub name: String,
    pub colors: ColorScheme,
    pub fonts: FontScheme,
    pub spacing: Spacing,
}

pub struct ColorScheme {
    pub fg: Color,
    pub bg: Color,
    pub cursor: Color,
    pub selection: Color,
    pub scrollbar: Color,
    pub border: Color,
    // ... component-specific colors
}

pub struct FontScheme {
    pub regular: String,
    pub bold: String,
    pub italic: String,
}
```

### Theme Files

Themes are TOML files:

```toml
[colors]
fg = "#f8f8f2"
bg = "#282a36"
cursor = "#f8f8f0"
selection = "#44475a"

[colors.chat]
user_fg = "#8be9fd"
assistant_fg = "#50fa7b"
system_fg = "#ffb86c"

[fonts]
regular = "JetBrains Mono"
size = 14
line_height = 1.4
```

### Loading Themes

```rust
let manager = ThemeManager::new();
manager.load_theme("nord")?;
manager.set_active("nord");

// Apply to surface
surface.fill(Cell::default().with_bg(theme.colors.bg));
```

## Built-in Components

### Text

Simple text display:

```rust
let text = Text::new("Hello, world!".to_string());
```

### Spacer

Flexible spacing:

```rust
// Vertical spacer
let spacer = Spacer::vertical(10);

// Horizontal spacer
let spacer = Spacer::horizontal(20);

// Flexible spacer (takes remaining space)
let spacer = Spacer::flexible();
```

### SelectList

Selectable list items:

```rust
let list = SelectList::new(vec![
    SelectItem::new("Option 1", Some("description")),
    SelectItem::new("Option 2", None),
]);
list.on_select(|item| { /* ... */ });
```

### CommandPalette

Fuzzy-filtered command list:

```rust
let palette = CommandPalette::new();
palette.add_command(Command::new("save", "Save file", |_| {}));
palette.add_command(Command::new("open", "Open file", |_| {}));
```

## Layout System

### Container

```rust
use oxi_tui::{Container, Direction, Constraint};

let container = Container::new(Direction::Vertical)
    .add(Box::new(header).height(Constraint::Length(1)))
    .add(Box::new(content).height(Constraint::Fill))
    .add(Box::new(footer).height(Constraint::Length(1)));
```

### Constraints

```rust
pub enum Constraint {
    /// Fixed pixel size
    Length(u16),
    /// Percentage of parent
    Percent(u16),
    /// Fill remaining space
    Fill,
}
```

## Overlay System

### Creating Overlays

```rust
let overlay = OverlayContent::new()
    .title("Settings")
    .component(Box::new(settings_panel))
    .on_close(|| { /* cleanup */ });

tui.push_overlay(overlay);
```

### Modal Pattern

```rust
let modal = OverlayBox::new(area, |surface, rect| {
    // Draw modal box with border
    surface.fill_row(rect.y, rect.x, ' ', Color::Blue, Color::DarkBlue);
    // ...
});
```

## Keyboard Shortcuts

### KeyBindings

```rust
use oxi_tui::KeyBindings;

let bindings = KeyBindings::new()
    .bind("C-s", Action::Save)
    .bind("C-o", Action::Open)
    .bind("C-c", Action::Cancel);
```

### Action Handling

```rust
fn handle_key(&mut self, event: &Event) -> bool {
    if let Some(action) = self.bindings.resolve(event) {
        match action {
            Action::Save => self.save(),
            Action::Open => self.open(),
            Action::Cancel => return false,  // Pass through
        }
        true
    } else {
        false
    }
}
```