//! Router configuration setup overlay.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_tui::{Theme, ThemeStyles};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use super::{OverlayAction, OverlayComponent, centered_layout};

/// Router profile data captured from the setup form.
#[derive(Debug, Clone, Default)]
pub struct RouterSetupData {
    pub profile_name: String,
    pub high_model: String,
    pub high_thinking: String,
    pub medium_model: String,
    pub low_model: String,
}

/// Which field has keyboard focus in the router setup form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusField {
    #[default]
    ProfileName,
    HighModel,
    HighThinking,
    MediumModel,
    LowModel,
    SaveBtn,
    CancelBtn,
}

const ALL_FIELDS: [FocusField; 7] = [
    FocusField::ProfileName,
    FocusField::HighModel,
    FocusField::HighThinking,
    FocusField::MediumModel,
    FocusField::LowModel,
    FocusField::SaveBtn,
    FocusField::CancelBtn,
];

// Router setup overlay — lets users configure router profile models per tier.

/// Callback types.
type SaveCallback = std::sync::Mutex<Box<dyn FnMut(&RouterSetupData) -> Result<(), String>>>;
type CancelCallback = std::sync::Mutex<Box<dyn FnMut()>>;

pub struct RouterSetupOverlay {
    pub profile_name: String,
    pub high_model: String,
    pub medium_model: String,
    pub low_model: String,
    pub high_thinking: String,
    focus: FocusField,
    picking_for: Option<FocusField>,
    model_list: Vec<String>,
    model_selected: usize,
    #[allow(dead_code)]
    on_save: SaveCallback,
    #[allow(dead_code)]
    on_cancel: CancelCallback,
}

impl std::fmt::Debug for RouterSetupOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterSetupOverlay")
            .field("profile_name", &self.profile_name)
            .field("high_model", &self.high_model)
            .field("medium_model", &self.medium_model)
            .field("low_model", &self.low_model)
            .field("high_thinking", &self.high_thinking)
            .field("focus", &self.focus)
            .field("picking_for", &self.picking_for)
            .field("model_selected", &self.model_selected)
            .finish_non_exhaustive()
    }
}

impl Default for RouterSetupOverlay {
    fn default() -> Self {
        Self {
            profile_name: Default::default(),
            high_model: Default::default(),
            medium_model: Default::default(),
            low_model: Default::default(),
            high_thinking: Default::default(),
            focus: FocusField::ProfileName,
            picking_for: None,
            model_list: Default::default(),
            model_selected: 0,
            on_save: std::sync::Mutex::new(Box::new(|_| Ok(()))),
            on_cancel: std::sync::Mutex::new(Box::new(|| {})),
        }
    }
}

impl RouterSetupOverlay {
    pub fn new(
        initial: RouterSetupData,
        model_list: Vec<String>,
        on_save: impl FnMut(&RouterSetupData) -> Result<(), String> + 'static,
        on_cancel: impl FnMut() + 'static,
    ) -> Self {
        Self {
            profile_name: initial.profile_name,
            high_model: initial.high_model,
            high_thinking: initial.high_thinking,
            medium_model: initial.medium_model,
            low_model: initial.low_model,
            focus: FocusField::ProfileName,
            picking_for: None,
            model_list,
            model_selected: 0,
            on_save: std::sync::Mutex::new(Box::new(on_save)),
            on_cancel: std::sync::Mutex::new(Box::new(on_cancel)),
        }
    }

    fn current_data(&self) -> RouterSetupData {
        RouterSetupData {
            profile_name: self.profile_name.clone(),
            high_model: self.high_model.clone(),
            high_thinking: self.high_thinking.clone(),
            medium_model: self.medium_model.clone(),
            low_model: self.low_model.clone(),
        }
    }

    /// Get the filter text for the current picker target field.
    fn picker_filter_text(&self) -> &str {
        match self.picking_for {
            Some(FocusField::HighModel) => &self.high_model,
            Some(FocusField::MediumModel) => &self.medium_model,
            Some(FocusField::LowModel) => &self.low_model,
            _ => &self.high_model,
        }
    }

    /// Push a character onto the active picker filter field.
    fn push_picker_char(&mut self, c: char) {
        match self.picking_for {
            Some(FocusField::MediumModel) => self.medium_model.push(c),
            Some(FocusField::LowModel) => self.low_model.push(c),
            _ => self.high_model.push(c),
        }
    }

    /// Pop a character from the active picker filter field.
    fn pop_picker_char(&mut self) {
        match self.picking_for {
            Some(FocusField::MediumModel) => {
                self.medium_model.pop();
            }
            Some(FocusField::LowModel) => {
                self.low_model.pop();
            }
            _ => {
                self.high_model.pop();
            }
        }
    }

    fn filtered_models(&self) -> Vec<(usize, &String)> {
        let filter = self.picker_filter_text();
        if filter.is_empty() {
            return self.model_list.iter().enumerate().collect();
        }
        let lower = filter.to_lowercase();
        self.model_list
            .iter()
            .enumerate()
            .filter(|(_, m)| m.to_lowercase().contains(&lower))
            .collect()
    }

    const LABEL_W: usize = 14;

    fn render_field_row(
        frame: &mut Frame,
        area: Rect,
        label: &str,
        value: &str,
        focused: bool,
        theme: &Theme,
        styles: &ThemeStyles,
    ) {
        let avail = (area.width as usize).saturating_sub(Self::LABEL_W + 6);
        let display = if value.is_empty() { "(not set)" } else { value };

        let value_style = if focused {
            Style::default()
                .fg(theme.colors.background)
                .bg(theme.colors.primary)
        } else if value.is_empty() {
            styles.muted
        } else {
            styles.normal
        };

        let label_span = Span::styled(
            format!("{:width$}", label, width = Self::LABEL_W),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::BOLD),
        );

        let value_span = Span::styled(format!(" {:width$}", display, width = avail), value_style);

        frame.render_widget(
            Paragraph::new(Line::from(vec![label_span, value_span])),
            area,
        );
    }

    fn render_thinking_row(
        frame: &mut Frame,
        area: Rect,
        label: &str,
        value: &str,
        _focused: bool,
        theme: &Theme,
        styles: &ThemeStyles,
    ) {
        const OPTIONS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];
        let current = if value.is_empty() { "medium" } else { value };

        let label_span = Span::styled(
            format!("{:width$}", label, width = Self::LABEL_W),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::BOLD),
        );

        let option_spans: Vec<Span> = OPTIONS
            .iter()
            .map(|opt| {
                if *opt == current {
                    Span::styled(
                        format!("[{}] ", opt),
                        Style::default()
                            .fg(theme.colors.background)
                            .bg(theme.colors.primary),
                    )
                } else {
                    Span::styled(format!("{} ", opt), styles.muted)
                }
            })
            .collect();

        let mut all: Vec<Span> = vec![label_span];
        all.extend(option_spans);
        all.push(Span::styled(" (thinking level)", styles.muted));
        frame.render_widget(Paragraph::new(Line::from(all)), area);
    }

    fn title_line() -> Line<'static> {
        Line::styled(
            " \u{2699}  Configure Router ",
            Style::default().bg(Color::Rgb(0, 0, 0)),
        )
    }

    fn picker_title() -> Line<'static> {
        Line::styled(" Select a model ", Style::default().bg(Color::Rgb(0, 0, 0)))
    }
}

/// Cycle the thinking level forward based on a typed character.
fn cycle_thinking(current: &mut String, _c: char) {
    const OPTIONS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];
    let cur = if current.is_empty() {
        "medium"
    } else {
        current.as_str()
    };
    let idx = OPTIONS.iter().position(|&o| o == cur).unwrap_or(3);
    let next = (idx + 1) % OPTIONS.len();
    *current = OPTIONS[next].to_string();
}

/// Cycle the thinking level backward (Left arrow).
fn cycle_thinking_back(current: &mut String) {
    const OPTIONS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];
    let cur = if current.is_empty() {
        "medium"
    } else {
        current.as_str()
    };
    let idx = OPTIONS.iter().position(|&o| o == cur).unwrap_or(3);
    let prev = if idx == 0 { OPTIONS.len() - 1 } else { idx - 1 };
    *current = OPTIONS[prev].to_string();
}

impl OverlayComponent for RouterSetupOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        // Model picker mode.
        if self.picking_for.is_some() {
            let filtered = self.filtered_models();
            match key.code {
                KeyCode::Up => {
                    self.model_selected = self.model_selected.saturating_sub(1);
                }
                KeyCode::Down if !filtered.is_empty() => {
                    self.model_selected = (self.model_selected + 1).min(filtered.len() - 1);
                }
                KeyCode::Enter => {
                    let maybe_model: Option<String> = {
                        let filtered = self.filtered_models();
                        filtered.get(self.model_selected).map(|(_, m)| (*m).clone())
                    };
                    if let Some(field) = self.picking_for.take()
                        && let Some(model_str) = maybe_model
                    {
                        match field {
                            FocusField::HighModel => self.high_model = model_str,
                            FocusField::MediumModel => self.medium_model = model_str,
                            FocusField::LowModel => self.low_model = model_str,
                            _ => {}
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Tab | KeyCode::Backspace => {
                    self.picking_for = None;
                    if key.code == KeyCode::Backspace {
                        self.pop_picker_char();
                        self.model_selected = 0;
                    }
                    return OverlayAction::None;
                }
                KeyCode::Char(c) => {
                    self.push_picker_char(c);
                    self.model_selected = 0;
                }
                _ => {}
            }
            return OverlayAction::None;
        }

        match key.code {
            KeyCode::Up | KeyCode::Left => {
                if self.focus == FocusField::HighThinking && key.code == KeyCode::Left {
                    cycle_thinking_back(&mut self.high_thinking);
                    return OverlayAction::None;
                }
                let idx = ALL_FIELDS
                    .iter()
                    .position(|f| *f == self.focus)
                    .unwrap_or(0);
                self.focus = ALL_FIELDS[idx.saturating_sub(1)];
                OverlayAction::None
            }
            KeyCode::Down | KeyCode::Tab => {
                let idx = ALL_FIELDS
                    .iter()
                    .position(|f| *f == self.focus)
                    .unwrap_or(0);
                self.focus = ALL_FIELDS[(idx + 1) % ALL_FIELDS.len()];
                OverlayAction::None
            }
            KeyCode::Right => {
                if self.focus == FocusField::HighThinking {
                    cycle_thinking(&mut self.high_thinking, ' ');
                }
                OverlayAction::None
            }
            KeyCode::Char('`') | KeyCode::Char('/') => {
                if matches!(
                    self.focus,
                    FocusField::HighModel | FocusField::MediumModel | FocusField::LowModel
                ) {
                    self.picking_for = Some(self.focus);
                    self.model_selected = 0;
                }
                OverlayAction::None
            }
            KeyCode::Char(c) => {
                match self.focus {
                    FocusField::ProfileName => self.profile_name.push(c),
                    FocusField::HighModel => self.high_model.push(c),
                    FocusField::MediumModel => self.medium_model.push(c),
                    FocusField::LowModel => self.low_model.push(c),
                    FocusField::HighThinking => cycle_thinking(&mut self.high_thinking, c),
                    _ => {}
                }
                OverlayAction::None
            }
            KeyCode::Backspace => {
                match self.focus {
                    FocusField::ProfileName => {
                        self.profile_name.pop();
                    }
                    FocusField::HighModel => {
                        self.high_model.pop();
                    }
                    FocusField::MediumModel => {
                        self.medium_model.pop();
                    }
                    FocusField::LowModel => {
                        self.low_model.pop();
                    }
                    FocusField::HighThinking => {
                        self.high_thinking.clear();
                    }
                    _ => {}
                }
                OverlayAction::None
            }
            KeyCode::Enter => match self.focus {
                FocusField::SaveBtn => {
                    let data = self.current_data();
                    if let Ok(mut f) = self.on_save.lock() {
                        (*f)(&data).ok();
                    }
                    OverlayAction::Close
                }
                FocusField::CancelBtn => {
                    if let Ok(mut f) = self.on_cancel.lock() {
                        (*f)();
                    }
                    OverlayAction::Close
                }
                _ => OverlayAction::None,
            },
            KeyCode::Esc => {
                if let Ok(mut f) = self.on_cancel.lock() {
                    (*f)();
                }
                OverlayAction::Close
            }
            _ => OverlayAction::None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.72, 0.75);
        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .title(Self::title_line())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let mut y = inner.y + 1;

        Self::render_field_row(
            frame,
            Rect {
                x: inner.x + 1,
                y,
                width: inner.width - 2,
                height: 1,
            },
            "Profile",
            &self.profile_name,
            self.focus == FocusField::ProfileName,
            theme,
            &styles,
        );
        y += 2;

        let sep = Line::from(Span::styled(
            "\u{2500}".repeat((inner.width - 2) as usize),
            Style::default().fg(theme.colors.border),
        ));
        frame.render_widget(
            Paragraph::new(sep),
            Rect {
                x: inner.x + 1,
                y,
                width: inner.width - 2,
                height: 1,
            },
        );
        y += 2;

        Self::render_field_row(
            frame,
            Rect {
                x: inner.x + 1,
                y,
                width: inner.width - 2,
                height: 1,
            },
            "High",
            &self.high_model,
            self.focus == FocusField::HighModel,
            theme,
            &styles,
        );
        y += 1;

        Self::render_thinking_row(
            frame,
            Rect {
                x: inner.x + 1,
                y,
                width: inner.width - 2,
                height: 1,
            },
            "Thinking",
            &self.high_thinking,
            self.focus == FocusField::HighThinking,
            theme,
            &styles,
        );
        y += 2;

        Self::render_field_row(
            frame,
            Rect {
                x: inner.x + 1,
                y,
                width: inner.width - 2,
                height: 1,
            },
            "Medium",
            &self.medium_model,
            self.focus == FocusField::MediumModel,
            theme,
            &styles,
        );
        y += 2;

        Self::render_field_row(
            frame,
            Rect {
                x: inner.x + 1,
                y,
                width: inner.width - 2,
                height: 1,
            },
            "Low",
            &self.low_model,
            self.focus == FocusField::LowModel,
            theme,
            &styles,
        );
        y += 2;

        frame.render_widget(
            Paragraph::new(Span::styled(
                " Up/Down/Tab move  |  type to edit  |  ` or /  to pick model  |  Esc cancel",
                styles.muted,
            )),
            Rect {
                x: inner.x + 1,
                y,
                width: inner.width - 2,
                height: 1,
            },
        );
        y += 1;

        let half = (inner.width - 4) / 2;
        let save_sel = self.focus == FocusField::SaveBtn;
        let cancel_sel = self.focus == FocusField::CancelBtn;

        let save_style = if save_sel {
            Style::default()
                .fg(theme.colors.background)
                .bg(theme.colors.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            styles.primary
        };
        let cancel_style = if cancel_sel {
            Style::default()
                .fg(theme.colors.background)
                .bg(theme.colors.muted)
                .add_modifier(Modifier::BOLD)
        } else {
            styles.muted
        };

        frame.render_widget(
            Paragraph::new(Span::styled(
                if save_sel { "[  Save  ]" } else { " [Save] " },
                save_style,
            )),
            Rect {
                x: inner.x + 2,
                y,
                width: half,
                height: 1,
            },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                if cancel_sel {
                    "[ Cancel ]"
                } else {
                    " [Cancel] "
                },
                cancel_style,
            )),
            Rect {
                x: inner.x + 2 + half,
                y,
                width: half,
                height: 1,
            },
        );

        // Model picker overlay.
        if self.picking_for.is_some() {
            let picker_popup = centered_layout(inner, 0.9, 0.65);
            frame.render_widget(Clear, picker_popup);

            let picker_block = Block::default()
                .title(Self::picker_title())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.colors.accent));
            let pi = picker_block.inner(picker_popup);
            frame.render_widget(picker_block, picker_popup);

            let filtered = self.filtered_models();

            if filtered.is_empty() {
                frame.render_widget(
                    Paragraph::new(Span::styled("(no models match)", styles.muted)),
                    pi,
                );
            } else {
                let max_show = (pi.height as usize).saturating_sub(3).max(1);
                let start = self.model_selected.saturating_sub(max_show / 2);

                let items: Vec<ListItem> = filtered
                    .iter()
                    .skip(start)
                    .take(max_show)
                    .enumerate()
                    .map(|(i, (_, model))| {
                        let is_sel = start + i == self.model_selected;
                        let ptr = if is_sel { "-> " } else { "   " };
                        let style = if is_sel {
                            Style::default()
                                .fg(theme.colors.background)
                                .bg(theme.colors.primary)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            styles.normal
                        };
                        ListItem::new(Span::styled(format!("{ptr}{model}"), style))
                    })
                    .collect();

                frame.render_widget(
                    List::new(items),
                    Rect {
                        x: pi.x + 1,
                        y: pi.y + 1,
                        width: pi.width - 2,
                        height: pi.height - 3,
                    },
                );
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        format!(
                            " {} models  |  Up/Down  |  Enter select  |  Esc back",
                            filtered.len()
                        ),
                        styles.muted,
                    )),
                    Rect {
                        x: pi.x + 1,
                        y: pi.y + pi.height - 2,
                        width: pi.width - 2,
                        height: 1,
                    },
                );
            }
        }
    }

    fn hint(&self) -> &str {
        " Up/Down move  |  type to edit  |  ` pick model  |  Enter save  |  Esc cancel"
    }
}

/// Create a boxed router setup overlay component.
pub fn router_setup(
    initial: RouterSetupData,
    models: Vec<String>,
    on_save: impl FnMut(&RouterSetupData) -> Result<(), String> + 'static,
    on_cancel: impl FnMut() + 'static,
) -> Box<dyn super::OverlayComponent> {
    Box::new(RouterSetupOverlay::new(initial, models, on_save, on_cancel))
}
