//! Main TUI event loop and application state.

use super::handlers;
use super::render;
use super::slash;
use super::welcome;
use crate::app::agent_session::SessionEvent;
use crate::app::agent_session_runtime::{
    create_agent_session_from_services, create_agent_session_services,
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
};
use crate::context::auto_compaction::CompactionReason;
use crate::util::slash_commands::BUILTIN_SLASH_COMMANDS;
use anyhow::Result;
use oxi_agent::AgentEvent;
use oxi_store::session::SessionManager;
use oxi_tui::theme::Theme;
use oxi_tui::widgets::{
    chat::{ChatMessage, ChatViewState, ContentBlock, MessageRole},
    footer::FooterState,
    input::InputState,
};
use std::io::{self, Write};
use std::panic;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::mpsc;

use crossterm::{
    cursor::Hide,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

// ── Terminal Lifecycle ───────────────────────────────────────────────────

/// Terminal wrapper following ratatui best practices.
/// Encapsulates setup/teardown, panic hook, and mouse tracking.
struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    tty_ok: bool,
}

impl Tui {
    fn enter() -> Result<Self> {
        // Set panic hook first — ensures terminal is restored on panic
        Self::set_panic_hook();

        let tty_ok = enable_raw_mode().is_ok();
        let mut stdout = io::stdout();

        if tty_ok {
            let _ = execute!(
                stdout,
                EnterAlternateScreen,
                Hide,
                EnableBracketedPaste,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
            );
            // Enable mouse scroll tracking without drag tracking.
            // ?1000h = click/release/scroll, ?1006h = SGR extended coords.
            // Intentionally skip ?1002h so terminal handles drag-to-select natively.
            let _ = stdout.write_all(b"\x1b[?1000h\x1b[?1006h");
            let _ = stdout.flush();
        }

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        if tty_ok {
            let _ = terminal.clear();
        }

        Ok(Self { terminal, tty_ok })
    }

    fn exit(&mut self) -> Result<()> {
        if self.tty_ok {
            // 1. Disable mouse tracking (before leaving alternate screen)
            let _ = io::stdout().write_all(b"\x1b[?1000l\x1b[?1006l");
            let _ = io::stdout().flush();
            // 2. Pop keyboard enhancements and bracketed paste
            execute!(
                self.terminal.backend_mut(),
                PopKeyboardEnhancementFlags,
                DisableBracketedPaste
            )?;
            // 3. Show cursor before leaving alternate screen
            self.terminal.show_cursor()?;
            // 4. Leave alternate screen
            execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
            // 5. Disable raw mode last
            disable_raw_mode()?;
        }
        Ok(())
    }

    fn set_panic_hook() {
        let original_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            // Restore terminal state before printing panic info
            let _ = io::stdout().write_all(b"\x1b[?1000l\x1b[?1006l");
            let _ = io::stdout().flush();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            original_hook(panic_info);
        }));
    }
}

impl std::ops::Deref for Tui {
    type Target = Terminal<CrosstermBackend<io::Stdout>>;
    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl std::ops::DerefMut for Tui {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.exit();
    }
}

// ── UI Events (agent → TUI) ──────────────────────────────────────────────

pub(crate) enum UiEvent {
    // ── Agent lifecycle (pi-mono: agent_start / agent_end) ──────────
    AgentStart,
    AgentEnd,

    // ── Turn lifecycle (pi-mono: turn_start / turn_end) ────────────
    #[allow(dead_code)]
    TurnStart {
        #[allow(dead_code)]
        turn_number: u32,
    },
    #[allow(dead_code)]
    TurnEnd {
        #[allow(dead_code)]
        turn_number: u32,
    },

    // ── Message lifecycle (pi-mono: message_start / update / end) ──
    /// A new message is being streamed. pi-mono: message_start.
    MessageStart {
        message: oxi_ai::Message,
    },
    /// Full message snapshot with current content blocks. pi-mono: message_update.
    /// Content blocks are already separated (text vs toolCall) by the provider.
    MessageUpdate {
        message: oxi_ai::Message,
        delta: Option<String>,
    },
    /// Message streaming is complete. pi-mono: message_end.
    MessageEnd {
        message: oxi_ai::Message,
    },

    // ── Tool execution ─────────────────────────────────────────────
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: oxi_ai::ToolResult,
        is_error: bool,
    },

    // ── Legacy events (kept for backward compat during transition) ──
    Thinking,
    ThinkingDelta(String),
    Error(String),

    // ── Session events ─────────────────────────────────────────────
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
    /// Token usage updated.
    TokenUsage {
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_write_tokens: u32,
        context_window_pct: f32,
        total_cost: f64,
    },

    /// A queued message is being auto-processed (sent from the worker
    /// thread when draining the steering/follow-up queue after a run).
    /// The TUI should display the user message and enter streaming state.
    AutoProcessStart {
        prompt: String,
    },

    /// A system message to display from an async operation (e.g., compaction).
    SystemMessage(String),
}

// ── Spinner ──────────────────────────────────────────────────────────────

pub(super) const SPINNER: &[&str] = &["|", "/", "-", "\\"];

// ── App State ────────────────────────────────────────────────────────────

/// Setup wizard state
#[derive(Debug, Clone)]
pub(crate) enum SetupStep {
    /// First step: OAuth or API Key
    SelectAuthType {
        auth_type: Option<String>, // "oauth" or "apikey"
        selected: usize,
    },
    /// Select provider from list
    SelectProvider {
        providers: Vec<(String, bool)>, // (name, has_key)
        selected: usize,
    },
    /// Enter API key for selected provider
    EnterApiKey {
        provider: String,
        key: String,
        #[allow(dead_code)]
        masked_cursor: usize,
    },
    /// Select a model from the provider's available models
    SelectModel {
        provider: String,
        models: Vec<String>,
        selected: usize,
    },
    /// Done — show success
    Done { provider: String, model: String },
}

/// Overlay types for interactive TUI dialogs.
#[derive(Debug, Clone)]
pub(crate) enum AppOverlay {
    /// Initial setup wizard
    Setup(SetupStep),
    /// Model selector overlay
    ModelSelect {
        models: Vec<String>,
        filter: String,
        selected: usize,
    },
    /// Provider config wizard (reuses SetupStep)
    ProviderConfig(SetupStep),
    /// Logout provider selector
    LogoutSelect {
        providers: Vec<String>,
        selected: usize,
    },
    /// Session resume selector
    ResumeSelect {
        sessions: Vec<oxi_store::session::SessionInfo>,
        selected: usize,
    },
    /// Routing status panel (toggle with Ctrl+R)
    #[allow(dead_code)]
    RoutingStatus {
        data: oxi_tui::widgets::routing::RoutingStatusData,
        visible: bool,
    },
}

// ── Session Switch Action ──────────────────────────────────────────────

/// Action requested by a slash command or overlay to switch sessions.
#[derive(Debug, Clone)]
pub(crate) enum TuiNextAction {
    /// Switch to an existing session file.
    SwitchSession(String),
    /// Start a fresh session.
    NewSession,
}

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
    /// Active overlay (None = normal chat mode)
    pub overlay: Option<AppOverlay>,
    /// Component-based overlay (takes priority over AppOverlay variants for
    /// ModelSelect, LogoutSelect, ResumeSelect). Migrated from AppOverlay.
    pub overlay_state: Option<Box<dyn super::overlay::OverlayComponent>>,
    /// WASM extension manager for dynamic commands
    pub wasm_ext: Option<std::sync::Arc<crate::extensions::WasmExtensionManager>>,
    /// Session file path for the current session
    pub session_file_path: Option<String>,
    /// Requested session switch action (checked by outer loop)
    pub next_action: Option<TuiNextAction>,
    /// Count of pending steering messages (shown in busy input)
    pub pending_steering: usize,
    /// Whether session needs to be persisted to disk
    pub needs_persist: bool,
    /// Length of text already rendered from the snapshot's Text block.
    /// Used to compute incremental text delta from full snapshot.
    /// Tracks bytes (not chars) to allow fast slicing of UTF-8 text.
    snapshot_text_rendered: usize,
    /// Per-block byte offsets for Thinking blocks already rendered.
    /// Prevents duplicate thinking content on repeated MessageUpdates.
    /// Uses Vec to support multiple Thinking blocks (defensive —
    /// current providers emit at most one, but future ones may differ).
    snapshot_thinking_rendered: Vec<usize>,
    /// Whether the initial empty Text block has been created in the TUI.
    /// Without this flag, the very first delta creates a Text content block
    /// via `insert(0, ...)`, but on the *next* MessageUpdate, `first_mut()`
    /// finds the existing block and we slice correctly.  The flag is defensive
    /// — it ensures we always go through the `first_mut` path once the
    /// block exists.
    snapshot_text_block_created: bool,
    /// Questionnaire bridge — set by run_tui_interactive_impl() from App::questionnaire_bridge().
    questionnaire_bridge:
        Option<std::sync::Arc<oxi_agent::tools::questionnaire::QuestionnaireBridge>>,
    /// Tool execution start times for measuring duration.
    pub(crate) tool_start_times: std::collections::HashMap<String, std::time::Instant>,
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
            overlay: None,
            overlay_state: None,
            wasm_ext: None,
            session_file_path: None,
            next_action: None,
            pending_steering: 0,
            needs_persist: false,
            snapshot_text_rendered: 0,
            snapshot_thinking_rendered: Vec::new(),
            snapshot_text_block_created: false,
            questionnaire_bridge: None,
            tool_start_times: std::collections::HashMap::new(),
        }
    }

    // ── Input helpers ──

    pub fn input_value(&self) -> String {
        self.input.text()
    }

    pub fn input_clear(&mut self) {
        self.input.clear();
        self.clear_slash_completions();
    }

    pub fn input_set_text(&mut self, text: String) {
        self.input.set_text(text);
    }

    pub fn clear_slash_completions(&mut self) {
        self.slash_completions.clear();
        self.slash_completion_index = 0;
        self.slash_completion_active = false;
    }

    pub fn update_slash_completions(&mut self) {
        let input_str = self.input_value();
        let text = input_str.trim();
        if !text.starts_with('/') || text.contains(' ') {
            self.clear_slash_completions();
            return;
        }
        let cmd_part = text.split_whitespace().next().unwrap_or("");
        let query = if cmd_part.len() > 1 {
            &cmd_part[1..]
        } else {
            ""
        };
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

    /// Get the currently selected slash command (for direct execution).
    pub fn selected_slash_command(&self) -> Option<&slash::SlashCompletion> {
        if !self.slash_completion_active || self.slash_completions.is_empty() {
            return None;
        }
        self.slash_completions.get(self.slash_completion_index)
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
        self.snapshot_text_rendered = 0;
        self.snapshot_thinking_rendered.clear();
        self.snapshot_text_block_created = false;
    }

    /// Reset snapshot tracking counters for a new streaming message.
    /// Called from the MessageStart handler to ensure counters are
    /// clean even when MessageStart arrives without a preceding
    /// AppState::start_streaming() call.
    pub fn reset_snapshot_tracking(&mut self) {
        self.snapshot_text_rendered = 0;
        self.snapshot_thinking_rendered.clear();
        self.snapshot_text_block_created = false;
    }

    #[allow(dead_code)]
    pub fn stream_text_delta(&mut self, _delta: &str) {}

    /// Update the streaming message from a full MessageUpdate snapshot.
    ///
    /// pi-mono pattern: render from the snapshot's content blocks, NOT from
    /// the raw delta string. The provider has already separated Text blocks
    /// from ToolCall blocks in the snapshot. If the provider sent tool call
    /// JSON as TextDelta, the snapshot's Text block won't contain it (it'll
    /// be in a ToolCall block instead). This prevents JSON appearing in chat.
    pub fn update_streaming_message(&mut self, msg: &oxi_ai::Message, _delta: Option<&str>) {
        if let oxi_ai::Message::Assistant(assistant) = msg {
            let mut thinking_block_idx: usize = 0;
            for block in &assistant.content {
                match block {
                    oxi_ai::ContentBlock::Text(t) => {
                        // Only render new text beyond what we've already rendered.
                        // This is the pi-mono snapshot-based approach: use the
                        // provider's Text block (which is properly separated
                        // from tool calls), not the raw delta string.
                        let text = &t.text;
                        // Use >= so that pure-whitespace additions (a single space)
                        // are not skipped. Previously `>` caused spaces between
                        // words to be dropped when the provider sent them as a
                        // separate delta that only grew the text by 1 byte.
                        if text.len() >= self.snapshot_text_rendered {
                            // Use char_indices to find the safe byte boundary
                            // closest to snapshot_text_rendered (multi-byte chars
                            // like Korean are 3 bytes each in UTF-8).
                            let byte_off = text
                                .char_indices()
                                .map(|(i, _)| i)
                                .find(|&i| i >= self.snapshot_text_rendered)
                                .unwrap_or(text.len());
                            let new_text = &text[byte_off..];
                            if !new_text.is_empty() {
                                self.chat.stream_text_delta(new_text);
                            }
                            self.snapshot_text_rendered = text.len();
                            if !text.is_empty() {
                                self.snapshot_text_block_created = true;
                            }
                        }
                    }
                    oxi_ai::ContentBlock::ToolCall(tc) => {
                        // stream_tool_call is idempotent — it checks tool_tracker
                        let args_str = serde_json::to_string(&tc.arguments)
                            .unwrap_or_else(|_| tc.arguments.to_string());
                        self.chat.stream_tool_call(
                            tc.id.clone(),
                            tc.name.clone(),
                            args_str,
                            oxi_tui::widgets::chat::ToolCallStatus::Requested,
                        );
                    }
                    oxi_ai::ContentBlock::Thinking(t) => {
                        // Thinking blocks — only append new content beyond
                        // what we've already rendered for this specific block.
                        // Per-block tracking prevents content loss if a future
                        // provider emits multiple Thinking blocks.
                        let thinking = &t.thinking;
                        while self.snapshot_thinking_rendered.len() <= thinking_block_idx {
                            self.snapshot_thinking_rendered.push(0);
                        }
                        if thinking.len() > self.snapshot_thinking_rendered[thinking_block_idx] {
                            let prev = self.snapshot_thinking_rendered[thinking_block_idx];
                            let byte_off = thinking
                                .char_indices()
                                .map(|(i, _)| i)
                                .find(|&i| i >= prev)
                                .unwrap_or(thinking.len());
                            let new_thinking = &thinking[byte_off..];
                            if !new_thinking.is_empty() {
                                self.chat.stream_thinking(new_thinking.to_string(), false);
                            }
                            self.snapshot_thinking_rendered[thinking_block_idx] = thinking.len();
                        }
                        thinking_block_idx += 1;
                    }
                    oxi_ai::ContentBlock::Image(img) => {
                        self.chat
                            .stream_image(img.mime_type.clone(), img.data.clone());
                    }
                    oxi_ai::ContentBlock::Unknown(_) => {}
                }
            }
        }
    }

    /// Finalize the streaming message from a MessageEnd snapshot.
    pub fn finalize_streaming_message(&mut self, msg: &oxi_ai::Message) {
        if let oxi_ai::Message::Assistant(assistant) = msg {
            // Update token usage from the completed message
            let usage = &assistant.usage;
            tracing::info!(
                "[TOKENS] input={} output={} cache_read={} cache_write={} total={}",
                usage.input,
                usage.output,
                usage.cache_read,
                usage.cache_write,
                usage.total_tokens
            );
            let context_window_pct = if usage.total_tokens > 0 {
                (usage.total_tokens as f32 / 200_000.0) * 100.0
            } else if usage.input > 0 || usage.output > 0 {
                ((usage.input + usage.output + usage.cache_read + usage.cache_write) as f32
                    / 200_000.0)
                    * 100.0
            } else {
                0.0
            };
            self.footer_state.data.input_tokens = usage.input as u32;
            self.footer_state.data.output_tokens = usage.output as u32;
            self.footer_state.data.cache_read_tokens = usage.cache_read as u32;
            self.footer_state.data.cache_write_tokens = usage.cache_write as u32;
            self.footer_state.data.context_window_pct = context_window_pct;
            self.footer_state.data.total_cost = usage.cost.total();
            self.footer_state.data.context_tokens =
                (usage.input + usage.output + usage.cache_read + usage.cache_write) as u32;
        }
    }

    #[allow(dead_code)]
    pub fn finish_streaming(&mut self) {
        let was_streaming = self.chat.is_streaming();
        self.chat.finish_streaming();
        self.is_agent_busy = false;
        self.snapshot_text_rendered = 0;
        self.snapshot_thinking_rendered.clear();
        self.snapshot_text_block_created = false;
        if was_streaming {
            self.message_count += 1;
            // Refresh last code block from completed message
            self.chat.refresh_last_code_block();
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

// ── Shared agent runtime ────────────────────────────────────────────────

/// Returns a process-lifetime Tokio runtime used by the agent worker thread.
/// Re-creating a multi-threaded runtime on every session switch is expensive;
/// this `OnceLock` ensures exactly one instance lives for the entire process.
fn get_agent_runtime() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to build agent runtime")
    })
}

// ── Main entry point ─────────────────────────────────────────────────────

/// Run the TUI interactive mode.
pub async fn run_tui_interactive(app: crate::App) -> Result<()> {
    run_tui_interactive_impl(app, false).await
}

/// Run TUI interactive mode, optionally resuming the most recent session.
pub async fn run_tui_interactive_with_continue(app: crate::App, resume_last: bool) -> Result<()> {
    run_tui_interactive_impl(app, resume_last).await
}

async fn run_tui_interactive_impl(app: crate::App, resume_last: bool) -> Result<()> {
    // ── Extract resources from App (needed for session switching loop) ──
    let settings = app.settings().clone();
    let mut model_id = app.model_id();
    let tools = app.agent().tools();
    let wasm_ext = app.wasm_ext().cloned();
    let questionnaire_bridge = app.questionnaire_bridge().cloned();
    let cwd: String = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    let cwd_path = std::env::current_dir().unwrap_or_default();
    let git_branch = crate::util::git_utils::get_current_branch(&cwd_path);

    // ── Determine initial session ──
    let mut session_target: Option<String> = if resume_last {
        oxi_store::session::find_recent_session_path(&cwd)
    } else {
        None
    };

    // ── Enter terminal ONCE ──
    let mut tui = Tui::enter()?;
    let theme = Theme::dark();

    // ── Session switching loop ──
    loop {
        let is_resuming = session_target.is_some();

        let session_manager = match &session_target {
            Some(path) => SessionManager::open(path, None, Some(&cwd)),
            None => SessionManager::create(&cwd, None),
        };

        let services = create_agent_session_services(CreateAgentSessionServicesOptions::new(
            cwd_path.clone(),
        ))?;
        let services = Arc::new(services);

        let create_result =
            create_agent_session_from_services(CreateAgentSessionFromServicesOptions {
                services: services.clone(),
                session_manager,
                model_id: Some(model_id.clone()),
                thinking_level: Some(settings.thinking_level),
                scoped_models: Vec::new(),
                tool_registry: Some(tools.clone()),
            })?;

        let agent_session = create_result.session;
        if let Some(msg) = create_result.model_fallback_message {
            tracing::warn!("Model fallback: {}", msg);
        }

        let (session_event_tx, mut session_event_rx) = mpsc::unbounded_channel::<SessionEvent>();
        agent_session.subscribe(Box::new(move |event| {
            let _ = session_event_tx.send(event.clone());
        }));

        let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiEvent>();
        let (prompt_tx, mut prompt_rx) = mpsc::channel::<String>(64);

        // Agent worker thread
        let session_handle = agent_session.clone_handle();
        // Clone prompt_tx so the worker thread can auto-reprocess queued messages.
        // The cloned sender feeds back into prompt_rx, triggering the outer while loop.
        let prompt_tx_worker = prompt_tx.clone();
        let ui_tx_for_thread = ui_tx.clone();
        let agent_handle = std::thread::spawn(move || {
            let rt = get_agent_runtime();
            rt.block_on(async {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async {
                        while let Some(prompt) = prompt_rx.recv().await {
                            tracing::info!("[TUI] Received prompt, starting agent run");
                            let (event_tx, event_rx) = std::sync::mpsc::channel::<AgentEvent>();
                            let ui_fwd = ui_tx_for_thread.clone();
                            let session_h = session_handle.clone_handle();
                            // Forward events on a dedicated thread so it's never starved
                            // by the agent's synchronous emit callbacks.
                            let forwarder_handle = std::thread::spawn(move || {
                                let mut event_count = 0u32;
                                tracing::info!("[FORWARDER] Thread started, waiting for events");
                                while let Ok(event) = event_rx.recv() {
                                    event_count += 1;
                                    tracing::info!(
                                        "[FORWARDER] Event #{}: {:?}",
                                        event_count,
                                        std::mem::discriminant(&event)
                                    );
                                    let ui_event = match event {
                                        // ── Agent lifecycle ─────────────────────────
                                        AgentEvent::AgentStart { .. } => UiEvent::AgentStart,
                                        AgentEvent::AgentEnd { .. } => UiEvent::AgentEnd,

                                        // ── Turn lifecycle ───────────────────────────
                                        AgentEvent::TurnStart { turn_number } => {
                                            UiEvent::TurnStart { turn_number }
                                        }
                                        AgentEvent::TurnEnd { turn_number, .. } => {
                                            UiEvent::TurnEnd { turn_number }
                                        }

                                        // ── Message lifecycle (pi-mono pattern) ─────
                                        // These carry full message snapshots with properly
                                        // separated content blocks from the provider.
                                        AgentEvent::MessageStart { message } => {
                                            UiEvent::MessageStart { message }
                                        }
                                        AgentEvent::MessageUpdate { message, delta } => {
                                            UiEvent::MessageUpdate { message, delta }
                                        }
                                        AgentEvent::MessageEnd { message } => {
                                            UiEvent::MessageEnd { message }
                                        }

                                        // ── Tool execution (structured events) ──────
                                        AgentEvent::ToolExecutionStart {
                                            tool_call_id,
                                            tool_name,
                                            args,
                                        } => UiEvent::ToolExecutionStart {
                                            tool_call_id,
                                            tool_name,
                                            args,
                                        },
                                        AgentEvent::ToolExecutionEnd {
                                            tool_call_id,
                                            tool_name,
                                            result,
                                            is_error,
                                        } => UiEvent::ToolExecutionEnd {
                                            tool_call_id,
                                            tool_name,
                                            result,
                                            is_error,
                                        },

                                        // ── Legacy tool events (from Agent::run_with_channel) ──
                                        // Map to the same structured UiEvents.
                                        AgentEvent::ToolStart {
                                            tool_call_id,
                                            tool_name,
                                            arguments,
                                        } => UiEvent::ToolExecutionStart {
                                            tool_call_id,
                                            tool_name,
                                            args: arguments,
                                        },
                                        AgentEvent::ToolComplete { result } => {
                                            UiEvent::ToolExecutionEnd {
                                                tool_call_id: result.tool_call_id.clone(),
                                                tool_name: String::new(),
                                                result,
                                                is_error: false, // status checked in handler
                                            }
                                        }
                                        AgentEvent::ToolError {
                                            tool_call_id,
                                            error,
                                        } => UiEvent::ToolExecutionEnd {
                                            tool_call_id,
                                            tool_name: String::new(),
                                            result: oxi_ai::ToolResult {
                                                tool_call_id: String::new(),
                                                content: error,
                                                status: "error".to_string(),
                                            },
                                            is_error: true,
                                        },

                                        // ── Legacy streaming events ─────────────────
                                        // Still emitted by agent.rs alongside MessageUpdate.
                                        // TUI now prefers MessageUpdate, so we skip these.
                                        AgentEvent::Start { .. } => {
                                            // AgentStart equivalent — no action needed
                                            // since we also get AgentStart from events.rs
                                            continue;
                                        }
                                        AgentEvent::Thinking => UiEvent::Thinking,
                                        AgentEvent::ThinkingDelta { text } => {
                                            UiEvent::ThinkingDelta(text)
                                        }
                                        AgentEvent::TextChunk { .. } => {
                                            // SKIP: TUI now renders from MessageUpdate snapshots,
                                            // not incremental text deltas. This prevents raw
                                            // JSON from tool calls appearing in chat.
                                            continue;
                                        }
                                        AgentEvent::ToolCall { .. } => {
                                            // SKIP: This is the LLM's request, not execution.
                                            // ToolExecutionStart arrives when execution begins.
                                            continue;
                                        }

                                        // ── Completion & errors ──────────────────────
                                        AgentEvent::Error { message, .. } => {
                                            UiEvent::Error(message)
                                        }

                                        // ── Usage ────────────────────────────────────
                                        AgentEvent::Usage {
                                            input_tokens,
                                            output_tokens,
                                        } => {
                                            let _ = ui_fwd.send(UiEvent::TokenUsage {
                                                input_tokens: input_tokens as u32,
                                                output_tokens: output_tokens as u32,
                                                cache_read_tokens: 0,
                                                cache_write_tokens: 0,
                                                context_window_pct: 0.0,
                                                total_cost: 0.0,
                                            });
                                            continue;
                                        }

                                        // ── Steering / follow-up consumption ──
                                        AgentEvent::SteeringMessage { .. } => {
                                            // A steering message was consumed from the queue
                                            // → emit queue update so TUI shows current count
                                            let steering_q = session_h.steering_queue();
                                            let follow_up_q = session_h.follow_up_queue();
                                            let pending =
                                                steering_q.read().len() + follow_up_q.read().len();
                                            let _ = ui_fwd.send(UiEvent::QueueUpdate { pending });
                                            continue;
                                        }
                                        AgentEvent::FollowUpMessage { .. } => {
                                            let steering_q = session_h.steering_queue();
                                            let follow_up_q = session_h.follow_up_queue();
                                            let pending =
                                                steering_q.read().len() + follow_up_q.read().len();
                                            let _ = ui_fwd.send(UiEvent::QueueUpdate { pending });
                                            continue;
                                        }

                                        // ── Everything else: skip ───────────────────
                                        _ => continue,
                                    };
                                    tracing::info!("[FORWARDER] Sending UiEvent to ui_fwd");
                                    if ui_fwd.send(ui_event).is_err() {
                                        tracing::warn!("[FORWARDER] ui_fwd send failed, breaking");
                                        break;
                                    }
                                    tracing::info!("[FORWARDER] UiEvent sent successfully");
                                }
                                tracing::info!("[FORWARDER] Event loop ended");
                            });
                            let sh = session_handle.clone_handle();
                            let agent = sh.agent_ref();
                            sh.reset_should_stop();
                            sh.agent_ref().reset_cancel();
                            let steering_q = sh.steering_queue();
                            let follow_up_q = sh.follow_up_queue();
                            let should_stop_flag = sh.should_stop_flag();
                            let hooks = oxi_agent::AgentHooks {
                                should_stop_after_turn: Some(Arc::new(move |_ctx| {
                                    should_stop_flag.load(Ordering::SeqCst)
                                })),
                                get_steering_messages: Some(Arc::new(move || {
                                    steering_q.write().drain(..).collect::<Vec<String>>()
                                })),
                                get_follow_up_messages: Some(Arc::new(move || {
                                    follow_up_q.write().drain(..).collect::<Vec<String>>()
                                })),
                                tool_execution: oxi_agent::ToolExecutionMode::Sequential,
                                ..Default::default()
                            };
                            agent.set_hooks(hooks);
                            let agent_clone = Arc::clone(&agent);
                            tracing::info!("[AGENT-WORKER] Spawning agent task");

                            // Mark AgentSession streaming flag so is_streaming() is accurate
                            // for any code that checks it (extensions, RPC, etc.).
                            session_handle
                                .streaming_flag()
                                .store(true, Ordering::SeqCst);

                            let agent_handle = tokio::task::spawn_local(async move {
                                tracing::info!(
                                    "[AGENT-WORKER] Agent task started, calling run_with_channel"
                                );
                                let result = agent_clone.run_with_channel(prompt, event_tx).await;
                                if let Err(ref e) = result {
                                    tracing::error!("Agent run_with_channel error: {:?}", e);
                                }
                                tracing::info!(
                                    "[AGENT-WORKER] Agent run_with_channel completed: {:?}",
                                    result
                                );
                                result
                            });
                            // Agent runs on LocalSet, forwarder on its own thread.
                            // Agent drops event_tx when done → forwarder sees disconnect → exits.
                            let _ = agent_handle.await;

                            // Clear streaming flag now that the agent run is complete.
                            session_handle
                                .streaming_flag()
                                .store(false, Ordering::SeqCst);

                            // Do NOT call forwarder_handle.join() here — it blocks the
                            // tokio runtime thread, preventing prompt_rx.recv() from
                            // resolving on the next iteration. The forwarder will exit
                            // on its own when event_rx is disconnected.
                            //
                            // NOTE: The forwarder thread is detached and will clean up
                            // when it sees the channel disconnect. If we need to wait
                            // for it before session teardown, the outer loop handles
                            // that via drop(prompt_tx) + agent_handle.join().
                            let _ = forwarder_handle; // move ownership, don't block

                            // ── Auto-process queued messages ──────────────────
                            // After the agent finishes, check if new messages were
                            // queued during the run (via steer_sync). If so, feed
                            // the first one back through prompt_tx_worker so the outer
                            // while loop picks it up as a new prompt. Remaining messages
                            // stay in the queue for subsequent iterations.
                            let first_pending: Option<String> = {
                                let sq = session_handle.steering_queue();
                                let fq = session_handle.follow_up_queue();
                                let mut sq_guard = sq.write();
                                let mut fq_guard = fq.write();
                                // Steering takes priority over follow-up
                                sq_guard.pop_front().or_else(|| fq_guard.pop_front())
                            };
                            if let Some(msg) = first_pending {
                                let remaining = session_handle.pending_message_count();
                                tracing::info!(
                                    "[AGENT-WORKER] Auto-processing queued message ({} remaining)",
                                    remaining
                                );
                                let _ = ui_tx_for_thread
                                    .send(UiEvent::QueueUpdate { pending: remaining });
                                // Tell TUI to show user message + enter streaming state
                                let _ = ui_tx_for_thread.send(UiEvent::AutoProcessStart {
                                    prompt: msg.clone(),
                                });
                                let _ = prompt_tx_worker.send(msg).await;
                            }
                        }
                    })
                    .await;
            });
        });

        // ── Create state ──
        let mut state = AppState::new();
        state.session_file_path = session_target.clone();

        // Restore previous messages if resuming
        if is_resuming {
            if let Some(ref path) = session_target {
                let sm = oxi_store::session::SessionManager::open(path, None, Some(&cwd));
                let branch = sm.get_branch(None);
                for entry in &branch {
                    match &entry.message {
                        oxi_store::session::AgentMessage::User { content } => {
                            state.add_user_message(content.as_str().to_string());
                        }
                        oxi_store::session::AgentMessage::Assistant { content, .. } => {
                            let text: String = content
                                .iter()
                                .filter_map(|b| match b {
                                    oxi_store::session::AssistantContentBlock::Text { text } => {
                                        Some(text.as_str())
                                    }
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("");
                            if !text.is_empty() {
                                state.add_system_message(text);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Footer
        state.footer_state.data.pwd = Some(cwd.clone());
        state.footer_state.data.model_name = model_id.clone();
        state.footer_state.data.git_branch = git_branch.clone();
        state.footer_state.data.provider_name =
            model_id.split('/').next().unwrap_or("").to_string();
        state.footer_state.data.version = env!("CARGO_PKG_VERSION").to_string();
        state.footer_state.data.thinking_level =
            Some(format!("{:?}", settings.thinking_level).to_lowercase());
        state.wasm_ext = wasm_ext.clone();
        state.questionnaire_bridge = questionnaire_bridge.clone();

        // Push welcome message (only for new sessions, not resumed)
        if !is_resuming {
            let tool_labels: Vec<(String, String)> = {
                let registry = tools.clone();
                let names = registry.names();
                names
                    .iter()
                    .filter_map(|name| {
                        registry
                            .get(name)
                            .map(|t| (name.clone(), t.label().to_string()))
                    })
                    .collect()
            };
            let tool_names: Vec<String> = tool_labels.iter().map(|(n, _)| n.clone()).collect();
            let skill_names: Vec<String> = {
                let sm = crate::skills::SkillManager::load_from_dir(
                    &crate::skills::SkillManager::skills_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("/dev/null")),
                )
                .unwrap_or_else(|_| crate::skills::SkillManager::new());
                sm.all().iter().map(|s| s.name.clone()).collect()
            };
            let agents_md_path = welcome::detect_agents_md(&cwd_path);
            let project_name = cwd_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".to_string());
            let welcome_info = welcome::WelcomeInfo {
                model_id: model_id.clone(),
                thinking_level: format!("{:?}", settings.thinking_level).to_lowercase(),
                tool_names,
                tool_labels,
                skill_names,
                agents_md_path,
                session_type: "new",
                git_branch: git_branch.clone(),
                project_name,
            };
            state.chat.add_message(oxi_tui::widgets::chat::ChatMessage {
                role: oxi_tui::widgets::chat::MessageRole::System,
                content_blocks: vec![oxi_tui::widgets::chat::ContentBlock::Dashboard {
                    info: welcome::build_dashboard_info(&welcome_info),
                }],
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            });
        }

        // Check if model is configured
        let has_model = !model_id.is_empty() && model_id.contains('/');
        if !has_model {
            let auth = oxi_store::auth_storage::shared_auth_storage();
            let providers: Vec<(String, bool)> = oxi_ai::register_builtins::get_builtin_providers()
                .iter()
                .map(|builtin| {
                    (
                        builtin.name.to_string(),
                        auth.get_api_key(builtin.name).is_some(),
                    )
                })
                .collect();
            state.overlay = Some(AppOverlay::Setup(SetupStep::SelectProvider {
                providers,
                selected: 0,
            }));
        }

        // ── Inner TUI loop ──
        let mut running = true;
        let mut last_spinner_tick = std::time::Instant::now();
        let session_start = std::time::Instant::now();
        let poll_timeout = std::time::Duration::from_millis(50);

        while running {
            let now = std::time::Instant::now();
            if now.duration_since(last_spinner_tick).as_millis() >= 80 {
                state.spinner_frame = (state.spinner_frame + 1) % SPINNER.len();
                state.chat.spinner_frame = state.spinner_frame;
                last_spinner_tick = now;
            }

            // Update session duration in footer
            state.footer_state.data.session_duration_secs = session_start.elapsed().as_secs();

            tui.draw(|f| render::draw(f, &mut state, &theme))?;
        tracing::warn!("[TUI] draw() completed");
            tracing::warn!("[TUI] running = {}", running);

            if event::poll(poll_timeout)? {
                tracing::warn!("[TUI] event::poll returned true");
                tracing::warn!("[TUI] running = {} before read()", running);
                if let Some(action) = handlers::handle_input(
                    event::read()?,
                    &mut state,
                    &agent_session,
                    &ui_tx,
                    &prompt_tx,
                    &mut running,
                )
                .await
                {
                    match action {
                        handlers::Action::SendPrompt(value) => {
                            tracing::info!(
                                "[TUI] SendPrompt action triggered: {:?}",
                                &value[..value
                                    .char_indices()
                                    .take(50)
                                    .last()
                                    .map(|(i, c)| i + c.len_utf8())
                                    .unwrap_or(0)]
                            );
                            state.add_user_message(value.clone());
                            state.input_history.insert(0, value.clone());
                            if state.input_history.len() > 100 {
                                state.input_history.remove(0);
                            }
                            state.history_index = 0;
                            state.start_streaming();
                            agent_session.reset_should_stop();
                            tracing::info!("[TUI] About to send prompt to channel");
                            let _ = prompt_tx.send(value).await;
                            tracing::info!("[TUI] Prompt sent to channel");
                            state.input_clear();
                        }
                        handlers::Action::ExecuteSlashCommand(cmd) => {
                            slash::handle_slash_command(
                                &cmd,
                                &agent_session,
                                &mut state,
                                &mut running,
                                &ui_tx,
                            );
                        }
                    }
                }
            }

            // Check for pending questionnaire from bridge (agent thread → TUI thread)
            if state.overlay.is_none() && state.overlay_state.is_none() {
                if let Some(bridge) = &state.questionnaire_bridge {
                    if let Some(pending) = bridge.try_take() {
                        use super::overlay::questionnaire::QuestionnaireOverlay;
                        state.overlay_state = Some(Box::new(QuestionnaireOverlay::new(
                            pending.questions,
                            pending.responder,
                        )));
                        tracing::info!("[TUI] Questionnaire overlay opened");
                    }
                }
            }

            // Check if session switch was requested by a slash command
            if state.next_action.is_some() {
                running = false;
            }

            while let Ok(ui_event) = ui_rx.try_recv() {
                handlers::handle_ui_event(ui_event, &mut state);
                // Persist session after message_end events (pi-mono: persist on every message_end)
                if state.needs_persist {
                    agent_session.persist();
                    state.needs_persist = false;
                }
            }
            while let Ok(session_event) = session_event_rx.try_recv() {
                handlers::handle_session_event(session_event, &ui_tx).await;
            }

            let chat_visible_height = {
                let size = tui.size()?;
                size.height.saturating_sub(5)
            };
            state.ensure_auto_scroll(chat_visible_height);
        }

        // ── Cleanup this iteration ──
        let next_action = state.next_action.take();

        tracing::warn!("[TUI] Exiting loop, next_action = {:?}", next_action);

        // Signal the agent to stop before dropping the prompt channel.
        // This ensures the agent loop exits at the next turn boundary
        // instead of continuing to process.
        agent_session
            .should_stop_flag()
            .store(true, Ordering::SeqCst);
        agent_session.clear_queue();

        drop(prompt_tx);
        let _ = agent_handle.join();

        match next_action {
            Some(TuiNextAction::SwitchSession(path)) => {
                tracing::info!("Switching to session: {}", path);
                session_target = Some(path);
                continue;
            }
            Some(TuiNextAction::NewSession) => {
                tracing::info!("Starting new session");
                // Reload settings so the new session picks up any config changes
                if let Ok(fresh) = oxi_store::settings::Settings::load() {
                    if let Some(m) = fresh.effective_model(None) {
                        if !m.is_empty() {
                            // effective_model may already include provider
                            model_id = if m.contains('/') {
                                m
                            } else {
                                let p = fresh.effective_provider(None).unwrap_or_default();
                                format!("{}/{}", p, m)
                            };
                        }
                    }
                }
                session_target = None;
                continue;
            }
            None => break,
        }
    }

    tui.exit()?;
    Ok(())
}
