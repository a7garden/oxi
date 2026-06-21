//! TodoPanel — sticky todo display widget.
//!
//! Renders a compact todo list between the chat scroll area and the input.
//! Uses ratatui's `StatefulWidget` pattern. The state (`TodoPanelState`)
//! lives in the caller (oxi-cli's `AppState`).
//!
//! oxi-tui is dependency-free (no oxi-agent), so the widget takes plain
//! data types (`TodoPanelPhase`, `TodoPanelItem`) that the caller converts
//! from `oxi_agent::tools::todo::TodoPhase`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{StatefulWidget, Widget};

use crate::theme::Theme;

// ── Data types (mirror of oxi_agent::tools::todo, kept independent) ───

/// Todo task status for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoPanelStatus {
    /// Not started.
    Pending,
    /// Currently being worked on.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Cancelled / skipped.
    Abandoned,
}

/// A single todo task.
#[derive(Debug, Clone)]
pub struct TodoPanelItem {
    /// Short task description (5-10 words).
    pub content: String,
    /// Current status.
    pub status: TodoPanelStatus,
}

/// A phase (group) of tasks.
#[derive(Debug, Clone)]
pub struct TodoPanelPhase {
    /// Phase name (e.g. "Foundation", "Auth").
    pub name: String,
    /// Tasks in this phase.
    pub tasks: Vec<TodoPanelItem>,
}

// ── State ─────────────────────────────────────────────────────────────

/// State for the [`TodoPanel`] widget.
///
/// Owned by the caller (typically `AppState` in oxi-cli).
/// Updated whenever the agent's `todo` tool changes phases.
#[derive(Debug, Clone, Default)]
pub struct TodoPanelState {
    /// All phases with their tasks.
    pub phases: Vec<TodoPanelPhase>,
    /// Expanded mode: show all phases. Collapsed: active phase only.
    pub expanded: bool,
    /// Max tasks to show in collapsed mode (per active phase).
    pub max_visible: usize,
}

impl TodoPanelState {
    /// Create new empty state.
    pub fn new() -> Self {
        Self {
            phases: Vec::new(),
            expanded: false,
            max_visible: 5,
        }
    }

    /// Number of lines the panel will occupy (for layout height calculation).
    pub fn line_count(&self) -> usize {
        let non_empty: Vec<&TodoPanelPhase> =
            self.phases.iter().filter(|p| !p.tasks.is_empty()).collect();
        if non_empty.is_empty() {
            return 0;
        }
        let header = 2; // blank line + "Todos" header
        if self.expanded {
            let body: usize = non_empty
                .iter()
                .map(|p| {
                    if non_empty.len() > 1 {
                        1 + p.tasks.len() // phase header + tasks
                    } else {
                        p.tasks.len()
                    }
                })
                .sum();
            header + body
        } else {
            match self.active_phase() {
                Some(phase) => {
                    let visible = phase.tasks.len().min(self.max_visible);
                    let hidden = phase.tasks.len().saturating_sub(self.max_visible);
                    let hidden_line = if hidden > 0 { 1 } else { 0 };
                    header + visible + hidden_line
                }
                None => 0,
            }
        }
    }

    /// The active phase: first with an `InProgress` task, or the first
    /// phase with any pending/in-progress tasks.
    pub fn active_phase(&self) -> Option<&TodoPanelPhase> {
        self.phases
            .iter()
            .find(|p| {
                p.tasks
                    .iter()
                    .any(|t| t.status == TodoPanelStatus::InProgress)
            })
            .or_else(|| {
                self.phases.iter().find(|p| {
                    p.tasks.iter().any(|t| {
                        matches!(
                            t.status,
                            TodoPanelStatus::Pending | TodoPanelStatus::InProgress
                        )
                    })
                })
            })
    }

    /// True if any phase has completed/abandoned tasks.
    pub fn has_closed_todos(&self) -> bool {
        self.phases.iter().any(|p| {
            p.tasks.iter().any(|t| {
                matches!(
                    t.status,
                    TodoPanelStatus::Completed | TodoPanelStatus::Abandoned
                )
            })
        })
    }

    /// Whether the panel should be rendered at all.
    pub fn is_empty(&self) -> bool {
        self.phases.iter().all(|p| p.tasks.is_empty())
    }
    /// Replace all phases. Used by the host (oxi-cli) to sync from the
    /// agent's `TodoStateProvider`.
    pub fn set_phases(&mut self, phases: Vec<TodoPanelPhase>) {
        self.phases = phases;
    }
}

// ── Widget ────────────────────────────────────────────────────────────

/// Sticky todo panel widget.
///
/// Renders between the chat view and the input field.
/// Pass `TodoPanelState` as the state.
pub struct TodoPanel<'a> {
    theme: &'a Theme,
}

impl<'a> TodoPanel<'a> {
    /// Create a new panel with the given theme.
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

impl StatefulWidget for TodoPanel<'_> {
    type State = TodoPanelState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if state.is_empty() {
            return;
        }

        let styles = self.theme.to_styles();
        let accent = styles.accent.add_modifier(Modifier::BOLD);
        let dim = styles.muted;
        let success = styles.success;
        let error = styles.error;

        let phases: Vec<&TodoPanelPhase> = state
            .phases
            .iter()
            .filter(|p| !p.tasks.is_empty())
            .collect();
        if phases.is_empty() {
            return;
        }

        let mut y = area.y;
        let x = area.x + 1; // 1-char left indent

        // Header: "<icon> Todos"
        let header_line = Line::from(vec![
            Span::styled(format!("{} ", styles.symbols.icon_todo), accent),
            Span::styled("Todos", accent),
        ]);
        buf.set_line(x, y, &header_line, area.width);
        y += 2; // header + blank line

        if state.expanded {
            for (i, phase) in phases.iter().enumerate() {
                if phases.len() > 1 {
                    let phase_header = format_phase_display(&phase.name, i + 1);
                    buf.set_string(x, y, &phase_header, accent);
                    y += 1;
                }
                for task in &phase.tasks {
                    let line =
                        render_task_line(task, &styles.symbols, &dim, &success, &error, &accent);
                    buf.set_line(x + 1, y, &line, area.width);
                    y += 1;
                }
            }
        } else {
            if let Some(phase) = state.active_phase() {
                let visible_count = phase.tasks.len().min(state.max_visible);
                for task in phase.tasks.iter().take(visible_count) {
                    let line =
                        render_task_line(task, &styles.symbols, &dim, &success, &error, &accent);
                    buf.set_line(x + 1, y, &line, area.width);
                    y += 1;
                }
                let hidden = phase.tasks.len().saturating_sub(state.max_visible);
                if hidden > 0 {
                    let hidden_text = format!("+ {} more", hidden);
                    buf.set_string(x + 1, y, &hidden_text, dim);
                }
            }
        }
    }
}

fn render_task_line(
    task: &TodoPanelItem,
    symbols: &crate::symbols::Symbols,
    dim: &Style,
    success: &Style,
    error: &Style,
    accent: &Style,
) -> Line<'static> {
    let (icon, style) = match task.status {
        TodoPanelStatus::Completed => (symbols.checkbox_on, *success),
        TodoPanelStatus::InProgress => (symbols.status_running, *accent),
        TodoPanelStatus::Abandoned => (symbols.status_error, *error),
        TodoPanelStatus::Pending => (symbols.checkbox_off, *dim),
    };

    let mut spans = vec![Span::styled(icon.to_string(), style), Span::raw(" ")];

    // Completed/Abandoned get strikethrough
    let content_style = match task.status {
        TodoPanelStatus::Completed | TodoPanelStatus::Abandoned => {
            style.add_modifier(Modifier::CROSSED_OUT)
        }
        _ => style,
    };
    spans.push(Span::styled(task.content.clone(), content_style));

    Line::from(spans)
}

fn format_phase_display(name: &str, one_based_index: usize) -> String {
    format!("{}. {}", roman_numeral(one_based_index), name)
}

fn roman_numeral(n: usize) -> String {
    let pairs: &[(usize, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut n = n;
    let mut out = String::new();
    for &(value, sym) in pairs {
        while n >= value {
            out.push_str(sym);
            n -= value;
        }
    }
    out
}

// Allow Widget trait impl for direct rendering without state (empty).
impl Widget for TodoPanel<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut state = TodoPanelState::new();
        StatefulWidget::render(self, area, buf, &mut state);
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(content: &str, status: TodoPanelStatus) -> TodoPanelItem {
        TodoPanelItem {
            content: content.into(),
            status,
        }
    }

    #[test]
    fn empty_state_renders_nothing() {
        let state = TodoPanelState::new();
        assert_eq!(state.line_count(), 0);
        assert!(state.is_empty());
    }

    #[test]
    fn collapsed_shows_active_phase_only() {
        let mut state = TodoPanelState::new();
        state.phases = vec![
            TodoPanelPhase {
                name: "A".into(),
                tasks: vec![
                    make_item("a1", TodoPanelStatus::Completed),
                    make_item("a2", TodoPanelStatus::InProgress),
                ],
            },
            TodoPanelPhase {
                name: "B".into(),
                tasks: vec![make_item("b1", TodoPanelStatus::Pending)],
            },
        ];
        // Active phase = A (has InProgress)
        assert_eq!(state.active_phase().unwrap().name, "A");
        // Collapsed: header(2) + 2 visible tasks = 4
        assert_eq!(state.line_count(), 4);
    }

    #[test]
    fn expanded_shows_all_phases() {
        let mut state = TodoPanelState::new();
        state.phases = vec![
            TodoPanelPhase {
                name: "A".into(),
                tasks: vec![make_item("a1", TodoPanelStatus::Completed)],
            },
            TodoPanelPhase {
                name: "B".into(),
                tasks: vec![make_item("b1", TodoPanelStatus::Pending)],
            },
        ];
        state.expanded = true;
        // header(2) + phase A header(1) + 1 task + phase B header(1) + 1 task = 6
        assert_eq!(state.line_count(), 6);
    }

    #[test]
    fn collapsed_caps_at_max_visible() {
        let mut state = TodoPanelState::new();
        state.phases = vec![TodoPanelPhase {
            name: "Big".into(),
            tasks: (0..10)
                .map(|i| make_item(&format!("task {}", i), TodoPanelStatus::Pending))
                .collect(),
        }];
        state.max_visible = 5;
        // header(2) + 5 visible + 1 "more" line = 8
        assert_eq!(state.line_count(), 8);
    }

    #[test]
    fn has_closed_todos_detects() {
        let mut state = TodoPanelState::new();
        state.phases = vec![TodoPanelPhase {
            name: "A".into(),
            tasks: vec![make_item("a1", TodoPanelStatus::Completed)],
        }];
        assert!(state.has_closed_todos());

        state.phases[0].tasks[0].status = TodoPanelStatus::Pending;
        assert!(!state.has_closed_todos());
    }

    #[test]
    fn active_phase_falls_back_to_pending() {
        let mut state = TodoPanelState::new();
        state.phases = vec![
            TodoPanelPhase {
                name: "Done".into(),
                tasks: vec![make_item("x", TodoPanelStatus::Completed)],
            },
            TodoPanelPhase {
                name: "Active".into(),
                tasks: vec![make_item("y", TodoPanelStatus::Pending)],
            },
        ];
        // No InProgress → falls back to first phase with Pending
        assert_eq!(state.active_phase().unwrap().name, "Active");
    }

    #[test]
    fn roman_numeral_correct() {
        assert_eq!(roman_numeral(1), "I");
        assert_eq!(roman_numeral(4), "IV");
        assert_eq!(roman_numeral(9), "IX");
        assert_eq!(roman_numeral(42), "XLII");
    }
}
