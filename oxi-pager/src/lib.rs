// oxi-pager — pager state machine for the oxi-cli TUI.

// `deny` (not `forbid`) so the vendored `render::grok` module — copied from
// grok-build under Apache-2.0 — can locally `#![allow(unsafe_code)]` for its
// raw-fd/tty code. First-party oxi-pager code remains unsafe-free.
#![deny(unsafe_code)]

pub mod dispatch;
pub mod emitter;
pub mod keymap;
pub mod main_loop;
pub mod modal;
pub mod prompt;
pub mod reducer;
pub mod render;
pub mod scrollback;
pub mod slash;
pub mod state;
pub mod status;
pub mod theme_bridge;
pub mod widgets;

pub use emitter::{BackgroundEvent, PagerEvent, ResolvedKey};
pub use keymap::{FocusTarget, KeyRouter, ModalInput};
pub use main_loop::run;
pub use prompt::PromptState;
pub use reducer::{AgentCmd, ExitReason, ModalCtx, PagerAction, Sound, TermCmd, reduce};
pub use scrollback::ScrollbackState;
pub use state::{ModalKind, PagerState, SharedState, StickyPanelState};
pub use status::StatusState;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod render_smoke_tests {
    use super::*;
    use crate::scrollback::{BlockKind, RenderedBlock};
    use oxi_tui::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Smoke test: render() must not panic on a populated PagerState and
    /// must write recognizable content (user label, assistant body, prompt
    /// border) into the frame buffer. This is the smallest end-to-end
    /// exercise of the vendored grok render pipeline.
    #[test]
    fn render_populated_state_writes_user_and_assistant_blocks() {
        let mut state = PagerState::default();
        state.scrollback.blocks.extend([
            RenderedBlock {
                id: 0,
                kind: BlockKind::User,
                text: "hello, can you say hi back?".to_owned(),
            },
            RenderedBlock {
                id: 1,
                kind: BlockKind::Assistant,
                text: "**Hi!** Here is a `code` span and a [link](https://example.com).".to_owned(),
            },
        ]);
        state.prompt.text = "next message".to_owned();
        state.status.model = Some("smoke-model".to_owned());

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let theme = Theme::dark();
        terminal
            .draw(|f| render::render(f, &state, &theme))
            .expect("draw");

        // Inspect the buffer: look for our distinctive labels.
        let buf = terminal.backend().buffer();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("You"), "user label must be in buffer");
        assert!(
            content.contains("Assistant"),
            "assistant label must be in buffer"
        );
        assert!(
            content.contains("smoke-model"),
            "model name must be in token bar"
        );
        assert!(
            content.contains("Input"),
            "prompt border with 'Input' title must be rendered"
        );
    }

    /// Smoke test: render() handles empty scrollback gracefully (welcome line).
    #[test]
    fn render_empty_state_shows_welcome() {
        let state = PagerState::default();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let theme = Theme::dark();
        terminal
            .draw(|f| render::render(f, &state, &theme))
            .expect("draw");

        let buf = terminal.backend().buffer();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            content.contains("waiting"),
            "empty state must show the welcome/waiting line"
        );
    }
}
