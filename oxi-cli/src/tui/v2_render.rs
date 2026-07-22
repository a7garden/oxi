//! v2 rendering path — renders the chat area using the new `oxi_tui` widgets
//! while keeping the bottom band clear for legacy footer / input rendering.
//!
//! Gated behind `OXI_V2_RENDER=1` for safe rollout. When enabled:
//! - The chat area is rendered by [`oxi_tui::widget::chat::ChatView`] (the new
//!   retained-tree widget) using its `Renderable` impl and the `RenderCtx`.
//! - The chat log is synced from `state.v2_chat` (dual-write source) into the
//!   widget's own `ChatLog` every frame.
//! - The bottom band (reserved for footer / input / status) is cleared; the
//!   eventual follow-up will render the legacy chrome there via `with_frame`.

use super::app::AppState;
use oxi_tui::content::ChatLog;
use oxi_tui::widget::chat::ChatView;
use oxi_tui::widget::{RenderCtx, Renderable};
use ratatui::layout::Rect;
use ratatui::widgets::Clear;

/// Reserve the bottom four rows for legacy footer / input / status rendering.
///
/// Matches the legacy `render::draw` split: `Constraint::Length(3)` footer +
/// dynamic status + queue + input. Until the legacy chrome delegation lands,
/// the bottom band is cleared each frame so the new ChatView doesn't bleed
/// under it.
const LEGACY_BOTTOM_ROWS: u16 = 4;

/// Draw a frame using the v2 ChatView-based rendering path.
///
/// Paints the chat area using [`oxi_tui::widget::chat::ChatView`] via its
/// `Renderable` impl, then clears the reserved bottom band for legacy
/// footer / input rendering (a follow-up will wire those in via
/// `ctx.with_frame`).
pub fn draw_v2(ctx: &mut RenderCtx, state: &mut AppState) {
    let area = ctx.area();

    // Chat area: everything except the bottom LEGACY_BOTTOM_ROWS rows.
    let chat_height = area.height.saturating_sub(LEGACY_BOTTOM_ROWS);
    let chat_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: chat_height,
    };

    // Sync ChatView's log from the dual-write ChatLog every frame.
    // O(N) per frame is acceptable for the cutover phase; an incremental
    // sync (replaying only new messages / tokens) is a follow-up.
    sync_chat_view(&mut state.v2_chat_view, &state.v2_chat);

    // Paint the new ChatView into the chat area via its Renderable impl.
    state.v2_chat_view.render(chat_area, ctx);

    // Clear the bottom band reserved for legacy chrome. The follow-up
    // commit will render the legacy footer / input / status here via
    // `ctx.with_frame(|frame| super::render::draw_chrome(frame, ...))`.
    let bottom = Rect {
        x: area.x,
        y: area.y.saturating_add(chat_height),
        width: area.width,
        height: LEGACY_BOTTOM_ROWS,
    };
    ctx.with_frame(|frame| {
        frame.render_widget(Clear, bottom);
    });
}

/// Replace [`ChatView`]'s log with a clone of the dual-write source.
///
/// `ChatLog` is append-only by construction; cloning the whole log per
/// frame is correct and O(N). For high-message-count sessions, switch to
/// incremental sync that only replays messages / tokens added since the
/// last frame.
fn sync_chat_view(view: &mut ChatView, log: &ChatLog) {
    *view.log_mut() = log.clone();
}
