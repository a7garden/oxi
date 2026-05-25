//! Custom markdown stylesheet for oxi TUI.
//!
//! Overrides the default `tui_markdown` styles to improve visibility
//! of inline code spans on dark terminals. The upstream default
//! (`white().on_black()`) is nearly invisible on dark themes — this
//! stylesheet uses high-contrast colors that work on both dark and
//! light backgrounds.

use ratatui::style::{Color, Modifier, Style};
use tui_markdown::StyleSheet;

/// oxi-themed markdown stylesheet.
///
/// Only `code()` is overridden for visibility; everything else
/// delegates to `DefaultStyleSheet` values.
#[derive(Clone, Copy, Debug, Default)]
pub struct OxiStyleSheet;

impl StyleSheet for OxiStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            0 | 1 => Style::new().on_cyan().bold().underlined(),
            2 => Style::new().cyan().bold(),
            3 => Style::new().cyan().bold().italic(),
            4..=6 | 7..=u8::MAX => Style::new().light_cyan().italic(),
        }
    }

    fn code(&self) -> Style {
        // High-contrast inline code style that is visible on dark terminals.
        // Yellow text on dark-gray background provides strong differentiation
        // from normal text while remaining readable on light themes too.
        Style::new()
            .fg(Color::Yellow)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    }

    fn link(&self) -> Style {
        Style::new().blue().underlined()
    }

    fn blockquote(&self) -> Style {
        Style::new().green()
    }

    fn heading_meta(&self) -> Style {
        Style::new().dim()
    }

    fn metadata_block(&self) -> Style {
        Style::new().light_yellow()
    }
}
