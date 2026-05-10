//! Cell primitives — Color type for theme integration.
//!
//! Provides the foundational color type used by the theme system
//! and ratatui style conversion.

use ratatui::style::{Color as RColor, Style};

/// Text color (24-bit true color).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// Black.
    Black,
    /// Red.
    Red,
    /// Green.
    Green,
    /// Yellow.
    Yellow,
    /// Blue.
    Blue,
    /// Magenta.
    Magenta,
    /// Cyan.
    Cyan,
    /// White.
    White,
    /// 256-color palette (0-255)
    Indexed(u8),
    /// 24-bit RGB
    Rgb(u8, u8, u8),
    /// Default terminal color
    #[default]
    Default,
}

impl Color {
    /// Convert to ratatui Color.
    pub fn to_ratatui(&self) -> RColor {
        match self {
            Color::Black => RColor::Black,
            Color::Red => RColor::Red,
            Color::Green => RColor::Green,
            Color::Yellow => RColor::Yellow,
            Color::Blue => RColor::Blue,
            Color::Magenta => RColor::Magenta,
            Color::Cyan => RColor::Cyan,
            Color::White => RColor::White,
            Color::Indexed(n) => RColor::Indexed(*n),
            Color::Rgb(r, g, b) => RColor::Rgb(*r, *g, *b),
            Color::Default => RColor::Reset,
        }
    }

    /// Convert to ratatui Style with optional background.
    pub fn to_style(&self, bg: Option<Color>) -> Style {
        let fg = self.to_ratatui();
        match bg {
            Some(bg_color) => Style::new().fg(fg).bg(bg_color.to_ratatui()),
            None => Style::new().fg(fg),
        }
    }
}
