use crate::cell::Cell;
use crate::surface::Surface;
use std::io::{self, Write};

/// ANSI escape codes for text attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Write SGR escape sequence directly into a String buffer.
    /// Avoids intermediate Vec allocation and per-code format!() calls.
    fn write_sgr_to(&self, buf: &mut String) {
        use crate::cell::Color;

        buf.push_str("\x1b[");

        let mut first = true;

        let emit = |buf: &mut String, code: u16, first: &mut bool| {
            if !*first {
                buf.push(';');
            }
            *first = false;
            write_u16(buf, code);
        };

        // Reset is always first
        emit(buf, 0, &mut first);

        if self.bold { emit(buf, 1, &mut first); }
        if self.italic { emit(buf, 3, &mut first); }
        if self.underline { emit(buf, 4, &mut first); }
        if self.strikethrough { emit(buf, 9, &mut first); }

        // Foreground color
        if let Some(fg) = &self.fg {
            match fg {
                Color::Default => emit(buf, 39, &mut first),
                Color::Black => emit(buf, 30, &mut first),
                Color::Red => emit(buf, 31, &mut first),
                Color::Green => emit(buf, 32, &mut first),
                Color::Yellow => emit(buf, 33, &mut first),
                Color::Blue => emit(buf, 34, &mut first),
                Color::Magenta => emit(buf, 35, &mut first),
                Color::Cyan => emit(buf, 36, &mut first),
                Color::White => emit(buf, 37, &mut first),
                Color::Indexed(n) => {
                    emit(buf, 38, &mut first);
                    emit(buf, 5, &mut first);
                    emit(buf, *n as u16, &mut first);
                }
                Color::Rgb(r, g, b) => {
                    emit(buf, 38, &mut first);
                    emit(buf, 2, &mut first);
                    emit(buf, *r as u16, &mut first);
                    emit(buf, *g as u16, &mut first);
                    emit(buf, *b as u16, &mut first);
                }
            }
        }

        // Background color
        if let Some(bg) = &self.bg {
            match bg {
                Color::Default => emit(buf, 49, &mut first),
                Color::Black => emit(buf, 40, &mut first),
                Color::Red => emit(buf, 41, &mut first),
                Color::Green => emit(buf, 42, &mut first),
                Color::Yellow => emit(buf, 43, &mut first),
                Color::Blue => emit(buf, 44, &mut first),
                Color::Magenta => emit(buf, 45, &mut first),
                Color::Cyan => emit(buf, 46, &mut first),
                Color::White => emit(buf, 47, &mut first),
                Color::Indexed(n) => {
                    emit(buf, 48, &mut first);
                    emit(buf, 5, &mut first);
                    emit(buf, *n as u16, &mut first);
                }
                Color::Rgb(r, g, b) => {
                    emit(buf, 48, &mut first);
                    emit(buf, 2, &mut first);
                    emit(buf, *r as u16, &mut first);
                    emit(buf, *g as u16, &mut first);
                    emit(buf, *b as u16, &mut first);
                }
            }
        }

        buf.push('m');
    }

    /// Generate SGR sequence string (kept for backward compat).
    #[allow(dead_code)]
    fn to_sgr(&self) -> String {
        let mut buf = String::with_capacity(32);
        self.write_sgr_to(&mut buf);
        buf
    }
}

/// Write a u16 value to a String buffer without using format!().
#[inline]
fn write_u16(buf: &mut String, mut n: u16) {
    if n == 0 {
        buf.push('0');
        return;
    }
    let mut digits = [0u8; 5];
    let mut i = 0;
    while n > 0 {
        digits[i] = (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    for j in (0..i).rev() {
        buf.push((b'0' + digits[j]) as char);
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

    /// Move cursor to position. Uses direct buffer writing instead of format!().
    #[inline]
    fn move_cursor(&self, row: u16, col: u16) {
        let mut buf = String::with_capacity(16);
        buf.push_str("\x1b[");
        write_u16(&mut buf, row + 1);
        buf.push(';');
        write_u16(&mut buf, col + 1);
        buf.push('H');
        print!("{}", buf);
    }

    /// Apply SGR codes — optimized to compare SGR structs directly
    /// and write to a pre-allocated buffer.
    #[inline]
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

        if new_sgr == self.current_sgr {
            return None; // No change needed
        }

        self.current_sgr = new_sgr;
        let mut buf = String::with_capacity(32);
        self.current_sgr.write_sgr_to(&mut buf);
        Some(buf)
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

        // Write character — avoid allocating a String for a single char
        print!("{}", cell.char);
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
        print!("{}", cell.char);

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
                    print!("{}", cell.char);
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
        let mut buf = String::with_capacity(48);
        sgr.write_sgr_to(&mut buf);
        buf.push(self.char);
        buf.push_str("\x1b[0m");
        buf
    }
}
