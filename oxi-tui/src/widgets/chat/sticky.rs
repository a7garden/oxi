//! Sticky turn-prompt header overlay.
//!
//! Renders the last N (=3) user messages at the top of the chat viewport
//! as a persistent overlay — like iOS table section headers. Helps users
//! keep their place in long conversations.
//!
//! Sticky is purely visual; it overlays the natural layout at viewport y=0.
//! When the user scrolls up such that a sticky candidate message is
//! already visible at the top of the viewport, the sticky label is hidden
//! (no double-display).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::theme::ThemeStyles;
use crate::widgets::chat::layout::LayoutEntry;

/// How many recent user messages to show as sticky headers.
pub(crate) const STICKY_CANDIDATES: usize = 3;

/// Maximum height of a sticky header (in rows). Keep small — sticky is a
/// glanceable label, not a full message.
pub(crate) const STICKY_MAX_HEIGHT: u16 = 1;

/// Pick the last N user message indices (in layout order) to be sticky
/// candidates. `layout` is the full LayoutEntry vector from `compute_layout`.
///
/// A "user message" is identified by the first Text block of a user message
/// in the layout — its `LayoutKind::Text { is_user: true, .. }`.
pub(crate) fn compute_sticky_candidates(layout: &[LayoutEntry]) -> Vec<(usize, u32)> {
    let mut candidates = Vec::new();
    // Walk layout in order, find Text entries with is_user=true.
    // Group consecutive is_user entries (rules + spacer + text = one user msg),
    // then pick the LAST message of each user block.
    // For simplicity (per W1 design): pick the FIRST LayoutKind::Text{is_user=true}
    // entry per msg_idx.
    let mut last_msg_idx: Option<usize> = None;
    for entry in layout {
        if let crate::widgets::chat::layout::LayoutKind::Text { is_user: true, .. } = entry.kind
            && last_msg_idx != Some(entry.msg_idx)
        {
            candidates.push((entry.msg_idx, entry.y));
            last_msg_idx = Some(entry.msg_idx);
        }
    }
    // Keep last N.
    if candidates.len() > STICKY_CANDIDATES {
        let skip = candidates.len() - STICKY_CANDIDATES;
        candidates.drain(..skip);
    }
    candidates
}

/// Render sticky headers into the viewport.
///
/// `viewport_area` is the chat viewport (where ChatView drew the layout).
/// `scroll_offset` is the current `ChatViewState::scroll_offset` (virtual y).
/// `viewport_height` is the visible height of the viewport (in rows).
///
/// Sticky renders at `viewport_area.y` (top) only if the sticky candidate
/// is NOT already visible at the top of the viewport. Otherwise it's hidden
/// (avoids double-display of the same message).
pub(crate) fn render_sticky_headers(
    candidates: &[(usize, u32)],
    viewport_area: Rect,
    viewport_height: u16,
    scroll_offset: u32,
    styles: &ThemeStyles,
    buf: &mut Buffer,
) {
    let sticky_h = STICKY_MAX_HEIGHT.min(viewport_area.height);
    if sticky_h == 0 {
        return;
    }
    let sticky_rect = Rect::new(
        viewport_area.x,
        viewport_area.y,
        viewport_area.width,
        sticky_h,
    );

    // Collect sticky labels: just show "user msg N" for each candidate.
    // (Real text rendering would need the actual user message — that's
    // ChatMessage-level state. For W1 minimal viable, the label is index-based.)
    let mut lines: Vec<String> = Vec::new();
    for (msg_idx, entry_y) in candidates {
        // Hide if this candidate is currently at viewport top.
        let at_top = *entry_y >= scroll_offset
            && *entry_y < scroll_offset.saturating_add(viewport_height as u32);
        if at_top {
            continue;
        }
        lines.push(format!("▸ you (msg {})", msg_idx));
    }

    if lines.is_empty() {
        return;
    }

    // Render as a single-line overlay with muted styling.
    let display = if lines.len() == 1 {
        lines[0].clone()
    } else {
        format!("{} +{} more", lines[0], lines.len() - 1)
    };
    let truncated: String = display.chars().take(sticky_rect.width as usize).collect();
    let padded = format!("{:width$}", truncated, width = sticky_rect.width as usize);

    // Render at sticky_rect using styles.muted for the bg.
    let style = styles.muted;
    for x in sticky_rect.x..sticky_rect.x.saturating_add(sticky_rect.width) {
        if let Some(cell) = buf.cell_mut((x, sticky_rect.y)) {
            cell.set_symbol(" ");
            cell.set_style(style);
        }
    }
    // Overlay text on top of the bg.
    for (i, c) in padded.chars().enumerate() {
        let x = sticky_rect.x + i as u16;
        if x < sticky_rect.x.saturating_add(sticky_rect.width)
            && let Some(cell) = buf.cell_mut((x, sticky_rect.y))
        {
            cell.set_symbol(&c.to_string());
            cell.set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::chat::layout::{LayoutEntry, LayoutKind};
    use ratatui::text::Line;

    fn make_text_entry(y: u32, height: u32, msg_idx: usize, is_user: bool) -> LayoutEntry {
        LayoutEntry {
            y,
            height,
            kind: LayoutKind::Text {
                lines: vec![Line::from("")],
                is_user,
            },
            msg_idx,
        }
    }

    #[test]
    fn test_picks_last_n_user_messages() {
        let layout = vec![
            make_text_entry(0, 1, 0, true),
            make_text_entry(1, 1, 1, false),
            make_text_entry(2, 1, 2, true),
            make_text_entry(3, 1, 3, false),
            make_text_entry(4, 1, 4, true),
            make_text_entry(5, 1, 5, true),
        ];
        let candidates = compute_sticky_candidates(&layout);
        // Only msg 2, 4, 5 are user messages → all 3 should be candidates.
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].0, 2);
        assert_eq!(candidates[1].0, 4);
        assert_eq!(candidates[2].0, 5);
    }

    #[test]
    fn test_caps_at_sticky_n() {
        // 5 user messages, STICKY_CANDIDATES = 3, so only last 3.
        let mut layout = Vec::new();
        for i in 0..5 {
            layout.push(make_text_entry(i as u32, 1, i, true));
        }
        let candidates = compute_sticky_candidates(&layout);
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].0, 2); // oldest of the 3
        assert_eq!(candidates[1].0, 3);
        assert_eq!(candidates[2].0, 4); // newest
    }

    #[test]
    fn test_no_user_messages_yields_empty() {
        let layout = vec![
            make_text_entry(0, 1, 0, false),
            make_text_entry(1, 1, 1, false),
        ];
        let candidates = compute_sticky_candidates(&layout);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_empty_layout_yields_empty() {
        let candidates = compute_sticky_candidates(&[]);
        assert!(candidates.is_empty());
    }
}
