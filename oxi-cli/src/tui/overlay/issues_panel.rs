//! Local issues panel — list / detail, modeled as an [`OverlayComponent`].
//!
//! Read-only view: list issues, drill into details. Mutations (start, release,
//! close) are available via slash commands (`/issue start 12`, etc.) so the
//! overlay stays synchronous and integrates with the same FileIssueStore
//! surface as the agent tool.
//!
//! Keys (list):  `j`/`↓` next · `k`/`↑` prev · `Enter`/`l` detail ·
//!               `f` toggle status filter · `Esc`/`q` close
//! Keys (detail): `Esc`/`h`/`q` back · `j`/`k` next/prev issue

use std::sync::Arc;

use super::{OverlayAction, OverlayComponent, centered_layout};
use crate::store::issues::{FileIssueStore, Issue, IssueFilter, Status};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_tui::Theme;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    List,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    Open,
    All,
}

pub struct IssuesPanelOverlay {
    store: Arc<FileIssueStore>,
    view: View,
    items: Vec<Issue>,
    list_state: ListState,
    status_filter: StatusFilter,
}

impl std::fmt::Debug for IssuesPanelOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuesPanelOverlay")
            .field("view", &self.view)
            .field("items", &self.items.len())
            .field("status_filter", &self.status_filter)
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
            status_filter: StatusFilter::Open,
        };
        me.refresh();
        me
    }

    fn refresh(&mut self) {
        let status = match self.status_filter {
            StatusFilter::Open => Some(Status::Open),
            StatusFilter::All => None,
        };
        let filter = IssueFilter {
            status,
            priority: None,
            label: None,
            assigned_to_session: None,
            text: None,
        };
        self.items = self.store.list(&filter).unwrap_or_default();
        if !self.items.is_empty()
            && self.list_state.selected().is_none_or(|s| s >= self.items.len())
        {
            self.list_state.select(Some(0));
        } else if self.items.is_empty() {
            self.list_state.select(None);
        }
    }

    /// Synthetic session id for TUI-driven mutations. The TUI runtime
    /// (`run_tui_interactive_impl`) holds the liveness lock for "tui" for
    /// the whole session, so any `start`/`release`/`close` issued through
    /// the panel satisfies the alive-owner check.
    pub fn session_id() -> &'static str {
        "tui"
    }

    fn selected(&self) -> Option<&Issue> {
        self.list_state.selected().and_then(|i| self.items.get(i))
    }

    fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        let len = self.items.len() as isize;
        let next = (cur as isize + delta).rem_euclid(len) as usize;
        self.list_state.select(Some(next));
    }
}

impl OverlayComponent for IssuesPanelOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        match self.view {
            View::List => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Close,
                KeyCode::Char('j') | KeyCode::Down => {
                    self.move_selection(1);
                    OverlayAction::None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.move_selection(-1);
                    OverlayAction::None
                }
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                    if self.selected().is_some() {
                        self.view = View::Detail;
                    }
                    OverlayAction::None
                }
                KeyCode::Char('f') | KeyCode::Char('F') => {
                    self.status_filter = match self.status_filter {
                        StatusFilter::Open => StatusFilter::All,
                        StatusFilter::All => StatusFilter::Open,
                    };
                    self.refresh();
                    OverlayAction::None
                }
                _ => OverlayAction::None,
            },
            View::Detail => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') | KeyCode::Left => {
                    self.view = View::List;
                    OverlayAction::None
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.move_selection(1);
                    OverlayAction::None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.move_selection(-1);
                    OverlayAction::None
                }
                _ => OverlayAction::None,
            },
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _theme: &Theme) {
        let popup = centered_layout(area, 0.9, 0.85);
        frame.render_widget(Clear, popup);
        match self.view {
            View::List => self.render_list(frame, popup),
            View::Detail => self.render_detail(frame, popup),
        }
    }

    fn hint(&self) -> &str {
        match self.view {
            View::List => "j/k:move  Enter:detail  f:filter  q:close",
            View::Detail => "j/k:next/prev  Esc:back  q:close",
        }
    }
}

impl IssuesPanelOverlay {
    fn render_list(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                "Issues ({}) — filter: {}",
                self.items.len(),
                match self.status_filter {
                    StatusFilter::Open => "open",
                    StatusFilter::All => "all",
                }
            ));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.items.is_empty() {
            let msg = "No issues. Use `/issue new <title>` or the agent's `issue create`.";
            frame.render_widget(Paragraph::new(msg).wrap(Wrap { trim: false }), inner);
            return;
        }

        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|i| {
                let lock = if i.meta.assigned_to.is_some() { "🔒" } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::raw(format!("#{:<4} ", i.meta.id)),
                    Span::styled(
                        format!("[{}] ", i.meta.status),
                        Style::default().fg(match i.meta.status {
                            Status::Open => Color::Green,
                            Status::Closed => Color::DarkGray,
                        }),
                    ),
                    Span::raw(format!("{:8} ", i.meta.priority)),
                    Span::raw(format!("{lock} ")),
                    Span::raw(i.meta.title.clone()),
                ]))
            })
            .collect();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(inner);
        frame.render_stateful_widget(
            List::new(items).highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            chunks[0],
            &mut self.list_state,
        );
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let Some(issue) = self.selected() else {
            // No selection (e.g. list refreshed and shrank): bounce back.
            // We can't mutate self here (we have &self), so just show empty.
            return;
        };
        let (_, hash) = self
            .store
            .read(issue.meta.id)
            .unwrap_or_else(|_| (issue.clone(), String::from("<hash-unavailable>")));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!("Issue #{}", issue.meta.id));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let text = format_issue_detail(issue, &hash);
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
    }
}

fn format_issue_detail(i: &Issue, hash: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# {} ({} / {})\n\n",
        i.meta.title, i.meta.status, i.meta.priority
    ));
    s.push_str(&format!("id: {}\n", i.meta.id));
    s.push_str(&format!("created: {}\n", i.meta.created_at));
    s.push_str(&format!("updated: {}\n", i.meta.updated_at));
    if let Some(c) = i.meta.closed_at {
        s.push_str(&format!("closed: {}\n", c));
    }
    s.push_str(&format!("labels: {:?}\n", i.meta.labels));
    s.push_str(&format!("sessions: {:?}\n", i.meta.sessions));
    if let Some(a) = &i.meta.assigned_to {
        s.push_str(&format!(
            "assigned: {} (since {})\n",
            a.session, a.acquired_at
        ));
    }
    s.push_str(&format!("content_hash: {}\n\n", hash));
    s.push_str(&i.body);
    s
}
