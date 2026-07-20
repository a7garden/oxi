//! Ratatui overlay rendering from PagerState.
//!
//! Renders incremental pager overlays on top of the existing TUI frame.
//! Called AFTER the old TUI's `draw()` so that the pager's additions
//! (token bar, spinner) sit on top rather than being overwritten.

use crate::state::PagerState;
use crate::widgets::token_bar::token_bar_line;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

/// Render pager overlays into the frame.
///
/// This should be called AFTER the old TUI's `draw()` so overlays are
/// drawn on top. Currently renders:
/// - Token bar (model + spinner + tokens + cost) when status is populated
///
/// Future PRs will migrate the chat area, prompt, and modals.
pub fn render_overlay(frame: &mut Frame, state: &PagerState) {
    let area = frame.area();
    if area.width < 10 || area.height < 3 {
        return;
    }

    // Only render token bar if there's data to show (model set).
    if state.status.model.is_some() || state.status.tokens_in > 0 || state.status.tokens_out > 0 {
        render_token_bar(frame, area, state);
    }
}

fn render_token_bar(frame: &mut Frame, area: Rect, state: &PagerState) {
    let bar_rect = Rect {
        x: area.left(),
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };

    // Clear the area before rendering.
    frame.render_widget(Clear, bar_rect);

    let line = token_bar_line(&state.status);
    let widget = Paragraph::new(Line::from(Span::styled(
        line,
        Style::new()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD),
    )))
    .block(Block::default().borders(Borders::NONE));
    frame.render_widget(widget, bar_rect);
}
