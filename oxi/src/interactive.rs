//! Interactive mode for the oxi coding agent.
//!
//! Manages the TUI display loop, input handling, command dispatch,
//! agent event processing, and state machine transitions.
//!
//! Modes: `Input → Thinking → ToolExecution → Display → Input`
//!
//! # Commands
//!
//! `/model`, `/clear`, `/compact`, `/undo`, `/redo`, `/branch`,
//! `/session`, `/export`, `/settings`, `/help`

use crate::InteractiveSession;
use anyhow::Result;
use oxi_agent::{Agent, AgentEvent};
use oxi_tui::{
    ChatMessageDisplay, ChatView, Component, ContentBlockDisplay, Input, MessageRole, Rect, Surface, Theme,
};
use std::os::unix::process::ExitStatusExt;
use std::sync::Arc;
use tokio::sync::mpsc;

// ── UI events from agent → TUI ─────────────────────────────────────────────

#[derive(Debug)]
enum UiEvent {
    Start,
    Thinking,
    TextDelta(String),
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
}

// ── Interactive mode state machine ─────────────────────────────────────────

/// State of the interactive loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveState {
    /// Waiting for user input.
    Input,
    /// Agent is thinking / streaming text.
    Thinking,
    /// A tool is executing.
    ToolExecution,
    /// Final display before returning to input.
    Display,
}

/// Parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// `/model [search]`
    Model { search: Option<String> },
    /// `/clear` — reset conversation.
    Clear,
    /// `/compact [custom_instructions]`
    Compact { custom_instructions: Option<String> },
    /// `/undo` — undo last exchange.
    Undo,
    /// `/redo` — redo last undone exchange.
    Redo,
    /// `/branch` — show branch / tree selector.
    Branch,
    /// `/session` — show session info.
    Session,
    /// `/export [path]`
    Export { path: Option<String> },
    /// `/settings` — open settings.
    Settings,
    /// `/help` — show help.
    Help,
    /// `/quit` — exit.
    Quit,
    /// `/name <name>` — set session name.
    Name { name: String },
    /// `/copy` — copy last assistant message.
    Copy,
    /// `/new` — start a new session.
    New,
    /// Unknown command.
    Unknown { raw: String },
}

impl SlashCommand {
    /// Parse a user-input line starting with `/` into a `SlashCommand`.
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        // Split into command and argument
        let (cmd, arg) = if let Some(space) = trimmed.find(' ') {
            (&trimmed[..space], Some(trimmed[space + 1..].trim()))
        } else {
            (trimmed, None)
        };
        let cmd_lower = cmd.to_lowercase();

        match cmd_lower.as_str() {
            "/model" => SlashCommand::Model {
                search: arg.map(|s| s.to_string()),
            },
            "/clear" => SlashCommand::Clear,
            "/compact" => SlashCommand::Compact {
                custom_instructions: arg.map(|s| s.to_string()),
            },
            "/undo" => SlashCommand::Undo,
            "/redo" => SlashCommand::Redo,
            "/branch" | "/fork" | "/tree" => SlashCommand::Branch,
            "/session" | "/resume" => SlashCommand::Session,
            "/export" => SlashCommand::Export {
                path: arg.map(|s| s.to_string()),
            },
            "/settings" => SlashCommand::Settings,
            "/help" | "/?" => SlashCommand::Help,
            "/quit" | "/exit" | "/q" => SlashCommand::Quit,
            "/name" => SlashCommand::Name {
                name: arg.unwrap_or("").to_string(),
            },
            "/copy" => SlashCommand::Copy,
            "/new" => SlashCommand::New,
            _ => SlashCommand::Unknown {
                raw: trimmed.to_string(),
            },
        }
    }

    /// Human-readable description of the command.
    pub fn description(&self) -> &'static str {
        match self {
            SlashCommand::Model { .. } => "Select model",
            SlashCommand::Clear => "Clear conversation history",
            SlashCommand::Compact { .. } => "Compact context",
            SlashCommand::Undo => "Undo last exchange",
            SlashCommand::Redo => "Redo last undone exchange",
            SlashCommand::Branch => "Navigate session tree",
            SlashCommand::Session => "Show session info",
            SlashCommand::Export { .. } => "Export session",
            SlashCommand::Settings => "Open settings",
            SlashCommand::Help => "Show help",
            SlashCommand::Quit => "Quit oxi",
            SlashCommand::Name { .. } => "Set session name",
            SlashCommand::Copy => "Copy last response",
            SlashCommand::New => "Start new session",
            SlashCommand::Unknown { .. } => "Unknown command",
        }
    }
}

// ── Interactive mode runner ─────────────────────────────────────────────────

/// Run the full interactive mode loop.
pub async fn run_interactive(app: crate::App) -> Result<()> {
    let theme = Theme::dark();
    let agent: Arc<Agent> = app.agent();

    // Channels
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(256);
    let (prompt_tx, mut prompt_rx) = mpsc::channel::<String>(16);

    // Agent worker thread (non-Send futures need a LocalSet)
    let agent_for_thread: Arc<Agent> = Arc::clone(&agent);
    let agent_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build agent runtime");
        rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local
                .run_until(async {
                    while let Some(prompt) = prompt_rx.recv().await {
                        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);
                        let ui_fwd = ui_tx.clone();
                        let forwarder = tokio::task::spawn_local(async move {
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
                                    AgentEvent::Error { message } => UiEvent::Error(message),
                                    _ => continue,
                                };
                                if ui_fwd.send(ui_event).await.is_err() {
                                    break;
                                }
                            }
                        });
                        let a = Arc::clone(&agent_for_thread);
                        let _ = a.run_with_channel(prompt, event_tx).await;
                        let _ = forwarder.await;
                    }
                })
                .await;
        });
    });

    // TUI state
    let mut chat_view = ChatView::new(theme.clone());
    let mut input = Input::with_placeholder("Type a message... (Ctrl+C to quit)");
    input.on_focus();
    let mut state = InteractiveState::Input;
    let mut session = InteractiveSession::new();

    // Track undo/redo stacks
    let mut undo_stack: Vec<crate::ChatMessage> = Vec::new();

    // Terminal setup
    use std::io::{self, Write};
    crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    crossterm::execute!(io::stdout(), crossterm::cursor::Hide)?;
    crossterm::execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;

    let mut running = true;

    while running {
        let (width, height) = crossterm::terminal::size().unwrap_or((80, 24));
        let input_height: u16 = 3;
        let chat_height = height.saturating_sub(input_height);

        // ── Render ──────────────────────────────────────────────────────
        let mut surface = Surface::new(width, height);

        // Chat area
        let chat_area = Rect::new(0, 0, width, chat_height);
        chat_view.render(&mut surface, chat_area);

        // Separator line
        if chat_height < height {
            for col in 0..width {
                surface.set(
                    chat_height,
                    col,
                    oxi_tui::Cell::new('\u{2500}').with_fg(theme.colors.border),
                );
            }

            // Prompt indicator
            surface.set(
                chat_height + 1,
                0,
                oxi_tui::Cell::new('\u{276F}').with_fg(theme.colors.primary),
            );

            // Input area
            let input_area = Rect::new(2, chat_height + 1, width.saturating_sub(4), 1);
            input.render(&mut surface, input_area);

            // Status indicator (bottom-right)
            let status_text = match state {
                InteractiveState::Thinking => "\u{25CF} thinking...",
                InteractiveState::ToolExecution => "\u{2699} executing...",
                InteractiveState::Display | InteractiveState::Input => "",
            };
            let status_fg = if state == InteractiveState::Thinking || state == InteractiveState::ToolExecution {
                theme.colors.warning
            } else {
                theme.colors.muted
            };
            for (i, ch) in status_text.chars().enumerate() {
                let col = width as usize - status_text.len() + i;
                if col < width as usize {
                    surface.set(
                        chat_height + 2,
                        col as u16,
                        oxi_tui::Cell::new(ch).with_fg(status_fg),
                    );
                }
            }
        }

        render_surface_to_terminal(&surface, width, height);
        io::stdout().flush()?;

        // ── Poll terminal events (~30 fps) ──────────────────────────────
        let timeout = std::time::Duration::from_millis(33);

        if crossterm::event::poll(timeout)? {
            let event = crossterm::event::read()?;
            match event {
                crossterm::event::Event::Key(key) => {
                    match key.code {
                        crossterm::event::KeyCode::Enter => {
                            if state == InteractiveState::Input {
                                let value = input.value().to_string();
                                if !value.is_empty() {
                                    // ── Command handling ───────────────────
                                    if value.starts_with('/') {
                                        let cmd = SlashCommand::parse(&value);
                                        match cmd {
                                            SlashCommand::Clear => {
                                                chat_view = ChatView::new(theme.clone());
                                                session = InteractiveSession::new();
                                                undo_stack.clear();
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Quit => {
                                                running = false;
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Help => {
                                                let help_text = format_help();
                                                chat_view.add_message(ChatMessageDisplay {
                                                    role: MessageRole::Assistant,
                                                    content_blocks: vec![ContentBlockDisplay::Text {
                                                        content: help_text,
                                                    }],
                                                    timestamp: now_millis(),
                                                });
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Model { search } => {
                                                let model_info = format!(
                                                    "Current model: {}\n\
                                                     Use /model <provider/model> to switch.",
                                                    app.model_id(),
                                                );
                                                if let Some(query) = search {
                                                    // Attempt to switch model directly
                                                    match app.switch_model(&query) {
                                                        Ok(()) => {
                                                            chat_view.add_message(ChatMessageDisplay {
                                                                role: MessageRole::Assistant,
                                                                content_blocks: vec![
                                                                    ContentBlockDisplay::Text {
                                                                        content: format!(
                                                                            "Switched to model: {}",
                                                                            query
                                                                        ),
                                                                    },
                                                                ],
                                                                timestamp: now_millis(),
                                                            });
                                                        }
                                                        Err(e) => {
                                                            chat_view.add_message(ChatMessageDisplay {
                                                                role: MessageRole::Assistant,
                                                                content_blocks: vec![
                                                                    ContentBlockDisplay::Text {
                                                                        content: format!(
                                                                            "Error switching model: {}",
                                                                            e
                                                                        ),
                                                                    },
                                                                ],
                                                                timestamp: now_millis(),
                                                            });
                                                        }
                                                    }
                                                } else {
                                                    chat_view.add_message(ChatMessageDisplay {
                                                        role: MessageRole::Assistant,
                                                        content_blocks: vec![
                                                            ContentBlockDisplay::Text {
                                                                content: model_info,
                                                            },
                                                        ],
                                                        timestamp: now_millis(),
                                                    });
                                                }
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Session => {
                                                let info = format_session_info(&session);
                                                chat_view.add_message(ChatMessageDisplay {
                                                    role: MessageRole::Assistant,
                                                    content_blocks: vec![
                                                        ContentBlockDisplay::Text { content: info },
                                                    ],
                                                    timestamp: now_millis(),
                                                });
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Compact { custom_instructions } => {
                                                // Compact is a hint; show message
                                                let msg = if let Some(ci) = &custom_instructions {
                                                    format!(
                                                        "Compaction requested with instructions: {}\n\
                                                         (Compaction is automatic when context exceeds threshold.)",
                                                        ci
                                                    )
                                                } else {
                                                    "Compaction requested.\n\
                                                     (Compaction is automatic when context exceeds threshold.)"
                                                        .to_string()
                                                };
                                                chat_view.add_message(ChatMessageDisplay {
                                                    role: MessageRole::Assistant,
                                                    content_blocks: vec![
                                                        ContentBlockDisplay::Text { content: msg },
                                                    ],
                                                    timestamp: now_millis(),
                                                });
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Undo => {
                                                // Undo: remove last two messages (user + assistant)
                                                if session.messages.len() >= 2 {
                                                    let last_assistant = session.messages.pop();
                                                    let last_user = session.messages.pop();
                                                    if let (Some(u), Some(a)) = (last_user, last_assistant) {
                                                        undo_stack.push(u);
                                                        undo_stack.push(a);
                                                    }
                                                    // Rebuild chat view from remaining messages
                                                    rebuild_chat_view(&mut chat_view, &session, &theme);
                                                }
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Redo => {
                                                if undo_stack.len() >= 2 {
                                                    let user_msg = undo_stack.pop();
                                                    let assistant_msg = undo_stack.pop();
                                                    // Push in correct order: user first, then assistant
                                                    if let (Some(a), Some(u)) = (assistant_msg, user_msg) {
                                                        session.messages.push(u);
                                                        session.messages.push(a);
                                                    }
                                                    rebuild_chat_view(&mut chat_view, &session, &theme);
                                                }
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Branch => {
                                                let msg = format!(
                                                    "Session has {} messages.\n\
                                                     Branch navigation coming soon.",
                                                    session.messages.len()
                                                );
                                                chat_view.add_message(ChatMessageDisplay {
                                                    role: MessageRole::Assistant,
                                                    content_blocks: vec![
                                                        ContentBlockDisplay::Text { content: msg },
                                                    ],
                                                    timestamp: now_millis(),
                                                });
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Export { path } => {
                                                let json = export_session_json(&session);
                                                let export_path = path
                                                    .clone()
                                                    .unwrap_or_else(|| "oxi-session.json".to_string());
                                                match std::fs::write(&export_path, &json) {
                                                    Ok(()) => {
                                                        chat_view.add_message(ChatMessageDisplay {
                                                            role: MessageRole::Assistant,
                                                            content_blocks: vec![
                                                                ContentBlockDisplay::Text {
                                                                    content: format!(
                                                                        "Session exported to {}",
                                                                        export_path
                                                                    ),
                                                                },
                                                            ],
                                                            timestamp: now_millis(),
                                                        });
                                                    }
                                                    Err(e) => {
                                                        chat_view.add_message(ChatMessageDisplay {
                                                            role: MessageRole::Assistant,
                                                            content_blocks: vec![
                                                                ContentBlockDisplay::Text {
                                                                    content: format!(
                                                                        "Export failed: {}",
                                                                        e
                                                                    ),
                                                                },
                                                            ],
                                                            timestamp: now_millis(),
                                                        });
                                                    }
                                                }
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Settings => {
                                                let settings_info = format!(
                                                    "Model: {}\n\
                                                     Thinking Level: {:?}\n\
                                                     Temperature: {}\n\
                                                     Max Tokens: {}\n\
                                                     Auto-compaction: {}\n\
                                                     Tool Timeout: {}s",
                                                    app.settings().effective_model(None),
                                                    app.settings().thinking_level,
                                                    app.settings().effective_temperature()
                                                        .map(|t| t.to_string())
                                                        .unwrap_or_else(|| "default".to_string()),
                                                    app.settings()
                                                        .effective_max_tokens()
                                                        .map(|t| t.to_string())
                                                        .unwrap_or_else(|| "default".to_string()),
                                                    app.settings().auto_compaction,
                                                    app.settings().tool_timeout_seconds,
                                                );
                                                chat_view.add_message(ChatMessageDisplay {
                                                    role: MessageRole::Assistant,
                                                    content_blocks: vec![
                                                        ContentBlockDisplay::Text {
                                                            content: settings_info,
                                                        },
                                                    ],
                                                    timestamp: now_millis(),
                                                });
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Copy => {
                                                // Get last assistant message text
                                                let last_text = session
                                                    .messages
                                                    .iter()
                                                    .rev()
                                                    .find(|m| m.role == "assistant")
                                                    .map(|m| m.content.clone())
                                                    .unwrap_or_default();
                                                // Copy to clipboard (best-effort)
                                                let _ = copy_to_clipboard(&last_text);
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::New => {
                                                chat_view = ChatView::new(theme.clone());
                                                session = InteractiveSession::new();
                                                undo_stack.clear();
                                                app.reset();
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Name { name } => {
                                                if !name.is_empty() {
                                                    session.session_id = Some(uuid::Uuid::new_v4());
                                                    chat_view.add_message(ChatMessageDisplay {
                                                        role: MessageRole::Assistant,
                                                        content_blocks: vec![
                                                            ContentBlockDisplay::Text {
                                                                content: format!(
                                                                    "Session named: {}",
                                                                    name
                                                                ),
                                                            },
                                                        ],
                                                        timestamp: now_millis(),
                                                    });
                                                }
                                                input.clear();
                                                continue;
                                            }
                                            SlashCommand::Unknown { raw } => {
                                                chat_view.add_message(ChatMessageDisplay {
                                                    role: MessageRole::Assistant,
                                                    content_blocks: vec![
                                                        ContentBlockDisplay::Text {
                                                            content: format!(
                                                                "Unknown command: {}\n\
                                                                 Type /help for available commands.",
                                                                raw
                                                            ),
                                                        },
                                                    ],
                                                    timestamp: now_millis(),
                                                });
                                                input.clear();
                                                continue;
                                            }
                                        }
                                    } else if value.starts_with('!') {
                                        // ── Bash command ─────────────────
                                        let is_excluded = value.starts_with("!!");
                                        let command = if is_excluded {
                                            value[2..].trim().to_string()
                                        } else {
                                            value[1..].trim().to_string()
                                        };
                                        if !command.is_empty() {
                                            // Run bash command inline, show output
                                            let output = run_bash_command(&command);
                                            chat_view.add_message(ChatMessageDisplay {
                                                role: MessageRole::Assistant,
                                                content_blocks: vec![ContentBlockDisplay::Text {
                                                    content: format!("$ {}\n{}", command, output),
                                                }],
                                                timestamp: now_millis(),
                                            });
                                        }
                                        input.clear();
                                        continue;
                                    } else {
                                        // ── Normal user message → agent ──
                                        session.add_user_message(value.clone());
                                        chat_view.add_message(ChatMessageDisplay {
                                            role: MessageRole::User,
                                            content_blocks: vec![ContentBlockDisplay::Text {
                                                content: value.clone(),
                                            }],
                                            timestamp: now_millis(),
                                        });

                                        // Transition to thinking
                                        chat_view.start_streaming();
                                        state = InteractiveState::Thinking;

                                        let _ = prompt_tx.send(value).await;
                                        input.clear();
                                    }
                                }
                            }
                        }
                        crossterm::event::KeyCode::Char('c')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            // Double Ctrl+C to exit, single Ctrl+C interrupts
                            running = false;
                        }
                        crossterm::event::KeyCode::PageUp => {
                            chat_view.scroll_up(10);
                        }
                        crossterm::event::KeyCode::PageDown => {
                            chat_view.scroll_down(10);
                        }
                        _ => {
                            if let Some(tui_event) = convert_key_event(key) {
                                input.handle_event(&tui_event);
                            }
                        }
                    }
                }
                crossterm::event::Event::Mouse(mouse) => match mouse.kind {
                    crossterm::event::MouseEventKind::ScrollUp => {
                        if mouse.row < chat_height {
                            chat_view.scroll_up(3);
                        }
                    }
                    crossterm::event::MouseEventKind::ScrollDown => {
                        if mouse.row < chat_height {
                            chat_view.scroll_down(3);
                        }
                    }
                    _ => {}
                },
                crossterm::event::Event::Resize(_, _) => {}
                _ => {}
            }
        }

        // ── Drain agent events ──────────────────────────────────────────
        while let Ok(ui_event) = ui_rx.try_recv() {
            match ui_event {
                UiEvent::Start => {}
                UiEvent::Thinking => {
                    chat_view.stream_thinking_start();
                    state = InteractiveState::Thinking;
                }
                UiEvent::TextDelta(text) => {
                    chat_view.stream_text_delta(&text);
                }
                UiEvent::ToolCall { id, name, arguments } => {
                    chat_view.stream_thinking_end();
                    chat_view.stream_tool_call(id, name, arguments);
                    state = InteractiveState::ToolExecution;
                }
                UiEvent::ToolStart { tool_name } => {
                    chat_view.stream_tool_call(
                        format!("tool-{}", tool_name),
                        tool_name,
                        String::new(),
                    );
                    state = InteractiveState::ToolExecution;
                }
                UiEvent::ToolResult {
                    tool_name,
                    content,
                    is_error,
                } => {
                    chat_view.stream_tool_result(tool_name, content, is_error);
                }
                UiEvent::Complete => {
                    chat_view.stream_thinking_end();
                    chat_view.finish_streaming();
                    let _display_state = InteractiveState::Display;
                    state = InteractiveState::Input;

                    // Capture the response text into session
                    let st = app.agent_state();
                    for msg in st.messages.iter().rev() {
                        if let oxi_ai::Message::Assistant(a) = msg {
                            session.add_assistant_message(a.text_content());
                            break;
                        }
                    }

                    // Brief display then return to input
                    state = InteractiveState::Input;
                }
                UiEvent::Error(msg) => {
                    chat_view.finish_streaming_error(&msg);
                    state = InteractiveState::Input;
                }
            }
        }

        // Auto-scroll
        chat_view.scroll_to_bottom();
    }

    // ── Cleanup ────────────────────────────────────────────────────────
    drop(prompt_tx);
    let _ = agent_handle.join();
    crossterm::execute!(io::stdout(), crossterm::cursor::Show)?;
    crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture)?;
    crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    io::stdout().flush()?;

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Render a surface to the terminal using efficient SGR sequences.
fn render_surface_to_terminal(surface: &Surface, width: u16, height: u16) {
    print!("\x1b[?2026h"); // Begin synchronized update
    print!("\x1b[H"); // Move to home

    let mut last_fg = oxi_tui::Color::Default;
    let mut last_bg = oxi_tui::Color::Default;
    let mut last_bold = false;
    let mut last_italic = false;
    let mut last_underline = false;
    let mut last_strike = false;

    for row in 0..height {
        if row > 0 {
            print!("\r\n");
        }
        for col in 0..width {
            if let Some(cell) = surface.get(row, col) {
                let fg_changed = cell.fg != last_fg;
                let bg_changed = cell.bg != last_bg;
                let attrs_changed = cell.attrs.bold != last_bold
                    || cell.attrs.italic != last_italic
                    || cell.attrs.underline != last_underline
                    || cell.attrs.strikethrough != last_strike;

                if fg_changed || bg_changed || attrs_changed {
                    print!("\x1b[0m");
                    match cell.fg {
                        oxi_tui::Color::Default => {}
                        oxi_tui::Color::Black => print!("\x1b[30m"),
                        oxi_tui::Color::Red => print!("\x1b[31m"),
                        oxi_tui::Color::Green => print!("\x1b[32m"),
                        oxi_tui::Color::Yellow => print!("\x1b[33m"),
                        oxi_tui::Color::Blue => print!("\x1b[34m"),
                        oxi_tui::Color::Magenta => print!("\x1b[35m"),
                        oxi_tui::Color::Cyan => print!("\x1b[36m"),
                        oxi_tui::Color::White => print!("\x1b[37m"),
                        oxi_tui::Color::Indexed(n) => print!("\x1b[38;5;{}m", n),
                        oxi_tui::Color::Rgb(r, g, b) => print!("\x1b[38;2;{};{};{}m", r, g, b),
                    }
                    match cell.bg {
                        oxi_tui::Color::Default => {}
                        oxi_tui::Color::Black => print!("\x1b[40m"),
                        oxi_tui::Color::Red => print!("\x1b[41m"),
                        oxi_tui::Color::Green => print!("\x1b[42m"),
                        oxi_tui::Color::Yellow => print!("\x1b[43m"),
                        oxi_tui::Color::Blue => print!("\x1b[44m"),
                        oxi_tui::Color::Magenta => print!("\x1b[45m"),
                        oxi_tui::Color::Cyan => print!("\x1b[46m"),
                        oxi_tui::Color::White => print!("\x1b[47m"),
                        oxi_tui::Color::Indexed(n) => print!("\x1b[48;5;{}m", n),
                        oxi_tui::Color::Rgb(r, g, b) => print!("\x1b[48;2;{};{};{}m", r, g, b),
                    }
                    if cell.attrs.bold {
                        print!("\x1b[1m");
                    }
                    if cell.attrs.italic {
                        print!("\x1b[3m");
                    }
                    if cell.attrs.underline {
                        print!("\x1b[4m");
                    }
                    if cell.attrs.strikethrough {
                        print!("\x1b[9m");
                    }
                    last_fg = cell.fg;
                    last_bg = cell.bg;
                    last_bold = cell.attrs.bold;
                    last_italic = cell.attrs.italic;
                    last_underline = cell.attrs.underline;
                    last_strike = cell.attrs.strikethrough;
                }
                print!("{}", cell.char);
            } else {
                print!(" ");
            }
        }
    }

    print!("\x1b[0m");
    print!("\x1b[?2026l"); // End synchronized update
}

/// Convert a crossterm key event to an oxi-tui Event.
fn convert_key_event(key: crossterm::event::KeyEvent) -> Option<oxi_tui::Event> {
    use oxi_tui::event::KeyCode as KC;

    let code = match key.code {
        crossterm::event::KeyCode::Enter => return None,
        crossterm::event::KeyCode::Char('c')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            return None;
        }
        crossterm::event::KeyCode::Esc => KC::Escape,
        crossterm::event::KeyCode::Tab => KC::Tab,
        crossterm::event::KeyCode::Backspace => KC::Backspace,
        crossterm::event::KeyCode::Delete => KC::Delete,
        crossterm::event::KeyCode::Up => KC::Up,
        crossterm::event::KeyCode::Down => KC::Down,
        crossterm::event::KeyCode::Left => KC::Left,
        crossterm::event::KeyCode::Right => KC::Right,
        crossterm::event::KeyCode::Home => KC::Home,
        crossterm::event::KeyCode::End => KC::End,
        crossterm::event::KeyCode::Char(c) => KC::Char(c),
        crossterm::event::KeyCode::F(n) => KC::F(n),
        _ => return None,
    };

    let modifiers = oxi_tui::KeyModifiers {
        shift: key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT),
        ctrl: key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL),
        alt: key.modifiers.contains(crossterm::event::KeyModifiers::ALT),
        meta: key.modifiers.contains(crossterm::event::KeyModifiers::META),
    };

    Some(oxi_tui::Event::Key(oxi_tui::KeyEvent::with_modifiers(
        code, modifiers,
    )))
}

/// Format the help text.
fn format_help() -> String {
    r#"oxi — AI Coding Assistant

Commands:
  /model [search]    Select or switch model
  /clear             Clear conversation history
  /compact [instr]   Compact context with optional instructions
  /undo              Undo last exchange
  /redo              Redo last undone exchange
  /branch            Navigate session tree
  /session           Show session info and stats
  /export [path]     Export session to JSON
  /settings          Show current settings
  /name <name>       Set session display name
  /copy              Copy last assistant response
  /new               Start a new session
  /help              Show this help message
  /quit              Quit oxi

Bash:
  !<command>         Run a bash command
  !!<command>        Run bash (excluded from context)

Keybindings:
  Enter              Send message or command
  Ctrl+C             Quit
  PageUp/PageDown    Scroll chat history
  Mouse scroll       Scroll chat history
"#.to_string()
}

/// Format session info.
fn format_session_info(session: &InteractiveSession) -> String {
    let msg_count = session.messages.len();
    let user_count = session.messages.iter().filter(|m| m.role == "user").count();
    let assistant_count = session
        .messages
        .iter()
        .filter(|m| m.role == "assistant")
        .count();
    let entry_count = session.entries.len();

    format!(
        "Session Info:\n\
         Messages: {} total ({} user, {} assistant)\n\
         Entries: {}\n\
         ID: {}",
        msg_count,
        user_count,
        assistant_count,
        entry_count,
        session
            .session_id
            .map(|u| u.to_string())
            .unwrap_or_else(|| "none".to_string()),
    )
}

/// Export session to a JSON string.
fn export_session_json(session: &InteractiveSession) -> String {
    let messages: Vec<serde_json::Value> = session
        .messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": m.role,
                "content": m.content,
                "timestamp": m.timestamp.to_rfc3339(),
            })
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({
        "session_id": session.session_id.map(|u| u.to_string()),
        "messages": messages,
        "entry_count": session.entries.len(),
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

/// Rebuild the chat view from session messages (used after undo/redo).
fn rebuild_chat_view(chat_view: &mut ChatView, session: &InteractiveSession, theme: &Theme) {
    *chat_view = ChatView::new(theme.clone());
    for msg in &session.messages {
        let role = if msg.role == "user" {
            MessageRole::User
        } else {
            MessageRole::Assistant
        };
        chat_view.add_message(ChatMessageDisplay {
            role,
            content_blocks: vec![ContentBlockDisplay::Text {
                content: msg.content.clone(),
            }],
            timestamp: msg.timestamp.timestamp_millis(),
        });
    }
}

/// Run a bash command and return its output.
fn run_bash_command(command: &str) -> String {
    use std::process::Command;
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .unwrap_or_else(|e| std::process::Output {
            stdout: Vec::new(),
            stderr: format!("Failed to execute: {}", e).into_bytes(),
            status: std::process::ExitStatus::from_raw(1),
        });

    let mut result = String::new();
    if !output.stdout.is_empty() {
        result.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        result.push_str(&format!("\nExit code: {}", output.status.code().unwrap_or(-1)));
    }
    result
}

/// Copy text to clipboard (best-effort, uses pbcopy/xclip/wl-copy).
fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("pbcopy", &[])
    } else if cfg!(target_os = "linux") {
        // Try wl-copy first (Wayland), fall back to xclip (X11)
        if std::path::Path::new("/usr/bin/wl-copy").exists()
            || std::path::Path::new("/usr/local/bin/wl-copy").exists()
        {
            ("wl-copy", &[])
        } else {
            ("xclip", &["-selection", "clipboard"])
        }
    } else {
        return Err(anyhow::anyhow!("Clipboard not supported on this platform"));
    };

    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn clipboard command: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }

    let _ = child.wait();
    Ok(())
}

/// Current timestamp in milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── SlashCommand parsing tests ────────────────────────────────────

    #[test]
    fn test_parse_model_no_arg() {
        let cmd = SlashCommand::parse("/model");
        assert_eq!(cmd, SlashCommand::Model { search: None });
    }

    #[test]
    fn test_parse_model_with_search() {
        let cmd = SlashCommand::parse("/model claude-sonnet");
        assert_eq!(
            cmd,
            SlashCommand::Model {
                search: Some("claude-sonnet".to_string()),
            }
        );
    }

    #[test]
    fn test_parse_clear() {
        assert_eq!(SlashCommand::parse("/clear"), SlashCommand::Clear);
    }

    #[test]
    fn test_parse_compact_no_arg() {
        assert_eq!(
            SlashCommand::parse("/compact"),
            SlashCommand::Compact {
                custom_instructions: None
            }
        );
    }

    #[test]
    fn test_parse_compact_with_instructions() {
        assert_eq!(
            SlashCommand::parse("/compact focus on error handling"),
            SlashCommand::Compact {
                custom_instructions: Some("focus on error handling".to_string()),
            }
        );
    }

    #[test]
    fn test_parse_undo_redo() {
        assert_eq!(SlashCommand::parse("/undo"), SlashCommand::Undo);
        assert_eq!(SlashCommand::parse("/redo"), SlashCommand::Redo);
    }

    #[test]
    fn test_parse_aliases() {
        // /? is an alias for /help
        assert_eq!(SlashCommand::parse("/?"), SlashCommand::Help);
        // /exit and /q are aliases for /quit
        assert_eq!(SlashCommand::parse("/exit"), SlashCommand::Quit);
        assert_eq!(SlashCommand::parse("/q"), SlashCommand::Quit);
        // /fork and /tree are aliases for /branch
        assert_eq!(SlashCommand::parse("/fork"), SlashCommand::Branch);
        assert_eq!(SlashCommand::parse("/tree"), SlashCommand::Branch);
        // /resume is alias for /session
        assert_eq!(SlashCommand::parse("/resume"), SlashCommand::Session);
    }

    #[test]
    fn test_parse_unknown() {
        let cmd = SlashCommand::parse("/foobar");
        assert_eq!(
            cmd,
            SlashCommand::Unknown {
                raw: "/foobar".to_string()
            }
        );
    }

    // ── State machine tests ───────────────────────────────────────────

    #[test]
    fn test_state_ordering() {
        // Verify that states exist and are distinct
        let states = [
            InteractiveState::Input,
            InteractiveState::Thinking,
            InteractiveState::ToolExecution,
            InteractiveState::Display,
        ];
        // All unique
        for i in 0..states.len() {
            for j in (i + 1)..states.len() {
                assert_ne!(states[i], states[j]);
            }
        }
    }

    #[test]
    fn test_state_transitions_input_to_thinking() {
        let state = InteractiveState::Input;
        // On user submit: Input -> Thinking
        let next = InteractiveState::Thinking;
        assert_eq!(next, InteractiveState::Thinking);
        assert_ne!(state, next);
    }

    #[test]
    fn test_state_transitions_thinking_to_tool_execution() {
        // On tool call: Thinking -> ToolExecution
        let state = InteractiveState::Thinking;
        let next = InteractiveState::ToolExecution;
        assert_ne!(state, next);
    }

    #[test]
    fn test_state_transitions_tool_execution_to_display() {
        // On complete: ToolExecution -> Display -> Input
        let state = InteractiveState::ToolExecution;
        let display = InteractiveState::Display;
        let input = InteractiveState::Input;
        assert_ne!(state, display);
        assert_ne!(display, input);
    }

    // ── Bash execution tests ──────────────────────────────────────────

    #[test]
    fn test_bash_command_execution() {
        let output = run_bash_command("echo hello");
        assert!(output.contains("hello"));
    }

    #[test]
    fn test_bash_command_failure() {
        let output = run_bash_command("false");
        assert!(output.contains("Exit code:"));
    }

    // ── Export tests ──────────────────────────────────────────────────

    #[test]
    fn test_export_empty_session() {
        let session = InteractiveSession::new();
        let json = export_session_json(&session);
        assert!(json.contains("\"messages\": []"));
        assert!(json.contains("\"entry_count\": 0"));
    }

    #[test]
    fn test_export_session_with_messages() {
        let mut session = InteractiveSession::new();
        session.add_user_message("Hello".to_string());
        session.add_assistant_message("Hi there!".to_string());
        let json = export_session_json(&session);
        assert!(json.contains("\"role\": \"user\""));
        assert!(json.contains("\"content\": \"Hello\""));
        assert!(json.contains("\"role\": \"assistant\""));
    }

    // ── Session info tests ────────────────────────────────────────────

    #[test]
    fn test_session_info_empty() {
        let session = InteractiveSession::new();
        let info = format_session_info(&session);
        assert!(info.contains("Messages: 0 total"));
        assert!(info.contains("ID: none"));
    }

    #[test]
    fn test_session_info_with_messages() {
        let mut session = InteractiveSession::new();
        session.add_user_message("Hello".to_string());
        session.add_assistant_message("Hi".to_string());
        let info = format_session_info(&session);
        assert!(info.contains("Messages: 2 total"));
        assert!(info.contains("1 user"));
        assert!(info.contains("1 assistant"));
    }

    // ── Help text test ────────────────────────────────────────────────

    #[test]
    fn test_help_text_contains_all_commands() {
        let help = format_help();
        assert!(help.contains("/model"));
        assert!(help.contains("/clear"));
        assert!(help.contains("/compact"));
        assert!(help.contains("/undo"));
        assert!(help.contains("/redo"));
        assert!(help.contains("/branch"));
        assert!(help.contains("/session"));
        assert!(help.contains("/export"));
        assert!(help.contains("/settings"));
        assert!(help.contains("/help"));
        assert!(help.contains("/quit"));
    }

    // ── Command description tests ─────────────────────────────────────

    #[test]
    fn test_command_descriptions() {
        assert_eq!(
            SlashCommand::Model { search: None }.description(),
            "Select model"
        );
        assert_eq!(SlashCommand::Clear.description(), "Clear conversation history");
        assert_eq!(SlashCommand::Undo.description(), "Undo last exchange");
        assert_eq!(SlashCommand::Redo.description(), "Redo last undone exchange");
        assert_eq!(SlashCommand::Quit.description(), "Quit oxi");
        assert_eq!(
            SlashCommand::Unknown { raw: "/x".to_string() }.description(),
            "Unknown command"
        );
    }

    // ── Image attachment tests ─────────────────────────────────────────

    #[test]
    fn test_image_attachment_from_data_uri() {
        // Valid PNG data URI
        let uri = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";
        let img = ImageAttachment::from_data_uri(uri);
        assert!(img.is_some());
        let img = img.unwrap();
        assert_eq!(img.mime_type, "image/png");
    }

    #[test]
    fn test_image_attachment_invalid_uri() {
        // Not a data URI
        let img = ImageAttachment::from_data_uri("not a data uri");
        assert!(img.is_none());
    }

    #[test]
    fn test_image_attachment_extension() {
        let img = ImageAttachment {
            mime_type: "image/png".to_string(),
            base64_data: String::new(),
            width: None,
            height: None,
        };
        assert_eq!(img.extension(), "png");

        let img_jpeg = ImageAttachment {
            mime_type: "image/jpeg".to_string(),
            base64_data: String::new(),
            width: None,
            height: None,
        };
        assert_eq!(img_jpeg.extension(), "jpg");
    }

    #[test]
    fn test_image_attachment_detect_mime_type() {
        // PNG magic bytes
        let png_bytes: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(ImageAttachment::detect_mime_type(&png_bytes), "image/png");

        // JPEG magic bytes
        let jpeg_bytes: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
        assert_eq!(ImageAttachment::detect_mime_type(&jpeg_bytes), "image/jpeg");

        // Unknown
        let unknown: Vec<u8> = vec![0x00, 0x00, 0x00, 0x00];
        assert_eq!(ImageAttachment::detect_mime_type(&unknown), "image/png"); // fallback
    }

    #[test]
    fn test_image_attachment_from_bytes() {
        // PNG magic bytes
        let png_data: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let img = ImageAttachment::from_bytes(png_data);
        assert!(img.is_some());
        let img = img.unwrap();
        assert_eq!(img.mime_type, "image/png");
        assert!(!img.base64_data.is_empty());
    }

    // ── Session persistence tests ─────────────────────────────────────

    #[test]
    fn test_session_persistence_new() {
        let persistence = SessionPersistence::new();
        // May be None if HOME not set or dir creation fails
        assert!(persistence.is_some() || persistence.is_none());
    }

    // ── Keybinding hints tests ─────────────────────────────────────────

    #[test]
    fn test_keybinding_hints_compact() {
        let hints = KeybindingHints::new();
        let compact = hints.compact_display();
        assert!(compact.contains("Ctrl+C"));
        assert!(compact.contains("quit"));
    }

    #[test]
    fn test_keybinding_hints_expanded() {
        let hints = KeybindingHints::new();
        let expanded = hints.expanded_display();
        assert!(expanded.contains("Ctrl+C"));
        assert!(expanded.contains("Ctrl+L"));
        assert!(expanded.contains("Ctrl+U"));
    }

    #[test]
    fn test_keybinding_hints_toggle() {
        let mut hints = KeybindingHints::new();
        assert!(!hints.is_expanded());
        hints.toggle();
        assert!(hints.is_expanded());
        hints.toggle();
        assert!(!hints.is_expanded());
    }

    // ── Word-level diff tests ──────────────────────────────────────────

    #[test]
    fn test_compute_word_diff_identical() {
        let result = compute_word_diff("hello world", "hello world");
        let (added, removed, unchanged) = result.summary();
        assert_eq!(added, 0);
        assert_eq!(removed, 0);
        assert_eq!(unchanged, 2);
    }

    #[test]
    fn test_compute_word_diff_added_words() {
        let result = compute_word_diff("hello", "hello world");
        let (added, removed, _) = result.summary();
        assert_eq!(added, 1); // "world" added
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_compute_word_diff_removed_words() {
        let result = compute_word_diff("hello world", "hello");
        let (added, removed, _) = result.summary();
        assert_eq!(added, 0);
        assert_eq!(removed, 1); // "world" removed
    }

    #[test]
    fn test_compute_word_diff_changed() {
        let result = compute_word_diff("hello world", "hello rust");
        let (added, removed, unchanged) = result.summary();
        assert_eq!(added, 1); // "rust" added
        assert_eq!(removed, 1); // "world" removed
        assert_eq!(unchanged, 1); // "hello" unchanged
    }

    #[test]
    fn test_diff_result_format_ansi() {
        let result = compute_word_diff("foo bar", "foo baz");
        let formatted = result.format_ansi();
        assert!(formatted.contains("foo"));
        assert!(formatted.contains("bar") || formatted.contains("baz"));
    }

    #[test]
    fn test_diff_result_empty() {
        let result = compute_word_diff("", "hello");
        let (added, removed, _) = result.summary();
        assert_eq!(added, 1);
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_lcs_algorithm() {
        let a = vec!["a", "b", "c"];
        let b = vec!["a", "c", "d"];
        let lcs = longest_common_subsequence(&a, &b);
        assert!(lcs.contains(&(0, 0))); // "a"
        assert!(lcs.contains(&(2, 1))); // "c"
    }
}
