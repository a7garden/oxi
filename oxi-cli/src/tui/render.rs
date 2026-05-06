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
    widgets::Paragraph,
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

    // Hint / popup row
    if state.slash_completion_active {
        render_slash_popup(f, hint_row, state, theme);
    } else if state.is_agent_busy {
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

// ── Slash popup ──────────────────────────────────────────────────────────

fn render_slash_popup(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if state.slash_completions.is_empty() {
        return;
    }
    let selected = state.slash_completion_index;
    let max_show = 6usize;
    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else {
        0
    };

    let mut spans: Vec<Span> = vec![Span::styled("  ", Style::default())];

    let visible: Vec<_> = state
        .slash_completions
        .iter()
        .enumerate()
        .skip(window_start)
        .take(max_show)
        .collect();

    for (i, comp) in &visible {
        if *i == selected {
            spans.push(Span::styled(
                format!(" {} ", comp.name),
                Style::default()
                    .fg(theme.colors.background.to_ratatui())
                    .bg(theme.colors.primary.to_ratatui())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" ", Style::default()));
        } else {
            spans.push(Span::styled(
                format!(" {} ", comp.name),
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ));
            spans.push(Span::styled(" ", Style::default()));
        }
    }

    if let Some(comp) = state.slash_completions.get(selected) {
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let remaining = area.width as usize;
        let desc_max = remaining.saturating_sub(used + 4);
        if desc_max > 5 {
            let desc: String = comp.description.chars().take(desc_max).collect();
            spans.push(Span::styled(
                format!("— {}", desc),
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ));
        }
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
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
