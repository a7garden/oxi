//! Agent Hub overlay — fullscreen TUI monitor for advisor + subagents.
//!
//! Two views:
//! - **Table** (default landing): one row per agent with name, kind, status,
//!   current task, and last-activity age. Selection drives keyboard nav.
//! - **Transcript** (per-agent): live tail of the underlying `.jsonl` file
//!   via [`TranscriptReader`], with manual scroll + tail-follow.
//!
//! The registry lives in [`AgentSession`] as `Arc<HubRegistry>` (cheap to
//! snapshot but *not* `Clone`). The overlay holds an
//! [`AgentSessionHandle`] (cheap clone of the session) and re-snapshots the
//! registry on every [`OverlayComponent::poll`] tick. Transcript readers are
//! owned by the overlay and keyed by agent id, so newly-appearing JSONL
//! files get a reader on the next poll without holding the registry.

pub mod keys;
pub mod state;
pub mod table;
pub mod transcript;

use std::collections::{HashMap, HashSet};

use crossterm::event::KeyEvent;
use oxi_tui::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::{OverlayAction, OverlayComponent};
use crate::app::agent_hub_registry::{HubRegistry, now_ms};
use crate::app::agent_session::AgentSessionHandle;

use self::keys::{HubAction, handle_key as dispatch_key};
use self::state::{HubState, HubView};
use self::table::render_table;
use self::transcript::TranscriptReader;

// Re-export transcript types so downstream callers don't depend on the
#[allow(unused_imports)]
pub use self::transcript::{TranscriptLine, TranscriptReader as TranscriptReaderExport};

/// Fullscreen overlay showing the agent registry + per-agent transcripts.
pub struct AgentHubOverlay {
    session: AgentSessionHandle,
    state: HubState,
    /// One reader per agent id, kept lazily so newly-discovered JSONL files
    /// get a reader on first poll.
    readers: HashMap<String, TranscriptReader>,
}

impl std::fmt::Debug for AgentHubOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHubOverlay")
            .field("rows", &self.state.rows.len())
            .field("view", &self.state.view)
            .field("selected", &self.state.selected)
            .field("readers", &self.readers.len())
            .finish()
    }
}

impl AgentHubOverlay {
    /// Build the overlay from the current session handle. Seeds rows from
    /// the registry snapshot and creates readers for any rows that already
    /// have a known `session_file`.
    #[must_use]
    pub fn new(session: AgentSessionHandle) -> Self {
        let rows = Self::snapshot_rows(session.hub());
        let mut row_order = HashMap::with_capacity(rows.len());
        for (i, r) in rows.iter().enumerate() {
            row_order.insert(r.id.clone(), i);
        }
        let mut readers = HashMap::new();
        for r in &rows {
            if let Some(path) = &r.session_file {
                readers.insert(r.id.clone(), TranscriptReader::new(path.clone()));
            }
        }
        Self {
            session,
            state: HubState {
                rows,
                view: HubView::Table,
                selected: 0,
                row_order,
                transcript_scroll: 0,
                transcript_follow: true,
            },
            readers,
        }
    }

    /// Snapshot the registry into render-ready rows. Cheap — one read-lock
    /// + clone of each entry (the lock is dropped before the iter).
    fn snapshot_rows(reg: &HubRegistry) -> Vec<state::HubRow> {
        HubState::from_registry(reg, now_ms())
    }

    /// Refresh readers; cheap no-op when mtime/size unchanged.
    fn poll_readers(&mut self) {
        for reader in self.readers.values_mut() {
            let _ = reader.refresh();
        }
    }

    /// Render the transcript view for `agent_id` into `area`.
    fn render_transcript(&self, f: &mut Frame, area: Rect, agent_id: &str) {
        let Some(reader) = self.readers.get(agent_id) else {
            // No file known yet — show a placeholder. This happens when a
            // registry row exists without a `session_file` (rare — typically
            // only seen for the in-memory main agent whose transcript is
            // owned by AgentSession).
            let p = Paragraph::new(Line::from(Span::raw(
                "  (no transcript file registered for this agent)",
            )))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {agent_id} ")),
            )
            .wrap(Wrap { trim: false });
            f.render_widget(p, area);
            return;
        };

        let lines = reader.lines();
        let visible = compute_visible_window(lines, area.height as usize, &self.state);
        let title = if self.state.transcript_follow {
            format!(" {agent_id} \u{2014} FOLLOW ")
        } else {
            format!(" {agent_id} ")
        };
        let p = Paragraph::new(visible)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
    }
}

/// Trait-level surface.
impl OverlayComponent for AgentHubOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match dispatch_key(&mut self.state, key) {
            HubAction::None => OverlayAction::None,
            HubAction::Close => OverlayAction::Close,
            HubAction::OpenTranscript(id) => {
                // Ensure a reader exists even if the file appeared after open.
                if !self.readers.contains_key(&id)
                    && let Some(row) = self.state.rows.iter().find(|r| r.id == id)
                    && let Some(path) = &row.session_file
                {
                    self.readers
                        .insert(id.clone(), TranscriptReader::new(path.clone()));
                }
                self.state.view = HubView::Transcript { agent_id: id };
                // Re-engage tail-follow on every entry.
                self.state.transcript_follow = true;
                self.state.transcript_scroll = keys::FOLLOW_TAIL;
                OverlayAction::None
            }
        }
    }

    /// Refresh registry + readers on every main-loop tick (~50ms).
    fn poll(&mut self) -> OverlayAction {
        let new_rows = Self::snapshot_rows(self.session.hub());

        // Preserve the user's selected id across refreshes.
        let selected_id = self
            .state
            .rows
            .get(self.state.selected)
            .map(|r| r.id.clone());

        // Open readers for newly discovered files.
        for r in &new_rows {
            if !self.readers.contains_key(&r.id)
                && let Some(path) = &r.session_file
            {
                self.readers
                    .insert(r.id.clone(), TranscriptReader::new(path.clone()));
            }
        }

        // Drop readers for agents that no longer exist.
        let current_ids: HashSet<&str> = new_rows.iter().map(|r| r.id.as_str()).collect();
        self.readers
            .retain(|id, _| current_ids.contains(id.as_str()));

        self.state.rows = new_rows;

        // Rebuild the id → index map.
        self.state.row_order.clear();
        for (i, r) in self.state.rows.iter().enumerate() {
            self.state.row_order.insert(r.id.clone(), i);
        }

        // Clamp / restore selection.
        if let Some(id) = selected_id
            && let Some(pos) = self.state.rows.iter().position(|r| r.id == id)
        {
            self.state.selected = pos;
        }
        if self.state.selected >= self.state.rows.len() {
            self.state.selected = self.state.rows.len().saturating_sub(1);
        }

        self.poll_readers();
        OverlayAction::None
    }

    fn render(&mut self, f: &mut Frame, area: Rect, theme: &Theme) {
        // Clear the entire overlay area first so the table / transcript owns
        // the screen — matches the alt-screen fullscreen expectation.
        f.render_widget(Clear, area);
        match &self.state.view {
            HubView::Table => render_table(f, area, &self.state, theme),
            HubView::Transcript { agent_id } => self.render_transcript(f, area, agent_id),
        }
    }

    fn hint(&self) -> &str {
        match self.state.view {
            HubView::Table => " j/k: nav  Enter: transcript  Esc/q: close",
            HubView::Transcript { .. } => {
                " j/k: scroll  PgUp/PgDn: page  g/G: start/end  f: follow  Esc/q: back"
            }
        }
    }
}

/// Compute the window of transcript lines to render, given the signed
/// `transcript_scroll` convention:
///   `scroll == FOLLOW_TAIL` (0) or `follow == true` → pin to the last `window` lines.
///   `scroll > 0`           → top-line offset (clamps at `total - window`).
///   `scroll < 0`           → `|scroll|` lines below the tail.
///   `scroll == JUMP_TOP` (isize::MAX) → start at the head of history.
fn compute_visible_window(
    lines: &[transcript::TranscriptLine],
    height: usize,
    state: &HubState,
) -> Vec<Line<'static>> {
    if lines.is_empty() || height == 0 {
        return Vec::new();
    }
    let total = lines.len();
    let window = height.min(total);
    let max_start = total - window;

    let start = if state.transcript_follow || state.transcript_scroll == keys::FOLLOW_TAIL {
        // Tail-follow or freshly-opened transcript.
        max_start
    } else if state.transcript_scroll >= 0 {
        // Top-line offset (or JUMP_TOP, which saturates to 0).
        let s = state.transcript_scroll as usize;
        s.min(max_start)
    } else {
        // Negative offset: `|scroll|` lines below the tail.
        let back = state.transcript_scroll.unsigned_abs();
        max_start.saturating_sub(back.min(max_start))
    };

    let end = start + window;
    lines[start..end]
        .iter()
        .map(|l| {
            let role = format!("[{}] ", l.role);
            Line::from(vec![Span::raw(role), Span::raw(l.text.clone())])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_rows_projects_registry_into_rows() {
        use crate::app::agent_hub_registry::{HubEntry, HubRegistry};
        use oxi_sdk::{HubKind, HubStatus};
        use std::path::PathBuf;

        let reg = HubRegistry::new();
        reg.register(
            "a".into(),
            HubEntry {
                kind: HubKind::Advisor,
                status: HubStatus::Idle,
                display_name: "advisor".into(),
                current_task: None,
                last_activity_ms: 0,
                session_file: Some(PathBuf::from("/tmp/a.jsonl")),
            },
        );
        let rows = AgentHubOverlay::snapshot_rows(&reg);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a");
        assert_eq!(rows[0].kind, HubKind::Advisor);
        assert_eq!(rows[0].status, HubStatus::Idle);
        assert_eq!(rows[0].display_name, "advisor");
        assert!(!rows[0].age_text.is_empty());
        assert_eq!(rows[0].session_file, Some(PathBuf::from("/tmp/a.jsonl")));
    }

    #[test]
    fn compute_visible_window_pins_to_tail_when_following() {
        let lines: Vec<transcript::TranscriptLine> = (0..10)
            .map(|i| transcript::TranscriptLine {
                timestamp_ms: i,
                role: "assistant".into(),
                text: format!("line-{i}"),
                tool_name: None,
                tool_status: None,
            })
            .collect();
        let mut state = HubState {
            rows: Vec::new(),
            view: HubView::Transcript {
                agent_id: "x".into(),
            },
            selected: 0,
            row_order: HashMap::new(),
            transcript_scroll: 0,
            transcript_follow: true,
        };
        // FOLLOW_TAIL with follow=true → last 3 lines.
        state.transcript_scroll = keys::FOLLOW_TAIL;
        let visible = compute_visible_window(&lines, 3, &state);
        assert_eq!(visible.len(), 3);
        // The tail should be the last 3 lines.
        assert!(visible[0].to_string().contains("line-7"));
        assert!(visible[2].to_string().contains("line-9"));
    }

    #[test]
    fn compute_visible_window_clamps_scroll_to_history() {
        let lines: Vec<transcript::TranscriptLine> = (0..10)
            .map(|i| transcript::TranscriptLine {
                timestamp_ms: i,
                role: "assistant".into(),
                text: format!("line-{i}"),
                tool_name: None,
                tool_status: None,
            })
            .collect();
        let state = HubState {
            rows: Vec::new(),
            view: HubView::Table,
            selected: 0,
            row_order: HashMap::new(),
            transcript_scroll: 100, // past the end
            transcript_follow: false,
        };
        let visible = compute_visible_window(&lines, 3, &state);
        // Window clamped to the last `window` lines.
        assert_eq!(visible.len(), 3);
        assert!(visible[0].to_string().contains("line-7"));
        assert!(visible[2].to_string().contains("line-9"));
    }

    #[test]
    fn compute_visible_window_empty_input_returns_empty() {
        let state = HubState {
            rows: Vec::new(),
            view: HubView::Table,
            selected: 0,
            row_order: HashMap::new(),
            transcript_scroll: 0,
            transcript_follow: true,
        };
        let visible = compute_visible_window(&[], 10, &state);
        assert!(visible.is_empty());
    }
}
