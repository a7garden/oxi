//! State, rendering, and input handling for the `/issue` TUI panel.
//! See docs/superpowers/specs/2026-08-27-tui-issues-panel-design.md.

use std::path::Path;

use crate::store::issues::{FileIssueStore, IssueFilter, IssuePatch, Priority, Status, liveness};
mod filter_parse;
// Consumed by `input::FilterInput` Enter handler in Task 6.
pub(crate) use filter_parse::parse_issue_filter;
mod store_handle;

pub(crate) use store_handle::get_or_open_store;

mod form;
// Task 8: `cycle_priority` is wired into the `Left/Right` cycling on the
// Priority field; `parse_labels` translates the comma-separated `labels_input`
// String into `Vec<String>` at submit time.
pub(crate) use form::{cycle_priority, parse_labels};

mod input;
mod render;

pub(crate) use input::handle_issues_panel_key;
pub(crate) use render::render_issues_panel;

// `status_filter`: `Some(Status::Open)` is the default view (open issues
// only); `None` = All (no status constraint). The `f` key in the panel
// toggles between the two. We hand-write `Default` so the derived form's
// `Option::default()` (= `None` = All) does NOT shadow the intended
// Open-on-first-open behavior.
#[derive(Debug)]
pub(crate) struct IssuesPanelState {
    pub mode: IssuesPanelMode,
    pub status_filter: Option<Status>,
    pub extra_filter: IssueFilter,
    pub rows: Vec<IssueRow>,
    pub selected: usize,
    pub pending: bool,
    pub error: Option<String>,
    /// Body text for the current Detail view, populated synchronously via
    /// `FileIssueStore::read(id)` on List→Detail transition. `None` while
    /// the read hasn't happened yet (or failed) — render layer shows a
    /// "(loading…)" placeholder in that case.
    pub detail_body_cache: Option<String>,
}

impl Default for IssuesPanelState {
    fn default() -> Self {
        Self {
            mode: IssuesPanelMode::default(),
            status_filter: Some(Status::Open),
            extra_filter: IssueFilter::default(),
            rows: Vec::new(),
            selected: 0,
            pending: false,
            error: None,
            detail_body_cache: None,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) enum IssuesPanelMode {
    #[default]
    List,
    Detail {
        id: u32,
        scroll: usize,
    },
    Form(Box<IssueFormState>),
    FilterInput(String),
}

#[derive(Clone, Debug)]
pub(crate) struct IssueRow {
    pub id: u32,
    pub title: String,
    pub status: Status,
    pub priority: Priority,
    pub labels: Vec<String>,
    pub assignee_badge: Option<AssigneeBadge>,
    /// Snapshot of `IssueMeta` timestamps at `refresh()` time (design §5
    /// Detail meta header). Carried read-only; never written back.
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub closed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AssigneeBadge {
    Live(String),
    Stale(String),
}

#[derive(Debug)]
pub(crate) struct IssueFormState {
    pub editing_id: Option<u32>,
    pub content_hash: Option<String>,
    pub title: String,
    pub priority: Priority,
    pub labels_input: String,
    pub body: oxicode_textarea::TextArea,
    pub focus: FormField,
}

impl Default for IssueFormState {
    fn default() -> Self {
        Self {
            editing_id: None,
            content_hash: None,
            title: String::new(),
            priority: Priority::default(),
            labels_input: String::new(),
            body: oxicode_textarea::TextArea::new(),
            focus: FormField::Title,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum FormField {
    #[default]
    Title,
    Priority,
    Labels,
    Body,
}

impl IssuesPanelState {
    pub fn refresh(&mut self, store: &FileIssueStore, issues_dir: &Path) {
        let filter = self.effective_filter();
        let issues = match store.list(&filter) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(e.to_string());
                self.rows.clear();
                return;
            }
        };
        self.rows = issues
            .into_iter()
            .map(|issue| IssueRow {
                id: issue.meta.id,
                title: issue.meta.title,
                status: issue.meta.status,
                priority: issue.meta.priority,
                labels: issue.meta.labels,
                assignee_badge: issue.meta.assigned_to.map(|a| {
                    if liveness::is_session_alive(issues_dir, &a.session) {
                        AssigneeBadge::Live(a.session)
                    } else {
                        AssigneeBadge::Stale(a.session)
                    }
                }),
                created_at: issue.meta.created_at,
                updated_at: issue.meta.updated_at,
                closed_at: issue.meta.closed_at,
            })
            .collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// Union of the status toggle and the `/` filter-modal fields (design §4).
    /// `None` in `status_filter` flows through as `None` here, matching the
    /// design's "All" view (no status constraint).
    fn effective_filter(&self) -> crate::store::issues::IssueFilter {
        crate::store::issues::IssueFilter {
            status: self.status_filter,
            ..self.extra_filter.clone()
        }
    }
}

/// Requests the panel's synchronous key-handling code cannot satisfy itself
/// (CAS-guarded async store writes). Sent over a dedicated channel into
/// `run_event_loop`'s `select!` — kept out of `oxicode_vtui::InlineEvent` so
/// the framework crate stays free of `oxicode-*` dependencies.
#[derive(Clone, Debug)]
pub(crate) enum IssueActionRequest {
    Close {
        id: u32,
        caller: String,
        hash: Option<String>,
    },
    Reopen {
        id: u32,
        hash: Option<String>,
    },
    ApplyPatch {
        id: u32,
        patch: IssuePatch,
        caller: Option<String>,
        hash: Option<String>,
    },
}

/// Receive an action from the panel's dedicated mpsc channel and resolve it
/// with a real CAS-guarded store write.
///
/// Runs the mutation on a spawned task so the event loop never blocks on
/// filesystem I/O. The `parking_lot` guard is dropped before the `.await`
/// and re-acquired after completion (AGENTS.md pitfall: never hold it across
/// an await). On completion the panel's `pending` flag clears, the outcome
/// lands in `panel.error`, and the row list refreshes regardless — a failed
/// write may still reflect a concurrent change made by another session.
pub(crate) fn dispatch_action(
    req: IssueActionRequest,
    state: std::sync::Arc<parking_lot::Mutex<crate::tui_vt::main_loop::RenderState>>,
) {
    let store = { state.lock().issue_store.clone() };
    let Some(store) = store else {
        let mut s = state.lock();
        if let Some(panel) = s.issues_panel.as_mut() {
            panel.pending = false;
            panel.error = Some("issue store not initialized".into());
        }
        return;
    };
    tokio::spawn(async move {
        let result = match req {
            IssueActionRequest::Close { id, caller, hash } => {
                // Mirror the CLI's `oxicode issue close` flow
                // (`oxicode-cli/src/cli/commands/issue.rs:84-103`):
                //   1. `start` claims ownership (fails `Assigned` for a
                //      live OTHER session — surfaced, not closed).
                //   2. Re-read for a fresh hash.
                //   3. `close` requires the assignee — now satisfied.
                // The whole dance lives inside `cas_retry`'s closure so a
                // `Conflict` from any step re-runs the sequence; `start`
                // is idempotent for the same caller, so a retry after a
                // close-side conflict is safe.
                crate::tools::issue_tool::cas_retry(&store, id, hash, |h| {
                    let store = store.clone();
                    let caller = caller.clone();
                    async move {
                        // `h: Option<String>` matches `store.start`'s
                        // `expected_hash: Option<String>` directly — do
                        // NOT wrap in `Some()`.
                        store.start(id, &caller, h).await?;
                        let (_, fresh_hash) = store.read(id)?;
                        store.close(id, &caller, Some(fresh_hash)).await
                    }
                })
                .await
            }
            IssueActionRequest::Reopen { id, hash } => {
                crate::tools::issue_tool::cas_retry(&store, id, hash, |h| {
                    let store = store.clone();
                    async move { store.reopen(id, h).await }
                })
                .await
            }
            IssueActionRequest::ApplyPatch {
                id,
                patch,
                caller,
                hash,
            } => {
                crate::tools::issue_tool::cas_retry(&store, id, hash, |h| {
                    let store = store.clone();
                    let patch = patch.clone();
                    let caller = caller.clone();
                    async move { store.apply_patch(id, patch, caller, h).await }
                })
                .await
            }
        };

        let mut s = state.lock();
        if let Some(panel) = s.issues_panel.as_mut() {
            panel.pending = false;
            match result {
                Ok(_) => panel.error = None,
                Err(e) => panel.error = Some(e.to_string()),
            }
        }
        // Refresh regardless of outcome — a failed write may still reflect
        // a concurrent change made by another session.
        let store2 = s.issue_store.clone();
        if let (Some(store2), Some(panel)) = (store2, s.issues_panel.as_mut()) {
            panel.refresh(&store2, &store2.issues_dir());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_state_opens_in_list_mode_with_open_filter() {
        let state = IssuesPanelState::default();
        assert!(matches!(state.mode, IssuesPanelMode::List));
        assert_eq!(state.status_filter, Some(Status::Open));
        assert!(!state.pending);
        assert!(state.error.is_none());
        assert!(state.rows.is_empty());
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use crate::store::issues::{FileIssueStore, Priority};

    fn tmp_store() -> (tempfile::TempDir, FileIssueStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileIssueStore::open(tmp.path().to_path_buf()).unwrap();
        (tmp, store)
    }

    #[test]
    fn refresh_populates_rows_sorted_by_recency() {
        let (tmp, store) = tmp_store();
        store
            .create("first".into(), "body".into(), Priority::Low, vec![], None)
            .unwrap();
        store
            .create("second".into(), "body".into(), Priority::High, vec![], None)
            .unwrap();

        let mut panel = IssuesPanelState::default();
        panel.refresh(&store, tmp.path());

        assert_eq!(panel.rows.len(), 2);
        // FileIssueStore::list sorts by updated_at desc — "second" was
        // created after "first" so it sorts first.
        assert_eq!(panel.rows[0].title, "second");
        assert_eq!(panel.rows[1].title, "first");
    }

    #[test]
    fn refresh_marks_unassigned_issues_with_no_badge() {
        let (tmp, store) = tmp_store();
        store
            .create("solo".into(), "body".into(), Priority::Medium, vec![], None)
            .unwrap();
        let mut panel = IssuesPanelState::default();
        panel.refresh(&store, tmp.path());
        assert!(panel.rows[0].assignee_badge.is_none());
    }
}

/// Task 9: real dispatch (`IssueActionRequest` → CAS-guarded store writes).
/// Both requests below carry the SAME (stale-after-the-first-write) hash so
/// the second one must flow through `cas_retry`'s re-read-and-retry path.
#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::store::issues::{FileIssueStore, IssuePatch, Priority};
    use std::sync::Arc;

    #[tokio::test]
    async fn concurrent_apply_patch_via_dispatch_action_both_eventually_succeed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(FileIssueStore::open(tmp.path().to_path_buf()).unwrap());
        let issue = store
            .create("t".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let (_issue, hash) = store.read(issue.meta.id).unwrap();

        let state = Arc::new(parking_lot::Mutex::new(
            crate::tui_vt::main_loop::RenderState {
                issue_store: Some(store.clone()),
                issues_panel: Some(IssuesPanelState::default()),
                ..Default::default()
            },
        ));

        // Both requests carry the SAME (now-stale-after-the-first-write) hash,
        // forcing the second one through cas_retry's re-read-and-retry path.
        dispatch_action(
            IssueActionRequest::ApplyPatch {
                id: issue.meta.id,
                patch: IssuePatch {
                    title: Some("first".into()),
                    ..Default::default()
                },
                caller: None,
                hash: Some(hash.clone()),
            },
            state.clone(),
        );
        dispatch_action(
            IssueActionRequest::ApplyPatch {
                id: issue.meta.id,
                patch: IssuePatch {
                    priority: Some(Priority::High),
                    ..Default::default()
                },
                caller: None,
                hash: Some(hash),
            },
            state.clone(),
        );

        // Give both spawned tasks a chance to run.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let (final_issue, _) = store.read(issue.meta.id).unwrap();
        assert_eq!(final_issue.meta.title, "first");
        assert_eq!(final_issue.meta.priority, Priority::High);
        let s = state.lock();
        assert!(s.issues_panel.as_ref().unwrap().error.is_none());
    }

    /// F1 regression: closing an UNASSIGNED issue via the panel's
    /// `IssueActionRequest::Close` path used to fail with `NotAssigned`
    /// because `store.close` requires the caller to hold the assignment.
    /// The fixed dispatch does `start` → re-read → `close` (mirroring
    /// the CLI), so an unassigned issue is now claimed-then-closed in
    /// one CAS-guarded sequence.
    #[tokio::test]
    async fn dispatch_action_close_unassigned_issue_claims_then_closes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(FileIssueStore::open(tmp.path().to_path_buf()).unwrap());
        let issue = store
            .create("t".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        // Sanity: the issue is unassigned at creation.
        assert!(issue.meta.assigned_to.is_none());

        let state = Arc::new(parking_lot::Mutex::new(
            crate::tui_vt::main_loop::RenderState {
                issue_store: Some(store.clone()),
                issues_panel: Some(IssuesPanelState::default()),
                ..Default::default()
            },
        ));

        dispatch_action(
            IssueActionRequest::Close {
                id: issue.meta.id,
                caller: "tui-ownership".into(),
                hash: None,
            },
            state.clone(),
        );

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let (closed, _) = store.read(issue.meta.id).unwrap();
        assert_eq!(closed.meta.status, Status::Closed);
        let s = state.lock();
        let panel = s.issues_panel.as_ref().unwrap();
        assert!(!panel.pending, "pending should clear on completion");
        assert!(
            panel.error.is_none(),
            "close must not error: {:?}",
            panel.error
        );
    }
}
