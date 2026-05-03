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
    /// Backspace
    Backspace,
    /// Delete key
    Delete,
    /// Arrow keys
    Up,
    Down,
    Left,
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
    /// Number keys (0-9) - distinct from Char('0') etc.
    Number(u8),
}

impl KeyCode {
    /// Get the character representation if any.
    pub fn as_char(&self) -> Option<char> {
        match self {
            KeyCode::Char(c) => Some(*c),
            KeyCode::Number(n) => Some(char::from(b'0' + *n)),
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
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl KeyModifiers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub fn with_ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    pub fn with_alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub fn with_meta(mut self) -> Self {
        self.meta = true;
        self
    }
}

/// Keyboard event with key code and modifiers.
#[derive(Debug, Clone)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyEvent {
    pub fn new(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::new(),
        }
    }

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
    Left,
    Right,
    Middle,
    None,
}

/// Mouse event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Press,
    Release,
    Click,
    DoubleClick,
    Drag,
    ScrollUp,
    ScrollDown,
}

/// Mouse event with position and button.
#[derive(Debug, Clone)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub button: MouseButton,
    pub row: u16,
    pub col: u16,
}

/// Resize event when terminal size changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeEvent {
    pub width: u16,
    pub height: u16,
}

/// Unified input event type.
#[derive(Debug, Clone)]
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
    /// No-op for tick/frame events
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

impl Default for Event {
    fn default() -> Self {
        Event::None
    }
}
