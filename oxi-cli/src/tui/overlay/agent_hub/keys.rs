//! Key dispatch for the Agent Hub overlay.
//!
//! The dispatcher is view-aware: the table view binds row navigation +
//! transcript-entry, the transcript view binds scroll/tail-follow. Both
//! views share `Esc` / `q` (back / close).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::state::{HubState, HubView};

/// Result of handling one key inside the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubAction {
    /// Nothing happened (other key, or navigation that doesn't change view).
    None,
    /// Close the overlay entirely (table view, `Esc`/`q`).
    Close,
    /// Switch into transcript view for the named agent id.
    OpenTranscript(String),
}

/// Page step for `PageDown` / `PageUp` — matches the issues_panel convention.
const PAGE_STEP: usize = 10;

/// Sentinel value stored in `transcript_scroll` when tail-follow is active.
/// The renderer treats it as "show the last `height` lines" rather than a
/// literal line index.
pub const FOLLOW_TAIL: usize = usize::MAX;

/// Top-level dispatch — picks the per-view handler.
pub fn handle_key(state: &mut HubState, key: KeyEvent) -> HubAction {
    // Release events don't drive this overlay.
    if key.kind != KeyEventKind::Press {
        return HubAction::None;
    }
    match &state.view {
        HubView::Table => handle_table_key(state, key),
        HubView::Transcript { .. } => handle_transcript_key(state, key),
    }
}

/// Table view keys: row navigation, Enter to open transcript, close.
fn handle_table_key(state: &mut HubState, key: KeyEvent) -> HubAction {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if !state.rows.is_empty() {
                // Clamp at len-1 so the highlight sits on the last row.
                let max = state.rows.len() - 1;
                if state.selected < max {
                    state.selected += 1;
                }
            }
            HubAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            HubAction::None
        }
        KeyCode::Enter => {
            if let Some(row) = state.rows.get(state.selected) {
                HubAction::OpenTranscript(row.id.clone())
            } else {
                HubAction::None
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => HubAction::Close,
        _ => HubAction::None,
    }
}

/// Transcript view keys: scroll, tail-follow toggle, back to table.
fn handle_transcript_key(state: &mut HubState, key: KeyEvent) -> HubAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.view = HubView::Table;
            // Reset scroll for next transcript visit.
            state.transcript_scroll = 0;
            state.transcript_follow = true;
            HubAction::None
        }
        KeyCode::Char('f') => {
            state.transcript_follow = !state.transcript_follow;
            if state.transcript_follow {
                state.transcript_scroll = FOLLOW_TAIL;
            }
            HubAction::None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            // Manual scroll disables tail-follow so the user can read history.
            // If scroll was the FOLLOW_TAIL sentinel, convert it to a real
            // numeric offset first — saturating_add would otherwise leave
            // us pinned to the sentinel.
            state.transcript_follow = false;
            let base = if state.transcript_scroll == FOLLOW_TAIL {
                0
            } else {
                state.transcript_scroll
            };
            state.transcript_scroll = base.saturating_add(1);
            HubAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            // Allow upward scroll even when tail-follow is on; the renderer
            // clamps the offset to the available history.
            state.transcript_scroll = state.transcript_scroll.saturating_sub(1);
            HubAction::None
        }
        KeyCode::PageDown => {
            state.transcript_follow = false;
            state.transcript_scroll = state.transcript_scroll.saturating_add(PAGE_STEP);
            HubAction::None
        }
        KeyCode::PageUp => {
            state.transcript_scroll = state.transcript_scroll.saturating_sub(PAGE_STEP);
            HubAction::None
        }
        KeyCode::Char('G') => {
            // Jump to end and re-engage tail-follow.
            state.transcript_scroll = FOLLOW_TAIL;
            state.transcript_follow = true;
            HubAction::None
        }
        KeyCode::Char('g') => {
            // Jump to start; tail-follow disengages (user is paging history).
            state.transcript_scroll = 0;
            state.transcript_follow = false;
            HubAction::None
        }
        _ => HubAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::agent_hub::state::HubRow;
    use crossterm::event::KeyModifiers;
    use oxi_sdk::{HubKind, HubStatus};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn press(code: KeyCode) -> KeyEvent {
        let mut k = KeyEvent::new(code, KeyModifiers::NONE);
        k.kind = KeyEventKind::Press;
        k
    }

    fn state_with_rows(n: usize) -> HubState {
        let rows = (0..n)
            .map(|i| HubRow {
                id: format!("agent-{i}"),
                kind: HubKind::Subagent,
                status: HubStatus::Running,
                display_name: format!("agent-{i}"),
                current_task: None,
                age_text: "0s ago".into(),
                session_file: Some(PathBuf::from("/tmp/x.jsonl")),
            })
            .collect();
        HubState {
            rows,
            view: HubView::Table,
            selected: 0,
            row_order: HashMap::new(),
            transcript_scroll: 0,
            transcript_follow: true,
        }
    }

    #[test]
    fn j_moves_selection_down_and_clamps_at_last() {
        let mut s = state_with_rows(2);
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('j'))),
            HubAction::None
        );
        assert_eq!(s.selected, 1);
        // Second j clamps at len-1 (no wrap).
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('j'))),
            HubAction::None
        );
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn down_arrow_moves_selection() {
        let mut s = state_with_rows(3);
        assert_eq!(handle_key(&mut s, press(KeyCode::Down)), HubAction::None);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn k_moves_selection_up_and_saturates_at_zero() {
        let mut s = state_with_rows(2);
        s.selected = 1;
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('k'))),
            HubAction::None
        );
        assert_eq!(s.selected, 0);
        // Past the top stays at 0.
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('k'))),
            HubAction::None
        );
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn up_arrow_moves_selection_up() {
        let mut s = state_with_rows(2);
        s.selected = 1;
        assert_eq!(handle_key(&mut s, press(KeyCode::Up)), HubAction::None);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn enter_opens_transcript_for_selected_row() {
        let mut s = state_with_rows(3);
        s.selected = 1;
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Enter)),
            HubAction::OpenTranscript("agent-1".into())
        );
    }

    #[test]
    fn esc_closes_from_table() {
        let mut s = state_with_rows(1);
        assert_eq!(handle_key(&mut s, press(KeyCode::Esc)), HubAction::Close);
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('q'))),
            HubAction::Close
        );
    }

    #[test]
    fn esc_returns_to_table_from_transcript() {
        let mut s = state_with_rows(2);
        s.view = HubView::Transcript {
            agent_id: "agent-0".into(),
        };
        assert_eq!(handle_key(&mut s, press(KeyCode::Esc)), HubAction::None);
        assert!(matches!(s.view, HubView::Table));
    }

    #[test]
    fn q_returns_to_table_from_transcript() {
        let mut s = state_with_rows(2);
        s.view = HubView::Transcript {
            agent_id: "agent-0".into(),
        };
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('q'))),
            HubAction::None
        );
        assert!(matches!(s.view, HubView::Table));
    }

    #[test]
    fn f_toggles_tail_follow_and_j_disables_it() {
        let mut s = state_with_rows(1);
        s.view = HubView::Transcript {
            agent_id: "agent-0".into(),
        };
        s.transcript_follow = false;
        s.transcript_scroll = 0;
        // `f` re-enables tail-follow and uses the sentinel.
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('f'))),
            HubAction::None
        );
        assert!(s.transcript_follow);
        assert_eq!(s.transcript_scroll, FOLLOW_TAIL);
        // `j` manually scrolls and disables tail-follow.
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('j'))),
            HubAction::None
        );
        assert!(!s.transcript_follow);
        assert_eq!(s.transcript_scroll, 1);
    }

    #[test]
    fn g_jumps_to_start_and_disables_follow() {
        let mut s = state_with_rows(1);
        s.view = HubView::Transcript {
            agent_id: "agent-0".into(),
        };
        s.transcript_scroll = 5;
        s.transcript_follow = true;
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('g'))),
            HubAction::None
        );
        assert_eq!(s.transcript_scroll, 0);
        assert!(!s.transcript_follow);
    }

    #[test]
    fn capital_g_jumps_to_tail_and_reenables_follow() {
        let mut s = state_with_rows(1);
        s.view = HubView::Transcript {
            agent_id: "agent-0".into(),
        };
        s.transcript_scroll = 0;
        s.transcript_follow = false;
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('G'))),
            HubAction::None
        );
        assert!(s.transcript_follow);
        assert_eq!(s.transcript_scroll, FOLLOW_TAIL);
    }

    #[test]
    fn page_keys_step_by_page() {
        let mut s = state_with_rows(1);
        s.view = HubView::Transcript {
            agent_id: "agent-0".into(),
        };
        s.transcript_follow = false;
        s.transcript_scroll = 0;
        assert_eq!(
            handle_key(&mut s, press(KeyCode::PageDown)),
            HubAction::None
        );
        assert_eq!(s.transcript_scroll, PAGE_STEP);
        assert_eq!(handle_key(&mut s, press(KeyCode::PageUp)), HubAction::None);
        assert_eq!(s.transcript_scroll, 0);
    }

    #[test]
    fn release_event_is_ignored() {
        let mut s = state_with_rows(3);
        s.selected = 1;
        let mut k = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        k.kind = KeyEventKind::Release;
        assert_eq!(handle_key(&mut s, k), HubAction::None);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn empty_rows_j_does_not_move() {
        let mut s = state_with_rows(0);
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('j'))),
            HubAction::None
        );
        assert_eq!(s.selected, 0);
    }
}
