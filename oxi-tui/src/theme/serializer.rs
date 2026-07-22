//! TOML theme loading and saving. Atomic writes (temp + rename).
//!
//! `ThemeFile` is the serde-friendly mirror of `ColorScheme`. The loaded
//! file is validated against the brightness hierarchy (AGENTS.md pitfall)
//! before being promoted to a runtime `Theme`.
//!
//! Field-level format (see `parse_color_str` / `color_to_str`):
//! - Hex: `#rrggbb` (lowercase)
//! - Named: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`,
//!   `white`, bright variants, `gray`/`grey`, `default`, `reset`
//! - Indexed: `i<N>` where N is 0–255

use std::path::Path;

use anyhow::{Context, Result};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::theme::palette::{ColorScheme, Theme, ThemeStyles};

/// Parsed `Theme` value, also its TOML representation.
#[derive(Debug, Serialize, Deserialize)]
pub struct ThemeFile {
    /// Human-readable theme name.
    pub name: String,
    /// 28-slot semantic palette, as strings.
    pub colors: ColorSchemeFile,
}

/// 28-slot semantic palette in serialized form.
///
/// Each field is either a hex string (`#rrggbb`), an indexed-color
/// reference (`i<N>`), or a named color (`red`, `default`, ...).
/// See [`parse_color_str`] for the full grammar.
#[derive(Debug, Serialize, Deserialize)]
pub struct ColorSchemeFile {
    // ── 21 "original" slots (14 fg + 7 bg) ──────────────────────────
    /// Default background color.
    pub background: String,
    /// Normal text foreground color.
    pub foreground: String,
    /// Primary accent color (UI elements, labels, user "You").
    pub primary: String,
    /// Accent highlight color.
    pub accent: String,
    /// Muted / dimmed text (e.g. placeholders, tool headers).
    pub muted: String,
    /// Border / separator color.
    pub border: String,
    /// User message foreground.
    pub user: String,
    /// User message background (subtle tint).
    pub user_bg: String,
    /// Assistant response text foreground.
    pub response: String,
    /// Assistant response text background (= background by default).
    pub response_bg: String,
    /// Thinking block foreground.
    pub thinking: String,
    /// Thinking block background (subtle accent tint).
    pub thinking_bg: String,
    /// Tool call text foreground.
    pub tool: String,
    /// Tool call text background (subtle tint).
    pub tool_bg: String,
    /// Success / confirmation color.
    pub success: String,
    /// Warning / caution color.
    pub warning: String,
    /// Error / danger color.
    pub error: String,
    /// Informational color (blue/accent).
    pub info: String,
    /// Diff added-line foreground.
    pub diff_add: String,
    /// Diff removed-line foreground.
    pub diff_remove: String,
    /// Diff hunk-header foreground.
    pub diff_hunk: String,

    // ── 7 Phase-1 background slots ──────────────────────────────────
    /// Footer / status bar background.
    pub surface_bg: String,
    /// Overlay popup background (most prominent surface).
    pub panel_bg: String,
    /// Inline code / code block background.
    pub code_bg: String,
    /// Selection / highlight background.
    pub selection_bg: String,
    /// Diff added-line background (reuses `tool_success_bg` tint in legacy).
    pub diff_add_bg: String,
    /// Diff removed-line background (reuses `tool_error_bg` tint in legacy).
    pub diff_remove_bg: String,
    /// Diff hunk-header background (subtle muted tint).
    pub diff_hunk_bg: String,
}

/// Load a theme from a TOML file at `path`.
///
/// Performs two validations:
/// 1. Strict color-string parsing — invalid values are rejected with
///    `anyhow::Error` (no silent fallback; unlike legacy which warns and
///    uses the dark default).
/// 2. Brightness-hierarchy check — emits `tracing::warn!` per violation
///    but does not abort (AGENTS.md pitfall: must hold, not fatal).
pub fn load_theme(path: &Path) -> Result<Theme> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read theme {}", path.display()))?;
    let file: ThemeFile =
        toml::from_str(&content).with_context(|| format!("parse theme {}", path.display()))?;
    let colors = file.colors.into_scheme()?;
    validate_brightness(&colors);
    let styles = ThemeStyles::from_colors(&colors);
    let name: std::borrow::Cow<'static, str> = if file.name.is_empty() {
        std::borrow::Cow::Borrowed("custom")
    } else {
        std::borrow::Cow::Owned(file.name)
    };
    Ok(Theme {
        colors,
        styles,
        name,
    })
}

/// Serialize `theme` to TOML and write atomically (temp + rename).
pub fn save_theme(theme: &Theme, path: &Path) -> Result<()> {
    let file = ThemeFile {
        name: theme.name.to_string(),
        colors: ColorSchemeFile::from_scheme(&theme.colors),
    };
    let content = toml::to_string_pretty(&file).context("serialize theme to TOML")?;
    // Atomic write: temp + rename. Writes always land in the same
    // directory so the rename is on the same filesystem.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content)
        .with_context(|| format!("write temp theme file {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename temp theme file to {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Color parsing
// ---------------------------------------------------------------------------

/// Parse a [`Color`] from one of:
/// - Hex: `#rrggbb` (case-insensitive)
/// - Named: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`,
///   `white`, `bright-<name>`, `gray`/`grey`, `default`, `reset`
/// - Indexed: `i<N>` where N is 0–255
fn parse_color_str(s: &str, field: &str) -> Result<Color> {
    let s = s.trim();
    // Hex (#rrggbb)
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex).with_context(|| format!("invalid hex color #{hex} for {field}"));
    }
    // Indexed (i<N>)
    if let Some(idx_str) = s.strip_prefix('i') {
        return match idx_str.parse::<u8>() {
            Ok(n) => Ok(Color::Indexed(n)),
            Err(_) => Err(anyhow::anyhow!(
                "invalid indexed color '{s}' for {field}: expected i<0..=255>"
            )),
        };
    }
    // Named (case-insensitive)
    match s.to_lowercase().as_str() {
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "white" => Ok(Color::White),
        "bright-black" | "brightblack" | "gray" | "grey" => Ok(Color::Indexed(8)),
        "bright-red" | "brightred" => Ok(Color::Indexed(9)),
        "bright-green" | "brightgreen" => Ok(Color::Indexed(10)),
        "bright-yellow" | "brightyellow" => Ok(Color::Indexed(11)),
        "bright-blue" | "brightblue" => Ok(Color::Indexed(12)),
        "bright-magenta" | "brightmagenta" => Ok(Color::Indexed(13)),
        "bright-cyan" | "brightcyan" => Ok(Color::Indexed(14)),
        "bright-white" | "brightwhite" => Ok(Color::Indexed(15)),
        "default" | "reset" => Ok(Color::Reset),
        _ => Err(anyhow::anyhow!(
            "unknown color name '{s}' for {field}: expected hex (#rrggbb), \
             named (red, blue, ..., default, reset), or indexed (i0..i255)"
        )),
    }
}

fn parse_hex(hex: &str) -> Option<Color> {
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        3 => {
            // #rgb short-form expands to #rrggbb.
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Color serialization
// ---------------------------------------------------------------------------

/// Serialize a [`Color`] to a string parseable by [`parse_color_str`].
///
/// - `Rgb` → `#rrggbb` (lowercase, always 6 digits, no short-form).
/// - `Indexed` → `i<N>`.
/// - `Reset` → `default`.
/// - Named variants → lowercase name (`black`, `red`, ...).
/// - Other named bright variants are emitted as their `bright-<name>`
///   alias so they round-trip back through the parser.
#[allow(clippy::match_same_arms)] // DarkGray→"bright-black" is intentional asymmetry: one input, one canonical output
fn color_to_str(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "black".to_string(),
        Color::Red => "red".to_string(),
        Color::Green => "green".to_string(),
        Color::Yellow => "yellow".to_string(),
        Color::Blue => "blue".to_string(),
        Color::Magenta => "magenta".to_string(),
        Color::Cyan => "cyan".to_string(),
        Color::White => "white".to_string(),
        Color::Gray => "bright-white".to_string(),
        Color::DarkGray => "bright-black".to_string(),
        Color::LightRed => "bright-red".to_string(),
        Color::LightGreen => "bright-green".to_string(),
        Color::LightYellow => "bright-yellow".to_string(),
        Color::LightBlue => "bright-blue".to_string(),
        Color::LightMagenta => "bright-magenta".to_string(),
        Color::LightCyan => "bright-cyan".to_string(),
        Color::Indexed(n) => format!("i{n}"),
        Color::Reset => "default".to_string(),
    }
}

// ---------------------------------------------------------------------------
// ColorSchemeFile <-> ColorScheme
// ---------------------------------------------------------------------------

impl ColorSchemeFile {
    /// Parse all 28 fields into a [`ColorScheme`]. Strict: any invalid
    /// value is rejected with the offending field name in the error chain.
    fn into_scheme(self) -> Result<ColorScheme> {
        Ok(ColorScheme {
            background: parse_color_str(&self.background, "background")?,
            foreground: parse_color_str(&self.foreground, "foreground")?,
            primary: parse_color_str(&self.primary, "primary")?,
            accent: parse_color_str(&self.accent, "accent")?,
            muted: parse_color_str(&self.muted, "muted")?,
            border: parse_color_str(&self.border, "border")?,
            user: parse_color_str(&self.user, "user")?,
            user_bg: parse_color_str(&self.user_bg, "user_bg")?,
            response: parse_color_str(&self.response, "response")?,
            response_bg: parse_color_str(&self.response_bg, "response_bg")?,
            thinking: parse_color_str(&self.thinking, "thinking")?,
            thinking_bg: parse_color_str(&self.thinking_bg, "thinking_bg")?,
            tool: parse_color_str(&self.tool, "tool")?,
            tool_bg: parse_color_str(&self.tool_bg, "tool_bg")?,
            success: parse_color_str(&self.success, "success")?,
            warning: parse_color_str(&self.warning, "warning")?,
            error: parse_color_str(&self.error, "error")?,
            info: parse_color_str(&self.info, "info")?,
            diff_add: parse_color_str(&self.diff_add, "diff_add")?,
            diff_remove: parse_color_str(&self.diff_remove, "diff_remove")?,
            diff_hunk: parse_color_str(&self.diff_hunk, "diff_hunk")?,
            surface_bg: parse_color_str(&self.surface_bg, "surface_bg")?,
            panel_bg: parse_color_str(&self.panel_bg, "panel_bg")?,
            code_bg: parse_color_str(&self.code_bg, "code_bg")?,
            selection_bg: parse_color_str(&self.selection_bg, "selection_bg")?,
            diff_add_bg: parse_color_str(&self.diff_add_bg, "diff_add_bg")?,
            diff_remove_bg: parse_color_str(&self.diff_remove_bg, "diff_remove_bg")?,
            diff_hunk_bg: parse_color_str(&self.diff_hunk_bg, "diff_hunk_bg")?,
        })
    }

    /// Serialize every slot of a [`ColorScheme`] to its string form.
    fn from_scheme(s: &ColorScheme) -> Self {
        Self {
            background: color_to_str(s.background),
            foreground: color_to_str(s.foreground),
            primary: color_to_str(s.primary),
            accent: color_to_str(s.accent),
            muted: color_to_str(s.muted),
            border: color_to_str(s.border),
            user: color_to_str(s.user),
            user_bg: color_to_str(s.user_bg),
            response: color_to_str(s.response),
            response_bg: color_to_str(s.response_bg),
            thinking: color_to_str(s.thinking),
            thinking_bg: color_to_str(s.thinking_bg),
            tool: color_to_str(s.tool),
            tool_bg: color_to_str(s.tool_bg),
            success: color_to_str(s.success),
            warning: color_to_str(s.warning),
            error: color_to_str(s.error),
            info: color_to_str(s.info),
            diff_add: color_to_str(s.diff_add),
            diff_remove: color_to_str(s.diff_remove),
            diff_hunk: color_to_str(s.diff_hunk),
            surface_bg: color_to_str(s.surface_bg),
            panel_bg: color_to_str(s.panel_bg),
            code_bg: color_to_str(s.code_bg),
            selection_bg: color_to_str(s.selection_bg),
            diff_add_bg: color_to_str(s.diff_add_bg),
            diff_remove_bg: color_to_str(s.diff_remove_bg),
            diff_hunk_bg: color_to_str(s.diff_hunk_bg),
        }
    }
}

// ---------------------------------------------------------------------------
// Brightness hierarchy validation (warn-only)
// ---------------------------------------------------------------------------

/// AGENTS.md pitfall:
/// `background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg`.
///
/// Approximate via W3C relative luminance. Emit a `tracing::warn!` per
/// violation; never abort. The hierarchy is a *visual* contract, not a
/// hard error — many themes intentionally blur it (e.g. Catppuccin's
/// equally-tinted surfaces), so we err on the side of letting the user
/// load their theme and just complain in the log.
fn validate_brightness(scheme: &ColorScheme) {
    const EPS: f64 = 1e-3;
    let bg = relative_luminance(scheme.background);
    let response_bg = relative_luminance(scheme.response_bg);
    let thinking_bg = relative_luminance(scheme.thinking_bg);
    let surface_bg = relative_luminance(scheme.surface_bg);
    let user_bg = relative_luminance(scheme.user_bg);
    let panel_bg = relative_luminance(scheme.panel_bg);

    if response_bg + EPS < bg {
        tracing::warn!(
            "theme brightness: background luminance {bg:.3} > response_bg {response_bg:.3} (background should be deepest)"
        );
    }
    if thinking_bg + EPS < response_bg {
        tracing::warn!(
            "theme brightness: response_bg {response_bg:.3} > thinking_bg {thinking_bg:.3} (response_bg should be ≤ thinking_bg)"
        );
    }
    if surface_bg + EPS < thinking_bg {
        tracing::warn!(
            "theme brightness: thinking_bg {thinking_bg:.3} > surface_bg {surface_bg:.3}"
        );
    }
    if user_bg + EPS < surface_bg {
        tracing::warn!("theme brightness: surface_bg {surface_bg:.3} > user_bg {user_bg:.3}");
    }
    if panel_bg + EPS < user_bg {
        tracing::warn!("theme brightness: user_bg {user_bg:.3} > panel_bg {panel_bg:.3}");
    }
}

/// W3C relative luminance for RGB-based colors. Non-RGB colors collapse
/// to mid-gray (`0.5`) so they don't fire spurious comparisons.
fn relative_luminance(color: Color) -> f64 {
    match color {
        Color::Rgb(r, g, b) => {
            let linear = |c: u8| {
                let x = f64::from(c) / 255.0;
                if x <= 0.04045 {
                    x / 12.92
                } else {
                    ((x + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
        }
        Color::Black => 0.0,
        Color::White => 1.0,
        Color::Indexed(n) => {
            let bright_offset = if n < 8 {
                f64::from(n) * 0.18
            } else if n < 16 {
                0.6 + (f64::from(n) - 8.0) * 0.05
            } else {
                // xterm 6×6×6 cube + grayscale ramp. Coarse approximation.
                0.4
            };
            bright_offset.clamp(0.0, 1.0)
        }
        // Reset / Red / Green / Yellow / Blue / Magenta / Cyan / Gray /
        // DarkGray / Light* don't appear in bg slots — collapse to mid-gray.
        _ => 0.5,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_nonexistent_returns_error() {
        let result = load_theme(Path::new("/nonexistent/theme.toml"));
        assert!(result.is_err(), "loading a missing file must error");
    }

    #[test]
    fn dark_theme_round_trips_through_toml() {
        let temp = std::env::temp_dir().join("oxi_tui_test_theme.toml");
        // Clean any stale file from a prior run to keep load deterministic.
        let _ = std::fs::remove_file(&temp);
        let original = Theme::dark();
        save_theme(&original, &temp).expect("save dark theme");
        let loaded = load_theme(&temp).expect("load dark theme");
        assert_eq!(
            loaded.colors.background, original.colors.background,
            "background must round-trip"
        );
        assert_eq!(
            loaded.colors.foreground, original.colors.foreground,
            "foreground must round-trip"
        );
        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn parses_named_colors_and_reset_alias() {
        assert_eq!(parse_color_str("red", "x").unwrap(), Color::Red);
        assert_eq!(parse_color_str("RED", "x").unwrap(), Color::Red);
        assert_eq!(parse_color_str("reset", "x").unwrap(), Color::Reset);
        assert_eq!(parse_color_str("default", "x").unwrap(), Color::Reset);
        assert_eq!(parse_color_str("i0", "x").unwrap(), Color::Indexed(0));
        assert_eq!(parse_color_str("i255", "x").unwrap(), Color::Indexed(255));
        assert_eq!(
            parse_color_str("#1a2b3c", "x").unwrap(),
            Color::Rgb(0x1a, 0x2b, 0x3c)
        );
    }

    #[test]
    fn rejects_invalid_color_strings() {
        assert!(parse_color_str("redd", "x").is_err());
        assert!(parse_color_str("#zz0000", "x").is_err());
        assert!(parse_color_str("#12345", "x").is_err());
        assert!(parse_color_str("i256", "x").is_err());
    }

    #[test]
    fn rgb_round_trip_through_string_form() {
        let cases = [
            (Color::Rgb(0x1a, 0x1b, 0x26), "#1a1b26"),
            (Color::Rgb(0xff, 0x00, 0x80), "#ff0080"),
            (Color::Black, "black"),
            (Color::White, "white"),
            (Color::Red, "red"),
            (Color::Indexed(42), "i42"),
            (Color::Reset, "default"),
        ];
        for (color, expected) in cases {
            let s = color_to_str(color);
            assert_eq!(
                s, expected,
                "color_to_str({color:?}) -> {s:?}, expected {expected:?}"
            );
            let back = parse_color_str(&s, "round-trip").unwrap();
            assert_eq!(
                back, color,
                "parse_color_str({s:?}) -> {back:?}, expected {color:?}"
            );
        }
    }
}
