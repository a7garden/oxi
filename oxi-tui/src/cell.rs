//! Cell primitives — Color and Attributes types for theme integration.
//!
//! Provides the foundational color and text-attribute types used by the
//! theme system and ratatui style conversion.

use ratatui::style::{Color as RColor, Modifier, Style};

/// Text attributes for styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attributes {
    /// Bold / bright weight.
    pub bold: bool,
    /// Italic / oblique style.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
    /// Strikethrough.
    pub strikethrough: bool,
    /// Reversed foreground/background.
    pub reversed: bool,
}

impl Attributes {
    /// Create a new empty attribute set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add bold attribute.
    pub fn with_bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Add italic attribute.
    pub fn with_italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Add underline attribute.
    pub fn with_underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// Convert to ratatui Modifier.
    pub fn to_modifier(&self) -> Modifier {
        let mut modifier = Modifier::empty();
        if self.bold {
            modifier |= Modifier::BOLD;
        }
        if self.italic {
            modifier |= Modifier::ITALIC;
        }
        if self.underline {
            modifier |= Modifier::UNDERLINED;
        }
        if self.strikethrough {
            modifier |= Modifier::CROSSED_OUT;
        }
        if self.reversed {
            modifier |= Modifier::REVERSED;
        }
        modifier
    }
}

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
