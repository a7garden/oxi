//! Main TUI event loop and application state.

use super::handlers;
use super::render;
use super::slash;
use super::welcome;
use crate::agent_session::{CompactionReason, SessionEvent};
use crate::agent_session_runtime::{
    create_agent_session_from_services, create_agent_session_services,
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
};
use crate::session::SessionManager;
use crate::slash_commands::BUILTIN_SLASH_COMMANDS;
use anyhow::Result;
use oxi_agent::AgentEvent;
use oxi_tui::theme::Theme;
use oxi_tui::widgets::{
    chat::{ChatMessage, ChatViewState, ContentBlock, MessageRole},
    footer::FooterState,
    input::InputState,
};
use std::io;
use std::sync::Arc;
use tokio::sync::mpsc;

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use crossterm::event::{EnableBracketedPaste, DisableBracketedPaste};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};

// ── IME-friendly raw mode ───────────────────────────────────────────────
// crossterm의 enable_raw_mode()는 cfmakeraw()를 호출하여 ICANON, ECHO, ISIG,
// IEXTEN 등을 모두 끕니다. 이렇게 하면 터미널 에뮬레이터의 IME(한국어, 일본어,
// 중국어 입력기)가 비활성화됩니다.
//
// 대신 ICANON과 ECHO만 끄고 나머지 플래그는 유지하면:
// - IME가 정상 동작 (터미널 에뮬레이터가 조합된 문자를 전달)
// - 비동기 입력 (read가 블록되지 않음)
// - 특수키(방향키, Enter 등)는 escape sequence로 전달

use std::os::fd::AsRawFd;

/// IME 친화적 부분 raw mode 활성화.
/// ICANON(라인 버퍼링)과 ECHO(화면 반향)만 끄고, IEXTEN/ISIG 등은 유지합니다.
fn enable_ime_friendly_raw_mode() -> io::Result<()> {
    use std::mem;
    // crossterm의 enable_raw_mode가 이미 호출되었는지 확인
    // 이미 활성화되어 있으면 아무것도 하지 않음
    if crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
        return Ok(());
    }

    // crossterm 내부의 original mode 저장 메커니즘을 우회하기 위해
    // crossterm의 enable_raw_mode를 먼저 호출하고, 그 다음 IME 친화적으로 덮어씁니다.
    enable_raw_mode()?;

    // 이제 IME 친화적으로 termios를 조정
    unsafe {
        let mut termios: libc::termios = mem::zeroed();
        if libc::tcgetattr(0, &mut termios) == 0 {
            // ICANON과 ECHO만 끄기 (OPOST는 켜둠 — 출력 처리 유지)
            termios.c_iflag &= !(libc::IGNBRK | libc::BRKINT | libc::PARMRK
                | libc::ISTRIP | libc::INLCR | libc::IGNCR | libc::ICRNL);
            // cfmakeraw는 IEXTEN도 끄지만, 우리는 IEXTEN을 유지 (IME에 필요)
            termios.c_oflag |= libc::OPOST; // 출력 처리 유지
            termios.c_lflag &= !(libc::ICANON | libc::ECHO);
            // ISIG 유지 (Ctrl+C 등 시그널 처리)
            // IEXTEN 유지 (IME 입력 처리)

            termios.c_cc[libc::VMIN] = 1;
            termios.c_cc[libc::VTIME] = 0;

            libc::tcsetattr(0, libc::TCSANOW, &termios);
        }
    }
    Ok(())
}

/// IME 친화적 raw mode 비활성화.
fn disable_ime_friendly_raw_mode() -> io::Result<()> {
    // crossterm의 disable_raw_mode가 original mode를 복원
    disable_raw_mode()
}
pub(crate) enum UiEvent {
    Start,
    Thinking,
    TextDelta(String),
    #[allow(dead_code)]
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    ToolStart {
        tool_name: String,
    },
    ToolResult {
        tool_name: String,
        content: String,
        is_error: bool,
    },
    Complete,
    Error(String),
    CompactionStart {
        reason: CompactionReason,
    },
    CompactionEnd {
        _reason: CompactionReason,
        error_message: Option<String>,
    },
    RetryStart {
        attempt: u32,
        max_attempts: u32,
        error_message: String,
    },
    ModelChanged {
        model_id: String,
    },
    ThinkingLevelChanged {
        level: String,
    },
    QueueUpdate {
        pending: usize,
    },
}

// ── Spinner ──────────────────────────────────────────────────────────────

pub(super) const SPINNER: &[&str] = &[("⠋"), ("⠙"), ("⠹"), ("⠸"), ("⠼"), ("⠴"), ("⠦"), ("⠧"), ("⠇"), ("⠏")];

// ── App State ────────────────────────────────────────────────────────────

/// Unified application state holding all widget states.
pub(crate) struct AppState {
    pub chat: ChatViewState,
    pub input: InputState,
    pub footer_state: FooterState,
    pub is_agent_busy: bool,
    pub spinner_frame: usize,
    pub auto_scroll: bool,
    pub input_history: Vec<String>,
    pub history_index: usize,
    pub saved_input: String,
    pub slash_completions: Vec<slash::SlashCompletion>,
    pub slash_completion_index: usize,
    pub slash_completion_active: bool,
    pub message_count: usize,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            chat: ChatViewState::default(),
            input: InputState::default(),
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

    // ── Input helpers ──

    pub fn input_value(&self) -> &str {
        &self.input.text
    }

    pub fn input_clear(&mut self) {
        self.input.clear();
        self.clear_slash_completions();
    }

    pub fn input_set_text(&mut self, text: String) {
        self.input.text = text;
        self.input.cursor = self.input.text.chars().count();
    }

    pub fn clear_slash_completions(&mut self) {
        self.slash_completions.clear();
        self.slash_completion_index = 0;
        self.slash_completion_active = false;
    }

    pub fn update_slash_completions(&mut self) {
        let text = self.input_value().trim();
        if !text.starts_with('/') || text.contains(' ') {
            self.clear_slash_completions();
            return;
        }
        let cmd_part = text.split_whitespace().next().unwrap_or("");
        let query = &cmd_part[1..];
        let mut matches: Vec<slash::SlashCompletion> = BUILTIN_SLASH_COMMANDS
            .iter()
            .filter(|cmd| query.is_empty() || cmd.name.starts_with(query))
            .map(|cmd| slash::SlashCompletion {
                name: format!("/{}", cmd.name),
                description: cmd.description.to_string(),
            })
            .collect();
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        self.slash_completions = matches;
        self.slash_completion_index = 0;
        self.slash_completion_active = !self.slash_completions.is_empty();
    }

    pub fn accept_slash_completion(&mut self) -> bool {
        if !self.slash_completion_active || self.slash_completions.is_empty() {
            return false;
        }
        let completion = &self.slash_completions[self.slash_completion_index];
        self.input_set_text(completion.name.clone());
        self.clear_slash_completions();
        true
    }

    pub fn next_slash_completion(&mut self) {
        if !self.slash_completions.is_empty() {
            self.slash_completion_index =
                (self.slash_completion_index + 1) % self.slash_completions.len();
        }
    }

    pub fn prev_slash_completion(&mut self) {
        if !self.slash_completions.is_empty() {
            if self.slash_completion_index == 0 {
                self.slash_completion_index = self.slash_completions.len() - 1;
            } else {
                self.slash_completion_index -= 1;
            }
        }
    }

    // ── Chat helpers ──

    pub fn add_user_message(&mut self, content: String) {
        self.chat.add_message(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text { content }],
            timestamp: now_millis(),
        });
        self.message_count += 1;
    }

    pub fn add_system_message(&mut self, content: String) {
        self.chat.add_message(ChatMessage {
            role: MessageRole::System,
            content_blocks: vec![ContentBlock::Text { content }],
            timestamp: now_millis(),
        });
    }

    pub fn start_streaming(&mut self) {
        self.chat.start_streaming();
        self.is_agent_busy = true;
        self.auto_scroll = true;
    }

    pub fn stream_text_delta(&mut self, delta: &str) {
        self.chat.stream_text_delta(delta);
    }

    pub fn finish_streaming(&mut self) {
        let was_streaming = self.chat.is_streaming();
        self.chat.finish_streaming();
        self.is_agent_busy = false;
        if was_streaming {
            self.message_count += 1;
        }
    }

    pub fn cancel_streaming(&mut self) {
        if self.chat.is_streaming() {
            self.chat.finish_streaming();
            self.message_count += 1;
        }
        self.is_agent_busy = false;
    }

    pub fn scroll_up(&mut self, n: u16) {
        self.chat.scroll_up(n);
        self.auto_scroll = false;
    }

    pub fn scroll_down(&mut self, n: u16) {
        self.chat.scroll_down(n);
    }

    pub fn ensure_auto_scroll(&mut self, visible_height: u16) {
        if self.auto_scroll {
            self.chat.scroll_to_bottom(visible_height);
        }
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.chat.messages
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ── Main entry point ─────────────────────────────────────────────────────

/// Run the TUI interactive mode.
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

    // Agent worker thread
    let session_handle = agent_session.clone_handle();
    let ui_tx_for_thread = ui_tx.clone();
    let agent_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build agent runtime");
        rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local
                .run_until(async {
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
                })
                .await;
        });
    });

    // Setup terminal — IME 친화적 부분 raw mode
    // crossterm의 enable_raw_mode()는 cfmakeraw()를 호출하여 IME를 완전히 비활성화합니다.
    // 대신 ICANON만 끄고 나머지는 유지하는 부분 raw mode를 사용합니다.
    enable_ime_friendly_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        )
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let theme = Theme::dark();
    let mut state = AppState::new();
    let model_id = agent_session.model_id();
    let git_branch =
        crate::git_utils::get_current_branch(&std::env::current_dir().unwrap_or_default());

    state.footer_state.data.pwd = Some(cwd.clone());
    state.footer_state.data.model_name = model_id.clone();
    state.footer_state.data.git_branch = git_branch.clone();
    state.add_system_message(welcome::format_welcome(&session_id, &model_id));

    let mut running = true;
    let mut last_spinner_tick = std::time::Instant::now();
    let poll_timeout = std::time::Duration::from_millis(50);

    while running {
        // Advance spinner
        let now = std::time::Instant::now();
        if now.duration_since(last_spinner_tick).as_millis() >= 80 {
            state.spinner_frame = (state.spinner_frame + 1) % SPINNER.len();
            last_spinner_tick = now;
        }

        // Render
        terminal.draw(|f| render::draw(f, &mut state, &theme))?;

        // Poll input events
        if event::poll(poll_timeout)? {
            if let Some(action) =
                handlers::handle_input(event::read()?, &mut state, &agent_session, &ui_tx, &prompt_tx, &mut running).await
            {
                match action {
                    handlers::Action::SendPrompt(value) => {
                        state.add_user_message(value.clone());
                        state.input_history.insert(0, value.clone());
                        if state.input_history.len() > 100 {
                            state.input_history.pop();
                        }
                        state.history_index = 0;
                        state.start_streaming();
                        let _ = prompt_tx.send(value).await;
                        state.input_clear();
                    }
                    // no action
                }
            }
        }

        // Drain agent events
        while let Ok(ui_event) = ui_rx.try_recv() {
            handlers::handle_ui_event(ui_event, &mut state);
        }

        // Drain session events
        while let Ok(session_event) = session_event_rx.try_recv() {
            handlers::handle_session_event(session_event, &ui_tx).await;
        }

        // Auto-scroll
        let chat_visible_height = {
            let size = terminal.size()?;
            size.height.saturating_sub(5)
        };
        state.ensure_auto_scroll(chat_visible_height);
    }

    // Cleanup
    drop(prompt_tx);
    disable_ime_friendly_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    let _ = agent_handle.join();
    Ok(())
}
