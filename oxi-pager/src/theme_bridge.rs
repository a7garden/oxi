//! Theme bridge — push oxi_tui::Theme into the vendored grok theme global.
//!
//! The vendored `crate::render::theme::Theme` exposes a *process-global*
//! singleton: render code reads `Theme::current()`, which quantizes every
//! color to the terminal's color level. We don't refactor 36k lines of
//! vendored render to take `&Theme` (advisory-confirmed). Instead, this
//! bridge maps the effective oxi theme to the nearest grok `ThemeKind`
//! and pushes it via `Theme::apply_kind(kind)`.
//!
//! Mapping policy (oxi theme name → grok ThemeKind):
//! - `dark` (default), `monokai`, `github_dark` → `GrokNight`
//! - `light` → `GrokDay`
//! - `nord`, `catppuccin` → `TokyoNight` (cool blue/teal family)
//! - fallback → `GrokNight`
//!
//! Called once at startup from the TUI run-mode entry (`oxi_pager::run`)
//! and again whenever the user changes theme in `/settings`.

use crate::render::theme::{Theme as GrokTheme, ThemeKind};

/// Map an oxi theme name to the nearest grok `ThemeKind`.
#[must_use]
pub fn oxi_theme_name_to_grok_kind(name: &str) -> ThemeKind {
    match name.to_ascii_lowercase().as_str() {
        "light" | "light_default" => ThemeKind::GrokDay,
        "nord" | "catppuccin" | "catppuccin-mocha" | "tokyonight" => ThemeKind::TokyoNight,
        "rosepine" | "rosepine-moon" => ThemeKind::RosePineMoon,
        "oscura" | "oscura-midnight" => ThemeKind::OscuraMidnight,
        // dark, monokai, github_dark, and unknowns all fall to GrokNight.
        _ => ThemeKind::GrokNight,
    }
}

/// Map an oxi_tui::Theme (by its background luminance) to a grok kind.
/// Used when only the live `Theme` value is available (no name).
#[must_use]
pub fn grok_kind_for_oxi_theme(theme: &oxi_tui::theme::Theme) -> ThemeKind {
    use ratatui::style::Color;
    // Heuristic: a true-black or near-black background is "night"; otherwise "day".
    match theme.colors.background {
        Color::Rgb(0, 0, 0) | Color::Black => ThemeKind::GrokNight,
        Color::Rgb(r, g, b) if r + g + b < 200 => ThemeKind::GrokNight,
        _ => ThemeKind::GrokDay,
    }
}

/// Push the mapped grok theme into the global so vendored render sees it.
pub fn apply_oxi_theme(theme: &oxi_tui::theme::Theme) {
    let kind = grok_kind_for_oxi_theme(theme);
    GrokTheme::apply_kind(kind);
}

/// One-shot init at TUI startup: pick a sensible default.
pub fn init_default() {
    GrokTheme::apply_kind(ThemeKind::GrokNight);
}
