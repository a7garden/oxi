//! Syntax highlighting initialization.
//!
//! Provides lazily-initialized `Syntect` instances for code highlighting.
//! Dark themes (GrokNight, TokyoNight) share `grok-night.tmTheme`;
//! GrokDay uses `grok-day.tmTheme` with deepened colors for light backgrounds.
//!
//! ## Minimal / terminal-native lock
//!
//! While [`crate::render::theme::cache::terminal_native_locked`] is set, chrome uses
//! [`Theme::terminal_default`](crate::render::theme::Theme::terminal_default) and
//! `current_kind()` is a nominal `GrokNight` (so leftover kind-keyed paths
//! still resolve). Syntect therefore loads the night `.tmTheme` whose pastel
//! RGB tokens, after naive ANSI-16 quantization, collapse to **White** —
//! invisible on light terminal profiles.
//!
//! Under the lock we do **not** detect light/dark. Instead:
//! 1. Near-gray tokens → `Color::Reset` (terminal default fg; always readable).
//! 2. Chromatic tokens → base ANSI-16 accents (Red/Green/Yellow/Blue/Magenta/Cyan),
//!    never White/Black/bright variants.
//!
//! That matches the "first + second" minimal syntax policy: default-fg baseline
//! plus a dual-polarity accent map, with zero polarity detection.

use std::sync::OnceLock;

pub use oxi_vendor_grok_markdown::Syntect;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::render::theme::ThemeKind;

static SYNTECT_GROKNIGHT: OnceLock<Syntect> = OnceLock::new();
static SYNTECT_TOKYONIGHT: OnceLock<Syntect> = OnceLock::new();
static SYNTECT_GROKDAY: OnceLock<Syntect> = OnceLock::new();

/// Convert syntect style to ratatui foreground-only style, quantized for
/// terminal color support (or polarity-safe under the terminal-native lock).
pub fn syntect_to_ratatui_fg(style: syntect::highlighting::Style) -> Style {
    let fg = syntect_rgb_to_fg(style.foreground.r, style.foreground.g, style.foreground.b);
    let mut out = Style::default().fg(fg);
    use syntect::highlighting::FontStyle;
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

/// Map a syntect RGB triplet to a ratatui foreground color.
///
/// Under the terminal-native lock, uses [`polarity_safe_syntax_fg`]; otherwise
/// quantizes via the normal theme color pipeline.
pub fn syntect_rgb_to_fg(r: u8, g: u8, b: u8) -> Color {
    if crate::render::theme::cache::terminal_native_locked() {
        polarity_safe_syntax_fg(r, g, b)
    } else {
        crate::render::theme::quantize(Color::Rgb(r, g, b))
    }
}

/// Dual-polarity-safe ANSI mapping for syntax tokens on a transparent canvas.
///
/// - Low chroma (gray / near-gray body text) → [`Color::Reset`] so the host
///   default fg carries contrast on both light and dark profiles.
/// - Saturated hues → base ANSI Red/Green/Yellow/Blue/Magenta/Cyan only.
///
/// Never returns White, Black, or bright (Light*) variants — those are the
/// colors that vanish on the opposite polarity after naive RGB→ANSI16.
pub fn polarity_safe_syntax_fg(r: u8, g: u8, b: u8) -> Color {
    let max = r.max(g).max(b) as i32;
    let min = r.min(g).min(b) as i32;
    let chroma = max - min;
    // Night default body (~#c8c8c8) and dim comments are near-gray.
    if chroma < 40 {
        return Color::Reset;
    }
    // Integer HSV hue in degrees [0, 360).
    let (ri, gi, bi) = (r as i32, g as i32, b as i32);
    let h = if max == ri {
        let mut h = (gi - bi) * 60 / chroma;
        if h < 0 {
            h += 360;
        }
        h
    } else if max == gi {
        (bi - ri) * 60 / chroma + 120
    } else {
        (ri - gi) * 60 / chroma + 240
    };
    // Magenta starts at 255° so Tokyo Night purple (#bb9af7, ~261°) lands
    // Magenta rather than Blue; pure blues (~221°) stay Blue.
    match h {
        0..30 | 330..=360 => Color::Red,
        30..90 => Color::Yellow,
        90..150 => Color::Green,
        150..210 => Color::Cyan,
        210..255 => Color::Blue,
        _ => Color::Magenta,
    }
}

/// Highlight a single line of source, falling back to plain text style.
///
/// Under the terminal-native lock, syntect tokens are remapped via
/// [`polarity_safe_syntax_fg`]; if highlighting fails, `fallback` (typically
/// [`Theme::primary`](crate::render::theme::Theme::primary) = Reset) is used.
pub fn highlight_line(
    text: &str,
    highlighter: &mut Option<syntect::easy::HighlightLines<'_>>,
    syntect: &Syntect,
    fallback: Style,
) -> Vec<Span<'static>> {
    if let Some(hl) = highlighter.as_mut()
        && let Ok(ranges) = hl.highlight_line(&format!("{text}\n"), &syntect.syntax_set)
    {
        let mut spans = Vec::new();
        for (style, segment) in ranges {
            let mut s = segment.to_owned();
            while s.ends_with('\n') || s.ends_with('\r') {
                s.pop();
            }
            if s.is_empty() {
                continue;
            }
            spans.push(Span::styled(s, syntect_to_ratatui_fg(style)));
        }
        if !spans.is_empty() {
            return spans;
        }
    }
    vec![Span::styled(text.to_string(), fallback)]
}

/// Returns the syntect instance matching the active theme.
///
/// Note: while the terminal-native lock is engaged, [`Theme::current_kind`]
/// reports a nominal `GrokNight`, so this returns the night theme. Token
/// colors are remapped in [`syntect_to_ratatui_fg`] — do not load a day
/// theme based on OS/terminal polarity detection.
pub fn get_syntect() -> &'static Syntect {
    match crate::render::theme::Theme::current_kind() {
        ThemeKind::GrokNight
        | ThemeKind::RosePineMoon
        | ThemeKind::OscuraMidnight
        | ThemeKind::Auto => SYNTECT_GROKNIGHT
            .get_or_init(|| Syntect::new(include_bytes!("../assets/grok-night.tmTheme"))),
        ThemeKind::TokyoNight => SYNTECT_TOKYONIGHT
            .get_or_init(|| Syntect::new(include_bytes!("../assets/tokyo-night.tmTheme"))),
        ThemeKind::GrokDay => SYNTECT_GROKDAY
            .get_or_init(|| Syntect::new(include_bytes!("../assets/grok-day.tmTheme"))),
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
