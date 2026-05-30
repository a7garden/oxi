//! Event handlers for the TUI.

use super::app::{AppOverlay, AppState, NotificationKind, ProviderInfo, SetupStep, UiEvent};
use super::overlay::router_integration;
use super::slash;
use crate::app::agent_session::{AgentSession, SessionEvent};
use crate::context::auto_compaction::CompactionReason;
use crate::media::clipboard_write;
use base64::Engine;
use oxi_agent::AgentEvent;
use oxi_tui::widgets::chat::ToolCallStatus;
use tokio::sync::mpsc;

use crossterm::event::{Event as CEvent, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

/// Actions returned from input handling that need async work in the main loop.
pub(crate) enum Action {
    SendPrompt(String),
    ExecuteSlashCommand(String),
}

/// Remove a message at the given index from the steering queue.
/// Returns the removed message text, or None if index was out of bounds.
fn remove_from_steering_queue(session: &AgentSession, index: usize) -> Option<String> {
    let queue = session.steering_queue();
    let mut guard = queue.write();
    if index < guard.len() {
        guard.remove(index)
    } else {
        None
    }
}

/// Handle a crossterm input event. Returns an action if the main loop needs to do async work.
pub async fn handle_input(
    event: CEvent,
    state: &mut AppState,
    session: &AgentSession,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    _prompt_tx: &mpsc::Sender<String>,
    running: &mut bool,
) -> Option<Action> {
    match event {
        CEvent::Key(key) => {
            if state.overlay.is_some() || state.overlay_state.is_some() {
                handle_overlay_key(key, state, session).await
            } else {
                handle_key(key, state, session, ui_tx, running).await
            }
        }
        CEvent::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => state.scroll_up(3),
                MouseEventKind::ScrollDown => state.scroll_down(3),
                MouseEventKind::Up(button) => {
                    use crossterm::event::MouseButton;
                    if button == MouseButton::Left {
                        handle_click(mouse.column, mouse.row, state);
                    }
                }
                _ => {}
            }
            None
        }
        // Handle IME composition completion or clipboard paste
        CEvent::Paste(text) => {
            if state.overlay.is_some() || state.overlay_state.is_some() {
                // When overlay is active, forward Paste to the overlay handler
                handle_overlay_paste(&text, state)
            } else {
                state.input.insert_str(&text);
                state.update_slash_completions();
                update_file_completions(state);
                None
            }
        }
        _ => None,
    }
}

async fn handle_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    session: &AgentSession,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    running: &mut bool,
) -> Option<Action> {
    // Only handle Press events when the keyboard event type is supported
    // (Ignore Repeat/Release — prevents Repeat events during IME composition)
    if key.kind != KeyEventKind::Press {
        return None;
    }

    // When queue panel is visible and has items, intercept navigation keys
    if state.queue_panel_visible && !state.steering_messages_snapshot.is_empty() {
        if let Some(action) = handle_queue_panel_key(key, state, session) {
            return action;
        }
    }

    use oxi_tui::keybindings::keys::KeyId;
    let key_id = KeyId::from(key);

    // Try keybinding lookup first
    if let Some(action) = state.keybindings.match_action(&key_id) {
        return dispatch_action(action, key, state, session, ui_tx, running).await;
    }

    // Fallback: printable character (no binding match)
    if !key_id.ctrl && !key_id.alt && !key_id.super_ {
        if let oxi_tui::keybindings::keys::BaseKey::Char(c) = key_id.base {
            state.input.insert_char(c);
            state.update_slash_completions();
            update_file_completions(state);
        }
    }
    None
}

/// Handle key events when the queue panel is visible.
/// Returns Some(action) if the key was consumed, None to fall through.
fn handle_queue_panel_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    session: &AgentSession,
) -> Option<Option<Action>> {
    match key.code {
        KeyCode::Up if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.queue_panel_selected > 0 {
                state.queue_panel_selected -= 1;
            }
            Some(None)
        }
        KeyCode::Down if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.queue_panel_selected < state.steering_messages_snapshot.len() - 1 {
                state.queue_panel_selected += 1;
            }
            Some(None)
        }
        KeyCode::Delete | KeyCode::Char('d') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = state.queue_panel_selected;
            let removed = remove_from_steering_queue(session, idx);
            if let Some(msg) = removed {
                let preview: String = msg.chars().take(40).collect();
                state.add_notification(format!("Removed: {}", preview), NotificationKind::Info);
            }
            refresh_queue_snapshot(state, session);
            Some(None)
        }
        KeyCode::Char('e') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = state.queue_panel_selected;
            let removed = remove_from_steering_queue(session, idx);
            if let Some(msg) = removed {
                state.input_set_text(msg);
            }
            refresh_queue_snapshot(state, session);
            state.queue_panel_selected = state.steering_messages_snapshot.len().saturating_sub(1);
            state.queue_panel_visible = false;
            Some(None)
        }
        KeyCode::Esc => {
            state.queue_panel_visible = false;
            Some(None)
        }
        _ => None, // Fall through to normal keybinding handling
    }
}

/// Refresh the queue panel snapshot from the session.
fn refresh_queue_snapshot(state: &mut AppState, session: &AgentSession) {
    let msgs = session.steering_messages();
    let fq = session.follow_up_messages();
    let pending = msgs.len() + fq.len();
    let mut all = msgs;
    all.extend(fq);
    state.pending_steering = pending;
    state.steering_messages_snapshot = all;
    if state.queue_panel_selected >= state.steering_messages_snapshot.len() {
        state.queue_panel_selected = state.steering_messages_snapshot.len().saturating_sub(1);
    }
}

/// Dispatch a resolved keybinding action.
async fn dispatch_action(
    action: oxi_tui::keybindings::registry::Action,
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    session: &AgentSession,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    running: &mut bool,
) -> Option<Action> {
    use oxi_tui::keybindings::registry::Action as KAction;

    match action {
        // ── Submit ────────────────────────────────────────────
        KAction::Submit => handle_submit(state, session, running, ui_tx).await,

        // ── Quit ──────────────────────────────────────────────
        KAction::Quit => {
            tracing::debug!("[TUI-Handler] Ctrl+C setting running = false");
            *running = false;
            session.agent_ref().cancel();
            session.abort_compaction_sync();
            tracing::debug!("[TUI-Handler] Ctrl+C done, running = {}", *running);
            None
        }

        // ── Cancel ────────────────────────────────────────────
        KAction::Cancel => {
            // Compaction cancel takes priority
            if state.footer_state.data.is_compacting {
                session.abort_compaction_sync();
                state.footer_state.data.is_compacting = false;
                state.add_notification("Compaction cancelled".to_string(), NotificationKind::Info);
            } else if state.slash_completion_active {
                state.clear_slash_completions();
            }
            None
        }

        // ── Editor navigation ─────────────────────────────────
        KAction::CursorLeft => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.input.move_word_left();
            } else {
                state.input.move_left();
            }
            None
        }
        KAction::CursorRight => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.input.move_word_right();
            } else {
                state.input.move_right();
            }
            None
        }
        KAction::CursorWordLeft => {
            state.input.move_word_left();
            None
        }
        KAction::CursorWordRight => {
            state.input.move_word_right();
            None
        }
        KAction::CursorLineStart => {
            state.input.move_home();
            None
        }
        KAction::CursorLineEnd => {
            state.input.move_end();
            None
        }

        // ── Editor editing ────────────────────────────────────
        KAction::DeleteCharBackward => {
            state.input.backspace();
            state.update_slash_completions();
            update_file_completions(state);
            None
        }
        KAction::DeleteCharForward => {
            state.input.delete();
            state.update_slash_completions();
            update_file_completions(state);
            None
        }
        KAction::DeleteToLineStart => {
            state.input.delete_to_line_start();
            state.update_slash_completions();
            update_file_completions(state);
            None
        }
        KAction::DeleteToLineEnd => {
            state.input.delete_to_line_end();
            state.update_slash_completions();
            update_file_completions(state);
            None
        }
        KAction::Undo => {
            state.input.undo();
            None
        }

        // ── Tab / Completion ──────────────────────────────────
        KAction::Tab => {
            if state.slash_completion_active {
                let cmd = state.selected_slash_command().map(|c| c.name.clone());
                state.clear_slash_completions();
                state.input_clear();
                if let Some(cmd) = cmd {
                    return Some(Action::ExecuteSlashCommand(cmd));
                }
            }
            None
        }

        // ── Cycle thinking ────────────────────────────────────
        KAction::CycleThinking => {
            if !state.slash_completion_active {
                if let Some(next_level) = session.cycle_thinking_level() {
                    state.footer_state.data.thinking_level =
                        Some(format!("{:?}", next_level).to_lowercase());
                    state.add_notification(
                        format!("Thinking: {:?}", next_level),
                        NotificationKind::Info,
                    );
                }
            }
            None
        }

        // ── View scrolling ────────────────────────────────────
        KAction::ScrollUp => {
            // Conditional: if input empty + has history → history up, else scroll
            if state.slash_completion_active {
                state.prev_slash_completion();
            } else if state.input.text().is_empty() && !state.input_history.is_empty() {
                navigate_history_up(state);
            } else {
                state.scroll_up(3);
            }
            None
        }
        KAction::ScrollDown => {
            if state.slash_completion_active {
                state.next_slash_completion();
            } else if state.history_index > 0 {
                navigate_history_down(state);
            } else {
                state.scroll_down(3);
            }
            None
        }
        KAction::ScrollPageUp => {
            state.scroll_up(10);
            None
        }
        KAction::ScrollPageDown => {
            state.scroll_down(10);
            None
        }

        // ── App actions ───────────────────────────────────────
        KAction::OpenImage => {
            open_last_image(state);
            None
        }
        KAction::ToggleRouting => {
            state.overlay_state = None;
            let snap = oxi_ai::router::RouterProvider::get_snapshot();
            let data = if let Some(ref s) = snap {
                use oxi_tui::widgets::routing::{ProviderHealth, ProviderInfo, RoutingStatusData};
                let chain = vec![ProviderInfo {
                    name: s.last_provider.clone().unwrap_or_default(),
                    health: ProviderHealth::Healthy,
                    failures: 0,
                    is_active: true,
                }];
                RoutingStatusData {
                    auto_routing_enabled: true,
                    fallback_enabled: true,
                    fallback_chain: chain,
                    active_index: 0,
                }
            } else {
                oxi_tui::widgets::routing::RoutingStatusData::default()
            };
            state.overlay_state = Some(super::overlay::factories::routing_status(data));
            None
        }
        KAction::ToggleQueue => {
            state.queue_panel_visible = !state.queue_panel_visible;
            if state.queue_panel_visible {
                state.queue_panel_selected =
                    state.steering_messages_snapshot.len().saturating_sub(1);
            }
            None
        }
        KAction::CopyCodeBlock => {
            if let Some(ref code) = state.chat.last_code_block {
                match clipboard_write::copy_to_clipboard(code) {
                    Ok(()) => state.add_notification(
                        "Code block copied".to_string(),
                        NotificationKind::Success,
                    ),
                    Err(e) => state
                        .add_notification(format!("Copy failed: {}", e), NotificationKind::Error),
                }
            } else {
                state.add_notification(
                    "No code block to copy".to_string(),
                    NotificationKind::Warning,
                );
            }
            None
        }

        // ── Actions not bound in main input mode ──────────────
        KAction::DeleteWordBackward => {
            state.input.delete_word_backward();
            state.update_slash_completions();
            update_file_completions(state);
            None
        }
        KAction::DeleteWordForward => {
            state.input.delete_word_forward();
            state.update_slash_completions();
            update_file_completions(state);
            None
        }
        KAction::OpenModelSelect
        | KAction::OpenProviderSetup
        | KAction::NewLine
        | KAction::HistoryUp
        | KAction::HistoryDown
        | KAction::CompletionNext
        | KAction::CompletionPrev
        | KAction::CompletionDismiss
        | KAction::CompletionAccept => None,
    }
}

/// Handle the Submit action (Enter key).
async fn handle_submit(
    state: &mut AppState,
    session: &AgentSession,
    running: &mut bool,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
) -> Option<Action> {
    let value = state.input_value().to_string();
    if value.is_empty() {
        return None;
    }

    // Slash command popup
    if state.slash_completion_active {
        let cmd = state.selected_slash_command().map(|c| c.name.clone());
        state.clear_slash_completions();
        state.input_clear();
        if let Some(cmd) = cmd {
            return Some(Action::ExecuteSlashCommand(cmd));
        }
        return None;
    }

    // Slash command in input
    if value.starts_with('/') {
        let handled = slash::handle_slash_command(&value, session, state, running, ui_tx);
        state.input_clear();
        if handled {
            return None;
        }
    }

    if state.is_agent_busy {
        // Agent busy — queue as steering message
        state.add_notification(
            format!("Queued: {}", value.chars().take(50).collect::<String>()),
            NotificationKind::Info,
        );
        state.input_history.insert(0, value.clone());
        if state.input_history.len() > 100 {
            state.input_history.remove(0);
        }
        state.history_index = 0;
        session.steer_sync(value.clone());
        state.steering_messages_snapshot.push(value);
        state.pending_steering = state.steering_messages_snapshot.len();
        state.input_clear();
        return None;
    }

    // Not busy — send directly
    Some(Action::SendPrompt(value))
}

/// Navigate to previous history entry.
fn navigate_history_up(state: &mut AppState) {
    if state.history_index == 0 {
        state.saved_input = state.input.text();
    }
    if state.history_index < state.input_history.len() {
        state.history_index += 1;
        state.input_set_text(state.input_history[state.history_index - 1].clone());
        state.clear_slash_completions();
    }
}

/// Navigate to next history entry.
fn navigate_history_down(state: &mut AppState) {
    state.history_index -= 1;
    if state.history_index == 0 {
        state.input_set_text(state.saved_input.clone());
    } else {
        state.input_set_text(state.input_history[state.history_index - 1].clone());
    }
    state.clear_slash_completions();
}

/// Handle an agent UI event.
/// pi-mono pattern: MessageUpdate drives rendering (not TextChunk deltas).
pub fn handle_ui_event(event: UiEvent, state: &mut AppState) {
    match event {
        // ── Agent lifecycle ───────────────────────────────────────
        UiEvent::AgentStart => {
            // Agent started processing
        }
        UiEvent::AgentEnd => {
            // Agent finished all processing.
            // Always clear busy state — AutoProcessStart will re-set it
            // if a queued message is being auto-processed.
            state.is_agent_busy = false;
            // Persist any remaining messages after the agent run completes
            // and state has been synced back from the agent loop.
            state.needs_persist = true;
        }

        // ── Turn lifecycle ────────────────────────────────────────
        UiEvent::TurnStart { .. } => {
            // New turn began
        }
        UiEvent::TurnEnd { .. } => {
            // Turn completed
        }

        // ── Message lifecycle (pi-mono pattern) ───────────────────
        // These are the primary rendering events.
        UiEvent::MessageStart { message } => {
            // pi-mono: message_start — begin streaming
            let auto_committed = state.chat.start_streaming();
            if auto_committed {
                state.message_count += 1;
                state.chat.refresh_last_code_block();
            }
            state.is_agent_busy = true;
            state.auto_scroll = true;
            // Reset snapshot tracking counters for the new message
            state.reset_snapshot_tracking();
            // Apply initial snapshot (delta = None for first message)
            state.update_streaming_message(&message, None);
        }
        UiEvent::MessageUpdate { message, delta } => {
            // pi-mono: message_update — full snapshot with separated content blocks.
            // The provider has already split text vs toolCall vs thinking.
            state.update_streaming_message(&message, delta.as_deref());
        }
        UiEvent::MessageEnd { message } => {
            // pi-mono: message_end — finalize the message
            state.finalize_streaming_message(&message);

            // Persist session data (pi-mono: persist on every message_end)
            state.needs_persist = true;

            // Finalize moves the message to the permanent list
            let was_streaming = state.chat.is_streaming();
            state.chat.finish_streaming();
            // NOTE: Do NOT set is_agent_busy = false here.
            // The agent may still be executing tools and starting another
            // turn. is_agent_busy is cleared on TurnEnd (via Complete)
            // or when the user cancels.
            if was_streaming {
                state.message_count += 1;
                state.chat.refresh_last_code_block();
            }
        }

        // ── Tool execution ────────────────────────────────────────
        UiEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            tracing::debug!(
                "[HANDLER] ToolExecutionStart: id={:?}, name={:?}",
                tool_call_id,
                tool_name
            );
            // Track start time for duration measurement
            state
                .tool_start_times
                .insert(tool_call_id.clone(), std::time::Instant::now());
            let args_str = serde_json::to_string(&args).unwrap_or_else(|_| args.to_string());
            state.chat.stream_tool_call(
                tool_call_id,
                tool_name,
                args_str,
                ToolCallStatus::Executing,
            );
        }
        UiEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => {
            tracing::debug!(
                "[HANDLER] ToolExecutionEnd: id={:?}, name={:?}",
                tool_call_id,
                tool_name
            );
            // Compute and store execution duration
            let duration = state.tool_start_times.remove(&tool_call_id).map(|t| {
                let elapsed = t.elapsed();
                if elapsed.as_secs() >= 60 {
                    format!("{}m{}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
                } else if elapsed.as_millis() >= 1000 {
                    format!("{:.1}s", elapsed.as_secs_f64())
                } else {
                    format!("{}ms", elapsed.as_millis())
                }
            });
            // Pass full result content (clamped by stream_tool_result internally)
            let content = result.content.clone();
            state.chat.stream_tool_result(
                Some(tool_call_id.clone()),
                tool_name,
                content,
                is_error || result.status == "error",
            );
            if let Some(dur) = duration {
                state.chat.set_tool_duration(&tool_call_id, dur);
            }
        }

        // ── Legacy events ─────────────────────────────────────────
        UiEvent::Thinking => {
            // Agent is waiting for first token
        }
        UiEvent::ThinkingDelta(text) => {
            // Thinking text — still useful for showing reasoning
            state.chat.stream_thinking(text, true);
        }
        UiEvent::Error(msg) => {
            state.cancel_streaming();
            state.add_notification(format!("Error: {}", msg), NotificationKind::Error);
        }

        // ── Session events ────────────────────────────────────────
        UiEvent::CompactionStart { reason: _reason } => {
            state.footer_state.data.is_compacting = true;
            let label = match _reason {
                CompactionReason::Manual => "Compacting context...",
                CompactionReason::Threshold | CompactionReason::Automatic => "Auto-compacting...",
                CompactionReason::Overflow => "Context overflow, compacting...",
                CompactionReason::Iteration { .. } => "Auto-compacting (iteration)...",
            };
            state.add_notification(format!("{} (Esc to cancel)", label), NotificationKind::Info);
        }
        UiEvent::CompactionEnd {
            _reason,
            error_message,
        } => {
            state.footer_state.data.is_compacting = false;
            if let Some(err) = error_message {
                state.add_notification(
                    format!("Compaction failed: {}", err),
                    NotificationKind::Error,
                );
            } else {
                // Compaction succeeded — flag chat rebuild so the main loop
                // can reconstruct ChatViewState from the agent's new messages.
                state.needs_chat_rebuild = true;
            }
        }
        UiEvent::RetryStart {
            attempt,
            max_attempts,
            error_message,
        } => {
            state.add_notification(
                format!("Retry ({}/{}): {}", attempt, max_attempts, error_message),
                NotificationKind::Warning,
            );
        }
        UiEvent::ModelChanged { model_id } => {
            state.add_notification(format!("Model: {}", model_id), NotificationKind::Success);
            state.footer_state.data.model_name = model_id;
        }
        UiEvent::ThinkingLevelChanged { level } => {
            state.add_notification(format!("Thinking: {}", level), NotificationKind::Info);
            state.footer_state.data.thinking_level = Some(level.to_lowercase());
        }
        UiEvent::QueueUpdate { pending, messages } => {
            state.pending_steering = pending;
            // If the agent consumed messages (snapshot is shorter than ours),
            // trim our snapshot to match the agent's view.
            // If the agent reports more messages, it means new ones were added
            // externally — update fully.
            if messages.len() < state.steering_messages_snapshot.len() {
                // Agent consumed some messages — trim from the front
                let consumed = state.steering_messages_snapshot.len() - messages.len();
                state.steering_messages_snapshot =
                    state.steering_messages_snapshot.drain(consumed..).collect();
            } else {
                state.steering_messages_snapshot = messages;
            }
            // Clamp selection
            if state.queue_panel_selected >= state.steering_messages_snapshot.len() {
                state.queue_panel_selected =
                    state.steering_messages_snapshot.len().saturating_sub(1);
            }
        }
        UiEvent::AutoProcessStart { prompt } => {
            // A queued message is being auto-processed by the worker thread.
            // Show the user message bubble and enter streaming state so the
            // TUI is ready for the agent's response.
            state.add_user_message(prompt.clone());
            state.input_history.insert(0, prompt.clone());
            if state.input_history.len() > 100 {
                state.input_history.remove(0);
            }
            state.history_index = 0;
            state.start_streaming();
            // Remove this message from the local snapshot
            if let Some(pos) = state
                .steering_messages_snapshot
                .iter()
                .position(|m| m == &prompt)
            {
                state.steering_messages_snapshot.remove(pos);
            } else if !state.steering_messages_snapshot.is_empty() {
                // Fallback: remove first if exact match not found
                state.steering_messages_snapshot.remove(0);
            }
            state.pending_steering = state.steering_messages_snapshot.len();
            // Clamp selection
            if state.queue_panel_selected >= state.steering_messages_snapshot.len() {
                state.queue_panel_selected =
                    state.steering_messages_snapshot.len().saturating_sub(1);
            }
        }
        UiEvent::SystemMessage(msg) => {
            state.add_notification(msg, NotificationKind::Info);
        }
        UiEvent::TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            context_window_pct,
            total_cost,
        } => {
            state.footer_state.data.input_tokens = input_tokens;
            state.footer_state.data.output_tokens = output_tokens;
            state.footer_state.data.cache_read_tokens = cache_read_tokens;
            state.footer_state.data.cache_write_tokens = cache_write_tokens;
            state.footer_state.data.context_window_pct = context_window_pct;
            state.footer_state.data.total_cost = total_cost;
            state.footer_state.data.context_tokens =
                input_tokens + output_tokens + cache_read_tokens + cache_write_tokens;
        }
    }
}

/// Handle a session event, forwarding relevant ones as UI events.
pub async fn handle_session_event(event: SessionEvent, ui_tx: &mpsc::UnboundedSender<UiEvent>) {
    match event {
        SessionEvent::CompactionStart { reason } => {
            let _ = ui_tx.send(UiEvent::CompactionStart { reason });
        }
        SessionEvent::CompactionEnd {
            reason,
            error_message,
            ..
        } => {
            let _ = ui_tx.send(UiEvent::CompactionEnd {
                _reason: reason,
                error_message,
            });
        }
        SessionEvent::ThinkingLevelChanged { level } => {
            let _ = ui_tx.send(UiEvent::ThinkingLevelChanged {
                level: format!("{:?}", level),
            });
        }
        SessionEvent::QueueUpdate {
            steering,
            follow_up,
        } => {
            let pending = steering.len() + follow_up.len();
            let mut all_messages = steering;
            all_messages.extend(follow_up);
            let _ = ui_tx.send(UiEvent::QueueUpdate {
                pending,
                messages: all_messages,
            });
        }
        SessionEvent::SessionInfoChanged => {}
        SessionEvent::Agent(agent_event) => match &agent_event {
            AgentEvent::Fallback { to_model, .. } => {
                let _ = ui_tx.send(UiEvent::ModelChanged {
                    model_id: to_model.clone(),
                });
            }
            AgentEvent::Retry {
                attempt,
                max_retries,
                reason,
                ..
            } => {
                let _ = ui_tx.send(UiEvent::RetryStart {
                    attempt: *attempt as u32,
                    max_attempts: *max_retries as u32,
                    error_message: reason.clone(),
                });
            }
            AgentEvent::Compaction { .. } => {}
            _ => {}
        },
    }
}

// ── Image viewer ────────────────────────────────────────────────────────

/// Open the last received image in the system viewer.
fn open_last_image(state: &mut AppState) {
    if let Some((base64_data, mime_type)) = state.chat.pending_images.last().cloned() {
        match base64::engine::general_purpose::STANDARD.decode(&base64_data) {
            Ok(bytes) => {
                let ext = match mime_type.as_str() {
                    "image/png" => "png",
                    "image/jpeg" | "image/jpg" => "jpg",
                    "image/gif" => "gif",
                    "image/webp" => "webp",
                    "image/bmp" => "bmp",
                    _ => "bin",
                };
                let path = std::env::temp_dir().join(format!("oxi_image.{}", ext));
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        #[cfg(target_os = "macos")]
                        {
                            std::process::Command::new("open").arg(&path).spawn().ok();
                        }
                        #[cfg(target_os = "linux")]
                        {
                            std::process::Command::new("xdg-open")
                                .arg(&path)
                                .spawn()
                                .ok();
                        }
                        #[cfg(target_os = "windows")]
                        {
                            std::process::Command::new("cmd")
                                .args(["/c", "start"])
                                .arg(&path)
                                .spawn()
                                .ok();
                        }
                        state.add_notification(
                            "Opened image in viewer".to_string(),
                            NotificationKind::Success,
                        );
                    }
                    Err(e) => {
                        state.add_notification(
                            format!("Failed to write image: {}", e),
                            NotificationKind::Error,
                        );
                    }
                }
            }
            Err(e) => {
                state.add_notification(
                    format!("Failed to decode image: {}", e),
                    NotificationKind::Error,
                );
            }
        }
    } else {
        state.add_notification(
            "No images to display".to_string(),
            NotificationKind::Warning,
        );
    }
}

// ── Overlay key handling ─────────────────────────────────────────────────

async fn handle_overlay_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    session: &AgentSession,
) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    // ── Component-based overlay (takes priority) ──
    if let Some(ref mut overlay) = state.overlay_state {
        use super::overlay::OverlayAction;
        let action = overlay.handle_key(key);
        match action {
            OverlayAction::Close => {
                state.overlay_state = None;
                state.overlay = None;
            }
            OverlayAction::SwitchSession(path) => {
                state.next_action = Some(super::app::TuiNextAction::SwitchSession(path));
                state.overlay_state = None;
            }
            OverlayAction::NewSession => {
                state.next_action = Some(super::app::TuiNextAction::NewSession);
                state.overlay_state = None;
            }
            OverlayAction::OpenRouterSetup { initial, models } => {
                state.overlay_state = None;
                state.overlay_state = Some(super::overlay::router_setup(
                    initial,
                    models,
                    move |data: &super::overlay::RouterSetupData| {
                        let store_cfg = router_integration::save_router_config(data)?;
                        let ai_cfg = router_integration::store_config_to_ai_config(&store_cfg);
                        oxi_ai::router::register_router(&ai_cfg);
                        Ok(())
                    },
                    || {},
                ));
                return None;
            }
            OverlayAction::ForkFromEntry { entry_id } => {
                state.overlay_state = None;
                if let Some(ref path) = state.session_file_path {
                    let sm = oxi_store::session::SessionManager::open(path, None, None);
                    match sm.branch_from_entry(&entry_id) {
                        Ok(new_path) => {
                            state.next_action =
                                Some(super::app::TuiNextAction::SwitchSession(new_path));
                            state.add_notification(
                                format!("Forked from [{}]", &entry_id[..8.min(entry_id.len())]),
                                NotificationKind::Success,
                            );
                        }
                        Err(e) => {
                            state.add_notification(
                                format!("Error forking: {}", e),
                                NotificationKind::Error,
                            );
                        }
                    }
                }
                return None;
            }
            OverlayAction::NavigateToEntry { entry_id } => {
                state.overlay_state = None;
                // TODO: integrate with SessionNavigator::navigate_tree() for branch switching
                state.add_notification(
                    format!("Selected entry: {}", &entry_id[..8.min(entry_id.len())]),
                    NotificationKind::Info,
                );
                return None;
            }
            _ => {}
        }
        return None;
    }

    // Clone overlay variant to avoid borrow conflicts
    let overlay = state.overlay.clone();
    match &overlay {
        // ── Setup wizard ──
        Some(AppOverlay::Setup(_)) => handle_wizard_step_key(key, state, session).await,

        // ── Provider config wizard (same steps as setup) ──
        Some(AppOverlay::ProviderConfig(_)) => handle_wizard_step_key(key, state, session).await,

        // ── Model selector ──
        Some(AppOverlay::ModelSelect { .. }) => handle_model_select_key(key, state, session).await,

        // ── Logout selector ──
        Some(AppOverlay::LogoutSelect { .. }) => handle_logout_select_key(key, state).await,

        // ── Resume selector ──
        Some(AppOverlay::ResumeSelect { .. }) => {
            handle_resume_select_key(key, state, session).await
        }

        // ── Routing status (handled by component overlay) ──
        Some(AppOverlay::RoutingStatus { .. }) => None,

        None => None,
    }
}

// ── Setup/Provider wizard — unified handler ────────────────────────────────

/// Extract the SetupStep from an overlay, if it's a setup-type overlay.
fn extract_step(overlay: &Option<AppOverlay>) -> Option<&SetupStep> {
    match overlay {
        Some(AppOverlay::Setup(s)) | Some(AppOverlay::ProviderConfig(s)) => Some(s),
        _ => None,
    }
}

/// Wrap a SetupStep back into the same overlay variant.
fn wrap_step(overlay: &Option<AppOverlay>, step: SetupStep) -> Option<AppOverlay> {
    match overlay {
        Some(AppOverlay::Setup(_)) => Some(AppOverlay::Setup(step)),
        Some(AppOverlay::ProviderConfig(_)) => Some(AppOverlay::ProviderConfig(step)),
        _ => None,
    }
}

/// Check if the overlay is a provider-config (vs initial setup).
fn is_provider_config(overlay: &Option<AppOverlay>) -> bool {
    matches!(overlay, Some(AppOverlay::ProviderConfig(_)))
}

/// Build the provider list from builtins, sorted by category and enriched
/// with display names, descriptions, and key status.
fn build_provider_list(is_config: bool) -> Vec<ProviderInfo> {
    let auth = oxi_store::auth_storage::shared_auth_storage();
    let mut providers: Vec<ProviderInfo> = oxi_ai::register_builtins::get_builtin_providers()
        .iter()
        .map(|builtin| {
            let has_key = if is_config {
                auth.has_auth(builtin.name)
            } else {
                auth.get_api_key(builtin.name).is_some()
            };
            ProviderInfo {
                name: builtin.name.to_string(),
                display_name: builtin.display_name.to_string(),
                has_key,
                category: builtin.category.to_string(),
                description: builtin.description.to_string(),
            }
        })
        .collect();

    // Sort by category order (matching render_provider_list) then by name.
    // This ensures the selected index matches the rendered position.
    let category_rank = |cat: &str| -> usize {
        match cat {
            "primary" => 0,
            "chinese" => 1,
            "open" => 2,
            "cloud" => 3,
            "enterprise" => 4,
            "specialized" => 5,
            _ => 6,
        }
    };
    providers.sort_by(|a, b| {
        category_rank(&a.category)
            .cmp(&category_rank(&b.category))
            .then_with(|| a.name.cmp(&b.name))
    });

    providers
}

/// Unified handler for Setup and ProviderConfig wizard steps.
async fn handle_wizard_step_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    session: &AgentSession,
) -> Option<Action> {
    let step_kind = match extract_step(&state.overlay) {
        Some(s) => match s {
            SetupStep::SelectAuthType { .. } => 0,
            SetupStep::SelectProvider { .. } => 1,
            SetupStep::EnterApiKey { .. } => 2,
            SetupStep::SelectModel { .. } => 3,
            SetupStep::Done { .. } => 4,
        },
        _ => return None,
    };
    let is_config = is_provider_config(&state.overlay);

    match step_kind {
        0 => {
            // SelectAuthType
            match key.code {
                KeyCode::Up | KeyCode::Down => {
                    if let Some(SetupStep::SelectAuthType {
                        auth_type,
                        selected,
                    }) = extract_step(&state.overlay)
                    {
                        let new_sel = if *selected == 0 { 1 } else { 0 };
                        state.overlay = wrap_step(
                            &state.overlay,
                            SetupStep::SelectAuthType {
                                auth_type: auth_type.clone(),
                                selected: new_sel,
                            },
                        );
                    }
                }
                KeyCode::Enter => {
                    if let Some(SetupStep::SelectAuthType { selected, .. }) =
                        extract_step(&state.overlay)
                    {
                        match *selected {
                            0 => {
                                // API Key flow
                                let providers = build_provider_list(is_config);
                                state.overlay = wrap_step(
                                    &state.overlay,
                                    SetupStep::SelectProvider {
                                        providers,
                                        selected: 0,
                                        filter: String::new(),
                                    },
                                );
                            }
                            1 => {
                                // OAuth — not yet implemented, just go to provider select
                                let providers = build_provider_list(is_config);
                                state.overlay = wrap_step(
                                    &state.overlay,
                                    SetupStep::SelectProvider {
                                        providers,
                                        selected: 0,
                                        filter: String::new(),
                                    },
                                );
                            }
                            _ => {}
                        }
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    state.overlay = None;
                }
                _ => {}
            }
        }

        1 => {
            // SelectProvider
            match key.code {
                KeyCode::Up => {
                    if let Some(SetupStep::SelectProvider {
                        providers,
                        selected,
                        ..
                    }) = extract_step(&state.overlay)
                    {
                        let new_sel = if *selected == 0 {
                            providers.len() - 1
                        } else {
                            *selected - 1
                        };
                        state.overlay = wrap_step(
                            &state.overlay,
                            SetupStep::SelectProvider {
                                providers: providers.clone(),
                                selected: new_sel,
                                filter: String::new(),
                            },
                        );
                    }
                }
                KeyCode::Down => {
                    if let Some(SetupStep::SelectProvider {
                        providers,
                        selected,
                        ..
                    }) = extract_step(&state.overlay)
                    {
                        let new_sel = (*selected + 1) % providers.len();
                        state.overlay = wrap_step(
                            &state.overlay,
                            SetupStep::SelectProvider {
                                providers: providers.clone(),
                                selected: new_sel,
                                filter: String::new(),
                            },
                        );
                    }
                }
                KeyCode::Enter => {
                    if let Some(SetupStep::SelectProvider {
                        providers,
                        selected,
                        ..
                    }) = extract_step(&state.overlay)
                    {
                        if let Some(pi) = providers.get(*selected).cloned() {
                            state.overlay = wrap_step(
                                &state.overlay,
                                SetupStep::EnterApiKey {
                                    provider: pi.name.clone(),
                                    key: String::new(),
                                    masked_cursor: 0,
                                },
                            );
                        }
                    }
                }
                KeyCode::Esc => {
                    state.overlay = None;
                }
                _ => {}
            }
        }

        2 => {
            // EnterApiKey
            let provider = match extract_step(&state.overlay) {
                Some(SetupStep::EnterApiKey { provider, .. }) => provider.clone(),
                _ => return None,
            };
            match key.code {
                KeyCode::Char(c) => {
                    if let Some(SetupStep::EnterApiKey { key, .. }) =
                        extract_step_mut(&mut state.overlay)
                    {
                        key.push(c);
                    }
                }
                KeyCode::Backspace => {
                    if let Some(SetupStep::EnterApiKey { key, .. }) =
                        extract_step_mut(&mut state.overlay)
                    {
                        key.pop();
                    }
                }
                KeyCode::Enter => {
                    let key_val = match extract_step(&state.overlay) {
                        Some(SetupStep::EnterApiKey { key, .. }) => key.clone(),
                        _ => String::new(),
                    };

                    if !key_val.is_empty() {
                        let auth = oxi_store::auth_storage::shared_auth_storage();
                        auth.set_api_key(&provider, key_val);

                        let models: Vec<String> = oxi_ai::model_db::get_all_models()
                            .filter(|e| e.provider == provider)
                            .map(|e| e.id.to_string())
                            .collect();

                        if models.is_empty() {
                            if !is_config {
                                let model_id = "default".to_string();
                                let full_model = format!("{}/{}", provider, model_id);
                                if let Ok(mut settings) = oxi_store::settings::Settings::load() {
                                    settings.default_model = Some(model_id.clone());
                                    settings.default_provider = Some(provider.clone());
                                    let _ = settings.save();
                                }
                                state.footer_state.data.model_name = full_model.clone();
                                state.footer_state.data.provider_name = provider.clone();
                                state.overlay = wrap_step(
                                    &state.overlay,
                                    SetupStep::Done {
                                        provider: provider.clone(),
                                        model: full_model,
                                    },
                                );
                            } else {
                                state.add_notification(format!("{} API key saved.", provider), NotificationKind::Success);
                                state.overlay = None;
                            }
                        } else {
                            state.overlay = wrap_step(
                                &state.overlay,
                                SetupStep::SelectModel {
                                    provider,
                                    models,
                                    selected: 0,
                                },
                            );
                        }
                    }
                }
                KeyCode::Esc => {
                    let providers = build_provider_list(is_config);
                    state.overlay = wrap_step(
                        &state.overlay,
                        SetupStep::SelectProvider {
                            providers,
                            selected: 0,
                            filter: String::new(),
                        },
                    );
                }
                _ => {}
            }
        }

        3 => {
            // SelectModel
            match key.code {
                KeyCode::Up => {
                    if let Some(SetupStep::SelectModel {
                        provider,
                        models,
                        selected,
                    }) = extract_step(&state.overlay)
                    {
                        let new_sel = if *selected == 0 {
                            models.len().saturating_sub(1)
                        } else {
                            *selected - 1
                        };
                        state.overlay = wrap_step(
                            &state.overlay,
                            SetupStep::SelectModel {
                                provider: provider.clone(),
                                models: models.clone(),
                                selected: new_sel,
                            },
                        );
                    }
                }
                KeyCode::Down => {
                    if let Some(SetupStep::SelectModel {
                        provider,
                        models,
                        selected,
                    }) = extract_step(&state.overlay)
                    {
                        let new_sel = if models.is_empty() {
                            0
                        } else {
                            (*selected + 1).min(models.len() - 1)
                        };
                        state.overlay = wrap_step(
                            &state.overlay,
                            SetupStep::SelectModel {
                                provider: provider.clone(),
                                models: models.clone(),
                                selected: new_sel,
                            },
                        );
                    }
                }
                KeyCode::Enter => {
                    if let Some(SetupStep::SelectModel {
                        provider,
                        models,
                        selected,
                    }) = extract_step(&state.overlay)
                    {
                        if let Some(model_id) = models.get(*selected) {
                            let full_model = format!("{}/{}", provider, model_id);
                            if let Ok(mut settings) = oxi_store::settings::Settings::load() {
                                settings.default_model = Some(model_id.to_string());
                                settings.default_provider = Some(provider.clone());
                                let _ = settings.save();
                            }
                            state.footer_state.data.model_name = full_model.clone();
                            state.footer_state.data.provider_name = provider.clone();
                            if !is_config {
                                state.overlay = wrap_step(
                                    &state.overlay,
                                    SetupStep::Done {
                                        provider: provider.clone(),
                                        model: full_model,
                                    },
                                );
                            } else {
                                // Actually switch the model in the running session
                                if let Err(e) = session.set_model(&full_model) {
                                    state.add_notification(format!("Error switching model: {}", e), NotificationKind::Error);
                                } else {
                                    state.add_notification(format!("Model set to {}", full_model), NotificationKind::Success);
                                }
                                state.overlay = None;
                            }
                        }
                    }
                }
                KeyCode::Esc => {
                    if let Some(SetupStep::SelectModel { provider, .. }) =
                        extract_step(&state.overlay)
                    {
                        state.overlay = wrap_step(
                            &state.overlay,
                            SetupStep::EnterApiKey {
                                provider: provider.clone(),
                                key: String::new(),
                                masked_cursor: 0,
                            },
                        );
                    }
                }
                _ => {}
            }
        }

        4
            // Done
            if key.code == KeyCode::Enter => {
                state.overlay = None;
                state.add_notification("Ready to chat".to_string(), NotificationKind::Info);
            }

        _ => {}
    }

    None
}

/// Extract a mutable reference to the SetupStep from an overlay.
fn extract_step_mut(overlay: &mut Option<AppOverlay>) -> Option<&mut SetupStep> {
    match overlay {
        Some(AppOverlay::Setup(s)) | Some(AppOverlay::ProviderConfig(s)) => Some(s),
        _ => None,
    }
}

async fn handle_model_select_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    session: &AgentSession,
) -> Option<Action> {
    let (models, filter, selected) = match &state.overlay {
        Some(AppOverlay::ModelSelect {
            models,
            filter,
            selected,
        }) => (models.clone(), filter.clone(), *selected),
        _ => return None,
    };

    // Compute filtered view
    let filtered: Vec<(usize, &String)> = if filter.is_empty() {
        models.iter().enumerate().collect()
    } else {
        let lower = filter.to_lowercase();
        models
            .iter()
            .enumerate()
            .filter(|(_, m)| m.to_lowercase().contains(&lower))
            .collect()
    };

    match key.code {
        KeyCode::Up => {
            let new_sel = if selected == 0 {
                filtered.len().saturating_sub(1)
            } else {
                selected.saturating_sub(1)
            };
            state.overlay = Some(AppOverlay::ModelSelect {
                models,
                filter,
                selected: new_sel,
            });
        }
        KeyCode::Down => {
            let new_sel = if filtered.is_empty() {
                0
            } else {
                (selected + 1).min(filtered.len() - 1)
            };
            state.overlay = Some(AppOverlay::ModelSelect {
                models,
                filter,
                selected: new_sel,
            });
        }
        KeyCode::Enter => {
            if let Some((_idx, model_id)) = filtered.get(selected) {
                let model_id = (*model_id).clone();
                match session.set_model(&model_id) {
                    Ok(()) => {
                        state.add_notification(
                            format!("Model: {}", model_id),
                            NotificationKind::Success,
                        );
                        state.footer_state.data.model_name = model_id.clone();
                        oxi_store::settings::Settings::save_last_used(&model_id);
                    }
                    Err(e) => {
                        state.add_notification(format!("Error: {}", e), NotificationKind::Error);
                    }
                }
            }
            state.overlay = None;
        }
        KeyCode::Esc => {
            state.overlay = None;
        }
        KeyCode::Backspace => {
            let mut new_filter = filter;
            new_filter.pop();
            state.overlay = Some(AppOverlay::ModelSelect {
                models,
                filter: new_filter,
                selected: 0,
            });
        }
        KeyCode::Char(c) => {
            let mut new_filter = filter;
            new_filter.push(c);
            state.overlay = Some(AppOverlay::ModelSelect {
                models,
                filter: new_filter,
                selected: 0,
            });
        }
        _ => {}
    }

    None
}

async fn handle_resume_select_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    _session: &crate::app::agent_session::AgentSession,
) -> Option<Action> {
    let (sessions, selected) = match &state.overlay {
        Some(AppOverlay::ResumeSelect { sessions, selected }) => (sessions.clone(), *selected),
        _ => return None,
    };

    match key.code {
        KeyCode::Up => {
            let new_sel = if selected == 0 {
                sessions.len().saturating_sub(1)
            } else {
                selected - 1
            };
            state.overlay = Some(AppOverlay::ResumeSelect {
                sessions,
                selected: new_sel,
            });
        }
        KeyCode::Down => {
            let new_sel = if sessions.is_empty() {
                0
            } else {
                (selected + 1).min(sessions.len() - 1)
            };
            state.overlay = Some(AppOverlay::ResumeSelect {
                sessions,
                selected: new_sel,
            });
        }
        KeyCode::Enter => {
            // Only select if input is empty — otherwise user is trying to send a message
            if !state.input.text().is_empty() {
                return None;
            }
            if let Some(session_info) = sessions.get(selected) {
                state.next_action = Some(super::app::TuiNextAction::SwitchSession(
                    session_info.path.clone(),
                ));
                state.add_notification(
                    format!("Switching to session: {}", session_info.path),
                    NotificationKind::Info,
                );
            }
            state.overlay = None;
        }
        KeyCode::Esc => {
            state.overlay = None;
        }
        _ => {}
    }

    None
}

async fn handle_logout_select_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
) -> Option<Action> {
    let (providers, selected) = match &state.overlay {
        Some(AppOverlay::LogoutSelect {
            providers,
            selected,
        }) => (providers.clone(), *selected),
        _ => return None,
    };

    match key.code {
        KeyCode::Up => {
            let new_sel = if selected == 0 {
                providers.len().saturating_sub(1)
            } else {
                selected - 1
            };
            state.overlay = Some(AppOverlay::LogoutSelect {
                providers,
                selected: new_sel,
            });
        }
        KeyCode::Down => {
            let new_sel = if providers.is_empty() {
                0
            } else {
                (selected + 1).min(providers.len() - 1)
            };
            state.overlay = Some(AppOverlay::LogoutSelect {
                providers,
                selected: new_sel,
            });
        }
        KeyCode::Enter => {
            if let Some(provider) = providers.get(selected) {
                let auth = oxi_store::auth_storage::shared_auth_storage();
                auth.remove(provider);
                state.add_notification(format!("Removed {}", provider), NotificationKind::Success);
            }
            state.overlay = None;
        }
        KeyCode::Esc => {
            state.overlay = None;
        }
        _ => {}
    }

    None
}

// ── Overlay paste handler ────────────────────────────────────────────────

fn handle_overlay_paste(text: &str, state: &mut AppState) -> Option<Action> {
    match &state.overlay {
        // Setup/Login EnterApiKey step — paste into key field
        Some(AppOverlay::Setup(SetupStep::EnterApiKey { .. }))
        | Some(AppOverlay::ProviderConfig(SetupStep::EnterApiKey { .. })) => {
            if let Some(SetupStep::EnterApiKey { key, .. }) = extract_step_mut(&mut state.overlay) {
                key.push_str(text);
            }
        }
        // ModelSelect — paste into filter
        Some(AppOverlay::ModelSelect { .. }) => {
            if let Some(AppOverlay::ModelSelect { filter, .. }) = &mut state.overlay {
                filter.push_str(text);
            }
        }
        _ => {}
    }
    None
}

/// Handle a mouse click — check if it hit a thinking or tool block.
fn handle_click(col: u16, row: u16, state: &mut AppState) {
    let thinking: Vec<(u16, u16, String)> = state.chat.thinking_regions.clone();
    for (y_start, y_end, key) in &thinking {
        if row >= *y_start && row < *y_end {
            state.chat.toggle_thinking(key);
            return;
        }
    }
    let tools: Vec<(u16, u16, String)> = state.chat.tool_regions.clone();
    for (y_start, y_end, key) in &tools {
        if row >= *y_start && row < *y_end {
            state.chat.toggle_tool(key);
            return;
        }
    }
    let _ = (col, row); // unused column
}

// ── File completion helpers ─────────────────────────────────────────────

/// Update file path completions based on current input text.
fn update_file_completions(state: &mut AppState) {
    let text = state.input.text();
    if text.starts_with("./")
        || text.starts_with("../")
        || text.starts_with('~')
        || (text.contains('/') && !text.starts_with('/'))
    {
        let results = state.completion_manager.get_completions(text.as_str());
        state.file_completions = results;
        state.file_completion_index = 0;
        state.file_completion_active = !state.file_completions.is_empty();
    } else if !text.starts_with('/') {
        // Not a slash command and not a path — clear file completions
        state.file_completions.clear();
        state.file_completion_active = false;
    }
}
