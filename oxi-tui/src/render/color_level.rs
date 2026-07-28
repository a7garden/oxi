//! Terminal color capability detection and color downgrade utilities.
//!
//! Detects the terminal's supported color level (None/Basic/Ansi256/TrueColor)
//! and provides conversions for downgrading RGB colors when the terminal
//! cannot render 24-bit color.
//!
//! ## Design
//!
//! `detect_color_level()` is `LazyLock`-cached — the terminal capability is
//! determined once on first call and reused. This avoids re-parsing env vars
//! on every cell render.
//!
//! `adapt_color(color, level)` is a free function (not a method on ratatui's
//! `Color` enum, since `cell::Color` is just a re-export of `ratatui::style::Color`
//! and we cannot add inherent methods to external types).

use std::sync::LazyLock;

use crate::cell::Color;

/// The level of color support detected for the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ColorLevel {
    /// No color support (monochrome terminals, NO_COLOR env)
    None,
    /// Basic 16-color ANSI support (colors 0-15)
    Basic,
    /// 256-color support (colors 0-255)
    Ansi256,
    /// 24-bit truecolor RGB support (16 million colors)
    #[default]
    TrueColor,
}

impl ColorLevel {
    /// Returns true if at least basic color is supported.
    pub fn has_color(self) -> bool {
        self >= Self::Basic
    }

    /// Returns true if 256-color mode is supported.
    pub fn has_256(self) -> bool {
        self >= Self::Ansi256
    }

    /// Returns true if 24-bit truecolor is supported.
    pub fn has_truecolor(self) -> bool {
        self >= Self::TrueColor
    }
}

static COLOR_LEVEL: LazyLock<ColorLevel> = LazyLock::new(detect_color_level_inner);

/// Detect the terminal's color support level. Cached after first call.
///
/// Respects `NO_COLOR` env (<https://no-color.org/>) — returns `ColorLevel::None`.
/// Checks `COLORTERM`, `TERM`, and terminal-specific env vars via the
/// `supports-color` crate. Recovers truecolor when tmux/SSH/mosh strip
/// `COLORTERM` by recognizing known truecolor terminals (iTerm2, WezTerm,
/// Ghostty, Kitty, etc.) via their env identifiers.
pub fn detect_color_level() -> ColorLevel {
    *COLOR_LEVEL
}

fn detect_color_level_inner() -> ColorLevel {
    // Explicit opt-out via NO_COLOR takes priority per the spec.
    if std::env::var_os("NO_COLOR").is_some() {
        return ColorLevel::None;
    }

    let level = match supports_color::on(supports_color::Stream::Stdout) {
        // Not a TTY (tests, piped) — default to TrueColor.
        // oxi-tui is a widget library; the actual TUI runtime decides.
        None => ColorLevel::TrueColor,
        Some(level) => {
            if level.has_16m {
                ColorLevel::TrueColor
            } else if level.has_256 {
                ColorLevel::Ansi256
            } else if level.has_basic {
                ColorLevel::Basic
            } else {
                ColorLevel::None
            }
        }
    };

    // The `supports-color` crate relies on COLORTERM=truecolor, but
    // tmux/SSH/mosh often strip that variable. When the crate reports
    // only 256-color support, upgrade to TrueColor if we can identify
    // a known truecolor-capable terminal via its env vars.
    if level < ColorLevel::TrueColor && terminal_supports_truecolor() {
        return ColorLevel::TrueColor;
    }

    level
}

/// Recognize known truecolor-capable terminals by their env identifiers.
///
/// This is a fallback for when `COLORTERM` is stripped by tmux/SSH/mosh.
/// We don't claim TrueColor for unknown terminals — the user can override
/// via `COLORTERM=truecolor` if their terminal is unrecognized.
fn terminal_supports_truecolor() -> bool {
    const TRUECOLOR_TERMINALS: &[&str] = &["WezTerm", "ghostty", "kitty", "rio", "tabby", "vscode"];

    // ITERM_SESSION_ID is set by iTerm2 (and only iTerm2).
    if std::env::var_os("ITERM_SESSION_ID").is_some() {
        return true;
    }
    // Ghostty sets GHOSTTY_RESOURCES_DIR.
    if std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some() {
        return true;
    }
    // TERM_PROGRAM is set by many modern terminals.
    if let Some(term_program) = std::env::var_os("TERM_PROGRAM")
        && let Some(s) = term_program.to_str()
        && TRUECOLOR_TERMINALS.contains(&s)
    {
        return true;
    }
    false
}

/// Map an RGB color to the nearest xterm 256-color cube entry.
///
/// Uses the standard xterm 216-cube + 24 grayscale ramp. Returns the
/// Ansi256 index (0-255).
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // Logic mirrors anstyle's algorithm for deterministic output.
    if r == g && g == b {
        // Grayscale: 24-step ramp from index 232 (blackest) to 255 (whitest).
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        // Map r in [8, 248] to grayscale index 0..23, then offset to 232.
        return ((r as u16 - 8) * 24 / 240) as u8 + 232;
    }
    // 6x6x6 cube starting at index 16.
    let cube = |c: u8| -> u8 {
        if c < 48 {
            0
        } else if c < 115 {
            1
        } else {
            (c as u16 * 5 / 255) as u8
        }
    };
    16 + 36 * cube(r) + 6 * cube(g) + cube(b)
}

/// Downgrade a 256-color index to the nearest 16-color ANSI basic color.
pub fn ansi256_to_basic(idx: u8) -> u8 {
    match idx {
        0..=7 => idx,    // Already basic
        8 => 0,          // Bright black → black
        9..=15 => idx,   // Already bright basic
        16 => 0,         // Black
        17..=21 => 4,    // Blue range
        22..=27 => 2,    // Green range
        28..=51 => 2,    // Green-cyan
        52..=87 => 3,    // Yellow range
        88..=123 => 1,   // Red range
        124..=159 => 1,  // Magenta range
        160..=195 => 3,  // Yellow-cyan
        196..=231 => 9,  // Bright red range (high intensity)
        232..=237 => 0,  // Dark grays → black
        238..=243 => 8,  // Mid grays → bright black
        244..=249 => 7,  // Light-mid grays → white
        250..=255 => 15, // Light grays → bright white
    }
}

/// Downgrade a ratatui `Color` to fit the terminal's color level.
///
/// At `TrueColor`: passthrough (no downgrade).
/// At `Ansi256`: `Rgb(r,g,b)` → `Indexed(rgb_to_ansi256(...))`; `Indexed` passthrough.
/// At `Basic`: further downgrades `Indexed` to the nearest of 16 basic ANSI colors.
/// At `None`: any color becomes `Reset` (monochrome).
///
/// Named colors (`Color::Red`, `Color::Blue`, etc.) stay as-is at Basic+,
/// since ratatui already maps them to the basic 16. At `None`, they become `Reset`.
pub fn adapt_color(color: Color, level: ColorLevel) -> Color {
    match (color, level) {
        (_, ColorLevel::None) => Color::Reset,
        (Color::Reset, _) => Color::Reset,
        (c, ColorLevel::TrueColor) => c,
        (Color::Rgb(r, g, b), ColorLevel::Ansi256) => Color::Indexed(rgb_to_ansi256(r, g, b)),
        (Color::Rgb(r, g, b), ColorLevel::Basic) => {
            Color::Indexed(ansi256_to_basic(rgb_to_ansi256(r, g, b)))
        }
        (c @ Color::Indexed(_), ColorLevel::Ansi256) => c,
        (Color::Indexed(idx), ColorLevel::Basic) => {
            if idx < 16 {
                Color::Indexed(idx) // Already basic
            } else {
                Color::Indexed(ansi256_to_basic(idx))
            }
        }
        // Named colors (Black, Red, Green, Yellow, Blue, Magenta, Cyan, Gray,
        // DarkGray, LightRed, LightGreen, LightYellow, LightBlue, LightMagenta,
        // LightCyan, White) are already basic 16 — passthrough.
        (c, ColorLevel::Basic | ColorLevel::Ansi256) => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_level_ordering() {
        assert!(ColorLevel::TrueColor > ColorLevel::Ansi256);
        assert!(ColorLevel::Ansi256 > ColorLevel::Basic);
        assert!(ColorLevel::Basic > ColorLevel::None);
    }

    #[test]
    fn test_color_level_predicates() {
        assert!(ColorLevel::TrueColor.has_truecolor());
        assert!(ColorLevel::TrueColor.has_256());
        assert!(ColorLevel::TrueColor.has_color());

        assert!(!ColorLevel::Ansi256.has_truecolor());
        assert!(ColorLevel::Ansi256.has_256());
        assert!(ColorLevel::Ansi256.has_color());

        assert!(!ColorLevel::Basic.has_truecolor());
        assert!(!ColorLevel::Basic.has_256());
        assert!(ColorLevel::Basic.has_color());

        assert!(!ColorLevel::None.has_color());
    }

    #[test]
    fn test_detect_caches_after_first_call() {
        // LazyLock guarantees a single init. Two calls return the same value.
        let first = detect_color_level();
        let second = detect_color_level();
        assert_eq!(first, second);
    }

    #[test]
    fn test_rgb_to_ansi256_pure_red() {
        // Pure red (255, 0, 0) maps to cube corner 196.
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
    }

    #[test]
    fn test_rgb_to_ansi256_pure_green() {
        // Pure green (0, 255, 0) maps to cube corner 46.
        assert_eq!(rgb_to_ansi256(0, 255, 0), 46);
    }

    #[test]
    fn test_rgb_to_ansi256_pure_blue() {
        // Pure blue (0, 0, 255) maps to cube corner 21.
        assert_eq!(rgb_to_ansi256(0, 0, 255), 21);
    }

    #[test]
    fn test_rgb_to_ansi256_black() {
        // Black is the bottom of grayscale ramp → 16.
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
    }

    #[test]
    fn test_rgb_to_ansi256_white() {
        // White (255,255,255) is the top of grayscale ramp → 231.
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
    }

    #[test]
    fn test_rgb_to_ansi256_midgray() {
        // (128, 128, 128) is grayscale → 244 (mid of ramp).
        let idx = rgb_to_ansi256(128, 128, 128);
        assert!((232..=255).contains(&idx));
    }

    #[test]
    fn test_ansi256_to_basic_corners() {
        // Index 0 (black) stays 0.
        assert_eq!(ansi256_to_basic(0), 0);
        // Index 196 (bright red) → 9 (bright red basic).
        assert_eq!(ansi256_to_basic(196), 9);
        // Index 46 (bright green) → falls in 28..=51 range → 2 (green).
        assert_eq!(ansi256_to_basic(46), 2);
        // Index 21 (blue) → 4.
        assert_eq!(ansi256_to_basic(21), 4);
        // Index 255 (bright white) → 15.
        assert_eq!(ansi256_to_basic(255), 15);
    }

    #[test]
    fn test_terminal_supports_truecolor_iterm() {
        // SAFETY: tests are single-threaded when run with --test-threads=1.
        // The env var is restored after the assertion.
        unsafe {
            std::env::set_var("ITERM_SESSION_ID", "test:value");
        }
        assert!(terminal_supports_truecolor());
        unsafe {
            std::env::remove_var("ITERM_SESSION_ID");
        }
    }

    #[test]
    fn test_terminal_does_not_support_truecolor_unknown() {
        // SAFETY: tests are single-threaded when run with --test-threads=1.
        unsafe {
            std::env::remove_var("ITERM_SESSION_ID");
            std::env::remove_var("GHOSTTY_RESOURCES_DIR");
            std::env::remove_var("TERM_PROGRAM");
        }
        assert!(!terminal_supports_truecolor());
    }

    #[test]
    fn test_adapt_color_truecolor_passes_through_rgb() {
        // TrueColor terminal: RGB stays as RGB.
        let c = Color::Rgb(123, 45, 67);
        assert_eq!(adapt_color(c, ColorLevel::TrueColor), c);
    }

    #[test]
    fn test_adapt_color_ansi256_downgrades_rgb() {
        // Ansi256 terminal: RGB downgrades to Indexed.
        let c = Color::Rgb(255, 0, 0);
        let adapted = adapt_color(c, ColorLevel::Ansi256);
        assert_eq!(adapted, Color::Indexed(196));
    }

    #[test]
    fn test_adapt_color_basic_downgrades_rgb_to_basic() {
        // Basic terminal: RGB downgrades to basic named color (still Indexed).
        let c = Color::Rgb(255, 0, 0);
        let adapted = adapt_color(c, ColorLevel::Basic);
        // Should be one of the basic 16 colors (Indexed < 16).
        match adapted {
            Color::Indexed(idx) => assert!(idx < 16, "expected basic color, got {idx}"),
            other => panic!("expected Indexed, got {other:?}"),
        }
    }

    #[test]
    fn test_adapt_color_none_returns_reset() {
        // Monochrome terminal: any color becomes Reset.
        let c = Color::Rgb(123, 45, 67);
        assert_eq!(adapt_color(c, ColorLevel::None), Color::Reset);
    }

    #[test]
    fn test_adapt_color_named_stays_named() {
        // Named colors (Color::Red etc) stay as-is at Basic and above.
        let c = Color::Red;
        assert_eq!(adapt_color(c, ColorLevel::Basic), c);
        assert_eq!(adapt_color(c, ColorLevel::Ansi256), c);
        assert_eq!(adapt_color(c, ColorLevel::TrueColor), c);
    }

    #[test]
    fn test_adapt_color_none_named_to_reset() {
        // Monochrome: even named colors become Reset.
        assert_eq!(adapt_color(Color::Red, ColorLevel::None), Color::Reset);
    }

    #[test]
    fn test_adapt_color_reset_always_reset() {
        // Reset stays Reset at any level.
        assert_eq!(adapt_color(Color::Reset, ColorLevel::None), Color::Reset);
        assert_eq!(
            adapt_color(Color::Reset, ColorLevel::TrueColor),
            Color::Reset
        );
    }

    #[test]
    fn test_adapt_color_ansi256_indexed_stays_at_ansi256() {
        // Indexed(196) at Ansi256 level stays as Indexed(196).
        let c = Color::Indexed(196);
        assert_eq!(adapt_color(c, ColorLevel::Ansi256), c);
    }

    #[test]
    fn test_adapt_color_ansi256_indexed_downgrades_at_basic() {
        // Indexed(196) at Basic level → Indexed(9) (bright red basic).
        let c = Color::Indexed(196);
        assert_eq!(adapt_color(c, ColorLevel::Basic), Color::Indexed(9));
    }
}
