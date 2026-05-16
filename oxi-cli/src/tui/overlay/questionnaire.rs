//! Questionnaire overlay — interactive multi-question TUI component.
//!
//! Implements `OverlayComponent` to display one or more questions
//! with options, tabs, and inline text input (allowOther).

use super::{centered_popup, OverlayAction, OverlayComponent};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_agent::tools::questionnaire::{Answer, Question, QuestionnaireResponse};
use oxi_tui::theme::{Theme, ThemeStyles};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use std::collections::{HashMap, HashSet};

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
}

impl QuestionnaireOverlay {
    pub fn new(
        questions: Vec<Question>,
        responder: tokio::sync::oneshot::Sender<QuestionnaireResponse>,
    ) -> Self {
        if questions.is_empty() {
            let _ = responder.send(QuestionnaireResponse {
                answers: vec![],
                cancelled: false,
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
            };
        }

        Self {
            questions,
            current_tab: 0,
            option_cursor: 0,
            selected: HashMap::new(),
            answers: HashMap::new(),
            input_mode: false,
            input_text: String::new(),
            input_question_id: String::new(),
            responder: Some(responder),
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
        self.questions
            .iter()
            .all(|q| self.answers.contains_key(&q.id))
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
                self.option_cursor = 0;
                return;
            }
            if !self.answers.contains_key(&self.questions[i].id) {
                self.current_tab = i;
                self.option_cursor = 0;
                return;
            }
        }
        self.current_tab = self.submit_tab();
        self.option_cursor = 0;
    }

    fn do_submit(&mut self, cancelled: bool) {
        let answers: Vec<Answer> = std::mem::take(&mut self.answers).into_values().collect();
        if let Some(responder) = self.responder.take() {
            let _ = responder.send(QuestionnaireResponse { answers, cancelled });
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
                    self.option_cursor = 0;
                    return OverlayAction::None;
                }
                KeyCode::BackTab | KeyCode::Left => {
                    self.current_tab =
                        (self.current_tab + self.total_tabs() - 1) % self.total_tabs();
                    self.option_cursor = 0;
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
            KeyCode::Down => {
                if opt_count > 0 {
                    self.option_cursor = (self.option_cursor + 1).min(opt_count - 1);
                }
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
        let popup = centered_popup(area, 0.85, 0.80);

        frame.render_widget(Clear, popup);

        let title_style = Style::default()
            .fg(theme.colors.primary.to_ratatui())
            .add_modifier(Modifier::BOLD);

        let border_block = Block::default()
            .title(Line::styled(" Questionnaire ", title_style))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));

        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        self.render_content(frame, inner, theme);
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
        let fg = theme.colors.accent.to_ratatui();

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
        let fg = theme.colors.accent.to_ratatui();

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
        let fg = theme.colors.accent.to_ratatui();

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
        let fg = theme.colors.accent.to_ratatui();

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
            .fg(theme.colors.foreground.to_ratatui())
            .bg(theme.colors.primary.to_ratatui());

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
