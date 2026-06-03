//! Custom markdown stylesheet for oxi TUI.
//!
//! Overrides the default `tui_markdown` styles to use theme-aware colors.
//! When `ThemeStyles` is available, code and heading styles derive from the
//! active theme. Otherwise, dark-theme defaults are used.

use ratatui::style::{Color, Modifier, Style};
use tui_markdown::StyleSheet;

/// oxi-themed markdown stylesheet.
///
/// All styles are customized for readability:
/// - Headings: accent purple (theme-aware)
/// - Code: warm amber on dark background (from theme `code_fg`/`code_bg`)
/// - Links: visible blue with underline
#[derive(Clone, Copy, Debug, Default)]
pub struct OxiStyleSheet;

impl StyleSheet for OxiStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            0 | 1 => Style::new()
                .fg(Color::Rgb(187, 154, 247)) // #bb9af7 accent purple
                .add_modifier(Modifier::BOLD),
            2 => Style::new().cyan().bold(),
            3 => Style::new().cyan().bold().italic(),
            4..=6 | 7..=u8::MAX => Style::new().light_cyan().italic(),
        }
    }

    fn code(&self) -> Style {
        // Warm amber text on subtle dark background.
        // These match the default dark theme's code_fg/code_bg.
        // When ThemeStyles is plumbed through tui-markdown's Options,
        // the theme's actual code_fg/code_bg will override these.
        Style::new()
            .fg(Color::Rgb(255, 200, 100)) // #ffc864 warm amber
            .bg(Color::Rgb(35, 30, 20)) // #231e14 warm dark
            .add_modifier(Modifier::BOLD)
    }

    fn link(&self) -> Style {
        Style::new().blue().underlined()
    }

    fn blockquote(&self) -> Style {
        Style::new().fg(Color::Rgb(187, 154, 247))
    }

    fn heading_meta(&self) -> Style {
        Style::new().dim()
    }

    fn metadata_block(&self) -> Style {
        Style::new().light_yellow()
    }
}
