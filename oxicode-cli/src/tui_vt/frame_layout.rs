//! Production bridge from the grok-build-style agent view layout
//! (`oxicode_vtui::design::layout`) to the live ratatui render path.
//!
//! `compute_chrome` computes the [`AgentViewLayout`] for the current
//! frame. There is no chrome row at all anymore — the static shortcuts
//! bar was removed (peer parity with Claude Code / omp: static hints
//! are read once and then waste a row forever). Session facts live on
//! the composer's top border; abort/quit hints appear contextually in
//! the row above the composer while a stream runs.
use oxicode_vtui::design::layout::{
    AgentViewLayout, LayoutConfig, LayoutInput, ScrollbarConfig, effective_compact,
};
use ratatui::layout::Rect;

/// Prompt composer height (matches `main_loop::COMPOSER_HEIGHT`).
const COMPOSER_HEIGHT: u16 = 3;
/// No shortcuts bar — 0 height removes it from the layout entirely.
const SHORTCUTS_HEIGHT: u16 = 0;

/// The live chat surface is intentionally denser than the generic vtui
/// defaults: it should reserve space for conversation, not decorative frame
/// padding.  A single horizontal gutter keeps text off the terminal edge;
/// vertical gutters and their implied separator rows are unnecessary here.
const CHAT_LAYOUT: LayoutConfig = LayoutConfig {
    hpad_left: 1,
    hpad_right: 1,
    hpad_left_compact: 1,
    hpad_right_compact: 1,
    outer_vpad: 0,
    outer_vpad_compact: 0,
};

// ─────────────────────────────────────────────────────────────────────────
// Chrome layout
// ─────────────────────────────────────────────────────────────────────────

/// Compute the agent view layout. The caller places the transcript into
/// `layout.scrollback` and the composer into `layout.prompt`.
/// One blank row kept between the transcript and the composer. The gap is
/// LAYOUT, not content — a breath row inside the scrolling display gets
/// windowed away exactly when the transcript is full, gluing the latest
/// response to the composer. Reserved here, it survives every height and
/// every scrollback commit; the streaming indicator / quit hint render
/// into it while a run is live.
const BREATH_ROW: u16 = 1;

/// Compute the agent view layout. The caller places the transcript into
/// `layout.scrollback` and the composer into `layout.prompt`.
pub(super) fn compute_chrome(area: Rect) -> AgentViewLayout {
    let compact = effective_compact(false, area.height);
    let mut layout = AgentViewLayout::compute(
        area,
        &CHAT_LAYOUT,
        &ScrollbarConfig {
            enabled: false,
            ..Default::default()
        },
        LayoutInput {
            prompt_height: COMPOSER_HEIGHT,
            shortcuts_height: SHORTCUTS_HEIGHT,
            compact,
            ..LayoutInput::default()
        },
    );
    layout.scrollback.height = layout.scrollback.height.saturating_sub(BREATH_ROW);
    layout
}

/// Transcript (scrollback) area height for a terminal of this size,
/// without rendering. The scrollback-commit path needs the live
/// region's height to decide how many rows to shed into the host
/// terminal's real scrollback.
pub(super) fn scrollback_height(area: Rect) -> u16 {
    let compact = effective_compact(false, area.height);
    AgentViewLayout::compute(
        area,
        &CHAT_LAYOUT,
        &ScrollbarConfig {
            enabled: false,
            ..Default::default()
        },
        LayoutInput {
            prompt_height: COMPOSER_HEIGHT,
            shortcuts_height: SHORTCUTS_HEIGHT,
            compact,
            ..LayoutInput::default()
        },
    )
    .scrollback
    .height
    .saturating_sub(BREATH_ROW)
}

/// Transcript (scrollback) area x/width for a terminal of this size,
/// without rendering. The layout insets the terminal by one gutter
/// column on each side (`CHAT_LAYOUT.hpad_*`), so tool boxes and
/// scrollback commits must size themselves to THIS width — not the
/// terminal width — or their right edges wrap in the live viewport.
pub(super) fn scrollback_geometry(area: Rect) -> (u16, u16) {
    let compact = effective_compact(false, area.height);
    let layout = AgentViewLayout::compute(
        area,
        &CHAT_LAYOUT,
        &ScrollbarConfig {
            enabled: false,
            ..Default::default()
        },
        LayoutInput {
            prompt_height: COMPOSER_HEIGHT,
            shortcuts_height: SHORTCUTS_HEIGHT,
            compact,
            ..LayoutInput::default()
        },
    );
    (layout.scrollback.x, layout.scrollback.width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_has_no_top_status_row() {
        let layout = compute_chrome(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        // The scrollback IS the top of the frame — no chrome row above
        // it, and the removed shortcuts bar's row now belongs to the
        // transcript.
        assert_eq!(layout.scrollback.y, 0, "scrollback starts at row 0");
        assert_eq!(
            layout.scrollback.height,
            24 - COMPOSER_HEIGHT - SHORTCUTS_HEIGHT - BREATH_ROW,
            "scrollback owns every row the composer does not, minus the breath row"
        );
        assert_eq!(
            layout.scrollback.bottom() + 1,
            layout.prompt.y,
            "the breath row separates transcript from composer"
        );
        assert_eq!(SHORTCUTS_HEIGHT, 0, "the static shortcuts bar is gone");
    }

    #[test]
    fn chrome_respects_short_terminal_without_panic() {
        // A short terminal must still lay out (degrades gracefully).
        let layout = compute_chrome(Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 12,
        });
        assert!(layout.scrollback.height > 0, "transcript keeps space");
        assert!(layout.prompt.height > 0, "composer keeps space");
    }
}
