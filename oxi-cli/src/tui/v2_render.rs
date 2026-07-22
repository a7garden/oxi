//! v2 rendering path — renders the chat area using the new oxi_tui ChatView widget.
//!
//! Gated behind `OXI_V2_RENDER=1` for safe rollout. When enabled, this function
//! replaces the legacy chat widget rendering with the new v2 widget while keeping
//! the legacy overlay/footer/status/notifications rendering intact.
//!
//! Current status: infrastructure-only. `draw_v2` delegates to the legacy
//! `render::draw` to prove the env-var gating wiring is correct. The actual
//! ChatView rendering will replace the delegation in a follow-up commit.

use super::app::AppState;
use oxi_tui_legacy::Theme;
use ratatui::Frame;

/// Draw a frame using the v2 ChatView-based rendering path.
///
/// Until the v2 ChatView bridging (state conversion, scroll/follow, theme
/// conversion) lands, this delegates to the legacy `render::draw`. The layout
/// (chat area, todo panel, input, footer, overlays, notifications) stays
/// identical, so the visible output is byte-for-byte equivalent to the legacy
/// path. Once bridging is complete, this function will render the chat area
/// via `oxi_tui::widget::chat::ChatView` and delegate only the footer/input/
/// overlay rendering to the legacy path.
pub fn draw_v2(f: &mut Frame, state: &mut AppState, theme: &Theme) {
    // Delegate to legacy rendering for now. The follow-up will replace the
    // chat-area rendering with `oxi_tui::widget::chat::ChatView` while keeping
    // overlays/footer/notifications rendered by the legacy path.
    super::render::draw(f, state, theme);
}
