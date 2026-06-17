//! Local issues panel — list / detail, modeled as an [`OverlayComponent`].
//!
//! Built on the standard ratatui widget set: `Block`, `List` + `ListState`,
//! `Table` + `Cell`, `Paragraph` + `Wrap`, `Scrollbar` + `ScrollbarState`,
//! `Layout` + `Constraint`, `Clear`, `Stylize`. Multi-pane layout, visible
//! scrollbars, position indicator, full keyboard + mouse navigation.
//!
//! **Mutations** are available both via slash commands (`/issue start 12`)
//! and inline keys (`s`/`r`/`c`) in either list or detail view. The panel
//! holds a cached `content_hash` per issue so CAS conflicts surface cleanly.
//!
//! Keys (list):  `j`/`↓` next · `k`/`↑` prev · `Enter`/`l` detail ·
//!               `g`/`Home` first · `G`/`End` last · `PgUp`/`PgDn` page ·
//!               `f` toggle status filter · `s` start · `r` release ·
//!               `c` close · `R` reload · `Esc`/`q` close
//! Keys (detail): `Esc`/`h`/`q` back · `j`/`k` next/prev issue ·
//!                `J`/`K` scroll body · `g`/`G` top/bottom ·
//!                `s` start · `r` release · `c` close
//!
//! Mouse: click a list row to select it; Enter/`l`/`→` to open detail;
//! scroll wheel to navigate.
//!
//! # Module split
//!
//! - [`state`] — selection, refresh, scroll clamp, action dispatch,
//!   content-hash cache. The heart of the panel.
//! - [`render`] — `render_list` + `render_detail` + styling helpers
//!   (`build_metadata_rows`, `build_list_title`, etc.).
//! - [`input`] — `handle_key` / `handle_mouse` and the per-view dispatch.

mod input;
mod render;
mod state;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use ratatui::{layout::Rect, widgets::ListState};

use crate::store::issues::{FileIssueStore, Issue};

pub(super) use state::{REFRESH_INTERVAL, StatusFilter, UiTx, View};

/// Maximum body lines to render in detail view. Capped to avoid runaway
/// `Vec<Line>` allocations on huge bodies. Scrolling handles overflow.
#[allow(dead_code)]
const MAX_BODY_RENDER_LINES: usize = 2000;

pub struct IssuesPanelOverlay {
    pub(super) store: Arc<FileIssueStore>,
    pub(super) view: View,
    pub(super) items: Vec<Issue>,
    pub(super) list_state: ListState,
    /// Vertical scroll offset for the detail body (lines, not pixels).
    pub(super) detail_scroll: usize,
    /// Last computed visible body height — used as PageUp/PageDown step.
    pub(super) detail_visible: usize,
    /// Total display rows after hard-wrap. Set by `render_detail` and used
    /// by `clamp_detail_scroll` to keep scroll position in bounds when
    /// long body lines wrap across multiple rows.
    pub(super) total_wrapped_rows: usize,
    pub(super) status_filter: StatusFilter,
    /// Optional custom filter overlay applied on top of `status_filter`.
    /// Set by the filter-input modal (triggered by `/` in list view).
    pub(super) custom_filter: Option<crate::store::issues::IssueFilter>,
    pub(super) last_refresh: Instant,
    pub(super) ui_tx: UiTx,
    /// Cached `content_hash` per issue id, populated lazily on detail view.
    /// Keyed by issue id so a list → detail → list round-trip doesn't
    /// trigger another `store.read()` for the same file. Invalidated on
    /// every `refresh()` (cheap — bounded by `items.len()`).
    pub(super) hash_cache: HashMap<u32, String>,
    /// True while an s/r/c task is in flight, so we don't fire concurrent
    /// mutations on the same issue. Resets on refresh (covers the case where
    /// the task completed between frames).
    pub(super) action_in_flight: bool,
    /// Last render frame area — used by mouse hit-testing.
    pub(super) last_inner: Rect,
    /// Mouse double-click state. `last_click_idx` + `last_click_at` track
    /// the most recent click; a second click on the same row within
    /// `DOUBLE_CLICK_MS` triggers an open-detail action.
    pub(super) last_click_idx: Option<usize>,
    pub(super) last_click_at: Option<Instant>,
    /// True while the `/`-triggered filter input bar is active. When true,
    /// `handle_key_list` intercepts all printable keys for text editing and
    /// `render_list` shows the input bar at the bottom.
    pub(super) filter_input_mode: bool,
    /// Current text in the filter input bar. Edited character-by-character.
    pub(super) filter_input_text: String,
}

impl std::fmt::Debug for IssuesPanelOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuesPanelOverlay")
            .field("view", &self.view)
            .field("items", &self.items.len())
            .field("status_filter", &self.status_filter)
            .field("detail_scroll", &self.detail_scroll)
            .field(
                "last_refresh_secs_ago",
                &self.last_refresh.elapsed().as_secs(),
            )
            .finish()
    }
}

impl IssuesPanelOverlay {
    pub fn new(store: FileIssueStore) -> Self {
        let s = Arc::new(store);
        let mut me = Self {
            store: s,
            view: View::List,
            items: Vec::new(),
            list_state: ListState::default(),
            detail_scroll: 0,
            detail_visible: render::DETAIL_BODY_LINES_HINT,
            total_wrapped_rows: 0,
            status_filter: StatusFilter::Open,
            custom_filter: None,
            last_refresh: Instant::now() - REFRESH_INTERVAL, // force initial refresh
            ui_tx: UiTx::default(),
            hash_cache: HashMap::new(),
            action_in_flight: false,
            last_inner: Rect::default(),
            last_click_idx: None,
            last_click_at: None,
            filter_input_mode: false,
            filter_input_text: String::new(),
        };
        me.refresh();
        me
    }

    /// Wire a UI event sender so in-overlay mutations (s/r/c keys) can route
    /// notifications back to the TUI without a thread-local indirection.
    /// Optional — when not set, mutations still complete, but no toast fires.
    pub fn set_ui_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<crate::tui::app::UiEvent>) {
        self.ui_tx = UiTx(Some(tx));
    }

    /// Select a specific issue by id, switching to detail view. Returns
    /// `false` if the id doesn't exist in the current filter (the caller can
    /// surface a warning).
    pub fn pre_select(&mut self, id: u32) -> bool {
        self.refresh();
        if let Some(pos) = self.items.iter().position(|i| i.meta.id == id) {
            self.list_state.select(Some(pos));
            self.detail_scroll = 0;
            self.view = View::Detail;
            true
        } else {
            false
        }
    }

    /// Periodically called by the TUI render loop to pick up external changes
    /// (e.g., another `oxi issue` CLI invocation). Idempotent — only does
    /// real work once per `REFRESH_INTERVAL`.
    pub fn tick(&mut self) {
        if self.last_refresh.elapsed() >= REFRESH_INTERVAL {
            self.refresh();
        }
    }

    /// Synthetic session id used by the TUI overlay for **ownership** purposes.
    ///
    /// See `handle_issue_command` in `tui/slash.rs` for the matching
    /// slash-command path, and `link_sessions_async` in `tui/handlers.rs`
    /// for the inline `@issue-N` reference path.
    ///
    /// Returns the canonical TUI liveness identity. In TUI mode this is
    /// the same string that `App::ownership_session_id` uses (and that the
    /// agent's `ToolContext.session_id` is set to), so the agent tool,
    /// the panel, and the `/issue` slash command all see one consistent
    /// flock holder. See [`crate::store::issues::liveness::TUI_OWNERSHIP_ID`].
    pub fn session_id() -> &'static str {
        crate::store::issues::liveness::TUI_OWNERSHIP_ID
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::issues::{FileIssueStore, Priority, Status};
    use crate::tui::overlay::OverlayComponent;

    fn tmp_store() -> (tempfile::TempDir, FileIssueStore) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".oxi").join("issues");
        std::fs::create_dir_all(&dir).unwrap();
        let store = FileIssueStore::open(dir).unwrap();
        (tmp, store)
    }

    #[tokio::test]
    async fn pre_select_finds_existing_id_and_switches_to_detail() {
        let (_tmp, store) = tmp_store();
        store
            .create("First".into(), "".into(), Priority::Medium, vec![], None)
            .unwrap();
        store
            .create("Second".into(), "".into(), Priority::High, vec![], None)
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store);
        let initial = panel.list_state.selected();
        assert!(initial.is_some());
        assert!(panel.pre_select(2));
        assert_eq!(panel.view, View::Detail);
        let sel = panel.list_state.selected().unwrap();
        assert_eq!(panel.items[sel].meta.id, 2);
    }

    #[tokio::test]
    async fn pre_select_returns_false_for_missing_id() {
        let (_tmp, store) = tmp_store();
        store
            .create("Only".into(), "".into(), Priority::Low, vec![], None)
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store);
        let ok = panel.pre_select(99);
        assert!(!ok, "pre_select should return false for unknown id");
        assert_eq!(panel.view, View::List);
    }

    #[test]
    fn session_id_is_tui_for_ownership() {
        assert_eq!(IssuesPanelOverlay::session_id(), "tui");
    }

    #[tokio::test]
    async fn refresh_recomputes_cache_after_create() {
        let (_tmp, store) = tmp_store();
        store
            .create("A".into(), "".into(), Priority::Low, vec![], None)
            .unwrap();
        let panel = IssuesPanelOverlay::new(store.clone());
        assert_eq!(panel.items.len(), 1);
        store
            .create("B".into(), "".into(), Priority::High, vec![], None)
            .unwrap();
        let mut panel2 = IssuesPanelOverlay::new(store.clone());
        assert_eq!(panel2.items.len(), 2);
        panel2.refresh();
        assert_eq!(panel2.items.len(), 2);
    }

    #[tokio::test]
    async fn hint_strings_mention_new_keys() {
        let (_tmp, store) = tmp_store();
        let mut panel = IssuesPanelOverlay::new(store);
        let list_hint = panel.hint().to_string();
        assert!(
            list_hint.contains("s/r/c:act"),
            "list hint missing: {list_hint}"
        );
        assert!(list_hint.contains("Esc:back") || list_hint.contains("q:close"));
        assert!(list_hint.contains("/:filter"), "list hint missing /:filter");
        panel.view = View::Detail;
        let detail_hint = panel.hint().to_string();
        assert!(
            detail_hint.contains("Esc:back"),
            "detail hint missing: {detail_hint}"
        );
        assert!(
            detail_hint.contains("J/K:scroll"),
            "detail hint missing scroll: {detail_hint}"
        );
    }

    #[tokio::test]
    async fn page_navigation_wraps_and_clamps() {
        let (_tmp, store) = tmp_store();
        for i in 0..15 {
            store
                .create(
                    format!("Issue {i}"),
                    "".into(),
                    Priority::Medium,
                    vec![],
                    None,
                )
                .unwrap();
        }
        let mut panel = IssuesPanelOverlay::new(store);
        assert_eq!(panel.items.len(), 15);
        panel.jump_first();
        assert_eq!(panel.list_state.selected(), Some(0));
        panel.page_selection(1);
        assert_eq!(panel.list_state.selected(), Some(10));
        panel.page_selection(-1);
        assert_eq!(panel.list_state.selected(), Some(0));
        panel.jump_last();
        assert_eq!(panel.list_state.selected(), Some(14));
    }

    #[tokio::test]
    async fn detail_scroll_clamps_against_body_length() {
        let (_tmp, store) = tmp_store();
        let body = (0..50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        store
            .create("Scrollable".into(), body, Priority::High, vec![], None)
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store);
        panel.detail_visible = 5;
        assert_eq!(panel.detail_scroll, 0);
        panel.detail_scroll = 999;
        panel.clamp_detail_scroll();
        assert_eq!(panel.detail_scroll, 45);
    }

    #[tokio::test]
    async fn status_filter_toggles_between_open_and_all() {
        let (_tmp, store) = tmp_store();
        store
            .create("Open one".into(), "".into(), Priority::Low, vec![], None)
            .unwrap();
        store
            .create("Will close".into(), "".into(), Priority::Low, vec![], None)
            .unwrap();
        let issues_dir = store.issues_dir();
        let _guard = crate::store::issues::liveness::acquire(&issues_dir, "tui").unwrap();
        let (_, h) = store.read(2).unwrap();
        store.start(2, "tui", Some(h)).await.unwrap();
        let (_, h) = store.read(2).unwrap();
        store.close(2, "tui", Some(h)).await.unwrap();
        drop(_guard);

        let mut panel = IssuesPanelOverlay::new(store);
        assert_eq!(panel.items.len(), 1);
        panel.status_filter = StatusFilter::All;
        panel.refresh();
        assert_eq!(panel.items.len(), 2);
    }

    // ── Buffer-based render regression tests ─────────────────────────────
    // Catch Layout-panic regressions on small terminals and verify that
    // position indicators land in the rendered frame.

    use oxi_tui::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Render `panel` into a `TestBackend` of the given size and return the
    /// accumulated text. Catches Layout-panic regressions on small terminals
    /// and lets us assert on rendered title content.
    fn render_to_text(panel: &mut IssuesPanelOverlay, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                panel.render(frame, area, &Theme::dark());
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect()
    }

    #[tokio::test]
    async fn render_list_does_not_panic_on_small_terminal() {
        let (_tmp, store) = tmp_store();
        let mut panel = IssuesPanelOverlay::new(store);
        // Pathological terminal sizes — these used to panic with Min(3).
        for (w, h) in [(40, 4), (40, 5), (40, 8), (20, 4)] {
            let _ = render_to_text(&mut panel, w, h);
        }
    }

    #[tokio::test]
    async fn render_detail_does_not_panic_on_small_terminal() {
        let (_tmp, store) = tmp_store();
        store
            .create("Test".into(), "body".into(), Priority::Low, vec![], None)
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store);
        panel.pre_select(1);
        for (w, h) in [(40, 4), (40, 5), (40, 10), (20, 6)] {
            let _ = render_to_text(&mut panel, w, h);
        }
    }

    #[tokio::test]
    async fn list_view_shows_count_in_title() {
        let (_tmp, store) = tmp_store();
        for i in 0..3 {
            store
                .create(format!("Issue {i}"), "".into(), Priority::Low, vec![], None)
                .unwrap();
        }
        let mut panel = IssuesPanelOverlay::new(store);
        let text = render_to_text(&mut panel, 80, 20);
        assert!(text.contains("(3)"), "list title missing count");
        assert!(text.contains("Issues"));
    }

    #[tokio::test]
    async fn detail_view_shows_issue_metadata() {
        let (_tmp, store) = tmp_store();
        store
            .create(
                "Test detail".into(),
                "first line\nsecond line".into(),
                Priority::High,
                vec!["bug".into()],
                None,
            )
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store);
        panel.pre_select(1);
        let text = render_to_text(&mut panel, 100, 24);
        assert!(text.contains("status"));
        assert!(text.contains("priority"));
        assert!(text.contains("high"));
        assert!(text.contains("bug"));
    }

    #[tokio::test]
    async fn hash_cache_avoids_repeated_disk_reads() {
        let (_tmp, store) = tmp_store();
        store
            .create(
                "Hash test".into(),
                "body".into(),
                Priority::Low,
                vec![],
                None,
            )
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store);
        panel.pre_select(1);
        // Render once to populate the cache.
        let _ = render_to_text(&mut panel, 80, 20);
        assert!(panel.hash_cache.contains_key(&1));
        let cached = panel.hash_cache.get(&1).cloned().unwrap();
        assert!(!cached.is_empty());
    }

    #[tokio::test]
    async fn hash_cache_pruned_after_refresh() {
        let (_tmp, store) = tmp_store();
        store
            .create("A".into(), "".into(), Priority::Low, vec![], None)
            .unwrap();
        store
            .create("B".into(), "".into(), Priority::Low, vec![], None)
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store.clone());
        let _ = render_to_text(&mut panel, 80, 20);
        panel.pre_select(1);
        let _ = render_to_text(&mut panel, 80, 20);
        panel.pre_select(2);
        let _ = render_to_text(&mut panel, 80, 20);
        assert!(panel.hash_cache.len() >= 1);
        // Delete one issue externally — refresh should prune its hash.
        store.invalidate();
        let path = store
            .list(&crate::store::issues::IssueFilter::default())
            .unwrap()
            .first()
            .map(|i| i.path.clone().unwrap());
        if let Some(p) = path {
            std::fs::remove_file(p).ok();
        }
        panel.refresh();
        let live_ids: std::collections::HashSet<u32> =
            panel.items.iter().map(|i| i.meta.id).collect();
        for cached_id in panel.hash_cache.keys() {
            assert!(
                live_ids.contains(cached_id),
                "stale hash entry for {cached_id}"
            );
        }
    }

    #[test]
    fn double_click_detection_works() {
        let (_tmp, store) = tmp_store();
        let mut panel = IssuesPanelOverlay::new(store);
        // First click on row 3 — not a double-click yet.
        assert!(!panel.is_double_click(3));
        panel.record_click(3);
        // Same row immediately → double-click.
        assert!(panel.is_double_click(3));
        // After the double-click resolves, state is reset — next click
        // starts a new sequence.
        assert!(!panel.is_double_click(3));
        panel.record_click(3);
        // Different row → not a double-click.
        assert!(!panel.is_double_click(5));
    }

    #[test]
    fn detail_body_renders_markdown_headers() {
        let (_tmp, store) = tmp_store();
        store
            .create(
                "Doc".into(),
                "# Heading\n\nlist items: alpha and beta\n\n`code` and **bold**".into(),
                Priority::Low,
                vec![],
                None,
            )
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store);
        panel.pre_select(1);
        let text = render_to_text(&mut panel, 100, 30);
        // The body content must appear (markdown rendering may transform
        // `# Heading` and `- item` markers, so we only check that the
        // payload text survives — not its exact format).
        assert!(text.contains("Heading"), "heading text missing: {text}");
        assert!(text.contains("alpha"));
        assert!(text.contains("beta"));
    }

    #[test]
    fn detail_body_handles_code_fence() {
        let (_tmp, store) = tmp_store();
        store
            .create(
                "Code".into(),
                "```rust\nfn main() {}\n```\n".into(),
                Priority::Low,
                vec![],
                None,
            )
            .unwrap();
        let mut panel = IssuesPanelOverlay::new(store);
        panel.pre_select(1);
        // Should not panic on fenced code.
        let _text = render_to_text(&mut panel, 100, 30);
    }

    #[test]
    fn filter_input_parses_priority_label_and_text() {
        let f = IssuesPanelOverlay::parse_filter_input("priority=critical label=auth text=login")
            .expect("expected filter");
        assert_eq!(f.priority, Some(crate::store::issues::Priority::Critical));
        assert_eq!(f.label.as_deref(), Some("auth"));
        assert_eq!(f.text.as_deref(), Some("login"));
        assert!(f.status.is_none());
    }

    #[test]
    fn filter_input_empty_returns_none() {
        assert!(IssuesPanelOverlay::parse_filter_input("").is_none());
        assert!(IssuesPanelOverlay::parse_filter_input("   ").is_none());
    }

    #[test]
    fn filter_input_status_overrides_status_field() {
        let f = IssuesPanelOverlay::parse_filter_input("status=closed").unwrap();
        assert_eq!(f.status, Some(Status::Closed));
    }

    #[test]
    fn filter_input_ignores_unknown_tokens() {
        // Unknown keys don't poison the parse; valid keys still apply.
        let f = IssuesPanelOverlay::parse_filter_input("priority=low bogus=value").unwrap();
        assert_eq!(f.priority, Some(crate::store::issues::Priority::Low));
    }

    #[tokio::test]
    async fn slash_enters_filter_input_mode() {
        let (_tmp, store) = tmp_store();
        let mut panel = IssuesPanelOverlay::new(store);
        assert!(!panel.filter_input_mode);
        // Press `/`.
        panel.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('/'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(panel.filter_input_mode);
    }

    #[test]
    fn filter_input_enter_applies_filter_and_exits_mode() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_tmp, store) = tmp_store();
        let mut panel = IssuesPanelOverlay::new(store);
        // Enter filter input mode and type a filter.
        panel.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for ch in "priority=high".chars() {
            panel.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert!(panel.filter_input_mode);
        assert_eq!(panel.filter_input_text, "priority=high");
        // Press Enter to apply.
        panel.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!panel.filter_input_mode);
        assert!(panel.custom_filter.is_some());
        assert_eq!(
            panel.custom_filter.as_ref().unwrap().priority,
            Some(crate::store::issues::Priority::High)
        );
    }

    #[test]
    fn filter_input_esc_cancels_without_applying() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_tmp, store) = tmp_store();
        let mut panel = IssuesPanelOverlay::new(store);
        panel.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for ch in "priority=high".chars() {
            panel.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        panel.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!panel.filter_input_mode);
        assert!(panel.custom_filter.is_none());
        assert_eq!(panel.filter_input_text, "");
    }
}
