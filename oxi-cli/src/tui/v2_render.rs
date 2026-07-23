//! v2 rendering path — delegates the entire frame to the legacy
//! `render::draw` through the v2 pipeline's `RenderCtx`.
//!
//! This is the cutover hybrid: the v2 `draw_frame_closure` owns cursor-blink
//! dedup, CSI 2026 sync, DECCARA background fills, and cell-level diffing,
//! while the actual widget painting still runs through the proven legacy
//! renderer. The two dispatch paths (`draw_v2` here vs. `render::draw` in the
//! fallback) are visually identical — this is the "both paths render
//! identically" guarantee the main loop relies on.
//!
//! The legacy renderer never touches the v2 cursor slot, so the main-loop
//! dispatch bridges the input cursor via `ctx.set_cursor` after this function
//! returns, reading `AppState::last_input_cursor` that `render_input_area`
//! populated during the delegated draw.
//!
//! Rendering the chat area via the v2 `ChatView` widget (which requires
//! completing the dual-write of all message types — user, tool, thinking —
//! plus incremental sync) is a separate follow-up. `AppState::v2_chat` /
//! `v2_chat_view` are kept for that future work but are not wired into the
//! default path yet.

use super::app::AppState;
use super::render;
use oxi_tui::widget::RenderCtx;
use oxi_tui_legacy::theme::Theme;

/// Draw one frame through the v2 pipeline, delegating all widget painting to
/// the legacy `render::draw`.
///
/// `state.last_input_cursor` is populated by `render::render_input_area`
/// during the delegated draw; the main-loop dispatch reads it and calls
/// `ctx.set_cursor` so the v2 `CursorState` positions the terminal cursor.
pub fn draw_v2(ctx: &mut RenderCtx, state: &mut AppState, theme: &Theme) {
    ctx.with_frame(|frame| render::draw(frame, state, theme));
}
