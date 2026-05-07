//! Rendering functions for the TUI.

use super::app::{AppState, SPINNER};
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

    // Layout: Chat | Separator(1) | Input(3) | Status bar(1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),   // Chat
            Constraint::Length(1), // Separator
            Constraint::Length(3), // Input (border + input + hint/popup)
            Constraint::Length(1), // Status bar
        ])
        .split(size);

    // Chat
    f.render_stateful_widget(ChatView::new(theme), chunks[0], &mut state.chat);

    // Separator
    render_separator(f, chunks[1], theme);

    // Input area
    render_input_area(f, chunks[2], state, theme);

    // Slash popup — overlay above the input area (drawn last so it's on top)
    if state.slash_completion_active {
        render_slash_popup_overlay(f, chunks[1], chunks[2], state, theme);
    }

    // Status bar
    f.render_stateful_widget(Footer::new(theme), chunks[3], &mut state.footer_state);
}

// ── Input area ───────────────────────────────────────────────────────────

fn render_input_area(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    if area.height < 2 {
        return;
    }
    let input_row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let hint_row = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: 1,
    };
    let border_row = Rect {
        x: area.x,
        y: area.y + 2,
        width: area.width,
        height: 1,
    };

    if state.is_agent_busy {
        render_busy_input(f, input_row, state, theme);
    } else {
        f.render_stateful_widget(
            Input::new(theme).with_placeholder("Type a message… (enter / for commands)"),
            input_row,
            &mut state.input,
        );
    }

    // Hint row (only when no slash popup)
    if state.is_agent_busy {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Ctrl+C to interrupt",
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ))),
            hint_row,
        );
    } else if state.input_value().is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Enter · / commands · ↑ history · Esc cancel",
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ))),
            hint_row,
        );
    } else {
        let count = state.input.text.chars().count();
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {} chars", count),
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ))),
            hint_row,
        );
    }

    render_separator(f, border_row, theme);
}

// ── Busy input (spinner) ─────────────────────────────────────────────────

fn render_busy_input(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let prompt = format!("{} ", SPINNER[state.spinner_frame]);
    let display = if state.input_value().is_empty() {
        "waiting for response…"
    } else {
        state.input_value()
    };
    let text_fg = if state.input_value().is_empty() {
        theme.colors.muted.to_ratatui()
    } else {
        theme.colors.foreground.to_ratatui()
    };

    let spans = vec![
        Span::styled(prompt, Style::default().fg(theme.colors.accent.to_ratatui())),
        Span::styled(display.to_string(), Style::default().fg(text_fg)),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── Slash popup (Pi-style vertical list overlay) ─────────────────────────

/// Render the slash command popup as a vertical list overlaying the chat area,
/// positioned just above the separator/input rows — matching Pi's `/` UI.
fn render_slash_popup_overlay(
    f: &mut Frame,
    separator_area: Rect,
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

    // Scroll window so the selected item is always visible
    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else {
        0
    };

    // Popup dimensions: full width, positioned above the separator
    let popup_width = separator_area.width;
    let popup_height = max_show as u16 + 2; // +2 for top/bottom borders
    let popup_x = separator_area.x;
    let popup_y = separator_area.y.saturating_sub(popup_height);

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    // Clear the area behind the popup
    f.render_widget(Clear, popup_area);

    // Build vertical list lines
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

        // Truncate description to fit
        let desc_space = (popup_width as usize).saturating_sub(name_width + 8);
        let desc: String = comp
            .description
            .chars()
            .take(desc_space)
            .collect();

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

    // Render with a bordered block
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.colors.border.to_ratatui()));

    let popup_inner = block.inner(popup_area);
    f.render_widget(block, popup_area);
    f.render_widget(Paragraph::new(lines), popup_inner);

    // Page indicator at bottom-right of popup
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

// ── Separator ────────────────────────────────────────────────────────────

pub(crate) fn render_separator(f: &mut Frame, area: Rect, theme: &Theme) {
    let w = area.width as usize;
    let mut spans: Vec<Span> = Vec::with_capacity(w);
    for i in 0..w {
        let c = match i % 4 {
            0 => '─',
            1 => '·',
            2 => '·',
            _ => ' ',
        };
        spans.push(Span::styled(
            c.to_string(),
            Style::default().fg(theme.colors.border.to_ratatui()),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
