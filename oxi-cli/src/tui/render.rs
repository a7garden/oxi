//! Rendering functions for the TUI.

use super::app::{AppOverlay, AppState, ProviderInfo, SetupStep};
use oxi_tui::theme::Theme;
use oxi_tui::widgets::{chat::ChatView, footer::Footer, input::Input};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

// ── Reusable UI components ──────────────────────────────────────────────

/// Render a centered popup frame: clear + dimmed background + border.
fn render_popup_frame(
    f: &mut Frame,
    area: Rect,
    bg: ratatui::style::Color,
    border: ratatui::style::Color,
) -> Rect {
    f.render_widget(Clear, area);
    let dimmed = Block::default().style(Style::default().bg(bg));
    f.render_widget(dimmed, area);
    let border_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border));
    let inner = border_block.inner(area);
    f.render_widget(border_block, area);
    inner
}

/// Compute a centered popup Rect covering `pct` of the screen.
fn centered_popup(area: Rect, width_pct: f32, height_pct: f32) -> Rect {
    let w = (area.width as f32 * width_pct) as u16;
    let h = (area.height as f32 * height_pct) as u16;
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

/// Render a bold title line.
fn render_title(
    f: &mut Frame,
    area: Rect,
    y_offset: u16,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    f.render_widget(
        Paragraph::new(Span::styled(
            text,
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        )),
        Rect {
            x: area.x,
            y: area.y + y_offset,
            width: area.width,
            height: 1,
        },
    );
}

/// Render a hint line at the bottom of the area.
fn render_hint(f: &mut Frame, area: Rect, text: &str, style: Style) {
    let y = area.y + area.height.saturating_sub(1);
    f.render_widget(
        Paragraph::new(Span::styled(text, style)),
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
    );
}

/// Render a selectable list with pointer, scrolling window, and highlight.
#[allow(clippy::too_many_arguments)]
fn render_selectable_list(
    f: &mut Frame,
    area: Rect,
    y_offset: u16,
    items: &[String],
    selected: usize,
    styles: &oxi_tui::theme::ThemeStyles,
    theme: &Theme,
    highlight_color: Option<ratatui::style::Color>,
) {
    if items.is_empty() {
        return;
    }

    let list_area = Rect {
        x: area.x,
        y: area.y + y_offset,
        width: area.width,
        height: area.height.saturating_sub(y_offset).saturating_sub(1),
    };

    let max_show = list_area.height as usize;
    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else {
        0
    };

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .skip(window_start)
        .take(max_show)
        .map(|(i, text)| {
            let is_sel = i == selected;
            let pointer = if is_sel { "-> " } else { "   " };
            let content = format!("{}{}", pointer, text);

            let style = if is_sel {
                let hl = highlight_color.unwrap_or_else(|| theme.colors.primary.to_ratatui());
                Style::default()
                    .fg(theme.colors.background.to_ratatui())
                    .bg(hl)
                    .bold()
            } else {
                styles.normal
            };

            ListItem::new(Span::styled(content, style))
        })
        .collect();

    f.render_widget(List::new(list_items), list_area);
}

/// Render a list with status indicators (filled/empty circle).
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn render_status_list(
    f: &mut Frame,
    area: Rect,
    y_offset: u16,
    items: &[(String, bool)],
    selected: usize,
    styles: &oxi_tui::theme::ThemeStyles,
    theme: &Theme,
    highlight_color: Option<ratatui::style::Color>,
) {
    let strings: Vec<String> = items
        .iter()
        .map(|(name, has_key)| {
            let status = if *has_key { "*" } else { "o" };
            format!("{} {}", status, name)
        })
        .collect();
    render_selectable_list(
        f,
        area,
        y_offset,
        &strings,
        selected,
        styles,
        theme,
        highlight_color,
    );
}

/// Render provider selection list grouped by category with descriptions.
fn render_provider_list(
    f: &mut Frame,
    area: Rect,
    providers: &[ProviderInfo],
    selected: usize,
    _styles: &oxi_tui::theme::ThemeStyles,
    _theme: &Theme,
) {
    // Category display order and labels
    let category_order = [
        ("primary", "Primary Providers"),
        ("chinese", "Chinese AI"),
        ("open", "Open Providers"),
        ("cloud", "Cloud"),
        ("enterprise", "Enterprise"),
        ("specialized", "Other"),
    ];

    let mut idx = 0;
    let mut last_category = "";

    let mut lines: Vec<Line> = Vec::new();

    for (cat_key, cat_label) in &category_order {
        let cat_providers: Vec<&ProviderInfo> = providers
            .iter()
            .filter(|p| p.category == *cat_key)
            .collect();
        if cat_providers.is_empty() {
            continue;
        }

        // Category header
        if !cat_label.is_empty() && *cat_key != last_category {
            lines.push(Line::from(vec![Span::styled(
                format!("  {}", cat_label),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(""));
        }

        for p in &cat_providers {
            let is_selected = idx == selected;
            let check = if p.has_key { "✓" } else { "○" };

            let highlight = if is_selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let name_span = Span::styled(
                format!(" {} {} ", check, p.display_name),
                highlight.fg(if is_selected {
                    Color::White
                } else {
                    Color::Gray
                }),
            );

            let desc_span = if !p.description.is_empty() {
                Span::styled(
                    format!(" — {}", p.description),
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                Span::raw("")
            };

            let key_span = if p.has_key {
                Span::styled(" [key set]", Style::default().fg(Color::Green))
            } else {
                Span::raw("")
            };

            lines.push(Line::from(vec![name_span, desc_span, key_span]));
            idx += 1;
        }
        last_category = cat_key;
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, area);
}

/// Render a text input field with label and cursor.
fn render_input_field(
    f: &mut Frame,
    area: Rect,
    y_offset: u16,
    label: &str,
    value: &str,
    theme: &Theme,
    styles: &oxi_tui::theme::ThemeStyles,
) {
    let field_y = area.y + y_offset;
    let field_w = area.width.saturating_sub(4);
    let display: String = value.chars().take(field_w as usize).collect();
    let input_line = format!(" {}: {}", label, display);

    f.render_widget(
        Paragraph::new(Span::styled(input_line, styles.normal)),
        Rect {
            x: area.x + 2,
            y: field_y,
            width: field_w,
            height: 1,
        },
    );

    // Cursor
    let cursor_col = (label.len() as u16) + 3 + (display.len().min(field_w as usize) as u16);
    f.render_widget(
        Paragraph::new(Span::styled(
            " ",
            Style::default()
                .fg(theme.colors.cursor_fg.to_ratatui())
                .bg(theme.colors.cursor_bg.to_ratatui()),
        )),
        Rect {
            x: area.x + 2 + cursor_col,
            y: field_y,
            width: 1,
            height: 1,
        },
    );
}

// ── Main draw function ──────────────────────────────────────────────────

/// Main draw function — renders the full TUI frame.
pub fn draw(f: &mut Frame, state: &mut AppState, theme: &Theme) {
    let size = f.area();

    // Overlay takes over the entire screen
    if state.overlay.is_some() || state.overlay_state.is_some() {
        render_overlay(f, size, state, theme);
        return;
    }

    // Horizontal: 1 col left, 1 col right. Vertical: 0 top/bottom.
    let inner = size.inner(Margin::new(1, 0));

    // Calculate queue area height
    let queue_count = state.steering_messages_snapshot.len();
    let is_active = state.queue_panel_visible && queue_count > 0;
    let queue_lines = if queue_count > 0 {
        if is_active {
            // Active: selected highlight, up to 5 items + hint line
            (queue_count.min(5) as u16 + 1).min(6) // items + hint
        } else {
            // Compact: up to 3 message previews
            queue_count.min(3) as u16
        }
    } else {
        0
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),                      // Chat
            Constraint::Length(2 + queue_lines),      // Status + queue + input
            Constraint::Length(3),                    // Footer
        ])
        .split(inner);

    f.render_stateful_widget(ChatView::new(theme), chunks[0], &mut state.chat);

    // Input area (status line + optional queue + input)
    render_input_area(f, chunks[1], state, theme);

    // Slash popup — overlay above the input area
    if state.slash_completion_active {
        render_slash_popup_overlay(f, chunks[1], state, theme);
    }

    // Status bar
    f.render_stateful_widget(Footer::new(theme), chunks[2], &mut state.footer_state);
}

// ── Input area ──────────────────────────────────────────────────────────

fn render_input_area(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let queue_count = state.steering_messages_snapshot.len();
    let is_active = state.queue_panel_visible && queue_count > 0;
    let queue_lines = if queue_count > 0 {
        if is_active {
            (queue_count.min(5) as u16 + 1).min(6)
        } else {
            queue_count.min(3) as u16
        }
    } else {
        0
    };
    let _total_lines = 2 + queue_lines;

    // Row 0: status line (Working/Idle + queue count)
    let status_row = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    render_status_line(f, status_row, state, theme);

    // Queue preview / active panel rows
    if is_active {
        render_queue_active(f, area, state, theme);
    } else {
        render_queue_compact(f, area, state, theme);
    }

    // Last row: input
    let input_row = Rect {
        x: area.x,
        y: area.y + 1 + queue_lines,
        width: area.width,
        height: 1,
    };
    state.input.set_placeholder(None);
    f.render_stateful_widget(Input::new(theme), input_row, &mut state.input);
}

/// Compact queue preview: dim ⏳ lines with message text.
fn render_queue_compact(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let queue_count = state.steering_messages_snapshot.len().min(3);
    for (i, msg) in state
        .steering_messages_snapshot
        .iter()
        .take(queue_count)
        .enumerate()
    {
        let row = Rect {
            x: area.x,
            y: area.y + 1 + i as u16,
            width: area.width,
            height: 1,
        };
        let prefix = if i == 0 { "  ▶ " } else { "  ⏳ " };
        let max_chars = area.width.saturating_sub(8) as usize;
        let truncated: String = msg.chars().take(max_chars).collect();
        let ellipsis = if msg.chars().count() > max_chars { "…" } else { "" };
        let content = format!("{}{}{}", prefix, truncated, ellipsis);
        let style = if i == 0 {
            // First message = next to send — slightly brighter
            Style::default().fg(theme.colors.foreground.to_ratatui())
        } else {
            Style::default().fg(theme.colors.muted.to_ratatui())
        };
        f.render_widget(Paragraph::new(Span::styled(content, style)), row);
    }
}

/// Active queue panel: selected highlight, navigation, delete/edit.
fn render_queue_active(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let queue_count = state.steering_messages_snapshot.len();
    let max_items = queue_count.min(5);
    let selected = state.queue_panel_selected;

    // Item rows
    for (i, msg) in state
        .steering_messages_snapshot
        .iter()
        .take(max_items)
        .enumerate()
    {
        let row = Rect {
            x: area.x,
            y: area.y + 1 + i as u16,
            width: area.width,
            height: 1,
        };
        let is_sel = i == selected;
        let pointer = if is_sel { " ›" } else { "  " };
        let prefix = if i == 0 { "▶ " } else { "⏳ " };
        let max_chars = area.width.saturating_sub(8) as usize;
        let truncated: String = msg.chars().take(max_chars).collect();
        let ellipsis = if msg.chars().count() > max_chars { "…" } else { "" };
        let content = format!("{}{}{}{}", pointer, prefix, truncated, ellipsis);

        let style = if is_sel {
            Style::default()
                .fg(theme.colors.background.to_ratatui())
                .bg(theme.colors.accent.to_ratatui())
                .add_modifier(Modifier::BOLD)
        } else if i == 0 {
            Style::default().fg(theme.colors.foreground.to_ratatui())
        } else {
            Style::default().fg(theme.colors.muted.to_ratatui())
        };
        f.render_widget(Paragraph::new(Span::styled(content, style)), row);
    }

    // Hint line
    if max_items < area.height as usize - 1 {
        let row = Rect {
            x: area.x,
            y: area.y + 1 + max_items as u16,
            width: area.width,
            height: 1,
        };
        let hint = " ↑/↓  d del  e edit  Esc close";
        f.render_widget(
            Paragraph::new(Span::styled(
                hint,
                Style::default().fg(theme.colors.muted.to_ratatui()),
            )),
            row,
        );
    } else if queue_count > 0 {
        // Items filled all space — overwrite last item row with hint appended
        // Actually just render hint in the last allocated row
        let row = Rect {
            x: area.x,
            y: area.y + max_items as u16,
            width: area.width,
            height: 1,
        };
        let hint = " ↑/↓  d del  e edit  Esc close";
        f.render_widget(
            Paragraph::new(Span::styled(
                hint,
                Style::default().fg(theme.colors.muted.to_ratatui()),
            )),
            row,
        );
    }
}

/// Render the status/separator line with Working/Idle indicator and queue count.
fn render_status_line(f: &mut Frame, row: Rect, state: &AppState, theme: &Theme) {
    let queue_count = state.steering_messages_snapshot.len();

    if state.is_agent_busy {
        // Moon phase spinner ◐ ◓ ◑ ◒
        let sp = ["\u{25D0}", "\u{25D3}", "\u{25D1}", "\u{25D2}"];
        let ch = sp[state.spinner_frame % sp.len()];
        let label = if queue_count > 0 {
            format!(" {} Working  ⏳{} queued", ch, queue_count)
        } else {
            format!(" {} Working", ch)
        };
        let label_len: u16 = label.chars().map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) as u16).sum();
        let dash_count = row.width.saturating_sub(label_len + 2) as usize;
        let line = format!("── {} {}", label, "─".repeat(dash_count));
        f.render_widget(
            Paragraph::new(Span::styled(
                line,
                Style::default().fg(theme.colors.accent.to_ratatui()),
            )),
            row,
        );
    } else {
        let label = if queue_count > 0 {
            format!(" ○ Idle  ⏳{} queued", queue_count)
        } else {
            " ○ Idle".to_string()
        };
        let label_len: u16 = label.chars().map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) as u16).sum();
        let dash_count = row.width.saturating_sub(label_len + 2) as usize;
        let line = format!("── {} {}", label, "─".repeat(dash_count));
        f.render_widget(
            Paragraph::new(Span::styled(
                line,
                Style::default().fg(theme.colors.muted.to_ratatui()),
            )),
            row,
        );
    }
}

// ── Slash popup overlay ─────────────────────────────────────────────────

fn render_slash_popup_overlay(f: &mut Frame, input_area: Rect, state: &AppState, theme: &Theme) {
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

    let list_items: Vec<ListItem> = visible
        .iter()
        .map(|(i, comp)| {
            let is_selected = *i == selected;
            let pointer = if is_selected { "->" } else { "  " };
            let name_padded = format!("{:<width$}", comp.name, width = name_width);
            let desc_space = (popup_width as usize).saturating_sub(name_width + 8);
            let desc: String = comp.description.chars().take(desc_space).collect();

            if is_selected {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {} ", pointer),
                        Style::default().fg(theme.colors.accent.to_ratatui()),
                    ),
                    Span::styled(
                        format!(" {}  ", name_padded),
                        Style::default()
                            .fg(theme.colors.background.to_ratatui())
                            .bg(theme.colors.primary.to_ratatui())
                            .bold(),
                    ),
                    Span::styled(desc, Style::default().fg(theme.colors.muted.to_ratatui())),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", pointer), Style::default()),
                    Span::styled(
                        format!(" {}  ", name_padded),
                        Style::default().fg(theme.colors.foreground.to_ratatui()),
                    ),
                    Span::styled(desc, Style::default().fg(theme.colors.muted.to_ratatui())),
                ]))
            }
        })
        .collect();

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
    let popup_inner = block.inner(popup_area);
    f.render_widget(block, popup_area);
    f.render_widget(List::new(list_items), popup_inner);

    // Page indicator
    let page = window_start / max_show + 1;
    let total_pages = total.div_ceil(max_show);
    if total_pages > 1 {
        let indicator = format!("({}/{})", page, total_pages);
        let indicator_area = Rect {
            x: popup_area.x
                + popup_area
                    .width
                    .saturating_sub(indicator.chars().count() as u16 + 2),
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

// ── Overlay dispatch ────────────────────────────────────────────────────

fn render_overlay(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    // ── Component-based overlay (takes priority) ──
    if let Some(ref mut overlay) = state.overlay_state {
        overlay.render(f, area, theme);
        return;
    }
    // ── Legacy AppOverlay enum ──
    match &state.overlay {
        Some(AppOverlay::Setup(step)) | Some(AppOverlay::ProviderConfig(step)) => {
            let step = step.clone();
            render_setup_step(f, area, state, theme, &step);
        }
        Some(AppOverlay::ModelSelect { .. }) => {
            render_model_select(f, area, state, theme);
        }
        Some(AppOverlay::LogoutSelect { .. }) => {
            render_logout_select(f, area, state, theme);
        }
        Some(AppOverlay::ResumeSelect { .. }) => {
            render_resume_select(f, area, state, theme);
        }
        // RoutingStatus uses component-based overlay (overlay_state), handled above
        Some(AppOverlay::RoutingStatus { .. }) | None => {}
    }
}

// ── Setup wizard ────────────────────────────────────────────────────────

fn render_setup_step(
    f: &mut Frame,
    area: Rect,
    _state: &mut AppState,
    theme: &Theme,
    step: &SetupStep,
) {
    let styles = theme.to_styles();
    let bg = theme.colors.background.to_ratatui();
    let fg = theme.colors.primary.to_ratatui();

    match step {
        SetupStep::SelectAuthType { selected, .. } => {
            render_title(f, area, 2, " How would you like to authenticate?", fg, bg);
            let items = vec![
                " API Key    Enter an API key manually".to_string(),
                " OAuth      Sign in with your account (coming soon)".to_string(),
            ];
            render_selectable_list(f, area, 4, &items, *selected, &styles, theme, None);
            render_hint(
                f,
                area,
                " Up/Down select  |  Enter confirm  |  Esc cancel",
                styles.muted,
            );
        }

        SetupStep::SelectProvider {
            providers,
            selected,
            ..
        } => {
            render_title(f, area, 2, " Select a provider to get started", fg, bg);
            render_provider_list(f, area, providers, *selected, &styles, theme);
            render_hint(
                f,
                area,
                " Up/Down select  |  Enter confirm  |  q quit",
                styles.muted,
            );
        }

        SetupStep::EnterApiKey { provider, key, .. } => {
            let title = format!(" Enter API key for {}", provider);
            render_title(f, area, 3, &title, fg, bg);
            render_input_field(f, area, 5, "API Key", key, theme, &styles);
            render_hint(
                f,
                area,
                " Type your key  |  Enter save  |  Esc back",
                styles.muted,
            );
        }

        SetupStep::SelectModel {
            provider,
            models,
            selected,
        } => {
            let title = format!(" Select a model for {}", provider);
            render_title(f, area, 2, &title, fg, bg);
            render_selectable_list(f, area, 4, models, *selected, &styles, theme, None);
            let hint = format!(
                " Up/Down select  |  Enter confirm  |  Esc back  ({} models)",
                models.len()
            );
            render_hint(f, area, &hint, styles.muted);
        }

        SetupStep::Done { provider, model } => {
            let msg = format!(" {} is ready!", provider);
            let msg_y = area.y + 4;
            f.render_widget(
                Paragraph::new(Span::styled(
                    msg,
                    Style::default()
                        .fg(theme.colors.success.to_ratatui())
                        .bg(bg)
                        .bold(),
                )),
                Rect {
                    x: area.x + 2,
                    y: msg_y,
                    width: area.width.saturating_sub(4),
                    height: 1,
                },
            );
            let model_line = format!(" Model: {}", model);
            f.render_widget(
                Paragraph::new(Span::styled(model_line, styles.normal)),
                Rect {
                    x: area.x + 2,
                    y: msg_y + 1,
                    width: area.width.saturating_sub(4),
                    height: 1,
                },
            );
            f.render_widget(
                Paragraph::new(Span::styled(" Press Enter to start chatting", styles.muted)),
                Rect {
                    x: area.x + 2,
                    y: msg_y + 3,
                    width: area.width.saturating_sub(4),
                    height: 1,
                },
            );
        }
    }
}

// ── Model select overlay ────────────────────────────────────────────────

fn render_model_select(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let styles = theme.to_styles();

    let (models, filter, selected) = match &state.overlay {
        Some(AppOverlay::ModelSelect {
            models,
            filter,
            selected,
        }) => (models.clone(), filter.clone(), *selected),
        _ => return,
    };

    // Compute filtered models
    let filtered: Vec<String> = if filter.is_empty() {
        models.clone()
    } else {
        let lower = filter.to_lowercase();
        models
            .iter()
            .filter(|m| m.to_lowercase().contains(&lower))
            .cloned()
            .collect()
    };

    // Map selected index to filtered list
    let selected_in_filtered = if filter.is_empty() {
        selected
    } else {
        filtered
            .iter()
            .position(|m| m == &models[selected])
            .unwrap_or(0)
    };

    let popup = centered_popup(area, 0.7, 0.7);
    let inner = render_popup_frame(
        f,
        popup,
        theme.colors.background.to_ratatui(),
        theme.colors.border.to_ratatui(),
    );

    // Title + filter
    let title_line = if filter.is_empty() {
        " Select a model ".to_string()
    } else {
        format!(" Filter: {} ", filter)
    };
    render_title(
        f,
        inner,
        0,
        &title_line,
        theme.colors.primary.to_ratatui(),
        theme.colors.background.to_ratatui(),
    );

    // List
    render_selectable_list(
        f,
        inner,
        2,
        &filtered,
        selected_in_filtered,
        &styles,
        theme,
        None,
    );

    // Hint
    let hint = format!(
        " {} models  |  Up/Down  |  type to filter  |  Enter select  |  Esc cancel",
        filtered.len()
    );
    render_hint(f, inner, &hint, styles.muted);
}

// ── Logout select overlay ───────────────────────────────────────────────

fn render_logout_select(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let styles = theme.to_styles();

    let (providers, selected) = match &state.overlay {
        Some(AppOverlay::LogoutSelect {
            providers,
            selected,
        }) => (providers.clone(), *selected),
        _ => return,
    };

    let popup = centered_popup(area, 0.5, 0.5);
    let inner = render_popup_frame(
        f,
        popup,
        theme.colors.background.to_ratatui(),
        theme.colors.border.to_ratatui(),
    );

    render_title(
        f,
        inner,
        0,
        " Select provider to logout",
        theme.colors.error.to_ratatui(),
        theme.colors.background.to_ratatui(),
    );
    render_selectable_list(
        f,
        inner,
        2,
        &providers,
        selected,
        &styles,
        theme,
        Some(theme.colors.error.to_ratatui()),
    );
    render_hint(
        f,
        inner,
        " Up/Down select  |  Enter remove  |  Esc cancel",
        styles.muted,
    );
}

fn render_resume_select(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let styles = theme.to_styles();

    let sessions = match &state.overlay {
        Some(AppOverlay::ResumeSelect { sessions, .. }) => sessions.clone(),
        _ => return,
    };

    if sessions.is_empty() {
        return;
    }

    let selected = match &state.overlay {
        Some(AppOverlay::ResumeSelect { selected, .. }) => *selected,
        _ => 0,
    };

    // Build session labels
    let items: Vec<String> = sessions
        .iter()
        .map(|s| {
            let name_or_id = s
                .name
                .clone()
                .unwrap_or_else(|| s.id[..8.min(s.id.len())].to_string());
            format!("{} — {} ({} messages)", name_or_id, s.cwd, s.message_count)
        })
        .collect();

    let popup = centered_popup(area, 0.6, 0.6);
    let inner = render_popup_frame(
        f,
        popup,
        theme.colors.background.to_ratatui(),
        theme.colors.border.to_ratatui(),
    );

    render_title(
        f,
        inner,
        0,
        " Resume session",
        theme.colors.primary.to_ratatui(),
        theme.colors.background.to_ratatui(),
    );
    render_selectable_list(f, inner, 2, &items, selected, &styles, theme, None);
    render_hint(
        f,
        inner,
        " Up/Down select  |  Enter resume  |  Esc cancel",
        styles.muted,
    );
}
