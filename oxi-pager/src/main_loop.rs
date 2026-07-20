//! Main event loop — crossterm input + agent background events → render.

use crate::emitter::{BackgroundEvent, PagerEvent, ResolvedKey};
use crate::reducer::reduce;
use crate::state::{PagerState, SharedState};
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::mpsc;

pub async fn run<S: Send + 'static>(
    _session: S,
    mut background_rx: mpsc::UnboundedReceiver<BackgroundEvent>,
) -> anyhow::Result<()> {
    let state: SharedState = Arc::new(RwLock::new(PagerState::default()));
    let mut reader = EventStream::new();

    loop {
        tokio::select! {
            Some(Ok(event)) = reader.next() => {
                match event {
                    CrosstermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        let _resolved = ResolvedKey::from(key);
                        // reduce(&state, PagerEvent::Input(resolved));
                    }
                    CrosstermEvent::Resize(_, _) => {}
                    _ => {}
                }
            }
            Some(event) = background_rx.recv() => {
                apply_background_event(&state, event);
            }
            else => break,
        }
    }

    Ok(())
}

fn apply_background_event(state: &SharedState, event: BackgroundEvent) {
    let mut s = state.write();
    match event {
        BackgroundEvent::AssistantDelta(text) => {
            if s.scrollback.blocks.is_empty()
                || s.scrollback.blocks.last().map(|b| &b.kind) != Some(&crate::scrollback::BlockKind::Assistant)
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
