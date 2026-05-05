//! Surface - a 2D grid of cells forming the render buffer.

use crate::cell::{Attributes, Cell, Color};

/// A rectangular area in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// X coordinate (column).
    pub x: u16,
    /// Y coordinate (row).
    pub y: u16,
    /// Width in columns.
    pub width: u16,
    /// Height in rows.
    pub height: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn from_size(width: u16, height: u16) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    /// Check if a point is within this rectangle.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Get the inner rectangle with an inset border.
    pub fn inner(&self, border: u16) -> Self {
        Rect::new(
            self.x + border,
            self.y + border,
            self.width.saturating_sub(border * 2),
            self.height.saturating_sub(border * 2),
        )
    }

    /// Get right edge x coordinate.
    pub fn right(&self) -> u16 {
        self.x + self.width
    }

    /// Get bottom edge y coordinate.
    pub fn bottom(&self) -> u16 {
        self.y + self.height
    }

    /// Check if this rectangle is valid (non-zero dimensions).
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// Intersect with another rectangle.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());

        if right > x && bottom > y {
            Some(Rect::new(x, y, right - x, bottom - y))
        } else {
            None
        }
    }
}

/// A 2D surface containing cells ready for rendering.
///
/// Surface provides a grid of cells that can be written to and then
/// rendered to the terminal. It tracks dirty cells for efficient updates.
#[derive(Debug, Clone)]
pub struct Surface {
    width: u16,
    height: u16,
    cells: Vec<Vec<Cell>>,
    /// Track which cells have changed since last render.
    dirty_cells: Vec<Vec<bool>>,
    /// First changed row index.
    first_dirty_row: Option<u16>,
    /// Last changed row index.
    last_dirty_row: Option<u16>,
}

impl Surface {
    /// Create a new surface with given dimensions.
    pub fn new(width: u16, height: u16) -> Self {
        let cells = vec![vec![Cell::default(); width as usize]; height as usize];
        let dirty_cells = vec![vec![false; width as usize]; height as usize];
        Self {
            width,
            height,
            cells,
            dirty_cells,
            first_dirty_row: None,
            last_dirty_row: None,
        }
    }

    /// Get the surface width.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Get the surface height.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Get the area as a Rect.
    pub fn area(&self) -> Rect {
        Rect::from_size(self.width, self.height)
    }

    /// Get a cell at (row, col).
    pub fn get(&self, row: u16, col: u16) -> Option<&Cell> {
        if row < self.height && col < self.width {
            self.cells
                .get(row as usize)
                .and_then(|r| r.get(col as usize))
        } else {
            None
        }
    }

    /// Get a mutable cell at (row, col).
    pub fn get_mut(&mut self, row: u16, col: u16) -> Option<&mut Cell> {
        if row < self.height && col < self.width {
            let r = self.cells.get_mut(row as usize)?;
            r.get_mut(col as usize)
        } else {
            None
        }
    }

    /// Set a cell and mark it dirty.
    pub fn set(&mut self, row: u16, col: u16, cell: Cell) {
        if row < self.height && col < self.width {
            self.cells[row as usize][col as usize] = cell;
            self.mark_dirty(row, col);
        }
    }

    /// Write a string at (row, col), respecting width limits and wide characters.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use oxi_tui::Surface;
    ///
    /// let mut surface = Surface::new(20, 5);
    /// surface.write_string(0, 0, "Hello, world!");
    /// surface.write_string(1, 0, "こんにちは"); // Korean chars are 2 columns wide
    /// surface.write_string(2, 0, "Hello 世界"); // Mixed ASCII + CJK
    /// ```
    pub fn write_string(&mut self, row: u16, col: u16, s: &str) {
        let mut current_col = col as usize;
        for c in s.chars() {
            let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
            if current_col + char_width > self.width as usize {
                break;
            }
            if current_col < self.width as usize {
                self.set(row, current_col as u16, Cell::new(c));
                // For wide chars, mark subsequent cells as continuation placeholders
                if char_width > 1 {
                    for offset in 1..char_width {
                        let next_col = current_col + offset;
                        if next_col < self.width as usize {
                            self.cells[row as usize][next_col] = Cell::wide_continuation();
                            self.mark_dirty(row, next_col as u16);
                        }
                    }
                }
            }
            current_col += char_width;
        }
    }

    /// Write a styled string at (row, col) with given fg/bg/attrs.
    pub fn write_string_styled(
        &mut self,
        row: u16,
        col: u16,
        s: &str,
        fg: Color,
        bg: Color,
        attrs: Attributes,
    ) {
        let mut current_col = col as usize;
        for c in s.chars() {
            let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
            if current_col + char_width > self.width as usize {
                break;
            }
            if current_col < self.width as usize {
                let cell = Cell::new(c).with_fg(fg).with_bg(bg).with_attrs(attrs);
                self.set(row, current_col as u16, cell);
                if char_width > 1 {
                    for offset in 1..char_width {
                        let next_col = current_col + offset;
                        if next_col < self.width as usize {
                            self.cells[row as usize][next_col] =
                                Cell::wide_continuation().with_bg(bg);
                            self.mark_dirty(row, next_col as u16);
                        }
                    }
                }
            }
            current_col += char_width;
        }
    }

    /// Write a string and fill the rest of the row (from end of string to end of row) with spaces using the given bg color.
    pub fn write_line(&mut self, row: u16, col: u16, s: &str, fg: Color, bg: Color) {
        self.write_string_styled(row, col, s, fg, bg, Attributes::new());
        // Fill remaining columns with bg-colored spaces
        // We need to figure out where the string ended in column terms
        let mut current_col = col as usize;
        for c in s.chars() {
            let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
            current_col += char_width;
        }
        let space_cell = Cell::new(' ').with_bg(bg);
        for c in current_col..self.width as usize {
            self.set(row, c as u16, space_cell.clone());
        }
    }

    /// Fill a row with a character.
    pub fn fill_row(&mut self, row: u16, start_col: u16, end_col: u16, cell: Cell) {
        for col in start_col..end_col.min(self.width) {
            self.set(row, col, cell.clone());
        }
    }

    /// Fill entire surface with a cell value.
    pub fn fill(&mut self, cell: Cell) {
        for row in 0..self.height {
            for col in 0..self.width {
                self.set(row, col, cell.clone());
            }
        }
    }

    /// Clear the surface with default cells.
    pub fn clear(&mut self) {
        for row in 0..self.height {
            for col in 0..self.width {
                let mut c = Cell::default();
                c.reset();
                self.cells[row as usize][col as usize] = c;
            }
        }
        self.clear_dirty();
    }

    /// Mark a cell as dirty (changed since last render).
    fn mark_dirty(&mut self, row: u16, col: u16) {
        self.dirty_cells[row as usize][col as usize] = true;
        self.first_dirty_row = Some(self.first_dirty_row.map_or(row, |r| r.min(row)));
        self.last_dirty_row = Some(self.last_dirty_row.map_or(row, |r| r.max(row)));
    }

    /// Check if a cell is dirty.
    pub fn is_dirty(&self, row: u16, col: u16) -> bool {
        if row < self.height && col < self.width {
            self.dirty_cells[row as usize][col as usize]
        } else {
            false
        }
    }

    /// Get first dirty row.
    pub fn first_dirty(&self) -> Option<u16> {
        self.first_dirty_row
    }

    /// Get last dirty row.
    pub fn last_dirty(&self) -> Option<u16> {
        self.last_dirty_row
    }

    /// Check if surface has any dirty cells.
    pub fn is_any_dirty(&self) -> bool {
        self.first_dirty_row.is_some()
    }

    /// Clear the dirty tracking.
    pub fn clear_dirty(&mut self) {
        for row in &mut self.dirty_cells {
            for cell in row.iter_mut() {
                *cell = false;
            }
        }
        self.first_dirty_row = None;
        self.last_dirty_row = None;
    }

    /// Mark entire surface as dirty.
    pub fn mark_all_dirty(&mut self) {
        for row in 0..self.height {
            for col in 0..self.width {
                self.dirty_cells[row as usize][col as usize] = true;
            }
        }
        self.first_dirty_row = Some(0);
        self.last_dirty_row = Some(self.height.saturating_sub(1));
    }

    /// Compare with another surface and mark differences.
    pub fn diff_from(&mut self, other: &Surface) {
        for row in 0..self.height.min(other.height) {
            for col in 0..self.width.min(other.width) {
                if self.cells[row as usize][col as usize] != other.cells[row as usize][col as usize]
                {
                    self.mark_dirty(row, col);
                }
            }
        }
    }
}

impl Default for Surface {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_ascii_string() {
        let mut surface = Surface::new(20, 1);
        surface.write_string(0, 0, "Hello");
        assert_eq!(surface.get(0, 0).unwrap().char, 'H');
        assert_eq!(surface.get(0, 1).unwrap().char, 'e');
        assert_eq!(surface.get(0, 4).unwrap().char, 'o');
        // Column 5 should still be default
        assert_eq!(surface.get(0, 5).unwrap().char, ' ');
    }

    #[test]
    fn write_korean_string() {
        // Each Korean character is 2 columns wide
        let mut surface = Surface::new(20, 1);
        surface.write_string(0, 0, "안녕하세요");
        // 5 chars * 2 columns = 10 columns
        assert_eq!(surface.get(0, 0).unwrap().char, '안');
        assert!(surface.get(0, 1).unwrap().is_wide_continuation());
        assert_eq!(surface.get(0, 2).unwrap().char, '녕');
        assert!(surface.get(0, 3).unwrap().is_wide_continuation());
        assert_eq!(surface.get(0, 4).unwrap().char, '하');
        assert!(surface.get(0, 5).unwrap().is_wide_continuation());
        assert_eq!(surface.get(0, 6).unwrap().char, '세');
        assert!(surface.get(0, 7).unwrap().is_wide_continuation());
        assert_eq!(surface.get(0, 8).unwrap().char, '요');
        assert!(surface.get(0, 9).unwrap().is_wide_continuation());
        // Column 10 should still be default
        assert_eq!(surface.get(0, 10).unwrap().char, ' ');
    }

    #[test]
    fn write_mixed_string() {
        // "Hello 세계" = H(1) e(1) l(1) l(1) o(1) ' '(1) 세(2) 계(2) = 10 columns
        let mut surface = Surface::new(20, 1);
        surface.write_string(0, 0, "Hello 세계");
        assert_eq!(surface.get(0, 0).unwrap().char, 'H');
        assert_eq!(surface.get(0, 5).unwrap().char, ' ');
        // '세' starts at column 6
        assert_eq!(surface.get(0, 6).unwrap().char, '세');
        assert!(surface.get(0, 7).unwrap().is_wide_continuation());
        // '계' starts at column 8
        assert_eq!(surface.get(0, 8).unwrap().char, '계');
        assert!(surface.get(0, 9).unwrap().is_wide_continuation());
        // Column 10 is empty
        assert_eq!(surface.get(0, 10).unwrap().char, ' ');
    }

    #[test]
    fn write_string_overflows_width() {
        // Surface is 5 columns wide, writing "안녕하세요" (10 columns)
        let mut surface = Surface::new(5, 1);
        surface.write_string(0, 0, "안녕하세요");
        // Only '안' fits (2 cols) + '녕' would need cols 2-3, fits too
        // '하' would need cols 4-5, but col 5 doesn't exist (width=5, cols 0-4)
        // Actually: '안' = cols 0-1, '녕' = cols 2-3, '하' needs cols 4-5, 5 >= width -> break
        assert_eq!(surface.get(0, 0).unwrap().char, '안');
        assert!(surface.get(0, 1).unwrap().is_wide_continuation());
        assert_eq!(surface.get(0, 2).unwrap().char, '녕');
        assert!(surface.get(0, 3).unwrap().is_wide_continuation());
        // Col 4 should be default (wide char '하' doesn't fit)
        assert_eq!(surface.get(0, 4).unwrap().char, ' ');
    }

    #[test]
    fn wide_continuation_cell() {
        let cont = Cell::wide_continuation();
        assert!(cont.is_wide_continuation());
        assert_eq!(cont.char, '\u{0}');

        let normal = Cell::new('A');
        assert!(!normal.is_wide_continuation());
    }

    #[test]
    fn cell_unicode_width() {
        assert_eq!(Cell::new('A').width(), 1);
        assert_eq!(Cell::new('안').width(), 2);
        assert_eq!(Cell::new(' ').width(), 1);
    }

    #[test]
    fn write_string_styled_applies_colors() {
        let mut surface = Surface::new(20, 1);
        surface.write_string_styled(
            0,
            0,
            "Hi",
            Color::Red,
            Color::Blue,
            Attributes::new().with_bold(),
        );
        let cell = surface.get(0, 0).unwrap();
        assert_eq!(cell.char, 'H');
        assert_eq!(cell.fg, Color::Red);
        assert_eq!(cell.bg, Color::Blue);
        assert!(cell.attrs.bold);
    }

    #[test]
    fn write_line_fills_rest_with_bg_spaces() {
        let mut surface = Surface::new(10, 1);
        surface.write_line(0, 0, "Hi", Color::Default, Color::Blue);
        assert_eq!(surface.get(0, 0).unwrap().char, 'H');
        assert_eq!(surface.get(0, 1).unwrap().char, 'i');
        // Remaining cols should be spaces with Blue bg
        for col in 2..10 {
            let cell = surface.get(0, col).unwrap();
            assert_eq!(cell.char, ' ');
            assert_eq!(cell.bg, Color::Blue);
        }
    }
}
