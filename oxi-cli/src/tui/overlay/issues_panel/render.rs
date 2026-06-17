//! Rendering for the issues panel — list view, detail view, styling helpers.
//!
//! All public field access is through `pub(super)` in `mod.rs`. The split
//! keeps the file under 400 lines and avoids the god-module antipattern.

use super::IssuesPanelOverlay;
use super::state::View;
use crate::store::issues::{Issue, Priority, Status};
use crate::tui::overlay::OverlayComponent;
use oxi_tui::Theme;
use oxi_tui::text::truncate_to_width;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, HighlightSpacing, List, ListItem, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, Wrap,
    },
};
use unicode_width::UnicodeWidthChar;

impl IssuesPanelOverlay {
    pub(super) fn render_list(&mut self, frame: &mut Frame, area: Rect, _theme: &Theme) {
        // ── Outer block ─────────────────────────────────────────────────
        let title = build_list_title(
            self.items.len(),
            &self.selected_position_label(),
            self.filter_label(),
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_bottom(Line::from(self.hint()).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.last_inner = inner;

        if self.items.is_empty() {
            let msg = Paragraph::new(
                "No issues. Use `/issue new <title>` or the agent's `issue create`.",
            )
            .alignment(ratatui::layout::Alignment::Center)
            .wrap(Wrap { trim: false });
            frame.render_widget(msg, inner);
            return;
        }

        // ── Two-row split: list + status footer ──────────────────────────
        // Use Min(0) so we don't panic on terminals smaller than 4 rows.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(1),
                // Filter input bar (only when active).
                Constraint::Length(if self.filter_input_mode { 1 } else { 0 }),
            ])
            .split(inner);

        // ── The list itself ─────────────────────────────────────────────
        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|i| {
                let lock_glyph = if i.meta.assigned_to.is_some() {
                    "🔒"
                } else {
                    "  "
                };
                let status_cell = styled_status(&i.meta.status);
                let prio_cell = styled_priority(i.meta.priority);
                ListItem::new(Line::from(vec![
                    format!("#{:<4} ", i.meta.id).into(),
                    status_cell,
                    " ".into(),
                    prio_cell,
                    format!("  {lock_glyph} ").into(),
                    i.meta.title.clone().into(),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED).bold())
            .highlight_symbol("▶ ")
            .highlight_spacing(HighlightSpacing::Always);
        frame.render_stateful_widget(list, rows[0], &mut self.list_state);

        // ── Status footer line ("N items · filter: X") ─────────────────
        let summary = format!(
            "{} items · filter: {} · {}",
            self.items.len(),
            self.filter_label(),
            self.selected_position_label()
        );
        let footer = Paragraph::new(summary).alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(footer, rows[1]);

        // ── Filter input bar (only when `/`-mode is active) ─────────
        if self.filter_input_mode && rows.len() > 2 {
            let bar = Paragraph::new(format!("/ filter: {}\u{2588}", self.filter_input_text))
                .style(Style::default().fg(oxi_tui::cell::Color::Yellow));
            frame.render_widget(bar, rows[2]);
        }

        // ── Scrollbar (right edge of the list area) ─────────────────────
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state =
            ScrollbarState::new(self.items.len()).position(self.list_state.selected().unwrap_or(0));
        frame.render_stateful_widget(
            scrollbar,
            rows[0].inner(Margin {
                vertical: 0,
                horizontal: 1,
            }),
            &mut scrollbar_state,
        );
    }

    pub(super) fn render_detail(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let Some(issue) = self.selected().cloned() else {
            self.view = View::List;
            return self.render_list(frame, area, theme);
        };
        let hash = self.current_hash();

        // ── Outer block + multi-segment title ───────────────────────────
        let title = build_detail_title(
            issue.meta.id,
            issue.meta.title.as_str(),
            &self.selected_position_label(),
            self.items.len(),
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_bottom(Line::from(self.hint()).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // ── Two-pane split: metadata table + scrollable body ────────────
        // Compute the metadata table height dynamically (rows.len() + header
        // + 1 for the outer border). Cap at half the inner area so the body
        // always gets at least one row on tall terminals.
        let meta_rows: Vec<Row<'_>> = build_metadata_rows(&issue, &hash);
        let meta_height = ((meta_rows.len() as u16) + 2).min(inner.height / 2);
        // NOTE: avoid Min(>=1) here — small terminals (inner height < 14)
        // would otherwise panic the Layout split. Min(0) lets the body
        // collapse gracefully when the metadata table fills the available
        // space.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(meta_height), Constraint::Min(0)])
            .split(inner);

        // ── Metadata table ──────────────────────────────────────────────
        let header = Row::new(vec![Cell::from("field").bold(), Cell::from("value").bold()]);
        let widths = [Constraint::Length(10), Constraint::Min(20)];
        let table = Table::new(meta_rows, widths)
            .header(header)
            .column_spacing(2);
        frame.render_widget(table, chunks[0]);

        // ── Body (scrollable) ───────────────────────────────────────────
        // Hard-wrap each source line to the body's inner width BEFORE
        // counting and slicing — so the scrollbar tracks display rows, not
        // source lines. Without this, a single 200-char URL in the body
        // would scroll 1 source line per display row, breaking the position
        // indicator and scrollbar accuracy.
        let body_block_pre = Block::default().borders(Borders::TOP);
        let body_inner = body_block_pre.inner(chunks[1]);
        let body_visible = body_inner.height as usize;
        self.detail_visible = body_visible.max(1);

        let all_lines = detail_body_lines_for_render(&issue, &hash, &theme.to_styles());
        let wrapped = hard_wrap_lines(&all_lines, body_inner.width as usize);
        self.total_wrapped_rows = wrapped.len();
        self.clamp_detail_scroll();

        let total_rows = wrapped.len();
        let start = self.detail_scroll;
        let end = (start + self.detail_visible).min(total_rows);
        let visible: Vec<Line<'_>> = wrapped[start..end].to_vec();

        let body_block = body_block_pre.title(Line::from(format!(
            "body  ({}/{} rows)",
            start + 1,
            total_rows
        )));
        frame.render_widget(body_block, chunks[1]);
        // `wrap(Wrap { trim: false })` keeps any line exactly at the inner
        // width from being re-wrapped — `hard_wrap_lines` already split it.
        frame.render_widget(
            Paragraph::new(visible).wrap(Wrap { trim: false }),
            body_inner,
        );

        // ── Body scrollbar ──────────────────────────────────────────────
        let body_scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut body_sb_state = ScrollbarState::new(total_rows).position(start);
        frame.render_stateful_widget(
            body_scrollbar,
            chunks[1].inner(Margin {
                vertical: 1,
                horizontal: 1,
            }),
            &mut body_sb_state,
        );
    }
}

// ── Free helpers (used by both render and mod tests) ─────────────────

/// Number of body lines visible in the detail view at once — used as a
/// heuristic for PageUp/PageDown inside the body. The actual value comes
/// from the live layout split.
pub(super) const DETAIL_BODY_LINES_HINT: usize = 12;

/// Build the metadata table rows for the detail view. Centralized so the
/// layout height (`rows.len() + 2`) and the rendered table stay in sync.
fn build_metadata_rows(issue: &Issue, hash: &str) -> Vec<Row<'static>> {
    let mut rows: Vec<Row<'static>> = vec![
        Row::new(vec![
            Cell::from("status"),
            Cell::from(styled_status_text(&issue.meta.status)),
        ]),
        Row::new(vec![
            Cell::from("priority"),
            Cell::from(styled_priority_text(issue.meta.priority)),
        ]),
        Row::new(vec![
            Cell::from("created"),
            Cell::from(issue.meta.created_at.to_string()),
        ]),
        Row::new(vec![
            Cell::from("updated"),
            Cell::from(issue.meta.updated_at.to_string()),
        ]),
        Row::new(vec![
            Cell::from("labels"),
            Cell::from(if issue.meta.labels.is_empty() {
                "—".dim().to_string()
            } else {
                issue.meta.labels.join(", ")
            }),
        ]),
        Row::new(vec![
            Cell::from("sessions"),
            Cell::from(if issue.meta.sessions.is_empty() {
                "—".dim().to_string()
            } else {
                issue.meta.sessions.join(", ")
            }),
        ]),
        Row::new(vec![Cell::from("hash"), Cell::from(hash.to_string())]),
    ];
    if let Some(c) = issue.meta.closed_at {
        rows.push(Row::new(vec![
            Cell::from("closed"),
            Cell::from(c.to_string()),
        ]));
    }
    if let Some(a) = &issue.meta.assigned_to {
        rows.push(Row::new(vec![
            Cell::from("owner"),
            Cell::from(format!("{} (since {})", a.session, a.acquired_at)),
        ]));
    }
    rows
}

/// Build the multi-segment title for the list view:
/// `Issues (12) — 3 of 12 — filter: open`
fn build_list_title(total: usize, position: &str, filter: &str) -> Line<'static> {
    Line::from(vec![
        "Issues ".bold(),
        format!("({total})").dim(),
        "  ──  ".dim(),
        position.to_string().dim(),
        "  ──  ".dim(),
        "filter:".dim(),
        format!(" {filter}").bold(),
    ])
}

/// Build the multi-segment title for the detail view:
/// `Issue #12 ── Fix login bug ── 3 of 12`
fn build_detail_title(id: u32, title: &str, position: &str, total: usize) -> Line<'static> {
    // Truncate by terminal columns, not char count — Korean/CJK titles
    // occupy 2 columns per char so char-based truncation overflows.
    let truncated = truncate_to_width(title, 48);
    Line::from(vec![
        "Issue ".bold(),
        format!("#{id}").bold().yellow(),
        "  ──  ".dim(),
        truncated.dim(),
        "  ──  ".dim(),
        format!("{position} of {total}").dim(),
    ])
}

/// Build the multi-line `Vec<Line>` for the detail body, with
/// markdown-ish section header + the actual body content.
fn detail_body_lines_for_render(
    issue: &Issue,
    hash: &str,
    styles: &oxi_tui::theme::ThemeStyles,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    out.push(Line::from(vec![
        "title: ".into(),
        issue.meta.title.clone().bold(),
    ]));
    out.push(Line::from(vec![
        "id: ".into(),
        format!("#{}", issue.meta.id).into(),
        "  status: ".into(),
        styled_status(&issue.meta.status),
        "  priority: ".into(),
        styled_priority(issue.meta.priority),
    ]));
    out.push(Line::from(""));
    out.push(Line::from("metadata".bold().underlined()));
    out.push(Line::from(format!("  created: {}", issue.meta.created_at)));
    out.push(Line::from(format!("  updated: {}", issue.meta.updated_at)));
    if let Some(c) = issue.meta.closed_at {
        out.push(Line::from(format!("  closed:  {c}")));
    }
    if !issue.meta.labels.is_empty() {
        out.push(Line::from(format!(
            "  labels:  {}",
            issue.meta.labels.join(", ")
        )));
    }
    if !issue.meta.sessions.is_empty() {
        out.push(Line::from(format!(
            "  sessions: {}",
            issue.meta.sessions.join(", ")
        )));
    }
    if let Some(a) = &issue.meta.assigned_to {
        out.push(Line::from(format!(
            "  owner:   {} (since {})",
            a.session, a.acquired_at
        )));
    }
    out.push(Line::from(format!("  hash:    {hash}")));
    out.push(Line::from(""));
    out.push(Line::from("body".bold().underlined()));
    out.push(Line::from(""));
    // Render the body via the shared markdown renderer so headers, code
    // fences, lists, and inline `code`/`bold` get the same treatment as
    // the chat widget.
    if issue.body.is_empty() {
        out.push(Line::from("(empty body)".dim().italic()));
    } else {
        let md_lines = oxi_tui::widgets::chat::markdown::render_markdown(&issue.body, styles);
        out.extend(md_lines);
    }
    out
}

// ── Styling helpers (centralized so every renderer agrees) ─────────────

fn styled_status(s: &Status) -> Span<'static> {
    let text = format!("[{}]", s);
    match s {
        Status::Open => text.green().bold(),
        Status::Closed => text.dark_gray(),
    }
}

fn styled_status_text(s: &Status) -> String {
    s.to_string()
}

fn styled_priority(p: Priority) -> Span<'static> {
    let text = format!("{:8}", p);
    match p {
        Priority::Critical => text.red().bold(),
        Priority::High => text.light_red(),
        Priority::Medium => text.yellow(),
        Priority::Low => text.green(),
    }
}

fn styled_priority_text(p: Priority) -> String {
    p.to_string()
}

/// Hard-wrap a slice of styled `Line`s to a maximum terminal width.
///
/// Each source line becomes one or more output lines, splitting at column
/// boundaries (no word-aware breaks, no ellipsis). Multi-span styling is
/// preserved only on the first chunk of each source line; subsequent
/// chunks inherit the first span's style.
///
/// Used by `render_detail` so the body scrollbar and position indicator
/// track **display rows**, not source lines. A 200-char URL that wraps
/// to 3 rows advances the scrollbar by 3 positions, not 1.
fn hard_wrap_lines(lines: &[Line<'static>], max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return lines.to_vec();
    }
    let mut out: Vec<Line<'static>> = Vec::with_capacity(lines.len());
    for line in lines {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        if text.is_empty() {
            out.push(Line::from(""));
            continue;
        }
        let total_width: usize = text
            .chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum();
        if total_width <= max_width {
            out.push(line.clone());
            continue;
        }
        // Split into chunks of ≤ max_width columns.
        let style = line.spans.first().map(|s| s.style).unwrap_or_default();
        let mut remaining: &str = text.as_str();
        while !remaining.is_empty() {
            let (chunk, rest) = take_columns(remaining, max_width);
            out.push(Line::from(Span::styled(chunk.to_string(), style)));
            remaining = rest;
        }
    }
    out
}

/// Take the first `max` columns of `s` and return `(chunk, rest)`.
/// `chunk.len() ≤ max` terminal columns; never appends an ellipsis.
fn take_columns(s: &str, max: usize) -> (&str, &str) {
    if max == 0 {
        return ("", s);
    }
    let mut width = 0usize;
    let mut end = 0usize;
    for (i, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > max {
            break;
        }
        width += cw;
        end = i + ch.len_utf8();
    }
    (&s[..end], &s[end..])
}
#[cfg(test)]
mod tests {
    use super::*;

    fn line_to_string(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.clone().into_owned())
            .collect()
    }

    #[test]
    fn build_list_title_segments_have_total_and_position() {
        let line = build_list_title(12, "3 of 12", "open");
        let s = line_to_string(&line);
        assert!(s.contains("(12)"));
        assert!(s.contains("3 of 12"));
        assert!(s.contains("open"));
    }

    #[test]
    fn build_detail_title_truncates_long_ascii_titles() {
        let line = build_detail_title(7, &"x".repeat(120), "1 of 1", 1);
        let s = line_to_string(&line);
        assert!(s.contains("Issue"));
        assert!(s.contains("#7"));
        assert!(s.contains("…"));
        assert!(!s.contains(&"x".repeat(120)));
    }

    #[test]
    fn build_detail_title_truncates_cjk_by_column_width() {
        // 30 Korean chars = 60 columns, exceeds the 48-column cap. Char-based
        // truncation would overflow the title bar; column-based truncation
        // (via `truncate_to_width`) keeps the title ≤ 49 columns wide.
        let cjk: String = "한글".repeat(30);
        let line = build_detail_title(3, &cjk, "1 of 1", 1);
        let s = line_to_string(&line);
        assert!(s.contains("…"), "CJK title must be truncated");
        let truncated: String = s
            .split("──")
            .nth(1)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let width: usize = truncated
            .chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum();
        assert!(
            width <= 49,
            "CJK title overflow: width={width}, content={truncated:?}"
        );
        assert!(
            !s.contains(&cjk),
            "truncation missing — full CJK title rendered"
        );
    }

    #[test]
    fn build_detail_title_keeps_short_titles_verbatim() {
        let line = build_detail_title(1, "Fix bug", "1 of 1", 1);
        let s = line_to_string(&line);
        assert!(s.contains("Fix bug"));
        assert!(!s.contains("…"));
    }

    #[test]
    fn hard_wrap_lines_keeps_short_lines_intact() {
        let lines: Vec<Line<'static>> = vec![Line::from("hello"), Line::from("world")];
        let wrapped = hard_wrap_lines(&lines, 80);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(line_to_string(&wrapped[0]), "hello");
        assert_eq!(line_to_string(&wrapped[1]), "world");
    }

    #[test]
    fn hard_wrap_lines_splits_long_lines_by_column_width() {
        // 30 ASCII chars with max_width=10 → 3 chunks of 10 chars each.
        let lines: Vec<Line<'static>> = vec![Line::from("x".repeat(30))];
        let wrapped = hard_wrap_lines(&lines, 10);
        assert_eq!(wrapped.len(), 3);
        for chunk in &wrapped {
            assert_eq!(chunk.spans[0].content.len(), 10);
        }
    }

    #[test]
    fn hard_wrap_lines_cjk_uses_column_width() {
        // 10 Korean chars = 20 columns; with max_width=8, splits into 3
        // rows (4 chars / 8 cols each).
        let lines: Vec<Line<'static>> = vec![Line::from("가".repeat(10))];
        let wrapped = hard_wrap_lines(&lines, 8);
        assert_eq!(wrapped.len(), 3);
    }

    #[test]
    fn hard_wrap_lines_preserves_empty_lines() {
        let lines: Vec<Line<'static>> = vec![Line::from("a"), Line::from(""), Line::from("b")];
        let wrapped = hard_wrap_lines(&lines, 80);
        assert_eq!(wrapped.len(), 3);
        assert_eq!(line_to_string(&wrapped[1]), "");
    }

    #[test]
    fn take_columns_splits_on_column_boundary() {
        // "abc" (3 cols) + "한글" (4 cols) at width 5 → "abc한" (5 cols).
        let (chunk, rest) = take_columns("abc한글", 5);
        assert_eq!(chunk, "abc한");
        assert_eq!(rest, "글");
    }

    #[test]
    fn take_columns_handles_zero_width() {
        let (chunk, rest) = take_columns("hello", 0);
        assert_eq!(chunk, "");
        assert_eq!(rest, "hello");
    }
}
