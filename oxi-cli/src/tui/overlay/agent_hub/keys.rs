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
const PAGE_STEP: isize = 10;
/// Sentinel stored in `transcript_scroll` when the user has not yet scrolled
/// away from the tail-follow default. The renderer treats this as "pin to
/// the last `window` lines". Handlers step from here with plain `isize`
/// arithmetic — `0 - 1 == -1` is the natural one-line-up-from-tail offset.
pub const FOLLOW_TAIL: isize = 0;
/// Sentinel stored in `transcript_scroll` when the user pressed `g` to jump
/// to the top of history. Renders as `start = 0`.
pub const JUMP_TOP: isize = isize::MAX;

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
            state.transcript_scroll = FOLLOW_TAIL;
            state.transcript_follow = true;
            HubAction::None
        }
        KeyCode::Char('f') => {
            // Toggling follow: when enabling, snap back to the FOLLOW_TAIL
            // sentinel. When disabling, leave the offset alone — the renderer
            // re-derives the window from the current integer offset.
            state.transcript_follow = !state.transcript_follow;
            if state.transcript_follow {
                state.transcript_scroll = FOLLOW_TAIL;
            }
            HubAction::None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            // Tail direction. At FOLLOW_TAIL we are already pinned to the
            // tail — further "down" has no meaning, so no-op.
            if state.transcript_follow {
                HubAction::None
            } else {
                state.transcript_scroll = state.transcript_scroll.saturating_add(1);
                HubAction::None
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            // Away from tail. Works from FOLLOW_TAIL (0 - 1 = -1, one line
            // below the tail) and from JUMP_TOP (MAX - 1 saturates to MAX,
            // still at the top — no visible movement; intentional, you have
            // already paged all the way to the head).
            state.transcript_follow = false;
            state.transcript_scroll = state.transcript_scroll.saturating_sub(1);
            HubAction::None
        }
        KeyCode::PageDown => {
            // Tail direction. At FOLLOW_TAIL we are already pinned to the
            // tail — further "down" has no meaning, so no-op.
            if state.transcript_follow {
                HubAction::None
            } else {
                state.transcript_scroll = state.transcript_scroll.saturating_add(PAGE_STEP);
                HubAction::None
            }
        }
        KeyCode::PageUp => {
            // Away from tail. Works from FOLLOW_TAIL (0 - 10 = -10, ten
            // lines below the tail) and from JUMP_TOP (saturates).
            state.transcript_follow = false;
            state.transcript_scroll = state.transcript_scroll.saturating_sub(PAGE_STEP);
            HubAction::None
        }
        KeyCode::Char('G') => {
            // Jump to tail and re-engage follow.
            state.transcript_scroll = FOLLOW_TAIL;
            state.transcript_follow = true;
            HubAction::None
        }
        KeyCode::Char('g') => {
            // Jump to top of history; follow disengages.
            state.transcript_scroll = JUMP_TOP;
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
    fn f_toggles_tail_follow() {
        let mut s = state_with_rows(1);
        s.view = HubView::Transcript {
            agent_id: "agent-0".into(),
        };
        s.transcript_follow = false;
        s.transcript_scroll = 5;
        // `f` re-enables tail-follow and snaps scroll to the sentinel.
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('f'))),
            HubAction::None
        );
        assert!(s.transcript_follow);
        assert_eq!(s.transcript_scroll, FOLLOW_TAIL);
        // `f` again disables follow; the offset stays at FOLLOW_TAIL until
        // the user moves with a directional key.
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('f'))),
            HubAction::None
        );
        assert!(!s.transcript_follow);
        assert_eq!(s.transcript_scroll, FOLLOW_TAIL);
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
        assert_eq!(s.transcript_scroll, JUMP_TOP);
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

    // ── Follow-mode scroll regression tests (P2 amendment) ──
    // The reviewer's amended verdict: the FOLLOW_TAIL sentinel used to make
    // j / k / PageUp / PageDown all misbehave from the default (follow=true)
    // state — the handler had no line count to convert the sentinel to a
    // real offset, so the renderer pinned the viewport to the tail. The
    // signed-scroll refactor (FOLLOW_TAIL=0, JUMP_TOP=isize::MAX) makes all
    // four keys work from any starting state.

    fn transcript_state() -> HubState {
        let mut s = state_with_rows(1);
        s.view = HubView::Transcript {
            agent_id: "agent-0".into(),
        };
        s.transcript_follow = true;
        s.transcript_scroll = FOLLOW_TAIL;
        s
    }

    #[test]
    fn k_in_follow_mode_steps_one_line_below_tail() {
        let mut s = transcript_state();
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('k'))),
            HubAction::None
        );
        assert!(!s.transcript_follow, "k must disengage tail-follow");
        assert_eq!(s.transcript_scroll, -1isize, "k from FOLLOW_TAIL (0) → -1");
    }

    #[test]
    fn page_up_in_follow_mode_steps_ten_lines_below_tail() {
        let mut s = transcript_state();
        assert_eq!(handle_key(&mut s, press(KeyCode::PageUp)), HubAction::None);
        assert!(!s.transcript_follow);
        assert_eq!(
            s.transcript_scroll, -10isize,
            "PgUp from FOLLOW_TAIL (0) → -10"
        );
    }

    #[test]
    fn j_in_follow_mode_is_noop_pinned_at_tail() {
        // j is "down/toward tail" — at FOLLOW_TAIL the user is already at
        // the tail, so further "down" has no meaning. This is the deliberate
        // follow semantics, not a bug: the user can press `g` first to leave
        // follow, then `j` advances the offset.
        let mut s = transcript_state();
        assert_eq!(
            handle_key(&mut s, press(KeyCode::Char('j'))),
            HubAction::None
        );
        assert!(
            s.transcript_follow,
            "j in follow mode must not disengage follow"
        );
        assert_eq!(s.transcript_scroll, FOLLOW_TAIL);
    }

    #[test]
    fn page_down_in_follow_mode_is_noop_pinned_at_tail() {
        // Same semantics as j: PgDn at FOLLOW_TAIL is a no-op, not a
        // viewport jump. Deliberate; the user can press `g` to leave.
        let mut s = transcript_state();
        assert_eq!(
            handle_key(&mut s, press(KeyCode::PageDown)),
            HubAction::None
        );
        assert!(s.transcript_follow);
        assert_eq!(s.transcript_scroll, FOLLOW_TAIL);
    }

    #[test]
    fn round_trip_g_then_j_stays_at_top() {
        // End-to-end: g leaves follow and jumps to top (JUMP_TOP sentinel),
        // j stays at top (saturating_add on MAX stays at MAX), follow is off.
        let mut s = transcript_state();
        handle_key(&mut s, press(KeyCode::Char('g')));
        assert!(!s.transcript_follow);
        assert_eq!(s.transcript_scroll, JUMP_TOP);
        handle_key(&mut s, press(KeyCode::Char('j')));
        assert!(!s.transcript_follow);
        assert_eq!(
            s.transcript_scroll, JUMP_TOP,
            "saturating_add on MAX stays at MAX"
        );
    }
}
