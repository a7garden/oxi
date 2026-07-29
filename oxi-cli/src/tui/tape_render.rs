//! Projection of `AppState` into transcript plus pinned sticky tape rows.

use oxi_tui::{
    render::terminal::TerminalCapabilities,
    tape::{LiveRegion, TranscriptRenderer},
    theme::Theme,
    truncate_to_width,
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
        if !app.todo_panel.phases.is_empty() {
            self.rows
                .push(format!(" {} todos", app.todo_panel.phases.len()));
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
