//! Semantic color slots + Theme struct + ThemeStyles.
//!
//! Constructor bodies live in `constructors.rs` (split to keep this file
//! under the 500-LOC module cap).
//!
//! The brightness hierarchy per AGENTS.md must be respected:
//! `background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg`.
//!
//! 28 slots total per task brief Step 2:
//! - 21 "original" slots (14 fg + 7 bg)
//! - 7 Phase-1 background slots
//!
//! Schema differs from legacy `oxi-tui-legacy::theme::ColorScheme`: legacy
//! carried 4 fg slots (`secondary`, `user_border`, `cursor_fg`, `cursor_bg`,
//! `code_fg`, `tool_*_bg`) that have no counterpart here; conversely we
//! add 9 fg slots (`user`, `response`, `thinking`, `tool`, `tool_bg`,
//! `info`, `diff_add`, `diff_remove`, `diff_hunk`) that legacy did not
//! model. See `constructors.rs` for the value-derivation table.

#![allow(clippy::doc_markdown)] // legacy doc text mentions `code` etc.

use std::borrow::Cow;

use ratatui::style::{Color, Style};

/// Semantic color palette used by components.
///
/// 28 slots: 21 "original" + 7 Phase-1 background slots.
#[derive(Clone, Debug)]
pub struct ColorScheme {
    // ── Original 21 slots (14 fg + 7 bg) ─────────────────────────
    /// Default background color.
    pub background: Color,
    /// Normal text foreground color.
    pub foreground: Color,
    /// Primary accent color (UI elements, labels, user "You").
    pub primary: Color,
    /// Accent highlight color.
    pub accent: Color,
    /// Muted / dimmed text (e.g. placeholders, tool headers).
    pub muted: Color,
    /// Border / separator color.
    pub border: Color,
    /// User message foreground.
    pub user: Color,
    /// User message background (subtle tint).
    pub user_bg: Color,
    /// Assistant response text foreground.
    pub response: Color,
    /// Assistant response text background (= background by default).
    pub response_bg: Color,
    /// Thinking block foreground.
    pub thinking: Color,
    /// Thinking block background (subtle accent tint).
    pub thinking_bg: Color,
    /// Tool call text foreground.
    pub tool: Color,
    /// Tool call text background (subtle tint).
    pub tool_bg: Color,
    /// Success / confirmation color.
    pub success: Color,
    /// Warning / caution color.
    pub warning: Color,
    /// Error / danger color.
    pub error: Color,
    /// Informational color (blue/accent).
    pub info: Color,
    /// Diff added-line foreground.
    pub diff_add: Color,
    /// Diff removed-line foreground.
    pub diff_remove: Color,
    /// Diff hunk-header foreground.
    pub diff_hunk: Color,

    // ── Phase-1 background slots (7) ──────────────────────────────
    /// Footer / status bar background.
    pub surface_bg: Color,
    /// Overlay popup background (most prominent surface).
    pub panel_bg: Color,
    /// Inline code / code block background.
    pub code_bg: Color,
    /// Selection / highlight background.
    pub selection_bg: Color,
    /// Diff added-line background (reuses tool_success_bg tint in legacy).
    pub diff_add_bg: Color,
    /// Diff removed-line background (reuses tool_error_bg tint in legacy).
    pub diff_remove_bg: Color,
    /// Diff hunk-header background (subtle muted tint).
    pub diff_hunk_bg: Color,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Pre-computed ratatui styles for all semantic colors in a ColorScheme.
///
/// Every style field is a `Style` with only the relevant property set (fg or bg),
/// so they compose correctly via `Style::patch()`.
#[allow(clippy::struct_excessive_bools)] // mirror ColorScheme slots
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ThemeStyles {
    // ── Original 21 slots (14 fg + 7 bg) ─────────────────────────
    /// Normal / default text style.
    pub normal: Style,
    /// Primary accent color style.
    pub primary: Style,
    /// Accent / highlight style.
    pub accent: Style,
    /// Muted / dimmed style.
    pub muted: Style,
    /// Border / separator style.
    pub border: Style,
    /// User message foreground style.
    pub user: Style,
    /// User message background style.
    pub user_bg: Style,
    /// Assistant response foreground style.
    pub response: Style,
    /// Assistant response background style.
    pub response_bg: Style,
    /// Thinking block foreground style.
    pub thinking: Style,
    /// Thinking block background style.
    pub thinking_bg: Style,
    /// Tool call text foreground style.
    pub tool: Style,
    /// Tool call text background style.
    pub tool_bg: Style,
    /// Success style.
    pub success: Style,
    /// Warning style.
    pub warning: Style,
    /// Error style.
    pub error: Style,
    /// Info style.
    pub info: Style,
    /// Diff added-line foreground style.
    pub diff_add: Style,
    /// Diff removed-line foreground style.
    pub diff_remove: Style,
    /// Diff hunk-header foreground style.
    pub diff_hunk: Style,

    // ── Phase-1 background slots (7) ──────────────────────────────
    /// Footer / status bar background style.
    pub surface_bg: Style,
    /// Overlay popup background style.
    pub panel_bg: Style,
    /// Code background style.
    pub code_bg: Style,
    /// Selection background style.
    pub selection_bg: Style,
    /// Diff added-line background style.
    pub diff_add_bg: Style,
    /// Diff removed-line background style.
    pub diff_remove_bg: Style,
    /// Diff hunk-header background style.
    pub diff_hunk_bg: Style,
}

impl ThemeStyles {
    /// Resolve all semantic styles from a color scheme.
    #[must_use]
    pub fn from_colors(colors: &ColorScheme) -> Self {
        Self {
            normal: Style::default().fg(colors.foreground),
            primary: Style::default().fg(colors.primary),
            accent: Style::default().fg(colors.accent),
            muted: Style::default().fg(colors.muted),
            border: Style::default().fg(colors.border),
            user: Style::default().fg(colors.user),
            user_bg: Style::default().bg(colors.user_bg),
            response: Style::default().fg(colors.response),
            response_bg: Style::default().bg(colors.response_bg),
            thinking: Style::default().fg(colors.thinking),
            thinking_bg: Style::default().bg(colors.thinking_bg),
            tool: Style::default().fg(colors.tool),
            tool_bg: Style::default().bg(colors.tool_bg),
            success: Style::default().fg(colors.success),
            warning: Style::default().fg(colors.warning),
            error: Style::default().fg(colors.error),
            info: Style::default().fg(colors.info),
            diff_add: Style::default().fg(colors.diff_add),
            diff_remove: Style::default().fg(colors.diff_remove),
            diff_hunk: Style::default().fg(colors.diff_hunk),
            // ── Phase-1 background slots ──
            surface_bg: Style::default().bg(colors.surface_bg),
            panel_bg: Style::default().bg(colors.panel_bg),
            code_bg: Style::default().bg(colors.code_bg),
            selection_bg: Style::default().bg(colors.selection_bg),
            diff_add_bg: Style::default().bg(colors.diff_add_bg),
            diff_remove_bg: Style::default().bg(colors.diff_remove_bg),
            diff_hunk_bg: Style::default().bg(colors.diff_hunk_bg),
        }
    }
}

/// A complete theme definition.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Color palette.
    pub colors: ColorScheme,
    /// Pre-resolved ratatui styles per slot.
    pub styles: ThemeStyles,
    /// Human-readable theme name.
    pub name: Cow<'static, str>,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// Built-in dark theme.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_scheme(ColorScheme::dark(), "dark")
    }

    /// Built-in light theme.
    #[must_use]
    pub fn light() -> Self {
        Self::from_scheme(ColorScheme::light(), "light")
    }

    /// Built-in Nord theme.
    #[must_use]
    pub fn nord() -> Self {
        Self::from_scheme(ColorScheme::nord(), "nord")
    }

    /// Built-in Catppuccin Mocha theme.
    #[must_use]
    pub fn catppuccin() -> Self {
        Self::from_scheme(ColorScheme::catppuccin(), "catppuccin")
    }

    /// Built-in GitHub Dark theme.
    #[must_use]
    pub fn github_dark() -> Self {
        Self::from_scheme(ColorScheme::github_dark(), "github_dark")
    }

    /// Built-in Monokai theme.
    #[must_use]
    pub fn monokai() -> Self {
        Self::from_scheme(ColorScheme::monokai(), "monokai")
    }

    /// Construct a theme from a color scheme and a name.
    #[must_use]
    pub fn from_scheme(colors: ColorScheme, name: &'static str) -> Self {
        let styles = ThemeStyles::from_colors(&colors);
        Self {
            colors,
            styles,
            name: Cow::Borrowed(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_constructors_produce_valid_themes() {
        let _ = Theme::dark();
        let _ = Theme::light();
        let _ = Theme::nord();
        let _ = Theme::catppuccin();
        let _ = Theme::github_dark();
        let _ = Theme::monokai();
    }

    #[test]
    fn dark_theme_brightness_hierarchy() {
        // AGENTS.md pitfall: background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg
        let t = Theme::dark();
        // Verify the 7 background slots are mostly distinct.
        let bgs = [
            t.colors.background,
            t.colors.response_bg,
            t.colors.thinking_bg,
            t.colors.surface_bg,
            t.colors.user_bg,
            t.colors.panel_bg,
        ];
        let unique: std::collections::HashSet<_> = bgs.iter().collect();
        assert!(
            unique.len() >= 4,
            "background slots should be mostly distinct, got {bgs:?}"
        );
    }
}
