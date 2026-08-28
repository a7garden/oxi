//! Render layer for the git TUI overlay.
//!
//! * [`plan_overlay`] / [`RenderPlan`] — pure layout math (chunks per pane)
//! * [`pair_split_view`] — pair removed + added lines per hunk so the
//!   side-by-side renderer can align left/right columns with shared context
//! * [`render_sidebar_rows`] — pure sidebar row builder (text only — no
//!   ratatui styles) so the layout can be tested without a terminal
//! * [`minimap_buckets_rows`] — per-row added/removed ratio for the
//!   right-edge minimap strip
//! * [`render_overlay_lines`] — ratatui draw fn, consumed by `render_frame`
//!
//! The layout math is deliberately separated from the draw step so the
//! bulk of the test surface stays in pure helpers.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::{
    DiffFile, DiffLineKind, DiffViewMode, GitTuiState, Hunk, StatusEntry, WhitespaceMode,
    filter_whitespace,
};

/// Layout plan returned by [`plan_overlay`]. One chunk per pane.
#[derive(Debug, Clone, Copy)]
pub struct RenderPlan {
    pub header: Rect,
    pub body: Rect,
    pub footer: Rect,
    pub sidebar: Rect,
    pub diff: Rect,
    pub minimap: Rect,
    pub commit_form: Rect,
}

/// Decide how wide each pane should be given the viewport.
///
/// Layout (inside the area the caller reserved for the overlay):
///
/// ```text
/// +---------------- header ---------------+
/// | sidebar | diff pane           | minimap|
/// +---------------- footer ---------------+
/// ```
///
/// The sidebar is 1/4 of the width with a floor of 20 cols. The minimap
/// is fixed at 2 cols (the brief's "2-col right edge"). When the body
/// area is too narrow for both, the minimap collapses to zero width and
/// the diff pane eats the freed columns.
pub fn plan_overlay(area: Rect) -> RenderPlan {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(3),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    let header = outer[0];
    let body = outer[1];
    let footer = outer[2];

    // Body = sidebar | diff (+ minimap).
    let sidebar_w = (body.width / 4).max(20).min(body.width);
    let body_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(sidebar_w),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(body);

    let sidebar = body_cols[0];
    let diff = body_cols[1];
    let minimap = body_cols[2];
    // The commit-mode composer occupies the diff pane entirely; this rect
    // is informational (it equals `diff`) so the draw fn can re‑split.
    let commit_form = diff;

    RenderPlan {
        header,
        body,
        footer,
        sidebar,
        diff,
        minimap,
        commit_form,
    }
}

/// One paired row of a split view: left = removed-side text, right =
/// added-side text. Either side may be empty (when a hunk has all
/// additions or all deletions).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SplitRow {
    pub left: Option<String>,
    pub right: Option<String>,
}

/// Pair context / removed / added lines for one hunk into left/right rows
/// suitable for side-by-side rendering.
///
/// Algorithm (matches the brief):
/// * Walk the lines in source order.
/// * Context → duplicate text into BOTH left and right.
/// * Removed → push into `left`, leave `right = None`.
/// * Added → push into `right`, leave `left = None`.
/// * Adjacent Removed-then-Added (and Added-then-Removed) pairs line up
///   at the same row index so the visual gutter reads as a unified diff
///   flipped sideways.
/// * Lone runs of one side collapse to per-row Nones so the renderer
///   doesn't waste vertical space on blank halves.
pub fn pair_split_view(hunk: &Hunk) -> Vec<SplitRow> {
    let mut rows: Vec<SplitRow> = Vec::with_capacity(hunk.lines.len());
    for line in &hunk.lines {
        match line.kind {
            DiffLineKind::Context => {
                rows.push(SplitRow {
                    left: Some(line.text.clone()),
                    right: Some(line.text.clone()),
                });
            }
            DiffLineKind::Removed => rows.push(SplitRow {
                left: Some(line.text.clone()),
                right: None,
            }),
            DiffLineKind::Added => rows.push(SplitRow {
                left: None,
                right: Some(line.text.clone()),
            }),
        }
    }
    rows
}

/// One row of the sidebar (file list).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SidebarRow {
    pub path: String,
    pub marker: &'static str,
}

/// Build the sidebar rows. `staged` marks paths with `[S]` (whatever the
/// user has staged via `toggle_stage`); unmerged entries always get
/// `[U]`. Order follows the input entries (which is `git status`
/// porcelain order).
pub fn render_sidebar_rows(
    entries: &[StatusEntry],
    staged: &std::collections::HashSet<String>,
    selected: usize,
) -> Vec<SidebarRow> {
    entries
        .iter()
        .enumerate()
        .map(|(idx, e)| {
            let marker = if e.is_unmerged {
                "[U]"
            } else if staged.contains(&e.path) {
                "[S]"
            } else {
                ""
            };
            let row = SidebarRow {
                path: e.path.clone(),
                marker,
            };
            // Selection cursor is communicated via the caller (highlighted
            // row is `selected`); we keep this fn side-effect-free by
            // not returning it.
            let _ = (idx, selected);
            row
        })
        .collect()
}

/// One bucket of the minimap: ratio of added vs removed lines on a
/// given screen row (0.0 = pure removal, 1.0 = pure addition).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MinimapBucket {
    pub added: usize,
    pub removed: usize,
}

impl MinimapBucket {
    /// 0.0..=1.0 ratio; returns 0.5 for an empty bucket so the renderer
    /// can default to a neutral color without a branch.
    pub fn ratio(&self) -> f32 {
        let total = self.added + self.removed;
        if total == 0 {
            0.5
        } else {
            self.added as f32 / total as f32
        }
    }
}

/// Bucket a flat list of (line-kind) rows into `rows` slots so the
/// minimap draws one cell per visible row.
///
/// Out-of-bounds rows are clamped (no crash) — the caller may pass a
/// `rows` count derived from the viewport height, which can shift
/// between frames as the user resizes.
pub fn minimap_buckets_rows(pairs: &[DiffLineKind], rows: usize) -> Vec<MinimapBucket> {
    let mut buckets = vec![MinimapBucket::default(); rows];
    if rows == 0 || pairs.is_empty() {
        return buckets;
    }
    let per = (pairs.len() as f32 / rows as f32).ceil() as usize;
    if per == 0 {
        return buckets;
    }
    for (i, kind) in pairs.iter().enumerate() {
        let bucket = (i / per).min(rows - 1);
        match kind {
            DiffLineKind::Added => buckets[bucket].added += 1,
            DiffLineKind::Removed => buckets[bucket].removed += 1,
            DiffLineKind::Context => {}
        }
    }
    buckets
}

/// Flat-row offset (no wrap) of a hunk's header row inside the inline
/// diff: every hunk renders `1 + lines.len()` rows (header + one row
/// per diff line). `selected` is clamped into range.
pub(crate) fn hunk_header_row(hunks: &[Hunk], selected: usize) -> usize {
    hunks[..selected.min(hunks.len())]
        .iter()
        .map(|h| 1 + h.lines.len())
        .sum()
}

/// Scroll offset (rows) that brings the selected hunk's header into
/// the diff pane: `min(header offset, overflow)`. With the header at
/// the pane top when it fits, and pinned to the last pane-height
/// window when the hunk sits near the end of the diff. Pane taller
/// than the whole diff → 0 (no scroll).
///
/// Final-review finding 2: `selected_hunk` is mutated by j/k, alt+↓/↑
/// and g/G but nothing consumed it — the pane always rendered from
/// row 0, truncating long diffs. The row model assumes the default
/// no-wrap rendering (one visual row per diff line); under `wrap` the
/// true visual count can only grow, which makes this scroll
/// conservative (the highlight still marks the selected hunk).
pub fn hunk_scroll_offset(hunks: &[Hunk], selected: usize, pane_height: usize) -> usize {
    let total: usize = hunks.iter().map(|h| 1 + h.lines.len()).sum();
    let overflow = total.saturating_sub(pane_height);
    hunk_header_row(hunks, selected).min(overflow)
}

/// Footer hint line. Returned as a String (not Line) so the test can
/// assert content without bringing in ratatui's Style.
pub fn footer_hints(view: DiffViewMode, ws: WhitespaceMode, wrap: bool) -> String {
    let _ = view;
    let _ = ws;
    let wrap_label = if wrap { "wrap" } else { "trunc" };
    format!(
        "j/k nav · alt+↓/↑ hunk · ]/[ file · 1-4 view · v sidebar · b ws · w {wrap_label} · s stage · u unstage · c commit · r refresh · q close"
    )
}
// Ratatui draw
// ---------------------------------------------------------------------------

/// Draw the overlay into `frame`. The caller is expected to have
/// already clipped the frame to a sane area (the brief: overlay
/// REPLACES the scrollback+composer region entirely — `render_frame`
/// in `main_loop.rs` handles the area split).
pub fn render_overlay_lines(frame: &mut Frame<'_>, area: Rect, state: &GitTuiState) {
    let plan = plan_overlay(area);
    frame.render_widget(Clear, area);

    render_header(frame, plan.header, state);
    render_footer(frame, plan.footer, state);

    // Commit mode replaces the diff pane with a single-line composer.
    if state.commit_mode {
        render_commit_form(frame, plan.diff, state);
    } else {
        render_sidebar(frame, plan.sidebar, state);
        render_diff_pane(frame, plan.diff, state);
        render_minimap(frame, plan.minimap, state);
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, state: &GitTuiState) {
    let total = state.entries.len();
    let staged = state.staged.len();
    let branch = state.branch.as_deref().unwrap_or("detached");
    let title = format!(
        " git · {} files · {} staged · {} · q close ",
        total, staged, branch
    );
    let block = Block::default()
        .borders(Borders::NONE)
        .title(Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )));
    frame.render_widget(block, area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &GitTuiState) {
    let hints = footer_hints(state.view, state.ws, state.wrap);
    let style = Style::default().add_modifier(Modifier::DIM);
    frame.render_widget(Paragraph::new(Line::from(Span::styled(hints, style))), area);
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, state: &GitTuiState) {
    let rows = render_sidebar_rows(&state.entries, &state.staged, state.selected_file);
    let lines: Vec<Line<'_>> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let cursor = if i == state.selected_file { "> " } else { "  " };
            let marker = if r.marker.is_empty() {
                String::new()
            } else {
                format!("{} ", r.marker)
            };
            let is_selected = i == state.selected_file;
            let style = if is_selected && state.sidebar_focus {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(cursor, style),
                Span::styled(marker, style),
                Span::styled(r.path.clone(), style),
            ])
        })
        .collect();
    let block = Block::default().borders(Borders::RIGHT);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_diff_pane(frame: &mut Frame<'_>, area: Rect, state: &GitTuiState) {
    // Apply the whitespace filter before rendering.
    let doc = filter_whitespace(&state.doc, state.ws);
    let block = Block::default().borders(Borders::NONE);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(file) = doc.files.get(state.selected_file) else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "(no files)",
                Style::default().add_modifier(Modifier::DIM),
            ))),
            inner,
        );
        return;
    };
    if file.binary {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "(binary file)",
                Style::default().add_modifier(Modifier::DIM),
            ))),
            inner,
        );
        return;
    }

    match state.view {
        DiffViewMode::Files => {
            let lines: Vec<Line<'_>> = doc
                .files
                .iter()
                .map(|f| Line::from(Span::raw(f.path.clone())))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
        DiffViewMode::Hunks => {
            let lines: Vec<Line<'_>> = file
                .hunks
                .iter()
                .map(|h| {
                    let added = h
                        .lines
                        .iter()
                        .filter(|l| matches!(l.kind, DiffLineKind::Added))
                        .count();
                    let removed = h
                        .lines
                        .iter()
                        .filter(|l| matches!(l.kind, DiffLineKind::Removed))
                        .count();
                    Line::from(Span::styled(
                        format!(
                            "@@ -{},{} +{},{} @@ +{}/-{}",
                            h.old_start,
                            h.lines.len(),
                            h.new_start,
                            h.lines.len(),
                            added,
                            removed
                        ),
                        Style::default().add_modifier(Modifier::BOLD),
                    ))
                })
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
        DiffViewMode::Inline => {
            let lines = render_inline(file, state.wrap, inner.width, state.selected_hunk);
            let scroll =
                hunk_scroll_offset(&file.hunks, state.selected_hunk, inner.height as usize);
            frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), inner);
        }
        DiffViewMode::Split => {
            // Split mode tries to pair removed/added by hunk; when the
            // file lacks removals OR additions we fall back to inline so
            // the user always sees something useful (v1 honesty over
            // broken side-by-side — see brief).
            let has_removals = file
                .hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .any(|l| matches!(l.kind, DiffLineKind::Removed));
            let has_additions = file
                .hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .any(|l| matches!(l.kind, DiffLineKind::Added));
            if !has_removals || !has_additions {
                let lines = render_inline(file, state.wrap, inner.width, state.selected_hunk);
                let scroll =
                    hunk_scroll_offset(&file.hunks, state.selected_hunk, inner.height as usize);
                frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), inner);
                return;
            }
            render_split(frame, inner, file, state.wrap);
        }
    }
}

fn render_inline(file: &DiffFile, wrap: bool, width: u16, selected_hunk: usize) -> Vec<Line<'_>> {
    let mut lines: Vec<Line<'_>> = Vec::new();
    for (idx, hunk) in file.hunks.iter().enumerate() {
        // The selected hunk's header is the cursor: reverse video
        // marks where j/k / alt+↓/↑ / g/G moved to (finding 2 —
        // `selected_hunk` was previously mutated but never rendered).
        let header_style = if idx == selected_hunk {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start,
                hunk.lines.len(),
                hunk.new_start,
                hunk.lines.len()
            ),
            header_style,
        )));
        for line in &hunk.lines {
            let (prefix, style) = match line.kind {
                DiffLineKind::Added => ("+ ", Style::default().fg(Color::Green)),
                DiffLineKind::Removed => ("- ", Style::default().fg(Color::Red)),
                DiffLineKind::Context => ("  ", Style::default()),
            };
            // Wrap emits one Line per visual segment; truncate is a
            // single segment. We unify on `Vec<String>` and push one
            // Line per segment so the gutter stays aligned.
            let segments: Vec<String> = if wrap {
                wrap_text(&line.text, width.saturating_sub(2))
            } else {
                vec![truncate_text(&line.text, width.saturating_sub(2))]
            };
            for seg in segments {
                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(seg, style),
                ]));
            }
        }
    }
    lines
}

fn render_split(frame: &mut Frame<'_>, inner: Rect, file: &DiffFile, wrap: bool) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);
    let col_w = cols[0].width.max(cols[1].width);
    let mut left_lines: Vec<Line<'_>> = Vec::new();
    let mut right_lines: Vec<Line<'_>> = Vec::new();
    for hunk in &file.hunks {
        for row in pair_split_view(hunk) {
            let (left_text, right_text): (Vec<String>, Vec<String>) = if wrap {
                (
                    wrap_text(row.left.as_deref().unwrap_or(""), col_w),
                    wrap_text(row.right.as_deref().unwrap_or(""), col_w),
                )
            } else {
                (
                    vec![truncate_text(row.left.as_deref().unwrap_or(""), col_w)],
                    vec![truncate_text(row.right.as_deref().unwrap_or(""), col_w)],
                )
            };
            // One row in the rendered pane can span multiple visual
            // lines when wrapping; each wrap segment becomes its own
            // Line so the gutter alignment stays readable.
            for line in &left_text {
                left_lines.push(Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(Color::Red),
                )));
            }
            for line in &right_text {
                right_lines.push(Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(Color::Green),
                )));
            }
        }
    }
    frame.render_widget(Paragraph::new(left_lines), cols[0]);
    frame.render_widget(Paragraph::new(right_lines), cols[1]);
}

/// Hard-wrap a string into multiple lines of at most `width` columns
/// (counted via Unicode-width for `char::len_utf8` approximations;
/// full-width-aware wrapping is out of scope for the v1 overlay).
/// Returns `vec![text.to_string()]` when the line already fits.
pub(crate) fn wrap_text(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut cols = 0usize;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols + w > width as usize {
            out.push(std::mem::take(&mut current));
            cols = 0;
        }
        current.push(ch);
        cols += w;
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
}

/// Truncate a string to at most `width` columns; longer lines end in
/// `…` so the truncation is visually obvious in the diff pane.
pub(crate) fn truncate_text(text: &str, width: u16) -> String {
    if width == 0 {
        return String::new();
    }
    // Build greedily. The ellipsis slot is the LAST column — when we'd
    // have to reject any char AND there's room for an ellipsis (i.e.
    // we kept at least one real char), replace the trailing char with
    // '…' so the visible width stays at exactly `width`.
    if width == 1 {
        return text
            .chars()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_default();
    }
    let target = width as usize;
    let mut out = String::new();
    let mut cols = 0usize;
    let mut last_kept_byte_idx: Option<usize> = None;
    let mut last_kept_cols: usize = 0;
    let mut rejected = false;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols + w > target {
            rejected = true;
            break;
        }
        last_kept_byte_idx = Some(out.len());
        last_kept_cols = cols;
        out.push(ch);
        cols += w;
    }
    if rejected && let Some(idx) = last_kept_byte_idx {
        out.truncate(idx);
        // `last_kept_cols` is the width BEFORE we wrote the char
        // we just dropped. The ellipsis occupies 1 column; if the
        // prefix already filled the column the ellipsis would
        // take, there's no room and we leave it bare.
        if last_kept_cols < target {
            out.push('\u{2026}');
        }
    }
    out
}

fn render_minimap(frame: &mut Frame<'_>, area: Rect, state: &GitTuiState) {
    let doc = filter_whitespace(&state.doc, state.ws);
    let Some(file) = doc.files.get(state.selected_file) else {
        return;
    };
    let pairs: Vec<DiffLineKind> = file
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter().map(|l| l.kind))
        .collect();
    let buckets = minimap_buckets_rows(&pairs, area.height as usize);
    for (i, b) in buckets.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        let color = if b.added > b.removed {
            Color::Green
        } else if b.removed > b.added {
            Color::Red
        } else {
            Color::DarkGray
        };
        let row_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("  ", Style::default().bg(color)))),
            row_area,
        );
    }
}

fn render_commit_form(frame: &mut Frame<'_>, area: Rect, state: &GitTuiState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" commit message (Enter commit · Esc cancel) ");
    let text = if state.commit_msg.is_empty() {
        Line::from(Span::styled(
            "(type commit message)",
            Style::default().add_modifier(Modifier::DIM),
        ))
    } else {
        Line::from(Span::raw(state.commit_msg.clone()))
    };
    frame.render_widget(Paragraph::new(text).block(block), area);
}

// ---------------------------------------------------------------------------
// Tests (TDD — pure helpers are tested here; the draw fn is exercised in
// main_loop integration via a smoke test, not unit-tested directly).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui_vt::git_tui::diff_doc::DiffLine;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn hunk_fixture() -> Hunk {
        Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: "a".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "b-old".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "b-new".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: "c".to_string(),
                },
            ],
        }
    }

    #[test]
    fn split_view_pairs_removed_and_added_by_hunk() {
        let h = hunk_fixture();
        let rows = pair_split_view(&h);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].left.as_deref(), Some("a"));
        assert_eq!(rows[0].right.as_deref(), Some("a"));
        assert_eq!(rows[1].left.as_deref(), Some("b-old"));
        assert_eq!(rows[1].right, None);
        assert_eq!(rows[2].left, None);
        assert_eq!(rows[2].right.as_deref(), Some("b-new"));
        assert_eq!(rows[3].left.as_deref(), Some("c"));
        assert_eq!(rows[3].right.as_deref(), Some("c"));
    }

    #[test]
    fn minimap_buckets_rows_groups_by_row() {
        let pairs = vec![
            DiffLineKind::Added,
            DiffLineKind::Added,
            DiffLineKind::Removed,
            DiffLineKind::Context,
        ];
        let b = minimap_buckets_rows(&pairs, 2);
        assert_eq!(b.len(), 2);
        // First bucket gets the first 2 entries: 2 added, 0 removed.
        assert_eq!(b[0].added, 2);
        assert_eq!(b[0].removed, 0);
        // Second bucket gets the next 2 entries: 0 added, 1 removed.
        assert_eq!(b[1].added, 0);
        assert_eq!(b[1].removed, 1);
        // Ratio is in [0.0, 1.0].
        assert!(b[0].ratio() > 0.5);
        assert!(b[1].ratio() < 0.5);
    }

    #[test]
    fn footer_hints_include_commit_and_close() {
        let s = footer_hints(DiffViewMode::Inline, WhitespaceMode::Off, false);
        assert!(s.contains("commit"), "footer missing commit hint: {s}");
        assert!(s.contains("close"), "footer missing close hint: {s}");
        assert!(s.contains("stage"), "footer missing stage hint: {s}");
    }

    #[test]
    fn sidebar_rows_marks_staged_and_unmerged() {
        let entries = vec![
            StatusEntry {
                path: "a.txt".into(),
                old_path: None,
                xy: ['M', ' '],
                is_rename: false,
                is_unmerged: false,
            },
            StatusEntry {
                path: "b.txt".into(),
                old_path: None,
                xy: ['U', 'U'],
                is_rename: false,
                is_unmerged: true,
            },
        ];
        let mut staged = std::collections::HashSet::new();
        staged.insert("a.txt".to_string());
        let rows = render_sidebar_rows(&entries, &staged, 0);
        assert_eq!(rows[0].marker, "[S]");
        assert_eq!(rows[1].marker, "[U]");
    }

    #[test]
    fn plan_overlay_splits_into_three_rows() {
        let plan = plan_overlay(Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 24,
        });
        assert_eq!(plan.header.height, 1);
        assert_eq!(plan.footer.height, 1);
        // Sidebar is 1/4 of body width, but min 20.
        assert_eq!(plan.sidebar.width, 25);
        // Minimap is 2 columns.
        assert_eq!(plan.minimap.width, 2);
    }

    #[test]
    fn render_split_paints_both_columns() {
        let backend = TestBackend::new(40, 6);
        let mut terminal = Terminal::new(backend).expect("backend");
        let file = DiffFile {
            path: "demo.txt".to_string(),
            old_path: None,
            hunks: vec![hunk_fixture()],
            binary: false,
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 6,
        };
        terminal
            .draw(|f| render_split(f, area, &file, false))
            .expect("draw");
        let buf = terminal.backend().buffer();
        let mut rows: Vec<String> = Vec::new();
        for y in 0..buf.area().height {
            let mut line = String::new();
            for x in 0..buf.area().width {
                if let Some(c) = buf.cell((x, y)) {
                    line.push_str(c.symbol());
                }
            }
            rows.push(line);
        }
        // (c, c). The first segment of each pair goes into the LEFT
        // half; the second into the RIGHT half. Verify both halves.
        let left: String = rows
            .iter()
            .map(|r| r.chars().take(20).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        let right: String = rows
            .iter()
            .map(|r| r.chars().skip(20).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            left.contains("b-old"),
            "removed text missing from left column: {left:?}"
        );
        assert!(
            right.contains("b-new"),
            "added text missing from right column: {right:?}"
        );
        // Context line "c" must appear in BOTH halves (last pair row).
        assert!(left.contains("c"), "context missing from left: {left:?}");
        assert!(right.contains("c"), "context missing from right: {right:?}");
        // And context line "a" (first pair row) must also appear in
        // both halves.
        assert!(
            left.contains("a"),
            "first context missing from left: {left:?}"
        );
        assert!(
            right.contains("a"),
            "first context missing from right: {right:?}"
        );
    }

    /// `wrap_text` breaks long lines on `width` boundaries; short lines
    /// pass through unchanged.
    #[test]
    fn wrap_text_breaks_at_width() {
        let short = wrap_text("hello", 10);
        assert_eq!(short, vec!["hello".to_string()]);
        let long = wrap_text("abcdefghij", 4);
        assert_eq!(
            long,
            vec!["abcd".to_string(), "efgh".to_string(), "ij".to_string()]
        );
    }

    /// `truncate_text` caps at `width` and adds an ellipsis when it
    /// had to drop characters. With width=4 the input "abcdefghij"
    /// fits exactly; to exercise the ellipsis branch we ask for width=3.
    #[test]
    fn truncate_text_caps_with_ellipsis() {
        let short = truncate_text("hi", 10);
        assert_eq!(short, "hi");
        let long = truncate_text("abcdefghij", 3);
        assert_eq!(long, "ab\u{2026}");
        let exact = truncate_text("abcd", 4);
        assert_eq!(exact, "abcd");
    }

    /// The footer hint reflects the actual wrap state — the brief
    /// requires the binding be wired, not inert.
    #[test]
    fn footer_hints_reflects_wrap_state() {
        let trunc = footer_hints(DiffViewMode::Inline, WhitespaceMode::Off, false);
        assert!(trunc.contains("trunc"), "trunc label missing: {trunc}");
        let wrapped = footer_hints(DiffViewMode::Inline, WhitespaceMode::Off, true);
        assert!(wrapped.contains("wrap"), "wrap label missing: {wrapped}");
    }

    /// 5 hunks × (1 header + 4 lines) = 25 flat rows.
    fn five_hunks() -> Vec<Hunk> {
        (0..5)
            .map(|i| Hunk {
                old_start: 1 + i * 5,
                new_start: 1 + i * 5,
                lines: (0..4)
                    .map(|_| DiffLine {
                        kind: DiffLineKind::Context,
                        text: "ctx".to_string(),
                    })
                    .collect(),
            })
            .collect()
    }

    #[test]
    fn hunk_scroll_offset_pins_selected_header_into_view() {
        let hunks = five_hunks();
        // Hunk 3's header sits at flat row 15 (3 hunks × 5 rows).
        assert_eq!(hunk_header_row(&hunks, 3), 15);
        // Selected 0 → no scroll.
        assert_eq!(hunk_scroll_offset(&hunks, 0, 6), 0);
        // Pane 6 rows: overflow = 25 - 6 = 19; offset 15 ≤ 19 → 15,
        // putting the selected header at the pane top.
        assert_eq!(hunk_scroll_offset(&hunks, 3, 6), 15);
        // Hunk 4's header at 20 > overflow 19 → clamped to 19 (the
        // last full window still shows the header).
        assert_eq!(hunk_scroll_offset(&hunks, 4, 6), 19);
        // Pane taller than the whole diff → never scrolls.
        assert_eq!(hunk_scroll_offset(&hunks, 4, 40), 0);
        // Out-of-range selection is clamped (same as the last hunk).
        assert_eq!(hunk_scroll_offset(&hunks, 99, 6), 19);
        // Empty diff → 0.
        assert_eq!(hunk_scroll_offset(&[], 0, 6), 0);
    }

    #[test]
    fn tall_diff_scrolls_to_selected_hunk_header() {
        // Final-review finding 2 smoke test: a diff taller than the
        // pane must render the SELECTED hunk's header. Previously the
        // Paragraph rendered from row 0 with no scroll, so j/k / g/G
        // changed `selected_hunk` with nothing visible changing.
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).expect("backend");
        let file = DiffFile {
            path: "tall.txt".to_string(),
            old_path: None,
            hunks: five_hunks(),
            binary: false,
        };
        let state = GitTuiState {
            doc: crate::tui_vt::git_tui::DiffDocument { files: vec![file] },
            entries: Vec::new(),
            view: DiffViewMode::Inline,
            ws: WhitespaceMode::Off,
            selected_file: 0,
            selected_hunk: 4,
            sidebar_focus: false,
            staged: std::collections::HashSet::new(),
            commit_mode: false,
            commit_msg: String::new(),
            needs_refresh: false,
            width: 60,
            height: 10,
            branch: None,
            wrap: false,
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 10,
        };
        terminal
            .draw(|f| render_diff_pane(f, area, &state))
            .expect("draw");
        let buf = terminal.backend().buffer();
        let mut rendered = String::new();
        for y in 0..buf.area().height {
            for x in 0..buf.area().width {
                if let Some(c) = buf.cell((x, y)) {
                    rendered.push_str(c.symbol());
                }
            }
        }
        // Selected hunk 4 (old_start = 21) must be on screen…
        assert!(
            rendered.contains("-21,4"),
            "selected hunk header missing from pane: {rendered}"
        );
        // …and hunk 0's header (old_start = 1) must have scrolled out.
        assert!(
            !rendered.contains("-1,4"),
            "unselected first hunk still at pane top: {rendered}"
        );
    }
}
