//! Rendering functions for the TUI.
//!
//! Delegates the main chat surface to `oxi_pager::render::render`, which
//! drives the vendored grok-build render pipeline. This module keeps:
//! - overlay rendering (settings, issues, MCP) — old TUI path
//! - notification toasts — rendered on top of everything
//! - AppState → PagerState sync each frame

use super::app::{AppState, NotificationKind};
use oxi_tui::theme::Theme;
use oxi_tui::widgets::chat::{ChatViewState, ContentBlock, FollowMode, MessageRole};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

/// Main draw function — syncs AppState into PagerState, then renders.
pub fn draw(f: &mut Frame, state: &mut AppState, theme: &Theme) {
    let size = f.area();

    if state.overlay.is_some() || state.overlay_state.is_some() {
        render_overlay(f, size, state, theme);
        return;
    }

    if let Some(ref pager_state) = state.pager_state {
        sync_app_state_into_pager(state, pager_state);
        let ps = pager_state.read();
        oxi_pager::render::render(f, &ps, theme);
        drop(ps);
    }

    render_notifications(f, size, state, theme);
}

/// Mirror AppState → PagerState each frame. The pager's scrollback is
/// rebuilt from `chat.messages` so the vendored grok render path sees
/// user/assistant/tool blocks identically to the old TUI widget path.
fn sync_app_state_into_pager(state: &AppState, pager_state: &oxi_pager::SharedState) {
    let mut ps = pager_state.write();
    ps.prompt.text = state.input.text();
    ps.status.spinner_phase = (state.spinner_frame % 12) as u8;
    if let Some(ref sid) = state.session_file_path {
        ps.status.session_id = Some(sid.clone());
    }
    ps.scrollback.blocks = build_pager_blocks(&state.chat);
    ps.scrollback.follow_tail = matches!(state.chat.follow, FollowMode::Following);
}

/// Map `ChatViewState.messages` → `RenderedBlock`s the vendored grok render
/// can draw. One `ContentBlock` becomes one `RenderedBlock`; a chat message
/// with multiple blocks becomes multiple render blocks (preserving order).
fn build_pager_blocks(chat: &ChatViewState) -> Vec<oxi_pager::scrollback::RenderedBlock> {
    let mut out: Vec<oxi_pager::scrollback::RenderedBlock> = Vec::new();
    for msg in &chat.messages {
        for block in &msg.content_blocks {
            let (kind, text) = match block {
                ContentBlock::Text { content } => {
                    let kind = match msg.role {
                        MessageRole::User => oxi_pager::scrollback::BlockKind::User,
                        MessageRole::Assistant => oxi_pager::scrollback::BlockKind::Assistant,
                        MessageRole::System => oxi_pager::scrollback::BlockKind::System,
                    };
                    (kind, content.clone())
                }
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    result,
                    ..
                } => {
                    let kind = oxi_pager::scrollback::BlockKind::ToolCall {
                        name: name.clone(),
                        call_id: id.clone(),
                    };
                    let text = match result {
                        Some((r, _is_error)) => format!("{}\n{}", arguments, r),
                        None => arguments.clone(),
                    };
                    (kind, text)
                }
                ContentBlock::ToolResult {
                    tool_name, content, ..
                } => {
                    let kind = oxi_pager::scrollback::BlockKind::ToolResult {
                        call_id: tool_name.clone(),
                    };
                    (kind, content.clone())
                }
                ContentBlock::Thinking { content, .. } => {
                    let kind = oxi_pager::scrollback::BlockKind::System;
                    (kind, format!("(thinking) {}", content))
                }
                ContentBlock::Error { title, message, .. } => {
                    let kind = oxi_pager::scrollback::BlockKind::System;
                    (kind, format!("error: {} — {}", title, message))
                }
                // Dashboard / Image are not part of the text render path.
                _ => continue,
            };
            out.push(oxi_pager::scrollback::RenderedBlock {
                id: out.len() as u64,
                kind,
                text,
            });
        }
    }
    out
}

#[allow(dead_code, unused_variables)]
fn render_overlay(f: &mut Frame, _area: Rect, state: &mut AppState, _theme: &Theme) {
    // Overlay mode is dispatched elsewhere; placeholder keeps the signature stable.
    let _ = (f, state);
}

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
