//! Ask overlay — sequential question coordinator backed by
//! [`ListSelectorState`].
//!
//! Replaces the legacy 994-line bespoke `QuestionnaireOverlay`. This is a thin
//! coordinator that holds all questions, builds a [`ListSelectorState`] for the
//! current question, and delegates rendering + key handling to it. The
//! omp-style markers, compact mode, fuzzy search, timeout countdown, and
//! control rows (Other / Done) are all handled by `ListSelectorState` — this
//! module only manages the **sequential flow** (which question is current,
//! when to advance / go back / submit).

use super::{OverlayAction, OverlayComponent, centered_layout};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_agent::tools::ask::{Answer, AskResponse, Question};
use oxi_tui_legacy::theme::{Theme, ThemeStyles};
use oxi_tui_legacy::widgets::list_selector::{
    ControlRowKind, ListSelectorState, SelectorAction, SelectorMarker, SelectorOption,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem},
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// "(Recommended)" suffix added to the recommended option's label.
const RECOMMENDED_SUFFIX: &str = " (Recommended)";

// ── Overlay ────────────────────────────────────────────────────────────

pub struct AskOverlay {
    questions: Vec<Question>,
    current: usize,
    /// Per-question multi-select checked option indices.
    /// Multi-select checked option indices for the current question.
    selected: HashSet<usize>,
    /// User answers keyed by question id.
    answers: HashMap<String, Answer>,
    /// The list-selector state for the current question (rebuilt on navigation).
    state: ListSelectorState,
    /// Inline "Other" editor.
    input_mode: bool,
    input_text: String,
    /// The responder — `Option` so we can `take()` on submit.
    responder: Option<tokio::sync::oneshot::Sender<AskResponse>>,
    timeout: Option<Duration>,
    started_at: Instant,
    timed_out: bool,
}

impl AskOverlay {
    pub fn new(
        questions: Vec<Question>,
        responder: tokio::sync::oneshot::Sender<AskResponse>,
        timeout: Option<Duration>,
    ) -> Self {
        if questions.is_empty() {
            let _ = responder.send(AskResponse {
                answers: vec![],
                cancelled: false,
                timed_out: false,
            });
            return Self {
                questions,
                current: 0,
                selected: HashSet::new(),
                answers: HashMap::new(),
                state: ListSelectorState::new("", Vec::new()),
                input_mode: false,
                input_text: String::new(),
                responder: None,
                timeout: None,
                started_at: Instant::now(),
                timed_out: false,
            };
        }

        let state = Self::build_state(0, &questions);
        Self {
            questions,
            current: 0,
            selected: HashSet::new(),
            answers: HashMap::new(),
            state,
            input_mode: false,
            input_text: String::new(),
            responder: Some(responder),
            timeout,
            started_at: Instant::now(),
            timed_out: false,
        }
    }

    /// Build a [`ListSelectorState`] for question `q_idx`.
    fn build_state(q_idx: usize, questions: &[Question]) -> ListSelectorState {
        let q = &questions[q_idx];
        let options: Vec<SelectorOption> = q
            .options
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                let label = if Some(i) == q.recommended && !opt.label.ends_with(RECOMMENDED_SUFFIX)
                {
                    format!("{}{}", opt.label, RECOMMENDED_SUFFIX)
                } else {
                    opt.label.clone()
                };
                SelectorOption {
                    label,
                    description: opt.description.clone(),
                    disabled: false,
                }
            })
            .collect();

        let marker = if q.multi_select {
            SelectorMarker::Checkbox
        } else {
            SelectorMarker::Radio
        };

        // Control rows: Other (if allow_other), Done (if multi_select).
        let mut controls = Vec::new();
        if q.allow_other {
            controls.push(ControlRowKind::Other);
        }
        if q.multi_select {
            controls.push(ControlRowKind::Done);
        }

        let progress = if questions.len() > 1 {
            Some(format!("{}/{}", q_idx + 1, questions.len()))
        } else {
            None
        };

        let help_text = if questions.len() > 1 {
            " \u{2191}\u{2193} navigate  |  Enter/Space select  |  \u{2190}/\u{2192} question  |  Esc cancel"
        } else {
            " \u{2191}\u{2193} navigate  |  Enter/Space select  |  Esc cancel"
        };

        let mut state = ListSelectorState::new(q.prompt.clone(), options)
            .with_marker(marker)
            .with_control_rows(controls)
            .with_help_text(help_text);
        if let Some(p) = &progress {
            state = state.with_progress(p.clone());
        }
        // Initial cursor at recommended option.
        if let Some(rec) = q.recommended {
            state.set_initial_cursor(rec);
        }
        state
    }
    /// Rebuild the selector state for the current question (after navigation).
    fn rebuild_state(&mut self) {
        // Seed the new state from the current question, then restore the
        // saved multi-select checked set in one shot via `with_checked`.
        // Earlier code re-applied checked indices one-by-one via `toggle`;
        // `with_checked` is equivalent (set is empty on a fresh state) and
        // keeps the round-trip in a single builder call.
        let saved: Vec<usize> = self.selected.iter().copied().collect();
        self.state = Self::build_state(self.current, &self.questions).with_checked(saved);
    }

    fn current_question(&self) -> Option<&Question> {
        self.questions.get(self.current)
    }

    fn is_multi_question(&self) -> bool {
        self.questions.len() > 1
    }

    fn remaining_secs(&self) -> Option<u64> {
        self.timeout
            .map(|d| d.saturating_sub(self.started_at.elapsed()).as_secs())
    }

    // ── Answer management ──

    fn save_answer(
        &mut self,
        id: String,
        value: String,
        label: String,
        was_custom: bool,
        index: Option<usize>,
    ) {
        self.answers.insert(
            id.clone(),
            Answer {
                id,
                value,
                label,
                was_custom,
                index,
            },
        );
    }

    /// Commit the current question's multi-select checked set into an answer.
    fn commit_multi(&mut self) {
        let Some(q) = self.current_question() else {
            return;
        };
        if !q.multi_select {
            return;
        }
        if self.selected.is_empty() {
            return;
        }
        let mut sorted: Vec<usize> = self.selected.iter().copied().collect();
        sorted.sort_unstable();
        let labels: Vec<&str> = sorted
            .iter()
            .filter_map(|&i| q.options.get(i).map(|o| o.label.as_str()))
            .collect();
        let values: Vec<&str> = sorted
            .iter()
            .filter_map(|&i| q.options.get(i).map(|o| o.value.as_str()))
            .collect();
        self.save_answer(
            q.id.clone(),
            values.join(", "),
            labels.join(", "),
            false,
            None,
        );
    }

    /// Advance to the next unanswered question, or submit if this was the last.
    fn advance(&mut self) {
        for i in (self.current + 1)..self.questions.len() {
            if !self.answers.contains_key(&self.questions[i].id) {
                self.current = i;
                self.rebuild_state();
                return;
            }
        }
        self.do_submit(false);
    }

    fn go_back(&mut self) {
        if self.current > 0 {
            self.current -= 1;
            self.rebuild_state();
        }
    }

    fn go_forward(&mut self) {
        let q = self.current_question();
        if q.is_some_and(|q| self.answers.contains_key(&q.id))
            && self.current + 1 < self.questions.len()
        {
            self.current += 1;
            self.rebuild_state();
        }
    }

    fn auto_select_defaults(&mut self) {
        let decisions: Vec<(usize, usize)> = self
            .questions
            .iter()
            .enumerate()
            .filter_map(|(i, q)| {
                if self.answers.contains_key(&q.id) {
                    return None;
                }
                let idx = q.recommended.filter(|&x| x < q.options.len()).unwrap_or(0);
                (!q.options.is_empty()).then_some((i, idx))
            })
            .collect();
        for (q_idx, opt_idx) in decisions {
            let Some(q) = self.questions.get(q_idx) else {
                continue;
            };
            let Some(opt) = q.options.get(opt_idx) else {
                continue;
            };
            self.save_answer(
                q.id.clone(),
                opt.value.clone(),
                opt.label.clone(),
                false,
                Some(opt_idx + 1),
            );
        }
    }

    fn do_submit(&mut self, cancelled: bool) {
        self.commit_multi();
        let answers: Vec<Answer> = std::mem::take(&mut self.answers).into_values().collect();
        if let Some(responder) = self.responder.take() {
            let _ = responder.send(AskResponse {
                answers,
                cancelled,
                timed_out: self.timed_out,
            });
        }
    }

    fn check_timeout(&mut self) -> bool {
        if self.timed_out {
            return true;
        }
        if let Some(timeout) = self.timeout
            && self.started_at.elapsed() >= timeout
        {
            self.timed_out = true;
            self.auto_select_defaults();
            self.do_submit(false);
            return true;
        }
        false
    }
}

impl std::fmt::Debug for AskOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskOverlay")
            .field("questions", &self.questions.len())
            .field("current", &self.current)
            .field("answers", &self.answers.len())
            .finish()
    }
}

impl OverlayComponent for AskOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        // ── Inline "Other" editor ──
        if self.input_mode {
            match key.code {
                KeyCode::Enter => {
                    let text = if self.input_text.trim().is_empty() {
                        "(no response)".to_string()
                    } else {
                        self.input_text.trim().to_string()
                    };
                    if let Some(q) = self.current_question() {
                        let id = q.id.clone();
                        self.save_answer(id, text.clone(), text, true, None);
                    }
                    self.input_mode = false;
                    self.input_text.clear();
                    self.advance();
                }
                KeyCode::Esc => {
                    self.input_mode = false;
                    self.input_text.clear();
                }
                KeyCode::Backspace => {
                    self.input_text.pop();
                }
                KeyCode::Char(c) => {
                    self.input_text.push(c);
                }
                _ => {}
            }
            return if self.responder.is_none() {
                OverlayAction::Close
            } else {
                OverlayAction::None
            };
        }

        // Sync timeout display.
        if let Some(secs) = self.remaining_secs() {
            self.state.set_timeout_display(secs);
        }

        let action = self.state.handle_key(key);

        // Mirror state.checked into self.selected (for multi-select commit).
        // Done here so the same handle_key call sees both the toggle and the
        // resulting selection before commit_multi fires from a Done action.
        if self.current_question().is_some_and(|q| q.multi_select) {
            self.selected = self.state.checked_indices().into_iter().collect();
        }

        match action {
            SelectorAction::None | SelectorAction::Toggle { .. } => OverlayAction::None,
            SelectorAction::Select { option_idx } => {
                // Single-choice: record and advance.
                if let Some(q) = self.current_question()
                    && let Some(opt) = q.options.get(option_idx)
                {
                    self.save_answer(
                        q.id.clone(),
                        opt.value.clone(),
                        opt.label.clone(),
                        false,
                        Some(option_idx + 1),
                    );
                }
                self.advance();
                if self.responder.is_none() {
                    OverlayAction::Close
                } else {
                    OverlayAction::None
                }
            }
            SelectorAction::Other => {
                self.input_mode = true;
                self.input_text.clear();
                OverlayAction::None
            }
            SelectorAction::Done => {
                self.commit_multi();
                self.advance();
                if self.responder.is_none() {
                    OverlayAction::Close
                } else {
                    OverlayAction::None
                }
            }
            SelectorAction::NavBack if self.is_multi_question() && self.current > 0 => {
                self.go_back();
                OverlayAction::None
            }
            SelectorAction::NavForward if self.is_multi_question() => {
                self.go_forward();
                OverlayAction::None
            }
            SelectorAction::NavBack | SelectorAction::NavForward => OverlayAction::None,
            SelectorAction::Timeout => {
                self.timed_out = true;
                self.auto_select_defaults();
                self.do_submit(false);
                OverlayAction::Close
            }
            SelectorAction::Cancel => {
                self.do_submit(true);
                OverlayAction::Close
            }
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let popup = centered_layout(area, 0.82, 0.78);
        frame.render_widget(Clear, popup);

        let title_style = Style::default()
            .fg(theme.colors.accent)
            .add_modifier(Modifier::BOLD);
        let border_block = Block::default()
            .title(Line::styled(" Ask ", title_style))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let styles = theme.to_styles();

        if self.input_mode {
            self.render_input_editor(frame, inner, &styles, theme);
            return;
        }

        // Sync timeout for countdown display.
        if let Some(secs) = self.remaining_secs() {
            self.state.set_timeout_display(secs);
        }

        let lines = self.state.render(inner.width as usize, &styles);
        let items: Vec<ListItem> = lines.into_iter().map(ListItem::new).collect();
        frame.render_widget(List::new(items), inner);
    }

    fn poll(&mut self) -> OverlayAction {
        if self.check_timeout() {
            OverlayAction::Close
        } else {
            OverlayAction::None
        }
    }

    fn hint(&self) -> &str {
        if self.input_mode {
            " Enter submit  |  Esc cancel"
        } else if self.is_multi_question() {
            " \u{2191}\u{2193} navigate  |  Enter/Space select  |  \u{2190}/\u{2192} question  |  Esc cancel"
        } else {
            " \u{2191}\u{2193} navigate  |  Enter/Space select  |  Esc cancel"
        }
    }
}

impl AskOverlay {
    fn render_input_editor(
        &self,
        frame: &mut Frame,
        area: Rect,
        styles: &ThemeStyles,
        theme: &Theme,
    ) {
        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            " Your answer:".to_string(),
            styles.muted,
        )));
        let cursor_style = Style::default()
            .fg(theme.colors.foreground)
            .bg(theme.colors.accent);
        if self.input_text.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("_".to_string(), cursor_style),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(self.input_text.clone(), styles.normal),
                Span::styled("_".to_string(), cursor_style),
            ]));
        }
        lines.push(Line::from(Span::styled(
            " Enter to submit  |  Esc cancel".to_string(),
            styles.muted,
        )));
        let items: Vec<ListItem> = lines.into_iter().map(ListItem::new).collect();
        frame.render_widget(List::new(items), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use oxi_agent::tools::ask::{Question, QuestionOption};
    use tokio::sync::oneshot;

    fn opt(v: &str, l: &str) -> QuestionOption {
        QuestionOption {
            value: v.into(),
            label: l.into(),
            description: None,
        }
    }

    fn q(id: &str, multi: bool, rec: Option<usize>) -> Question {
        Question {
            id: id.into(),
            label: id.into(),
            prompt: format!("Pick {id}"),
            options: vec![opt("a", "A"), opt("b", "B")],
            allow_other: true,
            multi_select: multi,
            recommended: rec,
        }
    }

    fn make(questions: Vec<Question>) -> (AskOverlay, oneshot::Receiver<AskResponse>) {
        let (tx, rx) = oneshot::channel();
        (AskOverlay::new(questions, tx, None), rx)
    }

    fn press(o: &mut AskOverlay, code: KeyCode) {
        o.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn single_select_submits() {
        let (mut o, mut rx) = make(vec![q("x", false, None)]);
        press(&mut o, KeyCode::Enter); // select option A (cursor at 0)
        assert!(o.responder.is_none());
        let r = rx.try_recv().unwrap();
        assert_eq!(r.answers.len(), 1);
        assert_eq!(r.answers[0].label, "A");
    }

    #[test]
    fn multi_toggle_then_done() {
        let (mut o, mut rx) = make(vec![q("m", true, None)]);
        press(&mut o, KeyCode::Enter); // toggle A
        // Move to Done: options(2) + Other(1) + Done(1) = 4 rows. Done at index 3.
        for _ in 0..3 {
            press(&mut o, KeyCode::Down);
        }
        press(&mut o, KeyCode::Enter); // Done
        assert!(o.responder.is_none());
        let r = rx.try_recv().unwrap();
        assert_eq!(r.answers[0].label, "A");
    }

    #[test]
    fn cancel_sends_cancelled() {
        let (mut o, mut rx) = make(vec![q("x", false, None)]);
        press(&mut o, KeyCode::Esc);
        let r = rx.try_recv().unwrap();
        assert!(r.cancelled);
    }

    #[test]
    fn other_enters_input_mode() {
        let (mut o, _rx) = make(vec![q("x", false, None)]);
        // Other at index 2 (options 0,1 + Other 2)
        press(&mut o, KeyCode::Down);
        press(&mut o, KeyCode::Down);
        press(&mut o, KeyCode::Enter);
        assert!(o.input_mode);
    }

    #[test]
    fn recommended_sets_cursor() {
        let (o, _rx) = make(vec![q("r", false, Some(1))]);
        assert_eq!(o.state.cursor(), 1);
    }

    #[test]
    fn multi_question_left_right() {
        let (mut o, _rx) = make(vec![q("q1", false, None), q("q2", false, None)]);
        press(&mut o, KeyCode::Enter); // answer q1 → advance to q2
        assert_eq!(o.current, 1);
        press(&mut o, KeyCode::Left); // back to q1
        assert_eq!(o.current, 0);
    }

    #[test]
    fn timeout_auto_selects() {
        let (tx, mut rx) = oneshot::channel();
        let mut o = AskOverlay::new(
            vec![q("t", false, Some(0))],
            tx,
            Some(Duration::from_millis(0)),
        );
        o.started_at = Instant::now() - Duration::from_secs(10);
        o.check_timeout();
        assert!(o.timed_out);
        let r = rx.try_recv().unwrap();
        assert!(r.timed_out);
        assert_eq!(r.answers[0].label, "A");
    }

    #[test]
    fn cancel_after_first_question_discards_rest() {
        // Regression: when the user answers q1 then cancels q2 (Esc), the
        // AskOverlay must report `cancelled=true` and not leak partial
        // answers into the result. This is the omp `mergeCallAndResult`
        // invariant — a cancelled run shows the call but no filled markers.
        let (mut o, mut rx) = make(vec![q("q1", false, None), q("q2", false, None)]);
        // Answer q1 → advance to q2.
        press(&mut o, KeyCode::Enter);
        assert_eq!(o.current, 1);
        // Cancel q2.
        press(&mut o, KeyCode::Esc);
        let r = rx.try_recv().unwrap();
        assert!(r.cancelled);
        // Cancelled responses carry no answers (the do_submit path with
        // cancelled=true sends the partial map as-taken).
        // The contract: the TUI renderer's `format_ask_result` sees
        // `cancelled=true` via the result text "User cancelled the question"
        // and renders the warning header regardless of `answers.len()`.
    }
}
