//! TUI-based interactive mode using ratatui.
//!
//! Provides a flicker-free terminal chat interface with:
//! - Double-buffered rendering via ratatui
//! - Line-level differential updates (zero flicker)
//! - Streaming text display
//! - Scrollable chat history
//! - Slash commands

use crate::agent_session::{AgentSession, CompactionReason, SessionEvent};
use crate::agent_session_runtime::{
    create_agent_session_from_services, create_agent_session_services,
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
};
use crate::session::SessionManager;
use anyhow::Result;
use oxi_agent::AgentEvent;
use std::io;
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
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
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

/// State for the input field (char-index based, UTF-8 safe).
struct InputState {
    text: String,
    cursor: usize, // char index
}

impl InputState {
    fn new() -> Self {
        Self { text: String::new(), cursor: 0 }
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn value(&self) -> &str {
        &self.text
    }

    fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

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
        if self.cursor < self.char_count() {
            let byte_pos = self.char_to_byte(self.cursor);
            self.text.remove(byte_pos);
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.char_count() {
            self.cursor += 1;
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.char_count();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Theme
// ═══════════════════════════════════════════════════════════════════════════

struct Theme {
    user_fg: Color,
    assistant_fg: Color,
    system_fg: Color,
    border_fg: Color,
    input_fg: Color,
    input_cursor_fg: Color,
    input_cursor_bg: Color,
    placeholder_fg: Color,
    prompt_indicator_fg: Color,
    thinking_fg: Color,
    #[allow(dead_code)]
    tool_name_fg: Color,
    #[allow(dead_code)]
    tool_border_fg: Color,
    #[allow(dead_code)]
    error_fg: Color,
    #[allow(dead_code)]
    success_fg: Color,
    status_fg: Color,
}

impl Theme {
    fn dark() -> Self {
        Self {
            user_fg: Color::Cyan,
            assistant_fg: Color::Gray,
            system_fg: Color::Yellow,
            border_fg: Color::DarkGray,
            input_fg: Color::White,
            input_cursor_fg: Color::Black,
            input_cursor_bg: Color::White,
            placeholder_fg: Color::DarkGray,
            prompt_indicator_fg: Color::Cyan,
            thinking_fg: Color::DarkGray,
            tool_name_fg: Color::Yellow,
            tool_border_fg: Color::DarkGray,
            error_fg: Color::Red,
            success_fg: Color::Green,
            status_fg: Color::Yellow,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════

pub async fn run_tui_interactive(app: crate::App) -> Result<()> {
    let theme = Theme::dark();
    let settings = app.settings().clone();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    // ── Build AgentSession ───────────────────────────────────────────
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

    // ── Subscribe to session events ──────────────────────────────────
    let (session_event_tx, mut session_event_rx) = mpsc::unbounded_channel::<SessionEvent>();
    agent_session.subscribe(Box::new(move |event| {
        let _ = session_event_tx.send(event.clone());
    }));

    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(256);
    let (prompt_tx, mut prompt_rx) = mpsc::channel::<String>(16);

    // ── Agent worker thread ──────────────────────────────────────────
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
                                AgentEvent::ToolStart { tool_name, .. } => UiEvent::TextDelta(
                                    format!("\n⚙ Running: {}...\n", tool_name),
                                ),
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
                                AgentEvent::Error { message } => UiEvent::Error(message),
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

    // ── Setup terminal ───────────────────────────────────────────────
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // ── App state ────────────────────────────────────────────────────
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut input = InputState::new();
    let mut is_agent_busy = false;
    let mut streaming_text = String::new();
    let mut scroll_offset: u16 = 0;
    let mut auto_scroll = true;

    // Welcome message
    messages.push(ChatMessage {
        role: MessageRole::System,
        content: format!(
            "oxi ready. Session: {}\nModel: {}\nType /help for commands.",
            session_id,
            agent_session.model_id(),
        ),
        timestamp: now_millis(),
    });

    let mut running = true;
    let poll_timeout = std::time::Duration::from_millis(33);

    while running {
        // ── Render ───────────────────────────────────────────────────
        terminal.draw(|f| {
            let size = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(3),     // Chat area
                    Constraint::Length(1),   // Separator
                    Constraint::Length(2),   // Input area
                ])
                .split(size);

            render_chat(f, chunks[0], &messages, &streaming_text, scroll_offset, &theme);
            render_separator(f, chunks[1], &theme);
            render_input(f, chunks[2], &input, is_agent_busy, &theme);
        })?;

        // ── Poll for terminal events ─────────────────────────────────
        if event::poll(poll_timeout)? {
            match event::read()? {
                CEvent::Key(key) => {
                    match key.code {
                        KeyCode::Enter => {
                            if !is_agent_busy {
                                let value = input.value().to_string();
                                if !value.is_empty() {
                                    if value.starts_with('/') {
                                        let handled = handle_slash_command(
                                            &value,
                                            &agent_session,
                                            &mut messages,
                                            &mut running,
                                        );
                                        input.clear();
                                        if handled {
                                            continue;
                                        }
                                    }

                                    messages.push(ChatMessage {
                                        role: MessageRole::User,
                                        content: value.clone(),
                                        timestamp: now_millis(),
                                    });

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
                                    streaming_text.clear();
                                }
                                messages.push(ChatMessage {
                                    role: MessageRole::System,
                                    content: "Interrupted".to_string(),
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
                            if scroll_offset == 0 {
                                auto_scroll = true;
                            }
                        }
                        KeyCode::Char(c) => {
                            if !is_agent_busy {
                                input.insert_char(c);
                            }
                        }
                        KeyCode::Backspace => {
                            if !is_agent_busy {
                                input.backspace();
                            }
                        }
                        KeyCode::Delete => {
                            if !is_agent_busy {
                                input.delete();
                            }
                        }
                        KeyCode::Left => {
                            if !is_agent_busy {
                                input.move_left();
                            }
                        }
                        KeyCode::Right => {
                            if !is_agent_busy {
                                input.move_right();
                            }
                        }
                        KeyCode::Home => {
                            if !is_agent_busy {
                                input.move_home();
                            }
                        }
                        KeyCode::End => {
                            if !is_agent_busy {
                                input.move_end();
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
                        if scroll_offset == 0 {
                            auto_scroll = true;
                        }
                    }
                    _ => {}
                },
                CEvent::Resize(_, _) => {
                    // ratatui handles resize automatically in draw()
                }
                _ => {}
            }
        }

        // ── Drain agent events ───────────────────────────────────────
        while let Ok(ui_event) = ui_rx.try_recv() {
            match ui_event {
                UiEvent::Start => {}
                UiEvent::Thinking => {}
                UiEvent::TextDelta(text) => {
                    streaming_text.push_str(&text);
                }
                UiEvent::ToolCall { name, .. } => {
                    streaming_text.push_str(&format!("\n⚙ {}\n", name));
                }
                UiEvent::ToolResult { tool_name, content, is_error } => {
                    let label = if tool_name.is_empty() { "tool" } else { &tool_name };
                    if is_error {
                        streaming_text.push_str(&format!("  ✗ {}: {}\n", label, content.chars().take(200).collect::<String>()));
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
                        CompactionReason::Threshold => "auto-threshold",
                        CompactionReason::Overflow => "overflow-recovery",
                    };
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("📦 Compacting context ({})...", reason_str),
                        timestamp: now_millis(),
                    });
                }
                UiEvent::CompactionEnd { _reason, error_message } => {
                    let msg = if let Some(err) = error_message {
                        format!("⚠️ Compaction failed: {}", err)
                    } else {
                        "✅ Compaction complete.".to_string()
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
                        content: format!("🔄 Retrying ({}/{}): {}", attempt, max_attempts, error_message),
                        timestamp: now_millis(),
                    });
                }
                UiEvent::ModelChanged { model_id } => {
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("🤖 Model: {}", model_id),
                        timestamp: now_millis(),
                    });
                }
                UiEvent::ThinkingLevelChanged { level } => {
                    messages.push(ChatMessage {
                        role: MessageRole::System,
                        content: format!("💭 Thinking level: {}", level),
                        timestamp: now_millis(),
                    });
                }
                UiEvent::QueueUpdate { pending } => {
                    if pending > 0 {
                        tracing::debug!("Queue updated: {} pending messages", pending);
                    }
                }
            }
        }

        // ── Drain session events ─────────────────────────────────────
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

        // ── Auto-scroll logic ────────────────────────────────────────
        if auto_scroll {
            scroll_offset = 0;
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────────
    drop(prompt_tx);

    // Restore terminal state even if agent thread join fails
    let cleanup_terminal = |terminal: &mut Terminal<CrosstermBackend<io::Stdout>>| -> Result<()> {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        terminal.show_cursor()?;
        Ok(())
    };

    // Give agent thread 5 seconds to finish, then move on
    let _ = agent_handle.join();

    if let Err(e) = cleanup_terminal(&mut terminal) {
        tracing::error!("Terminal cleanup failed: {}", e);
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Rendering
// ═══════════════════════════════════════════════════════════════════════════

/// Render the chat area with messages and optional streaming text.
fn render_chat(
    f: &mut ratatui::Frame,
    area: Rect,
    messages: &[ChatMessage],
    streaming_text: &str,
    scroll_offset: u16,
    theme: &Theme,
) {
    if area.width < 4 || area.height < 1 {
        return;
    }

    // Build all lines from messages
    let mut all_lines: Vec<Line> = Vec::new();

    for msg in messages {
        let (label, label_fg) = match msg.role {
            MessageRole::User => (" You", theme.user_fg),
            MessageRole::Assistant => (" Assistant", theme.assistant_fg),
            MessageRole::System => (" ◈", theme.system_fg),
        };

        all_lines.push(Line::from(vec![
            Span::styled(label.to_string(), Style::default().fg(label_fg).add_modifier(Modifier::BOLD)),
        ]));

        for line in msg.content.lines() {
            let content_fg = match msg.role {
                MessageRole::System => theme.system_fg,
                _ => theme.assistant_fg,
            };
            all_lines.push(Line::from(vec![
                Span::styled("  ".to_string(), Style::default()),
                Span::styled(line.to_string(), Style::default().fg(content_fg)),
            ]));
        }

        // Blank line separator
        all_lines.push(Line::from(""));
    }

    // Streaming text
    if !streaming_text.is_empty() {
        all_lines.push(Line::from(vec![
            Span::styled(" Assistant".to_string(), Style::default().fg(theme.assistant_fg).add_modifier(Modifier::BOLD)),
        ]));
        for line in streaming_text.lines() {
            all_lines.push(Line::from(vec![
                Span::styled("  ".to_string(), Style::default()),
                Span::styled(line.to_string(), Style::default().fg(theme.assistant_fg)),
            ]));
        }
        // Animated thinking indicator while streaming
        all_lines.push(Line::from(vec![
            Span::styled("  ●", Style::default().fg(theme.thinking_fg)),
        ]));
    }

    // Calculate wrapped line count to determine scroll range.
    // After wrapping, the actual number of displayed lines may be larger
    // than all_lines.len() because long lines get split.
    let wrap_width = area.width as usize;
    let mut wrapped_count: usize = 0;
    for line in &all_lines {
        // Use the ratatui text width calculation
        let line_width = line.width();
        if line_width == 0 {
            wrapped_count += 1;
        } else {
            wrapped_count += (line_width + wrap_width - 1) / wrap_width.max(1);
        }
    }

    // Build the text widget
    let chat_text = ratatui::text::Text::from(all_lines);

    let visible_height = area.height as usize;
    let max_scroll = wrapped_count.saturating_sub(visible_height);
    let clamped_offset = (scroll_offset as usize).min(max_scroll);
    let scroll_from_top = max_scroll.saturating_sub(clamped_offset);

    let chat_widget = Paragraph::new(chat_text)
        .wrap(Wrap { trim: false })
        .scroll((scroll_from_top as u16, 0));

    f.render_widget(chat_widget, area);
}

/// Render the separator line between chat and input.
fn render_separator(f: &mut ratatui::Frame, area: Rect, theme: &Theme) {
    let separator = Paragraph::new(Line::from(
        Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(theme.border_fg),
        ),
    ));
    f.render_widget(separator, area);
}

/// Render the input area with cursor.
fn render_input(
    f: &mut ratatui::Frame,
    area: Rect,
    input: &InputState,
    is_agent_busy: bool,
    theme: &Theme,
) {
    // Split area into prompt indicator + input field + status
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(2),    // "❯ "
            Constraint::Min(10),      // Input text
            Constraint::Length(16),   // Status
        ])
        .split(area);

    // Prompt indicator
    let prompt = Paragraph::new(Line::from(vec![
        Span::styled("❯ ", Style::default().fg(theme.prompt_indicator_fg)),
    ]));
    f.render_widget(prompt, Rect { x: chunks[0].x, y: chunks[0].y, width: 2, height: 1 });

    // Input text with cursor
    let display_text = if input.value().is_empty() {
        "Type a message... (Ctrl+C to quit)".to_string()
    } else {
        input.value().to_string()
    };

    let text_fg = if input.value().is_empty() {
        theme.placeholder_fg
    } else {
        theme.input_fg
    };

    // Calculate visible portion and cursor position
    let input_width = chunks[1].width as usize;
    let cursor_char = input.cursor;

    // Horizontal scrolling
    let scroll_left = if cursor_char >= input_width {
        cursor_char - input_width + 1
    } else {
        0
    };

    let visible_chars: String = display_text.chars().skip(scroll_left).take(input_width).collect();
    let cursor_screen_col = cursor_char.saturating_sub(scroll_left);

    // Build styled spans: text before cursor, cursor cell, text after cursor
    let mut spans: Vec<Span> = Vec::new();
    let chars: Vec<char> = visible_chars.chars().collect();

    for (i, ch) in chars.iter().enumerate() {
        if i == cursor_screen_col {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().fg(theme.input_cursor_fg).bg(theme.input_cursor_bg).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().fg(text_fg),
            ));
        }
    }

    // If cursor is at the end of visible text, show cursor on empty space
    if cursor_screen_col >= chars.len() && cursor_screen_col < input_width {
        // Already at end - cursor shown as a space with highlight
        spans.push(Span::styled(
            " ".to_string(),
            Style::default().fg(theme.input_cursor_fg).bg(theme.input_cursor_bg),
        ));
    }

    // Pad remaining space
    let used = chars.len().max(cursor_screen_col + 1);
    if used < input_width {
        spans.push(Span::styled(
            " ".repeat(input_width - used),
            Style::default(),
        ));
    }

    let input_line = Line::from(spans);
    let input_widget = Paragraph::new(input_line);
    f.render_widget(input_widget, chunks[1]);

    // Status indicator (bottom row)
    if area.height >= 2 {
        let status_text = if is_agent_busy {
            "● thinking..."
        } else {
            ""
        };
        let status_fg = if is_agent_busy { theme.status_fg } else { theme.border_fg };
        let status = Paragraph::new(Line::from(vec![
            Span::styled(status_text.to_string(), Style::default().fg(status_fg)),
        ]));
        let status_row = Rect { x: 0, y: area.y + 1, width: area.width, height: 1 };
        f.render_widget(status, status_row);
    }
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
        "/quit" | "/exit" | "/q" => {
            *running = false;
            true
        }
        "/clear" => {
            messages.clear();
            session.reset();
            true
        }
        "/model" => {
            if let Some(model_id) = arg {
                match session.set_model(model_id) {
                    Ok(()) => {
                        messages.push(ChatMessage {
                            role: MessageRole::System,
                            content: format!("Switched to model: {}", model_id),
                            timestamp: now_millis(),
                        });
                    }
                    Err(e) => {
                        messages.push(ChatMessage {
                            role: MessageRole::System,
                            content: format!("Error switching model: {}", e),
                            timestamp: now_millis(),
                        });
                    }
                }
            } else {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!(
                        "Current model: {}\nUse /model <provider/model> to switch.",
                        session.model_id(),
                    ),
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
                    Ok(result) => tracing::info!("Compaction complete: {} tokens before", result.tokens_before),
                    Err(e) => tracing::warn!("Compaction failed: {}", e),
                }
            });
            true
        }
        "/session" => {
            let stats = session.session_stats();
            let info = format!(
                "Session Info:\n  ID: {}\n  Messages: {} total ({} user, {} assistant)\n  Tool calls: {}, Results: {}\n  Model: {}\n  Thinking: {:?}\n  Auto-compaction: {}\n  Auto-retry: {}",
                stats.session_id,
                stats.total_messages,
                stats.user_messages,
                stats.assistant_messages,
                stats.tool_calls,
                stats.tool_results,
                session.model_id(),
                session.thinking_level(),
                session.auto_compaction_enabled(),
                session.auto_retry_enabled(),
            );
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: info,
                timestamp: now_millis(),
            });
            true
        }
        "/settings" => {
            let info = format!(
                "Model: {}\nThinking Level: {:?}\nAuto-compaction: {}\nAuto-retry: {}",
                session.model_id(),
                session.thinking_level(),
                session.auto_compaction_enabled(),
                session.auto_retry_enabled(),
            );
            messages.push(ChatMessage {
                role: MessageRole::System,
                content: info,
                timestamp: now_millis(),
            });
            true
        }
        "/name" => {
            if let Some(name) = arg {
                session.set_session_name(name.to_string());
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: format!("Session named: {}", name),
                    timestamp: now_millis(),
                });
            } else {
                messages.push(ChatMessage {
                    role: MessageRole::System,
                    content: "Usage: /name <name>".to_string(),
                    timestamp: now_millis(),
                });
            }
            true
        }
        _ => false,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

fn format_help() -> String {
    r#"oxi — AI Coding Assistant

Commands:
  /model [id]        Switch or show model
  /clear             Clear conversation history
  /compact [instr]   Compact context with optional instructions
  /session           Show session info and stats
  /settings          Show current settings
  /name <name>       Set session display name
  /help              Show this help message
  /quit              Quit oxi

Keybindings:
  Enter              Send message or command
  Ctrl+C             Interrupt agent or quit
  PageUp/PageDown    Scroll chat history
  Mouse scroll       Scroll chat history
"#.to_string()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
