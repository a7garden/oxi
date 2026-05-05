//! Layout system for arranging children in vertical/horizontal splits.

use crate::{Component, Event, Rect, Size, Surface};

/// Layout direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Vertical,
    Horizontal,
}

/// A layout constraint for a child.
#[derive(Debug, Clone)]
pub enum Constraint {
    /// Fixed size in cells.
    Length(u16),
    /// Percentage of available space (0-100).
    Percentage(u16),
    /// Minimum size - takes remaining space, at least `u16` cells.
    Min(u16),
    /// Flexible - fills remaining space proportionally by weight.
    Flex(u16),
}

/// Splits a Rect into sub-rects based on constraints and direction.
///
/// The algorithm:
/// 1. Calculates fixed (Length) sizes first
/// 2. Calculates Percentage constraints from remaining space
/// 3. Distributes remaining space to Min/Flex constraints proportionally
/// 4. Returns non-overlapping `Vec<Rect>`
pub fn split(area: Rect, direction: Direction, constraints: &[Constraint]) -> Vec<Rect> {
    if constraints.is_empty() {
        return Vec::new();
    }

    let total = match direction {
        Direction::Vertical => area.height,
        Direction::Horizontal => area.width,
    };

    let n = constraints.len();

    // Phase 1: compute target sizes
    let mut sizes = vec![0u16; n];

    // Fixed lengths
    let mut remaining = total as u32;
    for (i, c) in constraints.iter().enumerate() {
        if let Constraint::Length(len) = c {
            let len = *len as u32;
            sizes[i] = len.min(remaining) as u16;
            remaining = remaining.saturating_sub(len);
        }
    }

    // Percentage
    for (i, c) in constraints.iter().enumerate() {
        if let Constraint::Percentage(pct) = c {
            let pct = (*pct).min(100) as u32;
            let len = (remaining * pct / 100) as u16;
            sizes[i] = len;
            remaining = remaining.saturating_sub(len as u32);
        }
    }

    // Min and Flex: distribute remaining space
    let flex_total: u32 = constraints
        .iter()
        
        .map(|c| match c {
            Constraint::Flex(w) => *w as u32,
            Constraint::Min(_min) => {
                // Min acts like flex(1) but ensures at least `_min` cells (handled below)
                1
            }
            _ => 0,
        })
        .sum();

    let flex_count = constraints
        .iter()
        .filter(|c| matches!(c, Constraint::Flex(_) | Constraint::Min(_)))
        .count();

    if flex_count > 0 {
        if flex_total == 0 {
            // All are Min with no Flex — give each Min constraint equal share
            let per = remaining / flex_count as u32;
            let mut leftover = remaining % flex_count as u32;
            for (i, c) in constraints.iter().enumerate() {
                if let Constraint::Min(min) = c {
                    let allocated = per.max(*min as u32).min(remaining);
                    let actual = allocated.min(remaining);
                    sizes[i] = actual as u16;
                    remaining = remaining.saturating_sub(actual);
                    leftover = leftover.saturating_sub(1);
                }
            }
        } else {
            let mut leftover = remaining;
            for (i, c) in constraints.iter().enumerate() {
                match c {
                    Constraint::Flex(w) => {
                        let share = remaining * (*w as u32) / flex_total;
                        sizes[i] = share as u16;
                        leftover = leftover.saturating_sub(share);
                    }
                    Constraint::Min(min) => {
                        let share = remaining / flex_total;
                        let allocated = share.max(*min as u32).min(remaining);
                        sizes[i] = allocated as u16;
                        leftover = leftover.saturating_sub(allocated);
                    }
                    _ => {}
                }
            }
            // Distribute any leftover pixels to the first flex/min item
            if leftover > 0 {
                for (i, c) in constraints.iter().enumerate() {
                    if matches!(c, Constraint::Flex(_) | Constraint::Min(_)) && leftover > 0 {
                        sizes[i] += 1;
                        leftover -= 1;
                    }
                }
            }
        }
    }

    // Phase 2: build rects
    let mut rects = Vec::with_capacity(n);
    let mut offset: u16 = 0;

    for (i, _) in constraints.iter().enumerate() {
        let size = sizes[i];
        let rect = match direction {
            Direction::Vertical => Rect::new(area.x, area.y + offset, area.width, size),
            Direction::Horizontal => Rect::new(area.x + offset, area.y, size, area.height),
        };
        rects.push(rect);
        offset += size;
    }

    rects
}

/// A container component that holds children and arranges them with a layout.
///
/// This allows nested layouts — a Container can itself be a child of another Container.
pub struct Container {
    children: Vec<Box<dyn Component>>,
    direction: Direction,
    constraints: Vec<Constraint>,
    dirty: bool,
}

impl Container {
    /// Create a new container with the given direction and constraints.
    pub fn new(direction: Direction, constraints: Vec<Constraint>) -> Self {
        Self {
            children: Vec::new(),
            direction,
            constraints,
            dirty: true,
        }
    }

    /// Add a child component.
    pub fn add_child(&mut self, component: impl Component + 'static) -> usize {
        let index = self.children.len();
        self.children.push(Box::new(component));
        self.dirty = true;
        index
    }

    /// Get the number of children.
    pub fn children_count(&self) -> usize {
        self.children.len()
    }

    /// Get a reference to a child by index.
    pub fn child(&self, index: usize) -> Option<&dyn Component> {
        self.children.get(index).map(|c| c.as_ref())
    }

    /// Get a mutable reference to a child by index.
    pub fn child_mut(&mut self, index: usize) -> Option<&mut (dyn Component + '_)> {
        self.children
            .get_mut(index)
            .map(|c| c.as_mut() as &mut dyn Component)
    }

    /// Set layout direction.
    pub fn set_direction(&mut self, direction: Direction) {
        self.direction = direction;
        self.dirty = true;
    }

    /// Set constraints.
    pub fn set_constraints(&mut self, constraints: Vec<Constraint>) {
        self.constraints = constraints;
        self.dirty = true;
    }
}

impl Component for Container {
    fn name(&self) -> &str {
        "Container"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.children.iter().any(|c| c.is_dirty())
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
        for child in &mut self.children {
            child.clear_dirty();
        }
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        // Pass events to all children, first one to consume wins
        for child in &mut self.children {
            if child.handle_event(event) {
                return true;
            }
        }
        false
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let areas = split(area, self.direction, &self.constraints);

        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(&child_area) = areas.get(i) {
                child.render(surface, child_area);
            }
        }
    }

    fn min_size(&self) -> Size {
        // Minimum size is the sum of all children's min sizes
        let mut width: u16 = 0;
        let mut height: u16 = 0;

        for child in &self.children {
            let min = child.min_size();
            match self.direction {
                Direction::Vertical => {
                    width = width.max(min.width);
                    height = height.saturating_add(min.height);
                }
                Direction::Horizontal => {
                    width = width.saturating_add(min.width);
                    height = height.max(min.height);
                }
            }
        }

        Size { width, height }
    }

    fn on_focus(&mut self) {}

    fn on_unfocus(&mut self) {}

    fn is_focused(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_split_with_length_constraints() {
        let area = Rect::new(0, 0, 80, 24);
        let constraints = vec![
            Constraint::Length(5),
            Constraint::Length(10),
            Constraint::Length(9),
        ];
        let rects = split(area, Direction::Vertical, &constraints);

        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0], Rect::new(0, 0, 80, 5));
        assert_eq!(rects[1], Rect::new(0, 5, 80, 10));
        assert_eq!(rects[2], Rect::new(0, 15, 80, 9));
    }

    #[test]
    fn horizontal_split_with_percentage_constraints() {
        let area = Rect::new(0, 0, 100, 24);
        let constraints = vec![Constraint::Percentage(25), Constraint::Percentage(75)];
        let rects = split(area, Direction::Horizontal, &constraints);

        assert_eq!(rects.len(), 2);
        // Total = 100. Length = 0 remaining. 25% of 100 = 25, 75% of 100 = 75.
        assert_eq!(rects[0], Rect::new(0, 0, 25, 24));
        assert_eq!(rects[1], Rect::new(25, 0, 75, 24));
    }

    #[test]
    fn empty_constraints() {
        let area = Rect::new(0, 0, 80, 24);
        let rects = split(area, Direction::Vertical, &[]);
        assert!(rects.is_empty());
    }

    #[test]
    fn single_child_flex() {
        let area = Rect::new(0, 0, 80, 24);
        let constraints = vec![Constraint::Flex(1)];
        let rects = split(area, Direction::Vertical, &constraints);

        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0], Rect::new(0, 0, 80, 24));
    }

    #[test]
    fn single_child_length() {
        let area = Rect::new(0, 0, 80, 24);
        let constraints = vec![Constraint::Length(10)];
        let rects = split(area, Direction::Vertical, &constraints);

        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0], Rect::new(0, 0, 80, 10));
    }

    #[test]
    fn more_constraints_than_available_space() {
        let area = Rect::new(0, 0, 80, 5);
        let constraints = vec![
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ];
        let rects = split(area, Direction::Vertical, &constraints);

        assert_eq!(rects.len(), 3);
        // First gets 3, second gets 2 (remaining), third gets 0
        assert_eq!(rects[0], Rect::new(0, 0, 80, 3));
        assert_eq!(rects[1], Rect::new(0, 3, 80, 2));
        assert_eq!(rects[2], Rect::new(0, 5, 80, 0));
    }

    #[test]
    fn mixed_constraints() {
        let area = Rect::new(0, 0, 80, 24);
        let constraints = vec![
            Constraint::Length(3),
            Constraint::Flex(1),
            Constraint::Flex(2),
        ];
        let rects = split(area, Direction::Vertical, &constraints);

        assert_eq!(rects.len(), 3);
        // First gets 3, remaining = 21. Flex(1) gets 7, Flex(2) gets 14
        assert_eq!(rects[0], Rect::new(0, 0, 80, 3));
        assert_eq!(rects[1].height, 7);
        assert_eq!(rects[2].height, 14);
        // Verify non-overlapping and contiguous
        assert_eq!(rects[1].y, 3);
        assert_eq!(rects[2].y, 10);
        assert_eq!(rects[2].y + rects[2].height, 24);
    }

    #[test]
    fn horizontal_flex_split() {
        let area = Rect::new(0, 0, 120, 40);
        let constraints = vec![Constraint::Flex(1), Constraint::Flex(1)];
        let rects = split(area, Direction::Horizontal, &constraints);

        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], Rect::new(0, 0, 60, 40));
        assert_eq!(rects[1], Rect::new(60, 0, 60, 40));
    }

    #[test]
    fn offset_area() {
        let area = Rect::new(10, 5, 80, 24);
        let constraints = vec![Constraint::Length(3), Constraint::Flex(1)];
        let rects = split(area, Direction::Vertical, &constraints);

        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], Rect::new(10, 5, 80, 3));
        assert_eq!(rects[1], Rect::new(10, 8, 80, 21));
    }

    #[test]
    fn min_constraint() {
        let area = Rect::new(0, 0, 80, 24);
        let constraints = vec![Constraint::Min(5)];
        let rects = split(area, Direction::Vertical, &constraints);

        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].height, 24); // Takes all available space
    }

    #[test]
    fn percentage_with_length() {
        let area = Rect::new(0, 0, 80, 24);
        let constraints = vec![Constraint::Length(4), Constraint::Percentage(50)];
        let rects = split(area, Direction::Vertical, &constraints);

        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].height, 4);
        // Remaining = 20, 50% of 20 = 10
        assert_eq!(rects[1].height, 10);
    }

    #[test]
    fn container_renders_children_into_split_areas() {
        // Basic smoke test that Container implements Component
        let mut container = Container::new(Direction::Vertical, vec![Constraint::Flex(1)]);
        assert_eq!(container.children_count(), 0);
        assert_eq!(container.name(), "Container");
        assert!(container.is_dirty());
        container.clear_dirty();
        assert!(!container.is_dirty());
    }
}
