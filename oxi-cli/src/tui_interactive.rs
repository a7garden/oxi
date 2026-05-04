//! TUI-based interactive mode using oxi-tui components.
//!
//! Wires together ChatView, Input, Markdown, and Image components
//! into a cohesive terminal chat experience, using [`AgentSession`]
//! as the core session abstraction instead of bare [`Agent`].
//!
//! # Architecture
//!
//! ```text
//! main.rs
//!   └─ run_tui_interactive(app)
//!        │
//!        ├─ AgentSession  (session wrapper)
//!        │    └─ Agent  (core agent loop)
//!        │
//!        ├─ SessionManager  (persistence)
//!        ├─ Settings  (configuration)
//!        ├─ ExtensionRunner  (extension hooks)
//!        │
//!        └─ TUI event loop  (ChatView + Input)
//! ```

use crate::agent_session::{AgentSession, CompactionReason, SessionEvent};
use crate::agent_session_runtime::{
    create_agent_session_from_services, create_agent_session_services,
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
};
use crate::session::SessionManager;
use anyhow::Result;
use oxi_agent::AgentEvent;
use oxi_tui::component::Component;
use oxi_tui::{
    ChatMessageDisplay, ChatView, ContentBlockDisplay, Input, MessageRole, Surface, Theme,
};
use std::sync::Arc;
use tokio::sync::mpsc;

// ═══════════════════════════════════════════════════════════════════════════
// UI events (from agent → TUI)
// ═══════════════════════════════════════════════════════════════════════════

/// Messages sent from the agent/session layer to the TUI event loop.
#[derive(Debug)]
enum UiEvent {
    /// Agent started.
    Start,
    /// Agent is thinking.
    Thinking,
    /// Text delta from agent streaming.
    TextDelta(String),
    /// Tool call started.
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    /// Tool completed.
    ToolResult {
        tool_name: String,
        content: String,
        is_error: bool,
    },
    /// Agent response complete.
    Complete,
    /// Agent error.
    Error(String),
    /// Compaction started.
    CompactionStart {
        reason: CompactionReason,
    },
    /// Compaction finished.
    CompactionEnd {
        _reason: CompactionReason,
        error_message: Option<String>,
    },
    /// Auto-retry started.
    RetryStart {
        attempt: u32,
        max_attempts: u32,
        error_message: String,
    },
    /// Model changed.
    ModelChanged {
        model_id: String,
    },
    /// Thinking level changed.
    ThinkingLevelChanged {
        level: String,
    },
    /// Queue updated.
    QueueUpdate {
        pending: usize,
    },
}

// ═══════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Run the TUI-based interactive mode.
///
/// Creates an [`AgentSession`] from the app's settings, wiring up
/// session persistence, auto-compaction, auto-retry, extension hooks,
/// and settings changes.
pub async fn run_tui_interactive(app: crate::App) -> Result<()> {
    let theme = Theme::dark();
    let settings = app.settings().clone();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    // ── Build AgentSession via the runtime factory ───────────────────
    let session_manager = SessionManager::create(&cwd, None);
    let session_id = session_manager.get_session_id();

    // Create services
    let services = create_agent_session_services(
        CreateAgentSessionServicesOptions::new(std::env::current_dir().unwrap_or_default()),
    )?;
    let services = Arc::new(services);

    // Create agent session from services
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

    // Show model fallback message if any
    if let Some(msg) = create_result.model_fallback_message {
        tracing::warn!("Model fallback: {}", msg);
    }

    // ── Subscribe to session events ──────────────────────────────────
    let (session_event_tx, mut session_event_rx) = mpsc::unbounded_channel::<SessionEvent>();

    agent_session.subscribe(Box::new(move |event| {
        let _ = session_event_tx.send(event.clone());
    }));

    // Channel for agent → UI communication (for streaming text/tool events)
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(256);

    // Channel for user input → agent execution
    let (prompt_tx, mut prompt_rx) = mpsc::channel::<String>(16);

    // ── Agent worker thread ──────────────────────────────────────────
    // The agent uses non-Send futures internally, so it needs a
    // single-threaded runtime with LocalSet.
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

                        // Forward agent events to UI
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
                                        format!("\n\u{2699} Running: {}...\n", tool_name),
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

                        // Run agent with channel using the session's underlying agent
                        let sh = session_handle.clone_handle();
                        let agent = sh.agent_ref();
                        let _ = agent.run_with_channel(prompt, event_tx).await;
                        let _ = event_forwarder.await;
                    }
                })
                .await;
        });
    });

    // ── Build TUI components ─────────────────────────────────────────
    let mut chat_view = ChatView::new(theme.clone());
    let mut input = Input::with_placeholder("Type a message... (Ctrl+C to quit)");
    input.on_focus();
    let mut is_agent_busy = false;

    // Display session info at start
    chat_view.add_message(ChatMessageDisplay {
        role: MessageRole::Assistant,
        content_blocks: vec![ContentBlockDisplay::Text {
            content: format!(
                "oxi ready. Session: {}\nModel: {}\nType /help for commands.",
                session_id,
                agent_session.model_id(),
            ),
        }],
        timestamp: now_millis(),
    });

    use std::io::{self, Write};

    // Enter alternate screen
    crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    crossterm::execute!(io::stdout(), crossterm::cursor::Hide)?;
    crossterm::execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;

    let mut running = true;

    while running {
        // Get terminal size
        let (width, height) = crossterm::terminal::size().unwrap_or((80, 24));
        let input_height: u16 = 3;
        let chat_height = height.saturating_sub(input_height);

        // Create surface and render layout
        let mut surface = Surface::new(width, height);

        // Render chat view in upper area
        let chat_area = oxi_tui::Rect::new(0, 0, width, chat_height);
        chat_view.render(&mut surface, chat_area);

        // Render separator
        if chat_height < height {
            let sep_y = chat_height;
            for col in 0..width {
                let cell = oxi_tui::Cell::new('\u{2500}').with_fg(theme.colors.border);
                surface.set(sep_y, col, cell);
            }

            // Render prompt indicator
            surface.set(
                chat_height + 1,
                0,
                oxi_tui::Cell::new('\u{276F}').with_fg(theme.colors.primary),
            );

            // Render input area
            let input_area = oxi_tui::Rect::new(2, chat_height + 1, width.saturating_sub(4), 1);
            input.render(&mut surface, input_area);

            // Status indicator in bottom-right
            let status_text = if is_agent_busy {
                "\u{25CF} thinking..."
            } else {
                ""
            };
            let status_fg = if is_agent_busy {
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

        // Render surface to terminal
        render_surface_to_terminal(&surface, width, height);
        io::stdout().flush()?;

        // Poll for events with timeout (~30fps)
        let timeout = std::time::Duration::from_millis(33);

        if crossterm::event::poll(timeout)? {
            let event = crossterm::event::read()?;
            match event {
                crossterm::event::Event::Key(key) => {
                    match key.code {
                        crossterm::event::KeyCode::Enter => {
                            if !is_agent_busy {
                                let value = input.value().to_string();
                                if !value.is_empty() {
                                    // Handle slash commands
                                    if value.starts_with('/') {
                                        let handled = handle_slash_command(
                                            &value,
                                            &agent_session,
                                            &mut chat_view,
                                            &theme,
                                            &mut running,
                                        );
                                        input.clear();
                                        if handled {
                                            continue;
                                        }
                                        // If not handled, fall through to send as prompt
                                    }

                                    // Add user message to chat view
                                    chat_view.add_message(ChatMessageDisplay {
                                        role: MessageRole::User,
                                        content_blocks: vec![ContentBlockDisplay::Text {
                                            content: value.clone(),
                                        }],
                                        timestamp: now_millis(),
                                    });

                                    // Start agent streaming
                                    chat_view.start_streaming();
                                    is_agent_busy = true;

                                    // Send prompt to agent worker
                                    let _ = prompt_tx.send(value).await;

                                    // Clear input
                                    input.clear();
                                }
                            }
                        }
                        crossterm::event::KeyCode::Char('c')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            if is_agent_busy {
                                // Abort current operation
                                let sh = agent_session.clone_handle();
                                tokio::spawn(async move {
                                    sh.abort().await;
                                });
                                is_agent_busy = false;
                                chat_view.finish_streaming_error("Interrupted");
                            } else {
                                running = false;
                            }
                        }
                        crossterm::event::KeyCode::PageUp => {
                            chat_view.scroll_up(10);
                        }
                        crossterm::event::KeyCode::PageDown => {
                            chat_view.scroll_down(10);
                        }
                        _ => {
                            // Forward keyboard events to input component
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
                crossterm::event::Event::Resize(_, _) => {
                    // Handled on next render cycle via crossterm::terminal::size()
                }
                _ => {}
            }
        }

        // ── Drain agent events from the channel ──────────────────────
        while let Ok(ui_event) = ui_rx.try_recv() {
            match ui_event {
                UiEvent::Start => {}
                UiEvent::Thinking => {
                    chat_view.stream_thinking_start();
                }
                UiEvent::TextDelta(text) => {
                    chat_view.stream_text_delta(&text);
                }
                UiEvent::ToolCall {
                    id,
                    name,
                    arguments,
                } => {
                    chat_view.stream_thinking_end();
                    chat_view.stream_tool_call(id, name, arguments);
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
                    is_agent_busy = false;
                }
                UiEvent::Error(msg) => {
                    chat_view.finish_streaming_error(&msg);
                    is_agent_busy = false;
                }
                UiEvent::CompactionStart { reason } => {
                    let reason_str = match reason {
                        CompactionReason::Manual => "manual",
                        CompactionReason::Threshold => "auto-threshold",
                        CompactionReason::Overflow => "overflow-recovery",
                    };
                    chat_view.add_message(ChatMessageDisplay {
                        role: MessageRole::Assistant,
                        content_blocks: vec![ContentBlockDisplay::Text {
                            content: format!("\u{1f4e6} Compacting context ({})...", reason_str),
                        }],
                        timestamp: now_millis(),
                    });
                }
                UiEvent::CompactionEnd {
                    _reason: _,
                    error_message,
                } => {
                    let msg = if let Some(err) = error_message {
                        format!("\u{26a0}\u{fe0f} Compaction failed: {}", err)
                    } else {
                        "\u{2705} Compaction complete.".to_string()
                    };
                    chat_view.add_message(ChatMessageDisplay {
                        role: MessageRole::Assistant,
                        content_blocks: vec![ContentBlockDisplay::Text { content: msg }],
                        timestamp: now_millis(),
                    });
                }
                UiEvent::RetryStart {
                    attempt,
                    max_attempts,
                    error_message,
                } => {
                    chat_view.add_message(ChatMessageDisplay {
                        role: MessageRole::Assistant,
                        content_blocks: vec![ContentBlockDisplay::Text {
                            content: format!(
                                "\u{1f504} Retrying ({}/{}): {}",
                                attempt, max_attempts, error_message
                            ),
                        }],
                        timestamp: now_millis(),
                    });
                }
                UiEvent::ModelChanged { model_id } => {
                    chat_view.add_message(ChatMessageDisplay {
                        role: MessageRole::Assistant,
                        content_blocks: vec![ContentBlockDisplay::Text {
                            content: format!("\u{1f916} Model: {}", model_id),
                        }],
                        timestamp: now_millis(),
                    });
                }
                UiEvent::ThinkingLevelChanged { level } => {
                    chat_view.add_message(ChatMessageDisplay {
                        role: MessageRole::Assistant,
                        content_blocks: vec![ContentBlockDisplay::Text {
                            content: format!("\u{1f4ad} Thinking level: {}", level),
                        }],
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
                SessionEvent::CompactionEnd {
                    reason,
                    result: _,
                    aborted: _,
                    will_retry: _,
                    error_message,
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
                SessionEvent::SessionInfoChanged { name: _ } => {
                    // Could update title bar in future
                }
                SessionEvent::Agent(event) => {
                    // Agent events are handled by the agent worker thread's
                    // event forwarder; we only get them here if we subscribed
                    // to the session channel directly (which we do via subscribe).
                    // Forward relevant ones to UI.
                    match &event {
                        AgentEvent::Fallback {
                            from_model: _,
                            to_model,
                        } => {
                            let _ = ui_tx
                                .send(UiEvent::ModelChanged {
                                    model_id: to_model.clone(),
                                })
                                .await;
                        }
                        AgentEvent::Retry {
                            attempt,
                            max_retries,
                            retry_after_secs: _,
                            reason,
                        } => {
                            let _ = ui_tx
                                .send(UiEvent::RetryStart {
                                    attempt: *attempt as u32,
                                    max_attempts: *max_retries as u32,
                                    error_message: reason.clone(),
                                })
                                .await;
                        }
                        AgentEvent::Compaction { event: _ } => {
                            // Compaction events are handled by SessionEvent above
                        }
                        _ => {}
                    }
                }
            }
        }

        // Auto-scroll to bottom
        chat_view.scroll_to_bottom();
    }

    // ── Cleanup ──────────────────────────────────────────────────────
    drop(prompt_tx);
    let _ = agent_handle.join();
    crossterm::execute!(io::stdout(), crossterm::cursor::Show)?;
    crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture)?;
    crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    io::stdout().flush()?;

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Slash command handling
// ═══════════════════════════════════════════════════════════════════════════

/// Handle a slash command. Returns `true` if the command was handled
/// (and should NOT be sent to the agent).
fn handle_slash_command(
    input: &str,
    session: &AgentSession,
    chat_view: &mut ChatView,
    theme: &Theme,
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
            let help_text = format_help();
            chat_view.add_message(ChatMessageDisplay {
                role: MessageRole::Assistant,
                content_blocks: vec![ContentBlockDisplay::Text {
                    content: help_text,
                }],
                timestamp: now_millis(),
            });
            true
        }
        "/quit" | "/exit" | "/q" => {
            *running = false;
            true
        }
        "/clear" => {
            *chat_view = ChatView::new(theme.clone());
            session.reset();
            true
        }
        "/model" => {
            if let Some(model_id) = arg {
                match session.set_model(model_id) {
                    Ok(()) => {
                        chat_view.add_message(ChatMessageDisplay {
                            role: MessageRole::Assistant,
                            content_blocks: vec![ContentBlockDisplay::Text {
                                content: format!("Switched to model: {}", model_id),
                            }],
                            timestamp: now_millis(),
                        });
                    }
                    Err(e) => {
                        chat_view.add_message(ChatMessageDisplay {
                            role: MessageRole::Assistant,
                            content_blocks: vec![ContentBlockDisplay::Text {
                                content: format!("Error switching model: {}", e),
                            }],
                            timestamp: now_millis(),
                        });
                    }
                }
            } else {
                chat_view.add_message(ChatMessageDisplay {
                    role: MessageRole::Assistant,
                    content_blocks: vec![ContentBlockDisplay::Text {
                        content: format!(
                            "Current model: {}\nUse /model <provider/model> to switch.",
                            session.model_id(),
                        ),
                    }],
                    timestamp: now_millis(),
                });
            }
            true
        }
        "/compact" => {
            let instructions = arg.map(|s| s.to_string());
            // Compact runs asynchronously, but we fire and forget here
            // The session events will show progress
            let sh = session.clone_handle();
            tokio::spawn(async move {
                match sh.compact(instructions).await {
                    Ok(result) => {
                        tracing::info!("Compaction complete: {} tokens before", result.tokens_before);
                    }
                    Err(e) => {
                        tracing::warn!("Compaction failed: {}", e);
                    }
                }
            });
            true
        }
        "/session" => {
            let stats = session.session_stats();
            let info = format!(
                "Session Info:\n\
                 ID: {}\n\
                 Messages: {} total ({} user, {} assistant)\n\
                 Tool calls: {}, Results: {}\n\
                 Model: {}\n\
                 Thinking: {:?}\n\
                 Auto-compaction: {}\n\
                 Auto-retry: {}",
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
            chat_view.add_message(ChatMessageDisplay {
                role: MessageRole::Assistant,
                content_blocks: vec![ContentBlockDisplay::Text { content: info }],
                timestamp: now_millis(),
            });
            true
        }
        "/settings" => {
            let settings_info = format!(
                "Model: {}\n\
                 Thinking Level: {:?}\n\
                 Auto-compaction: {}\n\
                 Auto-retry: {}",
                session.model_id(),
                session.thinking_level(),
                session.auto_compaction_enabled(),
                session.auto_retry_enabled(),
            );
            chat_view.add_message(ChatMessageDisplay {
                role: MessageRole::Assistant,
                content_blocks: vec![ContentBlockDisplay::Text {
                    content: settings_info,
                }],
                timestamp: now_millis(),
            });
            true
        }
        "/name" => {
            if let Some(name) = arg {
                session.set_session_name(name.to_string());
                chat_view.add_message(ChatMessageDisplay {
                    role: MessageRole::Assistant,
                    content_blocks: vec![ContentBlockDisplay::Text {
                        content: format!("Session named: {}", name),
                    }],
                    timestamp: now_millis(),
                });
            } else {
                chat_view.add_message(ChatMessageDisplay {
                    role: MessageRole::Assistant,
                    content_blocks: vec![ContentBlockDisplay::Text {
                        content: "Usage: /name <name>".to_string(),
                    }],
                    timestamp: now_millis(),
                });
            }
            true
        }
        _ => false, // Not a recognized command, send to agent
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Surface rendering
// ═══════════════════════════════════════════════════════════════════════════

/// Render a surface to the terminal using efficient SGR sequences.
fn render_surface_to_terminal(surface: &Surface, width: u16, height: u16) {
    // Begin synchronized update
    print!("\x1b[?2026h");
    print!("\x1b[H"); // Move to top-left

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
                // Check if style changed
                let fg_changed = cell.fg != last_fg;
                let bg_changed = cell.bg != last_bg;
                let attrs_changed = cell.attrs.bold != last_bold
                    || cell.attrs.italic != last_italic
                    || cell.attrs.underline != last_underline
                    || cell.attrs.strikethrough != last_strike;

                if fg_changed || bg_changed || attrs_changed {
                    print!("\x1b[0m");

                    // Foreground
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

                    // Background
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

// ═══════════════════════════════════════════════════════════════════════════
// Event conversion helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a crossterm key event to an oxi-tui Event.
/// Returns None for special keys handled separately (Enter, Ctrl+C).
fn convert_key_event(key: crossterm::event::KeyEvent) -> Option<oxi_tui::Event> {
    use oxi_tui::event::KeyCode as KC;

    let code = match key.code {
        crossterm::event::KeyCode::Enter => return None,
        crossterm::event::KeyCode::Char('c')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            return None
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
        shift: key
            .modifiers
            .contains(crossterm::event::KeyModifiers::SHIFT),
        ctrl: key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL),
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
"#
    .to_string()
}

/// Get current timestamp in milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
