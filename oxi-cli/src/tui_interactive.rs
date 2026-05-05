//! TUI-based interactive mode using ratatui.
//!
//! Provides a flicker-free terminal chat interface with:
//! - Double-buffered rendering via ratatui
//! - Line-level differential updates (zero flicker)
//! - Streaming text display with spinner animation
//! - Scrollable chat history with scroll indicator
//! - Slash commands with autocomplete popup
//! - Status bar showing cwd, model, git branch, context usage
//! - Rich message rendering with role badges and visual hierarchy

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
    widgets::{Paragraph, Wrap},
    Terminal,
};

// ═══════════════════════════════════════════════════════════════════════════
// Color Palette — Tokyo Night inspired
// ═══════════════════════════════════════════════════════════════════════════

mod palette {
    #![allow(dead_code)]
    use ratatui::style::Color;

    // Background layers
    pub const BG:          Color = Color::Rgb(26, 27, 38);    // #1a1b26 base
    pub const BG_SURFACE:  Color = Color::Rgb(30, 32, 48);   // #1e2030 surface
    pub const BG_OVERLAY:  Color = Color::Rgb(36, 38, 56);   // #242638 overlay
    pub const BG_HOVER:    Color = Color::Rgb(55, 58, 82);   // #373a52 hover

    // Foreground
    pub const FG:          Color = Color::Rgb(169, 177, 214); // #a9b1d6 text
    pub const FG_DIM:      Color = Color::Rgb(88, 91, 112);  // #585b70 dimmed
    pub const FG_BRIGHT:   Color = Color::Rgb(205, 214, 244);// #cdd6f4 bright

    // Accents
    pub const BLUE:        Color = Color::Rgb(122, 162, 247); // #7aa2f7 blue
    pub const CYAN:        Color = Color::Rgb(125, 207, 255); // #7dcfff cyan
    pub const GREEN:       Color = Color::Rgb(158, 206, 106); // #9ece6a green
    pub const YELLOW:      Color = Color::Rgb(224, 175, 104); // #e0af68 yellow
    pub const RED:         Color = Color::Rgb(247, 118, 142); // #f7768e red
    pub const MAGENTA:     Color = Color::Rgb(187, 154, 247); // #bb9af7 magenta
    pub const ORANGE:      Color = Color::Rgb(255, 158, 100); // #ff9e64 orange
    pub const TEAL:        Color = Color::Rgb(84, 168, 160);  // #54a8a0 teal

    // Role colors
    pub const USER_BG:     Color = Color::Rgb(30, 40, 70);   // subtle blue bg
    pub const ASSISTANT_BG:Color = Color::Rgb(30, 35, 50);   // subtle dark bg
    pub const SYSTEM_BG:   Color = Color::Rgb(45, 35, 30);   // subtle warm bg
}

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
// Chat state
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageRole {
    User,
    Assistant,
    System,
}

/// A single chat message.
struct ChatMessage {
    role: MessageRole,
    content: String,
    #[allow(dead_code)]
    timestamp: i64,
}

/// Spinner frames for thinking animation.
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// State for the input field (char-index based, UTF-8 safe).
struct InputState {
    text: String,
    cursor: usize,
    slash_completions: Vec<SlashCompletion>,
    slash_completion_index: usize,
    slash_completion_active: bool,
}

struct SlashCompletion {
    name: String,
    description: String,
}

impl InputState {
    fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            slash_completions: Vec::new(),
            slash_completion_index: 0,
            slash_completion_active: false,
        }
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.clear_slash_completions();
    }

    fn clear_slash_completions(&mut self) {
        self.slash_completions.clear();
        self.slash_completion_index = 0;
        self.slash_completion_active = false;
    }

    fn update_slash_completions(&mut self) {
        let text = self.text.trim();
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
        self.text = completion.name.clone();
        self.cursor = self.text.chars().count();
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

    fn value(&self) -> &str { &self.text }

    fn insert_char(&mut self, c: char) {
        let byte_pos = self.char_to_byte(self.cursor);
        self.text.insert(byte_pos, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_pos = self.char_to_byte(self.cursor);
            self.text.remove(byte_pos);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.text.chars().count() {
            let byte_pos = self.char_to_byte(self.cursor);
            self.text.remove(byte_pos);
        }
    }

    fn move_left(&mut self) { if self.cursor > 0 { self.cursor -= 1; } }
    fn move_right(&mut self) { if self.cursor < self.text.chars().count() { self.cursor += 1; } }
    fn move_home(&mut self) { self.cursor = 0; }
    fn move_end(&mut self) { self.cursor = self.text.chars().count(); }

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.text.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(self.text.len())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════

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

    // App state
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut input = InputState::new();
    let mut is_agent_busy = false;
    let mut streaming_text = String::new();
    let mut scroll_offset: usize = 0;
    let mut auto_scroll: bool = true;
    let mut spinner_frame: usize = 0;
    let mut last_spinner_tick: std::time::Instant = std::time::Instant::now();
    let mut message_count: usize = 0; // user + assistant
    let mut input_history: Vec<String> = Vec::new();
    let mut history_index: usize = 0; // 0 = current, 1.. = history
    let mut saved_input: String = String::new(); // saved current input when entering history

    let model_id = agent_session.model_id();
    let git_branch = crate::git_utils::get_current_branch(
        &std::env::current_dir().unwrap_or_default(),
    );

    // Welcome
    messages.push(ChatMessage {
        role: MessageRole::System,
        content: format_welcome(&session_id, &model_id),
        timestamp: now_millis(),
    });

    let mut running = true;
    let poll_timeout = std::time::Duration::from_millis(50); // 50ms for smooth spinner

    while running {
        // Advance spinner based on wall-clock time (80ms per frame)
        let now = std::time::Instant::now();
        if now.duration_since(last_spinner_tick).as_millis() >= 80 {
            spinner_frame = (spinner_frame + 1) % SPINNER.len();
            last_spinner_tick = now;
        }

        // Render
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

            render_chat(f, chunks[0], &messages, &streaming_text, scroll_offset,
                        is_agent_busy, spinner_frame);
            render_separator(f, chunks[1]);
            render_input(f, chunks[2], &input, is_agent_busy, spinner_frame);
            render_status_bar(f, chunks[3], &cwd, &model_id,
                              git_branch.as_deref(), is_agent_busy, message_count);
        })?;

        // Poll events
        if event::poll(poll_timeout)? {
            match event::read()? {
                CEvent::Key(key) => {
                    match key.code {
                        KeyCode::Enter => {
                            if !is_agent_busy {
                                if input.slash_completion_active {
                                    input.accept_slash_completion();
                                    continue;
                                }
                                let value = input.value().to_string();
                                if !value.is_empty() {
                                    if value.starts_with('/') {
                                        let handled = handle_slash_command(
                                            &value, &agent_session, &mut messages, &mut running,
                                        );
                                        input.clear();
                                        if handled { continue; }
                                    }
                                    messages.push(ChatMessage {
                                        role: MessageRole::User,
                                        content: value.clone(),
                                        timestamp: now_millis(),
                                    });
                                    message_count += 1;
                                    // Save to input history
                                    input_history.insert(0, value.clone());
                                    if input_history.len() > 100 { input_history.pop(); }
                                    history_index = 0;
                                    streaming_text.clear();
                                    is_agent_busy = true;
                                    auto_scroll = true;
                                    let _ = prompt_tx.send(value).await;
                                    input.clear();
                                }
                            }
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if is_agent_busy {
                                let sh = agent_session.clone_handle();
                                tokio::spawn(async move { sh.abort().await; });
                                is_agent_busy = false;
                                if !streaming_text.is_empty() {
                                    messages.push(ChatMessage {
                                        role: MessageRole::Assistant,
                                        content: streaming_text.clone(),
                                        timestamp: now_millis(),
                                    });
                                    message_count += 1;
                                    streaming_text.clear();
                                }
                                messages.push(ChatMessage {
                                    role: MessageRole::System,
                                    content: "⏹ Interrupted".to_string(),
                                    timestamp: now_millis(),
                                });
                            } else {
                                running = false;
                            }
                        }
                        KeyCode::PageUp => {
                            scroll_offset = scroll_offset.saturating_add(10);
                            auto_scroll = false;
                        }
                        KeyCode::PageDown => {
                            scroll_offset = scroll_offset.saturating_sub(10);
                            if scroll_offset == 0 { auto_scroll = true; }
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if !is_agent_busy {
                                input.insert_char(c);
                                input.update_slash_completions();
                            }
                        }
                        KeyCode::Backspace => {
                            if !is_agent_busy {
                                input.backspace();
                                input.update_slash_completions();
                            }
                        }
                        KeyCode::Delete => {
                            if !is_agent_busy {
                                input.delete();
                                input.update_slash_completions();
                            }
                        }
                        KeyCode::Left => {
                            if !is_agent_busy {
                                if key.modifiers.contains(KeyModifiers::CONTROL) {
                                    // Word-left: find previous word boundary
                                    let text: Vec<char> = input.text.chars().collect();
                                    let mut pos = input.cursor;
                                    // Skip trailing spaces
                                    while pos > 0 && text[pos - 1].is_whitespace() { pos -= 1; }
                                    // Skip word chars
                                    while pos > 0 && !text[pos - 1].is_whitespace() { pos -= 1; }
                                    input.cursor = pos;
                                } else {
                                    input.move_left();
                                }
                            }
                        }
                        KeyCode::Right => {
                            if !is_agent_busy {
                                if key.modifiers.contains(KeyModifiers::CONTROL) {
                                    // Word-right: find next word boundary
                                    let text: Vec<char> = input.text.chars().collect();
                                    let mut pos = input.cursor;
                                    // Skip word chars
                                    while pos < text.len() && !text[pos].is_whitespace() { pos += 1; }
                                    // Skip spaces
                                    while pos < text.len() && text[pos].is_whitespace() { pos += 1; }
                                    input.cursor = pos;
                                } else {
                                    input.move_right();
                                }
                            }
                        }
                        KeyCode::Home => { if !is_agent_busy { input.move_home(); } }
                        KeyCode::End => { if !is_agent_busy { input.move_end(); } }
                        KeyCode::Tab => {
                            if !is_agent_busy && input.slash_completion_active {
                                input.accept_slash_completion();
                            }
                        }
                        KeyCode::Up => {
                            if !is_agent_busy && input.slash_completion_active {
                                input.prev_slash_completion();
                            } else if !is_agent_busy && input.text.is_empty() && !input_history.is_empty() {
                                // Input history: recall previous message
                                if history_index == 0 {
                                    saved_input = input.text.clone();
                                }
                                if history_index < input_history.len() {
                                    history_index += 1;
                                    input.text = input_history[history_index - 1].clone();
                                    input.cursor = input.text.chars().count();
                                    input.clear_slash_completions();
                                }
                            } else {
                                scroll_offset = scroll_offset.saturating_add(3);
                                auto_scroll = false;
                            }
                        }
                        KeyCode::Down => {
                            if !is_agent_busy && input.slash_completion_active {
                                input.next_slash_completion();
                            } else if !is_agent_busy && history_index > 0 {
                                // Input history: go to newer
                                history_index -= 1;
                                if history_index == 0 {
                                    input.text = saved_input.clone();
                                } else {
                                    input.text = input_history[history_index - 1].clone();
                                }
                                input.cursor = input.text.chars().count();
                                input.clear_slash_completions();
                            } else {
                                scroll_offset = scroll_offset.saturating_sub(3);
                                if scroll_offset == 0 { auto_scroll = true; }
                            }
                        }
                        KeyCode::Esc => {
                            if input.slash_completion_active {
                                input.clear_slash_completions();
                            }
                        }
                        _ => {}
                    }
                }
                CEvent::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        scroll_offset = scroll_offset.saturating_add(3);
                        auto_scroll = false;
                    }
                    MouseEventKind::ScrollDown => {
                        scroll_offset = scroll_offset.saturating_sub(3);
                        if scroll_offset == 0 { auto_scroll = true; }
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
                    streaming_text.push_str(&text);
                }
                UiEvent::ToolCall { name, .. } => {
                    streaming_text.push_str(&format!("\n⚙ {}\n", name));
                }
                UiEvent::ToolStart { tool_name } => {
                    streaming_text.push_str(&format!("\n▶ {}...\n", tool_name));
                }
                UiEvent::ToolResult { tool_name, content, is_error } => {
                    let label = if tool_name.is_empty() { "tool" } else { &tool_name };
                    if is_error {
                        let preview: String = content.chars().take(200).collect();
                        streaming_text.push_str(&format!("  ✗ {}: {}\n", label, preview));
                    } else {
                        let preview: String = content.lines().take(3).collect::<Vec<_>>().join("\n  ");
                        if !preview.is_empty() {
                            streaming_text.push_str(&format!("  ✓ {}\n", preview));
                        }
                    }
                }
                UiEvent::Complete => {
                    if !streaming_text.is_empty() {
                        messages.push(ChatMessage {
                            role: MessageRole::Assistant,
                            content: streaming_text.clone(),
                            timestamp: now_millis(),
                        });
                        message_count += 1;
                        streaming_text.clear();
                    }
                    is_agent_busy = false;
                }
                UiEvent::Error(msg) => {
                    if !streaming_text.is_empty() {
                        messages.push(ChatMessage {
                            role: MessageRole::Assistant,
                            content: streaming_text.clone(),
                            timestamp: now_millis(),
                        });
                        message_count += 1;
                        streaming_text.clear();
                    }
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("Error: {}", msg),
                        timestamp: now_millis(),
                    });
                    is_agent_busy = false;
                }
                UiEvent::CompactionStart { reason } => {
                    let reason_str = match reason {
                        CompactionReason::Manual => "manual",
                        CompactionReason::Threshold => "auto",
                        CompactionReason::Overflow => "overflow",
                    };
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("📦 Compacting ({})...", reason_str),
                        timestamp: now_millis(),
                    });
                }
                UiEvent::CompactionEnd { _reason, error_message } => {
                    let msg = if let Some(err) = error_message {
                        format!("⚠ Compaction failed: {}", err)
                    } else {
                        "✅ Compaction complete".to_string()
                    };
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: msg,
                        timestamp: now_millis(),
                    });
                }
                UiEvent::RetryStart { attempt, max_attempts, error_message } => {
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("🔄 Retry ({}/{}): {}", attempt, max_attempts, error_message),
                        timestamp: now_millis(),
                    });
                }
                UiEvent::ModelChanged { model_id } => {
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("🤖 → {}", model_id),
                        timestamp: now_millis(),
                    });
                }
                UiEvent::ThinkingLevelChanged { level } => {
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("💭 Thinking: {}", level),
                        timestamp: now_millis(),
                    });
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

        if auto_scroll { scroll_offset = 0; }
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
// Rendering — Chat
// ═══════════════════════════════════════════════════════════════════════════

fn render_chat(
    f: &mut ratatui::Frame,
    area: Rect,
    messages: &[ChatMessage],
    streaming_text: &str,
    scroll_offset: usize,
    is_agent_busy: bool,
    spinner_frame: usize,
) {
    if area.width < 4 || area.height < 1 { return; }

    let mut all_lines: Vec<Line> = Vec::new();

    for msg in messages {
        let ts = format_timestamp(msg.timestamp);

        match msg.role {
            MessageRole::User => {
                // ─── User message with cyan accent bar + timestamp ───
                let mut first_spans = vec![
                    Span::styled(" ▌".to_string(), Style::default().fg(palette::CYAN)),
                    Span::styled(" ".to_string(), Style::default()),
                ];
                let first_line = msg.content.lines().next().unwrap_or("");
                // We'll add the line + right-aligned timestamp later; for now just the line
                first_spans.push(Span::styled(first_line.to_string(), Style::default().fg(palette::FG_BRIGHT)));
                // Timestamp padding + time on the right
                let used_w: usize = first_spans.iter().map(|s| s.content.chars().count()).sum();
                let ts_w = ts.chars().count() + 2;
                if used_w + ts_w < area.width as usize {
                    let gap = area.width as usize - used_w - ts_w;
                    first_spans.push(Span::styled(" ".repeat(gap), Style::default()));
                    first_spans.push(Span::styled(ts, Style::default().fg(palette::FG_DIM)));
                }
                all_lines.push(Line::from(first_spans));

                for line in msg.content.lines().skip(1) {
                    let mut ln = vec![
                        Span::styled(" │ ".to_string(), Style::default().fg(palette::BG_HOVER)),
                    ];
                    ln.extend(render_rich_line(line));
                    all_lines.push(Line::from(ln));
                }
                all_lines.push(Line::from(""));
            }
            MessageRole::Assistant => {
                // ─── Assistant message with green accent + timestamp ───
                let mut first_spans = vec![
                    Span::styled(" ▐".to_string(), Style::default().fg(palette::GREEN)),
                    Span::styled(" ".to_string(), Style::default()),
                ];
                let first_line = msg.content.lines().next().unwrap_or("");
                first_spans.push(Span::styled(first_line.to_string(), Style::default().fg(palette::FG)));
                let used_w: usize = first_spans.iter().map(|s| s.content.chars().count()).sum();
                let ts_w = ts.chars().count() + 2;
                if used_w + ts_w < area.width as usize {
                    let gap = area.width as usize - used_w - ts_w;
                    first_spans.push(Span::styled(" ".repeat(gap), Style::default()));
                    first_spans.push(Span::styled(ts, Style::default().fg(palette::FG_DIM)));
                }
                all_lines.push(Line::from(first_spans));

                // Remaining lines with rich rendering (code blocks, etc)
                let mut in_code_block = false;
                for line in msg.content.lines().skip(1) {
                    if line.trim_start().starts_with("```") {
                        in_code_block = !in_code_block;
                        if in_code_block {
                            all_lines.push(Line::from(vec![
                                Span::styled(" │".to_string(), Style::default().fg(palette::BG_HOVER)),
                                Span::styled(" ┌".to_string(), Style::default().fg(palette::BG_HOVER)),
                            ]));
                        } else {
                            all_lines.push(Line::from(vec![
                                Span::styled(" │".to_string(), Style::default().fg(palette::BG_HOVER)),
                                Span::styled(" └".to_string(), Style::default().fg(palette::BG_HOVER)),
                            ]));
                        }
                        continue;
                    }
                    if in_code_block {
                        all_lines.push(Line::from(vec![
                            Span::styled(" │".to_string(), Style::default().fg(palette::BG_HOVER)),
                            Span::styled(" │ ".to_string(), Style::default().fg(palette::BG_HOVER)),
                            Span::styled(line.to_string(), Style::default().fg(palette::FG)),
                        ]));
                    } else {
                        let mut ln = vec![
                            Span::styled(" │".to_string(), Style::default().fg(palette::BG_HOVER)),
                            Span::styled(" ".to_string(), Style::default()),
                        ];
                        ln.extend(render_rich_line(line));
                        all_lines.push(Line::from(ln));
                    }
                }
                all_lines.push(Line::from(""));
            }
            MessageRole::System => {
                // ─── System message — dimmed + timestamp ───
                let mut first_spans = vec![
                    Span::styled("  · ".to_string(), Style::default().fg(palette::FG_DIM)),
                ];
                let first_line = msg.content.lines().next().unwrap_or("");
                first_spans.push(Span::styled(first_line.to_string(), Style::default().fg(palette::FG_DIM)));
                let used_w: usize = first_spans.iter().map(|s| s.content.chars().count()).sum();
                let ts_w = ts.chars().count() + 2;
                if used_w + ts_w < area.width as usize {
                    let gap = area.width as usize - used_w - ts_w;
                    first_spans.push(Span::styled(" ".repeat(gap), Style::default()));
                    first_spans.push(Span::styled(ts, Style::default().fg(palette::FG_DIM)));
                }
                all_lines.push(Line::from(first_spans));

                for line in msg.content.lines().skip(1) {
                    all_lines.push(Line::from(vec![
                        Span::styled("    ".to_string(), Style::default()),
                        Span::styled(line.to_string(), Style::default().fg(palette::FG_DIM)),
                    ]));
                }
                all_lines.push(Line::from(""));
            }
        }
    }

    // Streaming text — same rich rendering as assistant
    if !streaming_text.is_empty() {
        all_lines.push(Line::from(""));
        let mut in_code_block = false;
        for line in streaming_text.lines() {
            if line.trim_start().starts_with("```") {
                in_code_block = !in_code_block;
                if in_code_block {
                    all_lines.push(Line::from(vec![
                        Span::styled(" │".to_string(), Style::default().fg(palette::BG_HOVER)),
                        Span::styled(" ┌".to_string(), Style::default().fg(palette::BG_HOVER)),
                    ]));
                } else {
                    all_lines.push(Line::from(vec![
                        Span::styled(" │".to_string(), Style::default().fg(palette::BG_HOVER)),
                        Span::styled(" └".to_string(), Style::default().fg(palette::BG_HOVER)),
                    ]));
                }
                continue;
            }
            if in_code_block {
                all_lines.push(Line::from(vec![
                    Span::styled(" │".to_string(), Style::default().fg(palette::BG_HOVER)),
                    Span::styled(" │ ".to_string(), Style::default().fg(palette::BG_HOVER)),
                    Span::styled(line.to_string(), Style::default().fg(palette::FG)),
                ]));
            } else {
                let mut ln = vec![
                    Span::styled("  ".to_string(), Style::default()),
                ];
                ln.extend(render_rich_line(line));
                all_lines.push(Line::from(ln));
            }
        }
        // Animated spinner
        if is_agent_busy {
            let spinner = SPINNER[spinner_frame];
            all_lines.push(Line::from(vec![
                Span::styled(format!("  {} ", spinner), Style::default().fg(palette::MAGENTA)),
                Span::styled("thinking...".to_string(), Style::default().fg(palette::FG_DIM)),
            ]));
        }
    }

    // Calculate wrapped lines for scroll
    let wrap_width = area.width as usize;
    let mut wrapped_count: usize = 0;
    for line in &all_lines {
        let w = line.width();
        if w == 0 { wrapped_count += 1; }
        else { wrapped_count += (w + wrap_width - 1) / wrap_width.max(1); }
    }

    let visible_height = area.height as usize;
    let max_scroll = wrapped_count.saturating_sub(visible_height);
    let clamped_offset = scroll_offset.min(max_scroll);
    let scroll_from_top = max_scroll.saturating_sub(clamped_offset);

    // Scroll indicator on right edge
    let show_scroll = max_scroll > 0;
    let scroll_pct = if max_scroll > 0 {
        ((max_scroll - scroll_from_top) as f32 / max_scroll as f32 * 100.0) as u8
    } else { 100 };

    let chat_text = ratatui::text::Text::from(all_lines);
    let chat_widget = Paragraph::new(chat_text)
        .wrap(Wrap { trim: false })
        .scroll((scroll_from_top as u16, 0))
        .style(Style::default().bg(palette::BG));

    f.render_widget(chat_widget, area);

    // Scroll indicator — subtle bar on right
    if show_scroll && visible_height > 2 {
        let thumb_size = (visible_height * visible_height / (max_scroll + visible_height)).max(1);
        let thumb_start = (scroll_from_top as f32 / max_scroll as f32 * visible_height as f32) as usize;
        for i in 0..thumb_size.min(visible_height) {
            let row = area.y + (thumb_start + i).min(visible_height - 1) as u16;
            let col = area.x + area.width - 1;
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "█".to_string(),
                    Style::default().fg(palette::BG_HOVER),
                ))),
                Rect { x: col, y: row, width: 1, height: 1 },
            );
        }
        // Percentage text at bottom-right (only when not at top or bottom)
        if scroll_from_top > 0 && scroll_from_top < max_scroll {
            let pct_text = format!("{}%", scroll_pct);
            let pct_len = pct_text.len() as u16;
            let pct_row = area.y + area.height - 1;
            let pct_col = area.x + area.width - pct_len - 2;
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(pct_text, Style::default().fg(palette::FG_DIM)))),
                Rect { x: pct_col, y: pct_row, width: pct_len + 1, height: 1 },
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Rendering — Separator
// ═══════════════════════════════════════════════════════════════════════════

fn render_separator(f: &mut ratatui::Frame, area: Rect) {
    // Subtle dotted pattern: ─··─··─··─
    let w = area.width as usize;
    let mut spans: Vec<Span> = Vec::with_capacity(w);
    for i in 0..w {
        let c = match i % 4 {
            0 => '─',
            1 => '·',
            2 => '·',
            _ => ' ',
        };
        spans.push(Span::styled(c.to_string(), Style::default().fg(palette::BG_HOVER)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ═══════════════════════════════════════════════════════════════════════════
// Rendering — Input
// ═══════════════════════════════════════════════════════════════════════════

fn render_input(
    f: &mut ratatui::Frame,
    area: Rect,
    input: &InputState,
    is_agent_busy: bool,
    spinner_frame: usize,
) {
    if area.height < 2 { return; }

    // Row 0: input field
    // Row 1: hint/popup line
    let input_row = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    let hint_row = Rect { x: area.x, y: area.y + 1, width: area.width, height: 1 };
    let border_row = Rect { x: area.x, y: area.y + 2, width: area.width, height: 1 };

    // ── Input field ──
    let prompt_char = if is_agent_busy {
        format!("{} ", SPINNER[spinner_frame])
    } else {
        "❯ ".to_string()
    };
    let prompt_color = if is_agent_busy { palette::MAGENTA } else { palette::CYAN };

    let display_text = if input.value().is_empty() && !is_agent_busy {
        "Type a message… (enter / for commands)".to_string()
    } else {
        input.value().to_string()
    };

    let text_fg = if input.value().is_empty() && !is_agent_busy {
        palette::FG_DIM
    } else {
        palette::FG_BRIGHT
    };

    // Horizontal scroll
    let max_content = area.width as usize - 4; // prompt(2) + padding
    let cursor_char = input.cursor;
    let scroll_left = if cursor_char >= max_content {
        cursor_char - max_content + 1
    } else { 0 };

    let visible_chars: String = display_text.chars().skip(scroll_left).take(max_content).collect();
    let cursor_screen = cursor_char.saturating_sub(scroll_left);

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(prompt_char, Style::default().fg(prompt_color)));

    let chars: Vec<char> = visible_chars.chars().collect();
    let show_cursor = !input.value().is_empty();

    for (i, ch) in chars.iter().enumerate() {
        if show_cursor && i == cursor_screen {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().fg(palette::BG).bg(palette::FG_BRIGHT).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(ch.to_string(), Style::default().fg(text_fg)));
        }
    }

    if show_cursor && cursor_screen >= chars.len() {
        spans.push(Span::styled(
            " ".to_string(),
            Style::default().fg(palette::BG).bg(palette::FG_BRIGHT),
        ));
    }

    // Remaining padding
    let used = chars.len().max(cursor_screen + 1);
    if used < max_content {
        spans.push(Span::styled(" ".repeat(max_content - used), Style::default()));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), input_row);

    // ── Hint / popup row ──
    if input.slash_completion_active {
        render_slash_popup(f, hint_row, input);
    } else if is_agent_busy {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Ctrl+C to interrupt".to_string(),
                Style::default().fg(palette::FG_DIM),
            ))),
            hint_row,
        );
    } else if input.value().is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Enter · / commands · ↑ history · Esc cancel".to_string(),
                Style::default().fg(palette::FG_DIM),
            ))),
            hint_row,
        );
    } else {
        // Show char count when typing
        let count = input.text.chars().count();
        let count_str = format!("  {} chars", count);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(count_str, Style::default().fg(palette::FG_DIM)))),
            hint_row,
        );
    }

    // ── Bottom accent line (dotted, matching separator style) ──
    let mut border_spans: Vec<Span> = Vec::with_capacity(area.width as usize);
    for i in 0..area.width as usize {
        let c = match i % 4 { 0 => '─', 1 => '·', 2 => '·', _ => ' ' };
        border_spans.push(Span::styled(c.to_string(), Style::default().fg(palette::BG_HOVER)));
    }
    f.render_widget(Paragraph::new(Line::from(border_spans)), border_row);
}

/// Slash command popup — multi-column grid layout
fn render_slash_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    input: &InputState,
) {
    let completions = &input.slash_completions;
    if completions.is_empty() { return; }

    let selected = input.slash_completion_index;
    let max_show = 6usize;

    // Calculate visible window around selected
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
                Style::default().fg(palette::BG).bg(palette::BLUE).add_modifier(Modifier::BOLD),
            ));
            // space between items
            spans.push(Span::styled(" ".to_string(), Style::default()));
        } else {
            spans.push(Span::styled(
                format!(" {} ", comp.name),
                Style::default().fg(palette::FG_DIM),
            ));
            spans.push(Span::styled(" ".to_string(), Style::default()));
        }
    }

    // Description on the right
    if let Some(comp) = completions.get(selected) {
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let remaining = area.width as usize;
        let desc_max = remaining.saturating_sub(used + 4);
        if desc_max > 5 {
            let desc: String = comp.description.chars().take(desc_max).collect();
            spans.push(Span::styled(
                format!("— {}", desc),
                Style::default().fg(palette::FG_DIM),
            ));
        }
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ═══════════════════════════════════════════════════════════════════════════
// Rendering — Status Bar
// ═══════════════════════════════════════════════════════════════════════════

fn render_status_bar(
    f: &mut ratatui::Frame,
    area: Rect,
    cwd: &str,
    model_id: &str,
    git_branch: Option<&str>,
    is_agent_busy: bool,
    message_count: usize,
) {
    if area.width < 4 { return; }

    let mut left: Vec<Span> = Vec::new();
    let mut right: Vec<Span> = Vec::new();

    // Left: dir + branch
    let home = std::env::var("HOME").unwrap_or_default();
    let display_cwd = if !home.is_empty() && cwd.starts_with(&home) {
        format!("~{}", &cwd[home.len()..])
    } else {
        cwd.to_string()
    };
    let max_cwd = (area.width as usize / 3).max(8);
    let display_cwd = if display_cwd.len() > max_cwd {
        let short: String = display_cwd.chars().rev().take(max_cwd.saturating_sub(2)).collect();
        format!("…{}", short.chars().rev().collect::<String>())
    } else { display_cwd };

    left.push(Span::styled(" ".to_string(), Style::default()));
    left.push(Span::styled(display_cwd, Style::default().fg(palette::FG)));

    if let Some(branch) = git_branch {
        if !branch.is_empty() {
            left.push(Span::styled(" ⎇ ".to_string(), Style::default().fg(palette::MAGENTA)));
            left.push(Span::styled(branch.to_string(), Style::default().fg(palette::MAGENTA)));
        }
    }

    // Right: message count · model · status
    if message_count > 0 {
        right.push(Span::styled(format!("{} msgs", message_count), Style::default().fg(palette::FG_DIM)));
        right.push(Span::styled("  ".to_string(), Style::default()));
    }

    if !model_id.is_empty() {
        let model_display = model_id.split('/').last().unwrap_or(model_id);
        right.push(Span::styled("● ".to_string(), Style::default().fg(palette::GREEN)));
        right.push(Span::styled(model_display.to_string(), Style::default().fg(palette::CYAN)));
    }

    if is_agent_busy {
        right.push(Span::styled("  ⚡".to_string(), Style::default().fg(palette::YELLOW)));
    }

    // Compose with right-alignment
    let left_w: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let right_w: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let gap = area.width as usize;

    let mut all = left;
    let spacing = gap.saturating_sub(left_w).saturating_sub(right_w);
    if spacing > 0 {
        all.push(Span::styled(" ".repeat(spacing), Style::default()));
    }
    all.extend(right);

    // Render with surface background
    f.render_widget(
        Paragraph::new(Line::from(all)).style(Style::default().bg(palette::BG_SURFACE)),
        area,
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Welcome banner
// ═══════════════════════════════════════════════════════════════════════════

fn format_welcome(session_id: &str, model_id: &str) -> String {
    // Box is 35 chars wide: ╭ + 33 × ─ + ╮
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
    messages: &mut Vec<ChatMessage>,
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
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: format_help(),
                timestamp: now_millis(),
            });
            true
        }
        "/quit" | "/exit" | "/q" => { *running = false; true }
        "/clear" => { messages.clear(); session.reset(); true }
        "/model" => {
            if let Some(model_id) = arg {
                match session.set_model(model_id) {
                    Ok(()) => {
                        messages.push(ChatMessage {
                            role: MessageRole::System,
                            content: format!("→ model: {}", model_id),
                            timestamp: now_millis(),
                        });
                    }
                    Err(e) => {
                        messages.push(ChatMessage {
                            role: MessageRole::System,
                            content: format!("✗ {}", e),
                            timestamp: now_millis(),
                        });
                    }
                }
            } else {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("Model: {}\n/model <provider/model> to switch", session.model_id()),
                    timestamp: now_millis(),
                });
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
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: format!(
                    "Session: {}\nMessages: {} ({} user, {} assistant)\nTools: {} calls, {} results\nModel: {}\nThinking: {:?}\nAuto-compact: {}\nAuto-retry: {}",
                    stats.session_id, stats.total_messages, stats.user_messages, stats.assistant_messages,
                    stats.tool_calls, stats.tool_results, session.model_id(),
                    session.thinking_level(), session.auto_compaction_enabled(), session.auto_retry_enabled(),
                ),
                timestamp: now_millis(),
            });
            true
        }
        "/settings" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: format!(
                    "Model: {}\nThinking: {:?}\nAuto-compact: {}\nAuto-retry: {}",
                    session.model_id(), session.thinking_level(),
                    session.auto_compaction_enabled(), session.auto_retry_enabled(),
                ),
                timestamp: now_millis(),
            });
            true
        }
        "/name" => {
            if let Some(name) = arg {
                session.set_session_name(name.to_string());
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("Session → {}", name),
                    timestamp: now_millis(),
                });
            } else {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "/name <name>".to_string(),
                    timestamp: now_millis(),
                });
            }
            true
        }
        "/copy" => {
            let last = messages.iter().rev().find(|m| m.role == MessageRole::Assistant);
            if let Some(msg) = last {
                match clipboard_write::copy_to_clipboard(&msg.content) {
                    Ok(()) => messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: "✓ Copied to clipboard".to_string(),
                        timestamp: now_millis(),
                    }),
                    Err(e) => messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("✗ Copy failed: {}", e),
                        timestamp: now_millis(),
                    }),
                }
            } else {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "No assistant message".to_string(),
                    timestamp: now_millis(),
                });
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
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "No changelog found".to_string(),
                    timestamp: now_millis(),
                });
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
                messages.push(ChatMessage {
                    role: MessageRole::System, content: out, timestamp: now_millis(),
                });
            }
            true
        }
        "/hotkeys" | "/keys" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: format_hotkeys(),
                timestamp: now_millis(),
            });
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
            let entries: Vec<crate::session::SessionEntry> = messages.iter().map(|msg| {
                let role = match msg.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::System => "system",
                };
                crate::session::SessionEntry::simple_message(role, &msg.content)
            }).collect();
            match export::export_to_html(&entries, &meta, &HtmlExportOptions::default()) {
                Ok(html) => {
                    if let Some(path) = export_path {
                        match std::fs::write(&path, &html) {
                            Ok(()) => messages.push(ChatMessage {
                                role: MessageRole::System,
                                content: format!("✓ Exported: {}", path.display()),
                                timestamp: now_millis(),
                            }),
                            Err(e) => messages.push(ChatMessage {
                                role: MessageRole::System,
                                content: format!("✗ Write failed: {}", e),
                                timestamp: now_millis(),
                            }),
                        }
                    } else {
                        messages.push(ChatMessage {
                            role: MessageRole::System,
                            content: format!("HTML ready ({} bytes). /export <path> to save.", html.len()),
                            timestamp: now_millis(),
                        });
                    }
                }
                Err(e) => messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("✗ Export failed: {}", e),
                    timestamp: now_millis(),
                }),
            }
            true
        }
        "/import" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: if let Some(p) = arg {
                    format!("Import '{}' — coming soon", p)
                } else {
                    "/import <path-to-jsonl>".to_string()
                },
                timestamp: now_millis(),
            });
            true
        }
        "/share" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: "GitHub gist sharing coming soon. Use /export for HTML.".to_string(),
                timestamp: now_millis(),
            });
            true
        }
        "/fork" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: "Use /tree to view branches. Fork via session navigation.".to_string(),
                timestamp: now_millis(),
            });
            true
        }
        "/clone" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: "Run oxi --continue in a new terminal to clone.".to_string(),
                timestamp: now_millis(),
            });
            true
        }
        "/tree" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: "Linear session. Use /fork to branch from a previous message.".to_string(),
                timestamp: now_millis(),
            });
            true
        }
        "/login" => {
            if let Some(provider) = arg {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!(
                        "Set {} API key:\n  export {}_API_KEY=your-key",
                        provider, provider.to_uppercase()
                    ),
                    timestamp: now_millis(),
                });
            } else {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "/login <provider>\n\nProviders: anthropic, openai, google, groq, mistral, deepseek, xai".to_string(),
                    timestamp: now_millis(),
                });
            }
            true
        }
        "/logout" => {
            if let Some(provider) = arg {
                AuthStorage::new().remove(provider);
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("✓ Removed {}", provider),
                    timestamp: now_millis(),
                });
            } else {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "/logout <provider>".to_string(),
                    timestamp: now_millis(),
                });
            }
            true
        }
        "/new" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: "Starting new session…".to_string(),
                timestamp: now_millis(),
            });
            session.reset();
            messages.clear();
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
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: "No previous sessions".to_string(),
                        timestamp: now_millis(),
                    });
                } else {
                    let mut out = "Recent:\n\n".to_string();
                    for (i, entry) in list.iter().enumerate() {
                        if let Some(name) = entry.file_name().to_str() {
                            out.push_str(&format!("{}. {}\n", i + 1, name));
                        }
                    }
                    out.push_str("\n/import <path> to resume");
                    messages.push(ChatMessage {
                        role: MessageRole::System, content: out, timestamp: now_millis(),
                    });
                }
            } else {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "No sessions found".to_string(),
                    timestamp: now_millis(),
                });
            }
            true
        }
        "/reload" => {
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: "✓ Configuration reloaded".to_string(),
                timestamp: now_millis(),
            });
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
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("Scoped: {} (Ctrl+P to cycle)", names.join(", ")),
                        timestamp: now_millis(),
                    });
                } else {
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: "/scoped-models provider/model1,provider/model2".to_string(),
                        timestamp: now_millis(),
                    });
                }
            } else {
                let scoped = session.scoped_models();
                if scoped.is_empty() {
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: "No scoped models. /scoped-models <m1>,<m2>".to_string(),
                        timestamp: now_millis(),
                    });
                } else {
                    let names: Vec<String> = scoped.iter().map(|m| format!("{}/{}", m.provider, m.model_id)).collect();
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("Scoped: {}", names.join(", ")),
                        timestamp: now_millis(),
                    });
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

/// Format a millisecond timestamp as HH:MM.
fn format_timestamp(millis: i64) -> String {
    let secs = millis / 1000;
    let hours = ((secs / 3600) % 24) as u8;
    let mins = ((secs / 60) % 60) as u8;
    format!("{}:{:02}", hours, mins)
}

/// Render a line of text with inline markdown styling.
/// Returns Vec<Span> with proper fg/bg for **bold**, *italic*, `code`.
fn render_rich_line(line: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();

    let flush = |buf: &mut String, spans: &mut Vec<Span>| {
        if !buf.is_empty() {
            spans.push(Span::styled(buf.clone(), Style::default().fg(palette::FG)));
            buf.clear();
        }
    };

    while i < len {
        // Inline code `...`
        if chars[i] == '`' {
            flush(&mut buf, &mut spans);
            i += 1;
            let mut code = String::new();
            while i < len && chars[i] != '`' {
                code.push(chars[i]);
                i += 1;
            }
            if i < len { i += 1; } // closing `
            spans.push(Span::styled(
                code,
                Style::default().fg(palette::ORANGE),
            ));
        }
        // Bold **...**
        else if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            flush(&mut buf, &mut spans);
            i += 2;
            let mut bold = String::new();
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '*') {
                bold.push(chars[i]);
                i += 1;
            }
            if i + 1 < len { i += 2; }
            spans.push(Span::styled(
                bold,
                Style::default().fg(palette::FG_BRIGHT).add_modifier(Modifier::BOLD),
            ));
        }
        // Italic *...*  (but not **)
        else if chars[i] == '*' && (i + 1 >= len || chars[i + 1] != '*') {
            flush(&mut buf, &mut spans);
            i += 1;
            let mut italic = String::new();
            while i < len && chars[i] != '*' {
                italic.push(chars[i]);
                i += 1;
            }
            if i < len { i += 1; }
            spans.push(Span::styled(
                italic,
                Style::default().fg(palette::FG).add_modifier(Modifier::ITALIC),
            ));
        }
        // Heading ##
        else if chars[i] == '#' && (i == 0 || chars[i - 1] == ' ') {
            flush(&mut buf, &mut spans);
            while i < len && chars[i] == '#' { i += 1; }
            // skip space after #s
            if i < len && chars[i] == ' ' { i += 1; }
            let mut heading = String::new();
            while i < len { heading.push(chars[i]); i += 1; }
            spans.push(Span::styled(
                heading,
                Style::default().fg(palette::FG_BRIGHT).add_modifier(Modifier::BOLD),
            ));
        }
        else {
            buf.push(chars[i]);
            i += 1;
        }
    }
    flush(&mut buf, &mut spans);
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), Style::default().fg(palette::FG)));
    }
    spans
}
