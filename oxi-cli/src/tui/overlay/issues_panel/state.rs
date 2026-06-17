//! State management for the issues panel — selection, refresh, scroll, dispatch.
//!
//! Owns the mutable state of the panel and provides invariants (selection in
//! range, scroll clamped, hash cache pruned). Rendering and input handling
//! live in sibling modules.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::IssuesPanelOverlay;
use crate::store::issues::{IssueFilter, Priority, Status};

/// How often the overlay re-scans the on-disk store. Cheap (the store's
/// in-memory cache short-circuits the directory mtime unchanged).
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// Page size for PgUp/PgDn (rounded from visible height by the caller).
pub(crate) const PAGE_STEP: usize = 10;

/// Maximum interval between two mouse clicks to count as a double-click.
/// Matches the typical desktop threshold (≈500ms).
pub(super) const DOUBLE_CLICK_MS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    Open,
    All,
}

/// Optional UI event sender used to publish notifications / system messages
/// from in-overlay mutations (s/r/c keys).
#[derive(Clone, Default)]
pub struct UiTx(pub Option<tokio::sync::mpsc::UnboundedSender<crate::tui::app::UiEvent>>);

impl IssuesPanelOverlay {
    /// Re-scan the store. Prunes stale `hash_cache` entries (those whose
    /// issue id is no longer in the current filter), normalizes the
    /// selection (clamp to range or reset to 0), clamps `detail_scroll`
    /// against the new body length, and clears `action_in_flight` (the
    /// next refresh after a mutation task writes will see the new mtime).
    pub(super) fn refresh(&mut self) {
        let status = match self.status_filter {
            StatusFilter::Open => Some(Status::Open),
            StatusFilter::All => None,
        };
        let filter = IssueFilter {
            status,
            priority: self.custom_filter.as_ref().and_then(|f| f.priority),
            label: self.custom_filter.as_ref().and_then(|f| f.label.clone()),
            assigned_to_session: None,
            text: self.custom_filter.as_ref().and_then(|f| f.text.clone()),
        };
        self.items = self.store.list(&filter).unwrap_or_default();
        let live_ids: HashSet<u32> = self.items.iter().map(|i| i.meta.id).collect();
        self.hash_cache.retain(|id, _| live_ids.contains(id));
        let len = self.items.len();
        let cur = self.list_state.selected();
        if len == 0 {
            self.list_state.select(None);
        } else if cur.is_none_or(|s| s >= len) {
            self.list_state.select(Some(0));
        }
        self.clamp_detail_scroll();
        self.action_in_flight = false;
        self.last_refresh = Instant::now();
    }

    /// Parse user-typed filter syntax into an `IssueFilter` overlay.
    /// Accepted tokens (separated by whitespace):
    /// - `priority=low|medium|high|critical`
    /// - `label=<name>`
    /// - `text=<substring>` (matches title case-insensitively)
    /// - `status=open|closed|all` (overrides the panel's status filter)
    ///
    /// Unknown tokens are ignored (forward-compat). Returns `None` for an
    /// empty input — caller should treat that as "clear custom filter".
    pub(super) fn parse_filter_input(input: &str) -> Option<crate::store::issues::IssueFilter> {
        let mut filter = crate::store::issues::IssueFilter::default();
        let mut any = false;
        for tok in input.split_whitespace() {
            let Some((k, v)) = tok.split_once('=') else {
                continue;
            };
            match k.trim() {
                "priority" => {
                    if let Some(p) = Self::parse_priority_loose(v.trim()) {
                        filter.priority = Some(p);
                        any = true;
                    }
                }
                "label" => {
                    let l = v.trim();
                    if !l.is_empty() {
                        filter.label = Some(l.to_string());
                        any = true;
                    }
                }
                "text" => {
                    let t = v.trim();
                    if !t.is_empty() {
                        filter.text = Some(t.to_string());
                        any = true;
                    }
                }
                "status" => match v.trim().to_lowercase().as_str() {
                    "open" => {
                        filter.status = Some(Status::Open);
                        any = true;
                    }
                    "closed" => {
                        filter.status = Some(Status::Closed);
                        any = true;
                    }
                    "all" => {
                        filter.status = None;
                        any = true;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if any { Some(filter) } else { None }
    }

    /// Loose priority parser — accepts short forms (`h`, `crit`) and is
    /// case-insensitive. Used by `parse_filter_input`. Same semantics as
    /// the slash-command helper in `tui/slash.rs`.
    fn parse_priority_loose(s: &str) -> Option<Priority> {
        match s.to_lowercase().as_str() {
            "low" | "l" => Some(Priority::Low),
            "medium" | "med" | "m" | "default" => Some(Priority::Medium),
            "high" | "h" => Some(Priority::High),
            "critical" | "crit" | "c" | "urgent" => Some(Priority::Critical),
            _ => None,
        }
    }

    /// Resolve the `content_hash` for the currently selected issue.
    /// Reads from the on-disk store at most once per (issue, refresh cycle)
    /// — the result is cached in `self.hash_cache` until the next refresh.
    pub(super) fn current_hash(&mut self) -> String {
        let Some(issue) = self.selected().cloned() else {
            return String::new();
        };
        if let Some(h) = self.hash_cache.get(&issue.meta.id) {
            return h.clone();
        }
        let hash = self
            .store
            .read(issue.meta.id)
            .map(|(_, h)| h)
            .unwrap_or_else(|_| String::from("<hash-unavailable>"));
        self.hash_cache.insert(issue.meta.id, hash.clone());
        hash
    }

    pub(super) fn selected(&self) -> Option<&crate::store::issues::Issue> {
        self.list_state.selected().and_then(|i| self.items.get(i))
    }

    pub(super) fn selected_position_label(&self) -> String {
        match (self.list_state.selected(), self.items.len()) {
            (Some(i), n) if n > 0 => format!("{} of {}", i + 1, n),
            _ => format!("0 of {}", self.items.len()),
        }
    }

    pub(super) fn filter_label(&self) -> &'static str {
        match self.status_filter {
            StatusFilter::Open => "open",
            StatusFilter::All => "all",
        }
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        let len = self.items.len() as isize;
        let next = (cur as isize + delta).rem_euclid(len) as usize;
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
    }

    pub(super) fn jump_first(&mut self) {
        if !self.items.is_empty() {
            self.list_state.select(Some(0));
            self.detail_scroll = 0;
        }
    }

    pub(super) fn jump_last(&mut self) {
        if !self.items.is_empty() {
            self.list_state.select(Some(self.items.len() - 1));
            self.detail_scroll = 0;
        }
    }

    pub(super) fn page_selection(&mut self, delta_pages: isize) {
        let step = (PAGE_STEP as isize).max(1);
        self.move_selection(delta_pages * step);
    }

    /// Clamp `detail_scroll` so it can't exceed the (total - visible)
    /// max. Uses `total_wrapped_rows` if set by the most recent render
    /// (which accounts for hard-wrap), otherwise falls back to source
    /// line count. Safe to call after a refresh or before the first
    /// render.
    pub(super) fn clamp_detail_scroll(&mut self) {
        let total = if self.total_wrapped_rows > 0 {
            self.total_wrapped_rows
        } else {
            self.detail_body_lines()
        };
        let visible = self.detail_visible.max(1);
        let max = total.saturating_sub(visible);
        if self.detail_scroll > max {
            self.detail_scroll = max;
        }
    }

    /// Number of source body lines for the currently selected issue.
    /// The actual displayed row count is `total_wrapped_rows` (set during
    /// render after hard-wrap). This is kept as a fallback for the period
    /// between issue-selection and first render.
    pub(super) fn detail_body_lines(&self) -> usize {
        self.selected().map(|i| i.body.lines().count()).unwrap_or(0)
    }

    /// Convert a mouse Y row to a list index by reversing the render order.
    /// Returns `None` if the click was outside the list area.
    pub(super) fn list_hit_test(&self, row: u16) -> Option<usize> {
        if self.last_inner.width == 0 || self.last_inner.height == 0 {
            return None;
        }
        let inner_top = self.last_inner.y;
        let inner_bottom = self.last_inner.y + self.last_inner.height;
        if row < inner_top || row >= inner_bottom {
            return None;
        }
        let offset = (row - inner_top) as usize;
        let base = self.list_state.offset();
        let idx = base + offset;
        if idx < self.items.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// True iff `idx` was the most recently clicked row AND the click
    /// happened within `DOUBLE_CLICK_MS`. Resets the click state.
    pub(super) fn is_double_click(&mut self, idx: usize) -> bool {
        let Some(prev_idx) = self.last_click_idx else {
            return false;
        };
        let Some(prev_at) = self.last_click_at else {
            return false;
        };
        let within_window = prev_at.elapsed().as_millis() <= DOUBLE_CLICK_MS as u128;
        let same_row = prev_idx == idx;
        if within_window && same_row {
            self.last_click_idx = None;
            self.last_click_at = None;
            true
        } else {
            false
        }
    }

    /// Record a single click for double-click detection. Always called
    /// after `is_double_click` returns false (i.e. the first click of a
    /// potential double-click).
    pub(super) fn record_click(&mut self, idx: usize) {
        self.last_click_idx = Some(idx);
        self.last_click_at = Some(Instant::now());
    }

    /// Issue an async mutation through the store, surfacing results via
    /// `ui_tx` if set. The closure is responsible for calling the store
    /// method and forwarding the result as a `UiEvent::SystemMessage`.
    /// Sets `action_in_flight = true` and resets it on the next `refresh()`
    /// (the TUI loop calls `tick()` every frame; after the spawned task
    /// completes, the next refresh sees the on-disk effect and clears the
    /// flag). Prevents rapid s/r/c keypresses from racing on the same file.
    pub(super) fn dispatch_action<F, Fut>(&mut self, _label: &'static str, f: F)
    where
        F: FnOnce(
                crate::store::issues::FileIssueStore,
                u32,
                tokio::sync::mpsc::UnboundedSender<crate::tui::app::UiEvent>,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        if self.action_in_flight {
            return;
        }
        let Some(issue) = self.selected().cloned() else {
            return;
        };
        let Some(tx) = self.ui_tx.0.clone() else {
            return;
        };
        let store = (*self.store).clone();
        let id = issue.meta.id;
        self.action_in_flight = true;
        tokio::spawn(f(store, id, tx));
    }

    /// Prune `hash_cache` of entries no longer present in `items`.
    /// Extracted so it can be tested without constructing a full overlay.
    #[cfg(test)]
    pub(super) fn prune_hash_cache_for_test(&mut self, live_ids: &HashSet<u32>) -> usize {
        let before = self.hash_cache.len();
        self.hash_cache.retain(|id, _| live_ids.contains(id));
        before - self.hash_cache.len()
    }
}
