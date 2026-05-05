//! Terminal abstraction for cross-platform support.

use std::io::{self, Write};

use anyhow::Result;

/// Terminal dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// Width in columns.
    pub width: u16,
    /// Height in rows.
    pub height: u16,
}

impl Size {
    /// Create a new Size.
    pub fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

/// Terminal position (0-indexed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// Row number.
    pub row: u16,
    /// Column number.
    pub col: u16,
}

/// Cursor visibility state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorVisibility {
    /// Cursor is visible.
    #[default]
    Visible,
    /// Cursor is hidden.
    Hidden,
}

/// Abstract terminal interface.
pub trait Terminal: Send {
    /// Get the current terminal size.
    fn size(&mut self) -> Result<Size>;

    /// Get cursor position.
    fn cursor_pos(&self) -> Result<Position>;

    /// Set cursor position.
    fn set_cursor_pos(&mut self, pos: Position) -> Result<()>;

    /// Set cursor visibility.
    fn set_cursor_visibility(&mut self, visibility: CursorVisibility) -> Result<()>;

    /// Clear the entire screen.
    fn clear_screen(&mut self) -> Result<()>;

    /// Clear from cursor to end of line.
    fn clear_line(&mut self) -> Result<()>;

    /// Flush pending output.
    fn flush(&mut self) -> Result<()>;

    /// Query the terminal for the current cursor position.
    /// Writes `ESC[6n` to stdout and flushes. The terminal will respond
    /// with `ESC[row;colR` which should be parsed by the event loop.
    fn query_cursor_position(&mut self) -> Result<()>;

    /// Set the hardware cursor position for IME support.
    /// This writes the cursor positioning escape sequence directly.
    fn set_ime_cursor(&mut self, row: u16, col: u16) -> Result<()>;

    /// Check if terminal supports synchronized output (CSI 2026).
    fn supports_sync_update(&self) -> bool {
        true
    }

    /// Enter synchronized update mode.
    fn begin_sync_update(&mut self) -> Result<()> {
        print!("\x1b[?2026h");
        Ok(())
    }

    /// Exit synchronized update mode.
    fn end_sync_update(&mut self) -> Result<()> {
        print!("\x1b[?2026l");
        io::stdout().flush()?;
        Ok(())
    }
}

/// Standard terminal implementation using crossterm.
pub struct CrosstermTerminal {
    size_cache: Size,
}

impl CrosstermTerminal {
    /// Create a new terminal, querying the current size.
    pub fn new() -> Result<Self> {
        let size = Self::get_size()?;
        Ok(Self { size_cache: size })
    }

    fn get_size() -> Result<Size> {
        let (cols, rows) = crossterm::terminal::size()?;
        Ok(Size {
            width: cols,
            height: rows,
        })
    }
}

impl Default for CrosstermTerminal {
    fn default() -> Self {
        Self::new().expect("Failed to initialize terminal")
    }
}

impl Terminal for CrosstermTerminal {
    fn size(&mut self) -> Result<Size> {
        // Refresh size from terminal
        let new_size = Self::get_size()?;
        self.size_cache = new_size;
        Ok(new_size)
    }

    fn cursor_pos(&self) -> Result<Position> {
        let (col, row) = crossterm::cursor::position()?;
        Ok(Position { row, col })
    }

    fn set_cursor_pos(&mut self, pos: Position) -> Result<()> {
        crossterm::execute!(io::stdout(), crossterm::cursor::MoveTo(pos.col, pos.row))?;
        Ok(())
    }

    fn set_cursor_visibility(&mut self, visibility: CursorVisibility) -> Result<()> {
        match visibility {
            CursorVisibility::Visible => {
                crossterm::execute!(io::stdout(), crossterm::cursor::Show)?
            }
            CursorVisibility::Hidden => crossterm::execute!(io::stdout(), crossterm::cursor::Hide)?,
        }
        Ok(())
    }

    fn clear_screen(&mut self) -> Result<()> {
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
        )?;
        Ok(())
    }

    fn clear_line(&mut self) -> Result<()> {
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine)
        )?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        io::stdout().flush()?;
        Ok(())
    }

    fn query_cursor_position(&mut self) -> Result<()> {
        // ESC[6n — Device Status Report: request cursor position
        io::stdout().write_all(b"\x1b[6n")?;
        io::stdout().flush()?;
        Ok(())
    }

    fn set_ime_cursor(&mut self, row: u16, col: u16) -> Result<()> {
        // ESC[row+1;col+1H — Cursor Position
        write!(io::stdout(), "\x1b[{};{}H", row + 1, col + 1)?;
        io::stdout().flush()?;
        Ok(())
    }

    fn supports_sync_update(&self) -> bool {
        // Most modern terminals support CSI 2026
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock terminal for testing the trait without a real TTY.
    struct MockTerminal {
        size: Size,
        cursor: Position,
        cursor_visible: CursorVisibility,
    }

    impl MockTerminal {
        fn new(w: u16, h: u16) -> Self {
            Self {
                size: Size::new(w, h),
                cursor: Position { row: 0, col: 0 },
                cursor_visible: CursorVisibility::Visible,
            }
        }
    }

    impl Terminal for MockTerminal {
        fn size(&mut self) -> Result<Size> {
            Ok(self.size)
        }
        fn cursor_pos(&self) -> Result<Position> {
            Ok(self.cursor)
        }
        fn set_cursor_pos(&mut self, pos: Position) -> Result<()> {
            self.cursor = pos;
            Ok(())
        }
        fn set_cursor_visibility(&mut self, v: CursorVisibility) -> Result<()> {
            self.cursor_visible = v;
            Ok(())
        }
        fn clear_screen(&mut self) -> Result<()> {
            Ok(())
        }
        fn clear_line(&mut self) -> Result<()> {
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
        fn query_cursor_position(&mut self) -> Result<()> {
            Ok(())
        }
        fn set_ime_cursor(&mut self, _row: u16, _col: u16) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn size_struct_new() {
        let s = Size::new(80, 24);
        assert_eq!(s.width, 80);
        assert_eq!(s.height, 24);
    }

    #[test]
    fn position_struct_fields() {
        let p = Position { row: 5, col: 10 };
        assert_eq!(p.row, 5);
        assert_eq!(p.col, 10);
    }

    #[test]
    fn cursor_visibility_default() {
        assert_eq!(CursorVisibility::default(), CursorVisibility::Visible);
    }

    #[test]
    fn mock_terminal_size() {
        let mut t = MockTerminal::new(100, 50);
        let size = t.size().unwrap();
        assert_eq!(size.width, 100);
        assert_eq!(size.height, 50);
    }

    #[test]
    fn mock_terminal_cursor_movement() {
        let mut t = MockTerminal::new(80, 24);
        let pos = Position { row: 10, col: 20 };
        t.set_cursor_pos(pos).unwrap();
        let result = t.cursor_pos().unwrap();
        assert_eq!(result.row, 10);
        assert_eq!(result.col, 20);
    }

    #[test]
    fn mock_terminal_cursor_visibility() {
        let mut t = MockTerminal::new(80, 24);
        t.set_cursor_visibility(CursorVisibility::Hidden).unwrap();
        assert_eq!(t.cursor_visible, CursorVisibility::Hidden);
        t.set_cursor_visibility(CursorVisibility::Visible).unwrap();
        assert_eq!(t.cursor_visible, CursorVisibility::Visible);
    }

    #[test]
    fn terminal_trait_object_safe() {
        // Verify we can use the trait as a trait object (Box<dyn Terminal>)
        let _t: Box<dyn Terminal> = Box::new(MockTerminal::new(80, 24));
        // If this compiles, the trait is object-safe
    }

    #[test]
    fn terminal_trait_dispatch() {
        // Verify dynamic dispatch works correctly
        let mut t: Box<dyn Terminal> = Box::new(MockTerminal::new(120, 40));
        let size = t.size().unwrap();
        assert_eq!(size.width, 120);
        assert_eq!(size.height, 40);

        t.set_cursor_pos(Position { row: 5, col: 5 }).unwrap();
        let pos = t.cursor_pos().unwrap();
        assert_eq!(pos.row, 5);
        assert_eq!(pos.col, 5);
    }

    #[test]
    fn default_sync_update_supported() {
        let t = MockTerminal::new(80, 24);
        assert!(t.supports_sync_update());
    }
}
