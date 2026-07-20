//! Rendering functions for the TUI.
//!
//! Most rendering is now delegated to the pager's `render()` function.
//! This module only handles:
//! - Overlay rendering (settings, issues, MCP, etc.) — still uses old TUI path
//! - Notification toasts — still rendered on top of everything

use super::app::{AppState, NotificationKind};
use oxi_tui::theme::Theme;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

/// Main draw function — renders the full TUI frame.
pub fn draw(f: &mut Frame, state: &mut AppState, theme: &Theme) {
    let size = f.area();
    if state.overlay.is_some() || state.overlay_state.is_some() {
        render_overlay(f, size, state, theme);
        return;
    }
    if let Some(ref pager_state) = state.pager_state {
        let ps = pager_state.read();
        oxi_pager::render::render(f, &ps);
        drop(ps);
    }
    render_notifications(f, size, state, theme);
}

#[allow(dead_code, unused_variables)]
fn render_overlay(f: &mut Frame, _area: Rect, state: &mut AppState, _theme: &Theme) {}

fn render_notifications(f: &mut Frame, area: Rect, state: &AppState, _theme: &Theme) {
    let last = match state.notifications.last() {
        Some(n) => n,
        None => return,
    };
    let fg = match last.kind {
        NotificationKind::Info => Style::new().cyan(),
        NotificationKind::Warning => Style::new().yellow(),
        NotificationKind::Error => Style::new().red(),
        NotificationKind::Success => Style::new().green(),
    };
    let text = Line::from(Span::styled(&last.message, fg));
    let widget = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Notification "),
    );
    let w = area.width.min(60u16.max(last.message.len() as u16 + 4));
    let h = 3;
    let rect = Rect {
        x: area.right().saturating_sub(w),
        y: area.bottom().saturating_sub(h),
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(widget, rect);
}
