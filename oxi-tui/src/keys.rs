//! Terminal key input parsing.
//!
//! Parses raw terminal byte sequences into structured [`Event`] values.
//! Supports:
//! - Legacy CSI / SS3 escape sequences (arrows, function keys, etc.)
//! - Kitty keyboard protocol (CSI-u, with flags 1, 2, 4)
//! - xterm modifyOtherKeys (CSI 27 ; mod ; code ~)
//! - SGR mouse events
//! - Old-style X10 mouse events
//! - Focus events (focus-in / focus-out)
//! - Bracketed paste
//! - UTF-8 multi-byte character decoding
//!
//! Originally inspired by pi-mono's terminal key input parsing.

use crate::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ESC: u8 = 0x1b;
const DEL: u8 = 0x7f;
const BS: u8 = 0x08;

/// Modifier bit flags (match the Kitty / xterm convention).
mod modifier_bits {
    pub const SHIFT: u8 = 1;
    pub const ALT: u8 = 2;
    pub const CTRL: u8 = 4;
    pub const SUPER: u8 = 8;
    /// Caps Lock (ignored in modifier comparison).
    pub const CAPS_LOCK: u8 = 64;
    /// Num Lock (ignored in modifier comparison).
    pub const NUM_LOCK: u8 = 128;
}

/// Negative codepoints used by the Kitty protocol for non-printable keys.
mod codepoints {
    pub const ESCAPE: i64 = 27;
    pub const TAB: i64 = 9;
    pub const ENTER: i64 = 13;
    pub const SPACE: i64 = 32;
    pub const BACKSPACE: i64 = 127;
    pub const KP_ENTER: i64 = 57414;

    pub const UP: i64 = -1;
    pub const DOWN: i64 = -2;
    pub const RIGHT: i64 = -3;
    pub const LEFT: i64 = -4;

    pub const DELETE: i64 = -10;
    pub const INSERT: i64 = -11;
    pub const PAGE_UP: i64 = -12;
    pub const PAGE_DOWN: i64 = -13;
    pub const HOME: i64 = -14;
    pub const END: i64 = -15;
}

// ---------------------------------------------------------------------------
// Kitty protocol state
// ---------------------------------------------------------------------------

/// Global Kitty keyboard protocol state.
///
/// When active, some legacy sequences are re-interpreted (e.g. `\x1b\r`
/// becomes Shift+Enter instead of Alt+Enter).
static mut KITTY_PROTOCOL_ACTIVE: bool = false;

/// Set whether the Kitty keyboard protocol is currently active.
///
/// # Safety
/// Should only be called from a single-threaded context (before the TUI
/// event loop starts) or protected by external synchronisation.
pub unsafe fn set_kitty_protocol_active(active: bool) {
    KITTY_PROTOCOL_ACTIVE = active;
}

/// Query whether the Kitty keyboard protocol is currently active.
///
/// # Safety
/// Same constraints as [`set_kitty_protocol_active`].
pub unsafe fn is_kitty_protocol_active() -> bool {
    KITTY_PROTOCOL_ACTIVE
}

/// Thread-safe wrapper used at call-sites.
fn kitty_active() -> bool {
    // SAFETY: read of a bool that is only written once during init.
    unsafe { KITTY_PROTOCOL_ACTIVE }
}

// ---------------------------------------------------------------------------
// Event type (Kitty flag 2)
// ---------------------------------------------------------------------------

/// Key event type reported by Kitty keyboard protocol flag 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    Press,
    Repeat,
    Release,
}

// Track the last parsed event type so callers can query it.
static mut LAST_EVENT_TYPE: KeyEventType = KeyEventType::Press;

/// Retrieve the event type of the most recent key parse.
///
/// # Safety
/// Read-only; safe to call from the main TUI thread.
pub unsafe fn last_key_event_type() -> KeyEventType {
    LAST_EVENT_TYPE
}

fn set_last_event_type(ty: KeyEventType) {
    // SAFETY: single-threaded TUI context.
    unsafe {
        LAST_EVENT_TYPE = ty;
    }
}

fn parse_event_type_str(s: Option<&str>) -> KeyEventType {
    match s {
        Some("2") => KeyEventType::Repeat,
        Some("3") => KeyEventType::Release,
        _ => KeyEventType::Press,
    }
}

// ---------------------------------------------------------------------------
// Kitty numpad codepoint normalization
// ---------------------------------------------------------------------------

/// Map Kitty numpad codepoints to their equivalent main-keyboard codepoints.
fn normalize_kitty_functional_codepoint(cp: i64) -> i64 {
    match cp {
        57399 => 48,                // KP_0 -> '0'
        57400 => 49,                // KP_1 -> '1'
        57401 => 50,                // KP_2 -> '2'
        57402 => 51,                // KP_3 -> '3'
        57403 => 52,                // KP_4 -> '4'
        57404 => 53,                // KP_5 -> '5'
        57405 => 54,                // KP_6 -> '6'
        57406 => 55,                // KP_7 -> '7'
        57407 => 56,                // KP_8 -> '8'
        57408 => 57,                // KP_9 -> '9'
        57409 => 46,                // KP_DECIMAL -> '.'
        57410 => 47,                // KP_DIVIDE -> '/'
        57411 => 42,                // KP_MULTIPLY -> '*'
        57412 => 45,                // KP_SUBTRACT -> '-'
        57413 => 43,                // KP_ADD -> '+'
        57415 => 61,                // KP_EQUAL -> '='
        57416 => 44,                // KP_SEPARATOR -> ','
        57417 => codepoints::LEFT,  // KP_LEFT
        57418 => codepoints::RIGHT, // KP_RIGHT
        57419 => codepoints::UP,    // KP_UP
        57420 => codepoints::DOWN,  // KP_DOWN
        57421 => codepoints::PAGE_UP,
        57422 => codepoints::PAGE_DOWN,
        57423 => codepoints::HOME,
        57424 => codepoints::END,
        57425 => codepoints::INSERT,
        57426 => codepoints::DELETE,
        _ => cp,
    }
}

/// If Shift is held and the codepoint is an uppercase ASCII letter (A-Z),
/// normalise to lowercase (a-z) so that 'Shift+A' is treated the same as
/// 'Shift+a' for identity comparison purposes.
fn normalize_shifted_letter(cp: i64, modifier: u8) -> i64 {
    let effective = modifier & !(modifier_bits::CAPS_LOCK | modifier_bits::NUM_LOCK);
    if (effective & modifier_bits::SHIFT) != 0 && (65..=90).contains(&cp) {
        cp + 32
    } else {
        cp
    }
}

/// Mask out the lock bits (Caps Lock + Num Lock) from a modifier byte.
fn effective_modifier(modifier: u8) -> u8 {
    modifier & !(modifier_bits::CAPS_LOCK | modifier_bits::NUM_LOCK)
}

// ---------------------------------------------------------------------------
// Kitty sequence parser
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedKitty {
    codepoint: i64,
    shifted_key: Option<i64>,
    base_layout_key: Option<i64>,
    modifier: u8,
    #[allow(dead_code)] // stored for querying via last_key_event_type()
    event_type: KeyEventType,
}

/// Parse a Kitty CSI-u sequence.
///
/// Supported forms:
/// - `ESC [ <cp> u`
/// - `ESC [ <cp> ; <mod> u`
/// - `ESC [ <cp> ; <mod> : <event> u`
/// - `ESC [ <cp> : <shifted> ; <mod> u`
/// - `ESC [ <cp> : <shifted> : <base> ; <mod> u`
/// - `ESC [ <cp> :: <base> ; <mod> u`
fn parse_kitty_sequence(data: &[u8]) -> Option<ParsedKitty> {
    let s = std::str::from_utf8(data).ok()?;
    if !s.starts_with('\x1b') || s.len() < 4 {
        return None;
    }

    // CSI u format
    if let Some(rest) = s.strip_prefix("\x1b[") {
        if rest.ends_with('u') && !rest.starts_with('<') {
            return parse_csi_u(rest.strip_suffix('u')?);
        }
        // Arrow with modifier: \x1b[1;<mod>A/B/C/D  or  \x1b[1;<mod>:<event>A/B/C/D
        if let Some(arrow) = parse_arrow_kitty(rest) {
            return Some(arrow);
        }
        // Functional key: \x1b<num>~ or \x1b<num>;<mod>~ etc.
        if rest.ends_with('~') {
            return parse_functional_kitty(rest.strip_suffix('~')?);
        }
        // Home/End with modifier: \x1b[1;<mod>H/F
        if let Some(he) = parse_home_end_kitty(rest) {
            return Some(he);
        }
    }

    None
}

/// Parse the interior of `ESC [ ... u`.
fn parse_csi_u(inner: &str) -> Option<ParsedKitty> {
    // Split off the modifier part after ';'
    let (cp_part, mod_part) = if let Some(idx) = inner.find(';') {
        (&inner[..idx], &inner[idx + 1..])
    } else {
        (inner, "")
    };

    // cp_part may contain codepoint[:shifted[:base]]
    let mut cp_iter = cp_part.splitn(3, ':');
    let cp_str = cp_iter.next()?;
    let shifted_str = cp_iter.next();
    let base_str = cp_iter.next();

    let codepoint: i64 = cp_str.parse().ok()?;
    let shifted_key = shifted_str
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok());
    let base_layout_key = base_str.and_then(|s| s.parse().ok());

    // mod_part may be <mod>[:<event>]
    let (mod_val, event_type) = if mod_part.is_empty() {
        (1u8, KeyEventType::Press)
    } else {
        let mut miter = mod_part.splitn(2, ':');
        let m_str = miter.next()?;
        let e_str = miter.next();
        let m: u8 = m_str.parse().ok()?;
        (m, parse_event_type_str(e_str))
    };

    set_last_event_type(event_type);
    Some(ParsedKitty {
        codepoint,
        shifted_key,
        base_layout_key,
        modifier: mod_val.saturating_sub(1),
        event_type,
    })
}

/// Parse arrow key Kitty form: `1;<mod>A` or `1;<mod>:<event>A`.
fn parse_arrow_kitty(rest: &str) -> Option<ParsedKitty> {
    if !rest.starts_with('1') {
        return None;
    }
    let last = rest.chars().last()?;
    let letter = match last {
        'A' => codepoints::UP,
        'B' => codepoints::DOWN,
        'C' => codepoints::RIGHT,
        'D' => codepoints::LEFT,
        _ => return None,
    };

    // Strip trailing letter
    let inner = &rest[1..rest.len() - 1]; // strip leading '1' and trailing letter
    if inner.is_empty() {
        // bare `\x1b[1A` etc. – no modifier
        set_last_event_type(KeyEventType::Press);
        return Some(ParsedKitty {
            codepoint: letter,
            shifted_key: None,
            base_layout_key: None,
            modifier: 0,
            event_type: KeyEventType::Press,
        });
    }

    // inner starts with ';' if modifier present
    let inner = inner.strip_prefix(';')?;
    let mut iter = inner.splitn(2, ':');
    let mod_str = iter.next()?;
    let event_str = iter.next();
    let mod_val: u8 = mod_str.parse().ok()?;
    let event_type = parse_event_type_str(event_str);

    set_last_event_type(event_type);
    Some(ParsedKitty {
        codepoint: letter,
        shifted_key: None,
        base_layout_key: None,
        modifier: mod_val.saturating_sub(1),
        event_type,
    })
}

/// Parse functional key Kitty form: `<num>` or `<num>;<mod>` or `<num>;<mod>:<event>`.
fn parse_functional_kitty(inner: &str) -> Option<ParsedKitty> {
    let (num_part, rest) = if let Some(idx) = inner.find(';') {
        (&inner[..idx], &inner[idx + 1..])
    } else {
        (inner, "")
    };
    let key_num: u32 = num_part.parse().ok()?;
    let cp = match key_num {
        2 => codepoints::INSERT,
        3 => codepoints::DELETE,
        5 => codepoints::PAGE_UP,
        6 => codepoints::PAGE_DOWN,
        7 => codepoints::HOME,
        8 => codepoints::END,
        _ => return None,
    };

    let (mod_val, event_type) = if rest.is_empty() {
        (1u8, KeyEventType::Press)
    } else {
        let mut iter = rest.splitn(2, ':');
        let m_str = iter.next()?;
        let e_str = iter.next();
        let m: u8 = m_str.parse().ok()?;
        (m, parse_event_type_str(e_str))
    };

    set_last_event_type(event_type);
    Some(ParsedKitty {
        codepoint: cp,
        shifted_key: None,
        base_layout_key: None,
        modifier: mod_val.saturating_sub(1),
        event_type,
    })
}

/// Parse Home/End Kitty form: `1;<mod>H` or `1;<mod>:<event>F`.
fn parse_home_end_kitty(rest: &str) -> Option<ParsedKitty> {
    if !rest.starts_with('1') {
        return None;
    }
    let last = rest.chars().last()?;
    let cp = match last {
        'H' => codepoints::HOME,
        'F' => codepoints::END,
        _ => return None,
    };

    let inner = &rest[1..rest.len() - 1];
    if inner.is_empty() {
        set_last_event_type(KeyEventType::Press);
        return Some(ParsedKitty {
            codepoint: cp,
            shifted_key: None,
            base_layout_key: None,
            modifier: 0,
            event_type: KeyEventType::Press,
        });
    }

    let inner = inner.strip_prefix(';')?;
    let mut iter = inner.splitn(2, ':');
    let mod_str = iter.next()?;
    let event_str = iter.next();
    let mod_val: u8 = mod_str.parse().ok()?;
    let event_type = parse_event_type_str(event_str);

    set_last_event_type(event_type);
    Some(ParsedKitty {
        codepoint: cp,
        shifted_key: None,
        base_layout_key: None,
        modifier: mod_val.saturating_sub(1),
        event_type,
    })
}

// ---------------------------------------------------------------------------
// xterm modifyOtherKeys parser
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedModifyOtherKeys {
    codepoint: i64,
    modifier: u8,
}

/// Parse xterm modifyOtherKeys: `ESC [ 27 ; <mod> ; <code> ~`.
fn parse_modify_other_keys(data: &[u8]) -> Option<ParsedModifyOtherKeys> {
    let s = std::str::from_utf8(data).ok()?;
    if !s.starts_with("\x1b[27;") || !s.ends_with('~') {
        return None;
    }
    let inner = &s[5..s.len() - 1]; // strip "\x1b[27;" and "~"
    let (mod_str, cp_str) = inner.split_once(';')?;
    
    
    let mod_val: u8 = mod_str.parse().ok()?;
    let cp: i64 = cp_str.parse().ok()?;
    Some(ParsedModifyOtherKeys {
        codepoint: cp,
        modifier: mod_val.saturating_sub(1),
    })
}

// ---------------------------------------------------------------------------
// Kitty printable decoding (for text insertion)
// ---------------------------------------------------------------------------

/// Decode a Kitty CSI-u sequence into a printable character, if applicable.
///
/// Only plain or Shift-modified keys are decoded. Ctrl/Alt combinations are
/// rejected (those are keybindings, not text input).
pub fn decode_kitty_printable(data: &[u8]) -> Option<char> {
    let kitty = parse_kitty_sequence(data)?;
    let modifier = effective_modifier(kitty.modifier);
    // Only accept shift + lock bits
    if (modifier & !(modifier_bits::SHIFT | modifier_bits::CAPS_LOCK | modifier_bits::NUM_LOCK)) != 0 {
        return None;
    }
    if (modifier & (modifier_bits::ALT | modifier_bits::CTRL)) != 0 {
        return None;
    }

    let mut cp = kitty.codepoint;
    if (modifier & modifier_bits::SHIFT) != 0 {
        if let Some(sk) = kitty.shifted_key {
            cp = sk;
        }
    }
    cp = normalize_kitty_functional_codepoint(cp);
    if cp < 32 {
        return None;
    }
    char::from_u32(cp as u32)
}

/// Decode an xterm modifyOtherKeys sequence into a printable character.
fn decode_modify_other_keys_printable(data: &[u8]) -> Option<char> {
    let parsed = parse_modify_other_keys(data)?;
    let modifier = effective_modifier(parsed.modifier);
    if (modifier & !modifier_bits::SHIFT) != 0 {
        return None;
    }
    if parsed.codepoint < 32 {
        return None;
    }
    char::from_u32(parsed.codepoint as u32)
}

/// Decode a raw terminal sequence into a printable character.
///
/// Tries Kitty CSI-u first, then xterm modifyOtherKeys.
pub fn decode_printable_key(data: &[u8]) -> Option<char> {
    decode_kitty_printable(data).or_else(|| decode_modify_other_keys_printable(data))
}

// ---------------------------------------------------------------------------
// Key release / repeat detection
// ---------------------------------------------------------------------------

/// Check whether a raw input sequence is a key-release event
/// (Kitty protocol flag 2).
pub fn is_key_release(data: &[u8]) -> bool {
    let s = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // Don't treat bracketed paste content as key release
    if s.contains("\x1b[200~") {
        return false;
    }
    s.contains(":3u")
        || s.contains(":3~")
        || s.contains(":3A")
        || s.contains(":3B")
        || s.contains(":3C")
        || s.contains(":3D")
        || s.contains(":3H")
        || s.contains(":3F")
}

/// Check whether a raw input sequence is a key-repeat event
/// (Kitty protocol flag 2).
pub fn is_key_repeat(data: &[u8]) -> bool {
    let s = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return false,
    };
    if s.contains("\x1b[200~") {
        return false;
    }
    s.contains(":2u")
        || s.contains(":2~")
        || s.contains(":2A")
        || s.contains(":2B")
        || s.contains(":2C")
        || s.contains(":2D")
        || s.contains(":2H")
        || s.contains(":2F")
}

// ---------------------------------------------------------------------------
// Modifier bit → KeyModifiers conversion
// ---------------------------------------------------------------------------

fn modifiers_from_bitfield(modifier: u8) -> KeyModifiers {
    let eff = effective_modifier(modifier);
    KeyModifiers {
        shift: (eff & modifier_bits::SHIFT) != 0,
        ctrl: (eff & modifier_bits::CTRL) != 0,
        alt: (eff & modifier_bits::ALT) != 0,
        meta: (eff & modifier_bits::SUPER) != 0,
    }
}

// ---------------------------------------------------------------------------
// Codepoint → KeyCode
// ---------------------------------------------------------------------------

fn codepoint_to_key_code(cp: i64) -> Option<KeyCode> {
    match cp {
        codepoints::ESCAPE => Some(KeyCode::Escape),
        codepoints::TAB => Some(KeyCode::Tab),
        codepoints::ENTER | codepoints::KP_ENTER => Some(KeyCode::Enter),
        codepoints::SPACE => Some(KeyCode::Char(' ')),
        codepoints::BACKSPACE => Some(KeyCode::Backspace),
        codepoints::DELETE => Some(KeyCode::Delete),
        codepoints::INSERT => Some(KeyCode::Insert),
        codepoints::HOME => Some(KeyCode::Home),
        codepoints::END => Some(KeyCode::End),
        codepoints::PAGE_UP => Some(KeyCode::PageUp),
        codepoints::PAGE_DOWN => Some(KeyCode::PageDown),
        codepoints::UP => Some(KeyCode::Up),
        codepoints::DOWN => Some(KeyCode::Down),
        codepoints::LEFT => Some(KeyCode::Left),
        codepoints::RIGHT => Some(KeyCode::Right),
        cp if (48..=57).contains(&cp) => Some(KeyCode::Char(char::from_u32(cp as u32)?)),
        cp if (97..=122).contains(&cp) => Some(KeyCode::Char(char::from_u32(cp as u32)?)),
        cp if (65..=90).contains(&cp) => Some(KeyCode::Char(char::from_u32(cp as u32)?.to_ascii_lowercase())),
        cp if cp >= 32 => char::from_u32(cp as u32).map(KeyCode::Char),
        _ => None,
    }
}

/// Check if a codepoint represents a symbol key.
fn is_symbol_cp(cp: i64) -> bool {
    matches!(
        cp,
        96 // `
        | 45  // -
        | 61  // =
        | 91  // [
        | 93  // ]
        | 92  // \
        | 59  // ;
        | 39  // '
        | 44  // ,
        | 46  // .
        | 47  // /
        | 33  // !
        | 64  // @
        | 35  // #
        | 36  // $
        | 37  // %
        | 94  // ^
        | 38  // &
        | 42  // *
        | 40  // (
        | 41  // )
        | 95  // _
        | 43  // +
        | 124 // |
        | 126 // ~
        | 123 // {
        | 125 // }
        | 58  // :
        | 60  // <
        | 62  // >
        | 63  // ?
    )
}

/// Convert a Kitty-parsed codepoint into a KeyCode, using the base layout
/// key fallback for non-Latin keyboard layouts.
fn kitty_codepoint_to_key_code(parsed: &ParsedKitty) -> Option<KeyCode> {
    let cp = normalize_kitty_functional_codepoint(parsed.codepoint);
    let eff_cp = normalize_shifted_letter(cp, parsed.modifier);

    // Determine if we should use the base layout key
    let is_latin = (97..=122).contains(&eff_cp);
    let is_digit = (48..=57).contains(&eff_cp);
    let is_symbol = is_symbol_cp(eff_cp);
    let use_base = !is_latin && !is_digit && !is_symbol;

    let final_cp = if use_base {
        parsed.base_layout_key.unwrap_or(eff_cp)
    } else {
        eff_cp
    };

    codepoint_to_key_code(final_cp)
}

// ---------------------------------------------------------------------------
// Mouse event parsing
// ---------------------------------------------------------------------------

/// Parse an SGR mouse event: `ESC [ < B ; X ; Y M/m`.
fn parse_sgr_mouse(data: &[u8]) -> Option<Event> {
    let s = std::str::from_utf8(data).ok()?;
    if !s.starts_with("\x1b[<") {
        return None;
    }
    let last = s.chars().last()?;
    let release = last == 'm';
    let inner = &s[3..s.len() - 1]; // strip "\x1b[<" and trailing M/m
    let mut parts = inner.split(';');
    let button_raw: u16 = parts.next()?.parse().ok()?;
    let col: u16 = parts.next()?.parse().ok()?;
    let row: u16 = parts.next()?.parse().ok()?;

    let (kind, button) = if button_raw >= 64 {
        // Scroll events: 64 = scroll up, 65 = scroll down
        let scroll_kind = if button_raw == 64 {
            MouseEventKind::ScrollUp
        } else {
            MouseEventKind::ScrollDown
        };
        (scroll_kind, MouseButton::None)
    } else if button_raw >= 32 {
        // Motion / drag events: button = raw - 32
        let btn = match button_raw - 32 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::None,
        };
        (MouseEventKind::Drag, btn)
    } else if release {
        let btn = match button_raw {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::None,
        };
        (MouseEventKind::Release, btn)
    } else {
        let btn = match button_raw {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::None,
        };
        (MouseEventKind::Press, btn)
    };

    Some(Event::Mouse(MouseEvent {
        kind,
        button,
        row: row.saturating_sub(1),
        col: col.saturating_sub(1),
    }))
}

/// Parse an old-style X10 mouse event: `ESC [ M Cb Cx Cy` (6 bytes total).
fn parse_x10_mouse(data: &[u8]) -> Option<Event> {
    if data.len() != 6 || data[0] != ESC || data[1] != b'[' || data[2] != b'M' {
        return None;
    }
    let cb = data[3];
    let cx = data[4];
    let cy = data[5];

    let button_raw = cb.wrapping_sub(32);

    let (kind, button) = if button_raw >= 64 {
        let scroll_kind = if button_raw == 64 {
            MouseEventKind::ScrollUp
        } else {
            MouseEventKind::ScrollDown
        };
        (scroll_kind, MouseButton::None)
    } else {
        let btn = match button_raw & 0x03 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::None,
        };
        (MouseEventKind::Press, btn)
    };

    Some(Event::Mouse(MouseEvent {
        kind,
        button,
        row: (cy.wrapping_sub(32) as u16).saturating_sub(1),
        col: (cx.wrapping_sub(32) as u16).saturating_sub(1),
    }))
}

// ---------------------------------------------------------------------------
// UTF-8 decoding
// ---------------------------------------------------------------------------

/// Decode a UTF-8 multi-byte sequence from raw bytes.
/// Returns `None` if the bytes don't form a valid UTF-8 character.
fn decode_utf8_char(data: &[u8]) -> Option<char> {
    let s = std::str::from_utf8(data).ok()?;
    let ch = s.chars().next()?;
    Some(ch)
}

// ---------------------------------------------------------------------------
// Main parser: raw bytes → Event
// ---------------------------------------------------------------------------

/// Parse a raw terminal byte sequence into an [`Event`].
///
/// This is the primary entry point. It handles all supported escape sequence
/// types and falls back to UTF-8 character decoding for plain text.
pub fn parse_event(data: &[u8]) -> Option<Event> {
    if data.is_empty() {
        return None;
    }

    // --- Bracketed paste start/end ---
    if data == b"\x1b[200~" || data == b"\x1b[201~" {
        // These are handled by StdinBuffer; should not appear here.
        return None;
    }

    // --- Focus events ---
    if data == b"\x1b[I" {
        return Some(Event::FocusGained);
    }
    if data == b"\x1b[O" {
        return Some(Event::FocusLost);
    }

    // --- SGR mouse ---
    if data.starts_with(b"\x1b[<") {
        return parse_sgr_mouse(data);
    }

    // --- X10 mouse ---
    if data.starts_with(b"\x1b[M") {
        return parse_x10_mouse(data);
    }

    // --- Kitty protocol sequences ---
    if let Some(kitty) = parse_kitty_sequence(data) {
        let code = kitty_codepoint_to_key_code(&kitty)?;
        let modifiers = modifiers_from_bitfield(kitty.modifier);
        return Some(Event::Key(KeyEvent::with_modifiers(code, modifiers)));
    }

    // --- xterm modifyOtherKeys ---
    if let Some(mok) = parse_modify_other_keys(data) {
        let code = codepoint_to_key_code(mok.codepoint)?;
        let modifiers = modifiers_from_bitfield(mok.modifier);
        return Some(Event::Key(KeyEvent::with_modifiers(code, modifiers)));
    }

    // --- Legacy sequences ---
    if data[0] == ESC {
        return parse_legacy_escape(data);
    }

    // --- Raw single-byte / UTF-8 characters ---
    parse_raw_char(data)
}

// ---------------------------------------------------------------------------
// Legacy escape sequence handling
// ---------------------------------------------------------------------------

fn parse_legacy_escape(data: &[u8]) -> Option<Event> {
    if data.len() == 1 && data[0] == ESC {
        return Some(Event::key(KeyCode::Escape));
    }

    let s = std::str::from_utf8(data).ok()?;

    // Kitty-protocol-aware legacy sequences
    if kitty_active()
        && (data == b"\x1b\r" || data == b"\n") {
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Enter,
                KeyModifiers {
                    shift: true,
                    ..KeyModifiers::default()
                },
            )));
        }

    // F1-F4 via SS3: ESC O P/Q/R/S
    if data.len() == 3 && data[0] == ESC && data[1] == b'O' {
        match data[2] {
            b'P' => return Some(Event::key(KeyCode::F(1))),
            b'Q' => return Some(Event::key(KeyCode::F(2))),
            b'R' => return Some(Event::key(KeyCode::F(3))),
            b'S' => return Some(Event::key(KeyCode::F(4))),
            // SS3 arrows (some terminals)
            b'A' => return Some(Event::key(KeyCode::Up)),
            b'B' => return Some(Event::key(KeyCode::Down)),
            b'C' => return Some(Event::key(KeyCode::Right)),
            b'D' => return Some(Event::key(KeyCode::Left)),
            // SS3 Home/End
            b'H' => return Some(Event::key(KeyCode::Home)),
            b'F' => return Some(Event::key(KeyCode::End)),
            // SS3 M = numpad enter
            b'M' => return Some(Event::key(KeyCode::Enter)),
            // SS3 E = "clear" / "5" key on numpad
            b'E' => return Some(Event::key(KeyCode::Char('5'))),
            // SS3 ctrl arrows
            b'a' => {
                return Some(Event::Key(KeyEvent::with_modifiers(
                    KeyCode::Up,
                    KeyModifiers {
                        ctrl: true,
                        ..KeyModifiers::default()
                    },
                )))
            }
            b'b' => {
                return Some(Event::Key(KeyEvent::with_modifiers(
                    KeyCode::Down,
                    KeyModifiers {
                        ctrl: true,
                        ..KeyModifiers::default()
                    },
                )))
            }
            b'c' => {
                return Some(Event::Key(KeyEvent::with_modifiers(
                    KeyCode::Right,
                    KeyModifiers {
                        ctrl: true,
                        ..KeyModifiers::default()
                    },
                )))
            }
            b'd' => {
                return Some(Event::Key(KeyEvent::with_modifiers(
                    KeyCode::Left,
                    KeyModifiers {
                        ctrl: true,
                        ..KeyModifiers::default()
                    },
                )))
            }
            _ => {}
        }
    }

    // CSI sequences
    if data.len() >= 3 && data[0] == ESC && data[1] == b'[' {
        return parse_csi_sequence(data, s);
    }

    // Two-byte ESC sequences: ESC + char
    if data.len() == 2 && data[0] == ESC {
        return parse_esc_prefix(data[1]);
    }

    None
}

/// Parse CSI sequences (ESC [ ...).
fn parse_csi_sequence(data: &[u8], s: &str) -> Option<Event> {
    let inner = &s[2..]; // skip ESC[

    // Arrow keys: ESC[A/B/C/D
    if data == b"\x1b[A" {
        return Some(Event::key(KeyCode::Up));
    }
    if data == b"\x1b[B" {
        return Some(Event::key(KeyCode::Down));
    }
    if data == b"\x1b[C" {
        return Some(Event::key(KeyCode::Right));
    }
    if data == b"\x1b[D" {
        return Some(Event::key(KeyCode::Left));
    }

    // Home: ESC[H
    if data == b"\x1b[H" {
        return Some(Event::key(KeyCode::Home));
    }
    // End: ESC[F
    if data == b"\x1b[F" {
        return Some(Event::key(KeyCode::End));
    }

    // Shift+Tab: ESC[Z
    if data == b"\x1b[Z" {
        return Some(Event::Key(KeyEvent::with_modifiers(
            KeyCode::Tab,
            KeyModifiers {
                shift: true,
                ..KeyModifiers::default()
            },
        )));
    }

    // Shift arrows: ESC[a/b/c/d
    if data == b"\x1b[a" {
        return Some(shift_key(KeyCode::Up));
    }
    if data == b"\x1b[b" {
        return Some(shift_key(KeyCode::Down));
    }
    if data == b"\x1b[c" {
        return Some(shift_key(KeyCode::Right));
    }
    if data == b"\x1b[d" {
        return Some(shift_key(KeyCode::Left));
    }

    // Alt arrows (legacy): ESC p/n/b/f
    // (These come through as two-byte ESC sequences, handled in parse_esc_prefix)

    // CSI ~ sequences: functional keys
    if inner.ends_with('~') {
        return parse_csi_tilde(data, inner);
    }

    // Alt+arrow via explicit CSI: ESC[1;3A/B/C/D
    if data == b"\x1b[1;3A" {
        return Some(alt_key(KeyCode::Up));
    }
    if data == b"\x1b[1;3B" {
        return Some(alt_key(KeyCode::Down));
    }
    if data == b"\x1b[1;3C" {
        return Some(alt_key(KeyCode::Right));
    }
    if data == b"\x1b[1;3D" {
        return Some(alt_key(KeyCode::Left));
    }

    // Ctrl+arrow via CSI: ESC[1;5A/B/C/D
    if data == b"\x1b[1;5A" {
        return Some(ctrl_key(KeyCode::Up));
    }
    if data == b"\x1b[1;5B" {
        return Some(ctrl_key(KeyCode::Down));
    }
    if data == b"\x1b[1;5C" {
        return Some(ctrl_key(KeyCode::Right));
    }
    if data == b"\x1b[1;5D" {
        return Some(ctrl_key(KeyCode::Left));
    }

    None
}

/// Parse CSI ~ sequences for functional keys.
fn parse_csi_tilde(_data: &[u8], inner: &str) -> Option<Event> {
    // Strip trailing ~
    let payload = &inner[..inner.len() - 1];

    // Check for modifier: <num>;<mod>~
    let (num_str, modifier) = if let Some(idx) = payload.find(';') {
        let n = &payload[..idx];
        let m_str = &payload[idx + 1..];
        let m: u8 = m_str.parse().ok()?;
        (n, m.saturating_sub(1))
    } else {
        (payload, 0u8)
    };

    let num: u32 = num_str.parse().ok()?;
    let code = match num {
        1 => Some(KeyCode::Home),     // ESC[1~
        2 => Some(KeyCode::Insert),   // ESC[2~
        3 => Some(KeyCode::Delete),   // ESC[3~
        4 => Some(KeyCode::End),      // ESC[4~
        5 => Some(KeyCode::PageUp),   // ESC[5~
        6 => Some(KeyCode::PageDown), // ESC[6~
        7 => Some(KeyCode::Home),     // ESC[7~
        8 => Some(KeyCode::End),      // ESC[8~
        11 => Some(KeyCode::F(1)),    // ESC[11~
        12 => Some(KeyCode::F(2)),    // ESC[12~
        13 => Some(KeyCode::F(3)),    // ESC[13~
        14 => Some(KeyCode::F(4)),    // ESC[14~
        15 => Some(KeyCode::F(5)),    // ESC[15~
        17 => Some(KeyCode::F(6)),    // ESC[17~
        18 => Some(KeyCode::F(7)),    // ESC[18~
        19 => Some(KeyCode::F(8)),    // ESC[19~
        20 => Some(KeyCode::F(9)),    // ESC[20~
        21 => Some(KeyCode::F(10)),   // ESC[21~
        23 => Some(KeyCode::F(11)),   // ESC[23~
        24 => Some(KeyCode::F(12)),   // ESC[24~
        _ => None,
    };

    let code = code?;

    if modifier == 0 {
        return Some(Event::key(code));
    }

    // Shift-modified functional keys: ESC[<num>$
    // Ctrl-modified functional keys: ESC[<num>^
    // We handle these through the modifier value from the semicolon form.
    let mods = modifiers_from_bitfield(modifier);
    Some(Event::Key(KeyEvent::with_modifiers(code, mods)))
}

/// Parse two-byte ESC prefix sequences: ESC <byte>.
fn parse_esc_prefix(second: u8) -> Option<Event> {
    // Ctrl+Alt + letter: ESC + ctrl_char (1..26)
    if (1..=26).contains(&second) {
        let ch = char::from(second + 96); // 1→'a', 2→'b', ...
        return Some(Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char(ch),
            KeyModifiers {
                ctrl: true,
                alt: true,
                ..KeyModifiers::default()
            },
        )));
    }

    // Alt+letter/digit: ESC + letter/digit
    if second.is_ascii_lowercase() || second.is_ascii_digit() {
        let ch = second as char;
        return Some(Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char(ch),
            KeyModifiers {
                alt: true,
                ..KeyModifiers::default()
            },
        )));
    }

    // Alt+Backspace: ESC DEL or ESC BS
    if second == DEL {
        return Some(Event::Key(KeyEvent::with_modifiers(
            KeyCode::Backspace,
            KeyModifiers {
                alt: true,
                ..KeyModifiers::default()
            },
        )));
    }
    if second == BS {
        return Some(Event::Key(KeyEvent::with_modifiers(
            KeyCode::Backspace,
            KeyModifiers {
                alt: true,
                ..KeyModifiers::default()
            },
        )));
    }

    // Special two-byte sequences
    match second {
        // Alt+Enter: ESC \r (only in non-Kitty mode; Kitty mode handled above)
        b'\r' if !kitty_active() => {
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Enter,
                KeyModifiers {
                    alt: true,
                    ..KeyModifiers::default()
                },
            )));
        }
        // Alt+Space: ESC ' '
        b' ' => {
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Char(' '),
                KeyModifiers {
                    alt: true,
                    ..KeyModifiers::default()
                },
            )));
        }
        // Ctrl+\ : ESC Ctrl+\
        0x1c => {
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Char('\\'),
                KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::default()
                },
            )));
        }
        // Ctrl+] : ESC Ctrl+]
        0x1d => {
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Char(']'),
                KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::default()
                },
            )));
        }
        // Ctrl+- : ESC Ctrl+_
        0x1f => {
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Char('-'),
                KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::default()
                },
            )));
        }
        // Ctrl+[ : ESC ESC (double escape → ctrl+alt+[)
        ESC => {
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Char('['),
                KeyModifiers {
                    ctrl: true,
                    alt: true,
                    ..KeyModifiers::default()
                },
            )));
        }
        // Alt+Up: ESC p
        b'p' => {
            return Some(alt_key(KeyCode::Up));
        }
        // Alt+Down: ESC n
        b'n' => {
            return Some(alt_key(KeyCode::Down));
        }
        // Alt+Left: ESC b (legacy, only when Kitty not active)
        b'b' if !kitty_active() => {
            return Some(alt_key(KeyCode::Left));
        }
        // Alt+Right: ESC f (legacy, only when Kitty not active)
        b'f' if !kitty_active() => {
            return Some(alt_key(KeyCode::Right));
        }
        _ => {}
    }

    None
}

/// Parse a raw (non-ESC) byte sequence: single character or UTF-8 character.
fn parse_raw_char(data: &[u8]) -> Option<Event> {
    if data.len() == 1 {
        let b = data[0];
        // Enter
        if b == b'\r' {
            return Some(Event::key(KeyCode::Enter));
        }
        // LF → Enter (non-Kitty)
        if b == b'\n' && !kitty_active() {
            return Some(Event::key(KeyCode::Enter));
        }
        // Tab
        if b == b'\t' {
            return Some(Event::key(KeyCode::Tab));
        }
        // NUL → Ctrl+Space
        if b == 0 {
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Char(' '),
                KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::default()
                },
            )));
        }
        // DEL → Backspace
        if b == DEL {
            return Some(Event::key(KeyCode::Backspace));
        }
        // BS → Backspace (or Ctrl+Backspace on Windows Terminal)
        if b == BS {
            return Some(Event::key(KeyCode::Backspace));
        }
        // Ctrl+letter: raw 1..26
        if (1..=26).contains(&b) {
            let ch = char::from(b + 96);
            return Some(Event::Key(KeyEvent::with_modifiers(
                KeyCode::Char(ch),
                KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::default()
                },
            )));
        }
        // Printable ASCII (32..126)
        if (32..=126).contains(&b) {
            return Some(Event::key(KeyCode::Char(b as char)));
        }
    }

    // Multi-byte UTF-8
    if data.len() > 1 {
        if let Some(ch) = decode_utf8_char(data) {
            return Some(Event::key(KeyCode::Char(ch)));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn shift_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::with_modifiers(
        code,
        KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        },
    ))
}

fn ctrl_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::with_modifiers(
        code,
        KeyModifiers {
            ctrl: true,
            ..KeyModifiers::default()
        },
    ))
}

fn alt_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::with_modifiers(
        code,
        KeyModifiers {
            alt: true,
            ..KeyModifiers::default()
        },
    ))
}

// ---------------------------------------------------------------------------
// Key string matching
// ---------------------------------------------------------------------------

/// Parse a key identifier string like `"ctrl+c"` or `"shift+enter"` into
/// its components. Returns owned string for the key part.
fn parse_key_id(key_id: &str) -> Option<(String, bool, bool, bool, bool)> {
    let lower = key_id.to_lowercase();
    let parts: Vec<&str> = lower.split('+').collect();
    let key = (*parts.last()?).to_string();
    Some((
        key,
        parts.contains(&"ctrl"),
        parts.contains(&"shift"),
        parts.contains(&"alt"),
        parts.contains(&"super"),
    ))
}

/// Build a modifier bitmask from parsed components.
fn modifier_bits_from_parts(ctrl: bool, shift: bool, alt: bool, super_: bool) -> u8 {
    let mut m = 0u8;
    if shift {
        m |= modifier_bits::SHIFT;
    }
    if alt {
        m |= modifier_bits::ALT;
    }
    if ctrl {
        m |= modifier_bits::CTRL;
    }
    if super_ {
        m |= modifier_bits::SUPER;
    }
    m
}

/// Convert a key name string to its codepoint value.
fn key_name_to_codepoint(key: &str) -> Option<i64> {
    match key {
        "escape" | "esc" => Some(codepoints::ESCAPE),
        "space" => Some(codepoints::SPACE),
        "tab" => Some(codepoints::TAB),
        "enter" | "return" => Some(codepoints::ENTER),
        "backspace" => Some(codepoints::BACKSPACE),
        "up" => Some(codepoints::UP),
        "down" => Some(codepoints::DOWN),
        "left" => Some(codepoints::LEFT),
        "right" => Some(codepoints::RIGHT),
        "insert" => Some(codepoints::INSERT),
        "delete" => Some(codepoints::DELETE),
        "home" => Some(codepoints::HOME),
        "end" => Some(codepoints::END),
        "pageup" => Some(codepoints::PAGE_UP),
        "pagedown" => Some(codepoints::PAGE_DOWN),
        s if s.starts_with('f') => {
            // f1..f12 – not a single codepoint; handle at match level
            None
        }
        _ => None,
    }
}

/// Match a raw byte sequence against a key identifier string.
///
/// This is the Rust equivalent of the TypeScript `matchesKey(data, keyId)`.
pub fn matches_key(data: &[u8], key_id: &str) -> bool {
    let Some((key, ctrl, shift, alt, super_)) = parse_key_id(key_id) else {
        return false;
    };
    let modifier = modifier_bits_from_parts(ctrl, shift, alt, super_);

    // Helper: try Kitty match
    let try_kitty = |expected_cp: i64, expected_mod: u8| -> bool {
        let Some(kitty) = parse_kitty_sequence(data) else {
            return false;
        };
        let actual_mod = effective_modifier(kitty.modifier);
        let expected_mod = effective_modifier(expected_mod);
        if actual_mod != expected_mod {
            return false;
        }
        let actual_cp = normalize_shifted_letter(
            normalize_kitty_functional_codepoint(kitty.codepoint),
            kitty.modifier,
        );
        let expected_cp = normalize_shifted_letter(
            normalize_kitty_functional_codepoint(expected_cp),
            expected_mod,
        );
        if actual_cp == expected_cp {
            return true;
        }
        // Base layout key fallback
        if let Some(base) = kitty.base_layout_key {
            if base == expected_cp {
                let cp = actual_cp;
                let is_latin = (97..=122).contains(&cp);
                let is_digit = (48..=57).contains(&cp);
                let is_symbol = is_symbol_cp(cp);
                if !is_latin && !is_digit && !is_symbol {
                    return true;
                }
            }
        }
        false
    };

    // Helper: try modifyOtherKeys match
    let try_mok = |expected_cp: i64, expected_mod: u8| -> bool {
        let Some(mok) = parse_modify_other_keys(data) else {
            return false;
        };
        let actual_mod = effective_modifier(mok.modifier);
        let expected_mod = effective_modifier(expected_mod);
        if actual_mod != expected_mod {
            return false;
        }
        let actual = normalize_shifted_letter(mok.codepoint, mok.modifier);
        let expected = normalize_shifted_letter(expected_cp, expected_mod);
        actual == expected
    };

    match key.as_str() {
        "escape" | "esc" => {
            if modifier != 0 {
                return false;
            }
            if data == b"\x1b" {
                return true;
            }
            if try_kitty(codepoints::ESCAPE, 0) {
                return true;
            }
            if try_mok(codepoints::ESCAPE, 0) {
                return true;
            }
            false
        }
        "space" => {
            if !kitty_active() {
                if modifier == modifier_bits::CTRL && data == b"\x00" {
                    return true;
                }
                if modifier == modifier_bits::ALT && data == b"\x1b " {
                    return true;
                }
            }
            if modifier == 0 && (data == b" " || try_kitty(codepoints::SPACE, 0) || try_mok(codepoints::SPACE, 0)) {
                return true;
            }
            try_kitty(codepoints::SPACE, modifier) || try_mok(codepoints::SPACE, modifier)
        }
        "tab" => {
            if modifier == modifier_bits::SHIFT {
                return data == b"\x1b[Z"
                    || try_kitty(codepoints::TAB, modifier_bits::SHIFT)
                    || try_mok(codepoints::TAB, modifier_bits::SHIFT);
            }
            if modifier == 0 {
                return data == b"\t" || try_kitty(codepoints::TAB, 0);
            }
            try_kitty(codepoints::TAB, modifier) || try_mok(codepoints::TAB, modifier)
        }
        "enter" | "return" => {
            if modifier == modifier_bits::SHIFT {
                if try_kitty(codepoints::ENTER, modifier_bits::SHIFT)
                    || try_kitty(codepoints::KP_ENTER, modifier_bits::SHIFT)
                    || try_mok(codepoints::ENTER, modifier_bits::SHIFT)
                {
                    return true;
                }
                if kitty_active() {
                    return data == b"\x1b\r" || data == b"\n";
                }
                return false;
            }
            if modifier == modifier_bits::ALT {
                if try_kitty(codepoints::ENTER, modifier_bits::ALT)
                    || try_kitty(codepoints::KP_ENTER, modifier_bits::ALT)
                    || try_mok(codepoints::ENTER, modifier_bits::ALT)
                {
                    return true;
                }
                if !kitty_active() {
                    return data == b"\x1b\r";
                }
                return false;
            }
            if modifier == 0
                && (data == b"\r"
                    || (!kitty_active() && data == b"\n")
                    || data == b"\x1bOM"
                    || try_kitty(codepoints::ENTER, 0)
                    || try_kitty(codepoints::KP_ENTER, 0))
                {
                    return true;
                }
            try_kitty(codepoints::ENTER, modifier)
                || try_kitty(codepoints::KP_ENTER, modifier)
                || try_mok(codepoints::ENTER, modifier)
        }
        "backspace" => {
            if modifier == modifier_bits::ALT {
                if data == b"\x1b\x7f" || data == b"\x1b\x08" {
                    return true;
                }
                return try_kitty(codepoints::BACKSPACE, modifier_bits::ALT)
                    || try_mok(codepoints::BACKSPACE, modifier_bits::ALT);
            }
            if modifier == modifier_bits::CTRL {
                if data == b"\x08" || data == b"\x7f" {
                    // Simplified: treat raw BS as ctrl+backspace when modifier requested
                    return true;
                }
                return try_kitty(codepoints::BACKSPACE, modifier_bits::CTRL)
                    || try_mok(codepoints::BACKSPACE, modifier_bits::CTRL);
            }
            if modifier == 0 {
                if data == b"\x7f" || data == b"\x08" {
                    return true;
                }
                return try_kitty(codepoints::BACKSPACE, 0) || try_mok(codepoints::BACKSPACE, 0);
            }
            try_kitty(codepoints::BACKSPACE, modifier) || try_mok(codepoints::BACKSPACE, modifier)
        }
        _ => {
            // Generic: try Kitty, then modifyOtherKeys, then raw comparison
            // For single-char keys
            if key.len() == 1 {
                let Some(ch) = key.chars().next() else {
                    return false;
                };
                let cp = ch as i64;
                let is_letter = ch.is_ascii_lowercase();
                let is_digit = ch.is_ascii_digit();
                let is_symbol = is_symbol_cp(cp);

                if is_letter || is_digit || is_symbol {
                    // Ctrl+Alt legacy: ESC + ctrl_char
                    if modifier == modifier_bits::CTRL | modifier_bits::ALT
                        && !kitty_active()
                    {
                        if let Some(ctrl_ch) = raw_ctrl_char(ch) {
                            if data.len() == 2 && data[0] == ESC && data[1] == ctrl_ch as u8 {
                                return true;
                            }
                        }
                    }
                    // Alt+letter/digit legacy: ESC + char
                    if modifier == modifier_bits::ALT
                        && !kitty_active()
                        && (is_letter || is_digit)
                        && data.len() == 2 && data[0] == ESC && data[1] == ch as u8 {
                            return true;
                        }
                    // Ctrl legacy: raw ctrl char
                    if modifier == modifier_bits::CTRL {
                        if let Some(ctrl_ch) = raw_ctrl_char(ch) {
                            if data.len() == 1 && data[0] == ctrl_ch as u8 {
                                return true;
                            }
                        }
                        return try_kitty(cp, modifier_bits::CTRL)
                            || try_mok(cp, modifier_bits::CTRL);
                    }
                    // Shift+Ctrl
                    if modifier == modifier_bits::SHIFT | modifier_bits::CTRL {
                        return try_kitty(cp, modifier_bits::SHIFT | modifier_bits::CTRL)
                            || try_mok(cp, modifier_bits::SHIFT | modifier_bits::CTRL);
                    }
                    // Shift legacy: uppercase
                    if modifier == modifier_bits::SHIFT {
                        if is_letter && data.len() == 1 && data[0] == ch.to_ascii_uppercase() as u8 {
                            return true;
                        }
                        return try_kitty(cp, modifier_bits::SHIFT)
                            || try_mok(cp, modifier_bits::SHIFT);
                    }
                    // Other modifiers
                    if modifier != 0 {
                        return try_kitty(cp, modifier) || try_mok(cp, modifier);
                    }
                    // No modifier: raw char or Kitty
                    if data.len() == 1 && data[0] == ch as u8 {
                        return true;
                    }
                    // UTF-8 multi-byte
                    if let Ok(s) = std::str::from_utf8(data) {
                        if s == key {
                            return true;
                        }
                    }
                    return try_kitty(cp, 0);
                }
            }

            // Function keys
            if key.starts_with('f') && key.len() <= 3 {
                if let Ok(num) = key[1..].parse::<u8>() {
                    if (1..=12).contains(&num) && modifier == 0 {
                        return matches_legacy_fn(data, num);
                    }
                }
            }

            // Arrow / navigation keys
            if let Some(cp) = key_name_to_codepoint(&key) {
                if modifier == 0 {
                    return matches_legacy_cp(data, &key) || try_kitty(cp, 0);
                }
                if matches_legacy_modifier_cp(data, &key, modifier) {
                    return true;
                }
                return try_kitty(cp, modifier);
            }

            false
        }
    }
}

/// Get the raw ctrl character for a key.
fn raw_ctrl_char(ch: char) -> Option<char> {
    let lower = ch.to_ascii_lowercase();
    let code = lower as u32;
    if (97..=122).contains(&code)
        || lower == '['
        || lower == '\\'
        || lower == ']'
        || lower == '_'
    {
        Some(char::from((code & 0x1f) as u8))
    } else if lower == '-' {
        Some(char::from(31u8)) // same as Ctrl+_
    } else {
        None
    }
}

/// Check if data matches a legacy function key sequence.
fn matches_legacy_fn(data: &[u8], num: u8) -> bool {
    let candidates: &[&[u8]] = match num {
        1 => &[b"\x1bOP", b"\x1b[11~", b"\x1b[[A"],
        2 => &[b"\x1bOQ", b"\x1b[12~", b"\x1b[[B"],
        3 => &[b"\x1bOR", b"\x1b[13~", b"\x1b[[C"],
        4 => &[b"\x1bOS", b"\x1b[14~", b"\x1b[[D"],
        5 => &[b"\x1b[15~", b"\x1b[[E"],
        6 => &[b"\x1b[17~"],
        7 => &[b"\x1b[18~"],
        8 => &[b"\x1b[19~"],
        9 => &[b"\x1b[20~"],
        10 => &[b"\x1b[21~"],
        11 => &[b"\x1b[23~"],
        12 => &[b"\x1b[24~"],
        _ => &[],
    };
    candidates.contains(&data)
}

/// Check if data matches a legacy unmodified key sequence.
fn matches_legacy_cp(data: &[u8], key: &str) -> bool {
    let seqs: &[&[u8]] = match key {
        "up" => &[b"\x1b[A", b"\x1bOA"],
        "down" => &[b"\x1b[B", b"\x1bOB"],
        "right" => &[b"\x1b[C", b"\x1bOC"],
        "left" => &[b"\x1b[D", b"\x1bOD"],
        "home" => &[b"\x1b[H", b"\x1bOH", b"\x1b[1~", b"\x1b[7~"],
        "end" => &[b"\x1b[F", b"\x1bOF", b"\x1b[4~", b"\x1b[8~"],
        "insert" => &[b"\x1b[2~"],
        "delete" => &[b"\x1b[3~"],
        "pageup" => &[b"\x1b[5~", b"\x1b[[5~"],
        "pagedown" => &[b"\x1b[6~", b"\x1b[[6~"],
        _ => &[],
    };
    seqs.contains(&data)
}

/// Check if data matches a legacy modified key sequence (shift or ctrl).
fn matches_legacy_modifier_cp(data: &[u8], key: &str, modifier: u8) -> bool {
    if modifier == modifier_bits::SHIFT {
        let seqs: &[&[u8]] = match key {
            "up" => &[b"\x1b[a"],
            "down" => &[b"\x1b[b"],
            "right" => &[b"\x1b[c"],
            "left" => &[b"\x1b[d"],
            "insert" => &[b"\x1b[2$"],
            "delete" => &[b"\x1b[3$"],
            "pageup" => &[b"\x1b[5$"],
            "pagedown" => &[b"\x1b[6$"],
            "home" => &[b"\x1b[7$"],
            "end" => &[b"\x1b[8$"],
            _ => &[],
        };
        return seqs.contains(&data);
    }
    if modifier == modifier_bits::CTRL {
        let seqs: &[&[u8]] = match key {
            "up" => &[b"\x1bOa"],
            "down" => &[b"\x1bOb"],
            "right" => &[b"\x1bOc"],
            "left" => &[b"\x1bOd"],
            "insert" => &[b"\x1b[2^"],
            "delete" => &[b"\x1b[3^"],
            "pageup" => &[b"\x1b[5^"],
            "pagedown" => &[b"\x1b[6^"],
            "home" => &[b"\x1b[7^"],
            "end" => &[b"\x1b[8^"],
            _ => &[],
        };
        return seqs.contains(&data);
    }
    false
}

/// Parse a raw byte sequence and return a human-readable key identifier
/// string like `"ctrl+c"`, `"shift+enter"`, etc.
pub fn parse_key(data: &[u8]) -> Option<String> {
    // Try Kitty first
    if let Some(kitty) = parse_kitty_sequence(data) {
        return format_kitty_key(&kitty);
    }

    // Try modifyOtherKeys
    if let Some(mok) = parse_modify_other_keys(data) {
        return format_codepoint_key(mok.codepoint, mok.modifier);
    }

    let s = std::str::from_utf8(data).ok()?;

    // Kitty-active legacy
    if kitty_active()
        && (data == b"\x1b\r" || data == b"\n") {
            return Some("shift+enter".to_string());
        }

    // Legacy sequence map
    if let Some(id) = legacy_sequence_id(s) {
        return Some(id.to_string());
    }

    // Bare ESC
    if data == b"\x1b" {
        return Some("escape".to_string());
    }
    // Ctrl+\
    if data == b"\x1c" {
        return Some("ctrl+\\".to_string());
    }
    // Ctrl+]
    if data == b"\x1d" {
        return Some("ctrl+]".to_string());
    }
    // Ctrl+-
    if data == b"\x1f" {
        return Some("ctrl+-".to_string());
    }
    // Tab
    if data == b"\t" {
        return Some("tab".to_string());
    }
    // Enter
    if data == b"\r" || (!kitty_active() && data == b"\n") || data == b"\x1bOM" {
        return Some("enter".to_string());
    }
    // Ctrl+Space
    if data == b"\x00" {
        return Some("ctrl+space".to_string());
    }
    // Space
    if data == b" " {
        return Some("space".to_string());
    }
    // Backspace
    if data == b"\x7f" || data == b"\x08" {
        return Some("backspace".to_string());
    }
    // Shift+Tab
    if data == b"\x1b[Z" {
        return Some("shift+tab".to_string());
    }
    // Alt+Enter (non-Kitty)
    if !kitty_active() && data == b"\x1b\r" {
        return Some("alt+enter".to_string());
    }
    // Alt+Space (non-Kitty)
    if !kitty_active() && data == b"\x1b " {
        return Some("alt+space".to_string());
    }
    // Alt+Backspace
    if data == b"\x1b\x7f" || data == b"\x1b\x08" {
        return Some("alt+backspace".to_string());
    }
    // Alt+Left (legacy)
    if !kitty_active() && data == b"\x1bB" {
        return Some("alt+left".to_string());
    }
    // Alt+Right (legacy)
    if !kitty_active() && data == b"\x1bF" {
        return Some("alt+right".to_string());
    }
    // Ctrl+Alt+letter / Alt+letter from ESC prefix
    if data.len() == 2 && data[0] == ESC {
        let code = data[1];
        if (1..=26).contains(&code) {
            return Some(format!("ctrl+alt+{}", char::from(code + 96)));
        }
        if code.is_ascii_lowercase() || code.is_ascii_digit() {
            return Some(format!("alt+{}", code as char));
        }
    }
    // Ctrl+Alt+specials
    if data == b"\x1b\x1b" {
        return Some("ctrl+alt+[".to_string());
    }
    if data == b"\x1b\x1c" {
        return Some("ctrl+alt+\\".to_string());
    }
    if data == b"\x1b\x1d" {
        return Some("ctrl+alt+]".to_string());
    }
    if data == b"\x1b\x1f" {
        return Some("ctrl+alt+-".to_string());
    }

    // Remaining legacy CSI sequences (already covered by legacy_sequence_id mostly)
    if data == b"\x1b[A" {
        return Some("up".to_string());
    }
    if data == b"\x1b[B" {
        return Some("down".to_string());
    }
    if data == b"\x1b[C" {
        return Some("right".to_string());
    }
    if data == b"\x1b[D" {
        return Some("left".to_string());
    }
    if data == b"\x1b[H" || data == b"\x1bOH" {
        return Some("home".to_string());
    }
    if data == b"\x1b[F" || data == b"\x1bOF" {
        return Some("end".to_string());
    }
    if data == b"\x1b[3~" {
        return Some("delete".to_string());
    }
    if data == b"\x1b[5~" {
        return Some("pageup".to_string());
    }
    if data == b"\x1b[6~" {
        return Some("pagedown".to_string());
    }

    // Raw Ctrl+letter
    if data.len() == 1 {
        let code = data[0];
        if (1..=26).contains(&code) {
            return Some(format!("ctrl+{}", char::from(code + 96)));
        }
        if (32..=126).contains(&code) {
            return Some((code as char).to_string());
        }
    }

    // Multi-byte UTF-8 printable
    if data.len() > 1 {
        if let Ok(s) = std::str::from_utf8(data) {
            if s.chars().all(|c| c >= ' ') {
                return Some(s.to_string());
            }
        }
    }

    None
}

/// Legacy sequence → key ID lookup.
fn legacy_sequence_id(s: &str) -> Option<&'static str> {
    Some(match s {
        "\x1bOA" => "up",
        "\x1bOB" => "down",
        "\x1bOC" => "right",
        "\x1bOD" => "left",
        "\x1bOH" => "home",
        "\x1bOF" => "end",
        "\x1b[E" => "clear",
        "\x1bOE" => "clear",
        "\x1bOe" => "ctrl+clear",
        "\x1b[e" => "shift+clear",
        "\x1b[2~" => "insert",
        "\x1b[2$" => "shift+insert",
        "\x1b[2^" => "ctrl+insert",
        "\x1b[3$" => "shift+delete",
        "\x1b[3^" => "ctrl+delete",
        "\x1b[[5~" => "pageup",
        "\x1b[[6~" => "pagedown",
        "\x1b[a" => "shift+up",
        "\x1b[b" => "shift+down",
        "\x1b[c" => "shift+right",
        "\x1b[d" => "shift+left",
        "\x1bOa" => "ctrl+up",
        "\x1bOb" => "ctrl+down",
        "\x1bOc" => "ctrl+right",
        "\x1bOd" => "ctrl+left",
        "\x1b[5$" => "shift+pageup",
        "\x1b[6$" => "shift+pagedown",
        "\x1b[7$" => "shift+home",
        "\x1b[8$" => "shift+end",
        "\x1b[5^" => "ctrl+pageup",
        "\x1b[6^" => "ctrl+pagedown",
        "\x1b[7^" => "ctrl+home",
        "\x1b[8^" => "ctrl+end",
        "\x1bOP" => "f1",
        "\x1bOQ" => "f2",
        "\x1bOR" => "f3",
        "\x1bOS" => "f4",
        "\x1b[11~" => "f1",
        "\x1b[12~" => "f2",
        "\x1b[13~" => "f3",
        "\x1b[14~" => "f4",
        "\x1b[[A" => "f1",
        "\x1b[[B" => "f2",
        "\x1b[[C" => "f3",
        "\x1b[[D" => "f4",
        "\x1b[[E" => "f5",
        "\x1b[15~" => "f5",
        "\x1b[17~" => "f6",
        "\x1b[18~" => "f7",
        "\x1b[19~" => "f8",
        "\x1b[20~" => "f9",
        "\x1b[21~" => "f10",
        "\x1b[23~" => "f11",
        "\x1b[24~" => "f12",
        "\x1bb" => "alt+left",
        "\x1bf" => "alt+right",
        "\x1bp" => "alt+up",
        "\x1bn" => "alt+down",
        _ => return None,
    })
}

/// Format a Kitty-parsed key into a human-readable identifier.
fn format_kitty_key(kitty: &ParsedKitty) -> Option<String> {
    format_codepoint_key_impl(
        normalize_kitty_functional_codepoint(kitty.codepoint),
        kitty.modifier,
        kitty.base_layout_key,
    )
}

/// Format a codepoint + modifier into a human-readable key identifier.
fn format_codepoint_key(cp: i64, modifier: u8) -> Option<String> {
    format_codepoint_key_impl(cp, modifier, None)
}

fn format_codepoint_key_impl(cp: i64, modifier: u8, base_layout_key: Option<i64>) -> Option<String> {
    let normalized = normalize_kitty_functional_codepoint(cp);
    let identity = normalize_shifted_letter(normalized, modifier);

    let is_latin = (97..=122).contains(&identity);
    let is_digit = (48..=57).contains(&identity);
    let is_symbol = is_symbol_cp(identity);
    let effective = if is_latin || is_digit || is_symbol {
        identity
    } else {
        base_layout_key.unwrap_or(identity)
    };

    let key_name = codepoint_to_key_name(effective)?;

    let eff_mod = effective_modifier(modifier);
    let supported = modifier_bits::SHIFT
        | modifier_bits::CTRL
        | modifier_bits::ALT
        | modifier_bits::SUPER;
    if (eff_mod & !supported) != 0 {
        return None;
    }

    let mut parts: Vec<&str> = Vec::new();
    if (eff_mod & modifier_bits::SHIFT) != 0 {
        parts.push("shift");
    }
    if (eff_mod & modifier_bits::CTRL) != 0 {
        parts.push("ctrl");
    }
    if (eff_mod & modifier_bits::ALT) != 0 {
        parts.push("alt");
    }
    if (eff_mod & modifier_bits::SUPER) != 0 {
        parts.push("super");
    }

    if parts.is_empty() {
        Some(key_name)
    } else {
        let mut result = parts.join("+");
        result.push('+');
        result.push_str(&key_name);
        Some(result)
    }
}

/// Convert a codepoint to a human-readable key name.
fn codepoint_to_key_name(cp: i64) -> Option<String> {
    match cp {
        codepoints::ESCAPE => Some("escape".to_string()),
        codepoints::TAB => Some("tab".to_string()),
        codepoints::ENTER | codepoints::KP_ENTER => Some("enter".to_string()),
        codepoints::SPACE => Some("space".to_string()),
        codepoints::BACKSPACE => Some("backspace".to_string()),
        codepoints::DELETE => Some("delete".to_string()),
        codepoints::INSERT => Some("insert".to_string()),
        codepoints::HOME => Some("home".to_string()),
        codepoints::END => Some("end".to_string()),
        codepoints::PAGE_UP => Some("pageup".to_string()),
        codepoints::PAGE_DOWN => Some("pagedown".to_string()),
        codepoints::UP => Some("up".to_string()),
        codepoints::DOWN => Some("down".to_string()),
        codepoints::LEFT => Some("left".to_string()),
        codepoints::RIGHT => Some("right".to_string()),
        cp if (32..=126).contains(&cp) => Some((cp as u8 as char).to_string()),
        cp => char::from_u32(cp as u32).map(|c| c.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain_char() {
        let event = parse_event(b"a").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('a'),
                modifiers,
                ..
            }) => {
                assert!(!modifiers.ctrl);
                assert!(!modifiers.alt);
                assert!(!modifiers.shift);
            }
            _ => panic!("expected Char('a'), got {:?}", event),
        }
    }

    #[test]
    fn test_parse_escape() {
        let event = parse_event(b"\x1b").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Escape,
                ..
            }) => {}
            _ => panic!("expected Escape"),
        }
    }

    #[test]
    fn test_parse_enter() {
        let event = parse_event(b"\r").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {}
            _ => panic!("expected Enter"),
        }
    }

    #[test]
    fn test_parse_tab() {
        let event = parse_event(b"\t").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Tab,
                ..
            }) => {}
            _ => panic!("expected Tab"),
        }
    }

    #[test]
    fn test_parse_backspace() {
        let event = parse_event(b"\x7f").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {}
            _ => panic!("expected Backspace"),
        }
    }

    #[test]
    fn test_parse_ctrl_c() {
        let event = parse_event(b"\x03").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers,
            }) => {
                assert!(modifiers.ctrl);
            }
            _ => panic!("expected Ctrl+C"),
        }
    }

    #[test]
    fn test_parse_ctrl_space() {
        let event = parse_event(b"\x00").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char(' '),
                modifiers,
            }) => {
                assert!(modifiers.ctrl);
            }
            _ => panic!("expected Ctrl+Space"),
        }
    }

    #[test]
    fn test_parse_arrow_up_csi() {
        let event = parse_event(b"\x1b[A").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Up,
                modifiers,
            }) => {
                assert!(!modifiers.shift && !modifiers.ctrl && !modifiers.alt);
            }
            _ => panic!("expected Up"),
        }
    }

    #[test]
    fn test_parse_arrow_up_ss3() {
        let event = parse_event(b"\x1bOA").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) => {}
            _ => panic!("expected Up"),
        }
    }

    #[test]
    fn test_parse_f1_ss3() {
        let event = parse_event(b"\x1bOP").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::F(1), ..
            }) => {}
            _ => panic!("expected F(1)"),
        }
    }

    #[test]
    fn test_parse_f5_csi() {
        let event = parse_event(b"\x1b[15~").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::F(5), ..
            }) => {}
            _ => panic!("expected F(5)"),
        }
    }

    #[test]
    fn test_parse_home() {
        let event = parse_event(b"\x1b[H").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Home, ..
            }) => {}
            _ => panic!("expected Home"),
        }
    }

    #[test]
    fn test_parse_end() {
        let event = parse_event(b"\x1b[F").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::End, ..
            }) => {}
            _ => panic!("expected End"),
        }
    }

    #[test]
    fn test_parse_insert() {
        let event = parse_event(b"\x1b[2~").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Insert, ..
            }) => {}
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn test_parse_delete() {
        let event = parse_event(b"\x1b[3~").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Delete, ..
            }) => {}
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn test_parse_pageup() {
        let event = parse_event(b"\x1b[5~").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::PageUp, ..
            }) => {}
            _ => panic!("expected PageUp"),
        }
    }

    #[test]
    fn test_parse_pagedown() {
        let event = parse_event(b"\x1b[6~").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::PageDown, ..
            }) => {}
            _ => panic!("expected PageDown"),
        }
    }

    #[test]
    fn test_parse_shift_tab() {
        let event = parse_event(b"\x1b[Z").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Tab,
                modifiers,
            }) => {
                assert!(modifiers.shift);
            }
            _ => panic!("expected Shift+Tab"),
        }
    }

    #[test]
    fn test_parse_alt_letter() {
        let event = parse_event(b"\x1ba").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('a'),
                modifiers,
            }) => {
                assert!(modifiers.alt);
                assert!(!modifiers.ctrl);
            }
            _ => panic!("expected Alt+a"),
        }
    }

    #[test]
    fn test_parse_alt_backspace() {
        let event = parse_event(b"\x1b\x7f").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                modifiers,
            }) => {
                assert!(modifiers.alt);
            }
            _ => panic!("expected Alt+Backspace"),
        }
    }

    #[test]
    fn test_parse_focus_gained() {
        let event = parse_event(b"\x1b[I").unwrap();
        assert_eq!(event, Event::FocusGained);
    }

    #[test]
    fn test_parse_focus_lost() {
        let event = parse_event(b"\x1b[O").unwrap();
        assert_eq!(event, Event::FocusLost);
    }

    #[test]
    fn test_parse_sgr_mouse_press() {
        let event = parse_event(b"\x1b[<0;10;5M").unwrap();
        match event {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Press,
                button: MouseButton::Left,
                row: 4,
                col: 9,
            }) => {}
            _ => panic!("expected left mouse press at (9,4), got {:?}", event),
        }
    }

    #[test]
    fn test_parse_sgr_mouse_release() {
        let event = parse_event(b"\x1b[<0;10;5m").unwrap();
        match event {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Release,
                button: MouseButton::Left,
                row: 4,
                col: 9,
            }) => {}
            _ => panic!("expected left mouse release"),
        }
    }

    #[test]
    fn test_parse_sgr_mouse_scroll() {
        let event = parse_event(b"\x1b[<64;10;5M").unwrap();
        match event {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                ..
            }) => {}
            _ => panic!("expected scroll up"),
        }
    }

    #[test]
    fn test_parse_x10_mouse() {
        let event = parse_event(b"\x1b[M \x21\x21").unwrap();
        match event {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Press,
                button: MouseButton::Left,
                ..
            }) => {}
            _ => panic!("expected X10 mouse press"),
        }
    }

    #[test]
    fn test_parse_kitty_arrow_up() {
        // Kitty: ESC [ -1 u (arrow up = codepoint -1)
        // Actually Kitty uses ESC[1;<mod>A for arrows, tested below
        let event = parse_event(b"\x1b[1;1A").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) => {}
            _ => panic!("expected Up from Kitty"),
        }
    }

    #[test]
    fn test_parse_kitty_ctrl_c() {
        // Kitty CSI-u: ESC [ 99 ; 5 u  (codepoint 99='c', modifier 5=ctrl+1)
        let event = parse_event(b"\x1b[99;5u").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers,
            }) => {
                assert!(modifiers.ctrl);
            }
            _ => panic!("expected Ctrl+c from Kitty"),
        }
    }

    #[test]
    fn test_parse_modify_other_keys_ctrl_c() {
        // xterm modifyOtherKeys: ESC [ 27 ; 5 ; 99 ~
        let event = parse_event(b"\x1b[27;5;99~").unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers,
            }) => {
                assert!(modifiers.ctrl);
            }
            _ => panic!("expected Ctrl+c from modifyOtherKeys"),
        }
    }

    #[test]
    fn test_decode_printable_kitty() {
        // Kitty CSI-u for plain 'a': ESC [ 97 u
        let ch = decode_kitty_printable(b"\x1b[97u").unwrap();
        assert_eq!(ch, 'a');
    }

    #[test]
    fn test_decode_printable_rejects_ctrl() {
        // Kitty CSI-u with ctrl modifier should not decode as printable
        assert!(decode_kitty_printable(b"\x1b[97;5u").is_none());
    }

    #[test]
    fn test_matches_key_escape() {
        assert!(matches_key(b"\x1b", "escape"));
        assert!(matches_key(b"\x1b", "esc"));
        assert!(!matches_key(b"\x1b", "ctrl+escape"));
    }

    #[test]
    fn test_matches_key_ctrl_c() {
        assert!(matches_key(b"\x03", "ctrl+c"));
        assert!(!matches_key(b"\x03", "ctrl+d"));
    }

    #[test]
    fn test_matches_key_enter() {
        assert!(matches_key(b"\r", "enter"));
        assert!(matches_key(b"\r", "return"));
    }

    #[test]
    fn test_matches_key_shift_tab() {
        assert!(matches_key(b"\x1b[Z", "shift+tab"));
    }

    #[test]
    fn test_matches_key_alt_a() {
        assert!(matches_key(b"\x1ba", "alt+a"));
    }

    #[test]
    fn test_matches_key_function() {
        assert!(matches_key(b"\x1bOP", "f1"));
        assert!(matches_key(b"\x1b[15~", "f5"));
    }

    #[test]
    fn test_parse_key_string() {
        assert_eq!(parse_key(b"\x1b"), Some("escape".to_string()));
        assert_eq!(parse_key(b"\r"), Some("enter".to_string()));
        assert_eq!(parse_key(b"\t"), Some("tab".to_string()));
        assert_eq!(parse_key(b"a"), Some("a".to_string()));
        assert_eq!(parse_key(b"\x03"), Some("ctrl+c".to_string()));
        assert_eq!(parse_key(b"\x1b[Z"), Some("shift+tab".to_string()));
        assert_eq!(parse_key(b"\x1b[A"), Some("up".to_string()));
        assert_eq!(parse_key(b"\x1b[B"), Some("down".to_string()));
        assert_eq!(parse_key(b"\x1b[C"), Some("right".to_string()));
        assert_eq!(parse_key(b"\x1b[D"), Some("left".to_string()));
    }

    #[test]
    fn test_is_key_release() {
        assert!(is_key_release(b"\x1b[97;1:3u"));
        assert!(!is_key_release(b"\x1b[97;1:1u"));
        assert!(!is_key_release(b"a"));
    }

    #[test]
    fn test_is_key_repeat() {
        assert!(is_key_repeat(b"\x1b[97;1:2u"));
        assert!(!is_key_repeat(b"\x1b[97;1:1u"));
    }

    #[test]
    fn test_utf8_multibyte() {
        // 'é' in UTF-8 is 0xC3 0xA9
        let event = parse_event("é".as_bytes()).unwrap();
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('é'),
                ..
            }) => {}
            _ => panic!("expected Char('é')"),
        }
    }
}
