//! Terminal capability detection + theme color-level adaptation.
//!
//! ★ CRITICAL: detection and consumption live in the SAME MODULE.
//! This is the structural fix for the dead-code class exemplified by
//! legacy `render/color_level.rs` (394 LOC, re-exported, never called).
//! See spec §1.5, §7.2.
//!
//! Per-kind detection logic lives in `detect.rs` (split to keep this
//! module under the 500-LOC cap).

mod detect;

use ratatui::style::Color;

use crate::theme::{ColorScheme, Theme};

/// Supported image protocols for inline terminal images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// Kitty image protocol (supported by Kitty, Ghostty, `WezTerm`).
    Kitty,
    /// iTerm2 inline image protocol (supported by iTerm2, `WezTerm`).
    ITerm2,
}

/// Detected terminal color depth.
///
/// Ordered: `None < Basic < Ansi256 < TrueColor`. Comparison answers
/// "is feature X available" via the `has_*` helpers below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ColorLevel {
    /// No color — terminal is monochrome / `NO_COLOR` was set.
    #[default]
    None,
    /// 8-color basic ANSI.
    Basic,
    /// xterm 256-color palette.
    Ansi256,
    /// 24-bit true color (`Rgb(r,g,b)`).
    TrueColor,
}

impl ColorLevel {
    /// Any color (basic or better).
    #[must_use]
    pub fn has_color(self) -> bool {
        self >= Self::Basic
    }

    /// 256-color or better.
    #[must_use]
    pub fn has_256(self) -> bool {
        self >= Self::Ansi256
    }

    /// `TrueColor`.
    #[must_use]
    pub fn has_truecolor(self) -> bool {
        self >= Self::TrueColor
    }
}
/// All detected terminal capabilities. Populated once at bootstrap.
#[allow(clippy::struct_excessive_bools)] // mirror per-kind capability flags
#[derive(Debug, Clone)]
pub struct TerminalCaps {
    /// Detected color depth. Independent from per-kind booleans below
    /// because a Kitty terminal under tmux may still degrade to Ansi256.
    pub color_level: ColorLevel,
    /// Supported image protocol, if any.
    pub image_protocol: Option<ImageProtocol>,
    /// Whether the terminal supports 24-bit true color.
    pub true_color: bool,
    /// Whether the terminal supports OSC 8 hyperlinks.
    pub hyperlinks: bool,
    /// Whether the Kitty keyboard protocol is active.
    pub kitty_protocol: bool,
    /// Supports Sixel inline images. Defaults to **false**.
    pub sixel: bool,
    /// Supports synchronized output (CSI 2026 BSU/ESU). Defaults to **true**
    /// — harmless on unsupported terminals. Disable with `OXI_NO_SYNC_OUTPUT=1`.
    pub synchronized_output: bool,
    /// Supports Kitty's DECCARA rectangular background-fill extension.
    /// Defaults to **false** — emitting DECCARA to an unsupported terminal
    /// corrupts the display. Enabled for Kitty/Ghostty.
    pub deccara: bool,
    /// Cell size in pixels (width, height), if detectable.
    pub cell_size: Option<(u16, u16)>,
    /// Identified terminal family name, if recognized.
    pub terminal_name: Option<String>,
}

impl Default for TerminalCaps {
    fn default() -> Self {
        // Safe defaults: synchronized output on (harmless if ignored),
        // deccara/sixel off (harmful if unsupported).
        Self {
            color_level: ColorLevel::None,
            image_protocol: None,
            true_color: false,
            hyperlinks: false,
            kitty_protocol: false,
            sixel: false,
            synchronized_output: true,
            deccara: false,
            cell_size: None,
            terminal_name: None,
        }
    }
}

impl TerminalCaps {
    /// Detect terminal capabilities from environment variables.
    ///
    /// Reads `NO_COLOR`/`COLORTERM`/`TERM` for `color_level`, and
    /// `TERM`/`TERM_PROGRAM`/`KITTY_WINDOW_ID`/`GHOSTTY_RESOURCES_DIR`/
    /// `WEZTERM_PANE`/`ITERM_SESSION_ID`/`TMUX`/`OXI_NO_SYNC_OUTPUT` for
    /// per-kind booleans.
    #[must_use]
    pub fn detect() -> Self {
        detect::detect_with_env()
    }

    /// Downgrade all colors in the theme to match the terminal's color
    /// level. Called once at bootstrap after `Theme::dark()` etc.
    ///
    /// Re-derives [`crate::theme::ThemeStyles`] after mutating colors so
    /// downstream consumers never observe stale styles.
    pub fn adapt_theme(&self, theme: &mut Theme) {
        if self.color_level >= ColorLevel::TrueColor {
            return; // no downgrade needed
        }
        adapt_color_scheme(&mut theme.colors, self.color_level);
        theme.styles = crate::theme::ThemeStyles::from_colors(&theme.colors);
    }

    /// Check if images are supported.
    #[must_use]
    pub fn supports_images(&self) -> bool {
        self.image_protocol.is_some()
    }
}

/// Adapt a single [`Color`] for the terminal's [`ColorLevel`].
///
/// - `None` ⇒ everything becomes [`Color::Reset`] (no color).
/// - `TrueColor` ⇒ unchanged.
/// - `Ansi256` ⇒ `Rgb` mapped to nearest xterm-256 index.
/// - `Basic` ⇒ further collapsed to ANSI 16 via heuristic.
#[allow(clippy::match_same_arms)] // TrueColor catch-all + basic-16 passthrough share body but disjoint levels
#[must_use]
pub fn adapt_color(color: Color, level: ColorLevel) -> Color {
    match (color, level) {
        // None ⇒ strip everything to Reset.
        (_, ColorLevel::None) => Color::Reset,
        // Reset stays Reset at every level.
        (Color::Reset, _) => Color::Reset,
        // TrueColor ⇒ no downgrade needed (catch-all).
        (c, ColorLevel::TrueColor) => c,
        // Rgb ⇒ quantize to the nearest indexed palette entry.
        (Color::Rgb(r, g, b), ColorLevel::Ansi256) => Color::Indexed(rgb_to_ansi256(r, g, b)),
        (Color::Rgb(r, g, b), ColorLevel::Basic) => {
            Color::Indexed(ansi256_to_basic(rgb_to_ansi256(r, g, b)))
        }
        // DarkGray has no Basic counterpart ⇒ round up to Gray at Basic/Ansi256.
        (Color::DarkGray, ColorLevel::Basic | ColorLevel::Ansi256) => Color::Gray,
        // Light* variants aren't part of ANSI 16 ⇒ collapse to non-light at Basic.
        (
            c @ (Color::LightRed
            | Color::LightGreen
            | Color::LightYellow
            | Color::LightBlue
            | Color::LightMagenta
            | Color::LightCyan),
            ColorLevel::Basic,
        ) => darken_light(c),
        // Indexed at Basic ⇒ map through the 16-color heuristic.
        (Color::Indexed(idx), ColorLevel::Basic) => Color::Indexed(ansi256_to_basic(idx)),
        // Remaining basic 16 colors + Indexed at Ansi256 pass through unchanged.
        // (Light* at Ansi256 also survive — they're valid xterm-256 palette entries.)
        (
            c @ (Color::Black
            | Color::Red
            | Color::Green
            | Color::Yellow
            | Color::Blue
            | Color::Magenta
            | Color::Cyan
            | Color::White
            | Color::Gray
            | Color::LightRed
            | Color::LightGreen
            | Color::LightYellow
            | Color::LightBlue
            | Color::LightMagenta
            | Color::LightCyan
            | Color::Indexed(_)),
            ColorLevel::Basic | ColorLevel::Ansi256,
        ) => c,
    }
}

fn darken_light(c: Color) -> Color {
    match c {
        Color::LightRed => Color::Red,
        Color::LightGreen => Color::Green,
        Color::LightYellow => Color::Yellow,
        Color::LightBlue => Color::Blue,
        Color::LightMagenta => Color::Magenta,
        Color::LightCyan => Color::Cyan,
        _ => c,
    }
}

fn adapt_color_scheme(scheme: &mut ColorScheme, level: ColorLevel) {
    macro_rules! adapt_field {
        ($f:ident) => {
            scheme.$f = adapt_color(scheme.$f, level);
        };
    }
    adapt_field!(background);
    adapt_field!(foreground);
    adapt_field!(primary);
    adapt_field!(accent);
    adapt_field!(muted);
    adapt_field!(border);
    adapt_field!(user);
    adapt_field!(user_bg);
    adapt_field!(response);
    adapt_field!(response_bg);
    adapt_field!(thinking);
    adapt_field!(thinking_bg);
    adapt_field!(tool);
    adapt_field!(tool_bg);
    adapt_field!(success);
    adapt_field!(warning);
    adapt_field!(error);
    adapt_field!(info);
    adapt_field!(diff_add);
    adapt_field!(diff_remove);
    adapt_field!(diff_hunk);
    adapt_field!(surface_bg);
    adapt_field!(panel_bg);
    adapt_field!(code_bg);
    adapt_field!(selection_bg);
    adapt_field!(diff_add_bg);
    adapt_field!(diff_remove_bg);
    adapt_field!(diff_hunk_bg);
}

/// RGB → ANSI 256 (xterm 216-cube + 24 grayscale).
fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return u8::try_from(u16::from(r - 8) * 24 / 237 + 232).unwrap_or(232);
    }
    let ri = u16::from(r) * 5 / 255;
    let gi = u16::from(g) * 5 / 255;
    let bi = u16::from(b) * 5 / 255;
    u8::try_from(16 + 36 * ri + 6 * gi + bi).unwrap_or(16)
}

/// ANSI 256 → basic 16 (heuristic).
fn ansi256_to_basic(idx: u8) -> u8 {
    match idx {
        0..=15 => idx,            // already basic (8 normal + 8 bright)
        16 => 0,                  // #000000 → black
        17..=51 => 4,             // blue cube
        52..=87 | 124..=159 => 1, // red cube / red repeat
        88..=123 => 5,            // magenta cube
        160..=195 => 3,           // yellow cube
        196..=231 => {
            if idx < 214 {
                1
            } else {
                3
            }
        } // red-yellow
        232..=255 => 7,           // grayscale → white (lossy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapt_color_resets_at_none_level() {
        assert_eq!(adapt_color(Color::Red, ColorLevel::None), Color::Reset);
        assert_eq!(
            adapt_color(Color::Rgb(1, 2, 3), ColorLevel::None),
            Color::Reset
        );
        assert_eq!(
            adapt_color(Color::Indexed(42), ColorLevel::None),
            Color::Reset
        );
    }

    #[test]
    fn adapt_color_keeps_truecolor_unchanged() {
        let c = Color::Rgb(123, 45, 67);
        assert_eq!(adapt_color(c, ColorLevel::TrueColor), c);
        assert_eq!(adapt_color(Color::Red, ColorLevel::TrueColor), Color::Red);
    }

    #[test]
    fn adapt_color_rgb_to_ansi256_red() {
        assert_eq!(
            adapt_color(Color::Rgb(255, 0, 0), ColorLevel::Ansi256),
            Color::Indexed(196)
        );
    }

    #[test]
    fn adapt_color_keeps_basic_colors() {
        assert_eq!(adapt_color(Color::Red, ColorLevel::Basic), Color::Red);
        assert_eq!(adapt_color(Color::Cyan, ColorLevel::Basic), Color::Cyan);
        // DarkGray at Basic collapses to Gray (its bright counterpart).
        assert_eq!(adapt_color(Color::DarkGray, ColorLevel::Basic), Color::Gray);
    }

    #[test]
    fn adapt_color_collapses_light_to_basic_at_basic_level() {
        // LightRed at Basic → Red.
        assert_eq!(adapt_color(Color::LightRed, ColorLevel::Basic), Color::Red);
        assert_eq!(
            adapt_color(Color::LightCyan, ColorLevel::Basic),
            Color::Cyan
        );
        // At Ansi256, light variants survive (still Indexed mappable).
        assert_eq!(
            adapt_color(Color::LightRed, ColorLevel::Ansi256),
            Color::LightRed
        );
    }

    #[test]
    fn adapt_color_rgb_to_basic_via_index() {
        // Pure red RGB → Indexed(196) → Basic red (idx 1).
        assert_eq!(
            adapt_color(Color::Rgb(255, 0, 0), ColorLevel::Basic),
            Color::Indexed(1)
        );
    }

    #[test]
    fn no_color_env_returns_none_level() {
        // Pure-fn test: detects NO_COLOR via the parameterized
        // `detect::detect_color_level`. The crate forbids `unsafe env::set_var`,
        // so the real-env round-trip is exercised in `detect.rs` tests.
        let level = detect::detect_color_level(Some("1"), "truecolor", "xterm-256color", true);
        assert_eq!(level, ColorLevel::None);
    }

    #[test]
    fn color_level_has_helpers() {
        assert!(!ColorLevel::None.has_color());
        assert!(ColorLevel::Basic.has_color());
        assert!(!ColorLevel::Basic.has_256());
        assert!(ColorLevel::Ansi256.has_256());
        assert!(!ColorLevel::Ansi256.has_truecolor());
        assert!(ColorLevel::TrueColor.has_truecolor());
    }

    #[test]
    fn adapt_theme_skips_downgrade_at_truecolor() {
        let mut theme = Theme::dark();
        let original_bg = theme.colors.background;
        let caps = TerminalCaps {
            color_level: ColorLevel::TrueColor,
            ..TerminalCaps::default()
        };
        caps.adapt_theme(&mut theme);
        assert_eq!(theme.colors.background, original_bg);
    }

    #[test]
    fn adapt_theme_downgrades_rgb_to_ansi256() {
        let mut theme = Theme::dark();
        // Force one field to RGB so we can verify downgrade.
        theme.colors.accent = Color::Rgb(255, 0, 0);
        let caps = TerminalCaps {
            color_level: ColorLevel::Ansi256,
            ..TerminalCaps::default()
        };
        caps.adapt_theme(&mut theme);
        assert_eq!(theme.colors.accent, Color::Indexed(196));
        // Style must be re-derived from the new color.
        assert_eq!(theme.styles.accent.fg, Some(Color::Indexed(196)));
    }

    #[test]
    fn adapt_theme_at_none_resets_all_to_default_bg() {
        let mut theme = Theme::dark();
        let caps = TerminalCaps {
            color_level: ColorLevel::None,
            ..TerminalCaps::default()
        };
        caps.adapt_theme(&mut theme);
        // Every color must be Reset.
        assert_eq!(theme.colors.background, Color::Reset);
        assert_eq!(theme.colors.foreground, Color::Reset);
        assert_eq!(theme.colors.primary, Color::Reset);
        assert_eq!(theme.colors.accent, Color::Reset);
    }

    #[test]
    fn rgb_to_ansi256_pure_red_is_196() {
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
    }

    #[test]
    fn ansi256_to_basic_known_indices() {
        // 196 (red) → 1 (basic red).
        assert_eq!(ansi256_to_basic(196), 1);
        // 21 (basic blue in cube) → 4 (basic blue).
        assert_eq!(ansi256_to_basic(21), 4);
        // 0 (black) → 0.
        assert_eq!(ansi256_to_basic(0), 0);
        // 15 (bright white) → 15.
        assert_eq!(ansi256_to_basic(15), 15);
        // 240 (dark gray) → 7 (white, lossy).
        assert_eq!(ansi256_to_basic(240), 7);
    }
}
