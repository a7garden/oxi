// oxicode-cli/src/tui_vt/issues_panel/render.rs
//! Rendering for the `/issue` panel. Full-screen overlay — called from
//! `render_frame` when `state.issues_panel.is_some()`.

use super::{
    AssigneeBadge, FormField, IssueFormState, IssueRow, IssuesPanelMode, IssuesPanelState,
};
use oxicode_textarea::TextAreaState;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, FrameExt, List, ListItem, Paragraph};

pub(crate) fn render_issues_panel(frame: &mut Frame<'_>, area: Rect, panel: &IssuesPanelState) {
    match &panel.mode {
        IssuesPanelMode::List => render_list(frame, area, panel),
        IssuesPanelMode::FilterInput(buf) => {
            render_list(frame, area, panel);
            render_filter_hint(frame, area, buf);
        }
        IssuesPanelMode::Detail { id, scroll } => render_detail(frame, area, panel, *id, *scroll),
        IssuesPanelMode::Form(form) => render_form(frame, area, form, panel),
    }
}
fn render_list(frame: &mut Frame<'_>, area: Rect, panel: &IssuesPanelState) {
    // F3 fix: append a short " (busy…)" when an async mutation is in flight,
    // so the user gets visible feedback while `panel.pending == true` and
    // the mutating keys (`c`/`r`/Ctrl+Enter) are gated out below.
    let busy_suffix = if panel.pending { " (busy\u{2026})" } else { "" };
    let title = format!(
        "Issues \u{2014} {} ({}){}",
        match panel.status_filter {
            Some(crate::store::issues::Status::Open) => "open",
            Some(crate::store::issues::Status::Closed) => "closed",
            None => "all",
        },
        panel.rows.len(),
        busy_suffix,
    );
    let items: Vec<ListItem> = panel
        .rows
        .iter()
        .map(|row| ListItem::new(Line::from(row_spans(row))))
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(panel.selected.min(panel.rows.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut list_state);
    render_error_footer(frame, area, panel.error.as_deref());
}

/// Draw `err` (if any) as a single red line at the bottom of `area`.
/// Used by both `render_list` and `render_form` to surface a one-line error
/// footer (design §6) without coupling either to the panel's state shape.
fn render_error_footer(frame: &mut Frame<'_>, area: Rect, err: Option<&str>) {
    let Some(err) = err else { return };
    let footer = Rect {
        y: area.y + area.height.saturating_sub(1),
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            err.to_string(),
            Style::default().fg(ratatui::style::Color::Red),
        ))),
        footer,
    );
}

fn row_spans(row: &IssueRow) -> Vec<Span<'static>> {
    let badge = match &row.assignee_badge {
        Some(AssigneeBadge::Live(s)) => format!(" [working: {s}]"),
        Some(AssigneeBadge::Stale(s)) => format!(" [stale claim: {s}]"),
        None => String::new(),
    };
    vec![Span::raw(format!(
        "#{} [{}] {}  {}  {}{}",
        row.id,
        row.priority,
        row.title,
        row.status,
        row.labels.join(","),
        badge
    ))]
}

fn render_filter_hint(frame: &mut Frame<'_>, area: Rect, buf: &str) {
    let hint = Rect {
        y: area.y + area.height.saturating_sub(3),
        height: 3,
        ..area
    };
    let text = format!(
        "filter: {buf}\nEnter: apply · Esc: cancel · Ctrl+U: clear · syntax: priority=critical label=auth text"
    );
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL)),
        hint,
    );
}

fn render_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    panel: &IssuesPanelState,
    id: u32,
    scroll: usize,
) {
    // If the row vanished from `panel.rows` (e.g. the underlying issue was
    // deleted or the filter changed under us), fall back to the list view
    // rather than panicking on a missing header.
    let Some(row) = panel.rows.iter().find(|r| r.id == id) else {
        render_list(frame, area, panel);
        return;
    };
    // Meta header per design §5: id / status / priority / title / labels on
    // the first line; assignee badge + created/updated/closed timestamps on
    // the second.
    let badge = match &row.assignee_badge {
        Some(AssigneeBadge::Live(s)) => format!(" [working: {s}]"),
        Some(AssigneeBadge::Stale(s)) => format!(" [stale claim: {s}]"),
        None => String::new(),
    };
    let stamp = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%d %H:%M").to_string();
    let header = format!(
        "#{} {}  [{}] {}  labels: {}",
        row.id,
        row.status,
        row.priority,
        row.title,
        row.labels.join(",")
    );
    let meta_header = format!(
        "assignee:{}  created {}  updated {}  closed {}",
        badge,
        stamp(row.created_at),
        stamp(row.updated_at),
        row.closed_at.map(stamp).unwrap_or_else(|| "—".into()),
    );
    // F3 fix: surface pending state in the title (mirror of render_list).
    let busy_suffix = if panel.pending { " (busy\u{2026})" } else { "" };
    let block = Block::default().borders(Borders::ALL).title(format!(
        "Issue #{id} \u{2014} Esc: back, e: edit, c: close, r: reopen{busy_suffix}"
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_area = Rect { height: 1, ..inner };
    frame.render_widget(Paragraph::new(header), header_area);
    let meta_area = Rect {
        y: inner.y + 1,
        height: 1,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(meta_header).style(Style::default().add_modifier(Modifier::DIM)),
        meta_area,
    );

    let body_area = Rect {
        y: inner.y + 3,
        height: inner.height.saturating_sub(3),
        ..inner
    };
    let body_text = panel.detail_body_cache.as_deref().unwrap_or("(loading…)");
    let lines =
        oxicode_vtui::tui::ui::markdown::render_markdown(body_text, body_area.width as usize);
    let styles = oxicode_vtui::theme::active_styles();
    let ratatui_lines: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .map(|segs| {
            Line::from(
                segs.into_iter()
                    .map(|seg| {
                        let style = crate::tui_vt::main_loop::segment_style(
                            &seg,
                            Style::default(),
                            &styles,
                        );
                        Span::styled(seg.text, style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(ratatui_lines), body_area);
    // F2 fix: surface `panel.error` as a one-line footer (design §6).
    // Detail's `c` confirm + async c/r dispatch funnel failures here, and
    // without this footer the `start_edit` claim-block and CAS-retry
    // errors would be invisible. Mirrors the existing footer in
    // render_list / render_form.
    render_error_footer(frame, area, panel.error.as_deref());
}

/// Render the Create/Edit form for an issue.
///
/// Layout (top → bottom):
/// 1. Header block titled "New issue — …" or "Edit issue — …" with the
///    Tab / Ctrl+Enter / Esc hint line for the current mode.
/// 2. A 4-line preamble: Title, Priority (with `←/→` arrows), Labels, and a
///    `Body:` label row. The currently-focused field is prefixed with `"> "`;
///    other fields get `"  "`. `Priority` shows the current value inline.
/// 3. The TextArea body editor occupies whatever remains of `inner` (≥0
///    rows after the 4-line preamble).
///
/// Mirrors the composer's render call site in `main_loop.rs` (`render_widget_ref`
/// on a `&TextArea`) — `oxicode-textarea::TextArea` does not implement
/// `Widget` for an owned value, only `WidgetRef` for `&TextArea`.
fn render_form(frame: &mut Frame<'_>, area: Rect, form: &IssueFormState, panel: &IssuesPanelState) {
    let title = if form.editing_id.is_some() {
        "Edit issue — Tab: next field, Ctrl+Enter: save, Esc: cancel"
    } else {
        "New issue — Tab: next field, Ctrl+Enter: create, Esc: cancel"
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let focus_marker = |f: FormField| if form.focus == f { "> " } else { "  " };

    let title_line = format!("{}Title: {}", focus_marker(FormField::Title), form.title);
    let priority_line = format!(
        "{}Priority (\u{2190}/\u{2192}): {}",
        focus_marker(FormField::Priority),
        form.priority
    );
    let labels_line = format!(
        "{}Labels (comma-separated): {}",
        focus_marker(FormField::Labels),
        form.labels_input
    );

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(title_line),
            Line::from(priority_line),
            Line::from(labels_line),
            Line::from(format!("{}Body:", focus_marker(FormField::Body))),
        ]),
        Rect { height: 4, ..inner },
    );

    let body_area = Rect {
        y: inner.y + 4,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    // The composer (main_loop.rs:5599) renders the textarea via
    // `frame.render_widget_ref(&state.composer, textarea_area)` —
    // oxicode-textarea's `TextArea` exposes rendering through `WidgetRef`,
    // not an inherent `render` method, so this is the analogous call.
    frame.render_widget_ref(&form.body, body_area);

    // F3 fix: place the hardware caret at the body's cursor. Mirrors
    // `render_composer` (`main_loop.rs:5601-5612`). `cursor_pos_with_state`
    // returns ABSOLUTE coordinates already (it adds `body_area.x`/`.y`)
    // — do not re-add the area origin.
    if form.focus == FormField::Body
        && let Some((cx, cy)) = form
            .body
            .cursor_pos_with_state(body_area, TextAreaState::default())
    {
        frame.set_cursor_position(Position::new(cx, cy));
    }

    // F2 fix: surface `panel.error` as a one-line footer (design §6).
    // `submit_form`'s Create-Err path sets `panel.error` while leaving
    // `mode = Form` so the user can retry — without this footer the
    // failure is invisible.
    render_error_footer(frame, area, panel.error.as_deref());
}

#[cfg(test)]
mod render_tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::store::issues::{Priority, Status};

    fn sample_row() -> IssueRow {
        use chrono::TimeZone;
        let t = chrono::Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap();
        IssueRow {
            id: 1,
            title: "sample issue".into(),
            status: Status::Open,
            priority: Priority::High,
            labels: vec!["auth".into()],
            assignee_badge: Some(AssigneeBadge::Live("tui".into())),
            created_at: t,
            updated_at: t,
            closed_at: None,
        }
    }

    /// Draw `panel` on a `width × height` TestBackend and return every
    /// rendered cell symbol concatenated (row-major — mirrors the buffer
    /// assertion idiom in `main_loop.rs`'s render tests). The `draw`
    /// closure runs the full render path, so a `Rect` underflow inside
    /// any `render_*` helper panics this test; the returned text lets
    /// callers also assert the panel actually painted content.
    fn draw(panel: &IssuesPanelState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render_issues_panel(frame, frame.area(), panel))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn list_mode_renders_without_panicking() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            ..Default::default()
        };
        let text = draw(&panel, 80, 24);
        assert!(text.contains("sample issue"), "list body missing: {text:?}");
    }

    #[test]
    fn detail_mode_renders_without_panicking() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            mode: IssuesPanelMode::Detail { id: 1, scroll: 0 },
            detail_body_cache: Some("# Heading\n\nSome **body** text.".into()),
            ..Default::default()
        };
        // 100 cols: the §5 meta line (~86 chars) must not truncate.
        let text = draw(&panel, 100, 24);
        assert!(text.contains("Issue #1"), "detail title missing: {text:?}");
        // Design §5 meta header: assignee badge line + created/updated/closed
        // timestamps (fixture: Live("tui"), 2026-08-27 12:00, closed = —).
        assert!(
            text.contains("[working: tui]"),
            "assignee badge missing from detail meta header: {text:?}"
        );
        assert!(
            text.contains("created 2026-08-27 12:00")
                && text.contains("updated 2026-08-27 12:00")
                && text.contains("closed —"),
            "timestamp line missing from detail meta header: {text:?}"
        );
    }

    #[test]
    fn form_mode_renders_without_panicking() {
        let panel = IssuesPanelState {
            mode: IssuesPanelMode::Form(Box::default()),
            ..Default::default()
        };
        let text = draw(&panel, 80, 24);
        assert!(text.contains("New issue —"), "form title missing: {text:?}");
    }

    #[test]
    fn form_body_focus_and_error_footer_render() {
        // `focus == Body` drives render_form's caret branch
        // (`cursor_pos_with_state` → `set_cursor_position`); a non-None
        // `panel.error` drives `render_error_footer`'s Some path.
        let form = IssueFormState {
            focus: FormField::Body,
            ..Default::default()
        };
        let panel = IssuesPanelState {
            mode: IssuesPanelMode::Form(Box::new(form)),
            error: Some("boom".into()),
            ..Default::default()
        };
        let text = draw(&panel, 80, 24);
        assert!(text.contains("boom"), "error footer missing: {text:?}");
    }

    #[test]
    fn filter_input_mode_renders_without_panicking() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            mode: IssuesPanelMode::FilterInput("priority=high".into()),
            ..Default::default()
        };
        let text = draw(&panel, 80, 24);
        assert!(
            text.contains("filter: priority=high"),
            "filter hint missing: {text:?}"
        );
    }

    /// A 1-row viewport is the smallest a terminal can realistically
    /// report. Each risky mode below drives a different `saturating_sub`
    /// site; `List` (empty rows + selected clamp) is kept as the baseline.
    #[test]
    fn empty_list_tiny_viewport_does_not_panic() {
        let panel = IssuesPanelState::default();
        draw(&panel, 20, 1);
    }

    #[test]
    fn filter_input_tiny_viewport_does_not_panic() {
        // 20×1 hits render_filter_hint's `area.height.saturating_sub(3)`
        // at height 1 — the hardest underflow in the module (1 < 3).
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            mode: IssuesPanelMode::FilterInput("priority=high".into()),
            ..Default::default()
        };
        draw(&panel, 20, 1);
    }

    #[test]
    fn detail_tiny_viewport_does_not_panic() {
        // 20×1 hits render_detail's `inner.height.saturating_sub(2)` with
        // inner height 0 (the borders consume the single row).
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            mode: IssuesPanelMode::Detail { id: 1, scroll: 0 },
            detail_body_cache: Some("body".into()),
            ..Default::default()
        };
        draw(&panel, 20, 1);
    }

    #[test]
    fn form_tiny_viewport_does_not_panic() {
        // 20×1 hits render_form's `inner.height.saturating_sub(4)` (and
        // the `Rect { height: 4, ..inner }` preamble) with inner height 0.
        let panel = IssuesPanelState {
            mode: IssuesPanelMode::Form(Box::default()),
            ..Default::default()
        };
        draw(&panel, 20, 1);
    }

    /// F2 regression: Detail's `render_error_footer` must paint the
    /// panel's `error` string — without the footer, async `c`/`r` and
    /// `e` claim-block failures are invisible to the user.
    #[test]
    fn detail_mode_renders_error_footer() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            mode: IssuesPanelMode::Detail { id: 1, scroll: 0 },
            detail_body_cache: Some("body".into()),
            error: Some("boom-detail".into()),
            ..Default::default()
        };
        let text = draw(&panel, 80, 24);
        assert!(
            text.contains("boom-detail"),
            "detail error footer missing: {text:?}"
        );
    }

    /// F3 regression: List and Detail titles append " (busy…)" while
    /// `panel.pending == true`. The marker is the user's only visible
    /// signal that mutating keys are gated out by the input lock.
    #[test]
    fn list_mode_renders_busy_marker_when_pending() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            pending: true,
            ..Default::default()
        };
        let text = draw(&panel, 80, 24);
        assert!(text.contains("busy"), "list busy marker missing: {text:?}");
    }

    #[test]
    fn detail_mode_renders_busy_marker_when_pending() {
        let panel = IssuesPanelState {
            rows: vec![sample_row()],
            mode: IssuesPanelMode::Detail { id: 1, scroll: 0 },
            detail_body_cache: Some("body".into()),
            pending: true,
            ..Default::default()
        };
        let text = draw(&panel, 80, 24);
        assert!(
            text.contains("busy"),
            "detail busy marker missing: {text:?}"
        );
    }
}
