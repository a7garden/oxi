#![allow(
    clippy::field_reassign_with_default,
    clippy::let_and_return,
    clippy::borrow_interior_mutable_const,
    clippy::derivable_impls
)]
//! TUI main event loop — connects oxicode's `AgentSession` to vtcode-ui's
//! `InlineSession` protocol and a ratatui rendering backend.

use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate, disable_raw_mode, enable_raw_mode},
};
use oxicode_agent::AgentEvent;
use oxicode_vtui::theme::{ThemeStyles, active_styles};
use oxicode_vtui::tui::core::{
    InlineCommand, InlineEvent, InlineHandle, InlineHeaderContext, InlineHeaderStatusBadge,
    InlineHeaderStatusTone, InlineMessageKind, InlineSegment, InlineTextStyle,
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::App;
use crate::app::agent_hub_registry::HubEntry;
use crate::app::agent_session::SessionEvent;
use crate::tui_vt::slash::registry::{SlashCtx, SlashOutcome, SlashRegistry};

// ─────────────────────────────────────────────────────────────────────────
// Terminal lifecycle (RAII)
// ─────────────────────────────────────────────────────────────────────────

/// Terminal wrapper with deterministic enter / exit / Drop semantics.
///
/// Each cleanup step in `exit` is independent — a failure in one stage
/// (e.g. `PopKeyboardEnhancementFlags`) MUST NOT prevent later stages
/// (`disable_raw_mode`) from running, or the user's terminal is left in
/// raw mode (no echo, no line editing).
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    tty_ok: bool,
}

impl Tui {
    /// Enter the alternate screen, enable raw mode, push keyboard flags,
    /// enable bracketed paste, hide the cursor, install the panic hook.
    pub fn enter() -> Result<Self> {
        Self::set_panic_hook();

        let tty_ok = enable_raw_mode().is_ok();
        let mut stdout = io::stdout();

        if tty_ok {
            // Report event types so key-release / repeat events arrive as
            // distinct codes. Full Kitty flag set is gated on
            // OXICODE_KITTY_KEYBOARD=1; default mirrors pre-Kitty behavior.
            let flags = if std::env::var("OXICODE_KITTY_KEYBOARD").as_deref() == Ok("1") {
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            } else {
                KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            };
            let _ = execute!(
                stdout,
                Hide,
                EnableBracketedPaste,
                PushKeyboardEnhancementFlags(flags)
            );
            let _ = stdout.flush();
        }

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        if tty_ok {
            let _ = terminal.clear();
        }

        Ok(Self { terminal, tty_ok })
    }

    /// Restore the terminal to its pre-TUI state. Each step is independent;
    /// errors are swallowed so a partial restoration never strands the user
    /// in raw mode.
    pub fn exit(&mut self) -> Result<()> {
        if self.tty_ok {
            let _ = execute!(
                self.terminal.backend_mut(),
                PopKeyboardEnhancementFlags,
                DisableBracketedPaste
            );
            let _ = self.terminal.show_cursor();
            // disable_raw_mode is the most critical — always attempt it.
            disable_raw_mode()?;
            self.tty_ok = false;
        }
        Ok(())
    }

    /// Install a panic hook that restores the terminal before printing the
    /// panic message. Without this, a panic inside the TUI strands the
    /// user's shell in raw mode / alternate screen.
    fn set_panic_hook() {
        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = execute!(io::stdout(), Show);
            let _ = disable_raw_mode();
            original_hook(panic_info);
        }));
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.exit();
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Render state — shared between the input thread and the main loop.
// ─────────────────────────────────────────────────────────────────────────

/// Mutable state the input thread edits (text buffer, scroll, footer) and
/// the main loop reads for rendering.
#[derive(Default)]
pub struct RenderState {
    /// Editable text in the composer.
    pub input_buffer: String,
    /// Cursor position inside `input_buffer` (byte index).
    pub input_cursor: usize,
    /// Transcript lines, in display order.
    pub transcript: Vec<TranscriptLine>,
    /// Index of the line currently pinned at the top of the viewport.
    /// `usize::MAX` means "follow the tail" (auto-scroll).
    pub scroll_offset: usize,
    /// Header context mirrored from `InlineHeaderContext`.
    pub header_context: InlineHeaderContext,
    /// Composer enabled state — mirrored from `SetInputEnabled`.
    pub input_enabled: bool,
    /// Footer status (left + right) — mirrored from `SetInputStatus`.
    pub footer_left: Option<String>,
    pub footer_right: Option<String>,
    /// Composer prompt prefix — mirrored from `SetPrompt`.
    pub prompt_prefix: String,
    /// Composer placeholder — mirrored from `SetPlaceholder`.
    pub placeholder: Option<String>,
    /// Shutdown signal received from the harness.
    pub shutdown_requested: bool,
    /// Accumulated text for markdown rendering at message end.
    pub message_buffer: String,
    /// Agent Hub overlay open.
    pub agent_hub_open: bool,
    /// Hub entries snapshotted when the overlay was opened (`/agents`).
    pub hub_entries: Vec<(String, HubEntry)>,
    /// First Ctrl+C armed a quit; a second press exits (two-press quit).
    pub pending_quit: bool,
    /// Slash-command autocomplete popup state.
    pub slash_popup: SlashPopup,
    /// Current reasoning/tool stage (e.g. "tool: read"), shown above the composer.
    pub reasoning_stage: Option<String>,
}

/// One rendered transcript line.
#[derive(Debug, Clone)]
pub struct TranscriptLine {
    pub kind: InlineMessageKind,
    pub segments: Vec<InlineSegment>,
}

/// One filtered entry in the `/`-command autocomplete popup.
#[derive(Clone)]
pub struct SlashPopupItem {
    /// Display label, e.g. `"/quit, /exit, /q"`.
    pub label: String,
    /// Short human description.
    pub description: String,
    /// Canonical command name (no leading `/`), used for completion.
    pub name: String,
}

/// Slash-command autocomplete popup state, managed by the input thread and
/// read by the render loop. The popup is open when the input buffer starts
/// with `/` and contains no space (i.e. the user is still typing the command
/// token, not its arguments).
#[derive(Default, Clone)]
pub struct SlashPopup {
    pub open: bool,
    pub items: Vec<SlashPopupItem>,
    pub selected: usize,
}

impl RenderState {
    fn new_with_header(header: InlineHeaderContext) -> Self {
        let mut s = Self::default();
        s.header_context = header;
        s.prompt_prefix = "> ".to_string();
        s.input_enabled = true;
        s
    }

    /// Append a brand-new line to the transcript.
    fn append_line(&mut self, kind: InlineMessageKind, segments: Vec<InlineSegment>) {
        self.transcript.push(TranscriptLine { kind, segments });
    }

    /// Append a segment to the most recent transcript line, or create a new
    /// line if the transcript is empty. Used for `Inline { kind, segment }`
    /// where the segment is a streaming delta.
    fn inline_segment(&mut self, kind: InlineMessageKind, segment: InlineSegment) {
        if let Some(last) = self.transcript.last_mut()
            && last.kind == kind
        {
            last.segments.push(segment);
            return;
        }
        self.transcript.push(TranscriptLine {
            kind,
            segments: vec![segment],
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Main entry: `pub async fn run_tui(app: App) -> Result<()>`
// ─────────────────────────────────────────────────────────────────────────

/// Run the new oxicode-vtui powered TUI. Returns once the user exits or the
/// session is shut down.
pub async fn run_tui(app: App) -> Result<()> {
    // Resolve shared session-level context up-front so it can outlive the
    // TUI RAII guard via the worker thread.
    let cwd: PathBuf = std::env::current_dir().unwrap_or_default();
    let git_branch = crate::util::git_utils::get_current_branch(&cwd);
    super::host::activate_theme(app.settings());

    // Wire the inline-protocol channels. `cmd_tx` becomes the
    // `InlineHandle`; `evt_tx` is the input-thread → main-loop channel.
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<InlineCommand>();
    let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<InlineEvent>();
    let handle = InlineHandle::new_for_tests(cmd_tx);

    // Build the AgentSession from the App. The helper wraps
    // `create_agent_session_from_services` so we can construct the session
    // without duplicating the runtime plumbing here.
    let session = build_agent_session(&app).await?;
    // No install_runtime_hooks call: session queues and stop flag are
    // wired into the agent hook chain at agent-build time via
    // App::from_oxicode → with_session_hooks.
    let session_handle = session.clone_handle();

    // Forward session events to a tokio mpsc so the main loop can
    // `tokio::select!` on them. We do this in two stages:
    //  1. Subscribe to AgentSession — CompactionStart/End, Advisor,
    //     QueueUpdate, etc.
    //  2. A forwarder thread that drives `agent.run_with_channel` and
    //     calls `forward_event_to_extensions` so per-agent events also
    //     flow through the same listener.
    let (session_tx, mut session_rx) = tokio::sync::mpsc::unbounded_channel::<SessionEvent>();
    let _sub_guard = session.subscribe(Box::new(move |event| {
        let _ = session_tx.send(event.clone());
    }));

    // Header context — built once at startup with workspace + branch.
    let header = build_header_context(&app, &cwd, git_branch.as_deref());
    handle.set_header_context(header.clone());

    // Enter the terminal (RAII). Every setup step is fallible, but a
    // successful `Tui::enter` is required to draw anything.
    let mut tui = Tui::enter()?;

    // Initial composer + placeholder — the harness receives these as
    // `SetPrompt` / `SetPlaceholder` commands once it spins up its own
    // consumer; we set them eagerly so the very first frame is correct.
    handle.set_prompt("> ".to_string(), InlineTextStyle::default());
    handle.set_placeholder(Some("Describe what you want to build\u{2026}".to_string()));

    // Render state — shared between the input thread (which edits the
    // buffer) and the main loop (which reads it for drawing).
    let state = Arc::new(parking_lot::Mutex::new(RenderState::new_with_header(
        header,
    )));
    spawn_input_thread(state.clone(), evt_tx.clone());

    // Worker thread that owns the agent loop. Receives prompts over a
    // tokio mpsc and dispatches them through `run_with_channel`. The
    // returned `AgentEvent`s flow through a `std::sync::mpsc`; a paired
    // forwarder thread funnels them into the session's listener bus so
    // our subscriber above picks them up.
    let prompt_tx = spawn_agent_worker(session_handle.clone());

    let result = run_event_loop(
        &mut tui.terminal,
        &mut cmd_rx,
        &mut evt_rx,
        &mut session_rx,
        &handle,
        &state,
        &session_handle,
        prompt_tx.clone(),
    )
    .await;

    // Drain the harness before tearing down the terminal. Even if the
    // loop exited early we want to release the worker.
    drop(prompt_tx);
    handle.shutdown();
    // Dropping `tui` restores the terminal. Drop is at function return.
    drop(tui);

    result
}

// ─────────────────────────────────────────────────────────────────────────
// Event loop
// ─────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    cmd_rx: &mut tokio::sync::mpsc::UnboundedReceiver<InlineCommand>,
    evt_rx: &mut tokio::sync::mpsc::UnboundedReceiver<InlineEvent>,
    session_rx: &mut tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    handle: &InlineHandle,
    state: &Arc<parking_lot::Mutex<RenderState>>,
    session: &crate::app::agent_session::AgentSessionHandle,
    prompt_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<()> {
    // Drain any pending InlineCommands so the harness's initial set_header_context
    // (and similar) is observed before the first frame.
    while let Ok(cmd) = cmd_rx.try_recv() {
        apply_command(&mut state.lock(), cmd);
    }

    // Draw the initial frame *before* blocking on the first event. The
    // `select!` below parks until an event arrives, and the per-iteration
    // redraw only runs after it resolves — so without this eager draw the
    // screen stays black until the user presses a key.
    {
        let snapshot = state.lock();
        let _ = execute!(terminal.backend_mut(), BeginSynchronizedUpdate);
        if let Err(err) = terminal.draw(|frame| render_frame(frame, &snapshot, handle)) {
            tracing::warn!(?err, "initial tui draw failed");
        }
        let _ = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    }

    // Render tick. The input thread edits shared state (typing, cursor
    // movement, backspace, …) *without* sending an event, so without a
    // periodic wake the composer would never repaint what the user types.
    // The ratatui diff backend coalesces unchanged frames, so a steady tick
    // is cheap and also drives future spinner animation.
    let mut render_tick = tokio::time::interval(std::time::Duration::from_millis(50));
    render_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // biased: agent events take priority so streaming output is
            // never starved by Ctrl+C noise or sticky key repeats.
            biased;

            // 1. Agent → TUI commands (transcript updates).
            Some(cmd) = cmd_rx.recv() => {
                let shutdown = {
                    let mut s = state.lock();
                    apply_command(&mut s, cmd)
                };
                if shutdown {
                    break;
                }
            }

            // 2. Agent → TUI events (token deltas, tool calls, …).
            Some(event) = session_rx.recv() => {
                handle_session_event(&mut state.lock(), handle, &event);
            }

            // 3. Keyboard / paste / TUI events from the input thread.
            Some(evt) = evt_rx.recv() => {
                let outcome = handle_inline_event(
                    &mut state.lock(),
                    handle,
                    session,
                    &prompt_tx,
                    evt,
                );
                if outcome == LoopOutcome::Exit {
                    break;
                }
            }

            // 4. External SIGINT — route through the same idle-vs-streaming
            //    policy as the key path (some terminals deliver Ctrl+C both
            //    as a key event AND raise SIGINT; `kill -INT` also lands here).
            _ = tokio::signal::ctrl_c() => {
                let outcome = {
                    let mut s = state.lock();
                    handle_interrupt(&mut s, session, handle)
                };
                if outcome == LoopOutcome::Exit {
                    break;
                }
            }

            // 5. Periodic repaint — echoes typed input and drives animation
            //    even when no other event is ready.
            _ = render_tick.tick() => {}
        }

        // Redraw every iteration. The harness's redraw is idempotent —
        // the ratatui backend coalesces unchanged frames.
        let snapshot = state.lock();
        let _ = execute!(terminal.backend_mut(), BeginSynchronizedUpdate);
        let draw_err = terminal
            .draw(|frame| render_frame(frame, &snapshot, handle))
            .err();
        let _ = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
        if let Some(err) = draw_err {
            tracing::warn!(?err, "tui draw failed");
            break;
        }
    }

    Ok(())
}

#[derive(PartialEq, Eq)]
enum LoopOutcome {
    Continue,
    Exit,
}

// ─────────────────────────────────────────────────────────────────────────
// Command / event handlers
// ─────────────────────────────────────────────────────────────────────────

/// Apply a single `InlineCommand` to the render state. Returns `true`
/// when the harness has requested a shutdown.
fn apply_command(state: &mut RenderState, cmd: InlineCommand) -> bool {
    match cmd {
        InlineCommand::AppendLine { kind, segments } => {
            state.append_line(kind, segments);
        }
        InlineCommand::Inline { kind, segment } => {
            state.inline_segment(kind, segment);
        }
        InlineCommand::ReplaceLast {
            count, kind, lines, ..
        } => {
            // Drop the last `count` lines and replace with the new ones.
            let drop = count.min(state.transcript.len());
            for _ in 0..drop {
                state.transcript.pop();
            }
            for line in lines {
                state.append_line(kind, line);
            }
        }
        InlineCommand::AppendPastedMessage { kind, text, .. } => {
            state.append_line(kind, vec![plain_segment(text)]);
        }
        InlineCommand::SetPrompt { prefix, .. } => {
            state.prompt_prefix = prefix;
        }
        InlineCommand::SetPlaceholder { hint, .. } => {
            state.placeholder = hint;
        }
        InlineCommand::SetHeaderContext { context } => {
            state.header_context = *context;
        }
        InlineCommand::SetInputStatus { left, right } => {
            state.footer_left = left;
            state.footer_right = right;
        }
        InlineCommand::SetInputEnabled(enabled) => {
            state.input_enabled = enabled;
        }
        InlineCommand::SetCursorVisible(_) | InlineCommand::ForceRedraw => {
            // Redraw on the next loop iteration; nothing to persist.
        }
        InlineCommand::SetReasoningStage(stage) => {
            state.reasoning_stage = stage;
        }
        InlineCommand::Shutdown => {
            state.shutdown_requested = true;
            return true;
        }
        _ => {
            // Surface unknown commands as info so they are visible
            // during development.
            tracing::trace!("unhandled InlineCommand (not rendered)");
        }
    }
    false
}

/// Map a `SessionEvent` to the matching `InlineHandle` calls. This is the
/// single place where the agent's event vocabulary meets the harness's
/// transcript vocabulary.
fn handle_session_event(state: &mut RenderState, handle: &InlineHandle, event: &SessionEvent) {
    match event {
        SessionEvent::Agent(boxed) => {
            map_agent_event(handle, *boxed.clone(), state);
        }
        SessionEvent::CompactionStart { .. } => {
            handle.set_reasoning_stage(Some("Compacting\u{2026}".to_string()));
        }
        SessionEvent::CompactionEnd { error_message, .. } => {
            handle.set_reasoning_stage(None);
            if let Some(msg) = error_message {
                handle.append_line(
                    InlineMessageKind::Error,
                    vec![plain_segment(format!("Compaction failed: {msg}"))],
                );
            }
        }
        SessionEvent::ThinkingLevelChanged { .. } => {
            // No rendering — the footer reflects this implicitly via the
            // header context.
        }
        SessionEvent::QueueUpdate { .. } => {
            // Surface the queue length as a footer status update.
            // The exact count is computed lazily by the agent session;
            // we approximate it via the snapshot we hold.
            let pending = state.transcript.len();
            handle.set_input_status(
                None,
                Some(if pending == 0 {
                    "ready".to_string()
                } else {
                    "queued".to_string()
                }),
            );
        }
        SessionEvent::Advisor { body, .. } => {
            handle.append_line(InlineMessageKind::Info, vec![plain_segment(body.clone())]);
        }
        SessionEvent::SessionInfoChanged => {
            // The session name is reflected via header context on next
            // `set_header_context`. Nothing to do here.
        }
    }
}

/// Project the agent-level event variants onto the harness transcript.
fn map_agent_event(handle: &InlineHandle, event: AgentEvent, state: &mut RenderState) {
    match event {
        AgentEvent::TextChunk { text } => {
            state.message_buffer.push_str(&text);
            handle.inline(InlineMessageKind::Agent, plain_segment(text));
        }
        AgentEvent::MessageStart { .. } => {
            state.message_buffer.clear();
        }
        AgentEvent::MessageUpdate { delta, .. } => match &delta {
            oxicode_sdk::StreamDelta::Text(text) => {
                state.message_buffer.push_str(text);
                handle.inline(InlineMessageKind::Agent, plain_segment(text.clone()));
            }
            oxicode_sdk::StreamDelta::Thinking(text) => {
                // Show thinking blocks as dimmed Info lines with a ✻ marker,
                // visually distinct from the actual response text.
                let mut style = InlineTextStyle::default();
                style.effects |= anstyle::Effects::DIMMED;
                let seg = InlineSegment {
                    text: format!("\u{2733} {text}"),
                    style: Arc::new(style),
                };
                handle.inline(InlineMessageKind::Info, seg);
            }
            oxicode_sdk::StreamDelta::Sync => {
                // Re-render the complete message as markdown
                if !state.message_buffer.is_empty() {
                    let lines =
                        oxicode_vtui::tui::ui::markdown::render_markdown(&state.message_buffer);
                    let count = lines.len();
                    if count > 0 {
                        handle.replace_last(count, InlineMessageKind::Agent, lines);
                    }
                    state.message_buffer.clear();
                }
            }
        },
        AgentEvent::MessageEnd { .. } => {
            // Final rendering (same as delta:None for completeness)
            if !state.message_buffer.is_empty() {
                let lines = oxicode_vtui::tui::ui::markdown::render_markdown(&state.message_buffer);
                let count = lines.len();
                if count > 0 {
                    handle.replace_last(count, InlineMessageKind::Agent, lines);
                }
                state.message_buffer.clear();
            }
        }
        AgentEvent::ToolStart { tool_name, .. } => {
            handle.append_line(
                InlineMessageKind::Tool,
                vec![plain_segment(format!("\u{2192} {tool_name}"))],
            );
            handle.set_reasoning_stage(Some(format!("tool: {tool_name}")));
        }
        AgentEvent::ToolComplete { result } => {
            let preview = preview_tool_result(&result.content);
            handle.append_line(InlineMessageKind::Tool, vec![plain_segment(preview)]);
            handle.set_reasoning_stage(None);
            handle.set_input_enabled(true);
        }
        AgentEvent::ToolError { error, .. } => {
            handle.append_line(
                InlineMessageKind::Error,
                vec![plain_segment(format!("tool error: {error}"))],
            );
            handle.set_reasoning_stage(None);
            handle.set_input_enabled(true);
        }
        AgentEvent::Error { message, .. } => {
            handle.append_line(InlineMessageKind::Error, vec![plain_segment(message)]);
            handle.set_input_enabled(true);
            handle.set_input_status(None, None);
        }
        AgentEvent::Compaction { .. } => {
            // Detailed lifecycle is handled by the AgentSession layer
            // (CompactionStart/End SessionEvents).
        }
        AgentEvent::Cancelled => {
            handle.set_input_enabled(true);
            handle.set_input_status(None, Some("cancelled".to_string()));
        }
        AgentEvent::AutoRetryStart {
            attempt,
            max_attempts,
            ..
        } => {
            handle.set_input_status(None, Some(format!("retry {attempt}/{max_attempts}")));
        }
        _ => {
            // Other variants (TurnStart/End, AgentStart/End, Usage, …) are
            // logged but not rendered — they're either metadata or covered
            // by the dedicated SessionEvent variants above.
            tracing::debug!(?event, "ignored AgentEvent variant");
        }
    }
}

/// Map an input-thread `InlineEvent` to agent actions / state edits.
fn handle_inline_event(
    state: &mut RenderState,
    handle: &InlineHandle,
    session: &crate::app::agent_session::AgentSessionHandle,
    prompt_tx: &tokio::sync::mpsc::UnboundedSender<String>,
    evt: InlineEvent,
) -> LoopOutcome {
    match evt {
        InlineEvent::Submit(text) => {
            // Drain the composer — the input thread already cleared its
            // local copy once Submit fired, but we keep the canonical
            // buffer here in sync.
            let prompt = text.to_string();
            state.input_buffer.clear();
            state.input_cursor = 0;
            if prompt.is_empty() {
                return LoopOutcome::Continue;
            }
            state.pending_quit = false;
            // Slash commands: dispatch locally instead of forwarding to
            // the agent. The echoed line is appended before dispatch so
            // every command output appears after the prompt.
            if prompt.trim_start().starts_with('/') {
                state.append_line(InlineMessageKind::User, vec![plain_segment(prompt.clone())]);
                let mut ctx = SlashCtx {
                    session,
                    handle,
                    state,
                };
                return match SlashRegistry::builtins().dispatch(&prompt, &mut ctx) {
                    SlashOutcome::Quit => LoopOutcome::Exit,
                    SlashOutcome::Handled => LoopOutcome::Continue,
                    SlashOutcome::NotHandled => {
                        ctx.reply(
                            InlineMessageKind::Error,
                            format!("Unknown command: {}", prompt.trim()),
                        );
                        LoopOutcome::Continue
                    }
                };
            }
            state.append_line(InlineMessageKind::User, vec![plain_segment(prompt.clone())]);
            // Hand the prompt to the worker thread. If the worker has
            // already exited (e.g. shutdown), drop it on the floor.
            let _ = prompt_tx.send(prompt);
        }
        InlineEvent::Cancel | InlineEvent::Exit => {
            // Trigger shutdown via the harness.
            return LoopOutcome::Exit;
        }
        InlineEvent::Interrupt => {
            return handle_interrupt(state, session, handle);
        }
        InlineEvent::ScrollLineUp => {
            state.scroll_offset = state.scroll_offset.saturating_add(1);
        }
        InlineEvent::ScrollLineDown => {
            state.scroll_offset = state.scroll_offset.saturating_sub(1);
        }
        InlineEvent::ScrollPageUp => {
            state.scroll_offset = state.scroll_offset.saturating_add(10);
        }
        InlineEvent::ScrollPageDown => {
            state.scroll_offset = state.scroll_offset.saturating_sub(10);
        }
        InlineEvent::CyclePrimaryAgent => {
            let _ = session.cycle_model();
        }
        InlineEvent::CyclePrimaryAgentPrevious => {
            // No dedicated reverse-cycling API in AgentSession yet;
            // forward-cycle is the closest match.
            let _ = session.cycle_model();
        }
        _ => {
            // Other events (overlay, list-selection, etc.) are no-ops in
            // this harness — they are handled by the harness overlay
            // component, not by the inline protocol.
        }
    }
    LoopOutcome::Continue
}

// ─────────────────────────────────────────────────────────────────────────
// Ctrl+C policy / streaming guard
// ─────────────────────────────────────────────────────────────────────────

/// RAII guard that clears the streaming flag on drop (normal exit, error,
/// or panic cancellation). Wired in [`run_one_prompt`] around each run.
struct StreamingGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for StreamingGuard<'_> {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Central Ctrl+C policy.
///
/// - **Agent streaming** → abort the current run and tell the user to press
///   again to quit. The abort is effective because [`install_runtime_hooks`]
///   wires the session's `should_stop` flag into the agent loop.
/// - **Agent idle** → exit the application.
///
/// Both the input-thread key event (`InlineEvent::Interrupt`) and the OS
/// signal handler (`tokio::signal::ctrl_c()`) route through here so
/// behavior is identical regardless of how the interrupt arrives.
///
/// [`install_runtime_hooks`]: crate::app::agent_session::AgentSession::install_runtime_hooks
fn handle_interrupt(
    state: &mut RenderState,
    session: &crate::app::agent_session::AgentSessionHandle,
    _handle: &InlineHandle,
) -> LoopOutcome {
    // A second consecutive Ctrl+C (no intervening submit) quits.
    if state.pending_quit {
        return LoopOutcome::Exit;
    }
    // First Ctrl+C: abort any running stream and arm the quit flag so the
    // next press exits. A single accidental press never kills the session.
    if session.is_streaming() {
        let s = session.clone();
        tokio::spawn(async move {
            s.abort().await;
        });
        state.footer_left = Some("Stopping\u{2026} press Ctrl+C again to quit".to_string());
    } else {
        state.footer_left = Some("Press Ctrl+C again to quit".to_string());
    }
    state.pending_quit = true;
    LoopOutcome::Continue
}

// ─────────────────────────────────────────────────────────────────────────
// Input thread — polls crossterm, edits the shared buffer, and forwards
// lifecycle events (Submit, Cancel, …) over a tokio channel.
// ─────────────────────────────────────────────────────────────────────────

fn spawn_input_thread(
    state: Arc<parking_lot::Mutex<RenderState>>,
    evt_tx: tokio::sync::mpsc::UnboundedSender<InlineEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // Poll stdin in a tight loop. `event::poll` returns `Ok(false)` on
        // timeout (no key within the window) — that is NOT a reason to exit,
        // only to poll again. The previous `while let Ok(true) = poll(...)`
        // treated the first timeout as loop termination, killing this thread
        // ~50ms after launch, dropping `evt_tx`, and leaving the TUI unable
        // to receive keyboard input — a black screen that only redrew on
        // Ctrl+C. Exit only on a genuine read error (stdin closed).
        loop {
            match event::poll(std::time::Duration::from_millis(50)) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => break,
            }
            let event = match event::read() {
                Ok(ev) => ev,
                Err(_) => continue,
            };

            // Bracketed paste arrives as its own event; flatten into a
            // string of `Submit` text.
            let mut pasted = String::new();
            let mut key_event = None;
            match event {
                Event::Key(k) if k.kind == KeyEventKind::Press => key_event = Some(k),
                Event::Paste(p) => pasted = p,
                _ => {}
            }

            if !pasted.is_empty() {
                let mut s = state.lock();
                let cursor = s.input_cursor;
                s.input_buffer.insert_str(cursor, &pasted);
                s.input_cursor = cursor + pasted.len();
                continue;
            }

            let Some(key) = key_event else { continue };

            // Ctrl+C: even with raw mode enabled some terminals / shells
            // fall back to delivering it as a SIGINT. Handle it as an
            // explicit interrupt so we don't depend on the OS signal.
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let _ = evt_tx.send(InlineEvent::Interrupt);
                continue;
            }

            match key.code {
                KeyCode::Enter => {
                    // If the slash popup is open, complete the selected
                    // command so Enter runs it directly — the user arrowed
                    // to a match and pressed Enter to execute.
                    let submitted = {
                        let mut s = state.lock();
                        let buf = if s.slash_popup.open && !s.slash_popup.items.is_empty() {
                            let item = &s.slash_popup.items[s.slash_popup.selected];
                            format!("/{}", item.name)
                        } else {
                            std::mem::take(&mut s.input_buffer)
                        };
                        s.input_cursor = 0;
                        s.slash_popup = SlashPopup::default();
                        buf
                    };
                    let _ = evt_tx.send(InlineEvent::Submit(submitted.into()));
                }
                KeyCode::Esc => {
                    // If the slash popup is open, Esc just closes it rather
                    // than cancelling the whole session.
                    let mut s = state.lock();
                    if s.slash_popup.open {
                        s.slash_popup = SlashPopup::default();
                    } else {
                        drop(s);
                        let _ = evt_tx.send(InlineEvent::Cancel);
                    }
                }
                KeyCode::Tab => {
                    // Complete the selected slash command into the buffer
                    // (without submitting) so the user can type arguments.
                    let mut s = state.lock();
                    if s.slash_popup.open && !s.slash_popup.items.is_empty() {
                        let name = s.slash_popup.items[s.slash_popup.selected].name.clone();
                        s.input_buffer = format!("/{} ", name);
                        s.input_cursor = s.input_buffer.len();
                        refresh_slash_popup(&mut s);
                    }
                }
                KeyCode::Backspace => {
                    let mut s = state.lock();
                    if s.input_cursor > 0 {
                        let cursor = s.input_cursor;
                        // Walk back one UTF-8 char (not necessarily one
                        // byte, but chars are 1+ bytes).
                        let prev = s
                            .input_buffer
                            .char_indices()
                            .take_while(|(i, _)| *i < cursor)
                            .last()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        s.input_buffer.replace_range(prev..cursor, "");
                        s.input_cursor = prev;
                    }
                    refresh_slash_popup(&mut s);
                }
                KeyCode::Delete => {
                    let mut s = state.lock();
                    if s.input_cursor < s.input_buffer.len() {
                        let cursor = s.input_cursor;
                        let next = s.input_buffer[cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| cursor + i)
                            .unwrap_or(s.input_buffer.len());
                        s.input_buffer.replace_range(cursor..next, "");
                    }
                    refresh_slash_popup(&mut s);
                }
                KeyCode::Left => {
                    let mut s = state.lock();
                    s.input_cursor = s.input_cursor.saturating_sub(1);
                }
                KeyCode::Right => {
                    let mut s = state.lock();
                    let len = s.input_buffer.len();
                    s.input_cursor = (s.input_cursor + 1).min(len);
                }
                KeyCode::Up => {
                    let mut s = state.lock();
                    if s.slash_popup.open && !s.slash_popup.items.is_empty() {
                        let len = s.slash_popup.items.len();
                        s.slash_popup.selected = if s.slash_popup.selected == 0 {
                            len - 1
                        } else {
                            s.slash_popup.selected - 1
                        };
                    } else {
                        drop(s);
                        let _ = evt_tx.send(InlineEvent::ScrollLineUp);
                    }
                }
                KeyCode::Down => {
                    let mut s = state.lock();
                    if s.slash_popup.open && !s.slash_popup.items.is_empty() {
                        let len = s.slash_popup.items.len();
                        s.slash_popup.selected = if s.slash_popup.selected + 1 >= len {
                            0
                        } else {
                            s.slash_popup.selected + 1
                        };
                    } else {
                        drop(s);
                        let _ = evt_tx.send(InlineEvent::ScrollLineDown);
                    }
                }
                KeyCode::PageUp => {
                    let _ = evt_tx.send(InlineEvent::ScrollPageUp);
                }
                KeyCode::PageDown => {
                    let _ = evt_tx.send(InlineEvent::ScrollPageDown);
                }
                KeyCode::Char(ch) => {
                    let mut s = state.lock();
                    if s.agent_hub_open && ch == 'q' {
                        s.agent_hub_open = false;
                    } else {
                        let cursor = s.input_cursor;
                        s.input_buffer.insert(cursor, ch);
                        s.input_cursor = cursor + ch.len_utf8();
                        refresh_slash_popup(&mut s);
                    }
                }
                _ => {}
            }
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────
// Agent worker thread — owns the agent run loop, forwards events to the
// session bus, and accepts new prompts from a tokio channel.
// ─────────────────────────────────────────────────────────────────────────

fn spawn_agent_worker(
    session: crate::app::agent_session::AgentSessionHandle,
) -> tokio::sync::mpsc::UnboundedSender<String> {
    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                tracing::error!(?err, "failed to build agent worker runtime");
                return;
            }
        };

        runtime.block_on(async move {
            let local = tokio::task::LocalSet::new();
            local
                .run_until(async move {
                    while let Some(prompt) = prompt_rx.recv().await {
                        run_one_prompt(&session, prompt).await;
                    }
                })
                .await;
        });
    });

    prompt_tx
}

async fn run_one_prompt(session: &crate::app::agent_session::AgentSessionHandle, prompt: String) {
    let session_for_forward = session.clone();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<AgentEvent>();

    // Forwarder thread — runs `forward_event_to_extensions` on each event
    // so the AgentSession's subscribers (and therefore the main loop)
    // observe it.
    let forwarder = std::thread::spawn(move || {
        while let Ok(event) = event_rx.recv() {
            session_for_forward.forward_event_to_extensions(&event);
        }
    });

    // Reset the stop flag (a previous Ctrl+C may have left it set) and
    // mark streaming so the Ctrl+C policy can distinguish "interrupt"
    // from "quit". The guard clears the flag on any exit path.
    use std::sync::atomic::Ordering;
    session.reset_should_stop();
    let streaming = session.streaming_flag();
    streaming.store(true, Ordering::SeqCst);
    let _stream_guard = StreamingGuard(&streaming);

    let agent = session.agent_ref();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(agent.run_with_channel(prompt, event_tx))
        .await;

    // Wait for the forwarder to drain the channel (sender dropped when
    // `run_with_channel` returns).
    let _ = forwarder.join();
    if let Err(err) = result {
        tracing::warn!(?err, "agent run failed");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Header / AgentSession construction
// ─────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────
// Header / AgentSession construction
// ─────────────────────────────────────────────────────────────────────────

fn build_header_context(
    app: &App,
    cwd: &std::path::Path,
    git_branch: Option<&str>,
) -> InlineHeaderContext {
    let workspace_name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "oxicode".to_string());
    let model_id = app.model_id();
    let provider = model_id
        .split_once('/')
        .map(|(p, _)| p.to_string())
        .unwrap_or_else(|| "Provider".to_string());
    let branch = git_branch.unwrap_or("\u{2014}").to_string();
    let mut ctx = InlineHeaderContext::default();
    ctx.app_name = "oxicode".to_string();
    ctx.provider = provider;
    ctx.model = model_id.clone();
    ctx.git = format!("git: {workspace_name}@{branch}");
    ctx.tools = "Tools: ready".to_string();
    ctx.search_tools = Some(InlineHeaderStatusBadge {
        text: workspace_name,
        tone: InlineHeaderStatusTone::Ready,
    });
    ctx.persistent_memory = Some(InlineHeaderStatusBadge {
        text: branch,
        tone: InlineHeaderStatusTone::Ready,
    });
    ctx.editor_context = Some(model_id);
    ctx
}

/// Construct an `AgentSession` for the TUI using the runtime helpers from
/// `agent_session_runtime`. Mirrors the wiring in the legacy `tui/` harness.
async fn build_agent_session(app: &App) -> Result<crate::app::agent_session::AgentSession> {
    use crate::app::agent_session_runtime::{
        CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
        create_agent_session_from_services, create_agent_session_services,
    };
    use crate::store::session::SessionManager;

    let cwd: PathBuf = std::env::current_dir().unwrap_or_default();
    let hook_runner = Arc::clone(&app.oxicode().ports().hooks);
    let services = create_agent_session_services(
        CreateAgentSessionServicesOptions::new(cwd.clone()),
        Some(hook_runner),
    )?;
    let services = Arc::new(services);

    let model_id = app.model_id();
    let tools = app.agent_tools();

    let session_manager = SessionManager::create(&cwd.to_string_lossy(), None);

    let result = create_agent_session_from_services(CreateAgentSessionFromServicesOptions {
        services,
        session_manager,
        model_id: if model_id.is_empty() {
            None
        } else {
            Some(model_id)
        },
        thinking_level: None,
        scoped_models: Vec::new(),
        tool_registry: Some(tools),
        // TUI runtime: share the App's session state so /steer, /follow_up,
        // and Ctrl+C continue to take effect across the session.
        session_state: Some(app.session_state().clone()),
    })
    .await?;

    if let Some(msg) = result.model_fallback_message {
        tracing::warn!(message = %msg, "agent session model fallback");
    }
    Ok(result.session)
}

// ─────────────────────────────────────────────────────────────────────────
// Rendering
// ─────────────────────────────────────────────────────────────────────────

/// Compose one frame using the agent view layout (grok-build-style):
/// StatusBar (top) → Scrollback (dominant) → Prompt → ShortcutsBar (bottom).
/// Chrome geometry and the status/shortcuts bars are rendered by
/// [`frame_layout::render_chrome`]; the transcript and composer are placed
/// into the returned layout rects.
fn render_frame(frame: &mut Frame<'_>, state: &RenderState, _handle: &InlineHandle) {
    let area = frame.area();
    // Paint the theme background across the whole frame first. Without this
    // every span renders against the host terminal's transparent default bg,
    // so fg-only text can read as invisible when it clashes with that default
    // — the user only saw it after drag-selecting (which inverts colors).
    let bg = active_styles().background;
    frame
        .buffer_mut()
        .set_style(area, Style::default().bg(color_from_anstyle(Some(bg))));
    // Agent view layout (grok-build-style pure geometry). `render_chrome`
    // computes the layout and paints the top StatusBar + bottom ShortcutsBar;
    // the transcript and composer are placed into the returned rects.
    let layout = super::frame_layout::render_chrome(frame, area, state);
    render_transcript(frame, layout.scrollback, state);
    if let Some(stage) = &state.reasoning_stage {
        render_reasoning_indicator(frame, layout.prompt, stage);
    }
    render_composer(frame, layout.prompt, state);
    if state.slash_popup.open {
        render_slash_popup(frame, layout.prompt, state);
    }
    if state.agent_hub_open {
        render_agent_hub(frame, area, state);
    }
}

/// Render the Agent Hub overlay — a centered panel listing every registered
/// agent (kind, name, status). Populated from `RenderState::hub_entries`,
/// snapshotted when `/agents` fired. `q` (input thread Char arm) closes it.
fn render_agent_hub(frame: &mut Frame<'_>, area: Rect, state: &RenderState) {
    let rows = state.hub_entries.len() as u16;
    let height = rows.saturating_add(4).min(area.height.saturating_sub(1));
    let width = area.width.clamp(30, 80);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);

    let title = Line::from(Span::styled(
        " Agent Hub ",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let block = Block::default().borders(Borders::ALL).title(title);

    let items: Vec<ListItem<'_>> = if state.hub_entries.is_empty() {
        vec![ListItem::new(Line::from(Span::raw(
            "No agents registered.",
        )))]
    } else {
        state
            .hub_entries
            .iter()
            .map(|(id, e)| {
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{:?} ", e.kind)),
                    Span::raw(e.display_name.clone()),
                    Span::raw(format!("  — {:?} ({})", e.status, id)),
                ]))
            })
            .collect()
    };
    frame.render_widget(List::new(items).block(block), rect);
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, state: &RenderState) {
    if state.transcript.is_empty() {
        render_welcome(frame, area);
        return;
    }
    let styles = active_styles();

    let lines: Vec<Line<'_>> = state
        .transcript
        .iter()
        .map(|tl| transcript_line(tl, &styles))
        .collect();

    let total = lines.len();
    let start = if state.scroll_offset == usize::MAX {
        // Follow tail: compute how many lines fit so the last ones are visible.
        let avail = area.height as usize;
        total.saturating_sub(avail)
    } else {
        effective_scroll_offset(state.scroll_offset, total, area.height as usize)
    };

    // Render top-down, wrapping each line into multiple visual rows.
    let mut y = area.top();
    let width = area.width.max(1) as usize;
    for line in lines.into_iter().skip(start) {
        if y >= area.bottom() {
            break;
        }
        let text_w = line.width();
        let wrapped_h = if text_w == 0 {
            1
        } else {
            text_w.div_ceil(width).max(1) as u16
        };
        let row = Rect {
            x: area.x,
            y,
            width: area.width,
            height: wrapped_h.min(area.bottom().saturating_sub(y)),
        };
        frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), row);
        y += wrapped_h;
    }
}

/// Build a ratatui `Line` from a transcript line (extracted from the old
/// `transcript_item` which returned a `ListItem`).
fn transcript_line<'a>(line: &'a TranscriptLine, styles: &'a ThemeStyles) -> Line<'a> {
    let (kind_style, marker) = match line.kind {
        InlineMessageKind::Agent => (
            Style::default().fg(color_from_anstyle(styles.response.get_fg_color())),
            "\u{25cf}", // ●
        ),
        InlineMessageKind::User => (
            Style::default().fg(color_from_anstyle(styles.primary.get_fg_color())),
            "\u{276f}", // ❯
        ),
        InlineMessageKind::Tool => (
            Style::default().fg(color_from_anstyle(styles.tool.get_fg_color())),
            "\u{2699}", // ⚙
        ),
        InlineMessageKind::Error => (
            Style::default().fg(color_from_anstyle(styles.error.get_fg_color())),
            "\u{2717}", // ✗
        ),
        InlineMessageKind::Warning => (
            Style::default().fg(color_from_anstyle(styles.status.get_fg_color())),
            "\u{26a0}", // ⚠
        ),
        InlineMessageKind::Info => (
            Style::default().fg(color_from_anstyle(styles.info.get_fg_color())),
            "\u{2139}", // ℹ
        ),
        InlineMessageKind::Policy => (
            Style::default().fg(color_from_anstyle(styles.mcp.get_fg_color())),
            "\u{25c6}", // ◆
        ),
        InlineMessageKind::Pty => (
            Style::default().fg(color_from_anstyle(styles.pty_output.get_fg_color())),
            "\u{258c}", // ▌
        ),
    };

    let mut spans = Vec::with_capacity(line.segments.len() + 1);
    spans.push(Span::styled(format!("{marker} "), kind_style));
    for segment in &line.segments {
        let style = segment_style(segment, kind_style, styles);
        spans.push(Span::styled(segment.text.clone(), style));
    }
    Line::from(spans)
}

fn segment_style(segment: &InlineSegment, fallback: Style, styles: &ThemeStyles) -> Style {
    let mut style = fallback;
    let inline = segment.style.as_ref();
    if let Some(color) = inline.color {
        style = style.fg(color_from_anstyle(Some(color)));
    } else {
        // Fall back to the active palette's default for the kind. We
        // pick `response` for agent segments since the harness doesn't
        // carry its own theme.
        style = style.fg(color_from_anstyle(styles.response.get_fg_color()));
    }
    if inline.effects.contains(anstyle::Effects::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if inline.effects.contains(anstyle::Effects::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if inline.effects.contains(anstyle::Effects::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if inline.effects.contains(anstyle::Effects::DIMMED) {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}

fn render_composer(frame: &mut Frame<'_>, area: Rect, state: &RenderState) {
    let styles = active_styles();
    let prefix_style = Style::default()
        .fg(color_from_anstyle(styles.primary.get_fg_color()))
        .bold();
    let text_style = Style::default().fg(color_from_anstyle(Some(styles.foreground)));

    let prefix = state.prompt_prefix.clone();
    let body = state.input_buffer.clone();
    let placeholder = state.placeholder.clone();

    let mut line_spans = vec![Span::styled(prefix, prefix_style)];
    if body.is_empty()
        && let Some(ph) = placeholder
    {
        line_spans.push(Span::styled(
            ph,
            Style::default()
                .fg(color_from_anstyle(styles.secondary.get_fg_color()))
                .dim(),
        ));
    } else {
        line_spans.push(Span::styled(body, text_style));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())));
    let paragraph = Paragraph::new(Line::from(line_spans))
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);

    // Place the cursor inside the composer at the current edit position.
    // +1 on both axes to clear the rounded border.
    if state.input_enabled {
        let cursor_x = area.left()
            + 1
            + state.prompt_prefix.chars().count() as u16
            + state.input_cursor as u16;
        let cursor_y = area.top() + 1;
        frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
    }
}

/// Render a centred welcome banner when the transcript is empty.
fn render_welcome(frame: &mut Frame<'_>, area: Rect) {
    let styles = active_styles();
    let primary = color_from_anstyle(styles.primary.get_fg_color());
    let fg = color_from_anstyle(Some(styles.foreground));
    let secondary = color_from_anstyle(styles.secondary.get_fg_color());

    let text = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "\u{25cf} oxicode",
            Style::default().fg(primary).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Type a message to begin, or press / for commands.",
            Style::default().fg(fg),
        )),
        Line::from(Span::styled(
            "Use /help to see all available commands.",
            Style::default().fg(secondary),
        )),
    ];
    let para = Paragraph::new(text).alignment(Alignment::Center);
    frame.render_widget(para, area);
}

/// Render a 1-row reasoning/tool-stage indicator just above the composer.
fn render_reasoning_indicator(frame: &mut Frame<'_>, composer_area: Rect, stage: &str) {
    let styles = active_styles();
    let indicator_area = Rect {
        x: composer_area.x,
        y: composer_area.top().saturating_sub(1),
        width: composer_area.width,
        height: 1,
    };
    let spinner = "\u{25cc}"; // ◌
    let line = Line::from(vec![
        Span::styled(
            format!("{spinner} "),
            Style::default().fg(color_from_anstyle(styles.tool.get_fg_color())),
        ),
        Span::styled(
            stage.to_string(),
            Style::default()
                .fg(color_from_anstyle(styles.secondary.get_fg_color()))
                .add_modifier(Modifier::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), indicator_area);
}

/// Render the slash-command autocomplete popup as a floating panel above the
/// composer. Anchored to the composer's left edge, grows upward.
fn render_slash_popup(frame: &mut Frame<'_>, composer_area: Rect, state: &RenderState) {
    let styles = active_styles();
    let items = &state.slash_popup.items;
    if items.is_empty() {
        return;
    }

    let max_visible = 8usize;
    let visible = items.len().min(max_visible);
    let popup_h = visible as u16 + 2; // +2 for top/bottom border
    let width = composer_area.width.min(64);
    let popup_area = Rect {
        x: composer_area.left(),
        y: composer_area.top().saturating_sub(popup_h),
        width,
        height: popup_h,
    };
    frame.render_widget(Clear, popup_area);

    let border_color = color_from_anstyle(styles.secondary.get_fg_color());
    let title = Line::from(Span::styled(
        " Commands ",
        Style::default()
            .fg(color_from_anstyle(styles.primary.get_fg_color()))
            .add_modifier(Modifier::BOLD),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(title);
    let inner = block.inner(popup_area);
    frame.render_widget(&block, popup_area);

    // Column-align labels by padding to the widest visible label.
    let max_label = items
        .iter()
        .take(visible)
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(0);

    let primary = color_from_anstyle(styles.primary.get_fg_color());
    let fg = color_from_anstyle(Some(styles.foreground));
    let secondary = color_from_anstyle(styles.secondary.get_fg_color());

    for (i, item) in items.iter().take(visible).enumerate() {
        let is_selected = i == state.slash_popup.selected;
        let y = inner.top() + i as u16;
        let row_area = Rect {
            x: inner.left(),
            y,
            width: inner.width,
            height: 1,
        };

        let marker = if is_selected { "\u{25b8} " } else { "  " }; // ▸ or space
        let label_style = if is_selected {
            Style::default().fg(primary).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg)
        };
        let label_padded = format!("{:<width$}", item.label, width = max_label);
        let line = Line::from(vec![
            Span::styled(marker, label_style),
            Span::styled(label_padded, label_style),
            Span::raw("  "),
            Span::styled(&item.description, Style::default().fg(secondary)),
        ]);
        frame.render_widget(Paragraph::new(line), row_area);
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Small helpers
// ─────────────────────────────────────────────────────────────────────────

pub(crate) fn plain_segment(text: impl Into<String>) -> InlineSegment {
    InlineSegment {
        text: text.into(),
        style: Arc::new(InlineTextStyle::default()),
    }
}

pub(super) fn effective_scroll_offset(offset: usize, total: usize, viewport: usize) -> usize {
    if offset == usize::MAX {
        return total.saturating_sub(viewport);
    }
    // Clamp into [0, total.saturating_sub(viewport)].
    let max_start = total.saturating_sub(viewport);
    offset.min(max_start)
}

// ─────────────────────────────────────────────────────────────────────────
// Slash-command autocomplete popup
// ─────────────────────────────────────────────────────────────────────────

/// Filter the built-in slash commands by `token` (the text after `/`).
/// An empty token returns every command. Matching is prefix-based against
/// the canonical name and all aliases.
fn slash_filter(token: &str) -> Vec<SlashPopupItem> {
    SlashRegistry::builtin_commands()
        .into_iter()
        .filter(|(name, _, aliases)| {
            token.is_empty()
                || name.starts_with(token)
                || aliases.iter().any(|a| a.starts_with(token))
        })
        .map(|(name, desc, aliases)| {
            let mut label = format!("/{name}");
            for a in &aliases {
                label.push_str(&format!(", /{a}"));
            }
            SlashPopupItem {
                label,
                description: desc.to_string(),
                name: name.to_string(),
            }
        })
        .collect()
}

/// Recompute the slash popup from the current input buffer. The popup is
/// active when the buffer starts with `/` and has no space yet (the user is
/// still composing the command token, not its arguments). Called after every
/// buffer mutation in the input thread.
fn refresh_slash_popup(state: &mut RenderState) {
    let buf = state.input_buffer.clone();
    let active = buf.starts_with('/') && !buf[1..].contains(' ');
    if !active {
        state.slash_popup.open = false;
        state.slash_popup.items.clear();
        state.slash_popup.selected = 0;
        return;
    }
    let token = &buf[1..];
    let items = slash_filter(token);
    state.slash_popup.open = !items.is_empty();
    if items.is_empty() {
        state.slash_popup.selected = 0;
    } else {
        state.slash_popup.selected = state.slash_popup.selected.min(items.len() - 1);
    }
    state.slash_popup.items = items;
}

fn preview_tool_result(content: &str) -> String {
    const MAX: usize = 200;
    if content.chars().count() <= MAX {
        return content.to_string();
    }
    let truncated: String = content.chars().take(MAX).collect();
    format!("{truncated}\u{2026}")
}

fn color_from_anstyle(color: Option<anstyle::Color>) -> Color {
    match color {
        Some(anstyle::Color::Ansi(a)) => ansi_to_ratatui(a),
        Some(anstyle::Color::Ansi256(idx)) => Color::Indexed(idx.0),
        Some(anstyle::Color::Rgb(rgb)) => Color::Rgb(rgb.0, rgb.1, rgb.2),
        None => Color::Reset,
    }
}
fn ansi_to_ratatui(color: anstyle::AnsiColor) -> Color {
    use anstyle::AnsiColor as A;
    match color {
        A::Black => Color::Black,
        A::Red => Color::Red,
        A::Green => Color::Green,
        A::Yellow => Color::Yellow,
        A::Blue => Color::Blue,
        A::Magenta => Color::Magenta,
        A::Cyan => Color::Cyan,
        A::White => Color::Gray,
        A::BrightBlack => Color::DarkGray,
        A::BrightRed => Color::LightRed,
        A::BrightGreen => Color::LightGreen,
        A::BrightYellow => Color::LightYellow,
        A::BrightBlue => Color::LightBlue,
        A::BrightMagenta => Color::LightMagenta,
        A::BrightCyan => Color::LightCyan,
        A::BrightWhite => Color::White,
    }
}

// Suppress the unused-import warning while keeping the AtomicBool/Ordering
// available for future control flags (e.g. SIGINT safety net).
#[allow(dead_code, clippy::declare_interior_mutable_const)]
const _ATOMIC_REFS: (AtomicBool, Ordering) = (AtomicBool::new(false), Ordering::SeqCst);

#[cfg(test)]
mod slash_popup_tests {
    use super::*;

    #[test]
    fn empty_token_lists_all_commands() {
        let items = slash_filter("");
        // 7 built-in commands.
        assert!(items.len() >= 7);
        assert!(items.iter().any(|i| i.name == "quit"));
        assert!(items.iter().any(|i| i.name == "clear"));
        assert!(items.iter().any(|i| i.name == "model"));
    }

    #[test]
    fn prefix_filter_matches_name() {
        let items = slash_filter("qu");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "quit");
        assert!(items[0].label.contains("/quit"));
    }

    #[test]
    fn prefix_filter_matches_alias() {
        // "cl" should match "clear" (alias "cls") and "compact".
        let items = slash_filter("cl");
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"clear"));
    }

    #[test]
    fn popup_opens_on_slash() {
        let mut state = RenderState::default();
        state.input_buffer = "/".to_string();
        refresh_slash_popup(&mut state);
        assert!(state.slash_popup.open);
        assert!(!state.slash_popup.items.is_empty());
    }

    #[test]
    fn popup_closes_on_space() {
        let mut state = RenderState::default();
        state.input_buffer = "/quit ".to_string();
        refresh_slash_popup(&mut state);
        assert!(!state.slash_popup.open);
    }

    #[test]
    fn popup_closes_on_non_slash() {
        let mut state = RenderState::default();
        state.input_buffer = "hello".to_string();
        refresh_slash_popup(&mut state);
        assert!(!state.slash_popup.open);
    }

    #[test]
    fn popup_filters_as_user_types() {
        let mut state = RenderState::default();
        state.input_buffer = "/m".to_string();
        refresh_slash_popup(&mut state);
        assert!(state.slash_popup.open);
        // Every item's canonical name must start with 'm' (model is the
        // only command matching the "m" prefix).
        assert!(
            state
                .slash_popup
                .items
                .iter()
                .all(|i| i.name.starts_with('m'))
        );
    }

    #[test]
    fn popup_selection_clamps_on_shrink() {
        let mut state = RenderState::default();
        state.input_buffer = "/".to_string();
        refresh_slash_popup(&mut state);
        let full_count = state.slash_popup.items.len();
        state.slash_popup.selected = full_count - 1;
        // Narrow the filter so fewer items remain.
        state.input_buffer = "/qu".to_string();
        refresh_slash_popup(&mut state);
        assert!(state.slash_popup.selected < state.slash_popup.items.len());
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use oxicode_vtui::tui::core::InlineHandle;
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc;

    /// Render `render_frame` into a TestBackend and return the concatenated
    /// cell text. This catches regressions like a missing render_composer
    /// call — `#![allow(dead_code)]` in lib.rs suppresses the unused-fn lint,
    /// so only a render assertion can prove the composer is painted.
    fn render_frame_to_string(state: &RenderState) -> String {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = InlineHandle::new_for_tests(tx);
        terminal
            .draw(|f| render_frame(f, state, &handle))
            .expect("draw");
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn welcome_screen_shown_when_transcript_empty() {
        let state = RenderState::default();
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("oxicode"),
            "welcome banner must appear when transcript is empty"
        );
    }

    #[test]
    fn composer_is_painted() {
        // Regression guard: the composer prompt prefix must appear in the
        // rendered output. This would have caught the missing
        // render_composer call (advisory 2026-08-04).
        let mut state = RenderState::default();
        state.input_enabled = true;
        state.prompt_prefix = "> ".to_string();
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains('>'),
            "composer prompt prefix must be painted"
        );
    }

    #[test]
    fn slash_popup_renders_command_list() {
        let mut state = RenderState::default();
        state.slash_popup.open = true;
        state.slash_popup.items = slash_filter("");
        let rendered = render_frame_to_string(&state);
        assert!(rendered.contains("Commands"), "popup title must render");
        assert!(rendered.contains("/quit"), "popup must list /quit");
    }

    #[test]
    fn composer_and_popup_render_together() {
        let mut state = RenderState::default();
        state.prompt_prefix = "> ".to_string();
        state.input_buffer = "/qu".to_string();
        state.slash_popup.open = true;
        state.slash_popup.items = slash_filter("qu");
        let rendered = render_frame_to_string(&state);
        assert!(rendered.contains("Commands"), "popup must render");
        assert!(rendered.contains("/quit"), "popup must list /quit");
        assert!(rendered.contains('>'), "composer must still render");
    }

    #[test]
    fn transcript_wraps_long_lines() {
        // A line wider than the terminal must wrap, not clip.
        let mut state = RenderState::default();
        state.transcript.push(TranscriptLine {
            kind: InlineMessageKind::Agent,
            segments: vec![plain_segment(
                "This is a very long agent response line that should wrap across multiple terminal rows when rendered at a narrow width.".to_string()
            )],
        });
        let backend = TestBackend::new(40, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = InlineHandle::new_for_tests(tx);
        terminal
            .draw(|f| render_frame(f, &state, &handle))
            .expect("draw");
        let buf = terminal.backend().buffer();
        // The word "wrap" must appear somewhere — it would be clipped if
        // the List widget was still used at 40 cols.
        let mut full = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    full.push_str(cell.symbol());
                }
            }
            full.push('\n');
        }
        assert!(
            full.contains("wrap"),
            "long line must wrap, not clip — text should be visible past col 40"
        );
    }
}
