//! Event handlers for the TUI.

use super::app::{AppOverlay, AppState, SetupStep, UiEvent};
use super::slash;
use crate::agent_session::{AgentSession, CompactionReason, SessionEvent};
use crate::clipboard_write;
use oxi_agent::AgentEvent;
use tokio::sync::mpsc;
use base64::Engine;

use crossterm::event::{
    Event as CEvent, KeyCode, KeyModifiers, MouseEventKind,
    KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    KeyEventKind,
};

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
    ui_tx: &mpsc::Sender<UiEvent>,
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
            } else if !state.is_agent_busy {
                state.input.insert_str(&text);
                state.update_slash_completions();
                None
            } else {
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
    _ui_tx: &mpsc::Sender<UiEvent>,
    running: &mut bool,
) -> Option<Action> {
    // 키보드 이벤트 타입이 지원되는 경우 Press만 처리
    // (Repeat/Release 무시 — IME 조합 중 Repeat 이벤트 방지)
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        KeyCode::Enter => {
            if !state.is_agent_busy {
                // 슬래시 명령 팝업이 활성 상태면 선택된 명령 바로 실행
                if state.slash_completion_active {
                    let cmd = state.selected_slash_command().map(|c| c.name.clone());
                    state.clear_slash_completions();
                    state.input_clear();
                    if let Some(cmd) = cmd {
                        return Some(Action::ExecuteSlashCommand(cmd));
                    }
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
        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            open_last_image(state);
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
        }
        UiEvent::QueueUpdate { pending } => {
            if pending > 0 {
                tracing::debug!("Queue: {} pending", pending);
            }
        }
        UiEvent::ImageBlock { mime_type, base64_data } => {
            state.stream_image(mime_type, base64_data);
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
                        { std::process::Command::new("open").arg(&path).spawn().ok(); }
                        #[cfg(target_os = "linux")]
                        { std::process::Command::new("xdg-open").arg(&path).spawn().ok(); }
                        #[cfg(target_os = "windows")]
                        { std::process::Command::new("cmd").args(["/c", "start"]).arg(&path).spawn().ok(); }
                        state.add_system_message(format!("Opened image in viewer"));
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

    // Clone overlay variant to avoid borrow conflicts
    let overlay = state.overlay.clone();
    match &overlay {
        // ── Setup wizard ──
        Some(AppOverlay::Setup(_)) => {
            handle_setup_step_key(key, state).await
        }

        // ── Login wizard (same steps as setup) ──
        Some(AppOverlay::LoginProvider(_)) => {
            handle_login_step_key(key, state).await
        }

        // ── Model selector ──
        Some(AppOverlay::ModelSelect { .. }) => {
            handle_model_select_key(key, state, session).await
        }

        // ── Logout selector ──
        Some(AppOverlay::LogoutSelect { .. }) => {
            handle_logout_select_key(key, state).await
        }

        None => None,
    }
}

async fn handle_setup_step_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
) -> Option<Action> {
    // Clone the overlay to determine which step we're on without holding a borrow
    let step_kind = match &state.overlay {
        Some(AppOverlay::Setup(s)) => match s {
            SetupStep::SelectProvider { .. } => 0,
            SetupStep::EnterApiKey { provider, .. } => 1,
            SetupStep::SelectModel { .. } => 2,
            SetupStep::Done { .. } => 3,
        },
        _ => return None,
    };

    match step_kind {
        0 => { // SelectProvider
            match key.code {
                KeyCode::Up => {
                    if let Some(AppOverlay::Setup(SetupStep::SelectProvider { providers, selected })) = &state.overlay {
                        let new_sel = if *selected == 0 { providers.len() - 1 } else { *selected - 1 };
                        state.overlay = Some(AppOverlay::Setup(SetupStep::SelectProvider { providers: providers.clone(), selected: new_sel }));
                    }
                }
                KeyCode::Down => {
                    if let Some(AppOverlay::Setup(SetupStep::SelectProvider { providers, selected })) = &state.overlay {
                        let new_sel = (*selected + 1) % providers.len();
                        state.overlay = Some(AppOverlay::Setup(SetupStep::SelectProvider { providers: providers.clone(), selected: new_sel }));
                    }
                }
                KeyCode::Enter => {
                    if let Some(AppOverlay::Setup(SetupStep::SelectProvider { providers, selected })) = &state.overlay {
                        if let Some((name, _)) = providers.get(*selected).cloned() {
                            state.overlay = Some(AppOverlay::Setup(SetupStep::EnterApiKey {
                                provider: name,
                                key: String::new(),
                                masked_cursor: 0,
                            }));
                        }
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    state.overlay = None;
                }
                _ => {}
            }
        }

        1 => { // EnterApiKey
            // Get provider name from overlay
            let provider = match &state.overlay {
                Some(AppOverlay::Setup(SetupStep::EnterApiKey { provider, .. })) => provider.clone(),
                _ => return None,
            };
            match key.code {
                KeyCode::Char(c) => {
                    if let Some(AppOverlay::Setup(SetupStep::EnterApiKey { key, .. })) = &mut state.overlay {
                        key.push(c);
                    }
                }
                KeyCode::Backspace => {
                    if let Some(AppOverlay::Setup(SetupStep::EnterApiKey { key, .. })) = &mut state.overlay {
                        key.pop();
                    }
                }
                KeyCode::Enter => {
                    let key_val = if let Some(AppOverlay::Setup(SetupStep::EnterApiKey { key, .. })) = &state.overlay {
                        key.clone()
                    } else { String::new() };

                    if !key_val.is_empty() {
                        let auth = crate::auth_storage::AuthStorage::new();
                        auth.set_api_key(&provider, key_val);

                        // Get models for this provider from the model database
                        let models: Vec<String> = oxi_ai::model_db::get_all_models()
                            .into_iter()
                            .filter(|e| e.provider == provider)
                            .map(|e| e.id.to_string())
                            .collect();

                        if models.is_empty() {
                            // No models in DB for this provider, use default model
                            let model = format!("{}/default", provider);
                            
                            // Persist model to settings
                            if let Ok(mut settings) = crate::settings::Settings::load() {
                                settings.default_model = Some(model.clone());
                                settings.default_provider = Some(provider.clone());
                                eprintln!("[DEBUG] Saving model: {}", model);
                                if let Err(e) = settings.save() {
                                    eprintln!("[DEBUG] Save failed: {}", e);
                                }
                            } else {
                                eprintln!("[DEBUG] Failed to load settings");
                            }

                            state.footer_state.data.model_name = model.clone();
                            state.footer_state.data.provider_name = provider.clone();

                            state.overlay = Some(AppOverlay::Setup(SetupStep::Done {
                                provider: provider.clone(),
                                model,
                            }));
                        } else {
                            // Show model selection
                            state.overlay = Some(AppOverlay::Setup(SetupStep::SelectModel {
                                provider,
                                models,
                                selected: 0,
                            }));
                        }
                    }
                }
                KeyCode::Esc => {
                    let auth = crate::auth_storage::AuthStorage::new();
                    let providers: Vec<(String, bool)> = oxi_ai::register_builtins::get_builtin_providers()
                        .iter()
                        .map(|builtin| {
                            let has_key = auth.get_api_key(builtin.name).is_some();
                            (builtin.name.to_string(), has_key)
                        }).collect();
                    state.overlay = Some(AppOverlay::Setup(SetupStep::SelectProvider { providers, selected: 0 }));
                }
                _ => {}
            }
        }

        2 => { // SelectModel
            match key.code {
                KeyCode::Up => {
                    if let Some(AppOverlay::Setup(SetupStep::SelectModel { provider, models, selected })) = &state.overlay {
                        let new_sel = if *selected == 0 { models.len().saturating_sub(1) } else { *selected - 1 };
                        state.overlay = Some(AppOverlay::Setup(SetupStep::SelectModel { provider: provider.clone(), models: models.clone(), selected: new_sel }));
                    }
                }
                KeyCode::Down => {
                    if let Some(AppOverlay::Setup(SetupStep::SelectModel { provider, models, selected })) = &state.overlay {
                        let new_sel = if models.is_empty() { 0 } else { (*selected + 1).min(models.len() - 1) };
                        state.overlay = Some(AppOverlay::Setup(SetupStep::SelectModel { provider: provider.clone(), models: models.clone(), selected: new_sel }));
                    }
                }
                KeyCode::Enter => {
                    if let Some(AppOverlay::Setup(SetupStep::SelectModel { provider, models, selected })) = &state.overlay {
                        if let Some(model_id) = models.get(*selected) {
                            let model = format!("{}/{}", provider, model_id);
                            
                            // Persist model to settings
                            if let Ok(mut settings) = crate::settings::Settings::load() {
                                settings.default_model = Some(model.clone());
                                settings.default_provider = Some(provider.clone());
                                eprintln!("[DEBUG] Saving model: {}", model);
                                if let Err(e) = settings.save() {
                                    eprintln!("[DEBUG] Save failed: {}", e);
                                }
                            } else {
                                eprintln!("[DEBUG] Failed to load settings");
                            }

                            state.footer_state.data.model_name = model.clone();
                            state.footer_state.data.provider_name = provider.clone();

                            state.overlay = Some(AppOverlay::Setup(SetupStep::Done {
                                provider: provider.clone(),
                                model,
                            }));
                        }
                    }
                }
                KeyCode::Esc => {
                    // Go back to EnterApiKey, clear the key
                    if let Some(AppOverlay::Setup(SetupStep::SelectModel { provider, .. })) = &state.overlay {
                        state.overlay = Some(AppOverlay::Setup(SetupStep::EnterApiKey {
                            provider: provider.clone(),
                            key: String::new(),
                            masked_cursor: 0,
                        }));
                    }
                }
                _ => {}
            }
        }

        3 => { // Done
            if key.code == KeyCode::Enter {
                state.overlay = None;
                state.add_system_message(" Ready to chat. Type a message to start.".to_string());
            }
        }

        _ => {}
    }

    None
}

async fn handle_login_step_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
) -> Option<Action> {
    // Clone the overlay to determine which step we're on without holding a borrow
    let step_kind = match &state.overlay {
        Some(AppOverlay::LoginProvider(s)) => match s {
            SetupStep::SelectProvider { .. } => 0,
            SetupStep::EnterApiKey { provider, .. } => 1,
            SetupStep::SelectModel { .. } => 2,
            SetupStep::Done { .. } => 3,
        },
        _ => return None,
    };

    match step_kind {
        0 => { // SelectProvider
            match key.code {
                KeyCode::Up => {
                    if let Some(AppOverlay::LoginProvider(SetupStep::SelectProvider { providers, selected })) = &state.overlay {
                        let new_sel = if *selected == 0 { providers.len() - 1 } else { *selected - 1 };
                        state.overlay = Some(AppOverlay::LoginProvider(SetupStep::SelectProvider { providers: providers.clone(), selected: new_sel }));
                    }
                }
                KeyCode::Down => {
                    if let Some(AppOverlay::LoginProvider(SetupStep::SelectProvider { providers, selected })) = &state.overlay {
                        let new_sel = (*selected + 1) % providers.len();
                        state.overlay = Some(AppOverlay::LoginProvider(SetupStep::SelectProvider { providers: providers.clone(), selected: new_sel }));
                    }
                }
                KeyCode::Enter => {
                    if let Some(AppOverlay::LoginProvider(SetupStep::SelectProvider { providers, selected })) = &state.overlay {
                        if let Some((name, _)) = providers.get(*selected).cloned() {
                            state.overlay = Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey {
                                provider: name,
                                key: String::new(),
                                masked_cursor: 0,
                            }));
                        }
                    }
                }
                KeyCode::Esc => {
                    state.overlay = None;
                }
                _ => {}
            }
        }

        1 => { // EnterApiKey
            let provider = match &state.overlay {
                Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey { provider, .. })) => provider.clone(),
                _ => return None,
            };
            match key.code {
                KeyCode::Char(c) => {
                    if let Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey { key, .. })) = &mut state.overlay {
                        key.push(c);
                    }
                }
                KeyCode::Backspace => {
                    if let Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey { key, .. })) = &mut state.overlay {
                        key.pop();
                    }
                }
                KeyCode::Enter => {
                    let key_val = if let Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey { key, .. })) = &state.overlay {
                        key.clone()
                    } else { String::new() };

                    if !key_val.is_empty() {
                        let auth = crate::auth_storage::AuthStorage::new();
                        auth.set_api_key(&provider, key_val);

                        // Get models for this provider
                        let models: Vec<String> = oxi_ai::model_db::get_all_models()
                            .into_iter()
                            .filter(|e| e.provider == provider)
                            .map(|e| e.id.to_string())
                            .collect();

                        if models.is_empty() {
                            // No models in DB, close overlay
                            state.add_system_message(format!("{} API key saved.", provider));
                            state.overlay = None;
                        } else {
                            // Show model selection
                            state.overlay = Some(AppOverlay::LoginProvider(SetupStep::SelectModel {
                                provider,
                                models,
                                selected: 0,
                            }));
                        }
                    }
                }
                KeyCode::Esc => {
                    // Go back to provider selection
                    let auth = crate::auth_storage::AuthStorage::new();
                    let provider_list: Vec<(String, bool)> = oxi_ai::register_builtins::get_builtin_providers()
                        .iter()
                        .map(|builtin| {
                            let has_key = auth.has_auth(builtin.name);
                            (builtin.name.to_string(), has_key)
                        }).collect();
                    state.overlay = Some(AppOverlay::LoginProvider(SetupStep::SelectProvider {
                        providers: provider_list,
                        selected: 0,
                    }));
                }
                _ => {}
            }
        }

        2 => { // SelectModel
            match key.code {
                KeyCode::Up => {
                    if let Some(AppOverlay::LoginProvider(SetupStep::SelectModel { provider, models, selected })) = &state.overlay {
                        let new_sel = if *selected == 0 { models.len().saturating_sub(1) } else { *selected - 1 };
                        state.overlay = Some(AppOverlay::LoginProvider(SetupStep::SelectModel { provider: provider.clone(), models: models.clone(), selected: new_sel }));
                    }
                }
                KeyCode::Down => {
                    if let Some(AppOverlay::LoginProvider(SetupStep::SelectModel { provider, models, selected })) = &state.overlay {
                        let new_sel = if models.is_empty() { 0 } else { (*selected + 1).min(models.len() - 1) };
                        state.overlay = Some(AppOverlay::LoginProvider(SetupStep::SelectModel { provider: provider.clone(), models: models.clone(), selected: new_sel }));
                    }
                }
                KeyCode::Enter => {
                    if let Some(AppOverlay::LoginProvider(SetupStep::SelectModel { provider, models, selected })) = &state.overlay {
                        if let Some(model_id) = models.get(*selected) {
                            let model = format!("{}/{}", provider, model_id);
                            state.add_system_message(format!("{} API key saved.", provider));
                            state.overlay = None;
                        }
                    }
                }
                KeyCode::Esc => {
                    // Go back to EnterApiKey
                    if let Some(AppOverlay::LoginProvider(SetupStep::SelectModel { provider, .. })) = &state.overlay {
                        state.overlay = Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey {
                            provider: provider.clone(),
                            key: String::new(),
                            masked_cursor: 0,
                        }));
                    }
                }
                _ => {}
            }
        }

        3 | _ => {
            // Done or unexpected — close overlay
            state.overlay = None;
        }
    }

    None
}

async fn handle_model_select_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    session: &AgentSession,
) -> Option<Action> {
    let (models, filter, selected) = match &state.overlay {
        Some(AppOverlay::ModelSelect { models, filter, selected }) => (models.clone(), filter.clone(), *selected),
        _ => return None,
    };

    // Compute filtered view
    let filtered: Vec<(usize, &String)> = if filter.is_empty() {
        models.iter().enumerate().collect()
    } else {
        let lower = filter.to_lowercase();
        models.iter().enumerate().filter(|(_, m)| m.to_lowercase().contains(&lower)).collect()
    };

    match key.code {
        KeyCode::Up => {
            let new_sel = if selected == 0 { filtered.len().saturating_sub(1) } else { selected.saturating_sub(1) };
            state.overlay = Some(AppOverlay::ModelSelect { models, filter, selected: new_sel });
        }
        KeyCode::Down => {
            let new_sel = if filtered.is_empty() { 0 } else { (selected + 1).min(filtered.len() - 1) };
            state.overlay = Some(AppOverlay::ModelSelect { models, filter, selected: new_sel });
        }
        KeyCode::Enter => {
            if let Some((_idx, model_id)) = filtered.get(selected) {
                let model_id = (*model_id).clone();
                match session.set_model(&model_id) {
                    Ok(()) => {
                        state.add_system_message(format!("→ model: {}", model_id));
                        state.footer_state.data.model_name = model_id.clone();
                        crate::settings::Settings::save_last_used(&model_id);
                    }
                    Err(e) => {
                        state.add_system_message(format!("✗ {}", e));
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

async fn handle_logout_select_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
) -> Option<Action> {
    let (providers, selected) = match &state.overlay {
        Some(AppOverlay::LogoutSelect { providers, selected }) => (providers.clone(), *selected),
        _ => return None,
    };

    match key.code {
        KeyCode::Up => {
            let new_sel = if selected == 0 { providers.len().saturating_sub(1) } else { selected - 1 };
            state.overlay = Some(AppOverlay::LogoutSelect { providers, selected: new_sel });
        }
        KeyCode::Down => {
            let new_sel = if providers.is_empty() { 0 } else { (selected + 1).min(providers.len() - 1) };
            state.overlay = Some(AppOverlay::LogoutSelect { providers, selected: new_sel });
        }
        KeyCode::Enter => {
            if let Some(provider) = providers.get(selected) {
                let auth = crate::auth_storage::AuthStorage::new();
                auth.remove(provider);
                state.add_system_message(format!("✓ Removed {}", provider));
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
        Some(AppOverlay::Setup(SetupStep::EnterApiKey { .. })) |
        Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey { .. })) => {
            // Paste text into the key field
            let key_field = match &mut state.overlay {
                Some(AppOverlay::Setup(SetupStep::EnterApiKey { key, .. })) => key as &mut String,
                Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey { key, .. })) => key as &mut String,
                _ => return None,
            };
            key_field.push_str(text);
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
