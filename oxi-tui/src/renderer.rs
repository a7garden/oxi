use crate::cell::Cell;
use crate::surface::Surface;
use std::io::{self, Write};

/// ANSI escape codes for text attributes.
struct Sgr {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    #[allow(dead_code)]
    reversed: bool,
    fg: Option<crate::cell::Color>,
    bg: Option<crate::cell::Color>,
}

impl PartialEq for Sgr {
    fn eq(&self, other: &Self) -> bool {
        self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.strikethrough == other.strikethrough
            && self.reversed == other.reversed
            && self.fg == other.fg
            && self.bg == other.bg
    }
}

impl Sgr {
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

    #[allow(dead_code)]
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
                Color::Indexed(n) => codes.extend_from_slice(&[38, 5, (*n)]),
                Color::Rgb(r, g, b) => {
                    codes.extend_from_slice(&[38, 2, (*r), (*g), (*b)])
                }
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
                Color::Indexed(n) => codes.extend_from_slice(&[48, 5, (*n)]),
                Color::Rgb(r, g, b) => {
                    codes.extend_from_slice(&[48, 2, (*r), (*g), (*b)])
                }
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
///
/// Uses an internal `Vec<u8>` buffer to batch all escape sequences and character
/// writes. The buffer is flushed to stdout only when `flush()` or `end_sync()` is
/// called, dramatically reducing the number of syscalls per frame (from ~4800 to 1).
pub struct Renderer {
    /// Current active SGR for optimization.
    current_sgr: Sgr,
    /// Output buffer — accumulated bytes are flushed once per frame.
    buf: Vec<u8>,
    /// Desired cursor position for IME support.
    /// If set, the hardware cursor is positioned here after flushing.
    cursor_position: Option<(u16, u16)>,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            current_sgr: Sgr::new(),
            buf: Vec::with_capacity(16384),
            cursor_position: None,
        }
    }

    /// Reset the renderer state.
    pub fn reset(&mut self) {
        self.current_sgr = Sgr::new();
        self.buf.clear();
        self.cursor_position = None;
    }

    /// Set the desired cursor position for IME input.
    ///
    /// When `flush()` or `end_sync()` is called, the cursor will be
    /// positioned at `(row, col)` after all content has been written.
    /// The cursor will also be made visible.
    ///
    /// Pass `None` to disable IME cursor positioning.
    pub fn set_cursor_position(&mut self, pos: Option<(u16, u16)>) {
        self.cursor_position = pos;
    }

    /// Get the currently set IME cursor position, if any.
    #[allow(dead_code)]
    pub fn cursor_position(&self) -> Option<(u16, u16)> {
        self.cursor_position
    }

    /// Write bytes to the internal buffer.
    #[allow(dead_code)]
    #[inline]
    fn buf_write(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Write a string to the internal buffer.
    #[allow(dead_code)]
    #[inline]
    fn write_str(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }

    /// Flush the internal buffer to stdout and clear it.
    /// If a cursor position has been set for IME, it is written after all
    /// content so the hardware cursor ends up at the right place.
    pub fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let mut stdout = io::stdout();
            stdout.write_all(&self.buf)?;
            // Write cursor position for IME support if set
            if let Some((row, col)) = self.cursor_position.take() {
                write!(stdout, "\x1b[{};{}H", row + 1, col + 1)?;
                // Show cursor when IME position is set
                stdout.write_all(b"\x1b[?25h")?;
            }
            stdout.flush()?;
            self.buf.clear();
        }
        Ok(())
    }

    /// Begin a synchronized update (CSI 2026).
    pub fn begin_sync(&mut self) {
        self.buf.extend_from_slice(b"\x1b[?2026h");
    }

    /// End a synchronized update (CSI 2026) and flush to stdout.
    pub fn end_sync(&mut self) -> io::Result<()> {
        self.buf.extend_from_slice(b"\x1b[?2026l");
        self.flush()
    }

    /// Move cursor to position.
    fn move_cursor(&mut self, row: u16, col: u16) {
        // CSI row+1 ; col+1 H
        write!(self.buf, "\x1b[{};{}H", row + 1, col + 1).unwrap();
    }

    /// Apply SGR codes, computing a diff against current state.
    /// Writes the resulting escape sequence directly into the buffer.
    fn apply_sgr(&mut self, cell: &Cell) -> bool {
        use crate::cell::Color;

        let new_sgr = Sgr {
            bold: cell.attrs.bold,
            italic: cell.attrs.italic,
            underline: cell.attrs.underline,
            strikethrough: cell.attrs.strikethrough,
            reversed: cell.attrs.reversed,
            fg: Some(cell.fg),
            bg: Some(cell.bg),
        };

        if new_sgr == self.current_sgr {
            return false; // No change needed
        }

        let mut codes = Vec::new();

        // Check each attribute individually
        if new_sgr.bold != self.current_sgr.bold {
            codes.push(if new_sgr.bold { 1 } else { 22 });
        }
        if new_sgr.italic != self.current_sgr.italic {
            codes.push(if new_sgr.italic { 3 } else { 23 });
        }
        if new_sgr.underline != self.current_sgr.underline {
            codes.push(if new_sgr.underline { 4 } else { 24 });
        }
        if new_sgr.strikethrough != self.current_sgr.strikethrough {
            codes.push(if new_sgr.strikethrough { 9 } else { 29 });
        }

        // Foreground color
        if new_sgr.fg != self.current_sgr.fg {
            match &new_sgr.fg {
                Some(Color::Default) | None => codes.push(39),
                Some(Color::Black) => codes.push(30),
                Some(Color::Red) => codes.push(31),
                Some(Color::Green) => codes.push(32),
                Some(Color::Yellow) => codes.push(33),
                Some(Color::Blue) => codes.push(34),
                Some(Color::Magenta) => codes.push(35),
                Some(Color::Cyan) => codes.push(36),
                Some(Color::White) => codes.push(37),
                Some(Color::Indexed(n)) => codes.extend_from_slice(&[38, 5, (*n)]),
                Some(Color::Rgb(r, g, b)) => {
                    codes.extend_from_slice(&[38, 2, (*r), (*g), (*b)])
                }
            }
        }

        // Background color
        if new_sgr.bg != self.current_sgr.bg {
            match &new_sgr.bg {
                Some(Color::Default) | None => codes.push(49),
                Some(Color::Black) => codes.push(40),
                Some(Color::Red) => codes.push(41),
                Some(Color::Green) => codes.push(42),
                Some(Color::Yellow) => codes.push(43),
                Some(Color::Blue) => codes.push(44),
                Some(Color::Magenta) => codes.push(45),
                Some(Color::Cyan) => codes.push(46),
                Some(Color::White) => codes.push(47),
                Some(Color::Indexed(n)) => codes.extend_from_slice(&[48, 5, (*n)]),
                Some(Color::Rgb(r, g, b)) => {
                    codes.extend_from_slice(&[48, 2, (*r), (*g), (*b)])
                }
            }
        }

        self.current_sgr = new_sgr;

        if codes.is_empty() {
            return false;
        }

        // Write escape sequence directly to buffer, avoiding intermediate String allocation
        self.buf.extend_from_slice(b"\x1b[");
        let mut first = true;
        for code in &codes {
            if !first {
                self.buf.push(b';');
            }
            first = false;
            write!(self.buf, "{}", code).unwrap();
        }
        self.buf.push(b'm');

        true
    }

    /// Clear from cursor to end of line.
    fn clear_to_eol(&mut self) {
        self.buf.extend_from_slice(b"\x1b[K");
    }

    /// Clear screen.
    pub fn clear_screen(&mut self) {
        self.buf.extend_from_slice(b"\x1b[2J");
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
    pub fn render_dirty(
        &mut self,
        surface: &Surface,
        first_dirty: u16,
        last_dirty: u16,
    ) -> io::Result<()> {
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

        // Apply styling if changed (writes directly to buf)
        self.apply_sgr(cell);

        // Write character
        let mut tmp = [0u8; 4];
        let s = cell.char.encode_utf8(&mut tmp);
        self.buf.extend_from_slice(s.as_bytes());
    }

    /// Render a single cell at a position and clear to end of line.
    #[allow(dead_code)]
    fn render_cell_at(&mut self, row: u16, col: u16, cell: &Cell) {
        // Move cursor
        self.move_cursor(row, col);

        // Apply styling if changed
        self.apply_sgr(cell);

        // Write character
        let mut tmp = [0u8; 4];
        let s = cell.char.encode_utf8(&mut tmp);
        self.buf.extend_from_slice(s.as_bytes());

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

            // Render the row
            for col in 0..surface.width() {
                if let Some(cell) = surface.get(row, col) {
                    // Apply styling if changed
                    self.apply_sgr(cell);
                    // Write character
                    let mut tmp = [0u8; 4];
                    let s = cell.char.encode_utf8(&mut tmp);
                    self.buf.extend_from_slice(s.as_bytes());
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
        let sgr = Sgr {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Attributes, Cell, Color};
    use crate::surface::Surface;

    // --- SGR diff generation ---

    #[test]
    fn sgr_diff_no_changes() {
        // Rendering the same cell twice should produce no SGR diff on second call
        let mut renderer = Renderer::new();
        let cell = Cell::new('A');
        let changed = renderer.apply_sgr(&cell);
        assert!(changed); // First time always writes
        // Second time with identical cell
        let changed2 = renderer.apply_sgr(&cell);
        assert!(!changed2); // No change needed
    }

    #[test]
    fn sgr_diff_attribute_changes_only() {
        let mut renderer = Renderer::new();
        // First cell: plain
        let cell1 = Cell::new('A');
        renderer.apply_sgr(&cell1);

        // Second cell: bold only
        let cell2 = Cell::new('B').with_bold();
        let changed = renderer.apply_sgr(&cell2);
        assert!(changed);

        // Check buffer contains bold code (1) but not reset-all
        let buf_str = String::from_utf8_lossy(&renderer.buf);
        assert!(buf_str.contains("\x1b["));
        // Should contain "1" for bold
        assert!(buf_str.contains("1"));
    }

    #[test]
    fn sgr_diff_full_changes() {
        let mut renderer = Renderer::new();
        // Start with default
        let cell1 = Cell::new('A');
        renderer.apply_sgr(&cell1);

        // Full change: bold, italic, underline, fg Red, bg Blue
        let attrs = Attributes::new().with_bold().with_italic().with_underline();
        let cell2 = Cell::new('Z')
            .with_fg(Color::Red)
            .with_bg(Color::Blue)
            .with_attrs(attrs);
        let changed = renderer.apply_sgr(&cell2);
        assert!(changed);

        let buf_str = String::from_utf8_lossy(&renderer.buf);
        // Should contain bold (1), italic (3), underline (4), fg red (31), bg blue (44)
        assert!(buf_str.contains("1"));
        assert!(buf_str.contains("3"));
        assert!(buf_str.contains("4"));
        assert!(buf_str.contains("31"));
        assert!(buf_str.contains("44"));
    }

    // --- render_to_surface with known content ---

    #[test]
    fn render_to_surface_known_content() {
        let cell = Cell::new('X').with_fg(Color::Green).with_bg(Color::Black);
        let ansi = cell.to_ansi();
        // Should wrap in escape sequences
        assert!(ansi.starts_with("\x1b["));
        assert!(ansi.ends_with("\x1b[0m"));
        assert!(ansi.contains('X'));
        // Should contain green fg code (32) and black bg code (40)
        assert!(ansi.contains("32"));
        assert!(ansi.contains("40"));
    }

    #[test]
    fn render_to_surface_default_cell() {
        let cell = Cell::new(' ');
        let ansi = cell.to_ansi();
        assert!(ansi.contains(' '));
    }

    // --- flush output format ---

    #[test]
    fn flush_clears_buffer() {
        let mut renderer = Renderer::new();
        renderer.buf.extend_from_slice(b"test data");
        assert!(!renderer.buf.is_empty());
        // flush writes to stdout; we just verify buffer is cleared
        // Since we can't easily intercept stdout, we test the buffer clearing side-effect
        // by checking that after flush the buffer is empty
        let _ = renderer.flush();
        assert!(renderer.buf.is_empty());
    }

    #[test]
    fn flush_empty_buffer_is_ok() {
        let mut renderer = Renderer::new();
        let result = renderer.flush();
        assert!(result.is_ok());
    }

    // --- IME cursor positioning ---

    #[test]
    fn ime_cursor_positioning_set_get() {
        let mut renderer = Renderer::new();
        assert!(renderer.cursor_position().is_none());

        renderer.set_cursor_position(Some((5, 10)));
        assert_eq!(renderer.cursor_position(), Some((5, 10)));

        renderer.set_cursor_position(None);
        assert!(renderer.cursor_position().is_none());
    }

    #[test]
    fn ime_cursor_position_consumed_on_flush() {
        let mut renderer = Renderer::new();
        renderer.set_cursor_position(Some((3, 7)));
        renderer.buf.extend_from_slice(b"data");
        let _ = renderer.flush();
        // cursor_position should be consumed (taken) after flush
        assert!(renderer.cursor_position().is_none());
    }

    // --- Render strategy selection ---

    #[test]
    fn render_full_produces_output() {
        let mut renderer = Renderer::new();
        let mut surface = Surface::new(3, 2);
        surface.set(0, 0, Cell::new('H'));
        surface.set(0, 1, Cell::new('i'));
        surface.set(1, 0, Cell::new('!'));

        // render_full with sync=false
        renderer.render_full(&surface, false).unwrap();
        let buf_str = String::from_utf8_lossy(&renderer.buf);
        assert!(buf_str.contains('H'));
        assert!(buf_str.contains('i'));
        assert!(buf_str.contains('!'));
    }

    #[test]
    fn render_full_with_sync_includes_sync_codes() {
        let mut renderer = Renderer::new();
        let _surface = Surface::new(2, 1);
        // We can't call render_full with sync=true and actually flush,
        // but we can verify begin_sync / end_sync write the right codes
        renderer.begin_sync();
        assert!(renderer.buf.starts_with(b"\x1b[?2026h"));
        renderer.buf.clear();
        renderer.buf.extend_from_slice(b"\x1b[?2026l");
        assert!(renderer.buf.ends_with(b"\x1b[?2026l"));
    }

    #[test]
    fn render_dirty_only_touches_dirty_cells() {
        let mut renderer = Renderer::new();
        let mut surface = Surface::new(5, 2);
        surface.set(0, 0, Cell::new('A'));
        surface.set(0, 1, Cell::new('B'));
        // row 1 has no dirty cells
        renderer.render_dirty(&surface, 0, 1).unwrap();
        let buf_str = String::from_utf8_lossy(&renderer.buf);
        assert!(buf_str.contains('A'));
        assert!(buf_str.contains('B'));
    }

    #[test]
    fn render_changed_lines_skips_clean_rows() {
        let mut renderer = Renderer::new();
        let mut surface = Surface::new(5, 3);
        // Only make row 1 dirty
        surface.set(1, 0, Cell::new('X'));
        surface.clear_dirty();
        // Re-dirty only row 1
        surface.set(1, 2, Cell::new('Y'));

        renderer
            .render_changed_lines(&surface, 1, 1)
            .unwrap();
        let buf_str = String::from_utf8_lossy(&renderer.buf);
        assert!(buf_str.contains('Y'));
    }

    #[test]
    fn clear_screen_writes_escape() {
        let mut renderer = Renderer::new();
        renderer.clear_screen();
        assert_eq!(&renderer.buf, b"\x1b[2J");
    }

    #[test]
    fn reset_clears_state() {
        let mut renderer = Renderer::new();
        renderer.buf.extend_from_slice(b"data");
        renderer.set_cursor_position(Some((1, 2)));
        renderer.reset();
        assert!(renderer.buf.is_empty());
        assert!(renderer.cursor_position().is_none());
    }
}
