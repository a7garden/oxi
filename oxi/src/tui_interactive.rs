//! TUI-based interactive mode using oxi-tui components.
//!
//! Wires together ChatView, Input, Markdown, and Image components
//! into a cohesive terminal chat experience.

use anyhow::Result;
use oxi_agent::{Agent, AgentEvent};
use oxi_tui::component::Component;
use oxi_tui::{
    ChatMessageDisplay, ChatView, ContentBlockDisplay, Input, MessageRole, Surface, Theme,
};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Messages sent from the agent task to the TUI event loop.
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
}

/// Run the TUI-based interactive mode.
pub async fn run_tui_interactive(app: crate::App) -> Result<()> {
    let theme = Theme::dark();
    let agent: Arc<Agent> = app.agent();

    // Channel for agent → UI communication
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(256);

    // Channel for user input → agent execution
    let (prompt_tx, mut prompt_rx) = mpsc::channel::<String>(16);

    // Spawn agent worker on a dedicated thread with its own LocalSet runtime.
    // The agent uses non-Send futures internally, so it needs a single-threaded
    // runtime with LocalSet.
    let agent_for_thread: Arc<Agent> = Arc::clone(&agent);
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

                        // Run agent with channel on the local set
                        let a: Arc<Agent> = Arc::clone(&agent_for_thread);
                        let _ = a.run_with_channel(prompt, event_tx).await;
                        let _ = event_forwarder.await;
                    }
                })
                .await;
        });
    });

    // Build TUI components
    let mut chat_view = ChatView::new(theme.clone());
    let mut input = Input::with_placeholder("Type a message... (Ctrl+C to quit)");
    input.on_focus();
    let mut is_agent_busy = false;

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
                            running = false;
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

        // Drain agent events from the channel
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
            }
        }

        // Auto-scroll to bottom
        chat_view.scroll_to_bottom();
    }

    // Cleanup
    drop(prompt_tx);
    let _ = agent_handle.join();
    crossterm::execute!(io::stdout(), crossterm::cursor::Show)?;
    crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture)?;
    crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    io::stdout().flush()?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Surface rendering
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Event conversion helpers
// ---------------------------------------------------------------------------

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

/// Get current timestamp in milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
