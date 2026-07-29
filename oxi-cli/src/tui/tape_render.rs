//! Projection of `AppState` into transcript plus pinned sticky tape rows.

use std::cmp::min;

use oxi_tui::{
    render::terminal::TerminalCapabilities,
    tape::{LiveRegion, TranscriptRenderer},
    theme::Theme,
    truncate_to_width,
    widgets::todo_panel::{TodoPanelState, TodoPanelStatus},
};

use super::app::AppState;

pub(crate) struct TapeRenderState {
    transcript: TranscriptRenderer,
    rows: Vec<String>,
    live: LiveRegion,
}

impl TapeRenderState {
    pub(crate) fn new() -> Self {
        Self {
            transcript: TranscriptRenderer::new(),
            rows: Vec::new(),
            live: LiveRegion::Pinned { start: 0 },
        }
    }

    pub(crate) fn sync(
        &mut self,
        app: &AppState,
        theme: &Theme,
        caps: &TerminalCapabilities,
        width: u16,
    ) {
        let content_width = width.saturating_sub(2).max(1);
        self.transcript
            .sync(&app.chat.messages, app.chat.streaming.as_ref(), theme, caps);
        let (transcript, transcript_live) = self.transcript.compose(content_width);
        self.rows.clear();
        self.rows
            .extend(transcript.lines.iter().map(|line| format!(" {line}")));
        let sticky_start = self.rows.len();

        if !app.steering_messages_snapshot.is_empty() {
            self.rows
                .push(format!(" queued: {}", app.steering_messages_snapshot.len()));
        }
        if !app.todo_panel.is_empty() {
            let todo_lines = render_todo_lines(&app.todo_panel, content_width);
            self.rows.extend(todo_lines);
        }
        let status = if app.is_agent_busy {
            "working"
        } else {
            "ready"
        };
        self.rows.push(format!(" {status}"));
        let input = app.input.text();
        if input.is_empty() {
            self.rows.push(" > ".into());
        } else {
            self.rows.extend(
                input.lines().map(|line| {
                    format!(" > {}", truncate_to_width(line, content_width as usize - 2))
                }),
            );
        }
        if app.slash_completion_active {
            self.rows.extend(
                app.slash_completions
                    .iter()
                    .take(6)
                    .map(|item| format!("   /{} — {}", item.name, item.description)),
            );
        }
        if app.file_completion_active {
            self.rows.extend(
                app.file_completions
                    .iter()
                    .take(6)
                    .map(|item| format!("   {}", item.label)),
            );
        }
        self.rows
            .push(format!(" {}", app.footer_state.data.model_name));
        self.rows
            .extend(app.notifications.iter().map(|n| format!(" {}", n.message)));

        self.live = match transcript_live {
            LiveRegion::Mutable { start } => LiveRegion::Mutable { start },
            LiveRegion::Pinned { start } => LiveRegion::Pinned { start },
            LiveRegion::None => LiveRegion::Pinned {
                start: sticky_start,
            },
        };
    }

    pub(crate) fn frame(&self) -> (&[String], LiveRegion) {
        (&self.rows, self.live)
    }
}

/// Render the todo panel into plain-text sticky lines (plain-text pipeline).
///
/// Uses ASCII status markers instead of styled text, consistent with
/// the tape_render string pipeline (no ratatui Buffer support).
fn render_todo_lines(panel: &TodoPanelState, width: u16) -> Vec<String> {
    let _ = width; // used for future truncation
    if panel.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let indent = "  ";

    // Header: "Todos"
    lines.push(" ".to_string());
    lines.push(" Todos".to_string());

    let non_empty: Vec<_> = panel
        .phases
        .iter()
        .filter(|p| !p.tasks.is_empty())
        .collect();

    if panel.expanded {
        // Expanded mode: all phases with headers
        for phase in &non_empty {
            if non_empty.len() > 1 {
                lines.push(format!("{indent}{}:", phase.name));
            }
            for task in &phase.tasks {
                let marker = match task.status {
                    TodoPanelStatus::Pending => "[ ]",
                    TodoPanelStatus::InProgress => "[@]",
                    TodoPanelStatus::Completed => "[x]",
                    TodoPanelStatus::Abandoned => "[-]",
                };
                lines.push(format!("{indent}  {marker} {}", task.content));
            }
        }
    } else {
        // Collapsed mode: active phase only
        if let Some(phase) = panel.active_phase() {
            let visible = min(phase.tasks.len(), panel.max_visible);
            let hidden = phase.tasks.len().saturating_sub(panel.max_visible);
            for task in &phase.tasks[..visible] {
                let marker = match task.status {
                    TodoPanelStatus::Pending => "[ ]",
                    TodoPanelStatus::InProgress => "[@]",
                    TodoPanelStatus::Completed => "[x]",
                    TodoPanelStatus::Abandoned => "[-]",
                };
                lines.push(format!("{indent}{marker} {}", task.content));
            }
            if hidden > 0 {
                lines.push(format!("{indent}  ... {} more", hidden));
            }
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::AppState;

    #[test]
    fn sticky_rows_are_pinned_without_stream() {
        let mut app = AppState::new();
        app.add_user_message("hello".into());
        let mut tape = TapeRenderState::new();
        tape.sync(&app, &Theme::dark(), &TerminalCapabilities::default(), 80);
        let (rows, live) = tape.frame();
        assert!(rows.iter().any(|row| row.contains("hello")));
        assert!(matches!(live, LiveRegion::Pinned { start } if start > 0));
    }
}
