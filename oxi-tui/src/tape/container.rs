//! Container — composition of child Components.
//!
//! Unlike omp's TypeScript Container (which uses reference identity for
//! memoization), the Rust Container does NOT cache. The engine handles
//! memoization by comparing content hashes between frames. This avoids
//! `RefCell` in the hot path.

use super::component::{Component, LiveRegion, RenderResult};

/// A container that composes child components into a flat line array.
pub struct Container {
    children: Vec<Box<dyn Component>>,
}

impl Container {
    /// Create an empty container.
    #[must_use]
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }

    /// Create a container with pre-built children.
    #[must_use]
    pub fn with_children(children: Vec<Box<dyn Component>>) -> Self {
        Self { children }
    }

    /// Add a child component.
    pub fn add(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }

    /// Remove all children.
    pub fn clear(&mut self) {
        self.children.clear();
    }

    /// Number of children.
    #[must_use]
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Whether the container has no children.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Compose all children into a single `RenderResult` + `LiveRegion`.
    ///
    /// This is the primary method — it renders each child, concatenates
    /// the lines, and computes the aggregate live region (topmost child
    /// with a live region wins).
    pub fn compose(&self, width: u16) -> (RenderResult, LiveRegion) {
        let mut lines = Vec::new();
        let mut child_line_counts: Vec<usize> = Vec::with_capacity(self.children.len());
        let mut aggregate_live = LiveRegion::None;
        let mut offset = 0usize;

        for child in &self.children {
            let result = child.render(width);
            let count = result.lines.len();
            child_line_counts.push(count);

            // First child with a live region wins (omp: "topmost defines boundary")
            if matches!(aggregate_live, LiveRegion::None) {
                match child.live_region() {
                    LiveRegion::None => {}
                    LiveRegion::Mutable { start } => {
                        aggregate_live = LiveRegion::Mutable {
                            start: offset + start,
                        };
                    }
                    LiveRegion::Pinned { start } => {
                        aggregate_live = LiveRegion::Pinned {
                            start: offset + start,
                        };
                    }
                }
            }

            lines.extend(result.lines);
            offset += count;
        }

        (RenderResult::new(lines), aggregate_live)
    }

    /// Invalidate all children.
    pub fn invalidate_all(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tape::component::RenderResult;

    struct StaticText {
        text: String,
        live: LiveRegion,
    }

    impl Component for StaticText {
        fn render(&self, _width: u16) -> RenderResult {
            RenderResult::new(vec![self.text.clone()])
        }
        fn live_region(&self) -> LiveRegion {
            self.live
        }
    }

    #[test]
    fn compose_concatenates_children() {
        let mut c = Container::new();
        c.add(Box::new(StaticText {
            text: "a".into(),
            live: LiveRegion::None,
        }));
        c.add(Box::new(StaticText {
            text: "b".into(),
            live: LiveRegion::None,
        }));
        let (result, live) = c.compose(80);
        assert_eq!(result.lines, vec!["a", "b"]);
        assert_eq!(live, LiveRegion::None);
    }

    #[test]
    fn compose_live_region_from_first_child_with_one() {
        let mut c = Container::new();
        c.add(Box::new(StaticText {
            text: "final".into(),
            live: LiveRegion::None,
        }));
        c.add(Box::new(StaticText {
            text: "live".into(),
            live: LiveRegion::Mutable { start: 0 },
        }));
        let (result, live) = c.compose(80);
        assert_eq!(result.lines, vec!["final", "live"]);
        // "final" is 1 line, so live starts at offset 1
        assert_eq!(live, LiveRegion::Mutable { start: 1 });
    }

    #[test]
    fn compose_empty() {
        let c = Container::new();
        let (result, live) = c.compose(80);
        assert!(result.lines.is_empty());
        assert_eq!(live, LiveRegion::None);
    }

    #[test]
    fn compose_pinned_live_region() {
        let mut c = Container::new();
        c.add(Box::new(StaticText {
            text: "dash".into(),
            live: LiveRegion::Pinned { start: 0 },
        }));
        let (_, live) = c.compose(80);
        assert_eq!(live, LiveRegion::Pinned { start: 0 });
    }
}
