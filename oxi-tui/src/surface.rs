//! Surface - a 2D grid of cells forming the render buffer.

use crate::cell::{Cell, Color};

/// A rectangular area in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self { x, y, width, height }
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
            self.cells.get(row as usize).and_then(|r| r.get(col as usize))
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

    /// Write a string at (row, col), respecting width limits.
    pub fn write_string(&mut self, row: u16, col: u16, s: &str) {
        for (i, c) in s.chars().enumerate() {
            let col = col + i as u16;
            if col >= self.width {
                break;
            }
            self.set(row, col, Cell::new(c));
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
                if self.cells[row as usize][col as usize]
                    != other.cells[row as usize][col as usize]
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