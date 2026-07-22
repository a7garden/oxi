//! Keyboard + mouse input handling for the issues panel.
//!
//! The dispatch table is large but straightforward — every key falls into
//! either a navigation key, a view-switch key, or a mutation key. Mutations
//! are dispatched through `dispatch_action` in `state.rs` to share the
//! in-flight guard.

use super::IssuesPanelOverlay;
use super::state::{StatusFilter, View};
use crate::tui::overlay::{OverlayAction, OverlayComponent, centered_layout};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use oxi_tui_legacy::Theme;
use ratatui::{Frame, layout::Rect, widgets::Clear};

impl OverlayComponent for IssuesPanelOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        match self.view {
            View::List => self.handle_key_list(key),
            View::Detail => self.handle_key_detail(key),
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent) -> OverlayAction {
        if !matches!(
            event.kind,
            MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
        ) {
            return OverlayAction::None;
        }
        match self.view {
            View::List => match event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(idx) = self.list_hit_test(event.row) {
                        self.list_state.select(Some(idx));
                        // Double-click (consecutive clicks within
                        // DOUBLE_CLICK_MS on the same row) opens detail.
                        // Single-click only selects — matches the
                        // keyboard semantics (Enter to open).
                        if self.is_double_click(idx) {
                            self.detail_scroll = 0;
                            self.view = View::Detail;
                        } else {
                            self.record_click(idx);
                        }
                    }
                    OverlayAction::None
                }
                MouseEventKind::ScrollUp => {
                    self.move_selection(-1);
                    OverlayAction::None
                }
                MouseEventKind::ScrollDown => {
                    self.move_selection(1);
                    OverlayAction::None
                }
                _ => OverlayAction::None,
            },
            View::Detail => match event.kind {
                MouseEventKind::ScrollUp => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                    OverlayAction::None
                }
                MouseEventKind::ScrollDown => {
                    if self.detail_scroll + self.detail_visible < self.detail_body_lines() {
                        self.detail_scroll += 1;
                    }
                    OverlayAction::None
                }
                _ => OverlayAction::None,
            },
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        // Refresh on render so external changes (CLI, other agent) are
        // picked up even without explicit keypress-driven ticks.
        self.tick();

        let popup = centered_layout(area, 0.9, 0.85);
        frame.render_widget(Clear, popup);
        match self.view {
            View::List => self.render_list(frame, popup, theme),
            View::Detail => self.render_detail(frame, popup, theme),
        }
    }

    fn hint(&self) -> &str {
        match self.view {
            View::List => "j/k:move  ↵:detail  s/r/c:act  /:filter  f:status  R:reload  q:close",
            View::Detail => "j/k:issue  J/K:scroll  s/r/c:act  Esc:back  q:close",
        }
    }
}

impl IssuesPanelOverlay {
    pub(super) fn handle_key_list(&mut self, key: KeyEvent) -> OverlayAction {
        // ── Filter input modal — intercept ALL keys ─────────────────
        if self.filter_input_mode {
            return self.handle_filter_input(key);
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Close,
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                OverlayAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                OverlayAction::None
            }
            KeyCode::PageDown => {
                self.page_selection(1);
                OverlayAction::None
            }
            KeyCode::PageUp => {
                self.page_selection(-1);
                OverlayAction::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.jump_first();
                OverlayAction::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.jump_last();
                OverlayAction::None
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if self.selected().is_some() {
                    self.view = View::Detail;
                    self.detail_scroll = 0;
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
            KeyCode::Char('R') => {
                self.refresh();
                OverlayAction::None
            }
            KeyCode::Char('/') => {
                // Enter filter input mode. Seed the text with the current
                // custom filter so users can edit rather than retype.
                self.filter_input_text = self
                    .custom_filter
                    .as_ref()
                    .map(IssuesPanelOverlay::format_filter_for_input)
                    .unwrap_or_default();
                self.filter_input_mode = true;
                OverlayAction::None
            }
            _ => self.try_action_key(key.code).unwrap_or(OverlayAction::None),
        }
    }

    pub(super) fn handle_key_detail(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
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
            KeyCode::Char('J') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
                OverlayAction::None
            }
            KeyCode::Char('K') => {
                if self.detail_scroll + self.detail_visible < self.detail_body_lines() {
                    self.detail_scroll += 1;
                }
                OverlayAction::None
            }
            KeyCode::PageDown => {
                let step = self.detail_visible.max(1);
                self.detail_scroll = (self.detail_scroll + step)
                    .min(self.detail_body_lines().saturating_sub(self.detail_visible));
                OverlayAction::None
            }
            KeyCode::PageUp => {
                let step = self.detail_visible.max(1);
                self.detail_scroll = self.detail_scroll.saturating_sub(step);
                OverlayAction::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.detail_scroll = 0;
                OverlayAction::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.detail_scroll = self.detail_body_lines().saturating_sub(self.detail_visible);
                OverlayAction::None
            }
            KeyCode::Char('R') => {
                self.refresh();
                OverlayAction::None
            }
            _ => self.try_action_key(key.code).unwrap_or(OverlayAction::None),
        }
    }

    /// Handle keys while `filter_input_mode` is active. Enter applies the
    /// parsed filter (or clears it if empty), Esc cancels.
    fn handle_filter_input(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc => {
                self.filter_input_mode = false;
                self.filter_input_text.clear();
                OverlayAction::None
            }
            KeyCode::Enter => {
                let parsed = IssuesPanelOverlay::parse_filter_input(&self.filter_input_text);
                self.custom_filter = parsed;
                self.filter_input_mode = false;
                self.filter_input_text.clear();
                self.refresh();
                OverlayAction::None
            }
            KeyCode::Backspace => {
                self.filter_input_text.pop();
                OverlayAction::None
            }
            KeyCode::Char(c) => {
                self.filter_input_text.push(c);
                OverlayAction::None
            }
            _ => OverlayAction::None,
        }
    }

    /// Serialize an `IssueFilter` back to the user-facing text syntax so
    /// users can edit an existing filter via `/`.
    fn format_filter_for_input(filter: &crate::store::issues::IssueFilter) -> String {
        let mut out = String::new();
        if let Some(p) = filter.priority {
            out.push_str(&format!("priority={p} "));
        }
        if let Some(s) = filter.status {
            out.push_str(&format!("status={s} "));
        }
        if let Some(label) = &filter.label {
            out.push_str(&format!("label={label} "));
        }
        if let Some(text) = &filter.text {
            out.push_str(&format!("text={text} "));
        }
        out.trim_end().to_string()
    }

    /// Shared s/r/c action keys. Returns `Some(OverlayAction::None)` if the
    /// key matched an action and was handled (so the caller can stop
    /// searching). Returns `None` if the key didn't match an action.
    fn try_action_key(&mut self, code: KeyCode) -> Option<OverlayAction> {
        match code {
            KeyCode::Char('s') => {
                self.dispatch_action("start", |store, id, tx| async move {
                    let r = store
                        .start(id, IssuesPanelOverlay::session_id(), None)
                        .await;
                    let line = match r {
                        Ok(issue) => format!(
                            "Assigned issue #{} to {}",
                            issue.meta.id,
                            IssuesPanelOverlay::session_id()
                        ),
                        Err(e) => format!("start failed: {e}"),
                    };
                    let _ = tx.send(crate::tui::app::UiEvent::SystemMessage(line));
                });
                Some(OverlayAction::None)
            }
            KeyCode::Char('r') => {
                self.dispatch_action("release", |store, id, tx| async move {
                    let r = store
                        .release(id, IssuesPanelOverlay::session_id(), None)
                        .await;
                    let line = match r {
                        Ok(_) => format!("Released issue #{id}"),
                        Err(e) => format!("release failed: {e}"),
                    };
                    let _ = tx.send(crate::tui::app::UiEvent::SystemMessage(line));
                });
                Some(OverlayAction::None)
            }
            KeyCode::Char('c') => {
                self.dispatch_action("close", |store, id, tx| async move {
                    let r = store
                        .close(id, IssuesPanelOverlay::session_id(), None)
                        .await;
                    let line = match r {
                        Ok(issue) => {
                            format!("Closed issue #{}: {}", issue.meta.id, issue.meta.title)
                        }
                        Err(e) => format!("close failed: {e}"),
                    };
                    let _ = tx.send(crate::tui::app::UiEvent::SystemMessage(line));
                });
                Some(OverlayAction::None)
            }
            _ => None,
        }
    }
}
