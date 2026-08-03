//! Custom markdown stylesheet for oxicode TUI.
//!
//! Implements `tui_markdown::StyleSheet` with **theme-aware** colors.
//! Construct via [`OxicodeStyleSheet::from_styles`] so that heading, code, and
//! link styles all derive from the active `ThemeStyles` — switching themes
//! (dark ↔ light ↔ nord …) now recolours inline markdown too, not just the
//! chat viewport background.
//!
//! ## What is theme-aware here vs. not
//!
//! | Element | Source |
//! |---|---|
//! | Inline `` `code` `` | `styles.code_fg` / `styles.code_bg` |
//! | H1–H3 | `styles.accent` / `styles.primary` |
//! | H4–H6 | `styles.muted` |
//! | Links | `styles.primary` (underlined) |
//! | Blockquotes | `styles.accent` |
//! | Metadata | `styles.muted` |
//!
//! Fenced code blocks (``` ``` ```) are handled separately by
//! `highlight_code`, which
//! already receives `&ThemeStyles`.

use ratatui::style::{Color, Modifier, Style};
use tui_markdown::StyleSheet;

use crate::theme::ThemeStyles;

/// oxicode-themed markdown stylesheet — **theme-aware**.
///
/// Holds the semantic colors extracted from `ThemeStyles` at construction
/// time.  Because `StyleSheet` methods take `&self`, the colours must be
/// baked in up-front (the trait has no way to receive per-call context).
#[derive(Clone, Copy, Debug)]
pub struct OxicodeStyleSheet {
    code_fg: Color,
    code_bg: Color,
    accent: Color,
    primary: Color,
    muted: Color,
}

impl OxicodeStyleSheet {
    /// Build a stylesheet whose colours track the active theme.
    pub fn from_styles(styles: &ThemeStyles) -> Self {
        Self {
            code_fg: styles.code_fg.fg.unwrap_or(Color::Rgb(255, 200, 100)),
            code_bg: styles.code_bg.bg.unwrap_or(Color::Reset),
            accent: styles.accent.fg.unwrap_or(Color::Magenta),
            primary: styles.primary.fg.unwrap_or(Color::Blue),
            muted: styles.muted.fg.unwrap_or(Color::DarkGray),
        }
    }
}

impl StyleSheet for OxicodeStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            0 | 1 => Style::new().fg(self.accent).add_modifier(Modifier::BOLD),
            2 => Style::new().fg(self.primary).add_modifier(Modifier::BOLD),
            3 => Style::new()
                .fg(self.primary)
                .add_modifier(Modifier::BOLD | Modifier::ITALIC),
            _ => Style::new().fg(self.muted).add_modifier(Modifier::ITALIC),
        }
    }

    fn code(&self) -> Style {
        // Theme-aware inline code: code_fg on code_bg.
        Style::new()
            .fg(self.code_fg)
            .bg(self.code_bg)
            .add_modifier(Modifier::BOLD)
    }

    fn link(&self) -> Style {
        Style::new()
            .fg(self.primary)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::new().fg(self.accent)
    }

    fn heading_meta(&self) -> Style {
        Style::new().fg(self.muted).add_modifier(Modifier::DIM)
    }

    fn metadata_block(&self) -> Style {
        Style::new().fg(self.muted)
    }
}
