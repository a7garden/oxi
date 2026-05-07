//! Rendering functions for the TUI.

use super::app::{AppOverlay, AppState, SetupStep, SPINNER};
use oxi_tui::theme::Theme;
use oxi_tui::widgets::{
    chat::ChatView,
    footer::Footer,
    input::Input,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Main draw function — renders the full TUI frame.
pub fn draw(f: &mut Frame, state: &mut AppState, theme: &Theme) {
    let size = f.area();

    // Overlay takes over the entire screen
    if state.overlay.is_some() {
        render_overlay(f, size, state, theme);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),   // Chat
            Constraint::Length(2), // Input (separator + input)
            Constraint::Length(3), // Status bar (separator + 2 lines)
        ])
        .split(size);

    // Chat
    f.render_stateful_widget(ChatView::new(theme), chunks[0], &mut state.chat);

    // Input area
    render_input_area(f, chunks[1], state, theme);

    // Slash popup — overlay above the input area
    if state.slash_completion_active {
        render_slash_popup_overlay(f, chunks[1], state, theme);
    }

    // Status bar
    f.render_stateful_widget(Footer::new(theme), chunks[2], &mut state.footer_state);
}

// ── Input area ───────────────────────────────────────────────────────────

fn render_input_area(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    if area.height < 2 {
        return;
    }

    // Top separator line
    let top_row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let line = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(
            line,
            Style::default().fg(theme.colors.border.to_ratatui()),
        )),
        top_row,
    );

    // Input row
    let input_row = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: 1,
    };

    if state.is_agent_busy {
        render_busy_input(f, input_row, state, theme);
    } else {
        f.render_stateful_widget(
            Input::new(theme),
            input_row,
            &mut state.input,
        );
    }
}

// ── Busy input (spinner) ─────────────────────────────────────────────────

fn render_busy_input(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    // Show spinner on a separate line above the input, not in the same line
    // This avoids the spinner being flush against the input text
    let spinner_line = format!("  {} waiting for response…", SPINNER[state.spinner_frame]);
    f.render_widget(
        Paragraph::new(Span::styled(spinner_line, Style::default().fg(theme.colors.muted.to_ratatui()))),
        area,
    );
}

// ── Slash popup (Pi-style vertical list overlay) ─────────────────────────

fn render_slash_popup_overlay(
    f: &mut Frame,
    input_area: Rect,
    state: &AppState,
    theme: &Theme,
) {
    if state.slash_completions.is_empty() {
        return;
    }
    let selected = state.slash_completion_index;
    let total = state.slash_completions.len();
    let max_show = 8usize.min(total);

    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else {
        0
    };

    // Popup positioned above the input area
    let popup_width = input_area.width;
    let popup_height = max_show as u16 + 2;
    let popup_x = input_area.x;
    let popup_y = input_area.y.saturating_sub(popup_height);

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    f.render_widget(Clear, popup_area);

    let mut lines: Vec<Line> = Vec::with_capacity(max_show);
    let visible: Vec<_> = state
        .slash_completions
        .iter()
        .enumerate()
        .skip(window_start)
        .take(max_show)
        .collect();

    let name_width = state
        .slash_completions
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(10)
        .max(10);

    for (i, comp) in &visible {
        let is_selected = *i == selected;
        let pointer = if is_selected { "→" } else { " " };
        let name_padded = format!("{:<width$}", comp.name, width = name_width);

        let desc_space = (popup_width as usize).saturating_sub(name_width + 8);
        let desc: String = comp.description.chars().take(desc_space).collect();

        if is_selected {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} ", pointer),
                    Style::default().fg(theme.colors.accent.to_ratatui()),
                ),
                Span::styled(
                    format!(" {}  ", name_padded),
                    Style::default()
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.primary.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    desc,
                    Style::default().fg(theme.colors.muted.to_ratatui()),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} ", pointer),
                    Style::default(),
                ),
                Span::styled(
                    format!(" {}  ", name_padded),
                    Style::default().fg(theme.colors.foreground.to_ratatui()),
                ),
                Span::styled(
                    desc,
                    Style::default().fg(theme.colors.muted.to_ratatui()),
                ),
            ]));
        }
    }

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.colors.border.to_ratatui()));

    let popup_inner = block.inner(popup_area);
    f.render_widget(block, popup_area);
    f.render_widget(Paragraph::new(lines), popup_inner);

    // Page indicator
    let page = window_start / max_show + 1;
    let total_pages = (total + max_show - 1) / max_show;
    if total_pages > 1 {
        let indicator = format!("({}/{})", page, total_pages);
        let indicator_area = Rect {
            x: popup_area.x + popup_area.width.saturating_sub(indicator.chars().count() as u16 + 2),
            y: popup_area.y + popup_area.height.saturating_sub(1),
            width: indicator.chars().count() as u16 + 2,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                indicator,
                Style::default().fg(theme.colors.muted.to_ratatui()),
            )),
            indicator_area,
        );
    }
}

// ── Overlay rendering ─────────────────────────────────────────────────────

fn render_overlay(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    // Clone overlay to avoid borrow conflicts
    let overlay = state.overlay.clone();
    match &overlay {
        Some(AppOverlay::Setup(step)) => render_setup_step(f, area, state, theme, step),
        Some(AppOverlay::ProviderConfig(step)) => render_provider_step(f, area, state, theme, step),
        Some(AppOverlay::ModelSelect { .. }) => render_model_select(f, area, state, theme),
        Some(AppOverlay::LogoutSelect { .. }) => render_logout_select(f, area, state, theme),
        None => {}
    }
}

fn render_select_auth_type(f: &mut Frame, area: Rect, theme: &Theme, styles: &oxi_tui::theme::ThemeStyles, selected: usize) {
    let max_w = area.width as usize;
    let title = " How would you like to authenticate? ";
    let title_y = area.y + 2;
    for (i, c) in title.chars().enumerate() {
        if i < max_w {
            f.render_widget(
                Paragraph::new(Span::styled(
                    c.to_string(),
                    Style::default()
                        .fg(theme.colors.primary.to_ratatui())
                        .bg(theme.colors.background.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                )),
                Rect { x: area.x + (i as u16).min(area.width - 1), y: title_y, width: 1, height: 1 },
            );
        }
    }

    // Auth type options
    let list_y = title_y + 2;
    let options = [("\u{1F511} OAuth", "Sign in with your account (coming soon)"), ("\u{1F511} API Key", "Enter an API key manually")];

    for (i, (name, desc)) in options.iter().enumerate() {
        let row = Rect { x: area.x, y: list_y + i as u16, width: area.width, height: 1 };
        if row.y >= area.y + area.height { break; }

        let is_sel = i == selected;
        let pointer = if is_sel { "\u{2192}" } else { " " };

        let line_str = format!(" {}  {:<14} {}", pointer, name, desc);

        let style = if is_sel {
            Style::default()
                .fg(theme.colors.background.to_ratatui())
                .bg(theme.colors.primary.to_ratatui())
                .add_modifier(Modifier::BOLD)
        } else {
            styles.normal
        };

        f.render_widget(Paragraph::new(Span::styled(line_str, style)), row);
    }

    // Footer hint
    let hint_y = list_y + options.len() as u16 + 1;
    if hint_y < area.y + area.height {
        let hint = " \u{2191}/\u{2193} select \u{00b7} Enter confirm \u{00b7} Esc cancel";
        f.render_widget(
            Paragraph::new(Span::styled(hint, styles.muted)),
            Rect { x: area.x, y: hint_y, width: area.width, height: 1 },
        );
    }
}

fn render_setup_step(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme, step: &SetupStep) {
    let styles = theme.to_styles();
    let max_w = area.width as usize;

    match step {
        SetupStep::SelectAuthType { selected, .. } => {
            render_select_auth_type(f, area, theme, &styles, *selected);
        }

        SetupStep::SelectProvider { providers, selected } => {
            // Title
            let title = " Select a provider to get started ";
            let title_y = area.y + 2;
            for (i, c) in title.chars().enumerate() {
                if i < max_w {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            c.to_string(),
                            Style::default()
                                .fg(theme.colors.primary.to_ratatui())
                                .bg(theme.colors.background.to_ratatui())
                                .add_modifier(Modifier::BOLD),
                        )),
                        Rect { x: area.x + (i as u16).min(area.width - 1), y: title_y, width: 1, height: 1 },
                    );
                }
            }

            // Provider list
            let list_y = title_y + 2;
            for (i, (name, has_key)) in providers.iter().enumerate() {
                let row = Rect { x: area.x, y: list_y + i as u16, width: area.width, height: 1 };
                if row.y >= area.y + area.height { break; }

                let is_sel = i == *selected;
                let status = if *has_key { "●" } else { "○" };
                let pointer = if is_sel { "→" } else { " " };

                let line_str = format!(" {} {} {}", pointer, status, name);

                let style = if is_sel {
                    Style::default()
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.primary.to_ratatui())
                        .add_modifier(Modifier::BOLD)
                } else {
                    styles.normal
                };

                f.render_widget(Paragraph::new(Span::styled(line_str, style)), row);
            }

            // Footer hint
            let hint_y = list_y + providers.len() as u16 + 1;
            if hint_y < area.y + area.height {
                let hint = " ↑/↓ select · Enter confirm · q quit";
                f.render_widget(
                    Paragraph::new(Span::styled(hint, styles.muted)),
                    Rect { x: area.x, y: hint_y, width: area.width, height: 1 },
                );
            }
        }

        SetupStep::EnterApiKey { provider, key, .. } => {
            let title = format!(" Enter API key for {}", provider);
            let title_y = area.y + 3;

            f.render_widget(
                Paragraph::new(Span::styled(
                    title,
                    Style::default()
                        .fg(theme.colors.primary.to_ratatui())
                        .bg(theme.colors.background.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                )),
                Rect { x: area.x + 2, y: title_y, width: area.width.saturating_sub(4), height: 1 },
            );

            // Input field — plain text
            let input_y = title_y + 2;
            let display = if key.is_empty() {
                " ".to_string()
            } else {
                key.clone()
            };

            let input_line = format!(" API Key: {}", display);
            f.render_widget(
                Paragraph::new(Span::styled(input_line, styles.normal)),
                Rect { x: area.x + 2, y: input_y, width: area.width.saturating_sub(4), height: 1 },
            );

            // Cursor
            let cursor_col = 11u16 + display.len().min(max_w.saturating_sub(14) as usize) as u16;
            f.render_widget(
                Paragraph::new(Span::styled(
                    " ",
                    Style::default()
                        .fg(theme.colors.cursor_fg.to_ratatui())
                        .bg(theme.colors.cursor_bg.to_ratatui()),
                )),
                Rect { x: area.x + cursor_col, y: input_y, width: 1, height: 1 },
            );

            let hint_y = input_y + 2;
            if hint_y < area.y + area.height {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        " Type your key · Enter save · Esc back",
                        styles.muted,
                    )),
                    Rect { x: area.x + 2, y: hint_y, width: area.width.saturating_sub(4), height: 1 },
                );
            }
        }

        SetupStep::Done { provider, model } => {
            let msg = format!(" {} is ready!", provider);
            let msg_y = area.y + 4;

            f.render_widget(
                Paragraph::new(Span::styled(
                    msg,
                    Style::default()
                        .fg(theme.colors.success.to_ratatui())
                        .bg(theme.colors.background.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                )),
                Rect { x: area.x + 2, y: msg_y, width: area.width.saturating_sub(4), height: 1 },
            );

            let model_line = format!(" Model: {}", model);
            f.render_widget(
                Paragraph::new(Span::styled(model_line, styles.normal)),
                Rect { x: area.x + 2, y: msg_y + 1, width: area.width.saturating_sub(4), height: 1 },
            );

            f.render_widget(
                Paragraph::new(Span::styled(
                    " Press Enter to start chatting",
                    styles.muted,
                )),
                Rect { x: area.x + 2, y: msg_y + 3, width: area.width.saturating_sub(4), height: 1 },
            );
        }

        SetupStep::SelectModel { provider, models, selected } => {
            // Title
            let title = format!(" Select a model for {}", provider);
            let title_y = area.y + 2;
            for (i, c) in title.chars().enumerate() {
                if i < max_w {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            c.to_string(),
                            Style::default()
                                .fg(theme.colors.primary.to_ratatui())
                                .bg(theme.colors.background.to_ratatui())
                                .add_modifier(Modifier::BOLD),
                        )),
                        Rect { x: area.x + (i as u16).min(area.width - 1), y: title_y, width: 1, height: 1 },
                    );
                }
            }

            // Model list
            let list_y = title_y + 2;
            let max_show = (area.height as usize).saturating_sub(6).min(models.len());
            let window_start = if *selected >= max_show {
                selected.saturating_sub(max_show - 1)
            } else {
                0
            };

            for i in 0..max_show {
                let idx = window_start + i;
                if idx >= models.len() { break; }

                let row = Rect { x: area.x, y: list_y + i as u16, width: area.width, height: 1 };
                if row.y >= area.y + area.height { break; }

                let is_sel = idx == *selected;
                let pointer = if is_sel { "→" } else { " " };
                let model_id = &models[idx];

                let line_str = format!(" {} {}", pointer, model_id);

                let style = if is_sel {
                    Style::default()
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.primary.to_ratatui())
                        .add_modifier(Modifier::BOLD)
                } else {
                    styles.normal
                };

                f.render_widget(Paragraph::new(Span::styled(line_str, style)), row);
            }

            // Footer hint
            let hint_y = list_y + max_show as u16 + 1;
            if hint_y < area.y + area.height {
                let hint = format!(" ↑/↓ select · Enter confirm · Esc back ({})", models.len());
                f.render_widget(
                    Paragraph::new(Span::styled(hint, styles.muted)),
                    Rect { x: area.x, y: hint_y, width: area.width, height: 1 },
                );
            }
        }
    }
}

fn render_provider_step(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme, step: &SetupStep) {
    let styles = theme.to_styles();
    let max_w = area.width as usize;

    match step {
        SetupStep::SelectAuthType { selected, .. } => {
            render_select_auth_type(f, area, theme, &styles, *selected);
        }

        SetupStep::SelectProvider { providers, selected } => {
            let title = " Select a provider to configure ";
            let title_y = area.y + 2;
            for (i, c) in title.chars().enumerate() {
                if i < max_w {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            c.to_string(),
                            Style::default()
                                .fg(theme.colors.primary.to_ratatui())
                                .bg(theme.colors.background.to_ratatui())
                                .add_modifier(Modifier::BOLD),
                        )),
                        Rect { x: area.x + (i as u16).min(area.width - 1), y: title_y, width: 1, height: 1 },
                    );
                }
            }

            let list_y = title_y + 2;
            for (i, (name, has_key)) in providers.iter().enumerate() {
                let row = Rect { x: area.x, y: list_y + i as u16, width: area.width, height: 1 };
                if row.y >= area.y + area.height { break; }

                let is_sel = i == *selected;
                let status = if *has_key { "●" } else { "○" };
                let pointer = if is_sel { "→" } else { " " };

                let line_str = format!(" {} {} {}", pointer, status, name);
                let style = if is_sel {
                    Style::default()
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.primary.to_ratatui())
                        .add_modifier(Modifier::BOLD)
                } else {
                    styles.normal
                };

                f.render_widget(Paragraph::new(Span::styled(line_str, style)), row);
            }

            let hint_y = list_y + providers.len() as u16 + 1;
            if hint_y < area.y + area.height {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        " ↑/↓ select · Enter confirm · Esc cancel",
                        styles.muted,
                    )),
                    Rect { x: area.x, y: hint_y, width: area.width, height: 1 },
                );
            }
        }

        SetupStep::EnterApiKey { provider, key, .. } => {
            let title = format!(" Enter API key for {}", provider);
            let title_y = area.y + 3;

            f.render_widget(
                Paragraph::new(Span::styled(
                    title,
                    Style::default()
                        .fg(theme.colors.primary.to_ratatui())
                        .bg(theme.colors.background.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                )),
                Rect { x: area.x + 2, y: title_y, width: area.width.saturating_sub(4), height: 1 },
            );

            let input_y = title_y + 2;
            let display = if key.is_empty() {
                " ".to_string()
            } else {
                key.clone()
            };

            let input_line = format!(" API Key: {}", display);
            f.render_widget(
                Paragraph::new(Span::styled(input_line, styles.normal)),
                Rect { x: area.x + 2, y: input_y, width: area.width.saturating_sub(4), height: 1 },
            );

            let cursor_col = 11u16 + display.len().min(max_w.saturating_sub(14) as usize) as u16;
            f.render_widget(
                Paragraph::new(Span::styled(
                    " ",
                    Style::default()
                        .fg(theme.colors.cursor_fg.to_ratatui())
                        .bg(theme.colors.cursor_bg.to_ratatui()),
                )),
                Rect { x: area.x + cursor_col, y: input_y, width: 1, height: 1 },
            );

            let hint_y = input_y + 2;
            if hint_y < area.y + area.height {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        " Type your key · Enter save · Esc back",
                        styles.muted,
                    )),
                    Rect { x: area.x + 2, y: hint_y, width: area.width.saturating_sub(4), height: 1 },
                );
            }
        }

        SetupStep::SelectModel { provider, models, selected } => {
            // Same UI as setup wizard
            let title = format!(" Select a model for {}", provider);
            let title_y = area.y + 2;
            for (i, c) in title.chars().enumerate() {
                if i < max_w {
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            c.to_string(),
                            Style::default()
                                .fg(theme.colors.primary.to_ratatui())
                                .bg(theme.colors.background.to_ratatui())
                                .add_modifier(Modifier::BOLD),
                        )),
                        Rect { x: area.x + (i as u16).min(area.width - 1), y: title_y, width: 1, height: 1 },
                    );
                }
            }

            let list_y = title_y + 2;
            let max_show = (area.height as usize).saturating_sub(6).min(models.len());
            let window_start = if *selected >= max_show {
                selected.saturating_sub(max_show - 1)
            } else {
                0
            };

            for i in 0..max_show {
                let idx = window_start + i;
                if idx >= models.len() { break; }

                let row = Rect { x: area.x, y: list_y + i as u16, width: area.width, height: 1 };
                if row.y >= area.y + area.height { break; }

                let is_sel = idx == *selected;
                let pointer = if is_sel { "→" } else { " " };
                let model_id = &models[idx];

                let line_str = format!(" {} {}", pointer, model_id);

                let style = if is_sel {
                    Style::default()
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.primary.to_ratatui())
                        .add_modifier(Modifier::BOLD)
                } else {
                    styles.normal
                };

                f.render_widget(Paragraph::new(Span::styled(line_str, style)), row);
            }

            let hint_y = list_y + max_show as u16 + 1;
            if hint_y < area.y + area.height {
                let hint = format!(" ↑/↓ select · Enter confirm · Esc back ({})", models.len());
                f.render_widget(
                    Paragraph::new(Span::styled(hint, styles.muted)),
                    Rect { x: area.x, y: hint_y, width: area.width, height: 1 },
                );
            }
        }

        SetupStep::Done { .. } => {
            // Shouldn't be reached for login, but render something
            state.overlay = None;
        }
    }
}

fn render_model_select(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let styles = theme.to_styles();

    let (models, filter, selected) = match &state.overlay {
        Some(AppOverlay::ModelSelect { models, filter, selected }) => (models.clone(), filter.clone(), *selected),
        _ => return,
    };

    // Compute filtered models
    let filtered: Vec<(usize, &String)> = if filter.is_empty() {
        models.iter().enumerate().collect()
    } else {
        let lower = filter.to_lowercase();
        models.iter().enumerate().filter(|(_, m)| m.to_lowercase().contains(&lower)).collect()
    };

    // Draw a centered popup covering ~70% of the screen
    let popup_w = (area.width as f32 * 0.7) as u16;
    let popup_h = (area.height as f32 * 0.7) as u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_w) / 2);
    let popup_y = area.y + (area.height.saturating_sub(popup_h) / 2);
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_w,
        height: popup_h,
    };

    // Dim the background
    f.render_widget(Clear, popup_area);
    let dimmed = Block::default()
        .style(Style::default().bg(theme.colors.background.to_ratatui()));
    f.render_widget(dimmed, popup_area);

    // Title + filter line
    let title_line = if filter.is_empty() {
        " Select a model ".to_string()
    } else {
        format!(" Filter: {} ", filter)
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            title_line,
            Style::default()
                .fg(theme.colors.primary.to_ratatui())
                .bg(theme.colors.background.to_ratatui())
                .add_modifier(Modifier::BOLD),
        )),
        Rect { x: popup_x + 1, y: popup_y + 1, width: popup_w.saturating_sub(2), height: 1 },
    );

    // List of models
    let list_start_y = popup_y + 3;
    let list_height = popup_h.saturating_sub(5) as usize; // title + hint + borders
    let max_show = list_height.min(filtered.len());

    // Windowed scrolling
    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else {
        0
    };

    let visible: Vec<_> = filtered.iter().skip(window_start).take(max_show).collect();
    let model_col_width = (popup_w as usize).saturating_sub(6);

    for (vi, (orig_idx, model_id)) in visible.iter().enumerate() {
        let display_idx = vi + window_start;
        let is_sel = display_idx == selected;
        let pointer = if is_sel { "→" } else { " " };

        let display: String = model_id.chars().take(model_col_width).collect();
        let padded = format!(" {:<width$}", display, width = model_col_width);

        let row_y = list_start_y + vi as u16;
        if row_y >= popup_y + popup_h { break; }

        let row = Rect { x: popup_x + 1, y: row_y, width: popup_w.saturating_sub(2), height: 1 };

        if is_sel {
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!("{} {}", pointer, padded),
                    Style::default()
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.primary.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                )),
                row,
            );
        } else {
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!("{} {}", pointer, padded),
                    styles.normal,
                )),
                row,
            );
        }
    }

    // Footer hint
    let hint_y = popup_y + popup_h.saturating_sub(2);
    let hint = format!(" {} models · ↑/↓ navigate · type to filter · Enter select · Esc cancel", filtered.len());
    f.render_widget(
        Paragraph::new(Span::styled(hint, styles.muted)),
        Rect { x: popup_x + 1, y: hint_y, width: popup_w.saturating_sub(2), height: 1 },
    );

    // Border
    let border = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
    f.render_widget(border, popup_area);
}

fn render_logout_select(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let styles = theme.to_styles();

    let (providers, selected) = match &state.overlay {
        Some(AppOverlay::LogoutSelect { providers, selected }) => (providers.clone(), *selected),
        _ => return,
    };

    // Centered popup ~40% height
    let popup_w = (area.width as f32 * 0.5) as u16;
    let popup_h = ((providers.len() as u16 + 5).max(8)).min((area.height as f32 * 0.5) as u16);
    let popup_x = area.x + (area.width.saturating_sub(popup_w) / 2);
    let popup_y = area.y + (area.height.saturating_sub(popup_h) / 2);
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_w,
        height: popup_h,
    };

    f.render_widget(Clear, popup_area);
    let dimmed = Block::default()
        .style(Style::default().bg(theme.colors.background.to_ratatui()));
    f.render_widget(dimmed, popup_area);

    // Title
    let title = " Select provider to logout ";
    f.render_widget(
        Paragraph::new(Span::styled(
            title,
            Style::default()
                .fg(theme.colors.primary.to_ratatui())
                .bg(theme.colors.background.to_ratatui())
                .add_modifier(Modifier::BOLD),
        )),
        Rect { x: popup_x + 1, y: popup_y + 1, width: popup_w.saturating_sub(2), height: 1 },
    );

    // Provider list
    let list_y = popup_y + 3;
    for (i, provider) in providers.iter().enumerate() {
        let row_y = list_y + i as u16;
        if row_y >= popup_y + popup_h.saturating_sub(2) { break; }
        let row = Rect { x: popup_x + 1, y: row_y, width: popup_w.saturating_sub(2), height: 1 };

        let is_sel = i == selected;
        let pointer = if is_sel { "→" } else { " " };

        let line_str = format!(" {} {}", pointer, provider);
        let style = if is_sel {
            Style::default()
                .fg(theme.colors.background.to_ratatui())
                .bg(theme.colors.error.to_ratatui())
                .add_modifier(Modifier::BOLD)
        } else {
            styles.normal
        };

        f.render_widget(Paragraph::new(Span::styled(line_str, style)), row);
    }

    // Footer hint
    let hint_y = popup_y + popup_h.saturating_sub(2);
    f.render_widget(
        Paragraph::new(Span::styled(
            " ↑/↓ select · Enter remove · Esc cancel",
            styles.muted,
        )),
        Rect { x: popup_x + 1, y: hint_y, width: popup_w.saturating_sub(2), height: 1 },
    );

    // Border
    let border = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
    f.render_widget(border, popup_area);
}
