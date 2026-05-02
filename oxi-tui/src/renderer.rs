use crate::cell::Cell;
use crate::surface::Surface;
use std::io::{self, Write};

/// ANSI escape codes for text attributes.
struct SGR {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    #[allow(dead_code)]
    reversed: bool,
    fg: Option<crate::cell::Color>,
    bg: Option<crate::cell::Color>,
}

impl SGR {
    fn new() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            reversed: false,
            fg: None,
            bg: None,
        }
    }

    fn reset() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            reversed: false,
            fg: None,
            bg: None,
        }
    }

    /// Generate SGR sequence string.
    fn to_sgr(&self) -> String {
        use crate::cell::Color;

        let mut codes = Vec::new();

        // Reset
        codes.push(0);

        if self.bold {
            codes.push(1);
        }
        if self.italic {
            codes.push(3);
        }
        if self.underline {
            codes.push(4);
        }
        if self.strikethrough {
            codes.push(9);
        }

        // Foreground color
        if let Some(fg) = &self.fg {
            match fg {
                Color::Default => codes.extend_from_slice(&[39]),
                Color::Black => codes.push(30),
                Color::Red => codes.push(31),
                Color::Green => codes.push(32),
                Color::Yellow => codes.push(33),
                Color::Blue => codes.push(34),
                Color::Magenta => codes.push(35),
                Color::Cyan => codes.push(36),
                Color::White => codes.push(37),
                Color::Indexed(n) => codes.extend_from_slice(&[38, 5, *n as u8]),
                Color::Rgb(r, g, b) => codes.extend_from_slice(&[38, 2, *r as u8, *g as u8, *b as u8]),
            }
        }

        // Background color
        if let Some(bg) = &self.bg {
            match bg {
                Color::Default => codes.extend_from_slice(&[49]),
                Color::Black => codes.push(40),
                Color::Red => codes.push(41),
                Color::Green => codes.push(42),
                Color::Yellow => codes.push(43),
                Color::Blue => codes.push(44),
                Color::Magenta => codes.push(45),
                Color::Cyan => codes.push(46),
                Color::White => codes.push(47),
                Color::Indexed(n) => codes.extend_from_slice(&[48, 5, *n as u8]),
                Color::Rgb(r, g, b) => codes.extend_from_slice(&[48, 2, *r as u8, *g as u8, *b as u8]),
            }
        }

        codes
            .iter()
            .map(|c| format!("{}", c))
            .collect::<Vec<_>>()
            .join(";")
    }
}

/// Renderer that converts Surface to terminal output.
pub struct Renderer {
    /// Current active SGR for optimization.
    current_sgr: SGR,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            current_sgr: SGR::new(),
        }
    }

    /// Reset the renderer state.
    pub fn reset(&mut self) {
        self.current_sgr = SGR::new();
    }

    /// Write a string to stdout.
    fn write_str(&self, s: &str) {
        print!("{}", s);
    }

    /// Begin a synchronized update (CSI 2026).
    pub fn begin_sync(&self) {
        print!("\x1b[?2026h");
    }

    /// End a synchronized update (CSI 2026).
    pub fn end_sync(&self) -> io::Result<()> {
        print!("\x1b[?2026l");
        io::stdout().flush()
    }

    /// Move cursor to position.
    fn move_cursor(&self, row: u16, col: u16) {
        print!("\x1b[{};{}H", row + 1, col + 1);
    }

    /// Apply SGR codes.
    fn apply_sgr(&mut self, cell: &Cell) -> Option<String> {
        

        let new_sgr = SGR {
            bold: cell.attrs.bold,
            italic: cell.attrs.italic,
            underline: cell.attrs.underline,
            strikethrough: cell.attrs.strikethrough,
            reversed: cell.attrs.reversed,
            fg: Some(cell.fg),
            bg: Some(cell.bg),
        };

        if new_sgr.to_sgr() == self.current_sgr.to_sgr() {
            return None; // No change needed
        }

        self.current_sgr = new_sgr;
        Some(format!("\x1b[{}m", self.current_sgr.to_sgr()))
    }

    /// Clear from cursor to end of line.
    fn clear_to_eol(&self) {
        print!("\x1b[K");
    }

    /// Clear screen.
    pub fn clear_screen(&self) {
        print!("\x1b[2J");
    }

    /// Render a full surface with synchronized updates.
    pub fn render_full(&mut self, surface: &Surface, use_sync: bool) -> io::Result<()> {
        if use_sync {
            self.begin_sync();
        }

        for row in 0..surface.height() {
            for col in 0..surface.width() {
                if let Some(cell) = surface.get(row, col) {
                    self.render_cell(row, col, cell);
                }
            }
        }

        // Reset cursor to beginning
        self.move_cursor(0, 0);

        if use_sync {
            self.end_sync()?;
        }
        Ok(())
    }

    /// Render only dirty cells (differential rendering).
    pub fn render_dirty(&mut self, surface: &Surface, first_dirty: u16, last_dirty: u16) -> io::Result<()> {
        for row in first_dirty..=last_dirty {
            for col in 0..surface.width() {
                if surface.is_dirty(row, col) {
                    if let Some(cell) = surface.get(row, col) {
                        self.render_cell(row, col, cell);
                    }
                }
            }
        }
        Ok(())
    }

    /// Render a single cell at a position.
    pub fn render_cell(&mut self, row: u16, col: u16, cell: &Cell) {
        // Move cursor
        self.move_cursor(row, col);

        // Apply styling if changed
        if let Some(sgr) = self.apply_sgr(cell) {
            self.write_str(&sgr);
        }

        // Write character
        self.write_str(&cell.char.to_string());
    }

    /// Render a single cell at a position and clear to end of line.
    #[allow(dead_code)]
    fn render_cell_at(&mut self, row: u16, col: u16, cell: &Cell) {
        // Move cursor
        self.move_cursor(row, col);

        // Apply styling if changed
        if let Some(sgr) = self.apply_sgr(cell) {
            self.write_str(&sgr);
        }

        // Write character
        self.write_str(&cell.char.to_string());

        // Clear to end of line
        self.clear_to_eol();
    }

    /// Render only changed lines (optimized for most updates).
    pub fn render_changed_lines(
        &mut self,
        surface: &Surface,
        first_dirty: u16,
        last_dirty: u16,
    ) -> io::Result<()> {
        for row in first_dirty..=last_dirty {
            self.move_cursor(row, 0);

            // Check if entire row is dirty
            let mut any_dirty = false;
            for col in 0..surface.width() {
                if surface.is_dirty(row, col) {
                    any_dirty = true;
                    break;
                }
            }

            if !any_dirty {
                continue;
            }

            // Reset SGR for fresh line
            self.current_sgr = SGR::reset();

            // Render the row
            for col in 0..surface.width() {
                if let Some(cell) = surface.get(row, col) {
                    // Apply styling if changed
                    if let Some(sgr) = self.apply_sgr(cell) {
                        self.write_str(&sgr);
                    }
                    // Write character
                    self.write_str(&cell.char.to_string());
                }
            }

            // Clear to end of line
            self.clear_to_eol();
        }

        // Move cursor to first dirty position for next render
        if let Some(_cell) = surface.get(first_dirty, 0) {
            self.move_cursor(first_dirty, 0);
        }

        Ok(())
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Extension trait for rendering to surfaces with ANSI codes.
pub trait RenderToSurface {
    fn to_ansi(&self) -> String;
}

impl RenderToSurface for Cell {
    fn to_ansi(&self) -> String {
        

        let sgr = SGR {
            bold: self.attrs.bold,
            italic: self.attrs.italic,
            underline: self.attrs.underline,
            strikethrough: self.attrs.strikethrough,
            reversed: self.attrs.reversed,
            fg: Some(self.fg),
            bg: Some(self.bg),
        };
        format!("\x1b[{}m{}\x1b[0m", sgr.to_sgr(), self.char)
    }
}