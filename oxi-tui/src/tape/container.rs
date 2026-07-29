//! Container — composition of child Components.
//!
//! Unlike omp's TypeScript Container (which uses reference identity for
//! memoization), the Rust Container does NOT cache. The engine handles
//! memoization by comparing content hashes between frames. This avoids
//! `RefCell` in the hot path.

use super::component::{Component, LiveRegion, RenderResult};

#[derive(Clone)]
struct ChildCache {
    revision: u64,
    width: u16,
    result: RenderResult,
    live_region: LiveRegion,
}

/// A container that composes child components into a flat line array.
pub struct Container {
    children: Vec<Box<dyn Component>>,
    child_cache: Vec<Option<ChildCache>>,
    aggregate: RenderResult,
    aggregate_live: LiveRegion,
    aggregate_dirty: bool,
}

impl Container {
    /// Create an empty container.
    #[must_use]
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            child_cache: Vec::new(),
            aggregate: RenderResult::empty(),
            aggregate_live: LiveRegion::None,
            aggregate_dirty: false,
        }
    }

    /// Create a container with pre-built children.
    #[must_use]
    pub fn with_children(children: Vec<Box<dyn Component>>) -> Self {
        let child_cache = (0..children.len()).map(|_| None).collect();
        Self {
            children,
            child_cache,
            aggregate: RenderResult::empty(),
            aggregate_live: LiveRegion::None,
            aggregate_dirty: true,
        }
    }

    /// Add a child component.
    pub fn add(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
        self.child_cache.push(None);
        self.aggregate_dirty = true;
    }

    /// Remove all children.
    pub fn clear(&mut self) {
        self.children.clear();
        self.child_cache.clear();
        self.aggregate = RenderResult::empty();
        self.aggregate_live = LiveRegion::None;
        self.aggregate_dirty = false;
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

    /// Compose all children, returning the memoized aggregate result.
    ///
    /// Child renders are keyed by their O(1) revision and the terminal width.
    /// A cache hit neither calls `render` nor clones the aggregate rows.
    pub fn compose(&mut self, width: u16) -> (&RenderResult, LiveRegion) {
        for (index, child) in self.children.iter().enumerate() {
            let revision = child.revision();
            let hit = self.child_cache[index]
                .as_ref()
                .is_some_and(|entry| entry.revision == revision && entry.width == width);
            if hit {
                continue;
            }

            self.child_cache[index] = Some(ChildCache {
                revision,
                width,
                result: child.render(width),
                live_region: child.live_region(),
            });
            self.aggregate_dirty = true;
        }

        if self.aggregate_dirty {
            let line_count = self
                .child_cache
                .iter()
                .flatten()
                .map(|entry| entry.result.lines.len())
                .sum();
            let mut lines = Vec::with_capacity(line_count);
            let mut live_region = LiveRegion::None;
            let mut offset = 0usize;

            for entry in self.child_cache.iter().flatten() {
                if matches!(live_region, LiveRegion::None) {
                    live_region = match entry.live_region {
                        LiveRegion::None => LiveRegion::None,
                        LiveRegion::Mutable { start } => LiveRegion::Mutable {
                            start: offset + start,
                        },
                        LiveRegion::Pinned { start } => LiveRegion::Pinned {
                            start: offset + start,
                        },
                    };
                }
                lines.extend(entry.result.lines.iter().cloned());
                offset += entry.result.lines.len();
            }

            self.aggregate = RenderResult::new(lines);
            self.aggregate_live = live_region;
            self.aggregate_dirty = false;
        }

        (&self.aggregate, self.aggregate_live)
    }

    /// Invalidate all children.
    pub fn invalidate_all(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
        self.child_cache.iter_mut().for_each(|entry| *entry = None);
        self.aggregate_dirty = true;
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
    use std::sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    };

    struct CountingComponent {
        renders: Arc<AtomicU32>,
        revision: Arc<AtomicU64>,
        live: LiveRegion,
    }

    impl Component for CountingComponent {
        fn render(&self, width: u16) -> RenderResult {
            self.renders.fetch_add(1, Ordering::Relaxed);
            RenderResult::one(format!("rendered-{width}"))
        }

        fn revision(&self) -> u64 {
            self.revision.load(Ordering::Relaxed)
        }

        fn live_region(&self) -> LiveRegion {
            self.live
        }
    }

    fn counting(live: LiveRegion) -> (Box<dyn Component>, Arc<AtomicU32>, Arc<AtomicU64>) {
        let renders = Arc::new(AtomicU32::new(0));
        let revision = Arc::new(AtomicU64::new(1));
        (
            Box::new(CountingComponent {
                renders: Arc::clone(&renders),
                revision: Arc::clone(&revision),
                live,
            }),
            renders,
            revision,
        )
    }

    #[test]
    fn compose_caches_by_revision_and_width() {
        let (child, renders, revision) = counting(LiveRegion::Mutable { start: 0 });
        let mut container = Container::with_children(vec![child]);

        let (first, first_live) = container.compose(80);
        assert_eq!(first.lines, vec!["rendered-80"]);
        assert_eq!(first_live, LiveRegion::Mutable { start: 0 });
        assert_eq!(renders.load(Ordering::Relaxed), 1);

        let _ = container.compose(80);
        assert_eq!(
            renders.load(Ordering::Relaxed),
            1,
            "cache hit rendered child"
        );

        let _ = container.compose(100);
        assert_eq!(
            renders.load(Ordering::Relaxed),
            2,
            "width change missed cache"
        );

        revision.fetch_add(1, Ordering::Relaxed);
        let _ = container.compose(100);
        assert_eq!(
            renders.load(Ordering::Relaxed),
            3,
            "revision change missed cache"
        );
    }

    #[test]
    fn cached_live_region_keeps_child_offset() {
        let (child, renders, _) = counting(LiveRegion::Pinned { start: 0 });
        let mut container = Container::new();
        container.add(Box::new(StaticText {
            text: "final".into(),
            live: LiveRegion::None,
        }));
        container.add(child);

        assert_eq!(container.compose(80).1, LiveRegion::Pinned { start: 1 });
        assert_eq!(container.compose(80).1, LiveRegion::Pinned { start: 1 });
        assert_eq!(renders.load(Ordering::Relaxed), 1);
    }

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
        let mut c = Container::new();
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
