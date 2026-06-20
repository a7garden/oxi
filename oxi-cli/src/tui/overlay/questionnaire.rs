//! Questionnaire overlay — interactive multi-question TUI component.
//!
//! Implements `OverlayComponent` to display one or more questions
//! with options, tabs, and inline text input (allowOther).

use super::{OverlayAction, OverlayComponent, centered_layout};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_agent::tools::questionnaire::{Answer, Question, QuestionnaireResponse};
use oxi_tui::theme::{Theme, ThemeStyles};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

// ── Overlay ────────────────────────────────────────────────────────────

pub struct QuestionnaireOverlay {
    questions: Vec<Question>,
    /// Current active tab index (0..questions.len()), len = Submit tab.
    current_tab: usize,
    /// Cursor position within the options list for the current tab.
    option_cursor: usize,
    /// Multi-select: for each tab index, the set of selected option indices.
    selected: HashMap<usize, HashSet<usize>>,
    /// User answers keyed by question id.
    answers: HashMap<String, Answer>,
    /// Whether we're in the "Type something..." inline editor mode.
    input_mode: bool,
    /// Text in the inline editor.
    input_text: String,
    /// The question id for which input_mode is active.
    input_question_id: String,
    /// The responder — Option so we can take() it on submit.
    responder: Option<tokio::sync::oneshot::Sender<QuestionnaireResponse>>,
    /// Overlay timeout. `None` = disabled.
    timeout: Option<Duration>,
    /// When the overlay was created (for timeout countdown).
    started_at: Instant,
    /// Whether answers were auto-selected due to timeout.
    timed_out: bool,
}

impl QuestionnaireOverlay {
    pub fn new(
        questions: Vec<Question>,
        responder: tokio::sync::oneshot::Sender<QuestionnaireResponse>,
        timeout: Option<Duration>,
    ) -> Self {
        // Initial cursor: recommended option of the first question
        let initial_cursor = questions
            .first()
            .and_then(|q| q.recommended)
            .filter(|&idx| idx < questions[0].options.len())
            .unwrap_or(0);

        if questions.is_empty() {
            let _ = responder.send(QuestionnaireResponse {
                answers: vec![],
                cancelled: false,
                timed_out: false,
            });
            return Self {
                questions,
                current_tab: 0,
                option_cursor: 0,
                selected: HashMap::new(),
                answers: HashMap::new(),
                input_mode: false,
                input_text: String::new(),
                input_question_id: String::new(),
                responder: None,
                timeout: None,
                started_at: Instant::now(),
                timed_out: false,
            };
        }

        Self {
            questions,
            current_tab: 0,
            option_cursor: initial_cursor,
            selected: HashMap::new(),
            answers: HashMap::new(),
            input_mode: false,
            input_text: String::new(),
            input_question_id: String::new(),
            responder: Some(responder),
            timeout,
            started_at: Instant::now(),
            timed_out: false,
        }
    }

    fn is_multi(&self) -> bool {
        self.questions.len() > 1
    }

    fn submit_tab(&self) -> usize {
        self.questions.len()
    }

    fn total_tabs(&self) -> usize {
        self.questions.len() + 1
    }

    fn current_question(&self) -> Option<&Question> {
        self.questions.get(self.current_tab)
    }

    fn all_answered(&self) -> bool {
        self.questions.iter().enumerate().all(|(i, q)| {
            self.answers.contains_key(&q.id)
                || (q.multi_select && self.selected.get(&i).is_some_and(|s| !s.is_empty()))
        })
    }

    fn toggle_multi(&mut self, tab_idx: usize, opt_idx: usize) {
        let entry = self.selected.entry(tab_idx).or_default();
        if entry.contains(&opt_idx) {
            entry.remove(&opt_idx);
        } else {
            entry.insert(opt_idx);
        }
    }

    fn save_answer(
        &mut self,
        question_id: String,
        value: String,
        label: String,
        was_custom: bool,
        index: Option<usize>,
    ) {
        self.answers.insert(
            question_id.clone(),
            Answer {
                id: question_id,
                value,
                label,
                was_custom,
                index,
            },
        );
    }

    fn advance(&mut self) {
        if !self.is_multi() {
            self.do_submit(false);
            return;
        }

        for i in (self.current_tab + 1)..self.total_tabs() {
            if i >= self.submit_tab() {
                self.current_tab = self.submit_tab();
                self.option_cursor = self.cursor_for_tab(self.submit_tab());
                return;
            }
            if !self.answers.contains_key(&self.questions[i].id) {
                self.current_tab = i;
                self.option_cursor = self.cursor_for_tab(i);
                return;
            }
        }
        self.current_tab = self.submit_tab();
        self.option_cursor = 0;
    }

    /// Compute the initial cursor for a tab: the recommended option index if valid.
    fn cursor_for_tab(&self, tab_idx: usize) -> usize {
        self.questions
            .get(tab_idx)
            .and_then(|q| q.recommended)
            .filter(|&idx| {
                self.questions
                    .get(tab_idx)
                    .is_some_and(|q| idx < q.options.len())
            })
            .unwrap_or(0)
    }

    /// Convert multi-select `self.selected` entries into `self.answers`.
    /// Must be called before `do_submit` and before checking `all_answered`
    /// from mutation contexts.
    fn convert_selected_to_answers(&mut self) {
        let tab_indices: Vec<usize> = self.selected.keys().copied().collect();
        for tab_idx in tab_indices {
            let Some(q) = self.questions.get(tab_idx) else {
                continue;
            };
            if !q.multi_select {
                continue;
            }
            let Some(indices) = self.selected.get(&tab_idx) else {
                continue;
            };
            if indices.is_empty() {
                continue;
            }
            // Collect sorted for deterministic order
            let mut sorted: Vec<usize> = indices.iter().copied().collect();
            sorted.sort_unstable();
            let labels: Vec<&str> = sorted
                .iter()
                .filter_map(|&i| q.options.get(i).map(|o| o.label.as_str()))
                .collect();
            let values: Vec<&str> = sorted
                .iter()
                .filter_map(|&i| q.options.get(i).map(|o| o.value.as_str()))
                .collect();
            self.answers.insert(
                q.id.clone(),
                Answer {
                    id: q.id.clone(),
                    value: values.join(", "),
                    label: labels.join(", "),
                    was_custom: false,
                    index: None,
                },
            );
        }
    }

    /// Auto-select defaults (recommended or first option) for unanswered questions.
    fn auto_select_defaults(&mut self) {
        // Collect decisions first (immutable borrow phase)
        let multi_decisions: Vec<(usize, usize)> = self
            .questions
            .iter()
            .enumerate()
            .filter_map(|(i, q)| {
                if self.answers.contains_key(&q.id) {
                    return None;
                }
                if q.multi_select && self.selected.get(&i).is_some_and(|s| !s.is_empty()) {
                    return None;
                }
                let default_idx = q
                    .recommended
                    .filter(|&idx| idx < q.options.len())
                    .unwrap_or(0);
                if q.multi_select {
                    Some((i, default_idx))
                } else {
                    None
                }
            })
            .collect();

        let single_decisions: Vec<(String, String, String, usize)> = self
            .questions
            .iter()
            .enumerate()
            .filter_map(|(i, q)| {
                if self.answers.contains_key(&q.id) {
                    return None;
                }
                if q.multi_select && self.selected.get(&i).is_some_and(|s| !s.is_empty()) {
                    return None;
                }
                let default_idx = q
                    .recommended
                    .filter(|&idx| idx < q.options.len())
                    .unwrap_or(0);
                if !q.multi_select {
                    q.options.get(default_idx).map(|opt| {
                        (
                            q.id.clone(),
                            opt.value.clone(),
                            opt.label.clone(),
                            default_idx + 1,
                        )
                    })
                } else {
                    None
                }
            })
            .collect();

        // Apply decisions (mutable borrow phase)
        for (i, default_idx) in multi_decisions {
            self.selected.entry(i).or_default().insert(default_idx);
        }
        for (id, value, label, rec_idx) in single_decisions {
            self.save_answer(id, value, label, false, Some(rec_idx));
        }
        self.convert_selected_to_answers();
    }

    /// Remaining seconds before timeout, or `None` if no timeout.
    fn remaining_secs(&self) -> Option<u64> {
        self.timeout
            .map(|dur| dur.saturating_sub(self.started_at.elapsed()).as_secs())
    }

    fn do_submit(&mut self, cancelled: bool) {
        self.convert_selected_to_answers();
        let answers: Vec<Answer> = std::mem::take(&mut self.answers).into_values().collect();
        if let Some(responder) = self.responder.take() {
            let _ = responder.send(QuestionnaireResponse {
                answers,
                cancelled,
                timed_out: self.timed_out,
            });
        }
    }
}

impl std::fmt::Debug for QuestionnaireOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestionnaireOverlay")
            .field("questions", &self.questions.len())
            .field("current_tab", &self.current_tab)
            .field("input_mode", &self.input_mode)
            .finish()
    }
}

/// Timeout check: called every ~50ms by the main loop.
/// No-op when timeout is disabled. When it fires, auto-selects defaults
/// and submits, returning Close to dismiss the overlay.
impl QuestionnaireOverlay {
    fn check_timeout(&mut self) -> bool {
        if self.timed_out {
            return true;
        }
        if let Some(timeout) = self.timeout {
            if self.started_at.elapsed() >= timeout {
                self.timed_out = true;
                self.auto_select_defaults();
                self.do_submit(false);
                return true;
            }
        }
        false
    }
}

impl OverlayComponent for QuestionnaireOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        // ── Input mode ──
        if self.input_mode {
            match key.code {
                KeyCode::Enter => {
                    let text = if self.input_text.trim().is_empty() {
                        "(no response)".to_string()
                    } else {
                        self.input_text.trim().to_string()
                    };
                    let q_id = std::mem::take(&mut self.input_question_id);
                    self.save_answer(q_id, text.clone(), text, true, None);
                    self.input_mode = false;
                    self.input_text.clear();
                    self.advance();
                }
                KeyCode::Esc => {
                    self.input_mode = false;
                    self.input_text.clear();
                    let _ = std::mem::take(&mut self.input_question_id);
                }
                KeyCode::Backspace => {
                    self.input_text.pop();
                }
                KeyCode::Char(c) => {
                    self.input_text.push(c);
                }
                _ => {}
            }
            return OverlayAction::None;
        }

        // ── Tab navigation ──
        if self.is_multi() {
            match key.code {
                KeyCode::Tab | KeyCode::Right => {
                    self.current_tab = (self.current_tab + 1) % self.total_tabs();
                    self.option_cursor = self.cursor_for_tab(self.current_tab);
                    return OverlayAction::None;
                }
                KeyCode::BackTab | KeyCode::Left => {
                    self.current_tab =
                        (self.current_tab + self.total_tabs() - 1) % self.total_tabs();
                    self.option_cursor = self.cursor_for_tab(self.current_tab);
                    return OverlayAction::None;
                }
                _ => {}
            }
        }

        // ── Submit tab ──
        if self.current_tab == self.submit_tab() {
            match key.code {
                KeyCode::Enter if self.all_answered() => {
                    self.do_submit(false);
                    return OverlayAction::Close;
                }
                KeyCode::Esc => {
                    self.do_submit(true);
                    return OverlayAction::Close;
                }
                _ => return OverlayAction::None,
            }
        }

        // ── Question tab ──
        let q = match self.current_question() {
            Some(q) => q,
            None => return OverlayAction::None,
        };
        let opt_count = q.options.len() + if q.allow_other { 1 } else { 0 };
        let q_id = q.id.clone();

        match key.code {
            KeyCode::Up => {
                self.option_cursor = self.option_cursor.saturating_sub(1);
            }
            KeyCode::Down if opt_count > 0 => {
                self.option_cursor = (self.option_cursor + 1).min(opt_count - 1);
            }
            KeyCode::Enter => {
                let sel = self.option_cursor;
                if sel < q.options.len() {
                    // Regular option
                    let opt = &q.options[sel];
                    if q.multi_select {
                        self.toggle_multi(self.current_tab, sel);
                    } else {
                        self.save_answer(
                            q_id,
                            opt.value.clone(),
                            opt.label.clone(),
                            false,
                            Some(sel + 1),
                        );
                        self.advance();
                    }
                } else if q.allow_other {
                    // "Type something..."
                    self.input_mode = true;
                    self.input_question_id = q_id;
                    self.input_text.clear();
                }
            }
            KeyCode::Char(' ') if q.multi_select => {
                let sel = self.option_cursor;
                if sel < q.options.len() {
                    self.toggle_multi(self.current_tab, sel);
                }
            }
            KeyCode::Esc => {
                self.do_submit(true);
                return OverlayAction::Close;
            }
            _ => {}
        }

        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let popup = centered_layout(area, 0.85, 0.80);

        frame.render_widget(Clear, popup);

        let title_style = Style::default()
            .fg(theme.colors.primary)
            .add_modifier(Modifier::BOLD);

        let border_block = Block::default()
            .title(Line::styled(" Questionnaire ", title_style))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));

        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        self.render_content(frame, inner, theme);
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
        } else if self.is_multi() {
            " Tab/←→ tabs  |  ↑↓ navigate  |  Enter select  |  Esc cancel"
        } else {
            " ↑↓ navigate  |  Enter select  |  Esc cancel"
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

impl QuestionnaireOverlay {
    fn render_content(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let lines = self.build_lines(area.width, theme, &styles);

        let list_items: Vec<ListItem> = lines.into_iter().map(ListItem::new).collect();

        frame.render_widget(List::new(list_items), area);

        let hint_y = area.y + area.height.saturating_sub(1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(self.hint(), styles.muted))),
            Rect {
                x: area.x,
                y: hint_y,
                width: area.width,
                height: 1,
            },
        );
    }

    fn build_lines(&self, width: u16, theme: &Theme, styles: &ThemeStyles) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let sep_line = "─".repeat(width as usize);
        let fg = theme.colors.accent;

        if self.input_mode {
            self.build_input_mode_lines(&mut lines, width, theme, styles, &sep_line);
        } else if self.current_tab == self.submit_tab() {
            self.build_submit_lines(&mut lines, theme, styles, &sep_line);
        } else {
            self.build_question_lines(&mut lines, theme, styles, &sep_line);
        }

        lines.push(Line::from(Span::styled(
            sep_line.clone(),
            Style::default().fg(fg),
        )));
        lines
    }

    fn build_question_lines(
        &self,
        lines: &mut Vec<Line<'static>>,
        theme: &Theme,
        styles: &ThemeStyles,
        sep_line: &str,
    ) {
        let q = match self.current_question() {
            Some(q) => q,
            None => return,
        };
        let fg = theme.colors.accent;

        lines.push(Line::from(Span::styled(
            sep_line.to_string(),
            Style::default().fg(fg),
        )));

        if self.is_multi() {
            self.build_tab_bar(lines, theme, styles);
            lines.push(Line::from(Span::raw("")));
        }

        lines.push(Line::from(vec![
            Span::styled(" ? ", styles.accent.add_modifier(Modifier::BOLD)),
            Span::styled(q.prompt.clone(), styles.normal),
        ]));
        // Countdown timer line
        if let Some(secs) = self.remaining_secs() {
            let timer_style = if secs <= 5 {
                styles.warning.add_modifier(Modifier::BOLD)
            } else {
                styles.muted
            };
            lines.push(Line::from(Span::styled(
                format!(" ⏱ {}s remaining", secs),
                timer_style,
            )));
        }
        lines.push(Line::from(Span::styled(
            sep_line.to_string(),
            Style::default().fg(fg),
        )));

        let multi_selected = self.selected.get(&self.current_tab);
        let opt_count = q.options.len() + if q.allow_other { 1 } else { 0 };

        for i in 0..opt_count {
            let is_sel = i == self.option_cursor;
            let pointer = if is_sel { "→ " } else { "  " };
            let pointer_style = if is_sel { styles.accent } else { styles.normal };

            if i < q.options.len() {
                let opt = &q.options[i];
                let check = if q.multi_select {
                    let is_checked = multi_selected.map(|s| s.contains(&i)).unwrap_or(false);
                    Some(if is_checked { "☑" } else { "☐" })
                } else {
                    None
                };
                let check_str = check.map(|c| format!("{} ", c)).unwrap_or_default();

                lines.push(Line::from(vec![
                    Span::styled(pointer.to_string(), pointer_style),
                    Span::styled(
                        format!("{}. {}{}", i + 1, check_str, opt.label),
                        if is_sel { styles.accent } else { styles.normal },
                    ),
                    // Recommended marker
                    if Some(i) == q.recommended {
                        Span::styled(" ★", styles.warning)
                    } else {
                        Span::raw("")
                    },
                ]));

                if let Some(ref desc) = opt.description {
                    lines.push(Line::from(vec![
                        Span::styled("     ".to_string(), styles.muted),
                        Span::styled(desc.clone(), styles.muted),
                    ]));
                }
            } else {
                // "Type something..."
                lines.push(Line::from(vec![
                    Span::styled(pointer.to_string(), pointer_style),
                    Span::styled(format!("{}. ", i + 1), styles.accent),
                    Span::styled(
                        "Type something...".to_string(),
                        styles.accent.add_modifier(Modifier::DIM),
                    ),
                    Span::styled(" ✎".to_string(), styles.accent),
                ]));
            }
        }
    }

    fn build_submit_lines(
        &self,
        lines: &mut Vec<Line<'static>>,
        theme: &Theme,
        styles: &ThemeStyles,
        sep_line: &str,
    ) {
        let fg = theme.colors.accent;

        lines.push(Line::from(Span::styled(
            sep_line.to_string(),
            Style::default().fg(fg),
        )));

        if self.is_multi() {
            self.build_tab_bar(lines, theme, styles);
            lines.push(Line::from(Span::raw("")));
        }

        if self.all_answered() {
            lines.push(Line::from(vec![
                Span::styled(
                    " ✓ ".to_string(),
                    styles.success.add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "Ready to submit".to_string(),
                    styles.success.add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            let missing: Vec<String> = self
                .questions
                .iter()
                .filter(|q| !self.answers.contains_key(&q.id))
                .map(|q| q.label.clone())
                .collect();
            lines.push(Line::from(vec![
                Span::styled(
                    " ⚠ ".to_string(),
                    styles.warning.add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("Unanswered: {}", missing.join(", ")),
                    styles.warning,
                ),
            ]));
        }
        lines.push(Line::from(Span::raw("")));

        for q in &self.questions {
            if let Some(ans) = self.answers.get(&q.id) {
                let label = if ans.was_custom {
                    format!("(wrote) {}", ans.label)
                } else if let Some(idx) = ans.index {
                    format!("{}. {}", idx, ans.label)
                } else {
                    ans.label.clone()
                };

                lines.push(Line::from(vec![
                    Span::styled(format!(" {:>12} ", q.label), styles.muted),
                    Span::styled(label, styles.normal),
                ]));
            }
        }

        if self.all_answered() {
            lines.push(Line::from(Span::styled(
                " Press Enter to submit".to_string(),
                styles.success,
            )));
        }
    }

    fn build_input_mode_lines(
        &self,
        lines: &mut Vec<Line<'static>>,
        width: u16,
        theme: &Theme,
        styles: &ThemeStyles,
        sep_line: &str,
    ) {
        let q = match self.current_question() {
            Some(q) => q,
            None => return,
        };
        let fg = theme.colors.accent;

        lines.push(Line::from(Span::styled(
            sep_line.to_string(),
            Style::default().fg(fg),
        )));

        lines.push(Line::from(vec![
            Span::styled(
                " ? ".to_string(),
                styles.accent.add_modifier(Modifier::BOLD),
            ),
            Span::styled(q.prompt.clone(), styles.normal),
        ]));
        lines.push(Line::from(Span::styled(
            sep_line.to_string(),
            Style::default().fg(fg),
        )));

        for (i, opt) in q.options.iter().enumerate() {
            let is_sel = i == self.option_cursor;
            lines.push(Line::from(vec![
                Span::styled(if is_sel { "→ " } else { "  " }, styles.accent),
                Span::styled(
                    format!("{}. {}", i + 1, opt.label),
                    if is_sel { styles.accent } else { styles.normal },
                ),
            ]));
        }

        lines.push(Line::from(Span::styled(
            sep_line.to_string(),
            Style::default().fg(fg),
        )));
        lines.push(Line::from(Span::styled(
            " Your answer:".to_string(),
            styles.muted,
        )));

        let max_input_w = (width as usize).saturating_sub(4);
        let display = self
            .input_text
            .chars()
            .take(max_input_w)
            .collect::<String>();
        let cursor_style = Style::default()
            .fg(theme.colors.foreground)
            .bg(theme.colors.primary);

        if display.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  ".to_string()),
                Span::styled("_".to_string(), cursor_style),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw("  ".to_string()),
                Span::styled(display, styles.normal),
                Span::styled("_".to_string(), cursor_style),
            ]));
        }

        lines.push(Line::from(Span::styled(
            " Enter to submit  |  Esc cancel".to_string(),
            styles.muted,
        )));
    }

    fn build_tab_bar(&self, lines: &mut Vec<Line<'static>>, _theme: &Theme, styles: &ThemeStyles) {
        let left_style = if self.current_tab == 0 {
            styles.muted
        } else {
            styles.normal
        };
        lines.push(Line::from(vec![Span::styled("← ".to_string(), left_style)]));

        for (i, q) in self.questions.iter().enumerate() {
            let is_active = i == self.current_tab;
            let is_done = self.answers.contains_key(&q.id);
            let box_char = if is_done { "■" } else { "□" };

            let text_style = if is_active {
                styles.normal
            } else if is_done {
                styles.success
            } else {
                styles.muted
            };

            lines.push(Line::from(vec![Span::styled(
                format!("{} {} ", box_char, q.label),
                text_style,
            )]));
        }

        let can_submit = self.all_answered();
        let (submit_char, submit_style) = if can_submit {
            ("✓", styles.success)
        } else {
            ("·", styles.muted)
        };
        let is_submit_active = self.current_tab == self.submit_tab();
        lines.push(Line::from(vec![Span::styled(
            format!("{} Submit ", submit_char),
            submit_style,
        )]));

        let right_style = if is_submit_active {
            styles.muted
        } else {
            styles.normal
        };
        lines.push(Line::from(vec![Span::styled(
            " →".to_string(),
            right_style,
        )]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_agent::tools::questionnaire::{Question, QuestionOption, QuestionnaireResponse};
    use tokio::sync::oneshot;

    fn make_question(id: &str, multi: bool, recommended: Option<usize>) -> Question {
        Question {
            id: id.into(),
            label: id.into(),
            prompt: format!("Choose {}?", id),
            options: vec![
                QuestionOption {
                    value: "a".into(),
                    label: "Option A".into(),
                    description: None,
                },
                QuestionOption {
                    value: "b".into(),
                    label: "Option B".into(),
                    description: None,
                },
            ],
            allow_other: false,
            multi_select: multi,
            recommended,
        }
    }

    fn make_overlay(
        questions: Vec<Question>,
    ) -> (
        QuestionnaireOverlay,
        oneshot::Receiver<QuestionnaireResponse>,
    ) {
        let (tx, rx) = oneshot::channel();
        let overlay = QuestionnaireOverlay::new(questions, tx, None);
        (overlay, rx)
    }

    #[test]
    fn test_all_answered_single_select() {
        let (mut overlay, _rx) = make_overlay(vec![make_question("q1", false, None)]);
        assert!(!overlay.all_answered());
        overlay.save_answer("q1".into(), "a".into(), "Option A".into(), false, Some(1));
        assert!(overlay.all_answered());
    }

    #[test]
    fn test_all_answered_multiselect_with_selected() {
        let (mut overlay, _rx) = make_overlay(vec![make_question("q1", true, None)]);
        // Not answered initially
        assert!(!overlay.all_answered());
        // Toggle a selection
        overlay.toggle_multi(0, 0);
        // Now considered answered (has non-empty selected set)
        assert!(overlay.all_answered());
        // Toggle off
        overlay.toggle_multi(0, 0);
        assert!(!overlay.all_answered());
    }

    #[test]
    fn test_convert_selected_to_answers() {
        let (mut overlay, _rx) = make_overlay(vec![make_question("q1", true, None)]);
        overlay.toggle_multi(0, 0);
        overlay.toggle_multi(0, 1);
        assert!(overlay.answers.is_empty());

        overlay.convert_selected_to_answers();
        let ans = overlay.answers.get("q1").expect("answer should exist");
        assert_eq!(ans.value, "a, b");
        assert_eq!(ans.label, "Option A, Option B");
        assert!(!ans.was_custom);
        assert!(ans.index.is_none()); // multi-select has no single index
    }

    #[test]
    fn test_convert_selected_empty_does_nothing() {
        let (mut overlay, _rx) = make_overlay(vec![make_question("q1", true, None)]);
        overlay.convert_selected_to_answers();
        assert!(overlay.answers.is_empty());
    }

    #[test]
    fn test_auto_select_defaults_recommended() {
        let (mut overlay, _rx) = make_overlay(vec![
            make_question("q1", false, Some(1)), // recommended = index 1
            make_question("q2", true, None),     // no recommended → index 0
        ]);
        overlay.auto_select_defaults();

        // q1: single-select → recommended (index 1 = "b")
        let ans1 = overlay.answers.get("q1").expect("q1 should be answered");
        assert_eq!(ans1.value, "b");
        assert_eq!(ans1.label, "Option B");

        // q2: multi-select → first option (index 0 = "a"), converted to answers
        let ans2 = overlay.answers.get("q2").expect("q2 should be answered");
        assert_eq!(ans2.value, "a");
    }

    #[test]
    fn test_auto_select_skips_already_answered() {
        let (mut overlay, _rx) = make_overlay(vec![make_question("q1", false, Some(1))]);
        // Pre-answer with option A (index 0)
        overlay.save_answer("q1".into(), "a".into(), "Option A".into(), false, Some(1));
        overlay.auto_select_defaults();
        // Should still be option A, not overwritten by recommended (option B)
        let ans = overlay.answers.get("q1").unwrap();
        assert_eq!(ans.value, "a");
    }

    #[test]
    fn test_timeout_does_not_fire_without_config() {
        let (mut overlay, _rx) = make_overlay(vec![make_question("q1", false, None)]);
        // No timeout configured → check_timeout always returns false
        assert!(!overlay.check_timeout());
        assert!(!overlay.timed_out);
    }

    #[test]
    fn test_initial_cursor_uses_recommended() {
        let (overlay, _rx) = make_overlay(vec![make_question("q1", false, Some(1))]);
        assert_eq!(overlay.option_cursor, 1); // recommended index
    }

    #[test]
    fn test_initial_cursor_zero_without_recommended() {
        let (overlay, _rx) = make_overlay(vec![make_question("q1", false, None)]);
        assert_eq!(overlay.option_cursor, 0);
    }
}
