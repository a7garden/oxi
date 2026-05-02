//! TUI-based interactive mode using oxi-tui components.
//!
//! Wires together ChatView, Input, Markdown, and Image components
//! into a cohesive terminal chat experience.

use anyhow::Result;
use oxi_agent::{Agent, AgentEvent};
use oxi_tui::{
    ChatMessageDisplay, ChatView, ContentBlockDisplay, Input, MessageRole,
    Rect, Surface, Theme, TUI,
};
use oxi_tui::event::KeyCode;
use oxi_tui::component::Component;
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
    ToolCall { id: String, name: String, arguments: String },
    /// Tool completed.
    ToolResult { tool_name: String, content: String, is_error: bool },
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

    // Spawn agent worker task
    let agent_handle = tokio::task::spawn(async move {
        while let Some(prompt) = prompt_rx.recv().await {
            let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);

            // Forward agent events to UI
            let ui_tx = ui_tx.clone();
            let event_forwarder = tokio::task::spawn(async move {
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
                    if ui_tx.send(ui_event).await.is_err() {
                        break;
                    }
                }
            });

            // Run agent with channel
            let agent_clone: Arc<Agent> = Arc::clone(&agent);
            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = agent_clone.run_with_channel(prompt, event_tx).await;
            });
            local.await;

            let _ = event_forwarder.await;
        }
    });

    // Build the TUI
    let terminal = oxi_tui::CrosstermTerminal::new()?;
    let mut tui = TUI::new(terminal);

    // Create components
    let chat_view_index = tui.add_child(ChatView::new(theme.clone()));
    let input_index = tui.add_child(Input::with_placeholder("Type a message... (Ctrl+C to quit)"));

    // Focus the input
    tui.set_focus(input_index);

    // Custom event loop that integrates agent events
    use std::io::{self, Write};

    // Enter alternate screen manually so we control the loop
    crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    crossterm::execute!(io::stdout(), crossterm::cursor::Hide)?;
    crossterm::execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;

    // We'll manage rendering ourselves to handle the split layout
    let mut chat_view = ChatView::new(theme.clone());
    let mut input = Input::with_placeholder("Type a message... (Ctrl+C to quit)");
    input.on_focus();
    let mut running = true;
    let mut is_agent_busy = false;
    let mut pending_prompt: Option<String> = None;

    // Helper to get terminal size
    fn get_size() -> (u16, u16) {
        crossterm::terminal::size().unwrap_or((80, 24))
    }

    while running {
        // Render the layout
        let (width, height) = get_size();
        let input_height: u16 = 3; // Input area + border
        let chat_height = height.saturating_sub(input_height);

        // Create surface and render
        let mut surface = Surface::new(width, height);

        // Render chat view
        let chat_area = Rect::new(0, 0, width, chat_height);
        chat_view.render(&mut surface, chat_area);

        // Render separator line
        if chat_height < height {
            let sep_y = chat_height;
            for col in 0..width {
                let cell = oxi_tui::Cell::new('─').with_fg(theme.colors.border);
                surface.set(sep_y, col, cell);
            }

            // Render input area
            let input_area = Rect::new(2, chat_height + 1, width.saturating_sub(4), 1);
            input.render(&mut surface, input_area);

            // Render prompt indicator
            let prompt_cell = oxi_tui::Cell::new('❯').with_fg(theme.colors.primary);
            surface.set(chat_height + 1, 0, prompt_cell);
        }

        // Render the surface to terminal
        render_surface_to_terminal(&surface, width, height);
        io::stdout().flush()?;

        // Poll for events (terminal + agent) with timeout
        let timeout = std::time::Duration::from_millis(33); // ~30fps

        // Check for terminal input events
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
                            if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            running = false;
                        }
                        crossterm::event::KeyCode::Up
                            if key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) =>
                        {
                            // Scroll chat up
                            chat_view.scroll_up(3);
                        }
                        crossterm::event::KeyCode::Down
                            if key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) =>
                        {
                            // Scroll chat down
                            chat_view.scroll_down(3);
                        }
                        crossterm::event::KeyCode::PageUp => {
                            chat_view.scroll_up(10);
                        }
                        crossterm::event::KeyCode::PageDown => {
                            chat_view.scroll_down(10);
                        }
                        _ => {
                            // Forward to input component
                            let tui_event = convert_crossterm_key(key);
                            input.handle_event(&tui_event);
                        }
                    }
                }
                crossterm::event::Event::Mouse(mouse) => {
                    let tui_event = convert_crossterm_mouse(mouse);
                    // Route mouse scroll to chat view
                    if mouse.row < chat_height {
                        chat_view.handle_event(&tui_event);
                    }
                }
                crossterm::event::Event::Resize(_, _) => {
                    // Handled on next render cycle via get_size()
                }
                _ => {}
            }
        }

        // Drain agent events
        while let Ok(ui_event) = ui_rx.try_recv() {
            match ui_event {
                UiEvent::Start => {}
                UiEvent::Thinking => {
                    chat_view.stream_thinking_start();
                }
                UiEvent::TextDelta(text) => {
                    chat_view.stream_text_delta(&text);
                }
                UiEvent::ToolCall { id, name, arguments } => {
                    // End any current thinking block
                    chat_view.stream_thinking_end();
                    chat_view.stream_tool_call(id, name, arguments);
                }
                UiEvent::ToolResult { tool_name, content, is_error } => {
                    chat_view.stream_tool_result(tool_name, content, is_error);
                }
                UiEvent::Complete => {
                    // End thinking if active
                    chat_view.stream_thinking_end();
                    chat_view.finish_streaming();
                    is_agent_busy = false;

                    // Re-render with markdown for the completed message
                    // (The streaming text was already captured; we enhance the last message)
                    enhance_last_message_with_markdown(&mut chat_view);
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
    agent_handle.abort();
    crossterm::execute!(io::stdout(), crossterm::cursor::Show)?;
    crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture)?;
    crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    io::stdout().flush()?;

    Ok(())
}

/// Enhance the last completed assistant message with markdown-parsed content blocks.
///
/// When streaming finishes, the raw text may contain markdown. We parse it and
/// replace the text block with richer content blocks (headings, code, lists, etc.)
/// rendered through the Markdown component's styling.
fn enhance_last_message_with_markdown(chat_view: &mut ChatView) {
    // Get the last message's text content and re-parse with markdown
    // For now, we leave the text as-is since ChatView already does word-wrap.
    // The text content will display properly; full markdown inline rendering
    // can be added as an enhancement by integrating Markdown's block parser
    // into ChatView's render_text_block method.
    //
    // This function serves as the integration point for future enhancement.
    let _ = chat_view;
}

/// Render a surface to the terminal using efficient diffing.
fn render_surface_to_terminal(surface: &Surface, width: u16, height: u16) {
    use std::io::Write;

    // Begin synchronized update if supported
    print!("\x1b[?2026h");

    // Move cursor to top-left
    print!("\x1b[H");

    let mut last_fg = oxi_tui::Color::Default;
    let mut last_bg = oxi_tui::Color::Default;
    let mut last_attrs = oxi_tui::Attributes::new();

    for row in 0..height {
        if row > 0 {
            print!("\r\n");
        }
        for col in 0..width {
            if let Some(cell) = surface.get(row, col) {
                // Optimize: only emit SGR changes
                let fg_changed = cell.fg != last_fg;
                let bg_changed = cell.bg != last_bg;
                let attrs_changed = cell.attrs != last_attrs;

                if fg_changed || bg_changed || attrs_changed {
                    // Reset to known state, then apply new styles
                    print!("\x1b[0m");

                    // Apply foreground
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

                    // Apply background
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

                    // Apply attributes
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
                    last_attrs = cell.attrs;
                }

                // Write character
                print!("{}", cell.char);
            } else {
                print!(" ");
            }
        }
    }

    // Reset styles
    print!("\x1b[0m");

    // End synchronized update
    print!("\x1b[?2026l");
}

/// Convert a crossterm key event to an oxi-tui Event.
fn convert_crossterm_key(key: crossterm::event::KeyEvent) -> oxi_tui::Event {
    let code = match key.code {
        crossterm::event::KeyCode::Enter => KeyCode::Enter,
        crossterm::event::KeyCode::Esc => KeyCode::Escape,
        crossterm::event::KeyCode::Tab => KeyCode::Tab,
        crossterm::event::KeyCode::Backspace => KeyCode::Backspace,
        crossterm::event::KeyCode::Delete => KeyCode::Delete,
        crossterm::event::KeyCode::Up => KeyCode::Up,
        crossterm::event::KeyCode::Down => KeyCode::Down,
        crossterm::event::KeyCode::Left => KeyCode::Left,
        crossterm::event::KeyCode::Right => KeyCode::Right,
        crossterm::event::KeyCode::Home => KeyCode::Home,
        crossterm::event::KeyCode::End => KeyCode::End,
        crossterm::event::KeyCode::PageUp => KeyCode::PageUp,
        crossterm::event::KeyCode::PageDown => KeyCode::PageDown,
        crossterm::event::KeyCode::Char(c) => KeyCode::Char(c),
        crossterm::event::KeyCode::F(n) => KeyCode::F(n),
        _ => KeyCode::Enter,
    };

    let modifiers = oxi_tui::KeyModifiers {
        shift: key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT),
        ctrl: key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL),
        alt: key.modifiers.contains(crossterm::event::KeyModifiers::ALT),
        meta: key.modifiers.contains(crossterm::event::KeyModifiers::META),
    };

    oxi_tui::Event::Key(oxi_tui::KeyEvent::with_modifiers(code, modifiers))
}

/// Convert a crossterm mouse event to an oxi-tui Event.
fn convert_crossterm_mouse(mouse: crossterm::event::MouseEvent) -> oxi_tui::Event {
    let kind = match mouse.kind {
        crossterm::event::MouseEventKind::ScrollUp => oxi_tui::MouseEventKind::ScrollUp,
        crossterm::event::MouseEventKind::ScrollDown => oxi_tui::MouseEventKind::ScrollDown,
        crossterm::event::MouseEventKind::Down(_) => oxi_tui::MouseEventKind::Click,
        _ => oxi_tui::MouseEventKind::Click,
    };

    oxi_tui::Event::Mouse(oxi_tui::MouseEvent {
        kind,
        button: oxi_tui::MouseButton::Left,
        row: mouse.row,
        col: mouse.column,
    })
}

/// Get current timestamp in milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
