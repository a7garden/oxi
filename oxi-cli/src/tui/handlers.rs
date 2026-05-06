//! Event handlers for the TUI.

use super::app::{AppState, UiEvent};
use super::slash;
use crate::agent_session::{AgentSession, CompactionReason, SessionEvent};
use oxi_agent::AgentEvent;
use tokio::sync::mpsc;

use crossterm::event::{Event as CEvent, KeyCode, KeyModifiers, MouseEventKind};

/// Actions returned from input handling that need async work in the main loop.
pub(crate) enum Action {
    SendPrompt(String),
    None,
}

/// Handle a crossterm input event. Returns an action if the main loop needs to do async work.
pub async fn handle_input(
    event: CEvent,
    state: &mut AppState,
    session: &AgentSession,
    ui_tx: &mpsc::Sender<UiEvent>,
    _prompt_tx: &mpsc::Sender<String>,
    running: &mut bool,
) -> Option<Action> {
    match event {
        CEvent::Key(key) => handle_key(key, state, session, ui_tx, running).await,
        CEvent::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => state.scroll_up(3),
                MouseEventKind::ScrollDown => state.scroll_down(3),
                _ => {}
            }
            None
        }
        _ => None,
    }
}

async fn handle_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    session: &AgentSession,
    ui_tx: &mpsc::Sender<UiEvent>,
    running: &mut bool,
) -> Option<Action> {
    match key.code {
        KeyCode::Enter => {
            if !state.is_agent_busy {
                if state.slash_completion_active {
                    state.accept_slash_completion();
                    return None;
                }
                let value = state.input_value().to_string();
                if !value.is_empty() {
                    if value.starts_with('/') {
                        let handled = slash::handle_slash_command(
                            &value, session, state, running,
                        );
                        state.input_clear();
                        if handled {
                            return None;
                        }
                    }
                    return Some(Action::SendPrompt(value));
                }
            }
            None
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.is_agent_busy {
                let sh = session.clone_handle();
                tokio::spawn(async move { sh.abort().await });
                state.cancel_streaming();
                state.add_system_message("⏹ Interrupted".to_string());
            } else {
                *running = false;
            }
            None
        }
        KeyCode::PageUp => {
            state.scroll_up(10);
            None
        }
        KeyCode::PageDown => {
            state.scroll_down(10);
            None
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if !state.is_agent_busy {
                state.input.insert_char(c);
                state.update_slash_completions();
            }
            None
        }
        KeyCode::Backspace => {
            if !state.is_agent_busy {
                state.input.backspace();
                state.update_slash_completions();
            }
            None
        }
        KeyCode::Delete => {
            if !state.is_agent_busy {
                state.input.delete();
                state.update_slash_completions();
            }
            None
        }
        KeyCode::Left => {
            if !state.is_agent_busy {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    let text: Vec<char> = state.input.text.chars().collect();
                    let mut pos = state.input.cursor;
                    while pos > 0 && text[pos - 1].is_whitespace() {
                        pos -= 1;
                    }
                    while pos > 0 && !text[pos - 1].is_whitespace() {
                        pos -= 1;
                    }
                    state.input.cursor = pos;
                } else {
                    state.input.move_left();
                }
            }
            None
        }
        KeyCode::Right => {
            if !state.is_agent_busy {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    let text: Vec<char> = state.input.text.chars().collect();
                    let mut pos = state.input.cursor;
                    while pos < text.len() && !text[pos].is_whitespace() {
                        pos += 1;
                    }
                    while pos < text.len() && text[pos].is_whitespace() {
                        pos += 1;
                    }
                    state.input.cursor = pos;
                } else {
                    state.input.move_right();
                }
            }
            None
        }
        KeyCode::Home => {
            if !state.is_agent_busy {
                state.input.move_home();
            }
            None
        }
        KeyCode::End => {
            if !state.is_agent_busy {
                state.input.move_end();
            }
            None
        }
        KeyCode::Tab => {
            if !state.is_agent_busy && state.slash_completion_active {
                state.accept_slash_completion();
            }
            None
        }
        KeyCode::Up => {
            if !state.is_agent_busy && state.slash_completion_active {
                state.prev_slash_completion();
            } else if !state.is_agent_busy
                && state.input.text.is_empty()
                && !state.input_history.is_empty()
            {
                if state.history_index == 0 {
                    state.saved_input = state.input.text.clone();
                }
                if state.history_index < state.input_history.len() {
                    state.history_index += 1;
                    state.input_set_text(
                        state.input_history[state.history_index - 1].clone(),
                    );
                    state.clear_slash_completions();
                }
            } else {
                state.scroll_up(3);
            }
            None
        }
        KeyCode::Down => {
            if !state.is_agent_busy && state.slash_completion_active {
                state.next_slash_completion();
            } else if !state.is_agent_busy && state.history_index > 0 {
                state.history_index -= 1;
                if state.history_index == 0 {
                    state.input_set_text(state.saved_input.clone());
                } else {
                    state.input_set_text(
                        state.input_history[state.history_index - 1].clone(),
                    );
                }
                state.clear_slash_completions();
            } else {
                state.scroll_down(3);
            }
            None
        }
        KeyCode::Esc => {
            if state.slash_completion_active {
                state.clear_slash_completions();
            }
            None
        }
        _ => None,
    }
}

/// Handle an agent UI event.
pub fn handle_ui_event(event: UiEvent, state: &mut AppState) {
    match event {
        UiEvent::Start | UiEvent::Thinking => {}
        UiEvent::TextDelta(text) => {
            state.stream_text_delta(&text);
        }
        UiEvent::ToolCall { name, .. } => {
            state.stream_text_delta(&format!("\n⚙ {}\n", name));
        }
        UiEvent::ToolStart { tool_name } => {
            state.stream_text_delta(&format!("\n▶ {}...\n", tool_name));
        }
        UiEvent::ToolResult {
            tool_name,
            content,
            is_error,
        } => {
            let label = if tool_name.is_empty() {
                "tool"
            } else {
                &tool_name
            };
            if is_error {
                let preview: String = content.chars().take(200).collect();
                state.stream_text_delta(&format!("  ✗ {}: {}\n", label, preview));
            } else {
                let preview: String = content.lines().take(3).collect::<Vec<_>>().join("\n  ");
                if !preview.is_empty() {
                    state.stream_text_delta(&format!("  ✓ {}\n", preview));
                }
            }
        }
        UiEvent::Complete => {
            state.finish_streaming();
        }
        UiEvent::Error(msg) => {
            state.cancel_streaming();
            state.add_system_message(format!("Error: {}", msg));
        }
        UiEvent::CompactionStart { reason } => {
            let reason_str = match reason {
                CompactionReason::Manual => "manual",
                CompactionReason::Threshold => "auto",
                CompactionReason::Overflow => "overflow",
            };
            state.add_system_message(format!("📦 Compacting ({})...", reason_str));
        }
        UiEvent::CompactionEnd {
            _reason,
            error_message,
        } => {
            let msg = if let Some(err) = error_message {
                format!("⚠ Compaction failed: {}", err)
            } else {
                "✅ Compaction complete".to_string()
            };
            state.add_system_message(msg);
        }
        UiEvent::RetryStart {
            attempt,
            max_attempts,
            error_message,
        } => {
            state.add_system_message(format!(
                "🔄 Retry ({}/{}): {}",
                attempt, max_attempts, error_message
            ));
        }
        UiEvent::ModelChanged { model_id } => {
            state.add_system_message(format!("🤖 → {}", model_id));
            state.footer_state.data.model_name = model_id;
        }
        UiEvent::ThinkingLevelChanged { level } => {
            state.add_system_message(format!("💭 Thinking: {}", level));
        }
        UiEvent::QueueUpdate { pending } => {
            if pending > 0 {
                tracing::debug!("Queue: {} pending", pending);
            }
        }
    }
}

/// Handle a session event, forwarding relevant ones as UI events.
pub async fn handle_session_event(
    event: SessionEvent,
    ui_tx: &mpsc::Sender<UiEvent>,
) {
    match event {
        SessionEvent::CompactionStart { reason } => {
            let _ = ui_tx.send(UiEvent::CompactionStart { reason }).await;
        }
        SessionEvent::CompactionEnd {
            reason, error_message, ..
        } => {
            let _ = ui_tx
                .send(UiEvent::CompactionEnd {
                    _reason: reason,
                    error_message,
                })
                .await;
        }
        SessionEvent::ThinkingLevelChanged { level } => {
            let _ = ui_tx
                .send(UiEvent::ThinkingLevelChanged {
                    level: format!("{:?}", level),
                })
                .await;
        }
        SessionEvent::QueueUpdate { steering, follow_up } => {
            let pending = steering.len() + follow_up.len();
            let _ = ui_tx.send(UiEvent::QueueUpdate { pending }).await;
        }
        SessionEvent::SessionInfoChanged { name: _ } => {}
        SessionEvent::Agent(agent_event) => match &agent_event {
            AgentEvent::Fallback { to_model, .. } => {
                let _ = ui_tx
                    .send(UiEvent::ModelChanged {
                        model_id: to_model.clone(),
                    })
                    .await;
            }
            AgentEvent::Retry {
                attempt,
                max_retries,
                reason,
                ..
            } => {
                let _ = ui_tx
                    .send(UiEvent::RetryStart {
                        attempt: *attempt as u32,
                        max_attempts: *max_retries as u32,
                        error_message: reason.clone(),
                    })
                    .await;
            }
            AgentEvent::Compaction { .. } => {}
            _ => {}
        },
    }
}
