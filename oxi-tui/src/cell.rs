//! Cell - represents a single character cell with styling.

use std::fmt;

/// Text attributes for styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub reversed: bool,
}

impl Attributes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn with_italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn with_underline(mut self) -> Self {
        self.underline = true;
        self
    }
}

/// Text color (24-bit true color).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    /// 256-color palette (0-255)
    Indexed(u8),
    /// 24-bit RGB
    Rgb(u8, u8, u8),
    /// Default terminal color
    Default,
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

/// A single cell in the terminal grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub char: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attributes,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            char: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attributes::new(),
        }
    }
}

impl Cell {
    pub fn new(char: char) -> Self {
        Self {
            char,
            ..Default::default()
        }
    }

    pub fn with_fg(mut self, fg: Color) -> Self {
        self.fg = fg;
        self
    }

    pub fn with_bg(mut self, bg: Color) -> Self {
        self.bg = bg;
        self
    }

    pub fn with_attrs(mut self, attrs: Attributes) -> Self {
        self.attrs = attrs;
        self
    }

    pub fn with_bold(self) -> Self {
        let mut attrs = self.attrs;
        attrs.bold = true;
        self.with_attrs(attrs)
    }

    /// Reset cell to empty with default colors.
    pub fn reset(&mut self) {
        self.char = ' ';
        self.fg = Color::Default;
        self.bg = Color::Default;
        self.attrs = Attributes::new();
    }

    /// Check if cell is empty/default.
    pub fn is_empty(&self) -> bool {
        self.char == ' '
            && self.fg == Color::Default
            && self.bg == Color::Default
            && self.attrs == Attributes::new()
    }
}

impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.char)
    }
}

/// Builder for creating cells with multiple properties.
pub struct CellBuilder {
    char: char,
    fg: Color,
    bg: Color,
    attrs: Attributes,
}

impl CellBuilder {
    pub fn new(char: char) -> Self {
        Self {
            char,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attributes::new(),
        }
    }

    pub fn foreground(mut self, color: Color) -> Self {
        self.fg = color;
        self
    }

    pub fn background(mut self, color: Color) -> Self {
        self.bg = color;
        self
    }

    pub fn bold(mut self) -> Self {
        self.attrs.bold = true;
        self
    }

    pub fn italic(mut self) -> Self {
        self.attrs.italic = true;
        self
    }

    pub fn underline(mut self) -> Self {
        self.attrs.underline = true;
        self
    }

    pub fn build(self) -> Cell {
        Cell {
            char: self.char,
            fg: self.fg,
            bg: self.bg,
            attrs: self.attrs,
        }
    }
}