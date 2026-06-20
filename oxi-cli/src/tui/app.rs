//! Main TUI event loop and application state.

use super::handlers;
use super::render;
use super::slash;
use super::welcome;
use crate::app::agent_session::{AgentSession, SessionEvent};
use crate::app::agent_session_runtime::{
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
    create_agent_session_from_services, create_agent_session_services,
};
use crate::context::auto_compaction::CompactionReason;
use crate::store::session::SessionManager;
use anyhow::Result;
use oxi_agent::AgentEvent;
use oxi_tui::theme::Theme;
use oxi_tui::widgets::{
    chat::{ChatMessage, ChatViewState, ContentBlock, MessageRole},
    footer::FooterState,
    input::InputState,
};
use oxi_agent::tools::{TodoStateProvider, todo::{TodoPhase, TodoStatus}};
use oxi_tui::widgets::todo_panel::{TodoPanelItem, TodoPanelPhase, TodoPanelStatus};
use std::io::{self, Write};
use std::panic;
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};
use tokio::sync::mpsc;

use crossterm::{
    cursor::Hide,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use oxi_tui::render::DiffBackend;
use ratatui::{Terminal, backend::CrosstermBackend};

// ── Terminal Lifecycle ───────────────────────────────────────────────────

/// Terminal wrapper following ratatui best practices.
/// Encapsulates setup/teardown, panic hook, and mouse tracking.
struct Tui {
    terminal: Terminal<DiffBackend<io::Stdout>>,
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

        let backend = DiffBackend::new(CrosstermBackend::new(stdout));
        let mut terminal = Terminal::new(backend)?;
        if tty_ok {
            let _ = terminal.clear();
        }

        Ok(Self { terminal, tty_ok })
    }

    fn exit(&mut self) -> Result<()> {
        if self.tty_ok {
            // Each cleanup step is independent — errors in earlier steps
            // must NOT prevent later steps from running. Without this,
            // disable_raw_mode() could be skipped if an execute!() fails,
            // leaving the terminal in raw mode (no echo, no line editing).
            let _ = io::stdout().write_all(b"\x1b[?1000l\x1b[?1006l");
            let _ = io::stdout().flush();
            let _ = execute!(
                self.terminal.backend_mut(),
                PopKeyboardEnhancementFlags,
                DisableBracketedPaste
            );
            let _ = self.terminal.show_cursor();
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            // disable_raw_mode is the most critical — always attempt it.
            disable_raw_mode()?;
            // Mark as cleaned up so Drop doesn't re-enter this path.
            self.tty_ok = false;
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
    type Target = Terminal<DiffBackend<io::Stdout>>;
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
        message: oxi_sdk::Message,
    },
    /// Full message snapshot with current content blocks. pi-mono: message_update.
    /// Content blocks are already separated (text vs toolCall) by the provider.
    MessageUpdate {
        message: oxi_sdk::Message,
        delta: Option<String>,
    },
    /// Message streaming is complete. pi-mono: message_end.
    MessageEnd {
        message: oxi_sdk::Message,
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
        result: oxi_sdk::ToolResult,
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
        /// Snapshot of current steering queue messages
        messages: Vec<String>,
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

// NOTE: ProviderInfo and SetupStep have been removed.
// Provider selection now uses `overlay::provider_select::ProviderSelectOverlay`
// (a component-based overlay). Model selection uses
// `overlay::model_select_inline::ModelSelectInlineOverlay`.
// Both are `Box<dyn OverlayComponent>` stored in `overlay_state`.

/// Overlay types for interactive TUI dialogs.
///
/// NOTE: All overlay variants have been migrated to component-based
/// overlays (`Box<dyn OverlayComponent>` in `overlay_state`). This enum
/// is kept only as a sentinel — the `overlay` field should always be `None`
/// now. New overlays should implement [`super::overlay::OverlayComponent`]
/// and be stored in `overlay_state`.
#[derive(Debug, Clone)]
pub(crate) enum AppOverlay {}

// ── Session Switch Action ──────────────────────────────────────────────

/// Action requested by a slash command or overlay to switch sessions.
#[derive(Debug, Clone)]
pub(crate) enum TuiNextAction {
    /// Switch to an existing session file.
    SwitchSession(String),
    /// Start a fresh session.
    NewSession,
    /// Navigate to a specific entry within the current session (branch switch).
    GotoEntry(String),
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
    /// Central slash command registry (new layer; replaces the legacy match as
    /// commands migrate). Dispatch/completion read from here.
    #[allow(dead_code)]
    pub slash_registry: slash::registry::SlashRegistry,
    pub message_count: usize,
    /// Active overlay (None = normal chat mode)
    pub overlay: Option<AppOverlay>,
    /// Component-based overlay (takes priority over AppOverlay variants for
    /// ModelSelect, LogoutSelect, ResumeSelect). Migrated from AppOverlay.
    pub overlay_state: Option<Box<dyn super::overlay::OverlayComponent>>,
    /// Keybinding manager — maps keys to actions.
    pub keybindings: oxi_tui::keybindings::KeybindingsManager,
    /// File path completion manager
    pub completion_manager: crate::tui::completion::CompletionManager,
    /// General completion items (file paths, fuzzy search)
    pub file_completions: Vec<crate::tui::completion::CompletionItem>,
    /// Selected index in file completions
    pub file_completion_index: usize,
    /// Whether file completion popup is active
    pub file_completion_active: bool,
    /// WASM extension manager for dynamic commands
    pub wasm_ext: Option<std::sync::Arc<crate::extensions::WasmExtensionManager>>,
    /// Skill manager for /skill command
    pub skills: std::sync::Arc<parking_lot::RwLock<crate::skills::SkillManager>>,
    /// Currently active skill names
    pub active_skills: std::sync::Arc<parking_lot::RwLock<Vec<String>>>,
    /// Session file path for the current session
    pub session_file_path: Option<String>,
    /// Requested session switch action (checked by outer loop)
    pub next_action: Option<TuiNextAction>,
    /// Count of pending steering messages (shown in busy input)
    pub pending_steering: usize,
    /// Snapshot of steering queue message texts
    pub steering_messages_snapshot: Vec<String>,
    /// Whether the queue panel is visible (toggle with Ctrl+Q)
    pub queue_panel_visible: bool,
    /// Selected index in the queue panel
    pub queue_panel_selected: usize,
    /// Whether TUI chat needs to be rebuilt from agent state (after compaction)
    pub needs_chat_rebuild: bool,
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
    /// Active notifications (toast messages).
    pub notifications: Vec<Notification>,
    /// Local issue store, if one was opened. Used by the `/issue` slash
    /// command to open the issues panel overlay.
    pub issue_store: Option<crate::store::issues::FileIssueStore>,
    /// Catalog port handle for model/provider lookups without touching
    /// legacy global state. Populated from `App::oxi().catalog()` during
    /// TUI startup. `None` when the TUI is not driven by an `Oxi` engine
    /// (e.g. unit tests using `AppState::new()`).
    pub catalog: Option<std::sync::Arc<dyn oxi_sdk::ports::catalog::ModelCatalog>>,
    /// Todo state provider — shared with the agent's `todo` tool.
    /// Polled every frame to sync the sticky panel.
    pub todo_provider: Option<Arc<dyn TodoStateProvider>>,
    /// Todo panel state — synced from the agent's `todo` tool via
    /// `TodoStateProvider`. Rendered as a sticky panel above the input.
    pub todo_panel: oxi_tui::widgets::todo_panel::TodoPanelState,
}

/// A toast notification to display temporarily.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct Notification {
    /// Unique ID for this notification.
    pub id: u64,
    /// Message text to display.
    pub message: String,
    /// Notification type (affects styling).
    pub kind: NotificationKind,
    /// Timestamp when this notification was created.
    pub created_at: std::time::Instant,
    /// How long to display (if not auto-dismissed).
    pub duration: std::time::Duration,
}

impl Notification {
    /// Create a new notification.
    pub fn new(message: String, kind: NotificationKind) -> Self {
        Self {
            id: rand_u64(),
            message,
            kind,
            created_at: std::time::Instant::now(),
            duration: kind.default_duration(),
        }
    }

    /// Check if this notification should be auto-dismissed.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.duration
    }
}

/// Notification display level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotificationKind {
    /// Short status message (e.g., "Model: x"): 2 seconds
    Success,
    /// Warning message: 4 seconds
    Warning,
    /// Error message: 5 seconds (errors persist longer)
    Error,
    /// Informational message (e.g., "Cloned session"): 3 seconds
    Info,
}

impl NotificationKind {
    /// Default display duration for this kind.
    pub fn default_duration(&self) -> std::time::Duration {
        match self {
            NotificationKind::Success => std::time::Duration::from_secs(2),
            NotificationKind::Warning => std::time::Duration::from_secs(4),
            NotificationKind::Error => std::time::Duration::from_secs(5),
            NotificationKind::Info => std::time::Duration::from_secs(3),
        }
    }
}

/// Convert agent `TodoPhase` list to TUI `TodoPanelPhase` list and
/// update the panel state. Called every frame from the main loop.
fn sync_todo_panel(panel: &mut oxi_tui::widgets::todo_panel::TodoPanelState, phases: &[TodoPhase]) {
    panel.set_phases(
        phases
            .iter()
            .map(|p| TodoPanelPhase {
                name: p.name.clone(),
                tasks: p
                    .tasks
                    .iter()
                    .map(|t| TodoPanelItem {
                        content: t.content.clone(),
                        status: match t.status {
                            TodoStatus::Pending => TodoPanelStatus::Pending,
                            TodoStatus::InProgress => TodoPanelStatus::InProgress,
                            TodoStatus::Completed => TodoPanelStatus::Completed,
                            TodoStatus::Abandoned => TodoPanelStatus::Abandoned,
                        },
                    })
                    .collect(),
            })
            .collect(),
    );
}

fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

impl AppState {
    pub fn new() -> Self {
        let mut state = Self {
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
            slash_registry: slash::registry::SlashRegistry::builtins(),
            message_count: 0,
            overlay: None,
            overlay_state: None,
            keybindings: oxi_tui::keybindings::KeybindingsManager::new(),
            completion_manager: crate::tui::completion::CompletionManager::new(
                std::env::current_dir().unwrap_or_default(),
            ),
            file_completions: Vec::new(),
            file_completion_index: 0,
            file_completion_active: false,
            wasm_ext: None,
            skills: std::sync::Arc::new(parking_lot::RwLock::new(
                crate::skills::SkillManager::new(),
            )),
            active_skills: std::sync::Arc::new(parking_lot::RwLock::new(Vec::new())),
            session_file_path: None,
            next_action: None,
            pending_steering: 0,
            steering_messages_snapshot: Vec::new(),
            queue_panel_visible: false,
            queue_panel_selected: 0,
            needs_chat_rebuild: false,
            snapshot_text_rendered: 0,
            snapshot_thinking_rendered: Vec::new(),
            snapshot_text_block_created: false,
            questionnaire_bridge: None,
            tool_start_times: std::collections::HashMap::new(),
            notifications: Vec::new(),
            issue_store: None,
            catalog: None,
            todo_provider: None,
            todo_panel: oxi_tui::widgets::todo_panel::TodoPanelState::new(),
        };

        // Load user keybindings from settings
        if let Ok(settings) = crate::store::settings::Settings::load()
            && !settings.keybindings.is_empty()
        {
            state.keybindings.set_user_bindings(&settings.keybindings);
        }

        state
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

    pub fn update_slash_completions(&mut self, session: &AgentSession) {
        let input_str = self.input_value();
        let text = input_str.trim();
        if !text.starts_with('/') {
            self.clear_slash_completions();
            return;
        }

        // ── Argument completion: `/cmd <prefix>` routes to the matched
        // command's `complete_arg` (aliases + extension commands included).
        if let Some(space) = text.find(' ') {
            let cmd_token = &text[..space];
            let arg_prefix = text[space + 1..].trim_start();
            // Read-only access: borrow the registry and state immutably.
            let registry = &self.slash_registry;
            let state: &AppState = self;
            let cmd_token_no_slash = cmd_token.strip_prefix('/').unwrap_or(cmd_token);
            let items = registry.complete_arg(cmd_token, arg_prefix, session, state);
            let mut matches: Vec<slash::SlashCompletion> = items
                .into_iter()
                .map(|item| slash::SlashCompletion {
                    // Argument completion: insert the full `/cmd <arg>` text.
                    name: format!("/{} {}{}", cmd_token_no_slash, item.text, " "),
                    description: item.description.unwrap_or_default(),
                    is_arg: true,
                })
                .collect();
            matches.sort_by(|a, b| a.name.cmp(&b.name));
            self.slash_completions = matches;
            self.slash_completion_index = 0;
            self.slash_completion_active = !self.slash_completions.is_empty();
            return;
        }

        let cmd_part = text.split_whitespace().next().unwrap_or("");
        let query = if cmd_part.len() > 1 {
            &cmd_part[1..]
        } else {
            ""
        };
        let mut matches: Vec<slash::SlashCompletion> = self
            .slash_registry
            .complete_command(query)
            .into_iter()
            .map(|e| slash::SlashCompletion {
                name: e.display,
                description: e.description,
                is_arg: false,
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

    /// Add a notification (toast message) instead of a chat message.
    pub fn add_notification(&mut self, message: String, kind: NotificationKind) {
        self.notifications.push(Notification::new(message, kind));
    }

    /// Remove expired notifications.
    pub fn cleanup_notifications(&mut self) {
        self.notifications.retain(|n| !n.is_expired());
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
    pub fn update_streaming_message(&mut self, msg: &oxi_sdk::Message, delta: Option<&str>) {
        if let oxi_sdk::Message::Assistant(assistant) = msg {
            let mut thinking_block_idx: usize = 0;
            for block in &assistant.content {
                match block {
                    oxi_sdk::ContentBlock::Text(t) => {
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
                        } else if text.is_empty() {
                            // Fallback: provider did not accumulate text into
                            // the partial snapshot — use the raw delta instead.
                            // This guards against providers that emit TextDelta
                            // without updating partial_message.content.
                            if let Some(delta_str) = delta
                                && !delta_str.is_empty()
                            {
                                self.chat.stream_text_delta(delta_str);
                                // Keep snapshot_text_rendered at 0 so future
                                // updates also use the delta fallback.
                            }
                        }
                    }
                    oxi_sdk::ContentBlock::ToolCall(tc) => {
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
                    oxi_sdk::ContentBlock::Thinking(t) => {
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
                    oxi_sdk::ContentBlock::Image(img) => {
                        self.chat
                            .stream_image(img.mime_type.clone(), img.data.clone());
                    }
                    oxi_sdk::ContentBlock::Unknown(_) => {}
                }
            }
        }
    }

    /// Finalize the streaming message from a MessageEnd snapshot.
    pub fn finalize_streaming_message(&mut self, msg: &oxi_sdk::Message) {
        if let oxi_sdk::Message::Assistant(assistant) = msg {
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
    // ── Liveness lock is held by `App` itself (see App::from_oxi) ──────
    // `App::ownership_session_id` equals [`liveness::TUI_OWNERSHIP_ID`] in
    // TUI mode, so `is_session_alive` checks made from the agent tool,
    // the issues panel, and `/issue` slash commands all see this TUI
    // process as a single coherent owner. The lock is released when `App`
    // is dropped at the end of this function (kernel closes the fd →
    // flock released → process exit, including `kill -9`).
    debug_assert_eq!(
        app.ownership_session_id(),
        crate::store::issues::liveness::TUI_OWNERSHIP_ID
    );

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
        crate::store::session::find_recent_session_path(&cwd)
    } else {
        None
    };

    // ── Install SIGINT safety net ──
    // Raw mode should capture Ctrl+C as a key event, but if it doesn't
    // (e.g. child process modifies terminal state), we need a fallback.
    // This sets a flag checked by the TUI loop.
    let sigint_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    {
        let flag = sigint_flag.clone();
        let _sigint_guard = tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            flag.store(true, Ordering::SeqCst);
            tracing::warn!("[TUI] SIGINT received (bypassed raw mode), setting exit flag");
        });
    }

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
            })
            .await?;

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
        // NOTE: prompt_tx_worker was previously used for auto-processing queued
        // messages after agent completion. That logic now lives in the TUI main loop
        // (see `saw_agent_end` check). The clone is kept for potential future use.
        let ui_tx_for_thread = ui_tx.clone();
        let _agent_handle = std::thread::spawn(move || {
            let rt = get_agent_runtime();
            rt.block_on(async {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async {
                        while let Some(prompt) = prompt_rx.recv().await {
                            tracing::debug!(
                                "[TUI] Worker: received prompt, calling agent.run_with_channel"
                            );
                            tracing::info!("[TUI] Received prompt, starting agent run");

                            // Refresh API key from auth storage before each run.
                            // This ensures a key entered via the provider overlay
                            // mid-session is picked up without restarting.
                            if let Err(e) = session_handle.refresh_api_key() {
                                tracing::warn!("[TUI] Failed to refresh API key: {}", e);
                            }

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
                                            ..
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
                                            result: oxi_sdk::ToolResult {
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
                                            let sq = steering_q.read();
                                            let fq = follow_up_q.read();
                                            let pending = sq.len() + fq.len();
                                            let mut msgs: Vec<String> =
                                                sq.iter().cloned().collect();
                                            msgs.extend(fq.iter().cloned());
                                            drop(sq);
                                            drop(fq);
                                            let _ = ui_fwd.send(UiEvent::QueueUpdate {
                                                pending,
                                                messages: msgs,
                                            });
                                            continue;
                                        }
                                        AgentEvent::FollowUpMessage { .. } => {
                                            let steering_q = session_h.steering_queue();
                                            let follow_up_q = session_h.follow_up_queue();
                                            let sq = steering_q.read();
                                            let fq = follow_up_q.read();
                                            let pending = sq.len() + fq.len();
                                            let mut msgs: Vec<String> =
                                                sq.iter().cloned().collect();
                                            msgs.extend(fq.iter().cloned());
                                            drop(sq);
                                            drop(fq);
                                            let _ = ui_fwd.send(UiEvent::QueueUpdate {
                                                pending,
                                                messages: msgs,
                                            });
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
                            let should_stop_flag = sh.should_stop_flag();
                            let hooks = oxi_agent::AgentHooks {
                                should_stop_after_turn: Some(Arc::new(move |_ctx| {
                                    should_stop_flag.load(Ordering::SeqCst)
                                })),
                                // NOTE: get_steering_messages and get_follow_up_messages
                                // are intentionally NOT set. User-queued messages should
                                // only be processed AFTER the current run completes,
                                // not injected mid-loop. The TUI main loop handles
                                // post-completion queue processing (see `saw_agent_end`).
                                tool_execution: oxi_agent::ToolExecutionMode::Sequential,
                                ..Default::default()
                            };
                            agent.set_hooks(hooks);
                            let agent_clone = Arc::clone(&agent);
                            tracing::info!("[AGENT-WORKER] Spawning agent task");

                            // Mark AgentSession streaming flag so is_streaming() is accurate
                            // for any code that checks it (extensions, RPC, etc.).
                            let session_handle2 = session_handle.clone_handle();
                            let _sh_for_auto = session_handle.clone_handle();

                            tokio::task::spawn_local(async move {
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
                                // Clear streaming flag now that the agent run is complete.
                                session_handle2
                                    .streaming_flag()
                                    .store(false, Ordering::SeqCst);
                            });
                            // NOTE: Do NOT await the spawned task.
                            // If we await here, the outer thread blocks on the tokio runtime
                            // until the agent completes. If the agent is mid-turn (LLM request
                            // in flight, tool executing), this blocks forever. By not awaiting,
                            // the tokio runtime thread is free to recv the next prompt from
                            // prompt_rx. When the LocalSet is dropped (outer thread exits),
                            // the spawned task is aborted and the event_tx channel is dropped,
                            // causing the forwarder thread to exit.

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

                            // NOTE: Auto-processing of queued messages after agent
                            // completion is handled by the TUI main loop (see
                            // `saw_agent_end` check below). We cannot do it here
                            // because spawn_local is non-awaited, so this code runs
                            // immediately after the agent *starts*, not when it
                            // *finishes*.
                        }
                    })
                    .await;
            });
        });

        // ── Create state ──
        let mut state = AppState::new();
        state.session_file_path = session_target.clone();
        // Share the local issue store with the TUI so `/issue` can open the
        // overlay. The store is opened read-write by `App::from_oxi`; cloning
        // is cheap (inner Arc).
        state.issue_store = app.issue_store();
        // Inject the catalog port so TUI overlays/slash commands can query
        // models without going through legacy global state.
        state.catalog = Some(std::sync::Arc::clone(app.oxi().catalog()));
        // Inject the todo state provider so the sticky panel syncs from
        // the same `Arc<RwLock<Vec<TodoPhase>>>` the agent's `todo` tool
        // mutates.
        state.todo_provider = agent_session
            .agent_ref()
            .get_config()
            .todo
            .clone();

        // Restore previous messages if resuming
        if is_resuming && let Some(ref path) = session_target {
            let sm = crate::store::session::SessionManager::open(path, None, Some(&cwd));
            let branch = sm.get_branch(None);
            for entry in &branch {
                match &entry.message {
                    crate::store::session::AgentMessage::User { content } => {
                        state.add_user_message(content.as_str().to_string());
                    }
                    crate::store::session::AgentMessage::Assistant { content, .. } => {
                        let text: String = content
                            .iter()
                            .filter_map(|b| match b {
                                crate::store::session::AssistantContentBlock::Text { text } => {
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

        // Share skill manager and active skills from App
        {
            let app_skills = app.skills();
            *state.skills.write() = app_skills.clone();
        }
        *state.active_skills.write() = app.active_skills();
        state.questionnaire_bridge = questionnaire_bridge.clone();
        if let Some(ref bridge) = questionnaire_bridge {
            bridge.attach();
        }

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

        // Check if model is configured and resolvable
        let model_resolved = oxi_agent::model_id::resolve_model_from_id(&model_id).is_some();
        let has_model = !model_id.is_empty() && model_id.contains('/') && model_resolved;
        if !has_model {
            // Initial setup: open the provider-select overlay as a centered popup
            // (not a full-screen wizard). The user picks a provider, enters an
            // API key inline, and is then transitioned to the model selector
            // — all without leaving the TUI.
            let provider_entries =
                super::overlay::provider_select::build_provider_entries_with_catalog(
                    state.catalog.as_ref(),
                );
            state.overlay_state = Some(Box::new(
                super::overlay::provider_select::ProviderSelectOverlay::new(
                    provider_entries,
                    true, // is_initial_setup
                ),
            ));
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

            // Clean up expired notifications
            state.cleanup_notifications();

            // Poll overlay for self-initiated actions (timeout auto-submit, etc.)
            if let Some(ref mut overlay) = state.overlay_state {
                if matches!(overlay.poll(), super::overlay::OverlayAction::Close) {
                    state.overlay_state = None;
                }
            }

            // Sync todo panel from agent state (cheap: RwLock read + clone)
            if let Some(ref provider) = state.todo_provider {
                sync_todo_panel(&mut state.todo_panel, &provider.get_phases());
            }

            tui.draw(|f| render::draw(f, &mut state, &theme))?;

            // Post-render: display images via terminal protocol
            // After ratatui draws, output inline images that won't be overwritten
            if !state.chat.pending_images.is_empty() {
                let caps = oxi_tui::render::terminal::TerminalCapabilities::detect();
                if let Some(protocol) = caps.image_protocol {
                    use base64::Engine;
                    use oxi_tui::render::image::{
                        ImageOptions, detect_dimensions, encode_iterm2, encode_kitty,
                    };

                    // Display only the latest image to avoid flooding the terminal
                    if let Some((b64_data, _mime_type)) = state.chat.pending_images.last()
                        && let Ok(bytes) =
                            base64::engine::general_purpose::STANDARD.decode(b64_data)
                    {
                        let dims = detect_dimensions(&bytes);
                        let opts = ImageOptions {
                            width_cells: dims.map(|d| (d.width / 10).min(40) as u16),
                            height_cells: dims.map(|d| (d.height / 20).min(15) as u16),
                            ..Default::default()
                        };

                        let encoded = match protocol {
                            oxi_tui::render::terminal::ImageProtocol::Kitty => {
                                encode_kitty(b64_data, &opts)
                            }
                            oxi_tui::render::terminal::ImageProtocol::ITerm2 => {
                                encode_iterm2(b64_data, &opts)
                            }
                        };

                        // Write directly to stdout — this bypasses ratatui's buffer
                        let _ = std::io::stdout().write_all(encoded.as_bytes());
                        let _ = std::io::stdout().flush();
                    }
                }
            }

            if event::poll(poll_timeout)? {
                let ev = event::read()?;
                tracing::info!("[TUI] Event: {:?}", ev);
                if let Some(action) = handlers::handle_input(
                    ev,
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
                            // Persist the user message to the session manager before
                            // the agent loop starts so that the deferred-flush pattern
                            // (which writes to disk on the first assistant message)
                            // includes the user message in the output file.
                            agent_session.persist_user_message(value.clone());
                            tracing::info!("[TUI] About to send prompt to channel");
                            tracing::debug!("[TUI] SendPrompt: prompt_tx.send() called");
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
            if state.overlay.is_none()
                && state.overlay_state.is_none()
                && let Some(bridge) = &state.questionnaire_bridge
                && let Some(pending) = bridge.try_take()
            {
                use super::overlay::questionnaire::QuestionnaireOverlay;
                state.overlay_state = Some(Box::new(QuestionnaireOverlay::new(
                    pending.questions,
                    pending.responder,
                    pending.timeout,
                )));
                tracing::info!("[TUI] Questionnaire overlay opened");
            }

            if state.next_action.is_some() {
                tracing::debug!("[TUI] Loop: next_action set, breaking");
                break;
            }

            if !running || sigint_flag.load(Ordering::SeqCst) {
                tracing::debug!("[TUI] Loop: running=false, breaking");
                break;
            }

            let mut saw_agent_end = false;
            while let Ok(ui_event) = ui_rx.try_recv() {
                if matches!(ui_event, UiEvent::AgentEnd) {
                    saw_agent_end = true;
                }
                handlers::handle_ui_event(ui_event, &mut state, &agent_session);
                // Rebuild chat from agent state after compaction
                if state.needs_chat_rebuild {
                    rebuild_chat(&mut state, &agent_session);
                    state.needs_chat_rebuild = false;
                }
            }

            // After AgentEnd, check for queued steering/follow-up messages.
            // The worker thread's auto-process only runs immediately after spawn_local
            // (before the agent finishes), so messages queued during the run that
            // weren't consumed by poll_external_queues() are left stranded.
            // Feed the first one back through prompt_tx to re-enter the worker loop.
            if saw_agent_end && !state.is_agent_busy {
                let first_pending: Option<String> = {
                    let sq = agent_session.steering_queue();
                    let fq = agent_session.follow_up_queue();
                    let mut sq_guard = sq.write();
                    let mut fq_guard = fq.write();
                    sq_guard.pop_front().or_else(|| fq_guard.pop_front())
                };
                if let Some(msg) = first_pending {
                    let remaining = agent_session.pending_message_count();
                    tracing::info!(
                        "[TUI] Post-AgentEnd auto-processing queued message ({} remaining)",
                        remaining
                    );
                    let _ = ui_tx.send(UiEvent::QueueUpdate {
                        pending: remaining,
                        messages: agent_session
                            .steering_messages()
                            .into_iter()
                            .chain(agent_session.follow_up_messages())
                            .collect(),
                    });
                    let _ = ui_tx.send(UiEvent::AutoProcessStart {
                        prompt: msg.clone(),
                    });
                    state.is_agent_busy = true;
                    // Persist the queued message to the session manager before
                    // sending it through the prompt channel, same as SendPrompt.
                    agent_session.persist_user_message(msg.clone());
                    let _ = prompt_tx.send(msg).await;
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

        // Signal the agent to stop before dropping the prompt channel.
        tracing::debug!("[TUI] Cleanup: setting should_stop_flag");
        agent_session
            .should_stop_flag()
            .store(true, Ordering::SeqCst);
        // Cancel the agent's active stream so it stops waiting for LLM tokens.
        // This unblocks the worker thread's spawned task, allowing the
        // LocalSet to complete and the thread to exit.
        tracing::debug!("[TUI] Cleanup: cancelling agent");
        agent_session.agent_ref().cancel();
        agent_session.abort_compaction_sync();
        tracing::debug!("[TUI] Cleanup: clearing queue");
        agent_session.clear_queue();

        // Remove session file if no real conversation happened
        agent_session.cleanup_empty_session();

        tracing::debug!("[TUI] Cleanup: dropping prompt_tx");
        drop(prompt_tx);
        tracing::debug!("[TUI] Cleanup: prompt_tx dropped");

        match next_action {
            Some(TuiNextAction::SwitchSession(path)) => {
                tracing::info!("Switching to session: {}", path);
                session_target = Some(path);
                continue;
            }
            Some(TuiNextAction::NewSession) => {
                tracing::info!("Starting new session");
                // Reload settings so the new session picks up any config changes
                if let Ok(fresh) = crate::store::settings::Settings::load()
                    && let Some(m) = fresh.effective_model(None)
                    && !m.is_empty()
                {
                    // effective_model may already include provider
                    model_id = if m.contains('/') {
                        m
                    } else {
                        let p = fresh.effective_provider(None).unwrap_or_default();
                        format!("{}/{}", p, m)
                    };
                }
                session_target = None;
                continue;
            }
            Some(TuiNextAction::GotoEntry(entry_id)) => {
                tracing::info!("Navigating to entry: {}", entry_id);
                let Some(path) = session_target.as_ref() else {
                    tracing::warn!("GotoEntry: no session file path");
                    continue;
                };
                let sm = crate::store::session::SessionManager::open(path, None, Some(&cwd));
                sm.set_leaf_from_entry(&entry_id)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                // Reload messages from the new leaf position
                state.chat.clear();
                let branch = sm.get_branch(Some(&entry_id));
                for entry in &branch {
                    match &entry.message {
                        crate::store::session::AgentMessage::User { content } => {
                            state.add_user_message(content.as_str().to_string());
                        }
                        crate::store::session::AgentMessage::Assistant { content, .. } => {
                            let text: String = content
                                .iter()
                                .filter_map(|b| match b {
                                    crate::store::session::AssistantContentBlock::Text { text } => {
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
                state.add_notification(
                    format!("Jumped to entry {}", &entry_id[..8.min(entry_id.len())]),
                    NotificationKind::Success,
                );
                continue;
            }
            None => break,
        }
    }

    tracing::debug!("[TUI] About to call tui.exit()");
    if let Err(e) = tui.exit() {
        tracing::error!("[TUI] tui.exit() failed: {:?}", e);
    }
    tracing::debug!("[TUI] tui.exit() done, exiting process");

    // Force process exit to ensure background threads (agent worker, forwarder)
    // don't keep the process alive. The terminal is already restored by
    // tui.exit() above, and all critical cleanup is done.
    std::process::exit(0);
}

/// Rebuild the TUI chat view from the current agent state.
/// Called after compaction completes — the agent's message list has been replaced
/// with the compacted subset, so the TUI must reflect that.
fn rebuild_chat(state: &mut AppState, session: &crate::app::agent_session::AgentSession) {
    let agent_state = session.agent_ref().state();
    let messages = &agent_state.messages;

    state.chat.clear();
    state.message_count = 0;

    for msg in messages {
        match msg {
            oxi_sdk::Message::User(u) => {
                let content = match &u.content {
                    oxi_sdk::MessageContent::Text(t) => t.clone(),
                    oxi_sdk::MessageContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<Vec<_>>()
                        .join(""),
                };
                state.chat.add_message(ChatMessage {
                    role: MessageRole::User,
                    content_blocks: vec![ContentBlock::Text { content }],
                    timestamp: now_millis(),
                });
                state.message_count += 1;
            }
            oxi_sdk::Message::Assistant(a) => {
                let mut blocks = Vec::new();
                for cb in &a.content {
                    match cb {
                        oxi_sdk::ContentBlock::Text(t) => {
                            blocks.push(ContentBlock::Text {
                                content: t.text.clone(),
                            });
                        }
                        oxi_sdk::ContentBlock::Thinking(_t) => {
                            // Skip thinking blocks in rebuilt chat
                        }
                        oxi_sdk::ContentBlock::ToolCall(tc) => {
                            blocks.push(ContentBlock::ToolCall {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                arguments: tc.arguments.to_string(),
                                result: None,
                                status: oxi_tui::widgets::chat::ToolCallStatus::Done,
                                duration: None,
                            });
                        }
                        _ => {}
                    }
                }
                state.chat.add_message(ChatMessage {
                    role: MessageRole::Assistant,
                    content_blocks: blocks,
                    timestamp: now_millis(),
                });
            }
            oxi_sdk::Message::ToolResult(t) => {
                let content = t
                    .content
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect::<Vec<_>>()
                    .join("");
                state.chat.add_message(ChatMessage {
                    role: MessageRole::System,
                    content_blocks: vec![ContentBlock::ToolResult {
                        tool_name: t.tool_call_id.clone(),
                        content,
                        is_error: false,
                    }],
                    timestamp: now_millis(),
                });
            }
        }
    }

    // Add a compaction summary marker
    state.chat.add_message(ChatMessage {
        role: MessageRole::System,
        content_blocks: vec![ContentBlock::Text {
            content: "Context compacted — earlier messages summarized".to_string(),
        }],
        timestamp: now_millis(),
    });
}
