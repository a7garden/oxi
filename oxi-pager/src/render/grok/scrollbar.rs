//! Smooth scrollbar widget with follow-mode awareness.
//!
//! This module provides scrollbar rendering using `tui-scrollbar` for smooth
//! Unicode-based scrollbars with sub-character precision.
//!
//! # Visual Design
//!
//! The scrollbar visibility indicates follow mode state:
//! - **Following (at bottom):** Very dim scrollbar (subtle indicator of content above)
//! - **Not following:** Brighter scrollbar (draws attention to "scrolled up" state)
//!
//! This helps users understand when they're viewing live content vs. scrolled back.
//!
//! # Layout
//!
//! Callers should reserve space for the scrollbar:
//! - 1 column gap (visual separation from content)
//! - 1 column track (the scrollbar itself)
//!
//! Use [`split_area_for_scrollbar`] to compute content and scrollbar areas.
//!
//! # TODO: Mouse Support
//!
//! `tui-scrollbar` already provides mouse interaction support via:
//! - [`tui_scrollbar::ScrollBarInteraction`] for drag state
//! - [`tui_scrollbar::ScrollEvent`] / [`tui_scrollbar::PointerEvent`] for input
//! - [`tui_scrollbar::ScrollBar::handle_event`] for hit testing and drag math
//!
//! To wire this up:
//! 1. Store `ScrollBarInteraction` in pane state
//! 2. Translate crossterm `MouseEvent` to `tui_scrollbar::PointerEvent`
//! 3. Call `scrollbar.handle_event()` to get `ScrollCommand::SetOffset`
//! 4. Update scroll position accordingly

use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui_core::buffer::Buffer as CoreBuffer;
use ratatui_core::layout::Rect as CoreRect;
use ratatui_core::widgets::Widget as _;
use tui_scrollbar::ScrollBar;
use tui_scrollbar::ScrollLengths;
use tui_scrollbar::{SUBCELL, ScrollMetrics};

/// When set, every scrollbar renders as a no-op. The pager toggles this on in
/// minimal (scrollback-native) mode, where lists/dropdowns show
/// no scrollbar bar at all — they scroll internally and the footer carries the
/// "↑/↓ navigate" hint. Off (default) everywhere else, so the full TUI is
/// unaffected.
static SCROLLBARS_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Globally hide or show all scrollbars. See [`SCROLLBARS_HIDDEN`].
pub fn set_scrollbars_hidden(hidden: bool) {
    SCROLLBARS_HIDDEN.store(hidden, Ordering::Relaxed);
}

/// Whether scrollbars are currently globally hidden.
pub fn scrollbars_hidden() -> bool {
    SCROLLBARS_HIDDEN.load(Ordering::Relaxed)
}

/// Number of columns reserved between content and the scrollbar track.
/// This creates the "X" gap in the XSXBXX pattern (gap between selection_right and scrollbar).
const SCROLLBAR_GAP_COLS: u16 = 1;

/// Width of the scrollbar track itself (in terminal cells).
const SCROLLBAR_TRACK_COLS: u16 = 1;

/// Total columns reserved for scrollbar UI (gap + track).
pub const SCROLLBAR_TOTAL_COLS: u16 = SCROLLBAR_GAP_COLS + SCROLLBAR_TRACK_COLS;

/// Split an area into content + scrollbar regions.
///
/// Layout:
/// - `content_area`: original area minus [`SCROLLBAR_TOTAL_COLS`] on the right
/// - `scrollbar_area`: the last column of the original area (1 cell wide)
/// - The column between them is the "gap" (left intentionally blank)
///
/// Returns `(content_area, None)` when the terminal is too narrow.
///
/// **Note**: This always reserves space for scrollbar. Use [`maybe_split_for_scrollbar`]
/// to only reserve space when the scrollbar will actually be shown.
pub fn split_area_for_scrollbar(area: Rect) -> (Rect, Option<Rect>) {
    if area.width <= SCROLLBAR_TOTAL_COLS {
        return (area, None);
    }

    let content_width = area.width.saturating_sub(SCROLLBAR_TOTAL_COLS);
    let content_area = Rect {
        x: area.x,
        y: area.y,
        width: content_width,
        height: area.height,
    };
    let scrollbar_area = Rect {
        x: area.right().saturating_sub(1),
        y: area.y,
        width: SCROLLBAR_TRACK_COLS,
        height: area.height,
    };

    (content_area, Some(scrollbar_area))
}

/// Split an area only if scrollbar is actually needed.
///
/// Unlike [`split_area_for_scrollbar`], this gives full width to content
/// when scrollbar won't be shown (`total_lines <= viewport_lines`).
///
/// Use this when you know the content height before splitting.
pub fn maybe_split_for_scrollbar(area: Rect, total_lines: u16) -> (Rect, Option<Rect>) {
    // Only reserve space if scrollbar will actually be shown
    if needs_scrollbar(total_lines, area.height) {
        split_area_for_scrollbar(area)
    } else {
        // No scrollbar needed - give full width to content
        (area, None)
    }
}

/// Whether the scrollbar should be shown (content overflows viewport).
pub fn needs_scrollbar(total_lines: u16, viewport_lines: u16) -> bool {
    total_lines > viewport_lines
}

/// Whether the view is at the bottom (following mode position).
#[allow(dead_code)] // Useful helper, kept for future use
pub fn is_at_bottom(total_lines: u16, viewport_lines: u16, offset: u16) -> bool {
    let max_offset = total_lines.saturating_sub(viewport_lines);
    offset >= max_offset
}

/// Result of mapping a scrollbar click/drag position to a scroll offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarClickResult {
    /// Jump to the very top (click on first row of track).
    Top,
    /// Jump to the very bottom (click on last row of track).
    Bottom,
    /// Set scroll offset to this value (proportional position).
    Offset(usize),
}

/// Map a click on the scrollbar gutter to a scroll offset.
///
/// Uses the same `tui_scrollbar::ScrollMetrics` that the renderer uses to
/// position the thumb, so the click is the exact inverse of the rendering.
/// Emulates `JumpToClick` behavior: centers the thumb on the click position.
///
/// # Arguments
///
/// * `cell_index` — 0-based row within the scrollbar area (screen_y - sb.y)
/// * `track_cells` — height of the scrollbar area (sb.height)
/// * `total_lines` — total content height (pre-scaled)
/// * `viewport_lines` — viewport height
///
/// Returns `Top`/`Bottom` for clicks on the first/last row, otherwise
/// an offset that places the thumb centered on the click.
pub fn scrollbar_click_to_offset(
    cell_index: u16,
    track_cells: u16,
    total_lines: u16,
    viewport_lines: u16,
) -> ScrollbarClickResult {
    if track_cells == 0 {
        return ScrollbarClickResult::Top;
    }

    // First row → go to top.
    if cell_index == 0 {
        return ScrollbarClickResult::Top;
    }
    // Last row → go to bottom.
    if cell_index >= track_cells.saturating_sub(1) {
        return ScrollbarClickResult::Bottom;
    }

    let lengths = ScrollLengths {
        content_len: total_lines as usize,
        viewport_len: viewport_lines as usize,
    };
    let metrics = ScrollMetrics::new(lengths, 0, track_cells);

    // Center the thumb on the clicked cell (same as tui_scrollbar JumpToClick).
    let position = (cell_index as usize)
        .saturating_mul(SUBCELL)
        .saturating_add(SUBCELL / 2);
    let half_thumb = metrics.thumb_len() / 2;
    let thumb_start = position.saturating_sub(half_thumb);
    let offset = metrics.offset_for_thumb_start(thumb_start);

    ScrollbarClickResult::Offset(offset)
}

/// Render a scrollbar with follow-mode aware styling.
///
/// # Arguments
///
/// * `buf` - The ratatui buffer to render into
/// * `scrollbar_area` - The 1-column area for the scrollbar track
/// * `total_lines` - Total content height in lines
/// * `viewport_lines` - Visible viewport height in lines
/// * `offset` - Current scroll offset (lines from top)
/// * `is_following` - Whether follow mode is active (dims the scrollbar)
///
/// The scrollbar is always rendered when content overflows, but styled differently
/// based on follow state:
/// - Following: very dim (subtle indicator)
/// - Not following: brighter (draws attention)
pub fn render_scrollbar(
    buf: &mut Buffer,
    scrollbar_area: Option<Rect>,
    total_lines: u16,
    viewport_lines: u16,
    offset: u16,
    is_following: bool,
) {
    if SCROLLBARS_HIDDEN.load(Ordering::Relaxed) {
        return;
    }

    let Some(scrollbar_area) = scrollbar_area else {
        return;
    };

    if scrollbar_area.width == 0 || scrollbar_area.height == 0 {
        return;
    }

    if !needs_scrollbar(total_lines, viewport_lines) {
        return;
    }

    let lengths = ScrollLengths {
        content_len: total_lines as usize,
        viewport_len: viewport_lines as usize,
    };

    let scrollbar = ScrollBar::vertical(lengths).offset(offset as usize);

    // Render into ratatui-core scratch buffer
    let core_area = CoreRect {
        x: scrollbar_area.x,
        y: scrollbar_area.y,
        width: scrollbar_area.width,
        height: scrollbar_area.height,
    };
    let mut scratch = CoreBuffer::empty(core_area);
    (&scrollbar).render(core_area, &mut scratch);

    // Copy to ratatui buffer with follow-aware styling
    let (track_style, thumb_style) = scrollbar_styles(is_following);
    for row in 0..scrollbar_area.height {
        let x = scrollbar_area.x;
        let y = scrollbar_area.y + row;
        let src = &scratch[(x, y)];
        let dst = &mut buf[(x, y)];
        if src.symbol() == " " {
            dst.set_symbol(" ");
            dst.set_style(track_style);
        } else {
            dst.set_symbol("\u{2588}");
            dst.set_style(thumb_style);
        }
    }
}

/// Get track and thumb styles based on follow mode.
///
/// Following mode: very dim colors (scrollbar recedes into background)
/// Not following: brighter colors (scrollbar "pops out")
fn scrollbar_styles(is_following: bool) -> (Style, Style) {
    let theme = crate::render::theme::Theme::current();
    if is_following {
        // Very dim - scrollbar is subtle when following
        let track_style = Style::new().bg(theme.scrollbar_bg);
        let thumb_style = Style::new().fg(theme.scrollbar_fg).bg(theme.scrollbar_bg);
        (track_style, thumb_style)
    } else {
        // Brighter - scrollbar stands out when scrolled up
        let track_style = Style::new().bg(theme.bg_highlight);
        let thumb_style = Style::new().fg(theme.gray).bg(theme.bg_highlight);
        (track_style, thumb_style)
    }
}

/// Render a scrollbar with custom track and thumb styles.
///
/// Like [`render_scrollbar`] but allows custom styling for theme integration.
pub fn render_scrollbar_styled(
    buf: &mut Buffer,
    scrollbar_area: Option<Rect>,
    total_lines: u16,
    viewport_lines: u16,
    offset: u16,
    track_style: Style,
    thumb_style: Style,
) {
    if SCROLLBARS_HIDDEN.load(Ordering::Relaxed) {
        return;
    }

    let Some(scrollbar_area) = scrollbar_area else {
        return;
    };

    if scrollbar_area.width == 0 || scrollbar_area.height == 0 {
        return;
    }

    if !needs_scrollbar(total_lines, viewport_lines) {
        return;
    }

    let lengths = ScrollLengths {
        content_len: total_lines as usize,
        viewport_len: viewport_lines as usize,
    };

    let scrollbar = ScrollBar::vertical(lengths).offset(offset as usize);

    // Render into ratatui-core scratch buffer
    let core_area = CoreRect {
        x: scrollbar_area.x,
        y: scrollbar_area.y,
        width: scrollbar_area.width,
        height: scrollbar_area.height,
    };
    let mut scratch = CoreBuffer::empty(core_area);
    (&scrollbar).render(core_area, &mut scratch);

    // Copy to ratatui buffer with custom styling
    for row in 0..scrollbar_area.height {
        let x = scrollbar_area.x;
        let y = scrollbar_area.y + row;
        let src = &scratch[(x, y)];
        let dst = &mut buf[(x, y)];
        if src.symbol() == " " {
            dst.set_symbol(" ");
            dst.set_style(track_style);
        } else {
            dst.set_symbol("\u{2588}");
            dst.set_style(thumb_style);
        }
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
