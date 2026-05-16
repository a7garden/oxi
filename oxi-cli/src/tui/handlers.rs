//! Event handlers for the TUI.

use super::app::{AppOverlay, AppState, SetupStep, UiEvent};
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
            if state.overlay.is_some() {
                handle_overlay_key(key, state, session).await
            } else {
                handle_key(key, state, session, ui_tx, running).await
            }
        }
        CEvent::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => state.scroll_up(3),
                MouseEventKind::ScrollDown => state.scroll_down(3),
                _ => {}
            }
            None
        }
        // IME 조합 완료 텍스트나 클립보드 붙여넣기 처리
        CEvent::Paste(text) => {
            if state.overlay.is_some() {
                // 오버레이 활성 시 Paste를 overlay handler로 전달
                handle_overlay_paste(&text, state)
            } else {
                state.input.insert_str(&text);
                state.update_slash_completions();
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
    _ui_tx: &mpsc::UnboundedSender<UiEvent>,
    running: &mut bool,
) -> Option<Action> {
    // 키보드 이벤트 타입이 지원되는 경우 Press만 처리
    // (Repeat/Release 무시 — IME 조합 중 Repeat 이벤트 방지)
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        KeyCode::Enter => {
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
                let handled = slash::handle_slash_command(&value, session, state, running);
                state.input_clear();
                if handled {
                    return None;
                }
            }

            let value = value; // re-bind after slash check
            if state.is_agent_busy {
                // Agent busy — queue as steering message
                state.add_system_message(format!(
                    "Queued: {}",
                    value.chars().take(50).collect::<String>()
                ));
                state.input_history.insert(0, value.clone());
                if state.input_history.len() > 100 {
                    state.input_history.remove(0);
                }
                state.history_index = 0;
                let _ = session.steer(value);
                state.input_clear();
                return None;
            }

            // Not busy — send directly
            Some(Action::SendPrompt(value))
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.is_agent_busy {
                let sh = session.clone_handle();
                tokio::spawn(async move { sh.abort().await });
                state.cancel_streaming();
                state.add_system_message("Interrupted".to_string());
            } else {
                *running = false;
            }
            None
        }
        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            open_last_image(state);
            None
        }
        // Shift+Tab: cycle thinking level
        KeyCode::BackTab => {
            if state.slash_completion_active {
                return None;
            }
            if let Some(next_level) = session.cycle_thinking_level() {
                state.footer_state.data.thinking_level =
                    Some(format!("{:?}", next_level).to_lowercase());
                state.add_system_message(format!("Thinking: {:?}", next_level));
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
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+Y: copy last code block to clipboard
            if let Some(ref code) = state.chat.last_code_block {
                match clipboard_write::copy_to_clipboard(code) {
                    Ok(()) => state.add_system_message("\u{2713} Code block copied".to_string()),
                    Err(e) => state.add_system_message(format!("\u{2717} Copy failed: {}", e)),
                }
            } else {
                state.add_system_message("No code block to copy".to_string());
            }
            None
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.input.insert_char(c);
            state.update_slash_completions();
            None
        }
        KeyCode::Backspace => {
            state.input.backspace();
            state.update_slash_completions();
            None
        }
        KeyCode::Delete => {
            state.input.delete();
            state.update_slash_completions();
            None
        }
        KeyCode::Left => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.input.move_word_left();
            } else {
                state.input.move_left();
            }
            None
        }
        KeyCode::Right => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.input.move_word_right();
            } else {
                state.input.move_right();
            }
            None
        }
        KeyCode::Home => {
            state.input.move_home();
            None
        }
        KeyCode::End => {
            state.input.move_end();
            None
        }
        KeyCode::Tab => {
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
        KeyCode::Up => {
            if state.slash_completion_active {
                state.prev_slash_completion();
            } else if state.input.text().is_empty() && !state.input_history.is_empty() {
                if state.history_index == 0 {
                    state.saved_input = state.input.text();
                }
                if state.history_index < state.input_history.len() {
                    state.history_index += 1;
                    state.input_set_text(state.input_history[state.history_index - 1].clone());
                    state.clear_slash_completions();
                }
            } else {
                state.scroll_up(3);
            }
            None
        }
        KeyCode::Down => {
            if state.slash_completion_active {
                state.next_slash_completion();
            } else if state.history_index > 0 {
                state.history_index -= 1;
                if state.history_index == 0 {
                    state.input_set_text(state.saved_input.clone());
                } else {
                    state.input_set_text(state.input_history[state.history_index - 1].clone());
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
/// pi-mono pattern: MessageUpdate drives rendering (not TextChunk deltas).
pub fn handle_ui_event(event: UiEvent, state: &mut AppState) {
    match event {
        // ── Agent lifecycle ───────────────────────────────────────
        UiEvent::AgentStart => {
            // Agent started processing
        }
        UiEvent::AgentEnd => {
            // Agent finished all processing — clear busy state
            state.is_agent_busy = false;
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
            state.chat.start_streaming();
            state.is_agent_busy = true;
            state.auto_scroll = true;
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
            state.chat.stream_tool_result(
                Some(tool_call_id),
                tool_name,
                result.content.chars().take(500).collect(),
                is_error || result.status == "error",
            );
        }

        // ── Legacy events ─────────────────────────────────────────
        UiEvent::Thinking => {
            // Agent is waiting for first token
        }
        UiEvent::ThinkingDelta(text) => {
            // Thinking text — still useful for showing reasoning
            state.chat.stream_thinking(text, true);
        }
        UiEvent::Complete => {
            // Agent completed — only finalize if still streaming
            // (MessageEnd may have already finalized)
            if state.chat.is_streaming() {
                state.finish_streaming();
            } else {
                // Already finalized via MessageEnd, just clear busy state
                state.is_agent_busy = false;
            }
        }
        UiEvent::Error(msg) => {
            state.cancel_streaming();
            state.add_system_message(format!("Error: {}", msg));
        }

        // ── Session events ────────────────────────────────────────
        UiEvent::CompactionStart { reason } => {
            let reason_str = match reason {
                CompactionReason::Manual => "manual",
                CompactionReason::Threshold => "auto",
                CompactionReason::Automatic => "auto",
                CompactionReason::Overflow => "overflow",
                CompactionReason::Iteration { .. } => "iteration",
            };
            state.add_system_message(format!("Compacting ({})...", reason_str));
        }
        UiEvent::CompactionEnd {
            _reason,
            error_message,
        } => {
            let msg = if let Some(err) = error_message {
                format!("Compaction failed: {}", err)
            } else {
                "Compaction complete".to_string()
            };
            state.add_system_message(msg);
        }
        UiEvent::RetryStart {
            attempt,
            max_attempts,
            error_message,
        } => {
            state.add_system_message(format!(
                "Retry ({}/{}): {}",
                attempt, max_attempts, error_message
            ));
        }
        UiEvent::ModelChanged { model_id } => {
            state.add_system_message(format!("Model: {}", model_id));
            state.footer_state.data.model_name = model_id;
        }
        UiEvent::ThinkingLevelChanged { level } => {
            state.add_system_message(format!("Thinking: {}", level));
            state.footer_state.data.thinking_level = Some(level.to_lowercase());
        }
        UiEvent::QueueUpdate { pending } => {
            state.pending_steering = pending;
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
            let _ = ui_tx.send(UiEvent::QueueUpdate { pending });
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
                        state.add_system_message("Opened image in viewer".to_string());
                    }
                    Err(e) => {
                        state.add_system_message(format!("Failed to write image: {}", e));
                    }
                }
            }
            Err(e) => {
                state.add_system_message(format!("Failed to decode image: {}", e));
            }
        }
    } else {
        state.add_system_message("No images to display".to_string());
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
            _ => {}
        }
        return None;
    }

    // Clone overlay variant to avoid borrow conflicts
    let overlay = state.overlay.clone();
    match &overlay {
        // ── Setup wizard ──
        Some(AppOverlay::Setup(_)) => handle_wizard_step_key(key, state).await,

        // ── Provider config wizard (same steps as setup) ──
        Some(AppOverlay::ProviderConfig(_)) => handle_wizard_step_key(key, state).await,

        // ── Model selector ──
        Some(AppOverlay::ModelSelect { .. }) => handle_model_select_key(key, state, session).await,

        // ── Logout selector ──
        Some(AppOverlay::LogoutSelect { .. }) => handle_logout_select_key(key, state).await,

        // ── Resume selector ──
        Some(AppOverlay::ResumeSelect { .. }) => {
            handle_resume_select_key(key, state, session).await
        }

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

/// Unified handler for Setup and ProviderConfig wizard steps.
async fn handle_wizard_step_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
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
                            0 => { /* OAuth — not yet implemented */ }
                            1 => {
                                let auth = oxi_store::auth_storage::shared_auth_storage();
                                let providers: Vec<(String, bool)> =
                                    oxi_ai::register_builtins::get_builtin_providers()
                                        .iter()
                                        .map(|builtin| {
                                            let has_key = if is_config {
                                                auth.has_auth(builtin.name)
                                            } else {
                                                auth.get_api_key(builtin.name).is_some()
                                            };
                                            (builtin.name.to_string(), has_key)
                                        })
                                        .collect();
                                state.overlay = wrap_step(
                                    &state.overlay,
                                    SetupStep::SelectProvider {
                                        providers,
                                        selected: 0,
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
                            },
                        );
                    }
                }
                KeyCode::Down => {
                    if let Some(SetupStep::SelectProvider {
                        providers,
                        selected,
                    }) = extract_step(&state.overlay)
                    {
                        let new_sel = (*selected + 1) % providers.len();
                        state.overlay = wrap_step(
                            &state.overlay,
                            SetupStep::SelectProvider {
                                providers: providers.clone(),
                                selected: new_sel,
                            },
                        );
                    }
                }
                KeyCode::Enter => {
                    if let Some(SetupStep::SelectProvider {
                        providers,
                        selected,
                    }) = extract_step(&state.overlay)
                    {
                        if let Some((name, _)) = providers.get(*selected).cloned() {
                            state.overlay = wrap_step(
                                &state.overlay,
                                SetupStep::EnterApiKey {
                                    provider: name,
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
                                state.add_system_message(format!("{} API key saved.", provider));
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
                    let auth = oxi_store::auth_storage::shared_auth_storage();
                    let providers: Vec<(String, bool)> =
                        oxi_ai::register_builtins::get_builtin_providers()
                            .iter()
                            .map(|builtin| {
                                let has_key = if is_config {
                                    auth.has_auth(builtin.name)
                                } else {
                                    auth.get_api_key(builtin.name).is_some()
                                };
                                (builtin.name.to_string(), has_key)
                            })
                            .collect();
                    state.overlay = wrap_step(
                        &state.overlay,
                        SetupStep::SelectProvider {
                            providers,
                            selected: 0,
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
                                state.add_system_message(format!("Model set to {}", full_model));
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

        4 => {
            // Done
            if key.code == KeyCode::Enter {
                state.overlay = None;
                state.add_system_message(" Ready to chat. Type a message to start.".to_string());
            }
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
                        state.add_system_message(format!("Model: {}", model_id));
                        state.footer_state.data.model_name = model_id.clone();
                        oxi_store::settings::Settings::save_last_used(&model_id);
                    }
                    Err(e) => {
                        state.add_system_message(format!("Error: {}", e));
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
                state.add_system_message(format!("Switching to session: {}", session_info.path));
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
                state.add_system_message(format!("Removed {}", provider));
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
