//! Cell - represents a single character cell with styling.

use ratatui::style::{Color as RColor, Modifier, Style};
use std::fmt;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
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

    /// Convert to ratatui Style.
    pub fn to_style(&self, bg: Option<Color>) -> Style {
        let fg = self.to_ratatui();
        match bg {
            Some(bg_color) => Style::new().fg(fg).bg(bg_color.to_ratatui()),
            None => Style::new().fg(fg),
        }
    }
}


/// A single cell in the terminal grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The character stored in this cell.
    pub char: char,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Text attributes.
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
    /// Create a cell with a single character and default styling.
    pub fn new(char: char) -> Self {
        Self {
            char,
            ..Default::default()
        }
    }

    /// Set the foreground color, returning the cell.
    pub fn with_fg(mut self, fg: Color) -> Self {
        self.fg = fg;
        self
    }

    /// Set the background color, returning the cell.
    pub fn with_bg(mut self, bg: Color) -> Self {
        self.bg = bg;
        self
    }

    /// Set the text attributes, returning the cell.
    pub fn with_attrs(mut self, attrs: Attributes) -> Self {
        self.attrs = attrs;
        self
    }

    /// Add bold attribute.
    pub fn with_bold(self) -> Self {
        let mut attrs = self.attrs;
        attrs.bold = true;
        self.with_attrs(attrs)
    }

    /// Get the display width of this cell's character.
    pub fn width(&self) -> usize {
        unicode_width::UnicodeWidthChar::width(self.char).unwrap_or(0)
    }

    /// Create a wide-character continuation cell (placeholder for 2nd column of a wide char).
    pub fn wide_continuation() -> Self {
        Self {
            char: '\u{0}',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attributes::new(),
        }
    }

    /// Check if this cell is a wide-char continuation.
    pub fn is_wide_continuation(&self) -> bool {
        self.char == '\u{0}'
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
    /// Character to render.
    char: char,
    /// Foreground color.
    fg: Color,
    /// Background color.
    bg: Color,
    /// Text attributes.
    attrs: Attributes,
}

impl CellBuilder {
    /// Start building a cell with the given character.
    pub fn new(char: char) -> Self {
        Self {
            char,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attributes::new(),
        }
    }

    /// Set the foreground color.
    pub fn foreground(mut self, color: Color) -> Self {
        self.fg = color;
        self
    }

    /// Set the background color.
    pub fn background(mut self, color: Color) -> Self {
        self.bg = color;
        self
    }

    /// Add bold attribute.
    pub fn bold(mut self) -> Self {
        self.attrs.bold = true;
        self
    }

    /// Add italic attribute.
    pub fn italic(mut self) -> Self {
        self.attrs.italic = true;
        self
    }

    /// Add underline attribute.
    pub fn underline(mut self) -> Self {
        self.attrs.underline = true;
        self
    }

    /// Build the final cell.
    pub fn build(self) -> Cell {
        Cell {
            char: self.char,
            fg: self.fg,
            bg: self.bg,
            attrs: self.attrs,
        }
    }
}