//! Terminal abstraction for cross-platform support.

use std::io::{self, Write};

use anyhow::Result;

/// Terminal dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

/// Terminal position (0-indexed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub row: u16,
    pub col: u16,
}

/// Cursor visibility state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorVisibility {
    #[default]
    Visible,
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

    fn supports_sync_update(&self) -> bool {
        // Most modern terminals support CSI 2026
        true
    }
}
