//! Custom markdown stylesheet for oxi TUI.
//!
//! Overrides the default `tui_markdown` styles to improve visibility
//! and reduce visual noise. Uses warm, muted colors that work on
//! both dark and light terminal backgrounds.

use ratatui::style::{Color, Modifier, Style};
use tui_markdown::StyleSheet;

/// oxi-themed markdown stylesheet.
///
/// All styles are customized for readability:
/// - Headings: subtle tinted backgrounds (not harsh cyan)
/// - Code: warm amber on dark background
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
        // Avoids pure yellow which can cause vibration/eyestrain on dark bg.
        Style::new()
            .fg(Color::Rgb(255, 200, 100)) // #ffc864 warm amber
            .bg(Color::Rgb(35, 30, 20)) // #231e14 warm dark (not gray!)
            .add_modifier(Modifier::BOLD)
    }

    fn link(&self) -> Style {
        Style::new().blue().underlined()
    }

    fn blockquote(&self) -> Style {
        // Use accent purple instead of green for better visibility
        Style::new().fg(Color::Rgb(187, 154, 247))
    }

    fn heading_meta(&self) -> Style {
        Style::new().dim()
    }

    fn metadata_block(&self) -> Style {
        Style::new().light_yellow()
    }
}
