//! Kitty keyboard protocol parser.
//!
//! The Kitty keyboard protocol encodes keys as `ESC [ <codepoint> ; <modifiers> [u|r|~]`
//! sequences. Compared to legacy xterm-style escapes (e.g. `ESC [ A` for up arrow) this
//! carries a full Unicode codepoint and explicit modifier bits, so terminals can
//! disambiguate Ctrl+I from Tab, send press/repeat/release events, and report function
//! keys / numpad keys / shifted symbols without ambiguity.
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
//!
//! This module is standalone — it owns its own [`KeyType`]/[`KeyModifiers`]/[`KeyEventKind`]
//! types and does not depend on crossterm, ratatui, or the crate's own
//! [`crate::keybindings::keys`] types. The point is to provide a complete
//! protocol-level parser usable from any input pipeline.
//!
//! Wire format (Kitty CSI-u, full):
//!
//! ```text
//! ESC [ <codepoint> [:<shifted>[:<base>]] [; <modifiers> [:<event_type> [; <text>]]] <terminator>
//! ```
//!
//! - `codepoint` — Unicode codepoint of the base key, or a special-key codepoint
//!   in the private use range (`57344..=65533`).
//! - `shifted` — codepoint produced when Shift is held (optional).
//! - `base` — base-layout key when an alternate layout is active (optional).
//! - `modifiers` — 1-indexed bitmask (the wire value is `real + 1`).
//! - `event_type` — `1` press, `2` repeat, `3` release (optional; absent = press).
//! - `terminator` — `u` for full mode, `~` for legacy CSI-u alias keys, A–D/H for
//!   arrow/Home/End/F1–F4 shortcut.

// ---------------------------------------------------------------------------
// KeyModifiers
// ---------------------------------------------------------------------------

/// Modifier bits carried in the Kitty `modifiers` field.
///
/// Modifiers are 1-indexed on the wire (bit 0 means "no modifiers"); we normalize
/// to a 0-based bitmask before storing here. Bit 6 (`CAPS_LOCK`) and
/// bit 7 (`NUM_LOCK`) are lock keys and intentionally NOT exposed — they
/// are stripped by [`parse_kitty_key`] so callers only see the four primary modifiers
/// plus the two extended modifiers (`Hyper`, `Meta`) that real terminals can set.
///
/// # Examples
///
/// ```
/// use oxi_tui::input::kitty::KeyModifiers;
///
/// let m = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
/// assert!(m.contains(KeyModifiers::CONTROL));
/// assert_eq!(m.to_string(), "Ctrl+Shift");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct KeyModifiers(pub u8);

impl KeyModifiers {
    /// Shift is held.
    pub const SHIFT: Self = Self(1);
    /// Alt (also called "Option" on macOS) is held.
    pub const ALT: Self = Self(2);
    /// Control is held.
    pub const CONTROL: Self = Self(4);
    /// Super (Windows / Cmd) is held.
    pub const SUPER: Self = Self(8);
    /// Hyper modifier (rarely bound; some X11 setups use it).
    pub const HYPER: Self = Self(16);
    /// Meta modifier (rarely bound; some X11 setups use it).
    pub const META: Self = Self(32);
    /// No modifiers.
    pub const NONE: Self = Self(0);

    /// Construct from a raw bitmask. No validation; use the `const` values for
    /// known modifiers or compose with `Self::SHIFT.0 | Self::ALT.0`.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Raw bitmask value.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns `true` when none of the six modifier bits are set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` when every bit in `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Bitwise union of two modifier sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for KeyModifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl std::ops::BitOrAssign for KeyModifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for KeyModifiers {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl std::fmt::Display for KeyModifiers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts: Vec<&'static str> = Vec::with_capacity(6);
        if self.contains(Self::CONTROL) {
            parts.push("Ctrl");
        }
        if self.contains(Self::ALT) {
            parts.push("Alt");
        }
        if self.contains(Self::SUPER) {
            parts.push("Super");
        }
        if self.contains(Self::HYPER) {
            parts.push("Hyper");
        }
        if self.contains(Self::META) {
            parts.push("Meta");
        }
        if self.contains(Self::SHIFT) {
            parts.push("Shift");
        }
        f.write_str(&parts.join("+"))
    }
}

// ---------------------------------------------------------------------------
// KeyEventKind
// ---------------------------------------------------------------------------

/// Kind of key event emitted by a terminal that supports release/repeat reporting.
///
/// Without the Kitty keyboard protocol (or with `disambiguateEscapeCodes=0`) terminals
/// only send press events, so most input pipelines only ever observe
/// [`KeyEventKind::Press`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyEventKind {
    /// Initial key press. This is the default for sequences that omit `:1`.
    Press,
    /// Key auto-repeat event while held. Only emitted with Kitty flag 2 enabled.
    Repeat,
    /// Key release event. Only emitted with Kitty flag 2 enabled.
    Release,
}

// ---------------------------------------------------------------------------
// KeyType
// ---------------------------------------------------------------------------

/// Logical key produced by [`parse_kitty_key`].
///
/// `Char` covers everything that maps to a Unicode code point (ASCII letters,
/// shifted symbols, numpad digits when NumLock is on, etc). The named variants
/// exist for the private-use codepoints Kitty uses for non-text keys so callers
/// don't have to hard-code `57344..=65533` ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyType {
    /// A Unicode character key (printable, non-control).
    Char(char),
    /// Enter / Return.
    Enter,
    /// Tab (note: Tab is also `Char('\t')` when sent as text; this variant is
    /// returned only when the codepoint is the Tab private-use key).
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Up arrow.
    ArrowUp,
    /// Down arrow.
    ArrowDown,
    /// Left arrow.
    ArrowLeft,
    /// Right arrow.
    ArrowRight,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Delete (forward delete).
    Delete,
    /// Insert.
    Insert,
    /// F1..=F12.
    F(u8),
    /// Spacebar — equivalent to `Char(' ')` but kept distinct so call sites that
    /// care about "is this a space" don't have to ASCII-check.
    Space,
}

impl std::fmt::Display for KeyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Char(c) => f.write_str(&c.to_string()),
            Self::Enter => f.write_str("Enter"),
            Self::Tab => f.write_str("Tab"),
            Self::Backspace => f.write_str("Backspace"),
            Self::Escape => f.write_str("Escape"),
            Self::ArrowUp => f.write_str("Up"),
            Self::ArrowDown => f.write_str("Down"),
            Self::ArrowLeft => f.write_str("Left"),
            Self::ArrowRight => f.write_str("Right"),
            Self::Home => f.write_str("Home"),
            Self::End => f.write_str("End"),
            Self::PageUp => f.write_str("PageUp"),
            Self::PageDown => f.write_str("PageDown"),
            Self::Delete => f.write_str("Delete"),
            Self::Insert => f.write_str("Insert"),
            Self::F(n) => write!(f, "F{n}"),
            Self::Space => f.write_str("Space"),
        }
    }
}

// ---------------------------------------------------------------------------
// ParsedKey
// ---------------------------------------------------------------------------

/// Result of a successful [`parse_kitty_key`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParsedKey {
    /// Logical key (letter, arrow, function key, etc).
    pub key: KeyType,
    /// Active modifiers at the time of the event.
    pub modifiers: KeyModifiers,
    /// Press / Repeat / Release.
    pub kind: KeyEventKind,
}

// ---------------------------------------------------------------------------
// Kitty special-key codepoint table (Unicode private-use area)
// ---------------------------------------------------------------------------
//
// These constants are kept private — callers must go through [`parse_kitty_key`]
// which is the only place they need to be consistent with the wire format.
// Kitty uses a contiguous range of PUA codepoints (57344..=65533) for
// non-character keys. Keeping the table here, in source-order, makes
// additions visible during code review.

/// Kitty private-use codepoint for Up arrow.
const KP_UP: u32 = 57_344;
/// Kitty private-use codepoint for Down arrow.
const KP_DOWN: u32 = 57_345;
/// Kitty private-use codepoint for Left arrow.
const KP_LEFT: u32 = 57_346;
/// Kitty private-use codepoint for Right arrow.
const KP_RIGHT: u32 = 57_347;
/// Kitty private-use codepoint for Insert.
const KP_INSERT: u32 = 57_348;
/// Kitty private-use codepoint for Delete.
const KP_DELETE: u32 = 57_349;
/// Kitty private-use codepoint for Page Up.
const KP_PAGE_UP: u32 = 57_350;
/// Kitty private-use codepoint for Page Down.
const KP_PAGE_DOWN: u32 = 57_351;
/// Kitty private-use codepoint for Home.
const KP_HOME: u32 = 57_352;
/// Kitty private-use codepoint for End.
const KP_END: u32 = 57_353;
/// Kitty private-use codepoint for Caps Lock.
const KP_CAPS_LOCK: u32 = 57_354;
/// Kitty private-use codepoint for Scroll Lock.
const KP_SCROLL_LOCK: u32 = 57_355;
/// Kitty private-use codepoint for Num Lock.
const KP_NUM_LOCK: u32 = 57_356;
/// Kitty private-use codepoint for Print Screen.
const KP_PRINT_SCREEN: u32 = 57_357;
/// Kitty private-use codepoint for Pause. Also F1 on the wire — they alias.
const KP_PAUSE_F1: u32 = 57_358;
/// Kitty private-use codepoint for F2.
const KP_F2: u32 = 57_359;
/// Kitty private-use codepoint for F3.
const KP_F3: u32 = 57_360;
/// Kitty private-use codepoint for F4.
const KP_F4: u32 = 57_361;
/// Kitty private-use codepoint for F5.
const KP_F5: u32 = 57_362;
/// Kitty private-use codepoint for F6.
const KP_F6: u32 = 57_363;
/// Kitty private-use codepoint for F7.
const KP_F7: u32 = 57_364;
/// Kitty private-use codepoint for F8.
const KP_F8: u32 = 57_365;
/// Kitty private-use codepoint for F9.
const KP_F9: u32 = 57_366;
/// Kitty private-use codepoint for F10.
const KP_F10: u32 = 57_367;
/// Kitty private-use codepoint for F11.
const KP_F11: u32 = 57_368;
/// Kitty private-use codepoint for F12.
const KP_F12: u32 = 57_369;
/// Kitty private-use codepoint for BackTab (Shift+Tab).
const KP_BACKTAB: u32 = 57_370;
/// First codepoint in the Kitty private-use range.
const KP_FIRST_SPECIAL: u32 = 57_344;
/// Last codepoint in the Kitty private-use range.
const KP_LAST_SPECIAL: u32 = 65_533;

/// Map a Kitty special-key codepoint to a [`KeyType`].
///
/// Returns `None` for unknown PUA codepoints so callers can decide whether to
/// fall back to the literal codepoint (rare; useful for forward compat with
/// terminal extensions).
fn map_special_key(cp: u32) -> Option<KeyType> {
    if !(KP_FIRST_SPECIAL..=KP_LAST_SPECIAL).contains(&cp) {
        return None;
    }
    match cp {
        KP_UP => Some(KeyType::ArrowUp),
        KP_DOWN => Some(KeyType::ArrowDown),
        KP_LEFT => Some(KeyType::ArrowLeft),
        KP_RIGHT => Some(KeyType::ArrowRight),
        KP_INSERT => Some(KeyType::Insert),
        KP_DELETE => Some(KeyType::Delete),
        KP_PAGE_UP => Some(KeyType::PageUp),
        KP_PAGE_DOWN => Some(KeyType::PageDown),
        KP_HOME => Some(KeyType::Home),
        KP_END => Some(KeyType::End),
        KP_PAUSE_F1 => Some(KeyType::F(1)),
        KP_F2 => Some(KeyType::F(2)),
        KP_F3 => Some(KeyType::F(3)),
        KP_F4 => Some(KeyType::F(4)),
        KP_F5 => Some(KeyType::F(5)),
        KP_F6 => Some(KeyType::F(6)),
        KP_F7 => Some(KeyType::F(7)),
        KP_F8 => Some(KeyType::F(8)),
        KP_F9 => Some(KeyType::F(9)),
        KP_F10 => Some(KeyType::F(10)),
        KP_F11 => Some(KeyType::F(11)),
        KP_F12 => Some(KeyType::F(12)),
        KP_BACKTAB => Some(KeyType::Tab),
        // Caps / Scroll / Num Lock / Print Screen: we don't model these as
        // "keys" — the protocol reports them but most consumers ignore.
        KP_CAPS_LOCK | KP_SCROLL_LOCK | KP_NUM_LOCK | KP_PRINT_SCREEN => None,
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a Kitty keyboard protocol CSI-u sequence.
///
/// Accepts the full mode (`u` terminator) and the alternative `~` and `A`–`H`
/// terminators for keys that have an xterm alias. Returns `None` for any
/// sequence that does not match the protocol.
///
/// The parser is forgiving: it tolerates sequences whose `modifiers` field is
/// missing (treated as `1`, i.e. "no modifiers") and whose `event_type` is
/// missing (treated as `Press`). It is strict about the `ESC [` prefix and
/// terminator — anything else is not a Kitty sequence.
///
/// # Examples
///
/// ```
/// use oxi_tui::input::kitty::{parse_kitty_key, KeyType, KeyModifiers};
///
/// // ESC [ 97 ; 1 u = 'a' with no modifiers
/// let k = parse_kitty_key(b"\x1b[97;1u").unwrap();
/// assert_eq!(k.key, KeyType::Char('a'));
/// assert_eq!(k.modifiers, KeyModifiers::NONE);
///
/// // ESC [ 97 ; 6 u = Ctrl+Shift+A (bits 1+2, 1-indexed = 6)
/// let k = parse_kitty_key(b"\x1b[97;6u").unwrap();
/// assert_eq!(k.key, KeyType::Char('a'));
/// assert!(k.modifiers.contains(KeyModifiers::CONTROL));
/// assert!(k.modifiers.contains(KeyModifiers::SHIFT));
/// ```
#[must_use]
pub fn parse_kitty_key(data: &[u8]) -> Option<ParsedKey> {
    // Minimum: \x1b[ X = 3 bytes (bare arrow shortcut) or
    //          \x1b[ <digit> ; <digit> u = 6 bytes (full CSI-u).
    if data.len() < 3 {
        return None;
    }
    if data[0] != 0x1b || data[1] != b'[' {
        return None;
    }

    // Terminator must be one of: u (full), ~ (legacy alias), A-H (xterm-arrow shortcuts).
    let last = *data.last()?;
    let terminator_ok = matches!(last, b'u' | b'~' | b'A' | b'B' | b'C' | b'D' | b'H');
    if !terminator_ok {
        return None;
    }

    // Bare arrow shortcut form: \x1b[ <terminator> with no other content.
    // The kitty protocol re-uses the xterm shortcut codes for the four arrows
    // and Home (H). Map the terminator back to the matching logical key.
    if data.len() == 3 {
        let key = match last {
            b'A' => KeyType::ArrowUp,
            b'B' => KeyType::ArrowDown,
            b'C' => KeyType::ArrowRight,
            b'D' => KeyType::ArrowLeft,
            b'H' => KeyType::Home,
            _ => return None,
        };
        return Some(ParsedKey {
            key,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
        });
    }

    // Full CSI-u sequences (`u` / `~` terminator) MUST include a `;`-separated
    // modifier field per the kitty protocol. Without it we can't tell a stray
    // digit-string from a real key, so reject rather than guess.
    let inner = &data[2..data.len() - 1];
    let s = std::str::from_utf8(inner).ok()?;

    let mut semi = s.split(';');
    let key_part = semi.next()?;
    let mod_part = match semi.next() {
        Some(m) => m,
        None if matches!(last, b'u' | b'~') => return None,
        None => "1",
    };
    // `key_part` may itself contain colon-separated sub-fields (the optional
    // shifted/base layout fields). They are not consumed here — only the
    // codepoint is needed.
    let mut key_fields = key_part.split(':');
    let codepoint_str = key_fields.next()?;
    let codepoint: u32 = codepoint_str.parse().ok()?;

    // Modifiers and event type both live on the modifier field, separated by
    // `:`. Modifiers are 1-indexed on the wire (bit 0 = "no modifiers");
    // subtract 1 to get the bitmask, then strip Caps/Num Lock bits (64+128)
    // so callers never see them.
    let mut mod_fields = mod_part.split(':');
    let raw_modifier: u32 = mod_fields.next().unwrap_or("1").parse().ok()?;
    let modifier_bits = raw_modifier.saturating_sub(1);
    let mask = modifier_bits & 0b11_111_111;
    let modifiers = KeyModifiers::from_bits((mask & 0x3f) as u8);

    // Event type: present as the second `:`-delimited field of `mod_part`
    // (i.e. `modifiers:event_type`). Absent → press. We deliberately do NOT
    // pull from `key_fields` — the kitty spec puts the event type on the
    // modifier side, and a shifted-codepoint field like `97:65` would be
    // misread otherwise.
    let event_type = mod_fields.next().and_then(|s| s.parse::<u8>().ok()).map_or(
        KeyEventKind::Press,
        |n| match n {
            2 => KeyEventKind::Repeat,
            3 => KeyEventKind::Release,
            // 1 or unknown → press. The protocol only defines 1/2/3.
            _ => KeyEventKind::Press,
        },
    );
    let key = if (KP_FIRST_SPECIAL..=KP_LAST_SPECIAL).contains(&codepoint) {
        // Special-key codepoint (arrow, F-key, etc). If we don't know it,
        // bail out rather than emit garbage.
        map_special_key(codepoint)?
    } else if codepoint == 32 {
        // Space: distinguish so callers don't have to ASCII-check.
        KeyType::Space
    } else {
        // Real Unicode codepoint. Reject C0 controls (other than TAB which is
        // 9 and not normally sent via Kitty) and DEL, since those mean
        // something else in the protocol and would render as zero-width.
        match char::from_u32(codepoint) {
            Some(c) if !c.is_control() => KeyType::Char(c),
            _ => return None,
        }
    };

    Some(ParsedKey {
        key,
        modifiers,
        kind: event_type,
    })
}

// ---------------------------------------------------------------------------
// Legacy key fallback
// ---------------------------------------------------------------------------

/// Match raw terminal input against a legacy xterm key name.
///
/// Legacy sequences are the pre-Kitty format: `ESC [ A` for up, `ESC [ 3~` for
/// delete, `\r` for enter, etc. Callers that want a unified key matcher feed
/// input to [`parse_kitty_key`] first and only fall back to this on `None`.
///
/// `expected` uses the same names as [`KeyType`]'s `Display` impl, e.g.
/// `"up"`, `"enter"`, `"f5"`. The function is case-insensitive for the key
/// portion (`"Up"`, `"UP"`, `"up"` all work).
///
/// Currently this matcher only handles bare keys (no modifiers). It returns
/// `false` for any `expected` that contains a modifier suffix, leaving modifier
/// handling to a future expansion that can also handle legacy xterm modifier
/// sequences (`ESC [ 1 ; 5 A` for Ctrl+Up).
///
/// # Examples
///
/// ```
/// use oxi_tui::input::kitty::matches_legacy_key;
///
/// assert!(matches_legacy_key(b"\x1b[A", "up"));
/// assert!(matches_legacy_key(b"\r", "enter"));
/// assert!(matches_legacy_key(b"\x1b[15~", "f5"));
/// assert!(!matches_legacy_key(b"\x1b[A", "down"));
/// ```
#[must_use]
pub fn matches_legacy_key(data: &[u8], expected: &str) -> bool {
    if expected.contains('+') {
        // Legacy modifier sequences are not currently handled — see doc comment.
        return false;
    }

    let expected_key = match normalize_key_name(expected) {
        Some(k) => k,
        None => return false,
    };

    match expected_key {
        // Single-byte non-prefixed keys.
        LegacyKey::Enter => data == b"\r" || data == b"\n",
        LegacyKey::Tab => data == b"\t",
        LegacyKey::Backspace => data == b"\x7f" || data == b"\x08",
        LegacyKey::Escape => data == b"\x1b",

        // CSI-prefixed keys: ESC [ <code>
        LegacyKey::Up => data == b"\x1b[A",
        LegacyKey::Down => data == b"\x1b[B",
        LegacyKey::Right => data == b"\x1b[C",
        LegacyKey::Left => data == b"\x1b[D",
        LegacyKey::Home => data == b"\x1b[H" || data == b"\x1b[1~" || data == b"\x1bOH",
        LegacyKey::End => data == b"\x1b[F" || data == b"\x1b[4~" || data == b"\x1bOF",
        LegacyKey::Insert => data == b"\x1b[2~",
        LegacyKey::Delete => data == b"\x1b[3~",
        LegacyKey::PageUp => data == b"\x1b[5~",
        LegacyKey::PageDown => data == b"\x1b[6~",

        // F1-F4 have two common forms each; F5-F12 only one.
        LegacyKey::F(n) => data == legacy_f_sequence(n).as_slice(),
    }
}

/// Recognized legacy (pre-Kitty) key kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyKey {
    /// Enter / Return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
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
    /// Insert.
    Insert,
    /// Delete.
    Delete,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// F1..=F12.
    F(u8),
}

fn normalize_key_name(name: &str) -> Option<LegacyKey> {
    match name.to_ascii_lowercase().as_str() {
        "enter" | "return" | "cr" => Some(LegacyKey::Enter),
        "tab" => Some(LegacyKey::Tab),
        "backspace" | "bs" => Some(LegacyKey::Backspace),
        "escape" | "esc" => Some(LegacyKey::Escape),
        "up" => Some(LegacyKey::Up),
        "down" => Some(LegacyKey::Down),
        "right" => Some(LegacyKey::Right),
        "left" => Some(LegacyKey::Left),
        "home" => Some(LegacyKey::Home),
        "end" => Some(LegacyKey::End),
        "insert" => Some(LegacyKey::Insert),
        "delete" | "del" => Some(LegacyKey::Delete),
        "pageup" | "page_up" | "pgup" => Some(LegacyKey::PageUp),
        "pagedown" | "page_down" | "pgdn" => Some(LegacyKey::PageDown),
        "f1" => Some(LegacyKey::F(1)),
        "f2" => Some(LegacyKey::F(2)),
        "f3" => Some(LegacyKey::F(3)),
        "f4" => Some(LegacyKey::F(4)),
        "f5" => Some(LegacyKey::F(5)),
        "f6" => Some(LegacyKey::F(6)),
        "f7" => Some(LegacyKey::F(7)),
        "f8" => Some(LegacyKey::F(8)),
        "f9" => Some(LegacyKey::F(9)),
        "f10" => Some(LegacyKey::F(10)),
        "f11" => Some(LegacyKey::F(11)),
        "f12" => Some(LegacyKey::F(12)),
        _ => None,
    }
}

/// Build the legacy xterm sequence for a given F-key (1..=12).
///
/// xterm sends F1-F4 as `\x1bO{P,Q,R,S}` (the "ss3" form) and F5-F12 as
/// `\x1b[N~` where `N` starts at 15 for F5 and skips 16 (used as a pause key
/// by some terminals).
fn legacy_f_sequence(n: u8) -> Vec<u8> {
    match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        // Out-of-range F-numbers are not a thing on xterm; return empty so
        // the match arm in matches_legacy_key yields `data == b""` (false).
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_char() {
        // ESC [ 97 ; 1 u = 'a' with no modifiers
        let k = parse_kitty_key(b"\x1b[97;1u").unwrap();
        assert_eq!(k.key, KeyType::Char('a'));
        assert_eq!(k.modifiers, KeyModifiers::NONE);
        assert_eq!(k.kind, KeyEventKind::Press);
    }

    #[test]
    fn parse_ctrl_modifier() {
        // ESC [ 97 ; 5 u = Ctrl+A (modifiers = 5 → bits = 4 = Ctrl)
        let k = parse_kitty_key(b"\x1b[97;5u").unwrap();
        assert_eq!(k.key, KeyType::Char('a'));
        assert_eq!(k.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_shift_ctrl_modifier() {
        // ESC [ 120 ; 6 u = Ctrl+Shift+x (modifiers = 6 → bits = 5)
        let k = parse_kitty_key(b"\x1b[120;6u").unwrap();
        assert_eq!(k.key, KeyType::Char('x'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn parse_alt_super_modifier() {
        // ESC [ 49 ; 11 u = Alt+Super+1 (modifiers = 11 → bits = 10)
        let k = parse_kitty_key(b"\x1b[49;11u").unwrap();
        assert_eq!(k.key, KeyType::Char('1'));
        assert!(k.modifiers.contains(KeyModifiers::ALT));
        assert!(k.modifiers.contains(KeyModifiers::SUPER));
    }

    #[test]
    fn parse_arrow_up() {
        // ESC [ 57344 ; 1 u = Up arrow
        let k = parse_kitty_key(b"\x1b[57344;1u").unwrap();
        assert_eq!(k.key, KeyType::ArrowUp);
        assert_eq!(k.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_function_key() {
        // ESC [ 57362 ; 1 u = F5 (codepoint 57362)
        let k = parse_kitty_key(b"\x1b[57362;1u").unwrap();
        assert_eq!(k.key, KeyType::F(5));
    }

    #[test]
    fn parse_release_event() {
        // ESC [ 97 ; 1 : 3 u = release of 'a'
        let k = parse_kitty_key(b"\x1b[97;1:3u").unwrap();
        assert_eq!(k.key, KeyType::Char('a'));
        assert_eq!(k.kind, KeyEventKind::Release);
    }

    #[test]
    fn parse_repeat_event() {
        // ESC [ 57344 ; 1 : 2 u = repeat Up
        let k = parse_kitty_key(b"\x1b[57344;1:2u").unwrap();
        assert_eq!(k.key, KeyType::ArrowUp);
        assert_eq!(k.kind, KeyEventKind::Repeat);
    }

    #[test]
    fn parse_strips_caps_lock_bit() {
        // ESC [ 97 ; 65 u = modifiers wire = 65 → bits = 64 = Caps Lock.
        // Lock bits are not real modifiers and must be dropped.
        let k = parse_kitty_key(b"\x1b[97;65u").unwrap();
        assert_eq!(k.key, KeyType::Char('a'));
        assert_eq!(k.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_too_short() {
        assert!(parse_kitty_key(b"").is_none());
        assert!(parse_kitty_key(b"\x1b[").is_none());
        assert!(parse_kitty_key(b"\x1b[97u").is_none()); // missing modifier
    }

    #[test]
    fn parse_wrong_prefix() {
        // Not CSI.
        assert!(parse_kitty_key(b"hello").is_none());
        assert!(parse_kitty_key(b"\x1b97;1u").is_none());
    }

    #[test]
    fn parse_invalid_codepoint() {
        // Codepoint that is not a valid Unicode scalar (above U+10FFFF).
        assert!(parse_kitty_key(b"\x1b[0x110000;1u").is_none());
    }

    #[test]
    fn parse_alternate_terminator() {
        // ESC [ 97 ; 1 ~ is the legacy alias form for 'a'.
        let k = parse_kitty_key(b"\x1b[97;1~").unwrap();
        assert_eq!(k.key, KeyType::Char('a'));
    }

    #[test]
    fn parse_arrow_shortcut_terminator() {
        // ESC [ A is the xterm arrow shortcut for Up.
        let k = parse_kitty_key(b"\x1b[A").unwrap();
        // Modifiers default to NONE since field is missing.
        assert_eq!(k.key, KeyType::ArrowUp);
    }

    #[test]
    fn matches_legacy_arrow_up() {
        assert!(matches_legacy_key(b"\x1b[A", "up"));
        assert!(!matches_legacy_key(b"\x1b[A", "down"));
    }

    #[test]
    fn matches_legacy_enter() {
        assert!(matches_legacy_key(b"\r", "enter"));
        assert!(matches_legacy_key(b"\n", "enter"));
    }

    #[test]
    fn matches_legacy_function_key() {
        assert!(matches_legacy_key(b"\x1b[15~", "f5"));
        assert!(matches_legacy_key(b"\x1bOP", "f1"));
        assert!(!matches_legacy_key(b"\x1b[15~", "f6"));
    }

    #[test]
    fn matches_legacy_unknown_name() {
        // Unknown keys never panic — they just return false.
        assert!(!matches_legacy_key(b"\x1b[A", "foo"));
    }

    #[test]
    fn modifier_display() {
        let m = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(m.to_string(), "Ctrl+Shift");

        let m = KeyModifiers::NONE;
        assert_eq!(m.to_string(), "");
    }

    #[test]
    fn keytype_display() {
        assert_eq!(KeyType::ArrowUp.to_string(), "Up");
        assert_eq!(KeyType::F(12).to_string(), "F12");
        assert_eq!(KeyType::Char('q').to_string(), "q");
        assert_eq!(KeyType::Space.to_string(), "Space");
    }
}
