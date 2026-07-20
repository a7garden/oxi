//! Full-frame ratatui rendering from PagerState.
//!
//! Replaces the old TUI render. Draws scrollback, token bar, and prompt
//! from `PagerState`. When a modal is active, falls back to a simple
//! overlay placeholder (the old overlay components handle rich overlay
//! rendering through a separate code path).

use crate::scrollback::BlockKind;
use crate::state::PagerState;
use crate::widgets::token_bar::token_bar_line;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

/// Render the full TUI frame from PagerState.
///
/// Lays out:
/// 1. Chat scrollback (blocks from agent/assistant/user messages)
/// 2. Token bar / status line
/// 3. Prompt / input area
pub fn render(frame: &mut Frame, state: &PagerState) {
    let area = frame.area();
    if area.width < 10 || area.height < 3 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // scrollback
            Constraint::Length(1), // token bar
            Constraint::Length(3), // prompt
        ])
        .split(area);

    render_scrollback(frame, chunks[0], state);
    render_token_bar(frame, chunks[1], state);
    render_prompt(frame, chunks[2], state);
}

fn render_scrollback(frame: &mut Frame, area: Rect, state: &PagerState) {
    let items: Vec<ListItem> = state
        .scrollback
        .blocks
        .iter()
        .map(|block| {
            let style = match block.kind {
                BlockKind::User => Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
                BlockKind::Assistant => Style::new().fg(Color::Cyan),
                BlockKind::ToolCall { .. } => Style::new().fg(Color::Yellow).dim(),
                BlockKind::ToolResult { .. } => Style::new().fg(Color::White).dim(),
                BlockKind::System => Style::new().fg(Color::Magenta).dim(),
            };
            ListItem::new(Line::from(Span::styled(block.text.clone(), style)))
        })
        .collect();

    let widget = if items.is_empty() {
        List::new(vec![ListItem::new(Line::from(Span::styled(
            "✨ oxi — waiting for your first message",
            Style::new().fg(Color::DarkGray),
        )))])
    } else {
        List::new(items)
    }
    .block(Block::default().borders(Borders::NONE));

    frame.render_widget(widget, area);
}

fn render_token_bar(frame: &mut Frame, area: Rect, state: &PagerState) {
    let line = token_bar_line(&state.status);
    let widget = Paragraph::new(Line::from(Span::styled(
        line,
        Style::new()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(widget, area);
}

fn render_prompt(frame: &mut Frame, area: Rect, state: &PagerState) {
    let prefix = "> ";
    let display = if state.prompt.text.is_empty() {
        format!("{prefix}type a message...")
    } else {
        format!("{}{}", prefix, state.prompt.text)
    };
    let widget = Paragraph::new(Line::from(Span::styled(
        display,
        Style::new().fg(Color::White),
    )))
    .block(Block::default().borders(Borders::ALL).title(" Input "))
    .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}
