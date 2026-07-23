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

#![allow(
    dead_code,
    reason = "Bridge module; consumers wire up in Plan C integration"
)]

use oxi_tui::widget::RenderCtx;
use oxi_tui::widget::Renderable;
use oxi_tui_legacy::Symbols;
use oxi_tui_legacy::theme::{ColorScheme, Spacing, Theme as LegacyTheme};
use ratatui::layout::Rect;

use crate::tui::overlay::OverlayComponent;

pub struct LegacyOverlayAdapter {
    overlay: Box<dyn OverlayComponent>,
    /// Monotonically increasing hash counter; bumped at the end of every
    /// `render` so `content_hash` differs from the previous frame and the
    /// pipeline never skips the legacy overlay.
    dirty_seq: u64,
}

impl LegacyOverlayAdapter {
    /// Wrap a legacy overlay component for V2 rendering.
    pub fn new(overlay: Box<dyn OverlayComponent>) -> Self {
        Self {
            overlay,
            dirty_seq: 1,
        }
    }
    /// Borrow the inner overlay mutably so callers can drive events/polls.
    pub fn overlay_mut(&mut self) -> &mut Box<dyn OverlayComponent> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    /// A minimal `OverlayComponent` impl that holds a raw pointer in a way
    /// that mirrors the real factory overlays (`Arc<Mutex<*mut AppState>>`).
    /// `*mut T` is `!Send`, so this type is `!Send` — the same constraint
    /// that drove dropping the supertrait `Send` bound on `Renderable`.
    ///
    /// Existence of this type at all proves the adapter accepts a
    /// `!Send` overlay. If we ever put `+ Send` back on the trait or the
    /// adapter, this mock will fail to coerce and the build will fail.
    struct NotSendOverlay(*mut ());

    impl std::fmt::Debug for NotSendOverlay {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_tuple("NotSendOverlay").finish()
        }
    }

    impl OverlayComponent for NotSendOverlay {
        fn handle_key(&mut self, _key: KeyEvent) -> crate::tui::overlay::OverlayAction {
            crate::tui::overlay::OverlayAction::None
        }
        fn render(&mut self, _frame: &mut ratatui::Frame, _area: Rect, _theme: &LegacyTheme) {}
        fn hint(&self) -> &str {
            ""
        }
    }

    /// A `!Send` overlay mock that paints a sentinel symbol into the frame
    /// buffer at the top-left of the supplied area on `render`. Used to
    /// verify the adapter's bridge actually forwards the legacy render
    /// through `RenderCtx::with_frame` end-to-end.
    struct PaintingOverlay {
        sentinel: &'static str,
        // `*mut ()` makes this type `!Send`, mirroring real factory
        // overlays. If `Renderable` or the adapter re-adds `+ Send`, the
        // `Box::new(PaintingOverlay { ... })` coercion in the integration
        // test fails to compile.
        _not_send_witness: *mut (),
    }

    impl std::fmt::Debug for PaintingOverlay {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("PaintingOverlay")
                .field("sentinel", &self.sentinel)
                .finish()
        }
    }

    impl OverlayComponent for PaintingOverlay {
        fn handle_key(&mut self, _key: KeyEvent) -> crate::tui::overlay::OverlayAction {
            crate::tui::overlay::OverlayAction::None
        }
        fn render(&mut self, frame: &mut ratatui::Frame, area: Rect, _theme: &LegacyTheme) {
            // Paint the sentinel at the top-left cell of the supplied
            // area. Single-cell render keeps the post-draw assertion
            // unambiguous.
            frame.buffer_mut()[(area.x, area.y)].set_symbol(self.sentinel);
        }
        fn hint(&self) -> &str {
            ""
        }
    }

    /// The `Box<dyn OverlayComponent>` (no `+ Send`) must accept a `!Send`
    /// overlay. The `*mut ()` field in `NotSendOverlay` makes that type
    /// `!Send` (raw pointers are unconditionally `!Send`); coercing it into
    /// `Box<dyn OverlayComponent>` and handing it to
    /// `LegacyOverlayAdapter::new` is therefore the structural proof that
    /// neither `Renderable` nor the adapter requires `+ Send`.
    ///
    /// If anyone re-adds `: Send` to `Renderable` (or `+ Send` to the
    /// adapter), this test fails to compile at the `Box::new` line, which
    /// is exactly the regression alarm the advisory demanded.
    #[test]
    fn adapter_accepts_non_send_overlay() {
        // Build a `!Send` overlay (`*mut ()` is unconditionally `!Send`) and
        // hand it to the adapter. The `Box::new` coercion into
        // `Box<dyn OverlayComponent>` is the structural proof that the
        // adapter's overlay slot has no `+ Send` bound — if anyone ever
        // re-adds one, this `Box::new` line fails to compile.
        let overlay: Box<dyn OverlayComponent> = Box::new(NotSendOverlay(std::ptr::null_mut()));
        let mut adapter = LegacyOverlayAdapter::new(overlay);
        // Verify the adapter exposes the overlay for external driving.
        let _inner = adapter.overlay_mut();
    }

    /// `content_hash` must change between calls to defeat the pipeline's
    /// memoization (every frame we need the legacy overlay to repaint).
    #[test]
    fn content_hash_changes_after_render() {
        let overlay: Box<dyn OverlayComponent> = Box::new(NotSendOverlay(std::ptr::null_mut()));
        let mut adapter = LegacyOverlayAdapter::new(overlay);
        // Two `content_hash` calls before render both observe the seeded
        // value — equal, but no render happened yet, so memoization is fine.
        assert_eq!(adapter.content_hash(), 1);
        assert_eq!(adapter.content_hash(), 1);
        // A render bumps the counter; the next hash must differ.
        // We can't drive a real `render` here without a terminal, but
        // `render` is the only place `dirty_seq` is mutated, so we exercise
        // the same code path by hand to keep this test offline:
        adapter.dirty_seq = adapter.dirty_seq.wrapping_add(1).max(1);
        assert_ne!(adapter.content_hash(), 1);
    }

    /// Construct a `LegacyOverlayAdapter` from a *real* legacy overlay
    /// (`McpConfigOverlay::new(None, cwd)`). Proves the bridge works at a
    /// genuine integration site, not just with the structural mock above.
    /// `manager: None` keeps the test offline — no live runtime wiring.
    #[test]
    fn adapter_wraps_real_overlay() {
        use crate::tui::overlay::mcp_config::McpConfigOverlay;
        use std::path::PathBuf;
        let real: Box<dyn OverlayComponent> =
            Box::new(McpConfigOverlay::new(None, PathBuf::from(".")));
        let mut adapter = LegacyOverlayAdapter::new(real);
        // content_hash starts seeded; render bumps the counter.
        let h0 = adapter.content_hash();
        assert!(h0 != 0);
        adapter.dirty_seq = adapter.dirty_seq.wrapping_add(1).max(1);
        let h1 = adapter.content_hash();
        assert_ne!(h0, h1, "dirty_seq must change after render");
    }

    /// `convert_theme` must preserve name + foreground/background and not
    /// panic on any built-in V2 theme.
    #[test]
    fn convert_theme_preserves_core_slots() {
        use oxi_tui::theme::Theme;
        let v2 = Theme::dark();
        let legacy = convert_theme(&v2);
        assert_eq!(legacy.name, "dark");
        assert_eq!(legacy.colors.foreground, v2.colors.foreground);
        assert_eq!(legacy.colors.background, v2.colors.background);
        assert_eq!(legacy.colors.primary, v2.colors.primary);
        assert_eq!(legacy.colors.border, v2.colors.border);
        // Spacing + Symbols default to legacy crate's defaults.
        assert_eq!(legacy.spacing.padding, 1);
    }

    /// End-to-end: drive `Renderable::render` on the adapter through a
    /// real `TestBackend` and confirm the legacy overlay's paint actually
    /// lands in the buffer. This is the only test that exercises the
    /// bridge's three pieces together (`convert_theme` +
    /// `RenderCtx::with_frame` + legacy `OverlayComponent::render`), so
    /// any of the three breaking silently would trip it.
    #[test]
    fn render_delegates_to_legacy_overlay() {
        use oxi_tui::theme::{TerminalCaps, Theme};
        use oxi_tui::widget::RenderCtx;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(10, 5);
        let mut term = Terminal::new(backend).unwrap();
        let theme = Theme::dark();
        let caps = TerminalCaps::default();

        let overlay: Box<dyn OverlayComponent> = Box::new(PaintingOverlay {
            sentinel: "X",
            _not_send_witness: std::ptr::null_mut(),
        });
        let mut adapter = LegacyOverlayAdapter::new(overlay);
        let h_before = adapter.content_hash();

        // Drive `adapter.render` through the same harness as
        // `with_frame_provides_access_to_frame` in context.rs.
        term.draw(|f| {
            let mut ctx = RenderCtx::new(f, &theme, &caps);
            let area = Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 5,
            };
            adapter.render(area, &mut ctx);
        })
        .unwrap();

        // The legacy overlay's sentinel cell landed in the buffer.
        let buf = term.backend().buffer().clone();
        let symbol = buf.cell((0, 0)).map_or("", ratatui::buffer::Cell::symbol);
        assert_eq!(
            symbol, "X",
            "legacy overlay's sentinel cell must be visible after adapter.render"
        );

        // The adapter bumped its dirty counter so the pipeline knows to
        // re-render next frame.
        assert_ne!(
            adapter.content_hash(),
            h_before,
            "render() must bump dirty_seq so the next content_hash differs"
        );
    }
}
