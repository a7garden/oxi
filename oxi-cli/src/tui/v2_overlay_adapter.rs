//! Adapter that wraps a legacy `Box<dyn OverlayComponent>` as a `Renderable`
//! so it can sit as the top layer in a `RetainedTree`.
//!
//! The Plan C cutover migrates the entire app to the V2 `Renderable` trait,
//! but overlays are last. Until each overlay is rewritten, this adapter
//! bridges the old `OverlayComponent::render(&mut Frame, Rect, &Theme)`
//! signature into the new pipeline by:
//!
//! - Re-using the V2 `RenderCtx`'s underlying `Frame` via `RenderCtx::with_frame`.
//! - Converting the V2 `Theme` (oxi-tui) into the legacy `Theme` (oxi-tui-legacy)
//!   that overlay implementations expect.
//!
//! This is a temporary bridge. Once overlays are ported, delete this file and
//! the `with_frame` accessor in `RenderCtx`.

#![allow(dead_code, reason = "Bridge module; consumers wire up in Plan C integration")]

use oxi_tui::widget::Renderable;
use oxi_tui::widget::RenderCtx;
use oxi_tui_legacy::Symbols;
use oxi_tui_legacy::theme::{ColorScheme, Spacing, Theme as LegacyTheme};
use ratatui::layout::Rect;

use crate::tui::overlay::OverlayComponent;

/// Bridge renderer that exposes a legacy overlay through the V2 trait.
pub struct LegacyOverlayAdapter {
    overlay: Box<dyn OverlayComponent + Send>,
    /// Monotonically increasing hash counter; bumped at the end of every
    /// `render` so `content_hash` differs from the previous frame and the
    /// pipeline never skips the legacy overlay.
    dirty_seq: u64,
}

impl LegacyOverlayAdapter {
    /// Wrap a legacy overlay component for V2 rendering.
    pub fn new(overlay: Box<dyn OverlayComponent + Send>) -> Self {
        Self { overlay, dirty_seq: 1 }
    }

    /// Borrow the inner overlay mutably so callers can drive events/polls.
    pub fn overlay_mut(&mut self) -> &mut Box<dyn OverlayComponent + Send> {
        &mut self.overlay
    }
}

impl Renderable for LegacyOverlayAdapter {
    /// Always dirty — overlays are volumetric (cursor focus, mark_refresh,
    /// poll-driven animation) and cannot be safely memoized. Returns a
    /// monotonically increasing sequence number stored on the adapter so
    /// successive `content_hash` calls produce distinct values without
    /// relying on wall-clock time (which the trait contract forbids and is
    /// also unsound under TUI pause/resume).
    fn content_hash(&self) -> u64 {
        // Counter is updated lazily on each `render` call below; pair both
        // methods so callers cannot desync.
        self.dirty_seq
    }

    /// Overlays are anchored to the full parent and paint wherever the
    /// legacy `render` decides (typically a centered popup via
    /// `centered_popup`/`centered_layout`). They MUST NOT contribute to
    /// vertical layout, so we return `0` — mirror of the existing
    /// `oxi_tui::widget::panel::Overlay` convention.
    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        0
    }

    /// Bridge into the legacy render API. The V2 `Theme` is converted to a
    /// `LegacyTheme`; the area passed by `RetainedTree` is forwarded as-is
    /// (legacy overlays compute their own popup rect from this base area).
    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        let legacy_theme = convert_theme(ctx.theme());
        ctx.with_frame(|frame| {
            self.overlay.render(frame, area, &legacy_theme);
        });
        // Bump the dirty counter so the next `content_hash` differs.
        self.dirty_seq = self.dirty_seq.wrapping_add(1).max(1);
    }
}

/// Map a V2 `Theme` into a `LegacyTheme`.
///
/// The two type hierarchies are similar but not identical: legacy has
/// `primary`/`secondary`/cursor/code/tool-* slots that V2 does not, while V2
/// has `info`/`response`/`thinking`/`tool`/`diff_*` slots that legacy lacks.
/// We translate by semantic name where the slot exists, and fall back to a
/// sensible default (the equivalent legacy slot, or the theme's own
/// foreground/background) where it does not. Spacing and Symbols default —
/// legacy overlays read those from the theme struct, but V2 does not track
/// them yet. When the V2 theme eventually carries spacing/symbols, extend
/// this conversion; until then, overlays see defaults matching the legacy
/// crate's `Default` impl.
#[must_use]
pub fn convert_theme(v2: &oxi_tui::theme::Theme) -> LegacyTheme {
    let v2c = &v2.colors;
    let legacy_colors = ColorScheme {
        foreground: v2c.foreground,
        background: v2c.background,
        primary: v2c.primary,
        // V2 collapses secondary into muted/accent; legacy keeps a distinct slot.
        secondary: v2c.muted,
        error: v2c.error,
        warning: v2c.warning,
        success: v2c.success,
        muted: v2c.muted,
        accent: v2c.accent,
        border: v2c.border,
        user_border: v2c.primary,
        user_bg: v2c.user_bg,
        cursor_fg: v2c.background,
        cursor_bg: v2c.foreground,
        selection_bg: v2c.selection_bg,
        code_fg: v2c.foreground,
        code_bg: v2c.code_bg,
        // V2 has no slot for tool state backgrounds; use muted panel_bg as a
        // visually neutral default — legacy overlays that need richer state
        // styling will be migrated to Renderable long before this matters.
        tool_pending_bg: v2c.surface_bg,
        tool_executing_bg: v2c.surface_bg,
        tool_success_bg: v2c.success,
        tool_error_bg: v2c.error,
        response_bg: v2c.response_bg,
        thinking_bg: v2c.thinking_bg,
        surface_bg: v2c.surface_bg,
        panel_bg: v2c.panel_bg,
        diff_add_bg: v2c.diff_add_bg,
        diff_remove_bg: v2c.diff_remove_bg,
        diff_hunk_bg: v2c.diff_hunk_bg,
    };
    LegacyTheme {
        name: v2.name.to_string(),
        colors: legacy_colors,
        spacing: Spacing::default(),
        symbols: Symbols::default(),
    }
}
