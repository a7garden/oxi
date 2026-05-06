//! Input events for the TUI.

/// Keyboard key codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// Enter key
    Enter,
    /// Escape key
    Escape,
    /// Tab key
    Tab,
    /// Backspace key
    Backspace,
    /// Delete key
    Delete,
    /// Arrow up
    Up,
    /// Arrow down
    Down,
    /// Arrow left
    Left,
    /// Arrow right
    Right,
    /// Home key
    Home,
    /// End key
    End,
    /// Page Up
    PageUp,
    /// Page Down
    PageDown,
    /// Insert key
    Insert,
    /// Function keys F1-F12
    F(u8),
    /// Character keys (A-Z, numbers, symbols)
    Char(char),
}

impl KeyCode {
    /// Get the character representation if any.
    pub fn as_char(&self) -> Option<char> {
        match self {
            KeyCode::Char(c) => Some(*c),
            _ => None,
        }
    }

    /// Get uppercase character if it's an alphabetic key.
    pub fn as_upper(&self) -> Option<char> {
        self.as_char().map(|c| c.to_ascii_uppercase())
    }
}

/// Modifier keys that can be combined with other inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct KeyModifiers {
    /// Whether Shift is held.
    pub shift: bool,
    /// Whether Ctrl is held.
    pub ctrl: bool,
    /// Whether Alt is held.
    pub alt: bool,
    /// Whether Meta/Super is held.
    pub meta: bool,
}

impl KeyModifiers {
    /// Create a new zeroed modifier set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add Shift modifier.
    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    /// Add Ctrl modifier.
    pub fn with_ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    /// Add Alt modifier.
    pub fn with_alt(mut self) -> Self {
        self.alt = true;
        self
    }

    /// Add Meta/Super modifier.
    pub fn with_meta(mut self) -> Self {
        self.meta = true;
        self
    }
}

/// Keyboard event with key code and modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key code.
    pub code: KeyCode,
    /// Active modifier keys.
    pub modifiers: KeyModifiers,
}

impl KeyEvent {
    /// Create a new key event with no modifiers.
    pub fn new(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::new(),
        }
    }

    /// Create a new key event with explicit modifiers.
    pub fn with_modifiers(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// Check if Ctrl is held.
    pub fn is_ctrl(&self) -> bool {
        self.modifiers.ctrl
    }

    /// Check if Alt is held.
    pub fn is_alt(&self) -> bool {
        self.modifiers.alt
    }

    /// Check if Shift is held.
    pub fn is_shift(&self) -> bool {
        self.modifiers.shift
    }
}

/// Mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Left mouse button.
    Left,
    /// Right mouse button.
    Right,
    /// Middle / scroll wheel button.
    Middle,
    /// No button (scroll or move events).
    None,
}

/// Mouse event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    /// A mouse button was pressed.
    Press,
    /// A mouse button was released.
    Release,
    /// A complete click (press + release at same position).
    Click,
    /// A double click.
    DoubleClick,
    /// Mouse button held while moving.
    Drag,
    /// Scroll wheel up.
    ScrollUp,
    /// Scroll wheel down.
    ScrollDown,
    /// Horizontal scroll left (touchpad).
    ScrollLeft,
    /// Horizontal scroll right (touchpad).
    ScrollRight,
    /// Mouse moved without any button pressed (hover).
    Moved,
}

/// Mouse event with position and button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseEvent {
    /// The kind of mouse event.
    pub kind: MouseEventKind,
    /// Which button is involved.
    pub button: MouseButton,
    /// Row coordinate (0-indexed).
    pub row: u16,
    /// Column coordinate (0-indexed).
    pub col: u16,
}

/// Resize event when terminal size changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeEvent {
    /// New terminal width in columns.
    pub width: u16,
    /// New terminal height in rows.
    pub height: u16,
}

/// Unified input event type.
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(Default)]
pub enum Event {
    /// Keyboard input
    Key(KeyEvent),
    /// Mouse input
    Mouse(MouseEvent),
    /// Terminal resize
    Resize(ResizeEvent),
    /// Terminal gained focus
    FocusGained,
    /// Terminal lost focus
    FocusLost,
    /// Paste event (text being pasted)
    Paste(String),
    /// Cursor position report from terminal (response to ESC[6n query).
    /// This is NOT user input; it's a terminal response indicating row/col.
    CursorPosition(u16, u16),
    /// No-op for tick/frame events
    #[default]
    None,
}

impl Event {
    /// Create a simple key event.
    pub fn key(code: KeyCode) -> Self {
        Event::Key(KeyEvent::new(code))
    }

    /// Create a key event with modifiers.
    pub fn key_with(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Event::Key(KeyEvent::with_modifiers(code, modifiers))
    }

    /// Create a resize event.
    pub fn resize(width: u16, height: u16) -> Self {
        Event::Resize(ResizeEvent { width, height })
    }

    /// Create a mouse event.
    pub fn mouse(kind: MouseEventKind, button: MouseButton, row: u16, col: u16) -> Self {
        Event::Mouse(MouseEvent {
            kind,
            button,
            row,
            col,
        })
    }
}

