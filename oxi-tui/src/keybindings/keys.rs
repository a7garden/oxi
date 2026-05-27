//! Key representation and parsing for the keybinding system.
//!
//! Supports standard terminal key sequences and Kitty keyboard protocol
//! for enhanced key reporting (e.g., distinguishing modifiers on special keys).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MediaKeyCode, ModifierKeyCode};
use std::fmt;

// ---------------------------------------------------------------------------
// KeyId — normalized key representation
// ---------------------------------------------------------------------------

/// A normalized, hashable key identifier.
///
/// Captures the base key plus modifier state. Two `KeyId`s are equal when they
/// represent the same physical input, regardless of which terminal protocol
/// encoded it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyId {
    /// The base key (ignoring modifiers).
    pub base: BaseKey,
    /// Ctrl modifier active.
    pub ctrl: bool,
    /// Shift modifier active.
    pub shift: bool,
    /// Alt / Option modifier active.
    pub alt: bool,
    /// Super / Command modifier active (Kitty protocol only).
    pub super_: bool,
}

/// The base key, independent of modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BaseKey {
    /// Letter or digit.
    Char(char),
    /// Function key F1–F12.
    F(u8),
    /// Enter / Return.
    Enter,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Delete (forward).
    Delete,
    /// Tab.
    Tab,
    /// Shift+Tab.
    BackTab,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Insert.
    Insert,
    /// Media key (Kitty protocol).
    Media(MediaKeyCode),
    /// Modifier key press (Kitty protocol).
    Modifier(ModifierKeyCode),
    /// Null / unmapped.
    Null,
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.alt {
            write!(f, "Alt+")?;
        }
        if self.shift {
            write!(f, "Shift+")?;
        }
        if self.super_ {
            write!(f, "Super+")?;
        }
        write!(f, "{}", self.base)
    }
}

impl fmt::Display for BaseKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BaseKey::Char(c) => write!(f, "{}", c),
            BaseKey::F(n) => write!(f, "F{}", n),
            BaseKey::Enter => write!(f, "Enter"),
            BaseKey::Escape => write!(f, "Esc"),
            BaseKey::Backspace => write!(f, "Backspace"),
            BaseKey::Delete => write!(f, "Delete"),
            BaseKey::Tab => write!(f, "Tab"),
            BaseKey::BackTab => write!(f, "BackTab"),
            BaseKey::Up => write!(f, "Up"),
            BaseKey::Down => write!(f, "Down"),
            BaseKey::Left => write!(f, "Left"),
            BaseKey::Right => write!(f, "Right"),
            BaseKey::Home => write!(f, "Home"),
            BaseKey::End => write!(f, "End"),
            BaseKey::PageUp => write!(f, "PageUp"),
            BaseKey::PageDown => write!(f, "PageDown"),
            BaseKey::Insert => write!(f, "Insert"),
            BaseKey::Media(m) => write!(f, "Media({:?})", m),
            BaseKey::Modifier(m) => write!(f, "Modifier({:?})", m),
            BaseKey::Null => write!(f, "Null"),
        }
    }
}

// ---------------------------------------------------------------------------
// Conversion from crossterm KeyEvent
// ---------------------------------------------------------------------------

impl From<KeyEvent> for KeyId {
    fn from(event: KeyEvent) -> Self {
        let mods = event.modifiers;
        let base = match event.code {
            KeyCode::Char(c) => BaseKey::Char(c.to_ascii_lowercase()),
            KeyCode::F(n) => BaseKey::F(n),
            KeyCode::Enter => BaseKey::Enter,
            KeyCode::Esc => BaseKey::Escape,
            KeyCode::Backspace => BaseKey::Backspace,
            KeyCode::Delete => BaseKey::Delete,
            KeyCode::Tab => BaseKey::Tab,
            KeyCode::BackTab => BaseKey::BackTab,
            KeyCode::Up => BaseKey::Up,
            KeyCode::Down => BaseKey::Down,
            KeyCode::Left => BaseKey::Left,
            KeyCode::Right => BaseKey::Right,
            KeyCode::Home => BaseKey::Home,
            KeyCode::End => BaseKey::End,
            KeyCode::PageUp => BaseKey::PageUp,
            KeyCode::PageDown => BaseKey::PageDown,
            KeyCode::Insert => BaseKey::Insert,
            KeyCode::Media(m) => BaseKey::Media(m),
            KeyCode::Modifier(m) => BaseKey::Modifier(m),
            KeyCode::Null => BaseKey::Null,
            KeyCode::CapsLock | KeyCode::ScrollLock | KeyCode::NumLock => BaseKey::Null,
            KeyCode::Menu => BaseKey::Null,
            KeyCode::PrintScreen | KeyCode::Pause | KeyCode::KeypadBegin => BaseKey::Null,
        };

        KeyId {
            base,
            ctrl: mods.contains(KeyModifiers::CONTROL),
            shift: mods.contains(KeyModifiers::SHIFT),
            alt: mods.contains(KeyModifiers::ALT),
            super_: mods.contains(KeyModifiers::SUPER),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing from config strings
// ---------------------------------------------------------------------------

/// Parse a keybinding string like "Ctrl+c", "Shift+Enter", "Alt+Left", "F5".
///
/// Format: `[Mod+]Key` where Mod is one of Ctrl, Shift, Alt, Super (comma-separated for multiple).
/// Key is one of: a-z, 0-9, F1-F12, Enter, Esc, Backspace, Delete, Tab, BackTab,
/// Up, Down, Left, Right, Home, End, PageUp, PageDown, Insert, Space.
///
/// # Examples
/// ```
/// use oxi_tui::keybindings::keys::parse_key_id;
///
/// let k = parse_key_id("Ctrl+c").unwrap();
/// assert!(k.ctrl);
/// assert_eq!(k.base.to_string(), "c");
/// ```
pub fn parse_key_id(s: &str) -> Option<KeyId> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    let mut super_ = false;

    // Split on '+' to separate modifiers from the base key
    let parts: Vec<&str> = s.split('+').collect();
    if parts.is_empty() {
        return None;
    }

    // Last part is the key, everything before it are modifiers
    let key_part = parts.last()?;
    for &mod_part in &parts[..parts.len() - 1] {
        match mod_part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" | "option" => alt = true,
            "super" | "cmd" | "command" => super_ = true,
            _ => return None, // Unknown modifier
        }
    }

    let base = parse_base_key(key_part.trim())?;

    Some(KeyId {
        base,
        ctrl,
        shift,
        alt,
        super_,
    })
}

/// Parse the base key portion (without modifiers).
fn parse_base_key(s: &str) -> Option<BaseKey> {
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        // Named keys
        "enter" | "return" => Some(BaseKey::Enter),
        "esc" | "escape" => Some(BaseKey::Escape),
        "backspace" | "bs" => Some(BaseKey::Backspace),
        "delete" | "del" => Some(BaseKey::Delete),
        "tab" => Some(BaseKey::Tab),
        "backtab" => Some(BaseKey::BackTab),
        "up" => Some(BaseKey::Up),
        "down" => Some(BaseKey::Down),
        "left" => Some(BaseKey::Left),
        "right" => Some(BaseKey::Right),
        "home" => Some(BaseKey::Home),
        "end" => Some(BaseKey::End),
        "pageup" | "page_up" | "pgup" => Some(BaseKey::PageUp),
        "pagedown" | "page_down" | "pgdn" => Some(BaseKey::PageDown),
        "insert" | "ins" => Some(BaseKey::Insert),
        "space" => Some(BaseKey::Char(' ')),
        _ => {
            // Function keys F1-F12
            if let Some(n_str) = lower.strip_prefix('f') {
                if let Ok(n) = n_str.parse::<u8>() {
                    if (1..=12).contains(&n) {
                        return Some(BaseKey::F(n));
                    }
                }
            }
            // Single character
            let chars: Vec<char> = s.chars().collect();
            if chars.len() == 1 {
                return Some(BaseKey::Char(chars[0].to_ascii_lowercase()));
            }
            None
        }
    }
}

/// Check if a key represents a printable character (no modifiers except Shift).
pub fn is_printable(key_id: &KeyId) -> bool {
    if key_id.ctrl || key_id.alt || key_id.super_ {
        return false;
    }
    matches!(key_id.base, BaseKey::Char(_))
}

/// Get the character from a printable key, if any.
pub fn as_char(key_id: &KeyId) -> Option<char> {
    if key_id.ctrl || key_id.alt || key_id.super_ {
        return None;
    }
    match key_id.base {
        BaseKey::Char(c) => {
            if key_id.shift {
                Some(c.to_ascii_uppercase())
            } else {
                Some(c)
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Kitty keyboard protocol
// ---------------------------------------------------------------------------

/// Parse a Kitty keyboard protocol escape sequence into a KeyId.
///
/// Kitty encodes key events as `ESC [ codepoint ; modifiers event-type key-modifier \`.
/// For example: `ESC [ 97 ; 131 u` = Ctrl+Shift+A.
///
/// Returns `None` if the input is not a valid Kitty sequence.
///
/// Reference: https://sw.kovidgoyal.net/kitty/keyboard-protocol/
pub fn parse_kitty_sequence(data: &[u8]) -> Option<KeyId> {
    // Minimum: ESC [ 0 ; 0 u = 6 bytes
    if data.len() < 6 {
        return None;
    }
    if data[0] != 0x1b || data[1] != b'[' {
        return None;
    }

    // Find the trailing 'u'
    let s = std::str::from_utf8(data).ok()?;
    if !s.ends_with('u') {
        return None;
    }

    // Strip ESC [ prefix and 'u' suffix
    let inner = &s[2..s.len() - 1];
    let parts: Vec<&str> = inner.split(';').collect();
    if parts.len() < 2 {
        return None;
    }

    let codepoint: u32 = parts[0].parse().ok()?;
    let modifier_field: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

    // Decode modifiers: bits 0-3 of modifier_field
    // bit 0 = shift, bit 1 = alt, bit 2 = ctrl, bit 3 = super
    let shift = (modifier_field & (1 << 0)) != 0;
    let alt = (modifier_field & (1 << 1)) != 0;
    let ctrl = (modifier_field & (1 << 2)) != 0;
    let super_ = (modifier_field & (1 << 3)) != 0;

    // Decode base key from codepoint
    let base = if (57344..=65533).contains(&codepoint) {
        kitty_special_key(codepoint)?
    } else if (1..=26).contains(&codepoint) {
        // Ctrl+letter (1=A, 2=B, ... 26=Z)
        BaseKey::Char((b'a' + (codepoint - 1) as u8) as char)
    } else {
        match codepoint {
            13 => BaseKey::Enter,
            27 => BaseKey::Escape,
            8 => BaseKey::Backspace,
            9 => BaseKey::Tab,
            127 => BaseKey::Delete,
            cp => {
                if let Some(c) = char::from_u32(cp) {
                    BaseKey::Char(c.to_ascii_lowercase())
                } else {
                    return None;
                }
            }
        }
    };

    Some(KeyId {
        base,
        ctrl,
        shift,
        alt,
        super_,
    })
}

/// Map Kitty special key codepoints to BaseKey.
fn kitty_special_key(cp: u32) -> Option<BaseKey> {
    match cp {
        57344 => Some(BaseKey::Up),
        57345 => Some(BaseKey::Down),
        57346 => Some(BaseKey::Left),
        57347 => Some(BaseKey::Right),
        57348 => Some(BaseKey::Insert),
        57349 => Some(BaseKey::Delete),
        57350 => Some(BaseKey::PageUp),
        57351 => Some(BaseKey::PageDown),
        57352 => Some(BaseKey::Home),
        57353 => Some(BaseKey::End),
        57358 => Some(BaseKey::F(1)),
        57359 => Some(BaseKey::F(2)),
        57360 => Some(BaseKey::F(3)),
        57361 => Some(BaseKey::F(4)),
        57362 => Some(BaseKey::F(5)),
        57363 => Some(BaseKey::F(6)),
        57364 => Some(BaseKey::F(7)),
        57365 => Some(BaseKey::F(8)),
        57366 => Some(BaseKey::F(9)),
        57367 => Some(BaseKey::F(10)),
        57368 => Some(BaseKey::F(11)),
        57369 => Some(BaseKey::F(12)),
        57370 => Some(BaseKey::BackTab),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_char() {
        let k = parse_key_id("a").unwrap();
        assert_eq!(k.base, BaseKey::Char('a'));
        assert!(!k.ctrl);
        assert!(!k.shift);
        assert!(!k.alt);
    }

    #[test]
    fn test_parse_ctrl_c() {
        let k = parse_key_id("Ctrl+c").unwrap();
        assert_eq!(k.base, BaseKey::Char('c'));
        assert!(k.ctrl);
        assert!(!k.shift);
    }

    #[test]
    fn test_parse_shift_enter() {
        let k = parse_key_id("Shift+Enter").unwrap();
        assert_eq!(k.base, BaseKey::Enter);
        assert!(k.shift);
    }

    #[test]
    fn test_parse_alt_left() {
        let k = parse_key_id("Alt+Left").unwrap();
        assert_eq!(k.base, BaseKey::Left);
        assert!(k.alt);
    }

    #[test]
    fn test_parse_ctrl_shift_f() {
        let k = parse_key_id("Ctrl+Shift+F").unwrap();
        assert_eq!(k.base, BaseKey::Char('f'));
        assert!(k.ctrl);
        assert!(k.shift);
    }

    #[test]
    fn test_parse_f5() {
        let k = parse_key_id("F5").unwrap();
        assert_eq!(k.base, BaseKey::F(5));
    }

    #[test]
    fn test_parse_ctrl_f5() {
        let k = parse_key_id("Ctrl+F5").unwrap();
        assert_eq!(k.base, BaseKey::F(5));
        assert!(k.ctrl);
    }

    #[test]
    fn test_parse_space() {
        let k = parse_key_id("Space").unwrap();
        assert_eq!(k.base, BaseKey::Char(' '));
    }

    #[test]
    fn test_parse_backtab() {
        let k = parse_key_id("BackTab").unwrap();
        assert_eq!(k.base, BaseKey::BackTab);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse_key_id("").is_none());
        assert!(parse_key_id("Foo+Bar").is_none());
    }

    #[test]
    fn test_from_crossterm_key_event() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let key_id = KeyId::from(event);
        assert_eq!(key_id.base, BaseKey::Char('c'));
        assert!(key_id.ctrl);
    }

    #[test]
    fn test_display() {
        let k = parse_key_id("Ctrl+Shift+Enter").unwrap();
        assert_eq!(format!("{}", k), "Ctrl+Shift+Enter");
    }

    #[test]
    fn test_is_printable() {
        let k = parse_key_id("a").unwrap();
        assert!(is_printable(&k));
        let k = parse_key_id("Ctrl+a").unwrap();
        assert!(!is_printable(&k));
    }

    #[test]
    fn test_as_char() {
        let k = parse_key_id("a").unwrap();
        assert_eq!(as_char(&k), Some('a'));
        let k = parse_key_id("Enter").unwrap();
        assert_eq!(as_char(&k), None);
    }

    #[test]
    fn test_parse_kitty_simple() {
        // ESC [ 97 ; 0 u = 'a' with no modifiers
        let seq = b"\x1b[97;0u";
        let key_id = parse_kitty_sequence(seq).unwrap();
        assert_eq!(key_id.base, BaseKey::Char('a'));
        assert!(!key_id.ctrl);
    }

    #[test]
    fn test_parse_kitty_ctrl() {
        // ESC [ 97 ; 4 u = Ctrl+A (bit 2 = ctrl)
        let seq = b"\x1b[97;4u";
        let key_id = parse_kitty_sequence(seq).unwrap();
        assert_eq!(key_id.base, BaseKey::Char('a'));
        assert!(key_id.ctrl);
    }

    #[test]
    fn test_parse_kitty_shift_alt() {
        // ESC [ 97 ; 3 u = Shift+Alt+A (bits 0+1)
        let seq = b"\x1b[97;3u";
        let key_id = parse_kitty_sequence(seq).unwrap();
        assert!(key_id.shift);
        assert!(key_id.alt);
        assert!(!key_id.ctrl);
    }

    #[test]
    fn test_parse_kitty_up() {
        // ESC [ 57344 ; 0 u = Up arrow
        let seq = b"\x1b[57344;0u";
        let key_id = parse_kitty_sequence(seq).unwrap();
        assert_eq!(key_id.base, BaseKey::Up);
    }

    #[test]
    fn test_parse_kitty_invalid() {
        assert!(parse_kitty_sequence(b"").is_none());
        assert!(parse_kitty_sequence(b"hello").is_none());
        assert!(parse_kitty_sequence(b"\x1b[u").is_none());
    }
}
