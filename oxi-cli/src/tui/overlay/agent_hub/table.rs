//! Table rendering for the Agent Hub overlay.
//!
//! Renders a full-width table into the provided area (no centering — the
//! overlay owns the fullscreen alt-screen, so the table is the viewport).
//!
//! Status colors come from `theme.colors.*` directly. `Theme` does not
//! expose convenience helpers like `bold()` / `selection_bg()` / `warning()`;
//! we build the `Style` values from the raw `Color` fields.

use oxi_sdk::HubStatus;
use oxi_tui::Theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table};

use super::state::HubState;

/// Layout (column widths chosen to fit common 80–160-col terminals):
/// - Agent    — 20 chars (long enough for "advisor" / "subagent-abc12")
/// - Kind     —  9 chars (matches `HubKind::as_str` lengths)
/// - Status   — 10 chars
/// - Task     — min  (fills remaining space)
/// - Activity — 10 chars (matches "1234h ago" max)
pub fn render_table(f: &mut Frame, area: Rect, state: &HubState, theme: &Theme) {
    let styles = theme.to_styles();

    let header = Row::new(vec![
        Cell::from("Agent").style(styles.accent.add_modifier(Modifier::BOLD)),
        Cell::from("Kind").style(styles.accent.add_modifier(Modifier::BOLD)),
        Cell::from("Status").style(styles.accent.add_modifier(Modifier::BOLD)),
        Cell::from("Task").style(styles.accent.add_modifier(Modifier::BOLD)),
        Cell::from("Activity").style(styles.accent.add_modifier(Modifier::BOLD)),
    ])
    .style(styles.muted)
    .height(1);

    let rows: Vec<Row> = state
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let kind_str = r.kind.as_str();
            let status_str = status_label(r.status);
            let task = r.current_task.as_deref().unwrap_or("—");
            let mut row = Row::new(vec![
                Cell::from(r.display_name.clone()),
                Cell::from(kind_str),
                Cell::from(status_str).style(status_style(r.status, theme)),
                Cell::from(task),
                Cell::from(r.age_text.clone()),
            ]);
            if i == state.selected {
                // Selection highlight: layered on top of any per-cell styling.
                row = row.style(styles.selection_bg);
            }
            row
        })
        .collect();

    let widths = [
        Constraint::Length(20),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Min(1),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Agent Hub "));

    f.render_widget(table, area);
}

/// Lower-case label for a status. Equivalent to `HubStatus::as_str` but kept
/// local so the table can evolve its own labels (e.g. with glyph prefixes)
/// without touching the SDK type.
fn status_label(s: HubStatus) -> &'static str {
    match s {
        HubStatus::Running => "running",
        HubStatus::Idle => "idle",
        HubStatus::Parked => "parked",
        HubStatus::Aborted => "aborted",
    }
}

/// Map status → foreground color. Reads straight off `theme.colors.*`
/// rather than round-tripping through `ThemeStyles` (which only stores the
/// color used to build the style — same value, less indirection).
fn status_style(s: HubStatus, theme: &Theme) -> Style {
    let color = match s {
        HubStatus::Running => theme.colors.success,
        HubStatus::Idle => theme.colors.muted,
        HubStatus::Parked => theme.colors.warning,
        HubStatus::Aborted => theme.colors.error,
    };
    Style::default().fg(color)
}
