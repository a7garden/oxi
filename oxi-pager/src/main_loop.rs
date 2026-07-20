//! Main event loop — terminal init, crossterm input, agent events → render.
//!
//! Uses `oxi-vendor-ratatui-inline` for flicker-free terminal output and
//! the grok-quality `render` module for TokyoNight-themed TUI output.

use crate::emitter::{BackgroundEvent, ResolvedKey};
use crate::render;
use crate::state::{PagerState, SharedState};
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEventKind};
use futures::StreamExt;
use parking_lot::RwLock;
use std::io::{self, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

pub async fn run<S: Send + 'static>(
    _session: S,
    mut background_rx: mpsc::UnboundedReceiver<BackgroundEvent>,
) -> anyhow::Result<()> {
    // ── Terminal init ────────────────────────────────────────────────
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide,
        crossterm::event::EnableMouseCapture,
    )?;
    // Use ratatui with crossterm backend wrapped in grok's inline terminal
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = oxi_vendor_ratatui_inline::Terminal::new(backend)?;
    let state: SharedState = Arc::new(RwLock::new(PagerState::default()));
    let theme = render::theme::Theme::default();
    let mut reader = EventStream::new();
    // ── Event loop ───────────────────────────────────────────────────
    let result: anyhow::Result<()> = loop {
        // Render current state
        terminal.draw(|frame| {
            let s = state.read();
            render::render(frame, &s, &theme);
        })?;

        // Wait for next event
        tokio::select! {
            Some(Ok(event)) = reader.next() => {
                match event {
                    CrosstermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        if handle_key(key.code, key.modifiers, &state) {
                            break Ok(());
                        }
                    }
                    CrosstermEvent::Resize(_, _) => {}
                    _ => {}
                }
            }
            Some(event) = background_rx.recv() => {
                apply_background_event(&state, event);
            }
            else => break Ok(()),
        }
    };

    // ── Terminal restore ─────────────────────────────────────────────
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
        crossterm::event::DisableMouseCapture,
    )?;

    result
}

/// Handle a key event. Returns `true` if the user requested quit.
fn handle_key(
    code: KeyCode,
    modifiers: crossterm::event::KeyModifiers,
    state: &SharedState,
) -> bool {
    let mut s = state.write();
    match code {
        KeyCode::Char('c') if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            return true;
        }
        KeyCode::Esc => return true,

        KeyCode::Enter => {
            if !s.prompt.text.is_empty() {
                let text = std::mem::take(&mut s.prompt.text);
                let id = s.scrollback.next_id;
                s.scrollback.next_id += 1;
                s.scrollback.blocks.push(crate::scrollback::RenderedBlock {
                    id,
                    kind: crate::scrollback::BlockKind::User,
                    text,
                    lines: Vec::new(),
                });
                s.prompt.cursor = 0;
            }
        }

        KeyCode::Left => { if s.prompt.cursor > 0 { s.prompt.cursor -= 1; } }
        KeyCode::Right => { if s.prompt.cursor < s.prompt.text.len() { s.prompt.cursor += 1; } }

        KeyCode::Backspace => {
            if s.prompt.cursor > 0 {
                s.prompt.cursor -= 1;
                let pos = s.prompt.cursor;
                s.prompt.text.remove(pos);
            }
        }
        KeyCode::Delete => {
            if s.prompt.cursor < s.prompt.text.len() {
                let pos = s.prompt.cursor;
                s.prompt.text.remove(pos);
            }
        }

        KeyCode::Home => s.prompt.cursor = 0,
        KeyCode::End => s.prompt.cursor = s.prompt.text.len(),

        KeyCode::Char(ch) => {
            let pos = s.prompt.cursor;
            s.prompt.text.insert(pos, ch);
            s.prompt.cursor = pos + 1;
        }

        KeyCode::PageUp => { s.scrollback.scroll_offset = s.scrollback.scroll_offset.saturating_add(10); }
        KeyCode::PageDown => { s.scrollback.scroll_offset = s.scrollback.scroll_offset.saturating_sub(10); }
        KeyCode::Up => { s.scrollback.scroll_offset = s.scrollback.scroll_offset.saturating_add(1); }
        KeyCode::Down => { s.scrollback.scroll_offset = s.scrollback.scroll_offset.saturating_sub(1); }

        _ => {}
    }
    false
}

/// Apply a background event to the shared state.
fn apply_background_event(state: &SharedState, event: BackgroundEvent) {
    let mut s = state.write();
    match event {
        BackgroundEvent::AssistantDelta(text) => {
            if s.scrollback.blocks.is_empty()
                || s.scrollback.blocks.last().map(|b| &b.kind)
                    != Some(&crate::scrollback::BlockKind::Assistant)
            {
                s.scrollback.begin_assistant();
            }
            s.scrollback.append_token(&text);
        }
        BackgroundEvent::AssistantDone => {
            s.scrollback.end_assistant();
        }
        BackgroundEvent::ToolCall { id, name, params: _ } => {
            s.scrollback.begin_tool_call(&name, &id);
        }
        BackgroundEvent::ToolResult { id, content: _ } => {
            s.scrollback.end_tool_call(&id);
        }
        BackgroundEvent::UserMessage(text) => {
            let id = s.scrollback.next_id;
            s.scrollback.next_id += 1;
            s.scrollback.blocks.push(crate::scrollback::RenderedBlock {
                id,
                kind: crate::scrollback::BlockKind::User,
                text,
                lines: Vec::new(),
            });
        }
        BackgroundEvent::StatusUpdate(status) => {
            s.status = status;
        }
        BackgroundEvent::StreamDone => {}
    }
}
