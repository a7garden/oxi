//! Differential rendering backend for ratatui.
//!
//! Wraps `CrosstermBackend` with a line-level diffing layer. Only changed rows
//! are written to the terminal, dramatically reducing I/O for streaming AI chat
//! where most of the screen stays static between frames.

pub mod ansi;
pub mod diff;
pub mod image;
pub mod terminal;

use std::fmt;
use std::io;

use ratatui::{
    backend::{Backend, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error type for DiffBackend.
#[derive(Debug)]
pub struct DiffBackendError(io::Error);

impl fmt::Display for DiffBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for DiffBackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<io::Error> for DiffBackendError {
    fn from(e: io::Error) -> Self {
        DiffBackendError(e)
    }
}

// ---------------------------------------------------------------------------
// DiffBackend
// ---------------------------------------------------------------------------

/// A ratatui `Backend` wrapper that performs line-level differential rendering.
///
/// Instead of writing every cell to the terminal on each frame, it compares the
/// new frame buffer with the previous one and only writes changed rows.
pub struct DiffBackend<W: io::Write> {
    /// The underlying crossterm backend.
    inner: ratatui::backend::CrosstermBackend<W>,
    /// Whether we need to force a full redraw.
    force_full_redraw: bool,
}

impl<W: io::Write> DiffBackend<W> {
    /// Create a new DiffBackend wrapping the given crossterm backend.
    pub fn new(inner: ratatui::backend::CrosstermBackend<W>) -> Self {
        DiffBackend {
            inner,
            force_full_redraw: true,
        }
    }

    /// Force a full redraw on the next frame.
    pub fn invalidate(&mut self) {
        self.force_full_redraw = true;
    }
}

impl<W: io::Write> Backend for DiffBackend<W> {
    type Error = DiffBackendError;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)?;
        self.force_full_redraw = false;
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()?;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()?;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.inner.get_cursor_position()?)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)?;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.force_full_redraw = true;
        self.inner.clear()?;
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> Result<(), Self::Error> {
        self.force_full_redraw = true;
        self.inner.clear_region(clear_type)?;
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(self.inner.size()?)
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(self.inner.window_size()?)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()?;
        Ok(())
    }
}
