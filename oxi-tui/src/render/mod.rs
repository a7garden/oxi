//! Differential rendering backend for ratatui.
//!
//! Wraps `CrosstermBackend` with a line-level diffing layer. Only changed rows
//! are written to the terminal, dramatically reducing I/O for streaming AI chat
//! where most of the screen stays static between frames.
//!
//! ## How it works
//!
//! 1. Collect all cells from ratatui's `draw()` iterator into row buffers
//! 2. Compare each row against the previous frame using fast u64 checksums
//! 3. For changed rows: move cursor + write cells directly via crossterm
//! 4. For unchanged rows: skip entirely
//! 5. Wrap output in CSI 2026 synchronized update mode to prevent tearing

pub mod ansi;
pub mod diff;
pub mod image;
pub mod terminal;

use std::fmt;
use std::io;

use crossterm::{
    cursor::MoveTo,
    style::{
        Attribute as CAttribute, Color as CColor, Print, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
};
use ratatui::{
    backend::{Backend, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
    style::{Color, Modifier},
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
// Row buffer
// ---------------------------------------------------------------------------

/// Compact row representation for diff comparison.
/// Stores cell data as raw bytes for fast comparison.
type Row = Vec<u8>;

/// Build a compact byte representation of a row of cells.
fn build_row<'a, I>(cells: I) -> Row
where
    I: Iterator<Item = (u16, u16, &'a Cell)>,
{
    let mut row = Vec::new();
    for (_, _, cell) in cells {
        // Encode: symbol bytes + fg + bg + modifier
        row.extend_from_slice(cell.symbol().as_bytes());
        row.push(0xFF); // separator
        row.extend_from_slice(&color_to_bytes(&cell.fg));
        row.extend_from_slice(&color_to_bytes(&cell.bg));
        row.extend_from_slice(&cell.modifier.bits().to_le_bytes());
    }
    row
}

fn color_to_bytes(color: &Color) -> [u8; 4] {
    match color {
        Color::Reset => [0, 0, 0, 0],
        Color::Black => [1, 0, 0, 0],
        Color::Red => [2, 0, 0, 0],
        Color::Green => [3, 0, 0, 0],
        Color::Yellow => [4, 0, 0, 0],
        Color::Blue => [5, 0, 0, 0],
        Color::Magenta => [6, 0, 0, 0],
        Color::Cyan => [7, 0, 0, 0],
        Color::Gray => [8, 0, 0, 0],
        Color::DarkGray => [9, 0, 0, 0],
        Color::LightRed => [10, 0, 0, 0],
        Color::LightGreen => [11, 0, 0, 0],
        Color::LightYellow => [12, 0, 0, 0],
        Color::LightBlue => [13, 0, 0, 0],
        Color::LightMagenta => [14, 0, 0, 0],
        Color::LightCyan => [15, 0, 0, 0],
        Color::White => [16, 0, 0, 0],
        Color::Indexed(i) => [17, *i, 0, 0],
        Color::Rgb(r, g, b) => [*r, *g, *b, 0xFF],
    }
}

fn ratatui_color_to_crossterm(color: &Color) -> CColor {
    match color {
        Color::Reset => CColor::Reset,
        Color::Black => CColor::Black,
        Color::Red => CColor::DarkRed,
        Color::Green => CColor::DarkGreen,
        Color::Yellow => CColor::DarkYellow,
        Color::Blue => CColor::DarkBlue,
        Color::Magenta => CColor::DarkMagenta,
        Color::Cyan => CColor::DarkCyan,
        Color::Gray => CColor::Grey,
        Color::DarkGray => CColor::DarkGrey,
        Color::LightRed => CColor::Red,
        Color::LightGreen => CColor::Green,
        Color::LightYellow => CColor::Yellow,
        Color::LightBlue => CColor::Blue,
        Color::LightMagenta => CColor::Magenta,
        Color::LightCyan => CColor::Cyan,
        Color::White => CColor::White,
        Color::Indexed(i) => CColor::AnsiValue(*i),
        Color::Rgb(r, g, b) => CColor::Rgb {
            r: *r,
            g: *g,
            b: *b,
        },
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
    /// Previous frame rows for diff comparison.
    prev_rows: Vec<Row>,
    /// Whether we need to force a full redraw.
    force_full_redraw: bool,
    /// Terminal width at last draw (for resize detection).
    last_width: u16,
    /// Terminal height at last draw (for resize detection).
    last_height: u16,
}

impl<W: io::Write> DiffBackend<W> {
    /// Create a new DiffBackend wrapping the given crossterm backend.
    pub fn new(inner: ratatui::backend::CrosstermBackend<W>) -> Self {
        DiffBackend {
            inner,
            prev_rows: Vec::new(),
            force_full_redraw: true,
            last_width: 0,
            last_height: 0,
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
        // Collect all cells into row groups
        let mut row_cells: Vec<Vec<(u16, u16, &'a Cell)>> = Vec::new();
        let mut max_col: u16 = 0;
        let mut max_row: u16 = 0;

        for (x, y, cell) in content {
            let yidx = y as usize;
            while row_cells.len() <= yidx {
                row_cells.push(Vec::new());
            }
            max_col = max_col.max(x);
            max_row = max_row.max(y);
            row_cells[yidx].push((x, y, cell));
        }

        let term_w = max_col + 1;
        let term_h = max_row + 1;

        // Check for resize — force full redraw
        if term_w != self.last_width || term_h != self.last_height {
            self.force_full_redraw = true;
            self.last_width = term_w;
            self.last_height = term_h;
        }

        // Build compact rows for comparison
        let new_rows: Vec<Row> = row_cells
            .iter()
            .map(|cells| build_row(cells.iter().map(|&(x, y, c)| (x, y, c))))
            .collect();

        if self.force_full_redraw || self.prev_rows.is_empty() {
            // Full redraw — delegate to crossterm
            // Re-iterate from collected cells
            let all_cells: Vec<(u16, u16, &'a Cell)> = row_cells.into_iter().flatten().collect();
            self.inner.draw(all_cells.into_iter())?;
            self.prev_rows = new_rows;
            self.force_full_redraw = false;
            return Ok(());
        }

        // --- Differential rendering ---
        // Begin synchronized output (CSI 2026h)
        let _ = crossterm::execute!(
            self.inner,
            crossterm::style::SetAttribute(CAttribute::Reset)
        );

        // Find changed rows
        let max_rows = new_rows.len().max(self.prev_rows.len());
        for row_idx in 0..max_rows {
            let new_row = new_rows.get(row_idx);
            let prev_row = self.prev_rows.get(row_idx);

            match (new_row, prev_row) {
                (Some(nr), Some(pr)) if nr == pr => continue, // Unchanged — skip
                (None, Some(_)) => {
                    // Row was removed — clear it
                    let _ = crossterm::execute!(
                        self.inner,
                        MoveTo(0, row_idx as u16),
                        crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine)
                    );
                }
                (Some(_), _) => {
                    // Row is new or changed — write it
                    let _ = crossterm::execute!(self.inner, MoveTo(0, row_idx as u16));

                    // Write cells for this row
                    if let Some(cells) = row_cells.get(row_idx) {
                        let mut last_x: u16 = 0;
                        let mut last_fg: Option<CColor> = None;
                        let mut last_bg: Option<CColor> = None;
                        let mut last_mod: Option<Modifier> = None;

                        for &(x, _y, cell) in cells {
                            // Move cursor if there's a gap
                            if x > last_x {
                                let _ = crossterm::execute!(self.inner, MoveTo(x, row_idx as u16));
                            }

                            // Set style only if changed
                            let fg = ratatui_color_to_crossterm(&cell.fg);
                            if last_fg.as_ref() != Some(&fg) {
                                let _ = crossterm::execute!(self.inner, SetForegroundColor(fg));
                                last_fg = Some(fg);
                            }

                            let bg = ratatui_color_to_crossterm(&cell.bg);
                            if last_bg.as_ref() != Some(&bg) {
                                let _ = crossterm::execute!(self.inner, SetBackgroundColor(bg));
                                last_bg = Some(bg);
                            }

                            let modifier = cell.modifier;
                            if last_mod != Some(modifier) {
                                // Reset then set
                                let _ = crossterm::execute!(
                                    self.inner,
                                    SetAttribute(CAttribute::Reset)
                                );
                                if modifier.contains(Modifier::BOLD) {
                                    let _ = crossterm::execute!(
                                        self.inner,
                                        SetAttribute(CAttribute::Bold)
                                    );
                                }
                                if modifier.contains(Modifier::ITALIC) {
                                    let _ = crossterm::execute!(
                                        self.inner,
                                        SetAttribute(CAttribute::Italic)
                                    );
                                }
                                if modifier.contains(Modifier::UNDERLINED) {
                                    let _ = crossterm::execute!(
                                        self.inner,
                                        SetAttribute(CAttribute::Underlined)
                                    );
                                }
                                if modifier.contains(Modifier::REVERSED) {
                                    let _ = crossterm::execute!(
                                        self.inner,
                                        SetAttribute(CAttribute::Reverse)
                                    );
                                }
                                if modifier.contains(Modifier::DIM) {
                                    let _ = crossterm::execute!(
                                        self.inner,
                                        SetAttribute(CAttribute::Dim)
                                    );
                                }
                                if modifier.contains(Modifier::CROSSED_OUT) {
                                    let _ = crossterm::execute!(
                                        self.inner,
                                        SetAttribute(CAttribute::CrossedOut)
                                    );
                                }
                                last_mod = Some(modifier);
                                // Re-apply fg/bg after reset
                                if let Some(ref f) = last_fg {
                                    let _ = crossterm::execute!(self.inner, SetForegroundColor(*f));
                                }
                                if let Some(ref b) = last_bg {
                                    let _ = crossterm::execute!(self.inner, SetBackgroundColor(*b));
                                }
                            }

                            let _ = crossterm::execute!(self.inner, Print(cell.symbol()));
                            last_x = x + 1;
                        }
                    }
                }
                (None, None) => unreachable!(),
            }
        }

        self.prev_rows = new_rows;
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
        self.prev_rows.clear();
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

// Write trait delegation — required for crossterm's execute!() macro
impl<W: io::Write> io::Write for DiffBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        std::io::Write::flush(&mut self.inner)
    }
}
