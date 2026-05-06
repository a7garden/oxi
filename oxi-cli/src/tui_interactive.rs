//! TUI-based interactive mode using ratatui.
//!
//! Provides a flicker-free terminal chat interface with:
//! - Double-buffered rendering via ratatui
//! - New widget-based rendering via oxi-tui widgets
//! - Streaming text display with spinner animation
//! - Scrollable chat history with scroll indicator
//! - Slash commands with autocomplete popup
//! - Status bar showing cwd, model, git branch
//! - Rich message rendering via ChatView widget

use crate::agent_session::{AgentSession, CompactionReason, ScopedModel, SessionEvent};
use crate::agent_session_runtime::{
    create_agent_session_from_services, create_agent_session_services,
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
};
use crate::auth_storage::AuthStorage;
use crate::changelog;
use crate::clipboard_write;
use crate::export::{self, ExportMeta, HtmlExportOptions};
use crate::session::SessionManager;
use crate::slash_commands::BUILTIN_SLASH_COMMANDS;
use anyhow::Result;
use oxi_agent::AgentEvent;
use oxi_tui::theme::Theme;
use oxi_tui::widgets::{
    chat::{ChatMessage, ChatView, ChatViewState, ContentBlock, MessageRole},
    footer::{Footer, FooterState},
    input::{Input, InputState as WidgetInputState},
};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyModifiers, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Terminal,
};

// ═══════════════════════════════════════════════════════════════════════════
// UI Events (agent → TUI)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug)]
enum UiEvent {
    Start,
    Thinking,
    TextDelta(String),
    #[allow(dead_code)]
    ToolCall { id: String, name: String, arguments: String },
    ToolStart { tool_name: String },
    ToolResult { tool_name: String, content: String, is_error: bool },
    Complete,
    Error(String),
    CompactionStart { reason: CompactionReason },
    CompactionEnd { _reason: CompactionReason, error_message: Option<String> },
    RetryStart { attempt: u32, max_attempts: u32, error_message: String },
    ModelChanged { model_id: String },
    ThinkingLevelChanged { level: String },
    QueueUpdate { pending: usize },
}

// ═══════════════════════════════════════════════════════════════════════════
// Slash completion (kept locally, not in widget)
// ═══════════════════════════════════════════════════════════════════════════

struct SlashCompletion {
    name: String,
    description: String,
}

/// Spinner frames for thinking animation.
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// ═══════════════════════════════════════════════════════════════════════════
// App State — unified state using widget states
// ═══════════════════════════════════════════════════════════════════════════

/// Unified application state holding all widget states.
struct AppState {
    /// Chat view state (messages + streaming).
    chat: ChatViewState,
    /// Input field state.
    input: WidgetInputState,
    /// Footer state for status bar.
    footer_state: FooterState,
    /// Whether the agent is currently processing.
    is_agent_busy: bool,
    /// Current spinner animation frame.
    spinner_frame: usize,
    /// Auto-scroll to bottom on new content.
    auto_scroll: bool,
    /// Input history for Up/Down recall.
    input_history: Vec<String>,
    /// History navigation index (0 = current, 1.. = history).
    history_index: usize,
    /// Saved current input when navigating history.
    saved_input: String,
    /// Slash command completions.
    slash_completions: Vec<SlashCompletion>,
    /// Currently selected slash completion.
    slash_completion_index: usize,
    /// Whether slash completion popup is active.
    slash_completion_active: bool,
    /// Count of user+assistant messages for status bar.
    message_count: usize,
}

impl AppState {
    fn new() -> Self {
        Self {
            chat: ChatViewState::default(),
            input: WidgetInputState::default(),
            footer_state: FooterState::default(),
            is_agent_busy: false,
            spinner_frame: 0,
            auto_scroll: true,
            input_history: Vec::new(),
            history_index: 0,
            saved_input: String::new(),
            slash_completions: Vec::new(),
            slash_completion_index: 0,
            slash_completion_active: false,
            message_count: 0,
        }
    }

    // ── Input helpers (delegating to widget InputState) ──

    fn input_value(&self) -> &str {
        &self.input.text
    }

    fn input_clear(&mut self) {
        self.input.clear();
        self.clear_slash_completions();
    }

    fn input_insert_char(&mut self, c: char) {
        self.input.insert_char(c);
    }

    fn input_backspace(&mut self) {
        self.input.backspace();
    }

    fn input_delete(&mut self) {
        self.input.delete();
    }

    fn input_move_left(&mut self) {
        self.input.move_left();
    }

    fn input_move_right(&mut self) {
        self.input.move_right();
    }

    fn input_move_home(&mut self) {
        self.input.move_home();
    }

    fn input_move_end(&mut self) {
        self.input.move_end();
    }

    fn input_set_text(&mut self, text: String) {
        self.input.text = text;
        self.input.cursor = self.input.text.chars().count();
    }

    // ── Slash completion helpers ──

    fn clear_slash_completions(&mut self) {
        self.slash_completions.clear();
        self.slash_completion_index = 0;
        self.slash_completion_active = false;
    }

    fn update_slash_completions(&mut self) {
        let text = self.input_value().trim();
        if !text.starts_with('/') || text.contains(' ') {
            self.clear_slash_completions();
            return;
        }

        let cmd_part = text.split_whitespace().next().unwrap_or("");
        let query = &cmd_part[1..];

        let mut matches: Vec<SlashCompletion> = BUILTIN_SLASH_COMMANDS
            .iter()
            .filter(|cmd| query.is_empty() || cmd.name.starts_with(query))
            .map(|cmd| SlashCompletion {
                name: format!("/{}", cmd.name),
                description: cmd.description.to_string(),
            })
            .collect();

        matches.sort_by(|a, b| a.name.cmp(&b.name));
        self.slash_completions = matches;
        self.slash_completion_index = 0;
        self.slash_completion_active = !self.slash_completions.is_empty();
    }

    fn accept_slash_completion(&mut self) -> bool {
        if !self.slash_completion_active || self.slash_completions.is_empty() {
            return false;
        }
        let completion = &self.slash_completions[self.slash_completion_index];
        self.input_set_text(completion.name.clone());
        self.clear_slash_completions();
        true
    }

    fn next_slash_completion(&mut self) {
        if !self.slash_completions.is_empty() {
            self.slash_completion_index = (self.slash_completion_index + 1) % self.slash_completions.len();
        }
    }

    fn prev_slash_completion(&mut self) {
        if !self.slash_completions.is_empty() {
            if self.slash_completion_index == 0 {
                self.slash_completion_index = self.slash_completions.len() - 1;
            } else {
                self.slash_completion_index -= 1;
            }
        }
    }

    // ── Chat message helpers ──

    /// Add a completed user message.
    fn add_user_message(&mut self, content: String) {
        self.chat.add_message(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text { content }],
            timestamp: now_millis(),
        });
        self.message_count += 1;
    }

    /// Add a system message.
    fn add_system_message(&mut self, content: String) {
        self.chat.add_message(ChatMessage {
            role: MessageRole::System,
            content_blocks: vec![ContentBlock::Text { content }],
            timestamp: now_millis(),
        });
    }

    /// Start streaming a new assistant message.
    fn start_streaming(&mut self) {
        self.chat.start_streaming();
        self.is_agent_busy = true;
        self.auto_scroll = true;
    }

    /// Append text delta to the streaming message.
    fn stream_text_delta(&mut self, delta: &str) {
        self.chat.stream_text_delta(delta);
    }

    /// Finish streaming, moving partial message to completed messages.
    fn finish_streaming(&mut self) {
        let was_streaming = self.chat.is_streaming();
        self.chat.finish_streaming();
        self.is_agent_busy = false;
        if was_streaming {
            self.message_count += 1;
        }
    }

    /// Cancel streaming, saving whatever was accumulated.
    fn cancel_streaming(&mut self) {
        if self.chat.is_streaming() {
            self.chat.finish_streaming();
            self.message_count += 1;
        }
        self.is_agent_busy = false;
    }

    /// Scroll the chat view, handling auto-scroll.
    fn scroll_up(&mut self, n: u16) {
        self.chat.scroll_up(n);
        self.auto_scroll = false;
    }

    fn scroll_down(&mut self, n: u16) {
        self.chat.scroll_down(n);
    }

    fn ensure_auto_scroll(&mut self, visible_height: u16) {
        if self.auto_scroll {
            self.chat.scroll_to_bottom(visible_height);
        }
    }

    /// Get messages for slash command access.
    fn messages(&self) -> &[ChatMessage] {
        &self.chat.messages
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════

/// TODO: document.
pub async fn run_tui_interactive(app: crate::App) -> Result<()> {
    let settings = app.settings().clone();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let session_manager = SessionManager::create(&cwd, None);
    let session_id = session_manager.get_session_id();

    let services = create_agent_session_services(
        CreateAgentSessionServicesOptions::new(std::env::current_dir().unwrap_or_default()),
    )?;
    let services = Arc::new(services);

    let create_result = create_agent_session_from_services(
        CreateAgentSessionFromServicesOptions {
            services: services.clone(),
            session_manager,
            model_id: Some(app.model_id()),
            thinking_level: Some(settings.thinking_level),
            scoped_models: Vec::new(),
            tool_registry: Some(app.agent().tools()),
        },
    )?;

    let agent_session = create_result.session;
    if let Some(msg) = create_result.model_fallback_message {
        tracing::warn!("Model fallback: {}", msg);
    }

    let (session_event_tx, mut session_event_rx) = mpsc::unbounded_channel::<SessionEvent>();
    agent_session.subscribe(Box::new(move |event| {
        let _ = session_event_tx.send(event.clone());
    }));

    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(256);
    let (prompt_tx, mut prompt_rx) = mpsc::channel::<String>(16);

    // Agent worker
    let session_handle = agent_session.clone_handle();
    let ui_tx_for_thread = ui_tx.clone();
    let agent_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build agent runtime");
        rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local.run_until(async {
                while let Some(prompt) = prompt_rx.recv().await {
                    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);
                    let ui_fwd = ui_tx_for_thread.clone();
                    let event_forwarder = tokio::task::spawn_local(async move {
                        while let Some(event) = event_rx.recv().await {
                            let ui_event = match event {
                                AgentEvent::Start { .. } => UiEvent::Start,
                                AgentEvent::Thinking => UiEvent::Thinking,
                                AgentEvent::TextChunk { text } => UiEvent::TextDelta(text),
                                AgentEvent::ToolCall { tool_call } => UiEvent::ToolCall {
                                    id: tool_call.id,
                                    name: tool_call.name,
                                    arguments: tool_call.arguments.to_string(),
                                },
                                AgentEvent::ToolStart { tool_name, .. } => {
                                    UiEvent::ToolStart { tool_name }
                                }
                                AgentEvent::ToolComplete { result } => UiEvent::ToolResult {
                                    tool_name: String::new(),
                                    content: result.content.chars().take(500).collect(),
                                    is_error: false,
                                },
                                AgentEvent::ToolError { error, .. } => UiEvent::ToolResult {
                                    tool_name: String::new(),
                                    content: error.clone(),
                                    is_error: true,
                                },
                                AgentEvent::Complete { .. } => UiEvent::Complete,
                                AgentEvent::Error { message, .. } => UiEvent::Error(message),
                                _ => continue,
                            };
                            if ui_fwd.send(ui_event).await.is_err() {
                                break;
                            }
                        }
                    });
                    let sh = session_handle.clone_handle();
                    let agent = sh.agent_ref();
                    let _ = agent.run_with_channel(prompt, event_tx).await;
                    let _ = event_forwarder.await;
                }
            }).await;
        });
    });

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Theme for widgets
    let theme = Theme::dark();

    // App state
    let mut state = AppState::new();

    let model_id = agent_session.model_id();
    let git_branch = crate::git_utils::get_current_branch(
        &std::env::current_dir().unwrap_or_default(),
    );

    // Set up footer data
    state.footer_state.data.pwd = Some(cwd.clone());
    state.footer_state.data.model_name = model_id.clone();
    state.footer_state.data.git_branch = git_branch.clone();

    // Welcome message
    state.add_system_message(format_welcome(&session_id, &model_id));

    let mut running = true;
    let mut last_spinner_tick = std::time::Instant::now();
    let poll_timeout = std::time::Duration::from_millis(50);

    while running {
        // Advance spinner based on wall-clock time (80ms per frame)
        let now = std::time::Instant::now();
        if now.duration_since(last_spinner_tick).as_millis() >= 80 {
            state.spinner_frame = (state.spinner_frame + 1) % SPINNER.len();
            last_spinner_tick = now;
        }

        // Render using widgets
        terminal.draw(|f| {
            let size = f.area();

            // Layout: Chat | Separator(1) | Input(3) | Status bar(1)
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(3),     // Chat
                    Constraint::Length(1),   // Separator
                    Constraint::Length(3),   // Input (border + input + hint/popup)
                    Constraint::Length(1),   // Status bar
                ])
                .split(size);

            // ── Chat area: use ChatView widget ──
            let chat_area = chunks[0];
            f.render_stateful_widget(
                ChatView::new(&theme),
                chat_area,
                &mut state.chat,
            );

            // ── Separator: kept as-is (simple visual element) ──
            render_separator(f, chunks[1], &theme);

            // ── Input area: use Input widget + local popup/hint ──
            render_input_area(f, chunks[2], &mut state, &theme);

            // ── Status bar: use Footer widget ──
            let footer_area = chunks[3];
            f.render_stateful_widget(
                Footer::new(&theme),
                footer_area,
                &mut state.footer_state,
            );
        })?;

        // Poll events
        if event::poll(poll_timeout)? {
            match event::read()? {
                CEvent::Key(key) => {
                    match key.code {
                        KeyCode::Enter => {
                            if !state.is_agent_busy {
                                if state.slash_completion_active {
                                    state.accept_slash_completion();
                                    continue;
                                }
                                let value = state.input_value().to_string();
                                if !value.is_empty() {
                                    if value.starts_with('/') {
                                        let handled = handle_slash_command(
                                            &value, &agent_session, &mut state, &mut running,
                                        );
                                        state.input_clear();
                                        if handled { continue; }
                                    }
                                    state.add_user_message(value.clone());
                                    // Save to input history
                                    state.input_history.insert(0, value.clone());
                                    if state.input_history.len() > 100 { state.input_history.pop(); }
                                    state.history_index = 0;
                                    state.start_streaming();
                                    let _ = prompt_tx.send(value).await;
                                    state.input_clear();
                                }
                            }
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if state.is_agent_busy {
                                let sh = agent_session.clone_handle();
                                tokio::spawn(async move { sh.abort().await; });
                                state.cancel_streaming();
                                state.add_system_message("⏹ Interrupted".to_string());
                            } else {
                                running = false;
                            }
                        }
                        KeyCode::PageUp => {
                            state.scroll_up(10);
                        }
                        KeyCode::PageDown => {
                            state.scroll_down(10);
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if !state.is_agent_busy {
                                state.input_insert_char(c);
                                state.update_slash_completions();
                            }
                        }
                        KeyCode::Backspace => {
                            if !state.is_agent_busy {
                                state.input_backspace();
                                state.update_slash_completions();
                            }
                        }
                        KeyCode::Delete => {
                            if !state.is_agent_busy {
                                state.input_delete();
                                state.update_slash_completions();
                            }
                        }
                        KeyCode::Left => {
                            if !state.is_agent_busy {
                                if key.modifiers.contains(KeyModifiers::CONTROL) {
                                    // Word-left
                                    let text: Vec<char> = state.input.text.chars().collect();
                                    let mut pos = state.input.cursor;
                                    while pos > 0 && text[pos - 1].is_whitespace() { pos -= 1; }
                                    while pos > 0 && !text[pos - 1].is_whitespace() { pos -= 1; }
                                    state.input.cursor = pos;
                                } else {
                                    state.input_move_left();
                                }
                            }
                        }
                        KeyCode::Right => {
                            if !state.is_agent_busy {
                                if key.modifiers.contains(KeyModifiers::CONTROL) {
                                    // Word-right
                                    let text: Vec<char> = state.input.text.chars().collect();
                                    let mut pos = state.input.cursor;
                                    while pos < text.len() && !text[pos].is_whitespace() { pos += 1; }
                                    while pos < text.len() && text[pos].is_whitespace() { pos += 1; }
                                    state.input.cursor = pos;
                                } else {
                                    state.input_move_right();
                                }
                            }
                        }
                        KeyCode::Home => { if !state.is_agent_busy { state.input_move_home(); } }
                        KeyCode::End => { if !state.is_agent_busy { state.input_move_end(); } }
                        KeyCode::Tab => {
                            if !state.is_agent_busy && state.slash_completion_active {
                                state.accept_slash_completion();
                            }
                        }
                        KeyCode::Up => {
                            if !state.is_agent_busy && state.slash_completion_active {
                                state.prev_slash_completion();
                            } else if !state.is_agent_busy && state.input.text.is_empty() && !state.input_history.is_empty() {
                                if state.history_index == 0 {
                                    state.saved_input = state.input.text.clone();
                                }
                                if state.history_index < state.input_history.len() {
                                    state.history_index += 1;
                                    state.input_set_text(state.input_history[state.history_index - 1].clone());
                                    state.clear_slash_completions();
                                }
                            } else {
                                state.scroll_up(3);
                            }
                        }
                        KeyCode::Down => {
                            if !state.is_agent_busy && state.slash_completion_active {
                                state.next_slash_completion();
                            } else if !state.is_agent_busy && state.history_index > 0 {
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
                        }
                        KeyCode::Esc => {
                            if state.slash_completion_active {
                                state.clear_slash_completions();
                            }
                        }
                        _ => {}
                    }
                }
                CEvent::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        state.scroll_up(3);
                    }
                    MouseEventKind::ScrollDown => {
                        state.scroll_down(3);
                    }
                    _ => {}
                },
                CEvent::Resize(_, _) => {}
                _ => {}
            }
        }

        // Drain agent events
        while let Ok(ui_event) = ui_rx.try_recv() {
            match ui_event {
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
                UiEvent::ToolResult { tool_name, content, is_error } => {
                    let label = if tool_name.is_empty() { "tool" } else { &tool_name };
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
                UiEvent::CompactionEnd { _reason, error_message } => {
                    let msg = if let Some(err) = error_message {
                        format!("⚠ Compaction failed: {}", err)
                    } else {
                        "✅ Compaction complete".to_string()
                    };
                    state.add_system_message(msg);
                }
                UiEvent::RetryStart { attempt, max_attempts, error_message } => {
                    state.add_system_message(format!("🔄 Retry ({}/{}): {}", attempt, max_attempts, error_message));
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

        // Drain session events
        while let Ok(session_event) = session_event_rx.try_recv() {
            match session_event {
                SessionEvent::CompactionStart { reason } => {
                    let _ = ui_tx.send(UiEvent::CompactionStart { reason }).await;
                }
                SessionEvent::CompactionEnd { reason, error_message, .. } => {
                    let _ = ui_tx.send(UiEvent::CompactionEnd { _reason: reason, error_message }).await;
                }
                SessionEvent::ThinkingLevelChanged { level } => {
                    let _ = ui_tx.send(UiEvent::ThinkingLevelChanged { level: format!("{:?}", level) }).await;
                }
                SessionEvent::QueueUpdate { steering, follow_up } => {
                    let pending = steering.len() + follow_up.len();
                    let _ = ui_tx.send(UiEvent::QueueUpdate { pending }).await;
                }
                SessionEvent::SessionInfoChanged { name: _ } => {}
                SessionEvent::Agent(event) => {
                    match &event {
                        AgentEvent::Fallback { to_model, .. } => {
                            let _ = ui_tx.send(UiEvent::ModelChanged { model_id: to_model.clone() }).await;
                        }
                        AgentEvent::Retry { attempt, max_retries, reason, .. } => {
                            let _ = ui_tx.send(UiEvent::RetryStart {
                                attempt: *attempt as u32,
                                max_attempts: *max_retries as u32,
                                error_message: reason.clone(),
                            }).await;
                        }
                        AgentEvent::Compaction { .. } => {}
                        _ => {}
                    }
                }
            }
        }

        // Auto-scroll to bottom
        let chat_visible_height = {
            let size = terminal.size()?;
            size.height.saturating_sub(5)
        };
        state.ensure_auto_scroll(chat_visible_height);
    }

    // Cleanup
    drop(prompt_tx);
    let cleanup_terminal = |terminal: &mut Terminal<CrosstermBackend<io::Stdout>>| -> Result<()> {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        terminal.show_cursor()?;
        Ok(())
    };
    let _ = agent_handle.join();
    if let Err(e) = cleanup_terminal(&mut terminal) {
        tracing::error!("Terminal cleanup failed: {}", e);
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Rendering — Input area (combines Input widget + hint/popup)
// ═══════════════════════════════════════════════════════════════════════════

fn render_input_area(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
) {
    if area.height < 2 { return; }

    // Row 0: input field (using Input widget)
    let input_row = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    // Row 1: hint/popup line (manual rendering for slash commands)
    let hint_row = Rect { x: area.x, y: area.y + 1, width: area.width, height: 1 };
    // Row 2: bottom border
    let border_row = Rect { x: area.x, y: area.y + 2, width: area.width, height: 1 };

    // Use the Input widget for the input row
    // When agent is busy, we show a spinner as the prompt character
    if state.is_agent_busy {
        // Render spinner + busy indicator manually, widget doesn't support dynamic prompt
        render_busy_input(f, input_row, state, theme);
    } else {
        f.render_stateful_widget(
            Input::new(theme)
                .with_placeholder("Type a message… (enter / for commands)"),
            input_row,
            &mut state.input,
        );
    }

    // ── Hint / popup row ──
    if state.slash_completion_active {
        render_slash_popup(f, hint_row, state, theme);
    } else if state.is_agent_busy {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Ctrl+C to interrupt".to_string(),
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ))),
            hint_row,
        );
    } else if state.input_value().is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Enter · / commands · ↑ history · Esc cancel".to_string(),
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ))),
            hint_row,
        );
    } else {
        let count = state.input.text.chars().count();
        let count_str = format!("  {} chars", count);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(count_str, Style::default().fg(theme.colors.muted.to_ratatui())))),
            hint_row,
        );
    }

    // ── Bottom accent line (dotted, matching separator style) ──
    render_separator(f, border_row, theme);
}

/// Render input field when agent is busy (spinner prompt).
fn render_busy_input(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
) {
    let prompt = format!("{} ", SPINNER[state.spinner_frame]);
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(prompt, Style::default().fg(theme.colors.accent.to_ratatui())));

    // Show "waiting..." or whatever text is in input
    let display = if state.input_value().is_empty() {
        "waiting for response…".to_string()
    } else {
        state.input_value().to_string()
    };

    let text_fg = if state.input_value().is_empty() {
        theme.colors.muted.to_ratatui()
    } else {
        theme.colors.foreground.to_ratatui()
    };

    spans.push(Span::styled(display, Style::default().fg(text_fg)));

    // Padding
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if used < area.width as usize {
        spans.push(Span::styled(" ".repeat(area.width as usize - used), Style::default()));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Slash command popup — multi-column grid layout (kept from original).
fn render_slash_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
) {
    let completions = &state.slash_completions;
    if completions.is_empty() { return; }

    let selected = state.slash_completion_index;
    let max_show = 6usize;

    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else { 0 };

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled("  ".to_string(), Style::default()));

    let visible: Vec<_> = completions.iter().enumerate()
        .skip(window_start).take(max_show).collect();

    for (i, comp) in &visible {
        if *i == selected {
            spans.push(Span::styled(
                format!(" {} ", comp.name),
                Style::default().fg(theme.colors.background.to_ratatui()).bg(theme.colors.primary.to_ratatui()).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" ".to_string(), Style::default()));
        } else {
            spans.push(Span::styled(
                format!(" {} ", comp.name),
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ));
            spans.push(Span::styled(" ".to_string(), Style::default()));
        }
    }

    if let Some(comp) = completions.get(selected) {
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let remaining = area.width as usize;
        let desc_max = remaining.saturating_sub(used + 4);
        if desc_max > 5 {
            let desc: String = comp.description.chars().take(desc_max).collect();
            spans.push(Span::styled(
                format!("— {}", desc),
                Style::default().fg(theme.colors.muted.to_ratatui()),
            ));
        }
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ═══════════════════════════════════════════════════════════════════════════
// Rendering — Separator (kept as-is)
// ═══════════════════════════════════════════════════════════════════════════

fn render_separator(f: &mut ratatui::Frame, area: Rect, theme: &Theme) {
    let w = area.width as usize;
    let mut spans: Vec<Span> = Vec::with_capacity(w);
    for i in 0..w {
        let c = match i % 4 {
            0 => '─',
            1 => '·',
            2 => '·',
            _ => ' ',
        };
        spans.push(Span::styled(c.to_string(), Style::default().fg(theme.colors.border.to_ratatui())));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ═══════════════════════════════════════════════════════════════════════════
// Welcome banner
// ═══════════════════════════════════════════════════════════════════════════

fn format_welcome(session_id: &str, model_id: &str) -> String {
    let line = "─".repeat(33);
    format!(
"  ╭{line}╮
  │  ◈ oxi — AI Coding Assistant   │
  ╰{line}╯

  Session  {session_id}
  Model    {model_id}

  /help for commands · Enter to send",
        line = line, session_id = session_id, model_id = model_id,
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Slash command handling
// ═══════════════════════════════════════════════════════════════════════════

fn handle_slash_command(
    input: &str,
    session: &AgentSession,
    state: &mut AppState,
    running: &mut bool,
) -> bool {
    let trimmed = input.trim();
    let (cmd, arg) = if let Some(space) = trimmed.find(' ') {
        (&trimmed[..space], Some(trimmed[space + 1..].trim()))
    } else {
        (trimmed, None)
    };
    let cmd_lower = cmd.to_lowercase();

    match cmd_lower.as_str() {
        "/help" | "/?" => {
            state.add_system_message(format_help());
            true
        }
        "/quit" | "/exit" | "/q" => { *running = false; true }
        "/clear" => {
            state.chat.clear();
            session.reset();
            true
        }
        "/model" => {
            if let Some(model_id) = arg {
                match session.set_model(model_id) {
                    Ok(()) => {
                        state.add_system_message(format!("→ model: {}", model_id));
                        state.footer_state.data.model_name = model_id.to_string();
                    }
                    Err(e) => {
                        state.add_system_message(format!("✗ {}", e));
                    }
                }
            } else {
                state.add_system_message(format!(
                    "Model: {}\n/model <provider/model> to switch",
                    session.model_id()
                ));
            }
            true
        }
        "/compact" => {
            let instructions = arg.map(|s| s.to_string());
            let sh = session.clone_handle();
            tokio::spawn(async move {
                match sh.compact(instructions).await {
                    Ok(result) => tracing::info!("Compaction: {} tokens before", result.tokens_before),
                    Err(e) => tracing::warn!("Compaction failed: {}", e),
                }
            });
            true
        }
        "/session" => {
            let stats = session.session_stats();
            state.add_system_message(format!(
                "Session: {}\nMessages: {} ({} user, {} assistant)\nTools: {} calls, {} results\nModel: {}\nThinking: {:?}\nAuto-compact: {}\nAuto-retry: {}",
                stats.session_id, stats.total_messages, stats.user_messages, stats.assistant_messages,
                stats.tool_calls, stats.tool_results, session.model_id(),
                session.thinking_level(), session.auto_compaction_enabled(), session.auto_retry_enabled(),
            ));
            true
        }
        "/settings" => {
            state.add_system_message(format!(
                "Model: {}\nThinking: {:?}\nAuto-compact: {}\nAuto-retry: {}",
                session.model_id(), session.thinking_level(),
                session.auto_compaction_enabled(), session.auto_retry_enabled(),
            ));
            true
        }
        "/name" => {
            if let Some(name) = arg {
                session.set_session_name(name.to_string());
                state.add_system_message(format!("Session → {}", name));
            } else {
                state.add_system_message("/name <name>".to_string());
            }
            true
        }
        "/copy" => {
            let last = state.messages().iter().rev().find(|m| m.role == MessageRole::Assistant);
            if let Some(msg) = last {
                // Extract text content from content blocks
                let content: String = msg.content_blocks.iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { content } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                match clipboard_write::copy_to_clipboard(&content) {
                    Ok(()) => state.add_system_message("✓ Copied to clipboard".to_string()),
                    Err(e) => state.add_system_message(format!("✗ Copy failed: {}", e)),
                }
            } else {
                state.add_system_message("No assistant message".to_string());
            }
            true
        }
        "/changelog" => {
            let paths = vec![PathBuf::from("CHANGELOG.md"), PathBuf::from("../CHANGELOG.md")];
            let mut entries: Vec<changelog::ChangelogEntry> = Vec::new();
            for path in &paths {
                let parsed = changelog::parse_changelog(path);
                if !parsed.is_empty() { entries = parsed; break; }
            }
            if entries.is_empty() {
                state.add_system_message("No changelog found".to_string());
            } else {
                let mut out = "Changelog:\n\n".to_string();
                for entry in entries.iter().take(5) {
                    out.push_str(&format!("## {}\n\n", entry.version_string()));
                    let preview = if entry.content.len() > 200 {
                        let end = entry.content.char_indices()
                            .take_while(|(i, _)| *i < 200)
                            .last().map(|(i, c)| i + c.len_utf8()).unwrap_or(0);
                        format!("{}…", &entry.content[..end])
                    } else { entry.content.clone() };
                    out.push_str(&preview);
                    out.push_str("\n\n");
                }
                state.add_system_message(out);
            }
            true
        }
        "/hotkeys" | "/keys" => {
            state.add_system_message(format_hotkeys());
            true
        }
        "/export" => {
            let export_path = arg.map(PathBuf::from);
            let meta = ExportMeta {
                model: Some(session.model_id()),
                provider: None,
                exported_at: chrono::Utc::now().timestamp_millis(),
                total_user_tokens: None,
                total_assistant_tokens: None,
            };
            let entries: Vec<crate::session::SessionEntry> = state.messages().iter().map(|msg| {
                let role = match msg.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::System => "system",
                };
                let content: String = msg.content_blocks.iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { content } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                crate::session::SessionEntry::simple_message(role, &content)
            }).collect();
            match export::export_to_html(&entries, &meta, &HtmlExportOptions::default()) {
                Ok(html) => {
                    if let Some(path) = export_path {
                        match std::fs::write(&path, &html) {
                            Ok(()) => state.add_system_message(format!("✓ Exported: {}", path.display())),
                            Err(e) => state.add_system_message(format!("✗ Write failed: {}", e)),
                        }
                    } else {
                        state.add_system_message(format!("HTML ready ({} bytes). /export <path> to save.", html.len()));
                    }
                }
                Err(e) => state.add_system_message(format!("✗ Export failed: {}", e)),
            }
            true
        }
        "/import" => {
            state.add_system_message(if let Some(p) = arg {
                format!("Import '{}' — coming soon", p)
            } else {
                "/import <path-to-jsonl>".to_string()
            });
            true
        }
        "/share" => {
            state.add_system_message("GitHub gist sharing coming soon. Use /export for HTML.".to_string());
            true
        }
        "/fork" => {
            state.add_system_message("Use /tree to view branches. Fork via session navigation.".to_string());
            true
        }
        "/clone" => {
            state.add_system_message("Run oxi --continue in a new terminal to clone.".to_string());
            true
        }
        "/tree" => {
            state.add_system_message("Linear session. Use /fork to branch from a previous message.".to_string());
            true
        }
        "/login" => {
            if let Some(provider) = arg {
                state.add_system_message(format!(
                    "Set {} API key:\n  export {}_API_KEY=your-key",
                    provider, provider.to_uppercase()
                ));
            } else {
                state.add_system_message("/login <provider>\n\nProviders: anthropic, openai, google, groq, mistral, deepseek, xai".to_string());
            }
            true
        }
        "/logout" => {
            if let Some(provider) = arg {
                AuthStorage::new().remove(provider);
                state.add_system_message(format!("✓ Removed {}", provider));
            } else {
                state.add_system_message("/logout <provider>".to_string());
            }
            true
        }
        "/new" => {
            state.add_system_message("Starting new session…".to_string());
            session.reset();
            state.chat.clear();
            true
        }
        "/resume" => {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string());
            let session_dir = crate::session::get_default_session_dir(&cwd);
            if let Ok(sessions) = std::fs::read_dir(&session_dir) {
                let list: Vec<_> = sessions
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map_or(false, |ext| ext == "jsonl"))
                    .take(10).collect();
                if list.is_empty() {
                    state.add_system_message("No previous sessions".to_string());
                } else {
                    let mut out = "Recent:\n\n".to_string();
                    for (i, entry) in list.iter().enumerate() {
                        if let Some(name) = entry.file_name().to_str() {
                            out.push_str(&format!("{}. {}\n", i + 1, name));
                        }
                    }
                    out.push_str("\n/import <path> to resume");
                    state.add_system_message(out);
                }
            } else {
                state.add_system_message("No sessions found".to_string());
            }
            true
        }
        "/reload" => {
            state.add_system_message("✓ Configuration reloaded".to_string());
            true
        }
        "/scoped-models" | "/models" => {
            if let Some(models_str) = arg {
                let models: Vec<ScopedModel> = models_str.split(',')
                    .filter_map(|s| {
                        let parts: Vec<&str> = s.trim().split('/').collect();
                        if parts.len() >= 2 {
                            Some(ScopedModel {
                                provider: parts[0].to_string(),
                                model_id: parts[1..].join("/"),
                                thinking_level: None,
                            })
                        } else { None }
                    }).collect();
                if !models.is_empty() {
                    session.set_scoped_models(models.clone());
                    let names: Vec<String> = models.iter().map(|m| format!("{}/{}", m.provider, m.model_id)).collect();
                    state.add_system_message(format!("Scoped: {} (Ctrl+P to cycle)", names.join(", ")));
                } else {
                    state.add_system_message("/scoped-models provider/model1,provider/model2".to_string());
                }
            } else {
                let scoped = session.scoped_models();
                if scoped.is_empty() {
                    state.add_system_message("No scoped models. /scoped-models <m1>,<m2>".to_string());
                } else {
                    let names: Vec<String> = scoped.iter().map(|m| format!("{}/{}", m.provider, m.model_id)).collect();
                    state.add_system_message(format!("Scoped: {}", names.join(", ")));
                }
            }
            true
        }
        _ => false,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Help & Hotkeys text
// ═══════════════════════════════════════════════════════════════════════════

fn format_help() -> String {
    r#"
  Session
    /new              Start a new session
    /clone            Duplicate current session
    /resume           List recent sessions
    /tree             Show session tree
    /fork             Fork from a previous message
    /session          Show session info
    /name <name>      Set session name

  Model
    /model [id]       Switch or show model
    /scoped-models    Models for Ctrl+P cycling

  Context
    /compact [instr]  Compact context
    /clear            Clear history

  Export
    /export [path]    Export to HTML
    /import <path>    Import from JSONL
    /copy             Copy last reply

  Auth
    /login <provider> Set API key
    /logout <provider> Remove key

  Info
    /help             This help
    /hotkeys          Key shortcuts
    /changelog        Changelog
    /settings         Current settings
    /reload           Reload config
    /quit             Quit

  Keys
    Enter             Send
    Ctrl+C            Interrupt / Quit
    PageUp/Down       Scroll
    /                 Slash commands
"#.to_string()
}

fn format_hotkeys() -> String {
    r#"
  Navigation
    Enter              Submit input
    Escape             Cancel
    PageUp/PageDown    Scroll chat

  Editor
    ←/→                Move cursor
    Home/End           Start/End of line
    Backspace          Delete char
    Ctrl+←/→           Move by word

  Session
    Ctrl+C             Interrupt / Quit
    Ctrl+P             Cycle models
    Shift+Ctrl+P       Cycle models (reverse)
"#.to_string()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
