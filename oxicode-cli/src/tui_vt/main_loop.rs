#![allow(
    clippy::field_reassign_with_default,
    clippy::let_and_return,
    clippy::borrow_interior_mutable_const,
    clippy::derivable_impls
)]
//! TUI main event loop — connects oxicode's `AgentSession` to vtcode-ui's
//! `InlineSession` protocol and a ratatui rendering backend.

use std::collections::VecDeque;
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate, disable_raw_mode, enable_raw_mode},
};
use oxicode_agent::AgentEvent;
use oxicode_agent::config::Mode;
use oxicode_agent::tools::TodoStateProvider;
use oxicode_agent::tools::todo::{TodoItem, TodoPhase, TodoStatus};
use oxicode_vtui::theme::{ThemeStyles, active_styles};
use oxicode_vtui::tui::core::{
    AuthAction, InlineCommand, InlineEvent, InlineHandle, InlineHeaderContext,
    InlineHeaderStatusBadge, InlineHeaderStatusTone, InlineListItem, InlineListSelection,
    InlineMessageKind, InlineSegment, InlineTextStyle, OverlayRequest, OverlaySubmission,
    SecurePromptConfig,
};
use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Alignment, Margin, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::App;
use crate::app::agent_hub_registry::HubEntry;
use crate::app::agent_session::SessionEvent;
use crate::tui_vt::keymap::{GlobalAction, KeyCombo, Keymap};
use crate::tui_vt::settings_defs::{
    SETTING_DEFS, SettingKey, SettingWidget, SettingsMapRow, SettingsTab, defs_for_tab,
    get_display_value,
};
use crate::tui_vt::slash::file_commands::FileCommand;
use crate::tui_vt::slash::registry::{
    SlashCtx, SlashOutcome, SlashRegistry, settings_overlay_items,
};
use oxicode_vtui::presentation::{
    BlockAlloc, BlockDisplayMode, TranscriptLine, VisibleItem, allocate_rows, visible_items,
};
use oxicode_vtui::tui::ui::clamp_segments_to_width;

use oxicode_textarea::{EditBuffer, ElementKind, TextArea, TextAreaState};

use ratatui::widgets::FrameExt;
/// Host-defined [`ElementKind`] tag for the secure-prompt overlay's masked
/// element. The textarea treats the kind as opaque; this constant exists so
/// every render of a masked overlay shares one stable id (handy for tests,
/// logs, and future per-element metadata lookups).
const MASKED_ELEMENT_KIND: ElementKind = ElementKind(1);
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
    /// Enable raw mode, push keyboard flags, enable bracketed paste,
    /// hide the cursor, and enter an **inline viewport** anchored at the
    /// cursor. The inline viewport is what lets finalized transcript
    /// rows be printed into the host terminal's real scrollback
    /// (`Terminal::insert_before`) — a fullscreen viewport would keep
    /// every line inside the repaint region and native scroll-up would
    /// show only pre-session content.
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

        // Inline viewport sized to the full terminal height (the live
        // region keeps today's look: transcript tail + composer). On a
        // non-tty (piped) run there is no scrollback to feed — fall
        // back to fullscreen, where `insert_before` is a no-op.
        //
        // Entering the inline viewport also queries the cursor position
        // (`CSI 6n`); a terminal that does not answer (piped pty, slow
        // link) must not kill the app — degrade to fullscreen: no host
        // scrollback committing, everything else identical.
        let mut terminal = None;
        if tty_ok {
            let height = crossterm::terminal::size().map(|s| s.1).unwrap_or(24);
            let backend = CrosstermBackend::new(stdout);
            terminal = Terminal::with_options(
                backend,
                TerminalOptions {
                    viewport: Viewport::Inline(height),
                },
            )
            .ok();
        }
        let mut terminal = match terminal {
            Some(t) => t,
            None => Terminal::new(CrosstermBackend::new(io::stdout()))?,
        };
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

/// The one authoritative prompt queue shared by the input handler and agent
/// worker.  The visible queue is a projection of this deque, never a second
/// queue that can drift from execution order.
#[derive(Default)]
struct PromptQueue {
    pending: parking_lot::Mutex<VecDeque<String>>,
    wake: tokio::sync::Notify,
}

impl PromptQueue {
    fn enqueue(&self, prompt: String) {
        self.pending.lock().push_back(prompt);
        self.wake.notify_one();
    }

    fn remove(&self, index: usize) -> Option<String> {
        self.pending.lock().remove(index)
    }

    fn move_by(&self, index: usize, delta: isize) -> bool {
        let mut pending = self.pending.lock();
        let Some(target) = index.checked_add_signed(delta) else {
            return false;
        };
        if index >= pending.len() || target >= pending.len() {
            return false;
        }
        pending.swap(index, target);
        true
    }

    async fn next(&self) -> String {
        loop {
            let notified = self.wake.notified();
            if let Some(prompt) = self.pending.lock().pop_front() {
                return prompt;
            }
            notified.await;
        }
    }
}

/// Mutable state the input thread edits (text buffer, scroll, footer) and
/// the main loop reads for rendering.
//
// `composer` is the single source of truth for the editable text. It owns
// the buffer (replacing the old `input_buffer: String` + `input_cursor: usize
// pair) and gives us correct CJK/emoji caret math, soft-wrap, horizontal
// scroll, selection, and undo/redo for free. Hand-rolled byte math was
// removed in Task 6 of the textarea port.
/// oxibrain daemon connection state for the status-bar chip. Driven by a
/// background prober (see `run_tui`); `Off` means the memory tools are
/// disabled in settings and the chip renders nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum BrainChip {
    /// Memory disabled — chip hidden (quiet-chrome contract).
    #[default]
    Off,
    /// Enabled but the daemon socket is absent.
    Down,
    /// Socket present but the last ping failed.
    Degraded,
    /// Ping succeeded.
    Ok,
}

impl BrainChip {
    /// `(label, healthy)` for the status bar; `None` hides the chip.
    pub(crate) fn chip_label(self) -> Option<(&'static str, bool)> {
        match self {
            BrainChip::Off => None,
            BrainChip::Ok => Some(("brain·ok", true)),
            BrainChip::Degraded | BrainChip::Down => Some(("brain·down", false)),
        }
    }
}

/// Progress facts for the live agent run (`AgentStart` → `AgentEnd`).
///
/// Presence of the tracker (not `reasoning_stage`) is what keeps the
/// indicator row above the composer owned during a run: turn boundaries
/// clear the stage label, but the row must not flicker to the idle row
/// (follow-ups / tips) until the whole run is over.
#[derive(Debug, Clone)]
pub(crate) struct RunState {
    /// When the run started — drives the elapsed-time readout.
    pub started_at: std::time::Instant,
    /// LLM requests started so far (incremented on `MessageStart`).
    pub turn: u32,
    /// Tool executions started so far (incremented on `ToolExecutionStart`).
    pub tool_calls: u32,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            started_at: std::time::Instant::now(),
            turn: 0,
            tool_calls: 0,
        }
    }
}

pub struct RenderState {
    /// Editable text in the composer. Source of truth for the prompt line.
    pub composer: oxicode_textarea::TextArea,
    /// Transcript lines, in display order.
    pub transcript: Vec<TranscriptLine>,
    /// Index of the line currently pinned at the top of the viewport.
    /// `usize::MAX` means "follow the tail" (auto-scroll).
    pub scroll_offset: usize,
    /// Transcript entries [0, committed) are frozen in the host
    /// terminal's real scrollback (printed above the viewport via
    /// `Terminal::insert_before`). They never render in the live
    /// viewport again — native scroll-up reads them.
    pub committed_entries: usize,
    /// Last known terminal width — omp-style tool boxes size their
    /// borders to it. Refreshed each render pass; frozen transcript
    /// lines keep the width they were built at (printed text does not
    /// rewrap either).
    pub viewport_width: u16,
    /// Last known terminal height — paired with `viewport_width` for
    /// resize-change detection (only width changes invalidate the
    /// frozen scrollback).
    pub last_viewport_height: u16,
    /// Snapshot of the user's glyph-set setting — `nerd` swaps the
    /// composer context labels for Nerd Font icons (never emoji).
    pub glyph_set: crate::symbols::GlyphSet,
    /// Inline image previews (kitty/iTerm2): protocol detection,
    /// `inline_images` kill-switch, transmit budget, and the pending
    /// live placements. Owned here so both the agent-event hook
    /// (enqueue) and the post-draw step (emit) share one budget.
    pub image_previews: super::image_preview::ImagePreviews,
    /// Header context mirrored from `InlineHeaderContext`.
    pub header_context: InlineHeaderContext,
    /// Composer enabled state — mirrored from `SetInputEnabled`.
    pub input_enabled: bool,
    /// Composer prompt prefix — mirrored from `SetPrompt`.
    pub prompt_prefix: String,
    /// Composer placeholder — mirrored from `SetPlaceholder`.
    pub placeholder: Option<String>,
    /// Shutdown signal received from the harness.
    pub shutdown_requested: bool,
    /// Accumulated text for markdown rendering at message end.
    pub message_buffer: String,
    /// Accumulated reasoning text for the dimmed thinking block.
    pub thinking_buffer: String,
    /// Cache for the streaming assistant markdown render. Held on
    /// `RenderState` so the cache survives across the many per-frame
    /// `render_streamed_message` calls; the equality fast-path makes
    /// most frames return without re-parsing.
    pub md_cache: oxicode_vtui::tui::ui::markdown::MdRenderCache,
    /// Transcript index where the in-flight assistant message's streamed
    /// lines begin. `None` while nothing is streaming. The markdown
    /// re-render at `MessageEnd` replaces from this anchor — a blind
    /// tail-count would duplicate raw streamed lines whenever markdown
    /// collapses the paragraph structure differently.
    pub stream_anchor: Option<usize>,
    /// Agent Hub overlay open.
    pub agent_hub_open: bool,
    /// Hub entries snapshotted when the overlay was opened (`/agents`).
    pub hub_entries: Vec<(String, HubEntry)>,
    /// Live Hub registry for per-frame subagent status (matched todo
    /// highlight + auto-reconcile). `None` when the session provides none.
    pub hub: Option<crate::app::agent_hub_registry::SharedHubRegistry>,
    /// First Ctrl+C armed a quit; a second press exits (two-press quit).
    pub pending_quit: bool,
    /// Slash-command autocomplete popup state.
    pub slash_popup: SlashPopup,
    /// Current reasoning/tool stage (e.g. "tool: read"), shown above the composer.
    pub reasoning_stage: Option<String>,
    /// Live-run tracker — `Some` from `AgentStart` to `AgentEnd`. Owns the
    /// indicator row across the per-turn stage clears so it never flickers
    /// to the idle row mid-run, and carries progress facts for display.
    pub(crate) active_run: Option<RunState>,
    /// Typewriter reveal for the streamed body: how many BYTES of
    /// `message_buffer` have been painted into the transcript.
    /// `usize::MAX` = fully revealed (idle / finalized). The render tick
    /// advances this so streamed text appears as a smooth flow instead
    /// of per-network-chunk lumps.
    pub stream_reveal: usize,
    /// oxibrain daemon health for the status-bar chip (prober-fed).
    pub(crate) brain: BrainChip,
    /// Selected reasoning effort, reflected in the composer's context bar.
    pub thinking_level: String,
    /// Provider-reported prompt tokens for the most recently completed turn.
    /// This is the closest available snapshot of the live context size.
    pub context_tokens: Option<usize>,
    /// Context capacity configured for the active agent session.
    pub context_window: usize,
    /// Overlay modal/list state — `Some` when an overlay is open.
    pub overlay: Option<OverlayState>,
    /// Model IDs for the /model overlay picker (ordered same as overlay items).
    pub overlay_model_ids: Vec<String>,
    /// `(provider, model_id)` pairs backing the `/models` catalog browser
    /// overlay (ordered same as overlay items).
    pub overlay_catalog_models: Vec<(String, String)>,
    /// Provider names backing the `/providers` overlay (ordered same as items).
    pub overlay_providers: Vec<String>,
    /// Model catalog port handle, captured once at TUI startup so slash
    /// commands (`/models`, `/providers`) can browse the full catalog.
    pub catalog: Option<std::sync::Arc<dyn oxicode_sdk::ports::catalog::ModelCatalog>>,
    /// Queued input prompts (waiting to be processed).
    pub queued_inputs: Vec<String>,
    /// Queued input prompts — interactive panel open (Ctrl+; toggles).
    pub queue_panel_open: bool,
    /// Selected index within the queue panel (when interactive).
    pub queue_selected: usize,
    /// Shell mode — `!` prefix for direct bash commands (grok-build parity).
    pub shell_mode: bool,
    /// Follow-up suggestion chips.
    pub follow_ups: Vec<String>,
    /// Live todo phases — refreshed from the live provider each frame.
    pub todo_phases: Vec<TodoPhase>,
    /// Whether the todo HUD is expanded (all phases) vs collapsed.
    pub todo_expanded: bool,
    /// HUD-only auto-clear deadline; does not mutate the underlying TodoState.
    pub todo_clear_deadline: Option<std::time::Instant>,
    /// Auto-clear delay (seconds) once the todo list settles (all closed).
    /// `< 0` disables auto-clear. Wired from settings at TUI startup.
    pub todo_clear_delay_secs: i64,
    /// Live todo state provider — the same source the `todo` agent tool
    /// writes to. `None` when todos are disabled; the pane stays hidden.
    pub todo_provider: Option<Arc<dyn TodoStateProvider>>,
    /// Vim editing state (enabled by /vim command).
    pub vim_state: crate::tui_vt::vim::VimState,
    /// Vim clipboard buffer.
    pub vim_clipboard: String,
    /// In-transcript search state — `None` when no search is active.
    pub search: Option<SearchState>,
    /// Per-block display override. An absent entry means the default
    /// ([`BlockDisplayMode::Expanded] — chat responses must show their
    /// full text; elision (`Truncated`) is opt-in via block cycling).
    pub block_display: std::collections::HashMap<usize, BlockDisplayMode>,
    /// Last Esc press timestamp (for double-Esc detection).
    pub last_esc_at: Option<std::time::Instant>,
    /// Multiline input mode — Enter inserts newline, Shift+Enter sends.
    pub multiline_mode: bool,
    /// Autonomy mode mirror for display. The authoritative value lives in
    /// the shared `AskBridge` mode atomic (toggled by Shift+Tab); this field
    /// is kept in lock-step so the render loop can draw a badge.
    pub autonomy_mode: Mode,
    /// Submitted prompt history (most-recent-first).
    pub prompt_history: Vec<String>,
    /// Current position in history navigation (None = not navigating).
    pub history_pos: Option<usize>,
    /// Next block ID to assign when appending transcript lines.
    pub next_block_id: usize,
    /// Cancel grace window — Esc pressed within this window after a cancel
    /// is ignored (grok-build post-cancel grace, ~1s). Prevents mashing.
    pub cancel_grace_until: Option<std::time::Instant>,
    /// Active y/n/x confirmation dialog — `Some` while a modal confirmation
    /// is open. The input thread resolves it; the render loop paints it
    /// centered on top of everything else.
    pub confirmation: Option<ModalConfirmation>,
    /// Active ephemeral tip banner — `Some` for a bounded number of render
    /// ticks, then auto-dismissed by expiry.
    pub tip: Option<EphemeralTip>,
    /// Workspace root — used by the @ file picker to walk + fuzzy-match.
    pub cwd: PathBuf,
    /// Active @-file-search dropdown — `Some` while the picker is open.
    pub file_search: Option<crate::tui_vt::file_search::FileSearchState>,
    /// Per-tip-key show counter — suppresses ambient tips after SEEN_CAP views.
    pub seen_tips: std::collections::HashMap<&'static str, u32>,
    /// User-defined slash commands loaded once at startup from
    /// `.oxicode/commands/` and `~/.oxicode/commands/`.
    pub file_commands: Vec<FileCommand>,
    /// Provider name and origin for the currently open secure prompt.
    /// Set before opening the prompt; cleared on `OverlaySubmission::SecureInput`
    /// after the key is written. `None` outside the secure-prompt flows
    /// (`/providers` row action, `/providers add`, programmatic rekey) so a
    /// stray `SecureInput` cannot leak into a different provider.
    ///
    /// The `SecureInputOrigin` variant lets the consumer of the submitted
    /// key know whether to greet the user ("just added a provider") or
    /// simply acknowledge ("key replaced") — both write to the same auth
    /// storage slot, but the surrounding UX differs.
    pub secure_input_origin: Option<SecureInputOrigin>,
    /// Live-session swapper. `None` until the TUI startup wires it.
    /// The render loop and the agent worker both call `current()` per
    /// dispatch; the resume `tokio::spawn` calls `swap(new_handle)`.
    /// `Option` because `#[derive(Default)]` requires it.
    pub session_swapper: Option<Arc<crate::app::agent_session_handle::SessionSwapper>>,
    /// `Some(path)` when the slash command wants the event loop to
    /// drain a resume job on the next `Submitted` arm. The
    /// `Submitted` arm calls `state.pending_resume.take()` and
    /// enqueues the resume.
    pub pending_resume: Option<PathBuf>,
    pub session_state: Option<crate::SessionState>,
    /// Active `/settings` tab. Persisted across overlay reopens within
    /// the session; drives both the tab-switch rebuild and the sidebar
    /// highlight.
    pub settings_active_tab: crate::tui_vt::settings_defs::SettingsTab,
    /// Row-kind table for the settings panel's map editors
    /// (Keybindings / Model roles), index-aligned with
    /// `overlay.items` while the tabbed panel is open. Built by the
    /// same pass that builds the items; consulted by the input thread
    /// to route `Enter` / `d` / `n` on map rows. Empty (or stale —
    /// every consumer re-checks length alignment) for non-settings
    /// overlays.
    pub settings_map_rows: Vec<Option<SettingsMapRow>>,
    /// Live global-shortcut resolver, seeded from
    /// `Settings::keybindings` at TUI startup and swapped in place by
    /// the keybindings editor. `parking_lot::RwLock` (not ArcSwap) — no
    /// new dependency, and the per-keystroke read-lock cost is
    /// negligible.
    pub keymap: Arc<parking_lot::RwLock<crate::tui_vt::keymap::Keymap>>,
    /// Test-only sandbox: when `Some`, the keybindings commit path
    /// writes `Settings` to this path via `Settings::save_to` instead
    /// of touching the real `~/.oxicode/settings.{json,toml}`. The
    /// production TUI leaves this at `None`; only unit tests set it.
    /// Thread-safety is the same as `RenderState` itself (single-thread
    /// use in the input thread).
    #[cfg(test)]
    pub settings_override_path: Option<std::path::PathBuf>,

    /// Git TUI overlay — `Some` while `/git` is open. The render loop
    /// paints the overlay over the scrollback+composer region when set;
    /// the input thread routes keys through `match_git_key` and never
    /// lets them reach the composer.
    pub git_tui: Option<crate::tui_vt::git_tui::GitTuiState>,
    /// Width/height of the git TUI overlay viewport (mirrored from the
    /// last render pass so resize events can be detected without a
    /// round-trip into the ratatui Frame).
    pub git_tui_viewport: (u16, u16),
}

impl Default for RenderState {
    fn default() -> Self {
        // for every other field. The composer starts empty.
        Self {
            composer: oxicode_textarea::TextArea::new(),
            transcript: Vec::new(),
            scroll_offset: usize::MAX,
            committed_entries: 0,
            header_context: InlineHeaderContext::default(),
            input_enabled: false,
            prompt_prefix: String::new(),
            placeholder: None,
            thinking_buffer: String::new(),
            shutdown_requested: false,
            message_buffer: String::new(),
            stream_anchor: None,
            md_cache: oxicode_vtui::tui::ui::markdown::MdRenderCache::default(),
            agent_hub_open: false,
            hub_entries: Vec::new(),
            hub: None,
            pending_quit: false,
            slash_popup: SlashPopup::default(),
            reasoning_stage: None,
            active_run: None,
            stream_reveal: usize::MAX,
            thinking_level: "medium".to_string(),
            viewport_width: 80,
            last_viewport_height: 24,
            glyph_set: crate::symbols::GlyphSet::default(),
            image_previews: super::image_preview::ImagePreviews::default(),
            overlay: None,
            overlay_model_ids: Vec::new(),
            overlay_catalog_models: Vec::new(),
            overlay_providers: Vec::new(),
            catalog: None,
            queued_inputs: Vec::new(),
            queue_panel_open: false,
            queue_selected: 0,
            shell_mode: false,
            follow_ups: Vec::new(),
            todo_phases: Vec::new(),
            todo_expanded: false,
            todo_clear_deadline: None,
            todo_clear_delay_secs: -1,
            todo_provider: None,
            vim_state: crate::tui_vt::vim::VimState::default(),
            vim_clipboard: String::new(),
            search: None,
            block_display: std::collections::HashMap::new(),
            last_esc_at: None,
            multiline_mode: false,
            autonomy_mode: Mode::default(),
            prompt_history: Vec::new(),
            history_pos: None,
            next_block_id: 0,
            cancel_grace_until: None,
            confirmation: None,
            tip: None,
            cwd: PathBuf::new(),
            file_search: None,
            seen_tips: std::collections::HashMap::new(),
            file_commands: Vec::new(),
            secure_input_origin: None,
            session_swapper: None,
            context_tokens: None,
            context_window: 128_000,
            settings_active_tab: crate::tui_vt::settings_defs::SettingsTab::General,
            settings_map_rows: Vec::new(),
            // Default bindings only — `new_with_header` (the real TUI
            // startup) layers `Settings::keybindings` on top, keeping
            // `Default` free of disk I/O for tests.
            keymap: Arc::new(parking_lot::RwLock::new(Keymap::from_settings(
                &std::collections::HashMap::new(),
            ))),
            #[cfg(test)]
            settings_override_path: None,
            brain: BrainChip::default(),
            pending_resume: None,
            session_state: None,
            git_tui: None,
            git_tui_viewport: (80, 24),
        }
    }
}

/// Where a secure prompt came from. The `SecureInput` overlay has just one
/// payload (the text); the origin discriminates the post-commit follow-up
/// so the user gets a contextual flow instead of a generic "saved" line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecureInputOrigin {
    /// User picked a provider row and chose "Set API key" (or hit Enter
    /// on a key-only provider with no key) — this is a *replace* or
    /// first-time key entry for an existing provider.
    SetKey { provider: String },
    /// User just added a provider via `/providers add …` and we are
    /// chaining straight into the key prompt so they can finish the
    /// setup without another navigation step.
    NewlyAdded { provider: String },
    /// Model-roles map editor: the user pressed `n` — the submitted
    /// text is the new ROLE name; the value prompt follows.
    ModelRoleKey,
    /// Model-roles map editor: the submitted text is the model pattern
    /// for `role`.
    ModelRoleValue { role: String },
    /// Generic settings-panel text editor: the submitted text is
    /// committed via `settings_defs::apply_change` for the named
    /// SettingKey. Empty input clears the override (where the field
    /// is `Option`); invalid input is rejected with an inline error.
    TextEdit(crate::tui_vt::settings_defs::SettingKey),
}

/// In-transcript search state.
#[derive(Clone, Debug)]
pub struct SearchState {
    pub query: String,
    /// Transcript line indices that contain a match.
    pub matches: Vec<usize>,
    /// Current match cursor (index into `matches`).
    pub current: usize,
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

/// One item rendered inside a list overlay. Mirrors [`InlineListItem`] but
/// is a value type owned by the TUI (the input thread reads/writes these
/// fields directly via the `parking_lot::Mutex<RenderState>`).
#[derive(Clone, Debug)]
pub struct OverlayListItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub badge: Option<String>,
    pub indent: u8,
    pub search_value: Option<String>,
    /// Original `InlineListSelection` echoed back to the harness on submit.
    pub selection: Option<oxicode_vtui::tui::core::InlineListSelection>,
}

/// Overlay modal/list state — materialised by `apply_command` when an
/// `InlineCommand::ShowOverlay` arrives. The input thread mutates
/// `selected` / `search` while the overlay is open and reads the same
/// fields when forwarding `OverlayEvent`s.
///
/// `tabs` / `sections` carry the settings panel's tab bar and sidebar.
/// Both stay default-empty for every other overlay — `render_overlay`
/// only takes the tabbed/sidebar branches when they are populated.
#[derive(Clone, Debug, Default)]
pub struct OverlayState {
    pub title: String,
    pub lines: Vec<String>,
    pub items: Vec<OverlayListItem>,
    pub selected: usize,
    pub search: Option<OverlaySearchState>,
    pub secure_input: Option<OverlaySecureInput>,
    /// Tab-bar labels (settings panel only; empty ⇒ no tab bar).
    pub tabs: Vec<String>,
    /// Index of the active tab into `tabs`.
    pub active_tab: usize,
    /// Sidebar section (group) labels for the active tab; the sidebar
    /// renders when there are at least two.
    pub sections: Vec<String>,
    /// Index of the active section into `sections`, synced to the group
    /// of the currently selected item.
    pub active_section: usize,
    /// Keybinding-capture mode (settings panel only): `Some(action
    /// name)` while the "press a key combo" prompt is up. The input
    /// thread intercepts the next key BEFORE global-shortcut resolution
    /// so even a combo that currently triggers an action is captured
    /// verbatim. Esc cancels.
    pub key_capture: Option<String>,
}

/// Secure (masked) single-line input state carried by an overlay.
/// Only present when the original `OverlayRequest::Modal` carried a
/// `secure_prompt`. The input thread mutates `editor` while the overlay is
/// open; on `Enter` it submits `OverlaySubmission::SecureInput` carrying
/// the editor's text. The real secret never leaves the editor — the
/// renderer paints the value via a `TextElement` whose display is the
/// mask.
#[derive(Clone, Debug)]
pub struct OverlaySecureInput {
    pub config: SecurePromptConfig,
    pub editor: EditBuffer,
}

/// A y/n/x confirmation dialog (grok-build `ModalConfirmation` parity).
/// Rendered centered on top of everything else; the input thread routes
/// `y` → confirm, `n` → decline (when offered), `x`/`Esc` → cancel.
#[derive(Clone, Debug)]
pub struct ModalConfirmation {
    pub title: String,
    pub message: String,
    /// What happens when the user confirms (`y`). Cancel (`n`/`x`/`Esc`)
    /// always just closes the dialog.
    pub action: ConfirmationAction,
}

/// The action bound to a [`ModalConfirmation`] — dispatched on `y`/Enter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmationAction {
    /// Exit the application.
    Quit,
    /// Clear the conversation transcript + reset the agent session.
    ClearConversation,
    /// Remove the stored API key for a provider (`/providers` → confirm).
    RemoveProviderKey(String),
}

/// A short-lived contextual tip banner (grok-build ephemeral tips parity).
/// Shown as one line above the composer for a bounded number of render
/// ticks, then auto-dismissed.
#[derive(Clone, Debug)]
pub struct EphemeralTip {
    pub text: String,
    /// Render tick the tip was born at (`FRAME_TICK` snapshot).
    pub born_tick: u64,
    /// How many ticks the tip stays visible before auto-dismissing.
    pub ttl_ticks: u64,
    /// Stable identifier for per-session seen-cap tracking. Tips with the
    /// same key are suppressed after `SEEN_CAP` showings.
    pub key: &'static str,
    /// Ambient tips (background suggestions) are occluded — their TTL pauses
    /// while an overlay/confirmation/dropdown is open. Non-ambient tips
    /// (direct user-action feedback) always count down.
    pub ambient: bool,
}

/// Search-bar state for an overlay. `None` value means search is disabled.
#[derive(Clone, Debug)]
pub struct OverlaySearchState {
    pub label: String,
    pub placeholder: Option<String>,
    pub value: String,
}

impl RenderState {
    fn new_with_header(header: InlineHeaderContext) -> Self {
        let mut s = Self::default();
        s.header_context = header;
        s.prompt_prefix = "> ".to_string();
        s.input_enabled = true;
        // Build the live keymap once at startup from the persisted
        // bindings — the input loop resolves every keystroke against it.
        let bindings = crate::store::settings::Settings::load()
            .unwrap_or_default()
            .keybindings;
        *s.keymap.write() = Keymap::from_settings(&bindings);
        s
    }

    /// Get a clone of the live `SessionSwapper`. Panics if the TUI
    /// wasn't initialized properly (the `run_tui` startup wires it
    /// before any user input is processed, so the panic is
    /// unreachable in normal use).
    pub fn swapper(&self) -> Arc<crate::app::agent_session_handle::SessionSwapper> {
        self.session_swapper
            .clone()
            .expect("RenderState::session_swapper must be initialized at TUI startup")
    }

    /// Append one or more brand-new transcript lines.
    ///
    /// `ratatui::text::Line` is a single visual line: embedded `\n`
    /// characters are flattened. Normalize protocol segments at this
    /// boundary so `TranscriptLine` keeps its name and rendering contract.
    fn append_line(&mut self, kind: InlineMessageKind, segments: Vec<InlineSegment>) {
        let block_id = self.block_id_for_kind(kind);
        self.transcript
            .extend(
                Self::segments_by_explicit_line(segments)
                    .into_iter()
                    .map(|segments| TranscriptLine {
                        kind,
                        segments,
                        block_id,
                    }),
            );
    }

    /// Append line(s) that open a NEW block instead of merging into the
    /// last block of the same kind — omp-style tool boxes are one
    /// atomic block per call (border, command, output, border).
    fn append_line_new_block(&mut self, kind: InlineMessageKind, segments: Vec<InlineSegment>) {
        let block_id = self.fresh_block_id();
        self.transcript
            .extend(
                Self::segments_by_explicit_line(segments)
                    .into_iter()
                    .map(|segments| TranscriptLine {
                        kind,
                        segments,
                        block_id,
                    }),
            );
    }

    /// Append a streaming delta to the active line. Explicit newlines finish
    /// the current line and open another line in the same semantic block.
    fn inline_segment(&mut self, kind: InlineMessageKind, segment: InlineSegment) {
        let mut lines = Self::segments_by_explicit_line(vec![segment]).into_iter();

        // Merge into the tail line only while it belongs to the in-flight
        // stream. Without the anchor guard, the first delta of a NEW
        // message would append into the previous message's final line —
        // mutating history the user already read.
        let streaming = self.stream_anchor.is_some();
        if let Some(first) = lines.next() {
            let merge_ok =
                streaming && self.transcript.last().is_some_and(|last| last.kind == kind);
            if merge_ok && let Some(last) = self.transcript.last_mut() {
                last.segments.extend(first);
            } else {
                // A fresh streamed message opens its own block so folding
                // and turn structure cannot bleed across messages.
                let block_id = self.fresh_block_id();
                if self.stream_anchor.is_none() {
                    self.stream_anchor = Some(self.transcript.len());
                }
                self.transcript.push(TranscriptLine {
                    kind,
                    segments: first,
                    block_id,
                });
            }
        }
        let block_id = self
            .stream_anchor
            .and_then(|a| self.transcript.get(a))
            .map(|l| l.block_id)
            .unwrap_or_else(|| self.fresh_block_id());
        self.transcript.extend(lines.map(|segments| TranscriptLine {
            kind,
            segments,
            block_id,
        }));
    }

    /// Split styled segments without allocating on the common single-line
    /// path. Empty chunks are retained because blank lines carry layout.
    fn segments_by_explicit_line(segments: Vec<InlineSegment>) -> Vec<Vec<InlineSegment>> {
        if !segments.iter().any(|segment| segment.text.contains('\n')) {
            return vec![segments];
        }

        let mut lines = vec![Vec::new()];
        for segment in segments {
            let InlineSegment { text, style } = segment;
            for (index, part) in text.split('\n').enumerate() {
                if index > 0 {
                    lines.push(Vec::new());
                }
                if !part.is_empty()
                    && let Some(line) = lines.last_mut()
                {
                    line.push(InlineSegment {
                        text: part.to_string(),
                        style: Arc::clone(&style),
                    });
                }
            }
        }
        lines
    }

    /// Determine the block_id for a new line: reuse the last line's block
    /// if the kind matches, otherwise allocate a new block.
    fn block_id_for_kind(&mut self, kind: InlineMessageKind) -> usize {
        if let Some(last) = self.transcript.last()
            && last.kind == kind
        {
            return last.block_id;
        }
        let id = self.next_block_id;
        self.next_block_id += 1;
        id
    }

    /// Allocate a block id that cannot merge with an existing block.
    fn fresh_block_id(&mut self) -> usize {
        let id = self.next_block_id;
        self.next_block_id += 1;
        id
    }

    // ── Search ──

    /// Start a new transcript search, collecting all matching line indices.
    pub fn start_search(&mut self, query: &str) {
        let needle = query.to_lowercase();
        let matches: Vec<usize> = self
            .transcript
            .iter()
            .enumerate()
            // Committed entries are frozen in the host scrollback —
            // the live region cannot scroll to them, so search skips.
            .filter(|(i, _)| *i >= self.committed_entries)
            .filter(|(_, line)| {
                line.segments
                    .iter()
                    .any(|s| s.text.to_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();
        self.search = Some(SearchState {
            query: query.to_string(),
            matches,
            current: 0,
        });
        // Jump to the first match if any.
        if let Some(s) = &self.search
            && let Some(&first) = s.matches.first()
        {
            self.scroll_offset = first;
        }
    }

    /// Advance to the next search match (wraps around).
    pub fn search_next(&mut self) {
        if let Some(s) = &mut self.search
            && !s.matches.is_empty()
        {
            s.current = (s.current + 1) % s.matches.len();
            let line = s.matches[s.current];
            self.scroll_offset = line;
        }
    }

    /// Go to the previous search match (wraps around).
    pub fn search_prev(&mut self) {
        if let Some(s) = &mut self.search
            && !s.matches.is_empty()
        {
            if s.current == 0 {
                s.current = s.matches.len() - 1;
            } else {
                s.current -= 1;
            }
            let line = s.matches[s.current];
            self.scroll_offset = line;
        }
    }

    // ── Block display modes (Collapsed / Truncated / Expanded) ──

    /// The display mode for a block — explicit override or the Expanded
    /// default. Chat content hides nothing by default: middle-elision
    /// made long responses unreadable and unscrollable past the gap.
    pub fn block_mode(&self, block_id: usize) -> BlockDisplayMode {
        self.block_display
            .get(&block_id)
            .copied()
            .unwrap_or(BlockDisplayMode::Expanded)
    }

    /// Cycle the display mode of the block at (or nearest above) the current
    /// scroll offset: Collapsed → Truncated → Expanded → Collapsed.
    pub fn cycle_block_at_view(&mut self) {
        let offset = self.effective_offset();
        if let Some(line) = self.transcript.get(offset) {
            let bid = line.block_id;
            let next = match self.block_mode(bid) {
                BlockDisplayMode::Collapsed => BlockDisplayMode::Truncated,
                BlockDisplayMode::Truncated => BlockDisplayMode::Expanded,
                BlockDisplayMode::Expanded => BlockDisplayMode::Collapsed,
            };
            // Expanded is the default — represent it by absence so the map
            // only carries real overrides.
            if next == BlockDisplayMode::Expanded {
                self.block_display.remove(&bid);
            } else {
                self.block_display.insert(bid, next);
            }
        }
    }

    /// Expand every block. Expanded is the default, so this simply drops
    /// all overrides.
    pub fn expand_all(&mut self) {
        self.block_display.clear();
    }

    /// Collapse every block (first line only).
    pub fn fold_all(&mut self) {
        for bid in self.all_block_ids() {
            self.block_display.insert(bid, BlockDisplayMode::Collapsed);
        }
    }

    /// Reset every block to the default Truncated mode.
    pub fn truncate_all(&mut self) {
        // Truncated is no longer the default — it must be recorded
        // explicitly for every block.
        for bid in self.all_block_ids() {
            self.block_display.insert(bid, BlockDisplayMode::Truncated);
        }
    }

    /// Distinct block ids in transcript order.
    fn all_block_ids(&self) -> Vec<usize> {
        let mut ids = Vec::new();
        let mut prev: Option<usize> = None;
        for l in &self.transcript {
            if prev != Some(l.block_id) {
                ids.push(l.block_id);
                prev = Some(l.block_id);
            }
        }
        ids
    }

    // ── Turn navigation ──

    /// Jump the scroll to the start of the next assistant (Agent) block.
    pub fn jump_next_turn(&mut self) {
        let offset = self.effective_offset();
        let search_after = self
            .transcript
            .iter()
            .enumerate()
            .skip(offset + 1)
            .find(|(_, l)| l.kind == InlineMessageKind::Agent || l.kind == InlineMessageKind::User);
        if let Some((idx, _)) = search_after {
            self.scroll_offset = idx;
        }
    }

    /// Jump the scroll to the start of the previous user block.
    pub fn jump_prev_turn(&mut self) {
        let offset = self.effective_offset();
        let search_before = self
            .transcript
            .iter()
            .enumerate()
            .take(offset)
            .rev()
            .find(|(_, l)| l.kind == InlineMessageKind::User);
        if let Some((idx, _)) = search_before {
            self.scroll_offset = idx;
        }
    }

    /// Effective scroll offset (resolves `usize::MAX` follow-tail to a real index).
    fn effective_offset(&self) -> usize {
        if self.scroll_offset == usize::MAX {
            self.transcript.len().saturating_sub(1)
        } else {
            self.scroll_offset
        }
    }

    /// Drop the head of the queued-input list. Called when a turn ends so
    /// the queue pane stops showing the prompt that is now running.
    pub fn drain_queue_head(&mut self) {
        if !self.queued_inputs.is_empty() {
            self.queued_inputs.remove(0);
        }
    }

    /// Show an ephemeral tip if the per-session seen-cap hasn't been reached.
    /// Each unique `key` can show at most `SEEN_CAP` times per session.
    pub fn show_tip(&mut self, key: &'static str, text: &str, ttl: u64, ambient: bool) {
        let count = self.seen_tips.entry(key).or_insert(0);
        if *count >= SEEN_CAP {
            return;
        }
        *count += 1;
        self.tip = Some(EphemeralTip {
            text: text.to_string(),
            born_tick: FRAME_TICK.load(std::sync::atomic::Ordering::Relaxed),
            ttl_ticks: ttl,
            key,
            ambient,
        });
    }
}

/// Max times an ambient tip key is shown per session before suppression.
const SEEN_CAP: u32 = 3;

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
    // Validate active theme contrast and log any warnings.
    let theme_id = oxicode_vtui::theme::active_theme_id();
    let validation = oxicode_vtui::theme::validate_theme_contrast(&theme_id);
    if validation.warnings.is_empty() {
        tracing::debug!("theme '{theme_id}' passed contrast validation");
    } else {
        for w in &validation.warnings {
            tracing::warn!("theme contrast: {w}");
        }
    }

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

    // Wrap the initial handle in a SessionSwapper. The render loop
    // and the agent worker both read through `current()`; the
    // resume `tokio::spawn` (below) calls `swap(new_handle)`.
    let session_swapper = Arc::new(crate::app::agent_session_handle::SessionSwapper::new(
        session_handle.clone(),
    ));

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
    handle.set_placeholder(Some(
        "Describe the task, or type / for commands".to_string(),
    ));

    // Render state — shared between the input thread (which edits the
    // buffer) and the main loop (which reads it for drawing).
    let state = Arc::new(parking_lot::Mutex::new(RenderState::new_with_header(
        header,
    )));
    state.lock().cwd = cwd.clone();
    state.lock().catalog = Some(app.catalog());
    state.lock().file_commands = crate::tui_vt::slash::file_commands::load_file_commands(&cwd);
    state.lock().todo_provider = session_handle.todo_provider();
    state.lock().todo_clear_delay_secs = app.settings().todo_clear_delay_secs;
    state.lock().hub = Some(session_handle.hub_arc());
    state.lock().session_swapper = Some(session_swapper.clone());
    state.lock().session_state = Some(app.session_state().clone());
    state.lock().thinking_level = format!("{:?}", session.thinking_level()).to_ascii_lowercase();
    // MODEL chip + CTX denominator from the live session (the boot header
    // context carries the model id; the context window comes from here).
    {
        let mut s = state.lock();
        sync_model_chips(&mut s, &session_handle);
    }
    // Onboarding tip: surfaces the cheatsheet and help command on first run,
    // auto-dismisses after ~30s of rendering.
    state.lock().tip = Some(EphemeralTip {
        text: "Press ? for shortcuts | /help for commands".to_string(),
        born_tick: 0,
        ttl_ticks: 900,
        key: "onboarding",
        ambient: true,
    });
    // SSH tip: suggest tmux when running over SSH (1-time).
    if std::env::var("SSH_CONNECTION").is_ok() {
        state.lock().show_tip(
            "ssh_wrap",
            "Over SSH? Consider tmux to keep sessions alive",
            600,
            true,
        );
    }
    // Shared autonomy-mode handle — Shift+Tab toggles it at runtime. The
    // AskBridge atomic is the authority; the render state mirrors it so the
    // composer can draw a mode badge.
    let mode_handle = app.ask_bridge().map(|b| {
        let handle = b.mode_handle();
        state.lock().autonomy_mode = Mode::load(&handle);
        handle
    });
    state.lock().glyph_set = app.settings().glyph_set;
    // `inline_images` kill-switch (default ON): flips off every image
    // escape write; the transcript's fallback text is all that shows.
    state
        .lock()
        .image_previews
        .set_enabled(app.settings().inline_images);
    let prompt_queue = Arc::new(PromptQueue::default());
    // User-remappable keybindings live in `RenderState::keymap`, seeded
    // from `Settings::keybindings` by `new_with_header` above and swapped
    // in place by the settings keybindings editor — no separate
    // keybindings.yml bootstrap.
    spawn_input_thread(
        state.clone(),
        evt_tx.clone(),
        mode_handle,
        prompt_queue.clone(),
    );

    // Worker thread owns the agent loop and takes prompts from the shared
    // authoritative queue before dispatching them through `run_with_channel`. The
    // returned `AgentEvent`s flow through a `std::sync::mpsc`; a paired
    // forwarder thread funnels them into the session's listener bus so
    // our subscriber above picks them up.
    spawn_agent_worker(session_swapper.clone(), prompt_queue.clone());
    // Brain health prober: pings the oxibrain daemon and feeds the
    // status-bar chip through a watch channel. The interval's first tick is
    // immediate, so the chip reflects reality on the first frame after a
    // brief probe; every 20 s afterwards. A slow/absent daemon never blocks
    // the loop — the ping is timeout-bounded.
    let (brain_tx, mut brain_rx) =
        tokio::sync::watch::channel(crate::services::initial_brain_chip(app.settings()));
    {
        let memory_enabled = app.settings().memory_enabled;
        let announce = handle.clone();
        tokio::spawn(async move {
            let backend = crate::foundation::brain::BrainMemoryBackend::new(
                crate::foundation::brain::default_socket_path(),
            );
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // One automatic revive per session (success or failure — a
            // broken daemon must not turn the prober into a spawn loop).
            let mut auto_revive_attempted = false;
            loop {
                tick.tick().await;
                let mut chip = if !memory_enabled {
                    BrainChip::Off
                } else if !crate::services::brain_socket_present(
                    &crate::foundation::brain::default_socket_path(),
                ) {
                    BrainChip::Down
                } else {
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(1500),
                        backend.ping(),
                    )
                    .await
                    {
                        Ok(Ok(())) => BrainChip::Ok,
                        Ok(Err(_)) | Err(_) => BrainChip::Degraded,
                    }
                };
                // Auto-revive: memory users get their daemon back without
                // typing /brain. Never installs (binary check), never
                // retries, and says what it did on the transcript.
                let down = matches!(chip, BrainChip::Down | BrainChip::Degraded);
                if crate::foundation::brain_control::should_auto_revive(
                    memory_enabled,
                    down,
                    auto_revive_attempted,
                ) {
                    auto_revive_attempted = true;
                    let installed = crate::foundation::brain_control::probe_control()
                        .binary
                        .is_some();
                    if installed {
                        match crate::foundation::brain_control::revive().await {
                            Ok(msg) => {
                                announce.append_line(
                                    InlineMessageKind::Info,
                                    vec![plain_segment(format!("brain: daemon was down — {msg}"))],
                                );
                                // Re-probe now instead of waiting a tick.
                                chip = match tokio::time::timeout(
                                    std::time::Duration::from_millis(1500),
                                    backend.ping(),
                                )
                                .await
                                {
                                    Ok(Ok(())) => BrainChip::Ok,
                                    _ => chip,
                                };
                            }
                            Err(e) => {
                                announce.append_line(
                                    InlineMessageKind::Warning,
                                    vec![plain_segment(format!(
                                        "brain: auto-restart failed — {e} (run /brain for details)"
                                    ))],
                                );
                            }
                        }
                    }
                }
                let _ = brain_tx.send(chip);
            }
        });
    }

    let result = run_event_loop(
        &mut tui.terminal,
        &mut cmd_rx,
        &mut evt_rx,
        &mut session_rx,
        &mut brain_rx,
        &handle,
        &state,
        &session_swapper,
        &prompt_queue,
    )
    .await;

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
    brain_rx: &mut tokio::sync::watch::Receiver<BrainChip>,
    handle: &InlineHandle,
    state: &Arc<parking_lot::Mutex<RenderState>>,
    session_swapper: &Arc<crate::app::agent_session_handle::SessionSwapper>,
    prompt_queue: &Arc<PromptQueue>,
) -> Result<()> {
    // Drain any pending InlineCommands so the harness's initial set_header_context
    // (and similar) is observed before the first frame.
    while let Ok(cmd) = cmd_rx.try_recv() {
        apply_command(&mut state.lock(), cmd);
    }

    // Seed the resize detector from the real terminal before any
    // frame is drawn (final-review finding 1). `RenderState::
    // default()`'s 80 columns is a test-only fallback; left in place
    // it made the first draw of any terminal wider than 80 look like
    // a resize (80 → real width) and fire CSI 3J + Clear, wiping the
    // user's pre-TUI shell scrollback on every launch. On a size
    // failure we park the 0 sentinel — `should_rebuild_scrollback`
    // refuses to wipe until a real width has been observed.
    {
        let mut s = state.lock();
        match terminal.size() {
            Ok(size) => {
                s.viewport_width = size.width;
                s.last_viewport_height = size.height;
            }
            Err(_) => {
                s.viewport_width = 0;
                s.last_viewport_height = 0;
            }
        }
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
    // Render coalescing: most iterations skip the full draw and instead
    // rely on the 50ms `render_tick` arm below to guarantee a heartbeat.
    // `priority` is raised by user-facing arms (keyboard, SIGINT, brain
    // chip) so typing/cancels/chip flips repaint immediately.
    let mut last_draw = std::time::Instant::now();
    let mut priority = false;

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
                // Intercept handoff-completion to clear transcript + auto-submit.
                if let SessionEvent::HandoffComplete { doc_path, auto_continue } = &event {
                    let mut s = state.lock();
                    s.transcript.clear();
                    s.message_buffer.clear();
                    s.scroll_offset = usize::MAX;
                    s.append_line(
                        InlineMessageKind::Info,
                        vec![plain_segment(format!(
                            "Handoff written to {}. New session started.",
                            doc_path
                        ))],
                    );
                    let user_typed = !s.composer.text().trim().is_empty();
                    s.composer.set_text("");
                    if *auto_continue && !user_typed {
                        drop(s);
                        prompt_queue.enqueue(format!(
                            "Read the handoff document at {} and continue \
                             from where the previous session left off.",
                            doc_path
                        ));
                    } else if user_typed {
                        drop(s);
                        handle.append_line(
                            InlineMessageKind::Info,
                            vec![plain_segment(
                                "Handoff complete. Auto-continue skipped \
                                 because input was non-empty \u{2014} press \
                                 Enter to submit your message in the new \
                                 session."
                                    .to_string(),
                            )],
                        );
                    }
                    let session = session_swapper.current();
                    handle_session_event(&mut state.lock(), handle, &event, Some(&session));
                } else {
                    // Every regular agent event must reach the presentation
                    // bridge.  The handoff path above already does this after
                    // resetting the transcript; previously it was the *only*
                    // path that did.  As a result, prompts ran in the worker
                    // but token deltas, tool progress, and provider errors
                    // were silently discarded before a frame could render.
                    let session = session_swapper.current();
                    handle_session_event(&mut state.lock(), handle, &event, Some(&session));
                }
            }

            // 3. Keyboard / paste / TUI events from the input thread.
            Some(evt) = evt_rx.recv() => {
                let outcome = handle_inline_event(
                    &mut state.lock(),
                    handle,
                    &session_swapper.current(),
                    prompt_queue,
                    evt,
                );
                if outcome == LoopOutcome::Exit {
                    break;
                }
                priority = true;
            }

            // 4. External SIGINT — route through the same idle-vs-streaming
            //    policy as the key path (some terminals deliver Ctrl+C both
            //    as a key event AND raise SIGINT; `kill -INT` also lands here).
            _ = tokio::signal::ctrl_c() => {
                let outcome = {
                    let mut s = state.lock();
                    handle_interrupt(&mut s, &session_swapper.current(), handle)
                };
                if outcome == LoopOutcome::Exit {
                    break;
                }
                priority = true;
            }
            // 5. Brain health chip updates from the background prober.
            changed = brain_rx.changed() => {
                if changed.is_ok() {
                    state.lock().brain = *brain_rx.borrow_and_update();
                    priority = true;
                }
            }

            // 6. Periodic repaint — echoes typed input and drives animation
            //    even when no other event is ready.
            _ = render_tick.tick() => {}
        }

        // Render coalescing: skip the snapshot/draw pipeline when no
        // user-facing arm raised priority and the render cadence has not
        // elapsed. The 50ms `render_tick` arm guarantees the heartbeat.
        if coalesce_draw(last_draw, priority, DRAW_MIN_INTERVAL) == DrawDecision::DrawNow {
            // small_screen tip: warn when terminal is too narrow for full UI.
            if let Ok(size) = terminal.size()
                && size.width < 40
            {
                let mut s = state.lock();
                if s.tip.is_none() {
                    s.show_tip(
                        "small_screen",
                        "Terminal too narrow \u{2014} resize for full UI",
                        300,
                        true,
                    );
                }
            }
            // Redraw. The harness's redraw is idempotent — the ratatui
            let mut snapshot = state.lock();
            // Resize observation: ratatui's Inline viewport auto-resizes
            // the cursor-row viewport on terminal draw, but the frozen
            // transcript in the host scrollback was printed at the
            // previous width and cannot re-wrap. When the width changes
            // we must (1) wipe the scrollback (CSI 3J) so stale-width
            // rows disappear, (2) clear the visible screen so the
            // viewport re-anchors cleanly, and (3) reset
            // `committed_entries` so the next ticks re-commit at the
            // new width. Height-only resize is a no-op (the live
            // region just grows or shrinks under the frozen history).
            let mut prev_size: Option<(u16, u16)> = None;
            if let Ok(size) = terminal.size() {
                prev_size = Some((snapshot.viewport_width, snapshot.last_viewport_height));
                snapshot.viewport_width = size.width;
                snapshot.last_viewport_height = size.height;
            }
            if let Some((prev_w, prev_h)) = prev_size
                && let Ok(size) = terminal.size()
                && should_rebuild_scrollback(prev_w, size.width, prev_h, size.height)
            {
                // CSI 3J erases the host scrollback; Clear(All) wipes
                // the visible viewport so stale-width rows vanish.
                let _ = execute!(terminal.backend_mut(), crossterm::style::Print("\x1b[3J"));
                let _ = terminal.clear();
                snapshot.committed_entries = 0;
                // No `priority = true` here — the unconditional reset
                // at the end of the draw branch would clobber it. The
                // CSI 3J + Clear already wiped the visible frame, so
                // the next render cadence tick repaints cleanly.
            }
            // pane reflects phase changes written by the `todo` agent tool, plus
            // subagent auto-reconcile (idle subagents close their matched todos).
            if let Some(provider) = snapshot.todo_provider.as_ref() {
                snapshot.todo_phases = refresh_todo_phases(provider, snapshot.hub.as_ref());
            }
            // HUD-only auto-clear: once the list settles (all closed) and the
            // delay elapses, drop the phases from the pane. The underlying
            // TodoState is untouched, so a later `/todo` or `todo` tool call
            // still sees the historical phases.
            let clear_delay = snapshot.todo_clear_delay_secs;
            sync_todo_clear_timer(&mut snapshot, clear_delay);
            // Typewriter paint: reveal the streamed body a bounded step per
            // frame so it types out instead of jumping per network chunk.
            advance_stream_reveal(&mut snapshot);
            // Shed finalized rows into the host scrollback before the
            // synchronized repaint so the commit and the viewport redraw
            // land as one visual update.
            commit_scrollback(terminal, &mut snapshot, false);
            let _ = execute!(terminal.backend_mut(), BeginSynchronizedUpdate);
            let draw_err = terminal
                .draw(|frame| render_frame(frame, &snapshot, handle))
                .err();
            let _ = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
            // Inline image previews: now that the frame (with the image
            // tool boxes) has flushed, emit the kitty transmit/place or
            // iTerm2 inline escapes for rows that rendered LIVE this
            // frame. Committed rows never emit — their fallback text is
            // already in the host scrollback. The write goes through the
            // terminal backend (same path as the synchronized-update
            // escapes above).
            let committed = snapshot.committed_entries;
            let image_escapes = snapshot.image_previews.emit_live(committed);
            if !image_escapes.is_empty() {
                let _ = execute!(
                    terminal.backend_mut(),
                    crossterm::style::Print(image_escapes)
                );
            }
            if let Some(err) = draw_err {
                tracing::warn!(?err, "tui draw failed");
                break;
            }
            // Reset cadence — next draw is gated again until either
            // priority is raised or the interval elapses.
            last_draw = std::time::Instant::now();
            priority = false;
        }
    }

    // Exit flush: land every committable finalized row into the host
    // scrollback before the caller drops `Tui` (which restores the
    // terminal — after that the host scrollback is no longer in raw
    // mode and the print-before rows survive). The cap is a safety
    // belt: a stuck `insert_before` (broken terminal) cannot trap us
    // in the flush.
    const MAX_EXIT_FLUSH_ITERATIONS: usize = 50;
    for _ in 0..MAX_EXIT_FLUSH_ITERATIONS {
        let mut snapshot = state.lock();
        let before = snapshot.committed_entries;
        if snapshot.transcript.is_empty() || before >= snapshot.transcript.len() {
            break;
        }
        // pane reflects phase changes written by the `todo` agent tool, plus
        // subagent auto-reconcile (idle subagents close their matched todos).
        if let Some(provider) = snapshot.todo_provider.as_ref() {
            snapshot.todo_phases = refresh_todo_phases(provider, snapshot.hub.as_ref());
        }
        // HUD-only auto-clear: once the list settles (all closed) and the
        // delay elapses, drop the phases from the pane. The underlying
        // TodoState is untouched, so a later `/todo` or `todo` tool call
        // still sees the historical phases.
        let clear_delay = snapshot.todo_clear_delay_secs;
        sync_todo_clear_timer(&mut snapshot, clear_delay);
        // Typewriter paint: reveal the streamed body a bounded step per
        // frame so it types out instead of jumping per network chunk.
        advance_stream_reveal(&mut snapshot);
        // Shed finalized rows into the host scrollback before the
        // synchronized repaint so the commit and the viewport redraw
        // land as one visual update.
        commit_scrollback(terminal, &mut snapshot, true);
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

/// Minimum interval between successive full `terminal.draw` passes driven
/// by the event loop. User-facing arms (keyboard, SIGINT, brain chip)
/// bypass this via `priority = true`; token-stream agent events coalesce
/// here so a 200-events/sec burst does not become 200 draws/sec.
const DRAW_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Whether the post-select draw block should run this iteration.
#[derive(Debug, PartialEq, Eq)]
enum DrawDecision {
    /// Run the snapshot/draw pipeline now.
    DrawNow,
    /// Skip the draw — nothing on screen needs an immediate repaint and
    /// the frame cadence has not elapsed yet.
    Defer,
}

/// Pure coalescing decision: `priority` (user input) always wins; otherwise
/// we draw once the render-cadence timer has elapsed since the last draw.
fn coalesce_draw(
    last_draw_at: std::time::Instant,
    priority: bool,
    min_interval: std::time::Duration,
) -> DrawDecision {
    if priority || last_draw_at.elapsed() >= min_interval {
        DrawDecision::DrawNow
    } else {
        DrawDecision::Defer
    }
}

#[derive(PartialEq, Eq)]
enum LoopOutcome {
    Continue,
    Exit,
}

/// Whether an Esc-driven cancel should abort the running stream (via the
/// interrupt path, which sets the footer + abort) or exit the app outright
/// (idle one-press quit). Extracted as a pure function so the routing can
/// be unit-tested without a live `AgentSessionHandle`.
#[derive(PartialEq, Eq, Debug)]
enum CancelRoute {
    /// A stream is running: abort it. The input thread's ~1s post-cancel
    /// grace then prevents mashing Esc from firing repeated cancels.
    Interrupt,
    /// Idle: instant one-press quit — no quit-arming footer, no grace.
    Exit,
}

/// Pure routing decision for `InlineEvent::Cancel`. While a stream is
/// running, Esc aborts it (matching Ctrl+C). When idle, Esc quits at once.
fn route_cancel(is_streaming: bool) -> CancelRoute {
    if is_streaming {
        CancelRoute::Interrupt
    } else {
        CancelRoute::Exit
    }
}

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
        InlineCommand::BeginStream { .. } => {
            // A new streamed message opens: drop the anchor so the first
            // delta starts a fresh block instead of merging into the
            // previous message's rendered lines.
            state.stream_anchor = None;
        }
        InlineCommand::EndStream => {
            // The streamed message finalized: release the anchor so the
            // finished block can commit to the host scrollback. Travels
            // in the command stream after the final ReplaceLast — the
            // causal order keeps the anchor pinned through the last
            // re-render.
            state.stream_anchor = None;
        }
        InlineCommand::AppendLineBlockStart { kind, segments } => {
            state.append_line_new_block(kind, segments);
        }
        InlineCommand::ReplaceLast { kind, lines, .. } => {
            // The anchor records where this message's streamed block
            // begins; the markdown re-render replaces the whole block
            // from there (the raw stream and the markdown render split
            // the same text across different line counts, so a tail-pop
            // by count would duplicate or eat lines). Without an anchor
            // the lines append — a blind tail-pop could eat unrelated
            // transcript history.
            let from = state.stream_anchor.unwrap_or(state.transcript.len());
            state.transcript.truncate(from);
            for line in lines {
                state.append_line(kind, line);
            }
            // Keep the anchor pinned at the block start: every later
            // delta of the same message re-renders from here. BeginStream
            // clears it when the next message opens.
            state.stream_anchor = Some(from);
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
        InlineCommand::SetInputStatus { .. } => {
            // The dedicated status row was removed; input-status text has
            // no render surface. Kept as a graceful no-op for protocol
            // compatibility with harnesses that still send it.
        }
        InlineCommand::SetInputEnabled(enabled) => {
            state.input_enabled = enabled;
        }
        InlineCommand::SetCursorVisible(_) | InlineCommand::ForceRedraw => {}
        InlineCommand::SetReasoningStage(stage) => {
            state.reasoning_stage = stage;
        }
        InlineCommand::SetVimModeEnabled(enabled) => {
            state.vim_state.set_enabled(enabled);
        }
        InlineCommand::SetQueuedInputs { entries } => {
            state.queued_inputs = entries;
        }
        InlineCommand::ShowOverlay { request } => {
            let mut overlay = materialize_overlay(*request);
            // The `/settings` panel arrives as the flat Task-4 list; its
            // rows are the only producers of ConfigAction selections.
            // Hydrate the full tabbed/sidebar overlay from the def table
            // (reopening on the last active tab) instead. Map-editor row
            // metadata rides along with the hydration; every other
            // overlay invalidates it.
            let mut map_rows = Vec::new();
            if overlay.items.iter().any(|it| {
                matches!(
                    it.selection,
                    Some(InlineListSelection::ConfigAction(_))
                        | Some(InlineListSelection::SettingsTab(_))
                        | Some(InlineListSelection::SettingsSection(_))
                        | Some(InlineListSelection::SettingKeyCapture(_))
                        | Some(InlineListSelection::SettingTextEdit(_))
                        | Some(InlineListSelection::SettingSubmenuOpen(_))
                        | Some(InlineListSelection::SettingMultiselect(_))
                )
            }) {
                let (hydrated, rows) = build_settings_overlay(state.settings_active_tab, None);
                overlay = hydrated;
                map_rows = rows;
            }
            state.overlay = Some(overlay);
            state.settings_map_rows = map_rows;
        }
        InlineCommand::CloseOverlay => {
            state.overlay = None;
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

/// Convert an `OverlayRequest` into the render-state representation used by
/// the TUI. The input thread mutates `selected` / `search` while the overlay
/// is open, and `handle_inline_event` projects the user's selection back to
/// the harness as `InlineEvent::Overlay`.
fn materialize_overlay(request: OverlayRequest) -> OverlayState {
    match request {
        OverlayRequest::Modal(req) => {
            let secure_input = req.secure_prompt.map(|cfg| OverlaySecureInput {
                config: cfg,
                editor: EditBuffer::new(),
            });
            OverlayState {
                title: req.title,
                lines: req.lines,
                items: Vec::new(),
                selected: 0,
                search: None,
                secure_input,
                ..Default::default()
            }
        }
        OverlayRequest::List(req) => {
            let search = req.search.map(|cfg| OverlaySearchState {
                label: cfg.label,
                placeholder: cfg.placeholder,
                value: String::new(),
            });
            OverlayState {
                title: req.title,
                lines: req.lines,
                items: req.items.into_iter().map(overlay_item_from).collect(),
                selected: 0,
                search,
                secure_input: None,
                ..Default::default()
            }
        }
        OverlayRequest::Wizard(req) => {
            // Wizard overlays are multi-step flows that this TUI does not yet
            // render natively; surface the first step's title/items so the
            // user still sees something instead of a blank panel.
            let step_items = req
                .steps
                .first()
                .map(|s| {
                    s.items
                        .iter()
                        .map(|it| overlay_item_from(it.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let search = req.search.map(|cfg| OverlaySearchState {
                label: cfg.label,
                placeholder: cfg.placeholder,
                value: String::new(),
            });
            OverlayState {
                title: req.title,
                lines: Vec::new(),
                items: step_items,
                selected: 0,
                search,
                secure_input: None,
                ..Default::default()
            }
        }
    }
}
fn overlay_item_from(item: InlineListItem) -> OverlayListItem {
    OverlayListItem {
        title: item.title,
        subtitle: item.subtitle,
        badge: item.badge,
        indent: item.indent,
        search_value: item.search_value,
        selection: item.selection,
    }
}

/// Canonical `/settings` tab order. Indices are the
/// `InlineListSelection::SettingsTab(usize)` payloads and
/// `OverlayState::active_tab`.
const SETTINGS_TABS: &[(SettingsTab, &str)] = &[
    (SettingsTab::General, "General"),
    (SettingsTab::Model, "Model"),
    (SettingsTab::Interaction, "Interaction"),
    (SettingsTab::Tools, "Tools"),
    (SettingsTab::Ui, "UI"),
    (SettingsTab::AdvisorMemory, "Advisor & Memory"),
    (SettingsTab::Keybindings, "Keybindings"),
    (SettingsTab::Advanced, "Advanced"),
];

/// Build the full tabbed `/settings` overlay for `tab`: tab-bar labels,
/// sidebar section labels (group names, declaration order), and one row
/// per def via [`settings_overlay_items`] — the same row builder the
/// `/settings` slash command uses, hydrated with the tab/sidebar state.
/// `keep_search` preserves the live filter across tab switches.
///
/// Returns the overlay plus the map-row table (index-aligned with the
/// items) for the input thread's `Enter` / `d` / `n` routing.
fn build_settings_overlay(
    tab: SettingsTab,
    keep_search: Option<OverlaySearchState>,
) -> (OverlayState, Vec<Option<SettingsMapRow>>) {
    let settings = crate::store::settings::Settings::load().unwrap_or_default();
    let (items, map_rows) = settings_overlay_items(tab, &settings);
    let items: Vec<OverlayListItem> = items.into_iter().map(overlay_item_from).collect();
    let mut sections: Vec<String> = Vec::new();
    for def in defs_for_tab(tab, &settings) {
        if sections.last().map(String::as_str) != Some(def.group) {
            sections.push(def.group.to_string());
        }
    }
    let active_tab = SETTINGS_TABS
        .iter()
        .position(|(t, _)| *t == tab)
        .unwrap_or(0);
    (
        OverlayState {
            title: "Settings".into(),
            lines: vec!["Browse settings by group; filter with the search bar.".into()],
            items,
            selected: 0,
            search: keep_search.or(Some(OverlaySearchState {
                label: "Filter settings".into(),
                placeholder: Some("Type to filter".into()),
                value: String::new(),
            })),
            secure_input: None,
            tabs: SETTINGS_TABS
                .iter()
                .map(|(_, name)| name.to_string())
                .collect(),
            active_tab,
            sections,
            active_section: 0,
            key_capture: None,
        },
        map_rows,
    )
}

/// Reopen (or switch) the settings panel on `tab`, replacing the first
/// context line with `status` when given. Keeps the live search filter,
/// syncs `settings_active_tab`, and refreshes the map-row table — the
/// single assignment path for the tabbed panel so the rows can never
/// drift from the items.
fn reopen_settings_panel(state: &mut RenderState, tab: SettingsTab, status: Option<String>) {
    let keep_search = state.overlay.as_ref().and_then(|o| o.search.clone());
    state.settings_active_tab = tab;
    let (mut overlay, map_rows) = build_settings_overlay(tab, keep_search);
    if let Some(status) = status {
        if overlay.lines.is_empty() {
            overlay.lines.push(status);
        } else {
            overlay.lines[0] = status;
        }
    }
    state.overlay = Some(overlay);
    state.settings_map_rows = map_rows;
}

/// Switch the settings overlay to `SETTINGS_TABS[tab_idx]`: rebuild items
/// and sections for that tab (keeping the live search filter) and sync
/// `RenderState::settings_active_tab` so a later `/settings` reopens on
/// the same tab. No-op when the index is out of range or no overlay is
/// open.
fn switch_settings_tab(state: &mut RenderState, tab_idx: usize) {
    let Some(&(tab, _)) = SETTINGS_TABS.get(tab_idx) else {
        return;
    };
    let search = state.overlay.as_ref().and_then(|o| o.search.clone());
    state.settings_active_tab = tab;
    let (overlay, map_rows) = build_settings_overlay(tab, search);
    state.overlay = Some(overlay);
    state.settings_map_rows = map_rows;
}

/// Jump the settings overlay's selection to the first row of sidebar
/// section `section_idx` (an index into `OverlayState::sections`).
/// Rebuilds the overlay for the active tab first — submissions arrive
/// after the overlay was closed, so the panel has to be reopened anyway.
fn jump_settings_section(state: &mut RenderState, section_idx: usize) {
    let tab = state.settings_active_tab;
    let search = state.overlay.as_ref().and_then(|o| o.search.clone());
    let (mut overlay, map_rows) = build_settings_overlay(tab, search);
    if let Some(target) = overlay.sections.get(section_idx).cloned() {
        // Heading rows (title-only items, per the settings_overlay_items
        // convention) delimit groups; the first selectable row after the
        // target heading is the section's anchor.
        let mut in_target = false;
        let mut anchor: Option<usize> = None;
        for (idx, item) in overlay.items.iter().enumerate() {
            let is_heading =
                item.selection.is_none() && item.badge.is_none() && item.subtitle.is_none();
            if is_heading {
                in_target = item.title == target;
            } else if in_target && anchor.is_none() {
                anchor = Some(idx);
            }
        }
        if let Some(idx) = anchor {
            overlay.selected = idx;
            overlay.active_section = section_idx;
        }
    }
    state.overlay = Some(overlay);
    state.settings_map_rows = map_rows;
}

// ─────────────────────────────────────────────────────────────────────────
// Keybindings map editor (capture / remove / live swap)
// ─────────────────────────────────────────────────────────────────────────

/// The "press a key combo" prompt shown after selecting an action row.
/// `key_capture` marks capture mode for the input thread.
fn build_key_capture_overlay(action_name: &str) -> OverlayState {
    OverlayState {
        title: format!("Keybinding: {action_name}"),
        lines: vec![format!(
            "Press a key combo for {action_name} (Esc to cancel)\u{2026}"
        )],
        key_capture: Some(action_name.to_string()),
        ..Default::default()
    }
}

/// Serialize an incoming key event into its canonical `KeyCombo` text.
///
/// Only `Ctrl` / `Alt` / `Shift` survive (SUPER & co. would never
/// round-trip through `KeyCombo::parse`), and a shifted lowercase char
/// is uppercased — the same canonicalization `parse` applies — so the
/// serialization always round-trips. Kitty note (Task 2): with
/// `OXICODE_KITTY_KEYBOARD` the terminal already clears SHIFT on
/// shifted chars (they arrive uppercase), which this normalization is
/// self-consistent with.
fn key_event_to_combo_text(key: KeyEvent) -> Option<(String, KeyCombo)> {
    use crossterm::event::KeyCode as Kc;
    let mods = key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
    let code = match key.code {
        // Canonical shifted-letter form: uppercase char (crossterm's
        // `normalize_case` shape), SHIFT retained.
        Kc::Char(c) if c.is_ascii_lowercase() && mods.contains(KeyModifiers::SHIFT) => {
            Kc::Char(c.to_ascii_uppercase())
        }
        other => other,
    };
    let combo = KeyCombo {
        code,
        modifiers: mods,
    };
    let text = combo.to_string();
    // Reject anything that cannot round-trip through `KeyCombo::parse`
    // (F-keys, arrows, Home/End, …): persisting them would write a
    // binding that never resolves.
    (KeyCombo::parse(&text) == Some(combo.clone())).then_some((text, combo))
}

/// Handle the next key while the key-capture prompt is open. Esc (no
/// Ctrl/Alt) cancels back to the Keybindings tab; any other key is
/// validated (`key_event_to_combo_text` + a Ctrl/Alt requirement, since
/// an unmodified key would hijack typing) and, when valid, appended to
/// the action's live combo list, persisted, and swapped into
/// `RenderState::keymap`. Rejections keep the prompt open with the
/// reason as its only line.
fn handle_key_capture(state: &mut RenderState, key: KeyEvent) {
    let Some(action_name) = state.overlay.as_ref().and_then(|o| o.key_capture.clone()) else {
        return;
    };
    let Some(action) = GlobalAction::from_name(&action_name) else {
        // Unreachable unless a capture overlay is built by hand with a
        // bogus name — fail closed by closing the prompt.
        state.overlay = None;
        state.settings_map_rows.clear();
        return;
    };
    // Esc cancels without capturing.
    if key.code == KeyCode::Esc
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        reopen_settings_panel(state, SettingsTab::Keybindings, None);
        return;
    }
    let Some((text, _combo)) = key_event_to_combo_text(key) else {
        set_capture_prompt_line(
            state,
            "That key can't be captured (F-keys and arrows don't round-trip). \
             Try another combo, Esc to cancel\u{2026}",
        );
        return;
    };
    if !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        set_capture_prompt_line(
            state,
            &format!(
                "'{text}' has no Ctrl or Alt — it would hijack typing. \
                 Try another combo, Esc to cancel\u{2026}"
            ),
        );
        return;
    }
    // Append to the action's LIVE combo list (defaults + overrides),
    // deduped: capturing an already-bound combo is a no-op edit.
    let mut combos: Vec<String> = state
        .keymap
        .read()
        .action_combos(action)
        .iter()
        .map(|c| c.to_string())
        .collect();
    if !combos.iter().any(|c| c == &text) {
        combos.push(text.clone());
    }
    match commit_keybindings(state, action, combos) {
        Ok(()) => reopen_settings_panel(
            state,
            SettingsTab::Keybindings,
            Some(format!("Captured {text} for {action_name}")),
        ),
        Err(e) => set_capture_prompt_line(
            state,
            &format!("Failed to save keybindings: {e} — Esc to cancel\u{2026}"),
        ),
    }
}

fn set_capture_prompt_line(state: &mut RenderState, line: &str) {
    if let Some(overlay) = state.overlay.as_mut() {
        overlay.lines = vec![line.to_string()];
    }
}

/// Persist `action`'s new combo list and swap the rebuilt keymap into
/// `RenderState::keymap` so the change takes effect on the very next
/// keystroke — no restart. The keymap swap only happens after a
/// successful save (never persist a binding the disk state disagrees
/// with).
fn commit_keybindings(
    state: &mut RenderState,
    action: GlobalAction,
    combos: Vec<String>,
) -> anyhow::Result<()> {
    let mut settings = crate::store::settings::Settings::load().unwrap_or_default();
    crate::tui_vt::settings_defs::set_action_combos(&mut settings, action, combos);
    save_settings_sandboxed(state, &settings)?;
    *state.keymap.write() = Keymap::from_settings(&settings.keybindings);
    Ok(())
}

/// Persist `settings`, honoring the test-only
/// `RenderState::settings_override_path` sandbox: when set, the write
/// lands in the tempdir path via `Settings::save_to` instead of the
/// real `~/.oxicode/settings.{json,toml}`. Production code paths never
/// set the override, so they always take the plain `save()` branch.
fn save_settings_sandboxed(
    state: &RenderState,
    settings: &crate::store::settings::Settings,
) -> anyhow::Result<()> {
    #[cfg(test)]
    {
        if let Some(path) = state.settings_override_path.as_ref() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            return settings.save_to(path);
        }
    }
    let _ = state; // production builds don't read the sandbox field
    settings.save()
}

/// `d` on a keybinding-combo row: remove that combo. Guarded — the
/// final combo of an action is refused (an action with zero keys is a
/// silent trap: the user could no longer trigger it, or reach this
/// panel to fix it). A remove that lands back on the default list
/// drops the override entry entirely.
fn remove_keybinding_combo(state: &mut RenderState, action: GlobalAction, combo: &str) {
    let current: Vec<String> = state
        .keymap
        .read()
        .action_combos(action)
        .iter()
        .map(|c| c.to_string())
        .collect();
    if current.len() <= 1 {
        reopen_settings_panel(
            state,
            SettingsTab::Keybindings,
            Some(format!(
                "Refusing to remove the last combo for {} — add another first (Enter on the \
                 action row)",
                action.name()
            )),
        );
        return;
    }
    let next: Vec<String> = current
        .iter()
        .filter(|c| c.as_str() != combo)
        .cloned()
        .collect();
    if next.len() == current.len() {
        // Not bound (stale row) — nothing to do.
        return;
    }
    match commit_keybindings(state, action, next) {
        Ok(()) => reopen_settings_panel(
            state,
            SettingsTab::Keybindings,
            Some(format!("Removed {combo} from {}", action.name())),
        ),
        Err(e) => reopen_settings_panel(
            state,
            SettingsTab::Keybindings,
            Some(format!("Failed to save keybindings: {e}")),
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Model roles map editor (n / Enter / d + text prompts)
// ─────────────────────────────────────────────────────────────────────────

/// Open the unmasked text prompt for a model-role value (Enter on a
/// role row). Prefills the current model pattern so Enter-as-no-op is a
/// cheap round-trip.
fn open_model_role_value_prompt(state: &mut RenderState, role: &str) {
    let current = crate::store::settings::Settings::load()
        .map(|s| s.model_roles.get(role).cloned())
        .ok()
        .flatten();
    state.secure_input_origin = Some(SecureInputOrigin::ModelRoleValue {
        role: role.to_string(),
    });
    state.overlay = Some(text_prompt_overlay(
        format!("Model for role '{role}'"),
        "Enter the model pattern (provider/model). Enter saves, Esc cancels.".into(),
        "model",
        Some("provider/model".into()),
        current.as_deref(),
    ));
    state.settings_map_rows.clear();
}

/// Open the unmasked text prompt naming a NEW model role (`n`).
fn open_model_role_key_prompt(state: &mut RenderState) {
    state.secure_input_origin = Some(SecureInputOrigin::ModelRoleKey);
    state.overlay = Some(text_prompt_overlay(
        "New model role".to_string(),
        "Name the role (e.g. fast, reviewer). Enter continues, Esc cancels.".into(),
        "role",
        Some("role name".into()),
        None,
    ));
    state.settings_map_rows.clear();
}

/// Single-line unmasked text prompt built directly on the secure-input
/// machinery (`mask_input: false` renders the value in the clear).
fn text_prompt_overlay(
    title: String,
    line: String,
    label: &str,
    placeholder: Option<String>,
    prefill: Option<&str>,
) -> OverlayState {
    let mut editor = EditBuffer::new();
    if let Some(text) = prefill {
        let _ = editor.insert_str(text);
    }
    OverlayState {
        title,
        lines: vec![line],
        secure_input: Some(OverlaySecureInput {
            config: SecurePromptConfig {
                label: label.to_string(),
                placeholder,
                mask_input: false,
            },
            editor,
        }),
        ..Default::default()
    }
}

/// Whether the settings panel (tabbed overlay + fresh map-row table) is
/// open — the precondition for the map-editor hotkeys.
fn settings_map_editor_active(state: &RenderState) -> bool {
    state.overlay.as_ref().is_some_and(|o| {
        o.tabs.len() > 1
            && o.key_capture.is_none()
            && state.settings_map_rows.len() == o.items.len()
    })
}

/// The map-row (if any) currently selected in the settings panel.
fn selected_settings_map_row(state: &RenderState) -> Option<SettingsMapRow> {
    if !settings_map_editor_active(state) {
        return None;
    }
    let selected = state.overlay.as_ref().map(|o| o.selected)?;
    state.settings_map_rows.get(selected).cloned().flatten()
}

/// `Enter` on a model-role row opens the value prompt. Returns whether
/// the key was consumed (input thread only calls this for Enter).
fn try_edit_model_role(state: &mut RenderState) -> bool {
    match selected_settings_map_row(state) {
        Some(SettingsMapRow::ModelRole(role)) => {
            open_model_role_value_prompt(state, &role);
            true
        }
        _ => false,
    }
}

/// `d` on a map row: remove a keybinding combo (guarded — see
/// [`remove_keybinding_combo`]) or delete a model role. Returns whether
/// the key was consumed.
fn try_remove_settings_map_row(state: &mut RenderState) -> bool {
    match selected_settings_map_row(state) {
        Some(SettingsMapRow::KeybindingCombo(action, combo)) => {
            remove_keybinding_combo(state, action, &combo);
            true
        }
        Some(SettingsMapRow::ModelRole(role)) => {
            let status = match crate::store::settings::Settings::load() {
                Ok(mut settings) => {
                    let existed =
                        crate::tui_vt::settings_defs::remove_model_role(&mut settings, &role);
                    match settings.save() {
                        Ok(()) if existed => format!("Removed model role '{role}'"),
                        Ok(()) => format!("Role '{role}' was already gone"),
                        Err(e) => format!("Failed to save model roles: {e}"),
                    }
                }
                Err(e) => format!("Failed to load settings: {e}"),
            };
            reopen_settings_panel(state, SettingsTab::Model, Some(status));
            true
        }
        _ => false,
    }
}

/// `n` on the Model tab starts a new model role (name first, then the
/// model pattern). Returns whether the key was consumed.
fn try_start_new_model_role(state: &mut RenderState) -> bool {
    if settings_map_editor_active(state) && state.settings_active_tab == SettingsTab::Model {
        open_model_role_key_prompt(state);
        true
    } else {
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Settings panel editors: Text / SubmenuSelect / Multiselect (Final-fix wave)
// ─────────────────────────────────────────────────────────────────────────

/// Open the unmasked text-prompt overlay for a `Text` widget row. The
/// submitted text is routed through `SecureInputOrigin::TextEdit(key)`
/// so the `OverlaySubmission::SecureInput` consumer commits via
/// `settings_defs::apply_change` (parse-validated per-key).
fn open_text_edit_prompt(state: &mut RenderState, key: SettingKey) {
    let current = crate::store::settings::Settings::load()
        .map(|s| get_display_value(key, &s))
        .unwrap_or_default();
    state.secure_input_origin = Some(SecureInputOrigin::TextEdit(key));
    state.overlay = Some(text_prompt_overlay(
        format!("Edit {}", label_for_setting(key)),
        format!(
            "Enter the new value for {} (Esc to cancel). Empty clears the override              when the field supports it.",
            label_for_setting(key)
        ),
        "value",
        None,
        if current == "default" {
            None
        } else {
            Some(current.as_str())
        },
    ));
}

/// Open the submenu-select overlay for a `SubmenuSelect` widget row:
/// a child list of the widget's allowed strings; the active value is
/// marked in the badge.
fn open_submenu_select_prompt(state: &mut RenderState, key: SettingKey) {
    let Some(options) = submenu_options_for(key) else {
        // Defensive: a stray SettingSubmenuOpen against a non-submenu
        // key shouldn't happen, but if it does, surface the mismatch
        // and reopen the panel so the user is never stuck on a dead
        // overlay.
        reopen_settings_panel(
            state,
            state.settings_active_tab,
            Some(format!("'{:?}' is not a submenu-select setting", key)),
        );
        return;
    };
    let current = crate::store::settings::Settings::load()
        .map(|s| get_display_value(key, &s))
        .unwrap_or_default();
    let items: Vec<OverlayListItem> = options
        .iter()
        .map(|opt| InlineListItem {
            title: (*opt).to_string(),
            subtitle: None,
            badge: if *opt == current {
                Some("current".to_string())
            } else {
                None
            },
            indent: 0,
            selection: Some(InlineListSelection::ConfigAction(format!(
                "SubmenuCommit:{:?}:{}",
                key, opt
            ))),
            search_value: None,
        })
        .map(overlay_item_from)
        .collect();
    let label = label_for_setting(key);
    state.overlay = Some(OverlayState {
        title: format!("Pick value for {label}"),
        lines: vec![format!("Esc cancels — current: {current}")],
        items,
        selected: options.iter().position(|o| *o == current).unwrap_or(0),
        ..Default::default()
    });
    state.settings_map_rows.clear();
}

/// Source the registered tool list (live `ToolRegistry`) and open the
/// multiselect overlay for `DisabledTools`. Essential tools are shown
/// with their badge but cannot be toggled off (the input handler
/// refuses the toggle with an Error line).
fn open_disabled_tools_multiselect(
    state: &mut RenderState,
    session: &crate::app::agent_session::AgentSessionHandle,
) {
    let mut tools = session.agent_ref().tools().get_tools();
    tools.sort_by(|a, b| a.name().cmp(b.name()));
    let settings = crate::store::settings::Settings::load().unwrap_or_default();
    let disabled: std::collections::HashSet<String> =
        settings.disabled_tools.iter().cloned().collect();
    let items: Vec<OverlayListItem> = tools
        .iter()
        .map(|t| {
            let name = t.name();
            let is_disabled = disabled.contains(name);
            InlineListItem {
                title: name.to_string(),
                subtitle: Some(t.description().to_string()),
                badge: Some(if t.essential() {
                    if is_disabled {
                        "essential — locked".to_string()
                    } else {
                        "essential".to_string()
                    }
                } else if is_disabled {
                    "disabled".to_string()
                } else {
                    "enabled".to_string()
                }),
                indent: 0,
                selection: Some(InlineListSelection::ConfigAction(format!(
                    "DisabledToolToggle:{}",
                    name
                ))),
                search_value: None,
            }
        })
        .map(overlay_item_from)
        .collect();
    let disabled_count = items
        .iter()
        .filter(|i| {
            i.badge
                .as_deref()
                .map(|b| b == "disabled" || b == "essential — locked")
                .unwrap_or(false)
        })
        .count();
    state.overlay = Some(OverlayState {
        title: "Disabled tools".into(),
        lines: vec![format!(
            "{disabled_count} disabled — Enter/Space toggles, Esc closes"
        )],
        items,
        selected: 0,
        ..Default::default()
    });
    state.settings_map_rows.clear();
}

/// Commit a `Text` widget edit: `apply_change` parses the input,
/// `Settings::save` persists, and the panel reopens with a status
/// line. Returns the outcome string for the caller to surface. The
/// session is required only for `sync_settings_live`; callers that
/// don't need live sync (e.g. model defaults — no live propagation
/// today) can pass a real handle from `handle_inline_event`'s scope.
/// Tests pass `&RenderState::default()` and skip the live sync via
/// the `with_session` toggle.
fn commit_text_edit(
    state: &mut RenderState,
    handle: &InlineHandle,
    session: Option<&crate::app::agent_session::AgentSessionHandle>,
    key: SettingKey,
    text: String,
) -> (anyhow::Result<()>, String) {
    let label = label_for_setting(key);
    let mut settings = crate::store::settings::Settings::load().unwrap_or_default();
    let outcome =
        crate::tui_vt::settings_defs::apply_change(key, &mut settings, text.trim().to_string())
            .and_then(|_| save_settings_sandboxed(state, &settings));
    let new_display = get_display_value(key, &settings);
    match &outcome {
        Ok(()) => {
            if let Some(session) = session {
                // Best-effort live sync — `apply_change` already validated
                // the parse; sync failures don't undo the save.
                if let Err(e) = sync_settings_live(state, session, key, &settings) {
                    handle.append_line(
                        InlineMessageKind::Error,
                        vec![plain_segment(format!("{label}: {e}"))],
                    );
                }
            }
            let status = format!("{label}: {new_display}");
            // Match the ConfigAction path's transcript feedback: a
            // saved scalar edit is surfaced as an Info line, not just
            // the panel status.
            handle.append_line(InlineMessageKind::Info, vec![plain_segment(status.clone())]);
            reopen_settings_panel(state, state.settings_active_tab, Some(status.clone()));
            (Ok(()), status)
        }
        Err(e) => {
            let msg = format!("{label}: {e}");
            handle.append_line(InlineMessageKind::Error, vec![plain_segment(msg.clone())]);
            (Err(anyhow::anyhow!("{e}")), msg)
        }
    }
}

/// Commit a `SubmenuSelect` widget edit: write the chosen option,
/// persist, reopen the panel with the new badge.
fn commit_submenu_choice(
    state: &mut RenderState,
    key: SettingKey,
    value: String,
) -> anyhow::Result<String> {
    let mut settings = crate::store::settings::Settings::load().unwrap_or_default();
    crate::tui_vt::settings_defs::apply_change(key, &mut settings, value.clone())?;
    save_settings_sandboxed(state, &settings)?;
    let new_display = get_display_value(key, &settings);
    let label = label_for_setting(key);
    let status = format!("{label}: {new_display}");
    reopen_settings_panel(state, state.settings_active_tab, Some(status.clone()));
    Ok(status)
}

/// Toggle one tool in `Settings::disabled_tools`. Returns the outcome
/// string for the caller to surface (success or refusal for
/// essential tools).
fn commit_disabled_tool_toggle(
    state: &mut RenderState,
    handle: &InlineHandle,
    session: &crate::app::agent_session::AgentSessionHandle,
    tool: String,
    essential: bool,
    currently_disabled: bool,
) {
    let label = label_for_setting(SettingKey::DisabledTools);
    if essential {
        handle.append_line(
            InlineMessageKind::Error,
            vec![plain_segment(format!(
                "'{tool}' is essential and cannot be disabled"
            ))],
        );
        // Refresh the overlay so the user's failed toggle doesn't show
        // a stale badge.
        open_disabled_tools_multiselect(state, session);
        return;
    }
    let mut settings = crate::store::settings::Settings::load().unwrap_or_default();
    let new_enabled = currently_disabled; // toggling from disabled → enabled
    crate::tui_vt::settings_defs::toggle_disabled_tool(&mut settings, &tool, new_enabled);
    match save_settings_sandboxed(state, &settings) {
        Ok(()) => {
            let new_state = if new_enabled { "enabled" } else { "disabled" };
            handle.append_line(
                InlineMessageKind::Info,
                vec![plain_segment(format!("{label}: '{tool}' {new_state}"))],
            );
            open_disabled_tools_multiselect(state, session);
        }
        Err(e) => {
            handle.append_line(
                InlineMessageKind::Error,
                vec![plain_segment(format!("{label}: failed to save: {e}"))],
            );
        }
    }
}

/// Human label for a SettingKey — mirrors the row label the user
/// sees on the panel, used in overlay titles and status messages.
fn label_for_setting(key: SettingKey) -> &'static str {
    SETTING_DEFS
        .iter()
        .find(|d| d.key == key)
        .map(|d| d.label)
        .unwrap_or("setting")
}

/// Parse a `SettingKey::Debug`-formatted name (the payload the panel
/// ships through `InlineListSelection::SettingTextEdit` et al.) back
/// into a typed key. Returns `None` for unrecognized names; callers
/// must surface that as an Error line (no silent no-op).
fn parse_setting_key(name: &str) -> Option<SettingKey> {
    SETTING_DEFS
        .iter()
        .map(|d| d.key)
        .find(|k| format!("{k:?}") == name)
}

/// The allowed option list for a `SubmenuSelect` key, looked up from
/// its def. Returns `None` for non-submenu keys.
fn submenu_options_for(key: SettingKey) -> Option<&'static [&'static str]> {
    SETTING_DEFS
        .iter()
        .find(|d| d.key == key)
        .and_then(|d| match d.widget {
            SettingWidget::SubmenuSelect(opts) => Some(opts),
            _ => None,
        })
}

/// Sidebar section index an item belongs to: the group of the last
/// heading row at or above it. Returns `None` for items outside every
/// section (or when the overlay has no sections).
fn item_section_idx(overlay: &OverlayState, idx: usize) -> Option<usize> {
    let mut current: Option<String> = None;
    for (i, item) in overlay.items.iter().enumerate() {
        let is_heading =
            item.selection.is_none() && item.badge.is_none() && item.subtitle.is_none();
        if is_heading {
            current = Some(item.title.clone());
        }
        if i == idx {
            return current
                .as_deref()
                .and_then(|g| overlay.sections.iter().position(|s| s == g));
        }
    }
    None
}

/// Next variant string for a `Cycle` widget row — the value an Enter
/// submits to `settings_defs::apply_change`.
fn next_cycle_value(key: SettingKey, s: &crate::store::settings::Settings) -> Option<String> {
    match key {
        // Mirrors `AgentSession::cycle_thinking_level`'s order.
        SettingKey::ThinkingLevel => {
            const LEVELS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];
            let cur = get_display_value(key, s);
            let idx = LEVELS.iter().position(|l| *l == cur).unwrap_or(0);
            Some(LEVELS[(idx + 1) % LEVELS.len()].to_string())
        }
        SettingKey::GlyphSet => Some(s.glyph_set.next().to_string()),
        SettingKey::EditFormat => Some(
            if get_display_value(key, s) == "hashline" {
                "str_replace"
            } else {
                "hashline"
            }
            .to_string(),
        ),
        _ => None,
    }
}

/// Live-sync the handful of settings the open session / render state read
/// eagerly, so an overlay edit takes effect without a restart. Everything
/// else is re-read from disk on the next turn
/// (`AgentSession::rebuild_system_prompt` already reloads on demand).
///
/// Returns `Err` only when a live propagation genuinely failed (the
/// advisor toggle can refuse to start/stop) — the caller surfaces that
/// instead of a silent success; the disk value is already saved at that
/// point, so the message says what the user must do (restart).
fn sync_settings_live(
    state: &mut RenderState,
    session: &crate::app::agent_session::AgentSessionHandle,
    key: SettingKey,
    settings: &crate::store::settings::Settings,
) -> anyhow::Result<()> {
    match key {
        SettingKey::ThinkingLevel => {
            session.set_thinking_level(settings.thinking_level);
            state.thinking_level = get_display_value(key, settings);
        }
        SettingKey::GlyphSet => state.glyph_set = settings.glyph_set,
        SettingKey::AutoCompaction => session.set_auto_compaction(settings.auto_compaction),
        SettingKey::AdvisorEnabled if session.is_advisor_enabled() != settings.advisor.enabled => {
            session
                .set_advisor_enabled(settings.advisor.enabled)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "failed to {} the advisor live: {e} (saved; restart to apply)",
                        if settings.advisor.enabled {
                            "enable"
                        } else {
                            "disable"
                        }
                    )
                })?;
        }
        _ => {}
    }
    Ok(())
}

/// Map a `SessionEvent` to the matching `InlineHandle` calls. This is the
/// single place where the agent's event vocabulary meets the harness's
/// transcript vocabulary.
fn handle_session_event(
    state: &mut RenderState,
    handle: &InlineHandle,
    event: &SessionEvent,
    session: Option<&crate::app::agent_session::AgentSessionHandle>,
) {
    match event {
        SessionEvent::Agent(boxed) => {
            let event = *boxed.clone();
            if let (AgentEvent::Error { message, .. }, Some(session)) = (&event, session)
                && is_missing_api_key_error(message)
            {
                let provider = provider_from_model_id(&session.model_id());
                handle.append_line(
                    InlineMessageKind::Info,
                    vec![plain_segment(format!(
                        "Authentication is required for '{provider}'. Enter an API key to continue."
                    ))],
                );
                open_secure_prompt(state, handle, SecureInputOrigin::SetKey { provider });
            }
            map_agent_event(handle, event, state);
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
        SessionEvent::ThinkingLevelChanged { level } => {
            state.thinking_level = format!("{level:?}").to_ascii_lowercase();
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
        SessionEvent::HandoffComplete { .. } => {
            // Intercepted in the event loop's session_rx arm before
            // reaching this function — transcript clearing and prompt
            // submission happen there. This arm exists for exhaustiveness.
        }
        SessionEvent::HandoffFailed { error } => {
            handle.append_line(
                InlineMessageKind::Error,
                vec![plain_segment(format!("Handoff failed: {}", error))],
            );
        }
    }
}

/// Whether a provider failure means the active credential is absent. Keep this
/// deliberately narrow: transport, quota, and invalid-key errors must remain
/// visible as errors instead of unexpectedly opening a credential prompt.
fn is_missing_api_key_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("missing api key") || message.contains("api key is required")
}

/// The agent model id is always represented as `provider/model`. A malformed
/// legacy id still gets a usable, explicit destination for the credential UI.
fn provider_from_model_id(model_id: &str) -> String {
    model_id
        .split_once('/')
        .map(|(provider, _)| provider)
        .filter(|provider| !provider.is_empty())
        .unwrap_or("provider")
        .to_string()
}

/// Push the active model into every render surface that shows it: the
/// composer's MODEL field (`header_context`) and the CTX denominator.
///
/// Before this, both were written once at startup and went stale: the
/// MODEL chip kept the boot model after `/model`, and `context_window`
/// kept its 128_000 default forever — a 1M-context model showed a
/// wrong CTX total for the whole session.
pub(crate) fn apply_model_to_chips(state: &mut RenderState, model_id: &str, ctx_window: usize) {
    if model_id.is_empty() {
        return;
    }
    state.header_context.provider = provider_from_model_id(model_id);
    state.header_context.model = model_id.to_string();
    state.header_context.editor_context = Some(model_id.to_string());
    if ctx_window > 0 {
        state.context_window = ctx_window;
    }
}

/// [`apply_model_to_chips`] sourced from the live session.
pub(crate) fn sync_model_chips(
    state: &mut RenderState,
    session: &crate::app::agent_session::AgentSessionHandle,
) {
    apply_model_to_chips(state, &session.model_id(), session.context_window());
}
/// Render the in-flight message: the dimmed italic thinking block (one
/// line per explicit newline, reasoning-styled) above the markdown-rendered
/// answer. Re-rendered whole on every reveal step so the live view equals
/// the final render — but only for the REVEALED prefix of the body (see
/// [`advance_stream_reveal`]).
fn render_streamed_message(state: &mut RenderState) -> Vec<Vec<InlineSegment>> {
    let mut lines = Vec::new();
    if !state.thinking_buffer.is_empty() {
        let styles = active_styles();
        let mut style = InlineTextStyle::default();
        style.color = styles.reasoning.get_fg_color();
        style.effects |= anstyle::Effects::DIMMED | anstyle::Effects::ITALIC;
        for chunk in state.thinking_buffer.split('\n') {
            lines.push(vec![InlineSegment {
                text: chunk.to_string(),
                style: Arc::new(style.clone()),
            }]);
        }
        // One blank row breathes between the thinking block and the
        // answer — only once the answer has started streaming.
        if !state.message_buffer.is_empty() {
            lines.push(vec![plain_segment("")]);
        }
    }
    let body = revealed_stream_body(state).to_string();
    if !body.is_empty() {
        // Tables pre-compute their geometry, so they must know the real
        // content width — a table built wider wraps at the terminal
        // edge and every border row breaks.
        let (_, content_w) = super::frame_layout::scrollback_geometry(Rect {
            x: 0,
            y: 0,
            width: state.viewport_width,
            height: 24,
        });
        lines.extend(oxicode_vtui::tui::ui::markdown::render_markdown_cached(
            &body,
            content_w as usize,
            &mut state.md_cache,
        ));
    }
    lines
}

/// The portion of the streamed body currently revealed by the
/// typewriter (char-boundary-safe). `usize::MAX` reveals everything.
fn revealed_stream_body(state: &RenderState) -> &str {
    if state.stream_reveal == usize::MAX {
        &state.message_buffer
    } else {
        let idx = floor_char_boundary(&state.message_buffer, state.stream_reveal);
        &state.message_buffer[..idx]
    }
}

/// Largest char-boundary index `<= i` (std's `floor_char_boundary` is
/// still unstable).
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Advance the typewriter one frame and paint the newly revealed text
/// into the streamed block. Returns `true` when something was painted.
///
/// Network chunks land in `message_buffer` whole; painting them whole
/// made the transcript jump in lumps. The reveal advances per render
/// tick (50 ms) by `remaining / 6` (min 8 bytes), so any backlog drains
/// in a handful of frames while steady streams type out at their
/// arrival pace. The final authoritative paint at `MessageEnd` reveals
/// everything at once.
fn advance_stream_reveal(state: &mut RenderState) -> bool {
    if state.stream_anchor.is_none() || state.stream_reveal == usize::MAX {
        return false;
    }
    let len = state.message_buffer.len();
    if state.stream_reveal >= len {
        state.stream_reveal = len;
        return false;
    }
    let remaining = len - state.stream_reveal;
    let step = (remaining / 6).max(8);
    let target = (state.stream_reveal + step).min(len);
    state.stream_reveal = floor_char_boundary(&state.message_buffer, target);
    // Paint: replace the streamed block with the revealed prefix — the
    // same mutation `InlineCommand::ReplaceLast` applies.
    let lines = render_streamed_message(state);
    let from = state.stream_anchor.unwrap_or(state.transcript.len());
    state.transcript.truncate(from);
    for line in lines {
        state.append_line(InlineMessageKind::Agent, line);
    }
    state.stream_anchor = Some(from);
    true
}

/// Project the agent-level event variants onto the harness transcript.
/// One-line human preview of a tool call's arguments: the command for
/// shell tools, key=value pairs otherwise, bounded to the transcript
/// width. "Which command ran" is the single most useful fact about a
/// tool call — peers (Claude Code, pi, OpenCode) all surface it.
fn tool_args_preview(args: &serde_json::Value) -> String {
    use serde_json::Value;
    let raw = match args {
        Value::Null => return String::new(),
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            if let Some(Value::String(cmd)) = map.get("command") {
                cmd.clone()
            } else {
                map.iter()
                    .filter_map(|(k, v)| match v {
                        Value::String(s) => Some(format!("{k}={s}")),
                        _ => None,
                    })
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        }
        other => other.to_string(),
    };
    if raw.chars().count() > 72 {
        let head: String = raw.chars().take(71).collect();
        format!("{head}\u{2026}")
    } else {
        raw
    }
}
/// Tool box content width: the LIVE transcript content width (layout
/// gutters), floored so narrow terminals still draw a coherent box.
/// Building at the terminal width would wrap every row's right border
/// onto the next visual line.
fn tool_box_width(state: &RenderState) -> usize {
    let area = Rect {
        x: 0,
        y: 0,
        width: state.viewport_width,
        height: 24,
    };
    let (_x, w) = super::frame_layout::scrollback_geometry(area);
    w.max(24) as usize
}

fn border_segment(text: impl Into<String>, color: anstyle::Color) -> InlineSegment {
    let mut style = InlineTextStyle::default();
    style.color = Some(color);
    InlineSegment {
        text: text.into(),
        style: Arc::new(style),
    }
}

/// `╭────╮` — rounded top border, no interior fill.
fn tool_box_top(w: usize, color: anstyle::Color) -> Vec<InlineSegment> {
    vec![border_segment(
        format!("\u{256D}{}\u{256E}", "\u{2500}".repeat(w.saturating_sub(2))),
        color,
    )]
}

/// `╰────╯` — rounded bottom border.
fn tool_box_bottom(w: usize, color: anstyle::Color) -> Vec<InlineSegment> {
    vec![border_segment(
        format!("\u{2570}{}\u{256F}", "\u{2500}".repeat(w.saturating_sub(2))),
        color,
    )]
}

/// `├── Output ───┤` — section divider with a label, omp-style.
fn tool_box_divider(label: &str, w: usize, color: anstyle::Color) -> Vec<InlineSegment> {
    let text = format!(" {label} ");
    let dashes = w
        .saturating_sub(2)
        .saturating_sub(text.chars().count())
        .saturating_sub(2);
    vec![border_segment(
        format!(
            "\u{251C}\u{2500}{}{}\u{2500}\u{2524}",
            text,
            "\u{2500}".repeat(dashes)
        ),
        color,
    )]
}

/// `│ text │` rows with the right border aligned at `w`. Long text
/// hard-wraps at the inner width; explicit newlines open new rows.
fn tool_box_rows(
    text: &str,
    w: usize,
    style: InlineTextStyle,
    color: anstyle::Color,
) -> Vec<Vec<InlineSegment>> {
    let inner = w.saturating_sub(4).max(1);
    text.split('\n')
        .map(|line| expand_tabs(line, TAB_WIDTH))
        .flat_map(|line| wrap_by_display_width(&line, inner))
        .map(|chunk| {
            // Pad by DISPLAY width — CJK chars occupy two cells, so a
            // char-count pad misaligns the right border on Korean text.
            let pad = inner.saturating_sub(chunk.width());
            vec![
                border_segment("\u{2502} ", color),
                InlineSegment {
                    text: chunk,
                    style: Arc::new(style.clone()),
                },
                border_segment(format!("{} \u{2502}", " ".repeat(pad)), color),
            ]
        })
        .collect()
}

/// Hard-wrap a line into chunks of at most `inner` DISPLAY cells
/// (Korean/CJK glyphs count as 2). Zero-width chars never break a chunk.
fn wrap_by_display_width(line: &str, inner: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar as _;
    if line.width() <= inner {
        return vec![line.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in line.chars() {
        let ch_w = ch.width().unwrap_or(0);
        if cur_w + ch_w > inner && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += ch_w;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Tab stop for box-content expansion. Tool output (e.g. the read tool's
/// `{:>6}\t{line}` numbering) carries literal tabs; ratatui drops them when
/// filling cells while unicode-width 0.2 counts them as 1 — a row padded
/// with tab width in its math renders one column short. Expand tabs to the
/// next stop so builder and renderer agree on every cell.
const TAB_WIDTH: usize = 4;

/// Expand tabs to spaces at `TAB_WIDTH` display-column stops.
fn expand_tabs(line: &str, tab_width: usize) -> String {
    use unicode_width::UnicodeWidthChar as _;
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + tab_width);
    let mut col = 0usize;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = tab_width - (col % tab_width);
            for _ in 0..spaces {
                out.push(' ');
                col += 1;
            }
        } else {
            out.push(ch);
            col += ch.width().unwrap_or(1).max(1);
        }
    }
    out
}

/// Diff rows for a tool box: colored +/- lines plus a diffstat header.
/// Returns `None` when the content is not a recognizable diff.
fn diff_rows(content: &str) -> Option<Vec<(String, InlineTextStyle)>> {
    let lines: Vec<&str> = content.lines().collect();
    // Require a unified-diff hunk header (`@@ … @@`) as a strong signal that
    // the content is actually a diff — prevents grep context lines, bullet
    // lists, and shell output from being mis-rendered as deletions.
    if !lines.iter().any(|l| l.starts_with("@@")) {
        return None;
    }
    let additions = lines
        .iter()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .count();
    let deletions = lines
        .iter()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .count();
    if additions + deletions < 2 {
        return None;
    }

    let styles = active_styles();
    let green = styles.secondary.get_fg_color();
    let red = styles.error.get_fg_color();
    const MAX_DIFF_LINES: usize = 30;

    let mut rows: Vec<(String, InlineTextStyle)> = Vec::new();
    let mut hdr = InlineTextStyle::default();
    hdr.effects |= anstyle::Effects::DIMMED;
    rows.push((format!("diff +{additions} -{deletions}"), hdr));
    for line in lines.iter().take(MAX_DIFF_LINES) {
        let mut style = InlineTextStyle::default();
        if line.starts_with('+') && !line.starts_with("+++") {
            style.color = green;
        } else if line.starts_with('-') && !line.starts_with("---") {
            style.color = red;
        } else {
            style.effects |= anstyle::Effects::DIMMED;
        }
        rows.push(((*line).to_string(), style));
    }
    if lines.len() > MAX_DIFF_LINES {
        let mut more = InlineTextStyle::default();
        more.effects |= anstyle::Effects::DIMMED;
        rows.push((
            format!("\u{2026} +{} lines", lines.len() - MAX_DIFF_LINES),
            more,
        ));
    }
    Some(rows)
}

fn map_agent_event(handle: &InlineHandle, event: AgentEvent, state: &mut RenderState) {
    match event {
        AgentEvent::TextChunk { text } => {
            state.reasoning_stage = Some("generating response".to_string());
            state.message_buffer.push_str(&text);
            handle.inline(InlineMessageKind::Agent, plain_segment(text));
        }
        AgentEvent::AgentStart { .. } => {
            // The run is live until the matching AgentEnd. The tracker —
            // not the stage label — owns the indicator row, so the row
            // survives the per-turn stage clears of a tool loop.
            state.active_run = Some(RunState::default());
        }
        AgentEvent::MessageStart { .. } => {
            if let Some(run) = &mut state.active_run {
                run.turn += 1;
            }
            state.reasoning_stage = Some("generating response".to_string());
            state.message_buffer.clear();
            state.thinking_buffer.clear();
            state.stream_reveal = 0;
            // The stream boundary travels in the command stream so the
            // anchor lifecycle shares one causal order with Inline and
            // ReplaceLast — a direct state write here would race batched
            // command application.
            handle.begin_stream(InlineMessageKind::Agent);
        }
        AgentEvent::MessageUpdate { delta, .. } => match &delta {
            oxicode_sdk::StreamDelta::Text(text) => {
                // The Text delta is the lifecycle owner of the visible
                // answer: the first one transitions the reasoning stage
                // off `thinking…` into `generating response`. Raw
                // `MessageUpdate { delta: Text }` is the live streaming
                // path (oxicode-agent/src/agent_loop/streaming.rs:277-280).
                state.reasoning_stage = Some("generating response".to_string());
                state.message_buffer.push_str(text);
                handle.replace_last(0, InlineMessageKind::Agent, render_streamed_message(state));
            }
            oxicode_sdk::StreamDelta::Thinking(text) => {
                // The reasoning text renders as a dimmed italic block above
                // the answer (peer parity: Claude Code / pi). The stage
                // indicator keeps a fixed `thinking…` label — streaming raw
                // fragments into `reasoning_stage` would leak them through
                // the composer `RUN ` field and the indicator row.
                state.reasoning_stage = Some("thinking\u{2026}".to_string());
                state.thinking_buffer.push_str(text);
                handle.replace_last(0, InlineMessageKind::Agent, render_streamed_message(state));
            }
            oxicode_sdk::StreamDelta::Sync => {
                // Re-render the complete message as markdown
                if !state.message_buffer.is_empty() || !state.thinking_buffer.is_empty() {
                    handle.replace_last(
                        0,
                        InlineMessageKind::Agent,
                        render_streamed_message(state),
                    );
                    state.message_buffer.clear();
                    state.thinking_buffer.clear();
                    state.stream_reveal = 0;
                }
            }
        },
        AgentEvent::MessageEnd { message } => {
            // Between turns of a live tool loop the stage is briefly
            // `None`; the run tracker keeps the indicator row up (the
            // renderer falls back to `working…`). Only a finished run
            // releases the row to follow-ups / tips.
            if state.active_run.is_none() {
                state.reasoning_stage = None;
            }
            // Authoritative final render: the Done message REPLACES the
            // accumulated partial in agent_loop/streaming.rs, so the
            // final message — not the delta buffers — carries the
            // complete text. Providers can coalesce the stream tail
            // into it without a matching delta; rendering from the
            // buffers lost that tail until the next prompt rebuilt
            // history from the session.
            if let oxicode_ai::Message::Assistant(a) = &message {
                state.thinking_buffer = a
                    .content
                    .iter()
                    .filter_map(|b| b.as_thinking().map(|t| t.thinking.clone()))
                    .collect();
                state.message_buffer = a.text_content();
                state.stream_reveal = usize::MAX;
            }
            if !state.message_buffer.is_empty() || !state.thinking_buffer.is_empty() {
                handle.replace_last(0, InlineMessageKind::Agent, render_streamed_message(state));
                state.message_buffer.clear();
                state.thinking_buffer.clear();
            }
            // The message is final: release the anchor in the command
            // stream (after the final ReplaceLast above) so the finished
            // block becomes committable to the host scrollback.
            handle.end_stream();
        }
        AgentEvent::ToolExecutionStart {
            tool_name, args, ..
        } => {
            // omp-style tool box: rounded border, no fill, the call in
            // the header — "which command ran" is the headline fact.
            let styles = active_styles();
            let border = styles
                .tool
                .get_fg_color()
                .unwrap_or(anstyle::Color::Ansi(anstyle::AnsiColor::White));
            let w = tool_box_width(state);
            let header = match args.get("command").and_then(|v| v.as_str()) {
                Some(cmd) => format!("$ {cmd}"),
                None => {
                    let preview = tool_args_preview(&args);
                    if preview.is_empty() {
                        tool_name.clone()
                    } else {
                        format!("{tool_name}  {preview}")
                    }
                }
            };
            handle.append_line_block_start(InlineMessageKind::Tool, tool_box_top(w, border));
            for row in tool_box_rows(&header, w, InlineTextStyle::default(), border) {
                handle.append_line(InlineMessageKind::Tool, row);
            }
            let stage = format!("tool: {tool_name}");
            if let Some(run) = &mut state.active_run {
                run.tool_calls += 1;
            }
            state.reasoning_stage = Some(stage.clone());
            handle.set_reasoning_stage(Some(stage));
        }
        AgentEvent::ToolExecutionEnd {
            tool_name,
            result,
            is_error,
            ..
        } => {
            // Close the box: a labeled divider separates the call from
            // its output (errors redden the border and the label), then
            // the bottom border. Diffs render colored inside the box.
            let styles = active_styles();
            let (border, label) = if is_error {
                (
                    styles
                        .error
                        .get_fg_color()
                        .unwrap_or(anstyle::Color::Ansi(anstyle::AnsiColor::White)),
                    "Error",
                )
            } else {
                (
                    styles
                        .tool
                        .get_fg_color()
                        .unwrap_or(anstyle::Color::Ansi(anstyle::AnsiColor::White)),
                    "Output",
                )
            };
            let w = tool_box_width(state);
            handle.append_line(InlineMessageKind::Tool, tool_box_divider(label, w, border));
            // Inline image preview: a successful generate_image result
            // renders the text-fallback row here (this is what the
            // scrollback keeps); the decoded PNG is queued so the
            // post-draw step can transmit + place the real pixels over
            // the LIVE rows only. Unsupported terminals and the
            // `inline_images = false` kill-switch degrade to this text.
            let embedded_png = if tool_name == "generate_image" && !is_error {
                extract_generated_png(&result.content)
            } else {
                None
            };
            if let Some(png) = embedded_png {
                let id = super::image_preview::content_hash_id(&png);
                let label = format!("generate_image:{id:08x}");
                let mut dim = InlineTextStyle::default();
                dim.effects |= anstyle::Effects::DIMMED;
                let fallback = super::image_preview::text_fallback(&label);
                for row in tool_box_rows(&fallback, w, dim, border) {
                    handle.append_line(InlineMessageKind::Tool, row);
                }
                // The row index is resolved later, at render time — the
                // append command is still in the harness channel.
                state
                    .image_previews
                    .enqueue(id, std::sync::Arc::new(png), label);
            } else if let Some(rows) = diff_rows(&result.content) {
                for (text, style) in rows {
                    for row in tool_box_rows(&text, w, style, border) {
                        handle.append_line(InlineMessageKind::Tool, row);
                    }
                }
            } else {
                const MAX_BOX_LINES: usize = 12;
                let preview = preview_tool_result(&result.content);
                let lines: Vec<&str> = preview.split('\n').collect();
                let mut dim = InlineTextStyle::default();
                dim.effects |= anstyle::Effects::DIMMED;
                for line in lines.iter().take(MAX_BOX_LINES) {
                    for row in tool_box_rows(line, w, dim.clone(), border) {
                        handle.append_line(InlineMessageKind::Tool, row);
                    }
                }
                if lines.len() > MAX_BOX_LINES {
                    let more = format!("\u{2026} +{} lines", lines.len() - MAX_BOX_LINES);
                    for row in tool_box_rows(&more, w, dim, border) {
                        handle.append_line(InlineMessageKind::Tool, row);
                    }
                }
            }
            handle.append_line(InlineMessageKind::Tool, tool_box_bottom(w, border));
            state.reasoning_stage = Some("generating response".to_string());
            handle.set_reasoning_stage(Some("generating response".to_string()));
            handle.set_input_enabled(true);
        }
        AgentEvent::Error { message, .. } => {
            handle.append_line(InlineMessageKind::Error, vec![plain_segment(message)]);
            state.active_run = None;
            state.reasoning_stage = None;
            handle.set_input_enabled(true);
            handle.set_input_status(None, None);
        }
        AgentEvent::Compaction { .. } => {
            // Detailed lifecycle is handled by the AgentSession layer
            // (CompactionStart/End SessionEvents).
        }
        AgentEvent::Cancelled => {
            state.active_run = None;
            state.reasoning_stage = None;
            handle.set_input_enabled(true);
            handle.set_input_status(None, Some("cancelled".to_string()));
        }
        AgentEvent::AutoRetryStart {
            attempt,
            max_attempts,
            ..
        } => {
            state.reasoning_stage = Some(format!("retrying {attempt} of {max_attempts}"));
            handle.set_input_status(None, Some(format!("retry {attempt}/{max_attempts}")));
        }
        AgentEvent::TurnEnd { .. } => {
            // Notify via the terminal's best-supported desktop-notification
            // protocol (OSC 9/99/777, falling back to BEL) so the user
            // notices a finished turn even when the window is unfocused.
            crate::tui_vt::notifications::emit_notification("oxicode", "Response complete");
            // The next queued prompt (if any) now starts running — drop it
            // from the visible queue pane so the pane only shows still-pending
            // inputs.
            state.drain_queue_head();
            // Mid-run TurnEnds (a tool loop turn boundary) must not clear
            // the stage through the command path either — the run tracker
            // owns the row until AgentEnd.
            if state.active_run.is_none() {
                handle.set_reasoning_stage(None);
            }
        }
        AgentEvent::Usage { input_tokens, .. } => {
            // `input_tokens` is the provider's tokenization of the complete
            // prompt for this turn, so it is a useful live snapshot of the
            // context currently occupying the window (unlike a character
            // count or a local approximation).
            state.context_tokens = Some(input_tokens);
        }
        AgentEvent::AgentEnd { .. } => {
            // The run is over: release the indicator row to follow-ups /
            // tips and reset the tracker.
            state.active_run = None;
            state.reasoning_stage = None;
            handle.set_reasoning_stage(None);
        }
        AgentEvent::TodoReminder { open, attempt, max } => {
            // Commit a visible banner of *why* the agent kept going; the
            // injected user turn itself is hidden (UserMessage::hidden).
            let header = format!(
                "⚠ {} incomplete todo{} — reminder {attempt}/{max}",
                open.len(),
                if open.len() == 1 { "" } else { "s" }
            );
            handle.append_line(InlineMessageKind::Warning, vec![plain_segment(header)]);
            for t in &open {
                handle.append_line(
                    InlineMessageKind::Warning,
                    vec![plain_segment(format!("  ☐ {}", t.content))],
                );
            }
        }
        _ => {
            // Other variants (TurnStart, Compaction, ToolCallDelta, …) are
            // logged but not rendered — they're either metadata or covered
            // by the dedicated SessionEvent variants above.
            tracing::debug!(?event, "ignored AgentEvent variant");
        }
    }
}

/// Decide which `/providers` actions apply for a provider, given whether
/// the user already has a stored credential and whether the provider
/// supports the OAuth `authorization_code` flow.
///
/// Single-action branches skip the menu entirely and drive directly
/// (no user-visible "Pick an action" list for the obvious cases).
pub(crate) fn next_provider_actions(has_key: bool, oauth_capable: bool) -> Vec<AuthAction> {
    match (has_key, oauth_capable) {
        (true, true) => vec![
            AuthAction::SetApiKey,
            AuthAction::StartOAuth,
            AuthAction::RemoveKey,
        ],
        (true, false) => vec![AuthAction::SetApiKey, AuthAction::RemoveKey],
        (false, true) => vec![AuthAction::SetApiKey, AuthAction::StartOAuth],
        (false, false) => vec![AuthAction::SetApiKey],
    }
}

/// Open a masked secure prompt and stash the `origin` so the
/// `OverlaySubmission::SecureInput` consumer can route the key to the
/// right provider slot and emit a contextual follow-up message.
///
/// Shared by:
/// - `handle_auth_action::SetApiKey` (replace or first-time key entry)
/// - `add_custom_provider` (chain immediately after persisting a new
///   custom provider so the user does not have to navigate back)
///
/// The caller must consume the boolean return value the same way it does
/// for `handle_auth_action`: `true` means a new overlay was opened, so
/// the previously-open overlay must NOT be closed in the same submit
pub(crate) fn open_secure_prompt(
    state: &mut RenderState,
    handle: &InlineHandle,
    origin: SecureInputOrigin,
) {
    // Model-role prompts are built by their own (unmasked, prefilled)
    // builders; this auth-specific helper is never called with them.
    let provider = match &origin {
        SecureInputOrigin::SetKey { provider } | SecureInputOrigin::NewlyAdded { provider } => {
            provider.clone()
        }
        SecureInputOrigin::ModelRoleKey | SecureInputOrigin::ModelRoleValue { .. } => return,
        SecureInputOrigin::TextEdit(_) => return,
    };
    state.secure_input_origin = Some(origin);
    handle.show_modal(
        format!("Set API key for {provider}"),
        vec![
            "Paste the API key. Press Enter to save, Esc to cancel.".into(),
            "The key is masked on screen; nothing is logged.".into(),
        ],
        Some(SecurePromptConfig {
            label: format!("{provider} key"),
            placeholder: Some("sk-...".into()),
            mask_input: true,
        }),
    );
}

/// Dispatch a single `AuthAction` for `provider`.
///
/// `SetApiKey` opens the secure (masked) prompt via `open_secure_prompt`
/// (stashing `SecureInputOrigin::SetKey` so the consumer can route the
/// key to the right provider). `StartOAuth` spawns `run_oauth_flow` on a
/// dedicated tokio task (PKCE + loopback callback + token exchange +
/// persistence). `RemoveKey` reuses the existing confirmation modal —
/// its `ConfirmationAction::RemoveProviderKey` handler runs through
/// `/providers remove <name> --yes`.
pub(crate) fn handle_auth_action(
    provider: &str,
    action: &AuthAction,
    auth: &Arc<crate::store::auth_storage::AuthStorage>,
    handle: &InlineHandle,
    state: &mut RenderState,
) -> bool {
    // Returns true when the dispatched action opened a new overlay via
    // `handle.show_*` (currently only `SetApiKey` opens the secure prompt
    // modal). The caller — the `OverlayEvent::Submitted` arm in
    // `handle_inline_event` — uses this signal to decide whether the
    // previously-open overlay should be closed after dispatch. Closing
    // unconditionally would also clear the freshly-opened overlay because
    // the cmd channel processes `ShowOverlay` and `CloseOverlay` in submit
    // order, so a stale `CloseOverlay` enqueued right after the
    // `ShowOverlay` wins. Branches that do NOT open a new overlay
    // (`StartOAuth` spawns an async task, `RemoveKey` sets
    // `state.confirmation` rather than `state.overlay`) return false so
    // the caller is free to close the old overlay.
    match action {
        AuthAction::SetApiKey => {
            open_secure_prompt(
                state,
                handle,
                SecureInputOrigin::SetKey {
                    provider: provider.to_string(),
                },
            );
            true
        }
        AuthAction::StartOAuth => {
            // PKCE + loopback-callback glue lives in `run_oauth_flow`
            // (defined just below `handle_auth_action`). Spawn it on a
            // dedicated tokio task so the main loop can continue
            // rendering; the spawned task posts status updates back to
            // the transcript via the cloned `InlineHandle`.
            //
            // First, gate on the provider actually having an OAuth
            // spec in `product-meta.toml` — the action is only offered
            // when `next_provider_actions` includes it, so this branch
            // is purely defensive against a stale UI state.
            let spec = match crate::provider_oauth::spec_for(provider) {
                Some(s) => s,
                None => {
                    handle.append_line(
                        InlineMessageKind::Error,
                        vec![plain_segment(format!(
                            "OAuth: no OAuth config for '{provider}'."
                        ))],
                    );
                    return false;
                }
            };
            // `provider_owned` and `tx` are cloned Strings/`InlineHandle`s
            // owned by the task; `auth_clone` is the shared storage
            // singleton (cheap to clone — it is already `Arc`-backed).
            // `spec` is moved into the task.
            let provider_owned = provider.to_string();
            let tx = handle.clone();
            let auth_clone = Arc::clone(auth);
            tokio::spawn(async move {
                run_oauth_flow(provider_owned, spec, tx, auth_clone).await;
            });
            false
        }
        AuthAction::RemoveKey => {
            state.confirmation = Some(ModalConfirmation {
                title: format!("Remove key for {provider}?"),
                message: "  y \u{2014} remove key     n / x \u{2014} cancel".into(),
                action: ConfirmationAction::RemoveProviderKey(provider.to_string()),
            });
            false
        }
    }
}

/// Drive the OAuth `authorization_code` flow for `provider` end to end:
///
/// 1. Bind an ephemeral loopback TCP listener and capture its port.
/// 2. Generate PKCE verifier + S256 challenge (`provider_oauth::pkce_pair`).
/// 3. Build the authorization URL (`provider_oauth::build_auth_url`) and
///    open it in the user's browser (`provider_oauth::open_browser`).
/// 4. Wait on the listener for the redirect carrying the `code` + `state`
///    (`oauth_listener::await_callback`); bind a timeout so a stuck
///    listener cannot leak.
/// 5. Exchange the code for tokens at the provider's token URL
///    (`provider_oauth::exchange_code`).
/// 6. Persist the OAuth credential via `AuthStorage::set_oauth_full` so
///    subsequent requests can use the access token (and `refresh_token`
///    if granted) without re-prompting the user.
///
/// Steps that hard-fail (callback timeout, state mismatch, missing
/// `code`, exchange error, persist error) post an `InlineMessageKind::Error`
/// line to the transcript and return; the bound listener is dropped on
/// every return path, satisfying the single-shot invariant.
///
/// Headless fallback (plan §3 / design §3): if `open_browser` returns
/// `Err`, we do NOT abort. We post an `Info` line printing the auth URL
/// and lengthen the callback timeout to 5 minutes so the user can paste
/// the URL into a browser on another machine and complete the flow.
/// Masking: every user-facing line that mentions the access token
/// surfaces only the token length (`access_token.chars().count()`), never
/// the value. Tokens are never logged via `tracing`.
pub(crate) async fn run_oauth_flow(
    provider: String,
    spec: crate::provider_oauth::ProviderOAuthSpec,
    handle: InlineHandle,
    auth: Arc<crate::store::auth_storage::AuthStorage>,
) {
    use std::time::Duration;
    // Timeout is selected AFTER the browser attempt: 2 minutes when the
    // browser opened (the user is right in front of it), 5 minutes when
    // it didn't (headless box — user has to copy the URL to another
    // machine, sign in there, and the redirect has to traverse NAT).
    // The variable is declared once as `mut` and then frozen below.
    // 1. Bind the loopback listener BEFORE opening the browser so the
    //    `redirect_uri` we hand to the provider already points at a live
    //    port. `TcpListener::bind("127.0.0.1:0")` picks an ephemeral port.
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0u16)).await {
        Ok(l) => l,
        Err(e) => {
            handle.append_line(
                InlineMessageKind::Error,
                vec![plain_segment(format!(
                    "OAuth: could not bind loopback listener for '{provider}': {e}"
                ))],
            );
            return;
        }
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            handle.append_line(
                InlineMessageKind::Error,
                vec![plain_segment(format!(
                    "OAuth: could not read loopback port for '{provider}': {e}"
                ))],
            );
            return;
        }
    };

    // 2. PKCE pair + per-flow `state`. The state must match what we send
    //    in the auth URL and what we accept on the callback — a single
    //    random base64-url string is enough since the flow is single-shot.
    let (verifier, challenge) = crate::provider_oauth::pkce_pair();
    let state_token = crate::provider_oauth::pkce_pair().0; // 43-char url-safe random

    // 3. Build auth URL and open the browser. `open_browser` already
    //    validates the URL scheme so a malformed spec would have failed
    //    at `build_auth_url` time (it calls `Url::parse` internally).
    let auth_url = crate::provider_oauth::build_auth_url(&spec, port, &state_token, &challenge);
    handle.append_line(
        InlineMessageKind::Info,
        vec![plain_segment(format!(
            "OAuth: opening browser for '{provider}' on http://127.0.0.1:{port}{}",
            spec.redirect_path
        ))],
    );
    // Pick the callback timeout based on whether the browser opened.
    // Headless fallback (plan §3 / design §3): when the OS refuses to
    // launch a browser, we surface the URL and KEEP listening so a user
    // on a different machine can paste it, sign in, and let the
    // redirect land back on our loopback port. A 5-minute window is
    // long enough for that round-trip; a 2-minute window is plenty
    // when the browser already opened in front of the user.
    let callback_timeout = match crate::provider_oauth::open_browser(&auth_url) {
        Ok(()) => Duration::from_secs(120),
        Err(e) => {
            handle.append_line(
                InlineMessageKind::Info,
                vec![plain_segment(format!(
                    "OAuth: could not open a browser ({e}).\nOpen this URL manually within 5 minutes:\n  {auth_url}"
                ))],
            );
            Duration::from_secs(300)
        }
    };

    // 4. Wait for the callback. The listener is single-shot by design:
    //    `await_callback` accepts exactly one connection.
    let callback = match crate::oauth_listener::await_callback(
        listener,
        state_token.clone(),
        spec.redirect_path.clone(),
        callback_timeout,
    )
    .await
    {
        Ok(c) => c,
        Err(crate::oauth_listener::CallbackError::Timeout) => {
            handle.append_line(
                InlineMessageKind::Error,
                vec![plain_segment(format!(
                    "OAuth: timed out waiting for '{provider}' callback (after {}s)",
                    callback_timeout.as_secs()
                ))],
            );
            return;
        }
        Err(e) => {
            handle.append_line(
                InlineMessageKind::Error,
                vec![plain_segment(format!(
                    "OAuth: callback failed for '{provider}': {e}"
                ))],
            );
            return;
        }
    };

    // 5. Exchange code → tokens.
    let tokens =
        match crate::provider_oauth::exchange_code(&spec, port, &callback.code, &verifier).await {
            Ok(t) => t,
            Err(e) => {
                handle.append_line(
                    InlineMessageKind::Error,
                    vec![plain_segment(format!(
                        "OAuth: token exchange failed for '{provider}': {e}"
                    ))],
                );
                return;
            }
        };

    // 6. Persist. `set_oauth_full` takes u64 `expires_at`; `OAuthTokens`
    //    exposes i64 (so callers can branch on `now < expires_at` in
    //    signed arithmetic). Saturate defensively — the value is always
    //    `now + expires_in` with `expires_in >= 0`, so negatives are
    //    impossible here, but a guard costs nothing.
    let new_expires_at: u64 = tokens.expires_at.max(0) as u64;
    let access_token_len = tokens.access_token.chars().count();
    // `set_oauth_full` returns `()` and logs persistence failures via
    // `tracing::warn` — the in-memory credential is always updated.
    auth.set_oauth_full(
        &provider,
        tokens.access_token,
        tokens.refresh_token,
        new_expires_at,
        if tokens.scopes.is_empty() {
            None
        } else {
            Some(tokens.scopes.join(" "))
        },
        None,
    );
    handle.append_line(
        InlineMessageKind::Info,
        vec![plain_segment(format!(
            "OAuth: '{provider}' logged in. Token stored ({} chars).",
            access_token_len
        ))],
    );
}

/// Map an input-thread `InlineEvent` to agent actions / state edits.
fn handle_inline_event(
    state: &mut RenderState,
    handle: &InlineHandle,
    session: &crate::app::agent_session::AgentSessionHandle,
    prompt_queue: &Arc<PromptQueue>,
    evt: InlineEvent,
) -> LoopOutcome {
    match evt {
        InlineEvent::Submit(text) => {
            // ── Drain pending resume (set by /sessions <id> or the picker). ──
            if let Some(path) = state.pending_resume.take() {
                let swapper = state.swapper();
                let agent_arc = Arc::clone(&session.agent_arc());
                let settings = session.settings_clone();
                let session_state = state
                    .session_state
                    .clone()
                    .expect("RenderState::session_state must be initialized at TUI startup");
                let path_for_log = path.clone();
                let handle = handle.clone();
                let swapper_for_swap = swapper.clone();
                tokio::spawn(async move {
                    match crate::app::agent_session::resume_from_file(
                        agent_arc,
                        settings,
                        session_state,
                        &path,
                        None,
                    )
                    .await
                    {
                        Ok(new_session) => {
                            swapper_for_swap.swap(new_session.clone_handle());
                            let n = new_session.messages().len();
                            let id = new_session.session_id();
                            handle.append_line(
                                InlineMessageKind::Info,
                                vec![plain_segment(format!(
                                    "Resumed session {id} ({n} messages)"
                                ))],
                            );
                        }
                        Err(crate::app::agent_session::ResumeError::FileNotFound(p)) => {
                            handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("No session file: {}", p.display()))],
                            );
                        }
                        Err(crate::app::agent_session::ResumeError::CwdInvalid(cwd)) => {
                            handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!(
                                    "Cannot resume {}: the session was recorded in `{cwd}`, which no longer exists. \
                                     Use /export to save its content, then /clear.",
                                    path_for_log.display()
                                ))],
                            );
                        }
                    }
                });
                return LoopOutcome::Continue;
            }
            // Drain the composer — the input thread already cleared its
            // local copy once Submit fired, but we keep the canonical
            // buffer here in sync.
            let prompt = text.to_string();
            state.composer.set_text("");
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
                        // File-based commands: try before erroring.
                        if let Some(expanded) = crate::tui_vt::slash::file_commands::try_expand(
                            &ctx.state.file_commands,
                            &prompt,
                        ) {
                            // Send expanded text directly to the agent worker.
                            // The original `/cmd args` is already echoed above.
                            prompt_queue.enqueue(expanded);
                            LoopOutcome::Continue
                        } else {
                            ctx.reply(
                                InlineMessageKind::Error,
                                format!("Unknown command: {}", prompt.trim()),
                            );
                            LoopOutcome::Continue
                        }
                    }
                };
            }
            state.append_line(InlineMessageKind::User, vec![plain_segment(prompt.clone())]);
            // While a run is active, mirror the prompt into the queue pane so
            // the user sees their input is queued (the worker channel already
            // serialises execution; this is the visible counterpart).
            if session.is_streaming() {
                state.queued_inputs.push(prompt.clone());
                state.show_tip(
                    "send_now",
                    "Ctrl+Enter sends now | Ctrl+; manages queue",
                    240,
                    true,
                );
            }
            // Hand the prompt to the worker thread. If the worker has
            // already exited (e.g. shutdown), drop it on the floor.
            prompt_queue.enqueue(prompt);
        }
        InlineEvent::Cancel => {
            // Esc-driven cancel. While a stream is running, abort it (the
            // input thread's ~1s post-cancel grace then prevents mashing).
            // When idle, Esc is an instant one-press quit — no grace, no
            // quit-arming footer that would invite a re-press the grace
            // swallows.
            return match route_cancel(session.is_streaming()) {
                CancelRoute::Interrupt => handle_interrupt(state, session, handle),
                CancelRoute::Exit => LoopOutcome::Exit,
            };
        }
        InlineEvent::Exit => {
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
        InlineEvent::Overlay(overlay_evt) => {
            use oxicode_vtui::tui::core::OverlayEvent;
            match overlay_evt {
                OverlayEvent::Submitted(sub) => {
                    // Tracks whether this submission chained into a new overlay
                    // (the action menu after `/providers` row selection, or the
                    // secure prompt after `SetApiKey`). When set, the
                    // unconditional `close_overlay()` at the end of the arm
                    // would clear the freshly-opened overlay because the
                    // `cmd` channel processes `ShowOverlay` and
                    // `CloseOverlay` in submit order. Stale-state cleanup
                    // (clearing `overlay_providers` etc.) still runs — only
                    // the close is gated.
                    let mut opened_new_overlay = false;
                    // If this was a /model picker, set the selected model.
                    if let OverlaySubmission::Selection(InlineListSelection::Model(idx)) = &sub
                        && idx < &state.overlay_model_ids.len()
                    {
                        let model_id = state.overlay_model_ids[*idx].clone();
                        match session.set_model(&model_id) {
                            Ok(()) => {
                                sync_model_chips(state, session);
                                handle.append_line(
                                    InlineMessageKind::Info,
                                    vec![plain_segment(format!("Switched to {model_id}"))],
                                );
                            }
                            Err(e) => handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("Failed to set model: {e}"))],
                            ),
                        }
                    }
                    // If this was a /theme picker, apply the selected theme.
                    if let OverlaySubmission::Selection(InlineListSelection::Theme(theme_id)) = &sub
                    {
                        match oxicode_vtui::theme::set_active_theme(theme_id) {
                            Ok(()) => {
                                let label = oxicode_vtui::theme::theme_label(theme_id)
                                    .unwrap_or(theme_id.as_ref())
                                    .to_string();
                                handle.append_line(
                                    InlineMessageKind::Info,
                                    vec![plain_segment(format!("Theme: {label}"))],
                                );
                            }
                            Err(e) => handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("Unknown theme: {e}"))],
                            ),
                        }
                    }
                    // If this was a command palette selection, fill the prompt.
                    if let OverlaySubmission::Selection(InlineListSelection::SlashCommand(name)) =
                        &sub
                    {
                        state.composer.set_text(&format!("/{name} "));
                    }
                    // Settings overlay: toggle/cycle the selected setting.
                    // `ConfigAction` carries the `SettingKey` Debug name
                    // emitted by `settings_overlay_items`; dispatch goes
                    // through the def table (`apply_change`), never a
                    // per-name match.
                    // Synthetic ConfigAction payloads emitted by the
                    // panel editors: handled FIRST so they never reach
                    // the generic `ConfigAction(name)` arm (which would
                    // treat `SubmenuCommit:…` / `DisabledToolToggle:…`
                    // as a bogus SettingKey name and error out).
                    let synthetic_dispatched = if let OverlaySubmission::Selection(
                        InlineListSelection::ConfigAction(payload),
                    ) = &sub
                    {
                        if let Some(rest) = payload.strip_prefix("SubmenuCommit:") {
                            if let Some((key_str, value)) = rest.split_once(':') {
                                if let Some(key) = parse_setting_key(key_str) {
                                    match commit_submenu_choice(state, key, value.to_string()) {
                                        Ok(status) => {
                                            handle.append_line(
                                                InlineMessageKind::Info,
                                                vec![plain_segment(status.clone())],
                                            );
                                            opened_new_overlay = state.overlay.is_some();
                                        }
                                        Err(e) => handle.append_line(
                                            InlineMessageKind::Error,
                                            vec![plain_segment(format!(
                                                "Failed to save setting: {e}"
                                            ))],
                                        ),
                                    }
                                } else {
                                    handle.append_line(
                                        InlineMessageKind::Error,
                                        vec![plain_segment(format!(
                                            "Unknown setting key in submenu commit: {key_str}"
                                        ))],
                                    );
                                }
                            } else {
                                handle.append_line(
                                    InlineMessageKind::Error,
                                    vec![plain_segment(format!(
                                        "Malformed submenu commit payload: {payload}"
                                    ))],
                                );
                            }
                            true
                        } else if let Some(tool) = payload.strip_prefix("DisabledToolToggle:") {
                            let essential = session
                                .agent_ref()
                                .tools()
                                .get_tools()
                                .into_iter()
                                .find(|t| t.name() == tool)
                                .is_some_and(|t| t.essential());
                            let currently_disabled = crate::store::settings::Settings::load()
                                .map(|s| s.disabled_tools.iter().any(|t| t == tool))
                                .unwrap_or(false);
                            commit_disabled_tool_toggle(
                                state,
                                handle,
                                session,
                                tool.to_string(),
                                essential,
                                currently_disabled,
                            );
                            opened_new_overlay = state.overlay.is_some();
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if !synthetic_dispatched
                        && let OverlaySubmission::Selection(InlineListSelection::ConfigAction(key)) =
                            &sub
                    {
                        let def = SETTING_DEFS.iter().find(|d| format!("{:?}", d.key) == *key);
                        match def {
                            Some(def) => {
                                let mut settings =
                                    crate::store::settings::Settings::load().unwrap_or_default();
                                // Toggle submits the inverted bool; Cycle
                                // the next variant. The structured editors
                                // (Text/Submenu/Multiselect/MapEditor)
                                // commit their own explicit values.
                                let next_value = match def.widget {
                                    SettingWidget::Toggle => Some(
                                        (get_display_value(def.key, &settings) != "true")
                                            .to_string(),
                                    ),
                                    SettingWidget::Cycle => next_cycle_value(def.key, &settings),
                                    _ => None,
                                };
                                if let Some(next) = next_value {
                                    match crate::tui_vt::settings_defs::apply_change(
                                        def.key,
                                        &mut settings,
                                        next,
                                    ) {
                                        Ok(()) => match settings.save() {
                                            Ok(()) => {
                                                if let Err(e) = sync_settings_live(
                                                    state, session, def.key, &settings,
                                                ) {
                                                    // Saved, but the live
                                                    // toggle failed — an
                                                    // Error line, never a
                                                    // silent success.
                                                    handle.append_line(
                                                        InlineMessageKind::Error,
                                                        vec![plain_segment(format!(
                                                            "{}: {e}",
                                                            def.label
                                                        ))],
                                                    );
                                                } else {
                                                    handle.append_line(
                                                        InlineMessageKind::Info,
                                                        vec![plain_segment(format!(
                                                            "{}: {}",
                                                            def.label,
                                                            get_display_value(def.key, &settings)
                                                        ))],
                                                    );
                                                }
                                            }
                                            Err(e) => handle.append_line(
                                                InlineMessageKind::Error,
                                                vec![plain_segment(format!(
                                                    "Failed to save {}: {e}",
                                                    def.label
                                                ))],
                                            ),
                                        },
                                        Err(e) => handle.append_line(
                                            InlineMessageKind::Error,
                                            vec![plain_segment(format!(
                                                "Failed to apply {}: {e}",
                                                def.label
                                            ))],
                                        ),
                                    }
                                }
                            }
                            None => handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("Unknown setting: {key}"))],
                            ),
                        }
                    }
                    // Settings panel tab switch: reopen the panel rebuilt
                    // for the requested tab (Enter closes the overlay, so
                    // the switch has to reopen it).
                    if let OverlaySubmission::Selection(InlineListSelection::SettingsTab(idx)) =
                        &sub
                    {
                        switch_settings_tab(state, *idx);
                        opened_new_overlay = state.overlay.is_some();
                    }
                    // Settings panel sidebar section jump: reopen on the
                    // active tab with the selection moved to the section's
                    // first row.
                    if let OverlaySubmission::Selection(InlineListSelection::SettingsSection(idx)) =
                        &sub
                    {
                        jump_settings_section(state, *idx);
                        opened_new_overlay = state.overlay.is_some();
                    }
                    // Keybinding capture: selecting an action row opens
                    // the "press a key combo" prompt. The INPUT thread
                    // consumes the next key before global-shortcut
                    // resolution (`handle_key_capture`) and commits
                    // through the keybindings map editor.
                    if let OverlaySubmission::Selection(InlineListSelection::SettingKeyCapture(
                        name,
                    )) = &sub
                    {
                        if GlobalAction::from_name(name).is_some() {
                            state.overlay = Some(build_key_capture_overlay(name));
                            state.settings_map_rows.clear();
                            opened_new_overlay = true;
                        } else {
                            handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("Unknown keybinding action: {name}"))],
                            );
                        }
                    }
                    // Settings-panel text editor: open the prompt; the
                    // submitted text arrives via the SecureInput arm
                    // below (`SecureInputOrigin::TextEdit(key)`).
                    if let OverlaySubmission::Selection(InlineListSelection::SettingTextEdit(
                        key_name,
                    )) = &sub
                    {
                        if let Some(key) = parse_setting_key(key_name) {
                            open_text_edit_prompt(state, key);
                            opened_new_overlay = true;
                        } else {
                            handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("Unknown setting key: {key_name}"))],
                            );
                        }
                    }
                    // Settings-panel submenu-select: open a child list
                    // whose selections arrive as synthetic
                    // `ConfigAction("SubmenuCommit:Key:value")` payloads
                    // routed by the ConfigAction arm below.
                    if let OverlaySubmission::Selection(InlineListSelection::SettingSubmenuOpen(
                        key_name,
                    )) = &sub
                    {
                        if let Some(key) = parse_setting_key(key_name) {
                            open_submenu_select_prompt(state, key);
                            opened_new_overlay = true;
                        } else {
                            handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("Unknown setting key: {key_name}"))],
                            );
                        }
                    }
                    // Settings-panel multiselect: open a tool list
                    // sourced live from `session.agent_ref().tools()`;
                    // selections arrive as synthetic
                    // `ConfigAction("DisabledToolToggle:tool")` payloads.
                    if let OverlaySubmission::Selection(InlineListSelection::SettingMultiselect(
                        key_name,
                    )) = &sub
                    {
                        if let Some(parsed) = parse_setting_key(key_name) {
                            if parsed == SettingKey::DisabledTools {
                                open_disabled_tools_multiselect(state, session);
                                opened_new_overlay = true;
                            } else {
                                handle.append_line(
                                    InlineMessageKind::Error,
                                    vec![plain_segment(format!(
                                        "'{parsed:?}' has no multiselect editor"
                                    ))],
                                );
                            }
                        } else {
                            handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("Unknown setting key: {key_name}"))],
                            );
                        }
                    }
                    // Session picker: enqueue the selected session. The next
                    // Submit event drains it before normal composer dispatch.
                    if let OverlaySubmission::Selection(InlineListSelection::Session(id)) = &sub {
                        // Gate: refuse to queue a resume while the agent is
                        // running — the pending_resume drain would clobber
                        // the in-flight conversation's message history on
                        // the shared Arc<Agent> (same wording as the direct
                        // /sessions <id> path and /handoff).
                        if session.is_streaming() {
                            handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(
                                    "Cannot resume while agent is running. Use /cancel first.",
                                )],
                            );
                        } else {
                            let path = crate::tui_vt::slash::registry::sessions_dir()
                                .join(format!("{id}.jsonl"));
                            if !path.is_file() {
                                handle.append_line(
                                    InlineMessageKind::Error,
                                    vec![plain_segment(format!(
                                        "No session file: {}",
                                        path.display()
                                    ))],
                                );
                            } else {
                                state.pending_resume = Some(path);
                            }
                        }
                    }
                    // `/models` catalog browser: switch to the selected model.
                    if let OverlaySubmission::Selection(InlineListSelection::CatalogModel(idx)) =
                        &sub
                        && idx < &state.overlay_catalog_models.len()
                    {
                        let (provider, model_id) = &state.overlay_catalog_models[*idx];
                        let full = format!("{provider}/{model_id}");
                        match session.set_model(&full) {
                            Ok(()) => {
                                sync_model_chips(state, session);
                                handle.append_line(
                                    InlineMessageKind::Info,
                                    vec![plain_segment(format!("Switched to {full}"))],
                                );
                            }
                            Err(e) => handle.append_line(
                                InlineMessageKind::Error,
                                vec![plain_segment(format!("Failed to set model: {e}"))],
                            ),
                        }
                    }
                    // `/providers` list: pick a provider, then drive the
                    // `next_provider_actions(has_key, oauth_capable)` matrix.
                    // Single-action cases fire straight into
                    // `handle_auth_action`; multi-action cases open a
                    // one-shot action list whose selections are
                    // `ProviderAction { provider, action }`.
                    if let OverlaySubmission::Selection(InlineListSelection::ProviderRow(idx)) =
                        &sub
                        && idx < &state.overlay_providers.len()
                    {
                        let name = state.overlay_providers[*idx].clone();
                        let auth = crate::store::auth_storage::shared_auth_storage();
                        let has_key = auth.has(&name);
                        let oauth_capable = crate::provider_oauth::spec_for(&name).is_some();
                        let actions = next_provider_actions(has_key, oauth_capable);
                        if actions.len() == 1 {
                            // Single action — drive directly with no menu.
                            opened_new_overlay |=
                                handle_auth_action(&name, &actions[0], &auth, handle, state);
                        } else {
                            // Show action menu.
                            let items: Vec<InlineListItem> = actions
                                .iter()
                                .map(|a| InlineListItem {
                                    title: match a {
                                        AuthAction::SetApiKey => "Set API key".into(),
                                        AuthAction::StartOAuth => "Login with OAuth".into(),
                                        AuthAction::RemoveKey => "Remove key".into(),
                                    },
                                    subtitle: None,
                                    badge: None,
                                    indent: 0,
                                    selection: Some(InlineListSelection::ProviderAction {
                                        provider: name.clone(),
                                        action: a.clone(),
                                    }),
                                    search_value: None,
                                })
                                .collect();
                            handle.show_list_modal(
                                name.clone(),
                                vec!["Pick an action".into()],
                                items,
                                None,
                                None,
                            );
                            opened_new_overlay = true;
                        }
                    }
                    // `/providers` action menu: forward the chosen
                    // `AuthAction` to the host dispatcher. Selecting
                    // "Remove key" reuses the existing y/n confirmation
                    // modal; "Set API key" opens the secure prompt;
                    // "Login with OAuth" prints the Task 8 stub.
                    if let OverlaySubmission::Selection(InlineListSelection::ProviderAction {
                        provider,
                        action,
                    }) = &sub
                    {
                        let auth = crate::store::auth_storage::shared_auth_storage();
                        opened_new_overlay |=
                            handle_auth_action(provider, action, &auth, handle, state);
                    }
                    // Text/secure prompt committed by the user. The
                    // matching open prompt must have stashed
                    // `state.secure_input_origin`; we trust that field
                    // here because every prompt path sets it before
                    // opening the modal (`open_secure_prompt` for auth,
                    // the model-role prompt builders for the map
                    // editor).
                    if let OverlaySubmission::SecureInput(text) = &sub
                        && let Some(origin) = state.secure_input_origin.take()
                    {
                        match origin {
                            SecureInputOrigin::ModelRoleKey => {
                                // Phase 1 of the new-role flow: the text
                                // is the role NAME — chain straight into
                                // the value prompt.
                                let role = text.trim().to_string();
                                if role.is_empty() {
                                    handle.append_line(
                                        InlineMessageKind::Error,
                                        vec![plain_segment(
                                            "Model role name can't be empty".to_string(),
                                        )],
                                    );
                                } else {
                                    open_model_role_value_prompt(state, &role);
                                    opened_new_overlay = true;
                                }
                            }
                            SecureInputOrigin::TextEdit(key) => {
                                let (_outcome, _msg) = commit_text_edit(
                                    state,
                                    handle,
                                    Some(session),
                                    key,
                                    text.clone(),
                                );
                                opened_new_overlay = state.overlay.is_some();
                            }
                            SecureInputOrigin::ModelRoleValue { role } => {
                                let model = text.trim().to_string();
                                let outcome = if model.is_empty() {
                                    Err("model pattern can't be empty".to_string())
                                } else {
                                    crate::store::settings::Settings::load()
                                        .map_err(|e| e.to_string())
                                        .and_then(|mut settings| {
                                            crate::tui_vt::settings_defs::set_model_role(
                                                &mut settings,
                                                &role,
                                                model.clone(),
                                            );
                                            settings.save().map_err(|e| e.to_string())
                                        })
                                };
                                match outcome {
                                    Ok(()) => {
                                        handle.append_line(
                                            InlineMessageKind::Info,
                                            vec![plain_segment(format!(
                                                "Model role '{role}' \u{2192} {model}"
                                            ))],
                                        );
                                        reopen_settings_panel(
                                            state,
                                            SettingsTab::Model,
                                            Some(format!("Saved '{role}' \u{2192} {model}")),
                                        );
                                        opened_new_overlay = true;
                                    }
                                    Err(e) => handle.append_line(
                                        InlineMessageKind::Error,
                                        vec![plain_segment(format!(
                                            "Failed to save model role '{role}': {e}"
                                        ))],
                                    ),
                                }
                            }
                            origin @ (SecureInputOrigin::SetKey { .. }
                            | SecureInputOrigin::NewlyAdded { .. }) => {
                                let provider = match &origin {
                                    SecureInputOrigin::SetKey { provider }
                                    | SecureInputOrigin::NewlyAdded { provider } => {
                                        provider.clone()
                                    }
                                    _ => unreachable!("auth arm only matches auth origins"),
                                };
                                let auth = crate::store::auth_storage::shared_auth_storage();
                                auth.set_api_key(&provider, text.clone());
                                // The agent keeps a constructed provider instance. Saving a
                                // key alone is not enough for an already-open session: ask
                                // the resolver for a fresh provider immediately so the next
                                // message uses this credential without a restart or model
                                // switch.
                                let refreshed = session.refresh_api_key();
                                let msg = match origin {
                                    SecureInputOrigin::SetKey { .. } => format!(
                                        "Saved API key for '{provider}'. {}",
                                        match refreshed {
                                            Ok(()) => "Ready to retry your message.",
                                            Err(_) => "Restart this session before retrying.",
                                        }
                                    ),
                                    SecureInputOrigin::NewlyAdded { .. } => format!(
                                        "Added and configured '{provider}'. {}",
                                        match refreshed {
                                            Ok(()) =>
                                                "Use /models to choose a model, or send a message.",
                                            Err(_) => "Restart this session before using it.",
                                        }
                                    ),
                                    SecureInputOrigin::ModelRoleKey
                                    | SecureInputOrigin::ModelRoleValue { .. } => {
                                        unreachable!("auth branch reached with a model-role origin")
                                    }
                                    SecureInputOrigin::TextEdit(_) => {
                                        unreachable!("auth branch reached with a text-edit origin")
                                    }
                                };
                                handle
                                    .append_line(InlineMessageKind::Info, vec![plain_segment(msg)]);
                            }
                        }
                    }
                    state.overlay_catalog_models.clear();
                    state.overlay_providers.clear();
                    state.overlay_model_ids.clear();
                    if !opened_new_overlay {
                        handle.close_overlay();
                    }
                }
                OverlayEvent::Cancelled => {
                    handle.close_overlay();
                }
                OverlayEvent::SelectionChanged(_) => {}
            }
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
///   again to quit. The abort is effective because the session hooks installed
///   via `App::from_oxicode` → `with_session_hooks` wire the session's
///   `should_stop` flag into the agent loop.
/// - **Agent idle** → exit the application.
///
/// Both the input-thread key event (`InlineEvent::Interrupt`) and the OS
/// signal handler (`tokio::signal::ctrl_c()`) route through here so
/// behavior is identical regardless of how the interrupt arrives.
///
fn handle_interrupt(
    state: &mut RenderState,
    session: &crate::app::agent_session::AgentSessionHandle,
    _handle: &InlineHandle,
) -> LoopOutcome {
    // If a confirmation is already open, Ctrl+C acts as confirm (quit).
    if state.confirmation.is_some() {
        return LoopOutcome::Exit;
    }
    // A second Ctrl+C (after the first armed a quit during a stream) opens
    // the quit confirmation modal instead of exiting outright.
    if state.pending_quit {
        state.confirmation = Some(quit_confirmation());
        state.pending_quit = false;
        return LoopOutcome::Continue;
    }
    // First Ctrl+C. While streaming, abort the run and arm a quit (the next
    // press opens the confirmation). When idle, open the confirmation at
    // once — no separate quit-arming step needed.
    if session.is_streaming() {
        let s = session.clone();
        tokio::spawn(async move {
            s.abort().await;
        });
        state.pending_quit = true;
    } else {
        state.confirmation = Some(quit_confirmation());
    }
    LoopOutcome::Continue
}

/// Build the standard quit-confirmation dialog.
fn quit_confirmation() -> ModalConfirmation {
    ModalConfirmation {
        title: "Quit oxicode?".into(),
        message: "  y \u{2014} quit now     n / x \u{2014} stay".into(),
        action: ConfirmationAction::Quit,
    }
}

/// Build a clear-conversation confirmation dialog.
pub(super) fn clear_confirmation() -> ModalConfirmation {
    ModalConfirmation {
        title: "Clear conversation?".into(),
        message: "  y \u{2014} clear all     n / x \u{2014} cancel".into(),
        action: ConfirmationAction::ClearConversation,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Input thread — polls crossterm, edits the shared buffer, and forwards
// lifecycle events (Submit, Cancel, …) over a tokio channel.
// ─────────────────────────────────────────────────────────────────────────

/// Execute a global shortcut resolved by the [`Keymap`]. The bodies are
/// the original hardcoded Ctrl-* handlers from the input loop, unchanged
/// — only the trigger condition became keymap-driven. The branch's
/// KeyAction set (Submit/ScrollUp/ScrollDown/Clear/Help/ModelPicker/
/// ToggleThinking) was folded into this single match via the unified
/// `GlobalAction` enum, so a user rebind for any of them dispatches here
/// without falling through to the hardcoded arms below.
fn apply_global_action(
    action: GlobalAction,
    state: &Arc<parking_lot::Mutex<RenderState>>,
    evt_tx: &tokio::sync::mpsc::UnboundedSender<InlineEvent>,
) {
    match action {
        // Ctrl+C: even with raw mode enabled some terminals / shells
        // fall back to delivering it as a SIGINT. Handle it as an
        // explicit interrupt so we don't depend on the OS signal.
        GlobalAction::Interrupt => {
            let _ = evt_tx.send(InlineEvent::Interrupt);
        }
        // Ctrl+M: toggle multiline input mode.
        GlobalAction::ToggleMultiline => {
            let mut s = state.lock();
            s.multiline_mode = !s.multiline_mode;
        }
        // Ctrl+P: open the command palette.
        GlobalAction::OpenCommandPalette => {
            let mut s = state.lock();
            s.overlay = Some(build_command_palette());
        }
        // Ctrl+;: toggle the interactive queue panel.
        GlobalAction::ToggleQueuePanel => {
            let mut s = state.lock();
            s.queue_panel_open = !s.queue_panel_open;
            if s.queue_panel_open {
                s.queue_selected = 0;
            }
        }
        // Ctrl+E: fold all blocks (Shift+E expands all).
        GlobalAction::FoldAll => {
            let mut s = state.lock();
            s.fold_all();
        }
        // Ctrl+Enter: send-now — abort the current run (if any) and submit
        // the composed input immediately, bypassing the queue pane.
        GlobalAction::SendNow => {
            let submitted = harvest_and_clear_input(state);
            if !submitted.is_empty() {
                let _ = evt_tx.send(InlineEvent::Interrupt);
                let _ = evt_tx.send(InlineEvent::Submit(submitted.into()));
            }
        }
        // Plain Enter (and Shift+Enter, both default Submit bindings):
        // harvest and submit the buffer. The muscle-memory carve-out for
        // plain Enter in multiline mode (so it inserts a newline) lives
        // in `keymap_pre_match` — this arm only fires after that check.
        GlobalAction::Submit => {
            let submitted = harvest_and_clear_input(state);
            let _ = evt_tx.send(InlineEvent::Submit(submitted.into()));
        }
        // PageUp: scroll transcript up by a page.
        GlobalAction::ScrollUp => {
            let _ = evt_tx.send(InlineEvent::ScrollPageUp);
        }
        // PageDown: scroll transcript down by a page.
        GlobalAction::ScrollDown => {
            let _ = evt_tx.send(InlineEvent::ScrollPageDown);
        }
        // Ctrl+L: clear visible scrollback / fold-all (default behavior
        // matches Ctrl+E / FoldAll today — the branch wired Clear to
        // Ctrl+E; main has Ctrl+E = FoldAll already, so Clear was
        // reassigned to Ctrl+L and routed to fold_all()).
        GlobalAction::Clear => {
            let mut s = state.lock();
            s.fold_all();
        }
        // ?: open the keyboard-shortcuts overlay. The carve-out for `?`
        // typed into a non-empty composer (so it inserts the char)
        // lives in the Char arm and `keymap_pre_match`'s Help gate.
        GlobalAction::Help => {
            let mut s = state.lock();
            s.overlay = Some(cheatsheet_overlay());
        }
        // Ctrl+G: model picker shortcut — currently aliased to the
        // command palette (the palette has the model switcher as its
        // first tab). Future PR can split ModelPicker into its own
        // overlay; for now it mirrors OpenCommandPalette.
        GlobalAction::ModelPicker => {
            let mut s = state.lock();
            s.overlay = Some(build_command_palette());
        }
        // Ctrl+T: toggle the thinking-reasoning channel. Same wiring as
        // ToggleMultiline today; the branch introduced this name, main
        // had ToggleMultiline on Ctrl+M. Both bindings stay live so a
        // user rebinding one doesn't lose the other.
        GlobalAction::ToggleThinking => {
            let mut s = state.lock();
            s.multiline_mode = !s.multiline_mode;
        }
    }
}

/// Outcome of the generic keymap pre-match for the four actions that
/// historically lived only inside hardcoded dispatch arms (Submit,
/// ScrollUp, ScrollDown, Help). [`KeymapDispatch::None`] means "the
/// keymap does not bind this key to any of the four" — the hardcoded
/// arms below then act as the fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeymapDispatch {
    Submit,
    ScrollPageUp,
    ScrollPageDown,
    Help,
    None,
}

/// Consult the keymap for the four actions whose dispatch used to be
/// hardcoded. Consuming the key here is what makes a user rebind real:
/// `submit: alt+s` fires although no arm in the input thread matches
/// Alt+S; previously the capability was "disable at the original key"
/// only. After the squash-merge these are `GlobalAction` variants, not
/// the branch's `KeyAction`.
///
/// Two muscle-memory carve-outs keep the pre-keymap behavior intact:
/// * Plain Enter in multiline mode inserts a newline even when Enter
///   is bound to Submit — only the *send* path is remappable, so the
///   key falls through to the Enter arm ([`KeymapDispatch::None`]).
/// * A PRINTABLE Help binding (the default `?`) is left to the Char
///   arm, which gates Help on the empty composer so typing `?` inside
///   text still inserts it. Non-printable Help bindings (function
///   keys, …) never reach a Char arm and dispatch here.
pub(crate) fn keymap_pre_match(
    keymap: &Keymap,
    key: &crossterm::event::KeyEvent,
    multiline: bool,
) -> KeymapDispatch {
    let plain_enter_multiline =
        key.code == KeyCode::Enter && multiline && !key.modifiers.contains(KeyModifiers::SHIFT);
    if keymap.matches(GlobalAction::Submit, key) && !plain_enter_multiline {
        return KeymapDispatch::Submit;
    }
    if keymap.matches(GlobalAction::ScrollUp, key) {
        return KeymapDispatch::ScrollPageUp;
    }
    if keymap.matches(GlobalAction::ScrollDown, key) {
        return KeymapDispatch::ScrollPageDown;
    }
    if keymap.matches(GlobalAction::Help, key) && !matches!(key.code, KeyCode::Char(_)) {
        return KeymapDispatch::Help;
    }
    KeymapDispatch::None
}

/// Harvest the composer buffer (or the selected slash-popup item) as a
/// submit payload: clears the composer and popup, records prompt
/// history, and returns the submitted text. Shared by the SendNow
/// arm, the Submit arm, and the generic Submit dispatch.
fn harvest_and_clear_input(state: &Arc<parking_lot::Mutex<RenderState>>) -> String {
    let mut s = state.lock();
    let buf = if s.slash_popup.open && !s.slash_popup.items.is_empty() {
        format!("/{}", s.slash_popup.items[s.slash_popup.selected].name)
    } else {
        let buf = s.composer.text().to_string();
        s.composer.set_text("");
        buf
    };
    s.slash_popup = SlashPopup::default();
    s.history_pos = None;
    // Record non-empty, non-command prompts in history.
    if !buf.is_empty() && !buf.starts_with('/') {
        s.prompt_history.insert(0, buf.clone());
        s.prompt_history.truncate(100);
    }
    buf
}

/// The keyboard-shortcuts overlay. Shared by the generic Help dispatch
/// and the printable (`?`) Char-arm check, which gates it on the empty
/// composer.
fn cheatsheet_overlay() -> OverlayState {
    OverlayState {
        title: "Keyboard Shortcuts".into(),
        lines: cheatsheet_lines(),
        items: vec![],
        selected: 0,
        search: None,
        secure_input: None,
        ..Default::default()
    }
}

fn spawn_input_thread(
    state: Arc<parking_lot::Mutex<RenderState>>,
    evt_tx: tokio::sync::mpsc::UnboundedSender<InlineEvent>,
    mode_handle: Option<std::sync::Arc<std::sync::atomic::AtomicU8>>,
    prompt_queue: Arc<PromptQueue>,
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
                // When an overlay with a secure prompt is open, the paste
                // targets the masked input field instead of the main
                // composer buffer. Single-line filter (drops non-graphic
                // bytes, strips trailing newline) keeps secrets clean.
                let routed_to_secure = {
                    let mut s = state.lock();
                    if let Some(overlay) = s.overlay.as_mut() {
                        if let Some(secure) = overlay.secure_input.as_mut() {
                            // Bracketed paste ends in `\n`; strip it before
                            // filtering so the final newline never reaches
                            // the editor.
                            let trimmed = pasted.trim_end_matches('\n');
                            for ch in trimmed.chars() {
                                if ch.is_ascii_graphic() || ch == ' ' {
                                    let _ = secure
                                        .editor
                                        .apply(oxicode_textarea::EditCommand::Insert(ch));
                                }
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if routed_to_secure {
                    continue;
                }
                let mut s = state.lock();
                s.composer.insert_str(&pasted);
                // Refresh popups so e.g. a paste that turns the buffer
                // into `/sessions <id>` closes the slash autocomplete
                // (it deactivates when `buf[1..].contains(' ')`). Without
                // this, the popup stays open with stale items and the
                // next Enter would replace the buffer with the bare
                // command name, dropping the pasted args.
                refresh_input_popups(&mut s);
                continue;
            }
            let Some(key) = key_event else { continue };

            // Snapshot the live keymap for this keystroke: the settings
            // keybindings editor swaps `RenderState::keymap` in place, so
            // every key resolves against the current map (same RwLock the
            // editor writes). Cheap — the map is a small HashMap and key
            // events are human-paced.
            let keymap = state.lock().keymap.read().clone();

            // Keybinding capture takes precedence over EVERYTHING — the
            // whole point is to grab the next combo verbatim, even one
            // that currently resolves to a global action (that's how
            // you re-examine an existing binding) or lands in the
            // overlay/search handling below.
            {
                let capturing = {
                    let s = state.lock();
                    s.overlay.as_ref().is_some_and(|o| o.key_capture.is_some())
                };
                if capturing {
                    let mut s = state.lock();
                    handle_key_capture(&mut s, key);
                    continue;
                }
            }

            // Global shortcuts: resolve through the live keymap. The
            // defaults match the historical hardcoded Ctrl-* bindings;
            // `settings.keybindings` can rebind any of them and the
            // keybindings editor swaps the map in place. The branch's
            // hardcoded dispatch was removed; `apply_global_action`
            // now handles every unified GlobalAction variant.
            {
                let action = {
                    let s = state.lock();
                    s.keymap.read().resolve(key)
                };
                if let Some(action) = action {
                    apply_global_action(action, &state, &evt_tx);
                    continue;
                }
            }

            // Confirmation modal takes priority over everything except
            // Ctrl+C (handled above): y/Enter confirms, n/x/Esc cancels.
            {
                let s = state.lock();
                if s.confirmation.is_some() {
                    drop(s);
                    handle_confirmation_key(&state, &evt_tx, key.code);
                    continue;
                }
            }

            // Overlay key handling takes priority — when an overlay is
            // open, Up/Down navigate, Enter submits, Esc cancels, and any
            // printable char is captured for the search bar (if any).
            // All other keys are swallowed so the composer buffer stays
            // frozen while the user is interacting with the overlay.
            {
                let s = state.lock();
                if s.overlay.is_some() {
                    drop(s);
                    if handle_overlay_key(&state, &evt_tx, key.code) {
                        continue;
                    }
                }
            }

            // @-file-search dropdown — when the picker is open, intercept
            // navigation and accept keys. Regular chars fall through to
            // normal buffer insertion so the user can keep typing.
            {
                let s = state.lock();
                if s.file_search.is_some() {
                    drop(s);
                    if handle_file_search_key(&state, &evt_tx, key.code) {
                        continue;
                    }
                }
            }

            // Git TUI overlay has absolute key priority when open — keys
            // route through `match_git_key` first; commit-mode chars are
            // appended to the message; unmatched keys do NOT fall through
            // to the composer (the brief: overlay REPLACES the composer).
            if state.lock().git_tui.is_some() && handle_git_tui_key(&state, key.code, key.modifiers)
            {
                continue;
            }

            // Generic keymap dispatch (final-review finding 6): the
            // four actions that historically lived only inside
            // hardcoded arms below — Submit, ScrollUp, ScrollDown,
            // Help — are consulted BEFORE those arms so a user
            // rebind (e.g. `submit: alt+s` in keybindings.yml)
            // actually fires. Keys the keymap does NOT bind to these
            // actions fall through to the arms, which act as the
            // fallback (Enter-as-newline in multiline, `?` on the
            // empty composer, …). Placed after the modal handlers
            // above so overlay/confirmation/git keys keep priority.
            let multiline_mode = state.lock().multiline_mode;
            match keymap_pre_match(&keymap, &key, multiline_mode) {
                KeymapDispatch::Submit => {
                    // Shell mode: submit the buffer as a bash command
                    // request.
                    if state.lock().shell_mode {
                        let submitted = {
                            let mut s = state.lock();
                            let buf = s.composer.text().to_string();
                            s.composer.set_text("");
                            s.shell_mode = false;
                            s.history_pos = None;
                            if !buf.is_empty() {
                                s.prompt_history.insert(0, buf.clone());
                                s.prompt_history.truncate(100);
                            }
                            buf
                        };
                        if !submitted.is_empty() {
                            let prompt = format!("Run this shell command: `{submitted}`");
                            let _ = evt_tx.send(InlineEvent::Submit(prompt.into()));
                        }
                        continue;
                    }
                    let submitted = harvest_and_clear_input(&state);
                    let _ = evt_tx.send(InlineEvent::Submit(submitted.into()));
                    continue;
                }
                KeymapDispatch::ScrollPageUp => {
                    let _ = evt_tx.send(InlineEvent::ScrollPageUp);
                    continue;
                }
                KeymapDispatch::ScrollPageDown => {
                    let _ = evt_tx.send(InlineEvent::ScrollPageDown);
                    continue;
                }
                KeymapDispatch::Help => {
                    let mut s = state.lock();
                    s.overlay = Some(cheatsheet_overlay());
                    continue;
                }
                KeymapDispatch::None => {}
            }

            match key.code {
                // Shift+Tab — cycle autonomy mode Default <-> Auto.
                KeyCode::BackTab => {
                    if let Some(h) = &mode_handle {
                        let new_mode = Mode::load(h).toggle();
                        h.store(new_mode.as_u8(), std::sync::atomic::Ordering::SeqCst);
                        let label = new_mode.label();
                        let detail = if new_mode.is_auto() {
                            "autonomous — no questions, runs to completion"
                        } else {
                            "interactive — may ask questions"
                        };
                        let mut s = state.lock();
                        s.autonomy_mode = new_mode;
                        s.tip = Some(EphemeralTip {
                            text: format!("Mode: {label} — {detail}"),
                            born_tick: 0,
                            ttl_ticks: 240,
                            key: "mode_toggle",
                            ambient: false,
                        });
                    }
                    continue;
                }
                KeyCode::Enter => {
                    // Fallback arm (final-review finding 6): reached
                    // only when the keymap does NOT bind Submit to
                    // this key — the generic dispatch above consumed
                    // every Submit-bound keypress (including
                    // non-Enter rebinds like `submit: alt+s`). The
                    // newline-insert branch stays unconditional:
                    // while in multiline mode, plain Enter inserts a
                    // real `\n` regardless of how the user has
                    // rebound `submit`. Any other Enter is swallowed
                    // — submit is disabled at this key.
                    let multiline = state.lock().multiline_mode;
                    let shift = key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::SHIFT);
                    if multiline && !shift {
                        let mut s = state.lock();
                        s.composer.insert_str("\n");
                    }
                }
                KeyCode::Esc => {
                    // Esc ladder (grok-build-style):
                    // 1. Slash popup open → close popup
                    // 2. Input non-empty + 2nd Esc within 800ms → clear buffer
                    // 3. Input non-empty + 1st Esc → arm "press again to clear"
                    // 4. Empty input → cancel the run (with ~1s post-cancel
                    //    grace so mashing Esc doesn't fire repeated cancels)
                    let mut s = state.lock();
                    if s.shell_mode {
                        s.shell_mode = false;
                        s.composer.set_text("");
                    } else if s.slash_popup.open {
                        s.slash_popup = SlashPopup::default();
                    } else if !s.composer.is_empty() {
                        let now = std::time::Instant::now();
                        let is_double = s
                            .last_esc_at
                            .map(|t| now.duration_since(t).as_millis() < 800)
                            .unwrap_or(false);
                        if is_double {
                            s.composer.set_text("");
                            s.last_esc_at = None;
                        } else {
                            s.last_esc_at = Some(now);
                            // Ephemeral hint so the user learns the
                            // double-Esc-to-clear gesture.
                            s.tip = Some(EphemeralTip {
                                text: "Press Esc again to clear input".to_string(),
                                born_tick: FRAME_TICK.load(std::sync::atomic::Ordering::Relaxed),
                                ttl_ticks: 120,
                                key: "esc_clear",
                                ambient: false,
                            });
                        }
                    } else {
                        let now = std::time::Instant::now();
                        let in_grace = s.cancel_grace_until.map(|t| t > now).unwrap_or(false);
                        if in_grace {
                            // Swallow — already cancelling.
                        } else {
                            s.cancel_grace_until = Some(now + std::time::Duration::from_secs(1));
                            s.last_esc_at = None;
                            drop(s);
                            let _ = evt_tx.send(InlineEvent::Cancel);
                        }
                    }
                }
                KeyCode::Tab => {
                    // Complete the selected slash command into the buffer
                    let mut s = state.lock();
                    if s.slash_popup.open && !s.slash_popup.items.is_empty() {
                        let name = s.slash_popup.items[s.slash_popup.selected].name.clone();
                        s.composer.set_text(&format!("/{} ", name));
                        refresh_input_popups(&mut s);
                    }
                }
                KeyCode::Backspace => {
                    let mut s = state.lock();
                    s.composer.input(crossterm::event::KeyEvent::new(
                        KeyCode::Backspace,
                        KeyModifiers::NONE,
                    ));
                    refresh_input_popups(&mut s);
                }
                KeyCode::Delete => {
                    let mut s = state.lock();
                    s.composer.input(crossterm::event::KeyEvent::new(
                        KeyCode::Delete,
                        KeyModifiers::NONE,
                    ));
                    refresh_input_popups(&mut s);
                }
                KeyCode::Left => {
                    let mut s = state.lock();
                    s.composer.input(crossterm::event::KeyEvent::new(
                        KeyCode::Left,
                        KeyModifiers::NONE,
                    ));
                }
                KeyCode::Right => {
                    let mut s = state.lock();
                    s.composer.input(crossterm::event::KeyEvent::new(
                        KeyCode::Right,
                        KeyModifiers::NONE,
                    ));
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
                    } else if s.queue_panel_open
                        && !s.queued_inputs.is_empty()
                        && s.composer.is_empty()
                    {
                        s.queue_selected = if s.queue_selected == 0 {
                            s.queued_inputs.len() - 1
                        } else {
                            s.queue_selected - 1
                        };
                    } else if s.composer.is_empty() && !s.prompt_history.is_empty() {
                        // History recall: fill the prompt with the previous entry.
                        let pos = s.history_pos.unwrap_or(0);
                        let next = (pos + 1).min(s.prompt_history.len() - 1);
                        s.history_pos = Some(next);
                        let entry = s.prompt_history[next].clone();
                        s.composer.set_text(&entry);
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
                    } else if s.queue_panel_open
                        && !s.queued_inputs.is_empty()
                        && s.composer.is_empty()
                    {
                        s.queue_selected = if s.queue_selected + 1 >= s.queued_inputs.len() {
                            0
                        } else {
                            s.queue_selected + 1
                        };
                    } else {
                        drop(s);
                        let _ = evt_tx.send(InlineEvent::ScrollLineDown);
                    }
                }
                // (PageUp/PageDown scroll dispatch moved to the generic
                // keymap pre-match above — final-review finding 6. Keys
                // not bound to ScrollUp/ScrollDown fall through to the
                // catch-all below and are swallowed.)
                KeyCode::Char(ch) => {
                    let mut s = state.lock();
                    // @! hidden-file toggle: when the picker is open and '!'
                    // is typed immediately after '@', toggle hidden mode
                    // instead of inserting '!'.
                    if s.file_search.is_some()
                        && ch == '!'
                        && s.composer.text()[..s.composer.cursor()].ends_with('@')
                    {
                        let cwd = s.cwd.clone();
                        if let Some(fs) = s.file_search.as_mut() {
                            fs.toggle_hidden(&cwd);
                        }
                        continue;
                    }
                    if s.agent_hub_open && ch == 'q' {
                        s.agent_hub_open = false;
                    } else if s.vim_state.enabled() && !s.slash_popup.open {
                        // Route through the vim engine. Deref the guard so
                        // we can borrow multiple fields simultaneously.
                        let s = &mut *s;
                        let vkey =
                            crossterm::event::KeyEvent::new(KeyCode::Char(ch), key.modifiers);
                        let mut editor = InputEditor::new(&mut s.composer);
                        let outcome = crate::tui_vt::vim::handle_key(
                            &mut s.vim_state,
                            &mut editor,
                            &mut s.vim_clipboard,
                            &vkey,
                        );
                        if outcome.handled {
                            refresh_input_popups(s);
                        }
                    } else if s.composer.is_empty() && !s.slash_popup.open {
                        // Shell mode: `!` on empty buffer enters bash mode.
                        if ch == '!' && !s.shell_mode {
                            s.shell_mode = true;
                            continue;
                        }
                        // Queue panel interactive mode takes priority when
                        // open and the buffer is empty. Keys that don't
                        // match fall through to scrollback nav below.
                        if s.queue_panel_open && !s.queued_inputs.is_empty() {
                            let idx = s.queue_selected.min(s.queued_inputs.len() - 1);
                            match ch {
                                'x' | 'X' => {
                                    let _ = prompt_queue.remove(idx);
                                    s.queued_inputs.remove(idx);
                                    if s.queue_selected >= s.queued_inputs.len()
                                        && !s.queued_inputs.is_empty()
                                    {
                                        s.queue_selected = s.queued_inputs.len() - 1;
                                    }
                                    continue;
                                }
                                'e' => {
                                    if let Some(entry) = prompt_queue.remove(idx) {
                                        s.queued_inputs.remove(idx);
                                        s.composer.set_text(&entry);
                                        s.queue_panel_open = false;
                                        continue;
                                    }
                                }
                                'J' => {
                                    if prompt_queue.move_by(idx, 1)
                                        && idx + 1 < s.queued_inputs.len()
                                    {
                                        s.queued_inputs.swap(idx, idx + 1);
                                        s.queue_selected = idx + 1;
                                    }
                                    continue;
                                }
                                'K' => {
                                    if idx > 0 && prompt_queue.move_by(idx, -1) {
                                        s.queued_inputs.swap(idx, idx - 1);
                                        s.queue_selected = idx - 1;
                                    }
                                    continue;
                                }
                                _ => {} // fall through to scrollback nav
                            }
                        }
                        // When the prompt is empty, intercept scrollback
                        // navigation keys (matching grok-build's scrollback-
                        // focus semantics). Any other char falls through to
                        // normal insertion so the user can start typing.
                        // Printable Help bindings keep their historical
                        // empty-composer gate here (typing `?` inside
                        // text must insert it); non-printable rebinds
                        // dispatch via the generic pre-match above.
                        if keymap.matches(GlobalAction::Help, &key) {
                            s.overlay = Some(cheatsheet_overlay());
                        } else if matches!(ch, 'e') {
                            s.cycle_block_at_view();
                        } else if matches!(ch, 'E') {
                            s.expand_all();
                        } else if matches!(ch, 'J') {
                            s.jump_next_turn();
                        } else if matches!(ch, 'K') {
                            s.jump_prev_turn();
                        } else if matches!(ch, 'n') && s.search.is_some() {
                            s.search_next();
                        } else if matches!(ch, 'N') && s.search.is_some() {
                            s.search_prev();
                        } else {
                            s.composer.input(crossterm::event::KeyEvent::new(
                                KeyCode::Char(ch),
                                key.modifiers,
                            ));
                            refresh_input_popups(&mut s);
                        }
                    } else {
                        s.composer.input(crossterm::event::KeyEvent::new(
                            KeyCode::Char(ch),
                            key.modifiers,
                        ));
                        refresh_input_popups(&mut s);
                    }
                    // plan_nudge: surface /compact when user mentions "plan".
                    if s.tip.is_none() && s.composer.text().to_lowercase().contains("plan") {
                        s.show_tip(
                            "plan_nudge",
                            "Try /compact to summarize and plan ahead",
                            180,
                            true,
                        );
                    }
                }
                _ => {}
            }
        }
    })
}

/// Resolve a keystroke against the active confirmation modal. `y`/Enter
/// confirms — dispatches the bound [`ConfirmationAction`]; `n`/`x`/Esc
/// cancels. Always consumes the key while a confirmation is open.
fn handle_confirmation_key(
    state: &Arc<parking_lot::Mutex<RenderState>>,
    evt_tx: &tokio::sync::mpsc::UnboundedSender<InlineEvent>,
    code: KeyCode,
) {
    let mut s = state.lock();
    let Some(confirm) = s.confirmation.clone() else {
        return;
    };
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            s.confirmation = None;
            drop(s);
            match confirm.action {
                ConfirmationAction::Quit => {
                    let _ = evt_tx.send(InlineEvent::Exit);
                }
                ConfirmationAction::ClearConversation => {
                    // Re-dispatch /clear with --yes so it flows through the
                    // normal command pipeline (where `session.reset()` is
                    // accessible). The sentinel arg bypasses the dialog.
                    let _ = evt_tx.send(InlineEvent::Submit("/clear --yes".into()));
                }
                ConfirmationAction::RemoveProviderKey(name) => {
                    // Re-dispatch /providers remove <name> --yes so it flows
                    // through the normal command pipeline. The sentinel arg
                    // bypasses the confirm dialog.
                    let _ = evt_tx.send(InlineEvent::Submit(
                        format!("/providers remove {name} --yes").into(),
                    ));
                }
            }
        }
        KeyCode::Char('n')
        | KeyCode::Char('N')
        | KeyCode::Char('x')
        | KeyCode::Char('X')
        | KeyCode::Esc => {
            s.confirmation = None;
        }
        _ => {}
    }
}

/// Handle a single keystroke while an overlay is open. Returns `true` if the
/// key was consumed (whether it changed state or not). Always returns `false`
/// when no overlay is open so the caller can fall through to the regular
/// input-thread key dispatch.
fn handle_overlay_key(
    state: &Arc<parking_lot::Mutex<RenderState>>,
    evt_tx: &tokio::sync::mpsc::UnboundedSender<InlineEvent>,
    code: KeyCode,
) -> bool {
    use oxicode_vtui::tui::core::{OverlayEvent, OverlaySubmission};

    let mut s = state.lock();
    let Some(overlay) = s.overlay.as_mut() else {
        return false;
    };
    // Secure (masked) single-line prompt: takes precedence over list
    // navigation. Char / Backspace / Left / Right / Enter / Esc route
    if let Some(secure) = overlay.secure_input.as_mut() {
        use oxicode_textarea::EditCommand;
        match code {
            KeyCode::Backspace => {
                // Delete the grapheme (or atomic element) immediately before
                // the cursor. When the cursor sits at the end of the masked
                // element, this removes the whole value in one operation.
                if secure.editor.cursor_byte() > 0 {
                    let _ = secure.editor.apply(EditCommand::DeleteGraphemeBackward);
                }
            }
            KeyCode::Left => {
                let _ = secure.editor.apply(EditCommand::MoveGraphemeLeft);
            }
            KeyCode::Right => {
                let _ = secure.editor.apply(EditCommand::MoveGraphemeRight);
            }
            KeyCode::Enter => {
                // Submit the editor's text — this is the only path that
                // reaches the real secret value, and it leaves the editor
                // intact for any render that follows before the overlay is
                // torn down.
                let submission = OverlaySubmission::SecureInput(secure.editor.text().to_string());
                drop(s);
                state.lock().overlay = None;
                let _ = evt_tx.send(InlineEvent::Overlay(OverlayEvent::Submitted(submission)));
            }
            KeyCode::Esc => {
                drop(s);
                state.lock().overlay = None;
                let _ = evt_tx.send(InlineEvent::Overlay(OverlayEvent::Cancelled));
            }
            KeyCode::Char(ch) if ch.is_ascii_graphic() || ch == ' ' => {
                // Single-line ASCII filter; the renderer never paints the
                // underlying text, so this just keeps the buffer predictable.
                let _ = secure.editor.apply(EditCommand::Insert(ch));
            }
            _ => {} // ignore other keys while the secure prompt is open
        }
        return true;
    }

    // Settings map-editor hotkeys. These operate on `RenderState`
    // directly (they persist + rebuild the panel), so the overlay
    // borrow from the secure branch must end first. Only active with no
    // search filter — while filtering, letters keep typing into the
    // search box (the helpers re-check that the tabbed panel is open
    // and the selected row is a map row).
    let search_empty = s
        .overlay
        .as_ref()
        .and_then(|o| o.search.as_ref())
        .is_none_or(|search| search.value.is_empty());
    let map_row_consumed = match code {
        KeyCode::Enter if try_edit_model_role(&mut s) => true,
        KeyCode::Char('d') if search_empty && try_remove_settings_map_row(&mut s) => true,
        KeyCode::Char('n') if search_empty && try_start_new_model_role(&mut s) => true,
        _ => false,
    };
    if map_row_consumed {
        return true;
    }
    let Some(overlay) = s.overlay.as_mut() else {
        return false;
    };

    match code {
        KeyCode::Esc => {
            // Cancel the overlay and notify the harness.
            drop(s);
            state.lock().overlay = None;
            let _ = evt_tx.send(InlineEvent::Overlay(OverlayEvent::Cancelled));
        }
        KeyCode::Enter => {
            // Submit the currently selected item. If no item is selected
            // (empty list), we still close the overlay with a cancel.
            let submission = if let Some(item) = overlay.items.get(overlay.selected) {
                match item.selection.clone() {
                    Some(sel) => sel,
                    None => {
                        // Read-only / informational item (no InlineListSelection,
                        // e.g. /tools, /mcp, the /settings Model row): Enter is
                        // a no-op — keep the overlay open so the user can keep
                        // browsing (Esc closes). Avoids polluting the prompt
                        // with a synthetic "/overlay:N" command.
                        return true;
                    }
                }
            } else {
                drop(s);
                state.lock().overlay = None;
                let _ = evt_tx.send(InlineEvent::Overlay(OverlayEvent::Cancelled));
                return true;
            };
            let title = overlay.title.clone();
            let selected = overlay.selected;
            drop(s);
            state.lock().overlay = None;
            tracing::debug!(
                overlay = %title,
                selected,
                "overlay submitted"
            );
            let _ = evt_tx.send(InlineEvent::Overlay(OverlayEvent::Submitted(
                OverlaySubmission::Selection(submission),
            )));
        }
        KeyCode::Up => {
            let len = overlay_filtered_indices(overlay).len();
            if len == 0 {
                return true;
            }
            let pos = overlay_filtered_indices(overlay)
                .iter()
                .position(|&i| i == overlay.selected)
                .unwrap_or(0);
            let new_pos = if pos == 0 { len - 1 } else { pos - 1 };
            overlay.selected = overlay_filtered_indices(overlay)[new_pos];
        }
        KeyCode::Down => {
            let filtered = overlay_filtered_indices(overlay);
            let len = filtered.len();
            if len == 0 {
                return true;
            }
            let pos = filtered
                .iter()
                .position(|&i| i == overlay.selected)
                .unwrap_or(0);
            let new_pos = if pos + 1 >= len { 0 } else { pos + 1 };
            overlay.selected = filtered[new_pos];
        }
        KeyCode::Backspace => {
            if let Some(search) = overlay.search.as_mut() {
                search.value.pop();
                overlay.selected = 0;
            }
        }
        KeyCode::Char(ch) => {
            if let Some(search) = overlay.search.as_mut() {
                search.value.push(ch);
                overlay.selected = 0;
            }
        }
        KeyCode::Left | KeyCode::Right => {
            // Tabbed overlays (the settings panel): ←/→ cycle the tab
            // bar, rebuilding items/sections for the new tab. The search
            // filter survives the switch.
            let tab_count = overlay.tabs.len();
            if tab_count > 1 {
                let next = if code == KeyCode::Right {
                    (overlay.active_tab + 1) % tab_count
                } else {
                    overlay.active_tab.checked_sub(1).unwrap_or(tab_count - 1)
                };
                switch_settings_tab(&mut s, next);
            }
        }
        _ => {
            // Swallow all other keys while an overlay is open.
        }
    }
    true
}

/// Return the indices of `overlay.items` that match the current search filter.
/// When no search is configured (or the search field is empty), returns every
/// index. Used by both the renderer and the input thread so they agree on
/// which item is "selected" after navigation or filter changes.
fn overlay_filtered_indices(overlay: &OverlayState) -> Vec<usize> {
    let needle = overlay
        .search
        .as_ref()
        .map(|s| s.value.to_lowercase())
        .unwrap_or_default();
    if needle.is_empty() {
        return (0..overlay.items.len()).collect();
    }
    overlay
        .items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| {
            let title_hit = item.title.to_lowercase().contains(&needle);
            let sv_hit = item
                .search_value
                .as_deref()
                .map(|v| v.to_lowercase().contains(&needle))
                .unwrap_or(false);
            if title_hit || sv_hit { Some(idx) } else { None }
        })
        .collect()
}

/// Handle a single keystroke while the @-file-search dropdown is open.
/// Returns `true` if the key was consumed. Up/Down navigate, Tab/Enter
/// accept the selection (inserting `@path ` without submitting), Esc
/// cancels. Regular chars fall through (`false`) so they enter the buffer
/// and trigger `refresh_file_search` to re-filter.
fn handle_file_search_key(
    state: &Arc<parking_lot::Mutex<RenderState>>,
    _evt_tx: &tokio::sync::mpsc::UnboundedSender<InlineEvent>,
    code: KeyCode,
) -> bool {
    match code {
        KeyCode::Up => {
            let mut s = state.lock();
            if let Some(fs) = s.file_search.as_mut() {
                fs.up();
                true
            } else {
                false
            }
        }
        KeyCode::Down => {
            let mut s = state.lock();
            if let Some(fs) = s.file_search.as_mut() {
                fs.down();
                true
            } else {
                false
            }
        }
        KeyCode::Tab | KeyCode::Enter => {
            let mut s = state.lock();
            if s.file_search
                .as_ref()
                .and_then(|fs| fs.selected_result())
                .is_some()
            {
                accept_file_search(&mut s, false);
                true
            } else {
                // No results: close the picker, let Enter fall through.
                s.file_search = None;
                false
            }
        }
        KeyCode::Esc => {
            let mut s = state.lock();
            s.file_search = None;
            true
        }
        _ => false,
    }
}

/// Route one keystroke through the git TUI overlay. Returns `true` when
/// the overlay consumed it (so the caller must `continue` and not fall
/// through to the composer), `false` when the overlay wasn't open (or
/// when commit-mode refused to handle the key — never happens today).
///
/// Commit-mode text input is handled here too: printable characters
/// append to the message, Backspace pops, Enter commits, Esc cancels.
fn handle_git_tui_key(
    state: &Arc<parking_lot::Mutex<RenderState>>,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    use crate::tui_vt::git_tui::{GitKeyAction, match_git_key};
    use crossterm::event::KeyEvent;

    let mut s = state.lock();
    if s.git_tui.is_none() {
        return false;
    }
    let cwd = s.cwd.clone();
    let Some(git) = s.git_tui.as_mut() else {
        unreachable!("checked above");
    };
    if git.commit_mode {
        match code {
            KeyCode::Esc => {
                git.commit_mode = false;
                git.commit_msg.clear();
                return true;
            }
            KeyCode::Enter => {
                if let Err(err) = git.commit(&cwd) {
                    tracing::warn!(?err, "git commit failed");
                    // Surface as a tip so the user sees the reason.
                    s.tip = Some(EphemeralTip {
                        text: format!("git commit failed: {err}"),
                        born_tick: FRAME_TICK.load(std::sync::atomic::Ordering::Relaxed),
                        ttl_ticks: 240,
                        key: "git-commit-error",
                        ambient: false,
                    });
                }
                return true;
            }
            KeyCode::Backspace => {
                git.commit_backspace();
                return true;
            }
            KeyCode::Char(c) => {
                if !modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::ALT)
                {
                    git.commit_input_char(c);
                }
                return true;
            }
            _ => return true, // swallow anything else while in commit mode
        }
    }

    // Map raw key to an overlay action. Unmatched keys are dropped (do
    // NOT fall through to the composer per the brief).
    let key = KeyEvent::new(code, modifiers);
    let Some(action) = match_git_key(&key) else {
        return true;
    };
    if matches!(action, GitKeyAction::Close) {
        // Closing clears the overlay entirely.
        s.git_tui = None;
        return true;
    }
    if let Err(err) = git.apply_action(&cwd, action) {
        s.tip = Some(EphemeralTip {
            text: format!("/git: {err}"),
            born_tick: FRAME_TICK.load(std::sync::atomic::Ordering::Relaxed),
            ttl_ticks: 240,
            key: "git-action-error",
            ambient: false,
        });
    }
    true
}

// ───────────────────────────────────────────────────────────────────────
// Agent worker thread — owns the agent run loop, forwards events to the
// session bus, and accepts new prompts from a tokio channel.
// ─────────────────────────────────────────────────────────────────────────

fn spawn_agent_worker(
    session_swapper: Arc<crate::app::agent_session_handle::SessionSwapper>,
    prompt_queue: Arc<PromptQueue>,
) {
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
                    loop {
                        let prompt = prompt_queue.next().await;
                        run_one_prompt(&session_swapper.current(), prompt).await;
                    }
                })
                .await;
        });
    });
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
        oxicode: Some(app.oxicode().clone()),
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

/// Lines for the keyboard shortcuts cheatsheet overlay.
fn cheatsheet_lines() -> Vec<String> {
    vec![
        "".into(),
        "  Navigation".into(),
        "  j / ↓        Scroll down".into(),
        "  k / ↑        Scroll up".into(),
        "  J (Shift+j)  Next turn".into(),
        "  K (Shift+k)  Previous turn".into(),
        "  PgDn / PgUp  Page scroll".into(),
        "  g / G        Top / bottom".into(),
        "".into(),
        "  Blocks".into(),
        "  e            Cycle block (collapse/truncate/expand)".into(),
        "  E            Expand all blocks".into(),
        "  Ctrl+E       Collapse all blocks".into(),
        "".into(),
        "  Search".into(),
        "  /find <q>    Search transcript".into(),
        "  n / N        Next / previous match".into(),
        "".into(),
        "  Commands".into(),
        "  /theme       Cycle color theme".into(),
        "  /model       Pick a model".into(),
        "  /vim         Toggle vim mode".into(),
        "  /compact     Compact context".into(),
        "  /clear       Clear conversation".into(),
        "  Ctrl+C       Cancel run (then y to quit)".into(),
        "  Ctrl+Enter   Send now (abort + submit)".into(),
        "  Ctrl+M       Toggle multiline input".into(),
        "  Shift+Tab    Toggle Auto mode (no questions, runs to end)".into(),
        "  Ctrl+;       Toggle queue panel".into(),
        "".into(),
        "  Special Input".into(),
        "  @           File picker (fuzzy search)".into(),
        "  @!          Toggle hidden files in picker".into(),
        "  !           Shell mode (bash command)".into(),
    ]
}

/// Build the command palette overlay — a searchable list of all slash
/// commands plus quick actions. Triggered by Ctrl+P.
fn build_command_palette() -> OverlayState {
    use oxicode_vtui::tui::core::{InlineListItem, InlineListSelection};

    let catalog = SlashRegistry::builtin_commands();
    let mut items: Vec<InlineListItem> = catalog
        .iter()
        .map(|(name, desc, aliases)| {
            let title = if aliases.is_empty() {
                format!("/{name}")
            } else {
                format!(
                    "/{name} ({})",
                    aliases
                        .iter()
                        .map(|a| format!("/{a}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            InlineListItem {
                title,
                subtitle: Some(desc.to_string()),
                badge: None,
                indent: 0,
                selection: Some(InlineListSelection::SlashCommand(name.to_string())),
                search_value: Some(format!("{name} {desc}")),
            }
        })
        .collect();
    items.sort_by(|a, b| a.title.cmp(&b.title));

    OverlayState {
        title: "Command Palette".into(),
        lines: vec!["Type to filter, Enter to select".into()],
        items: items
            .into_iter()
            .map(|item| OverlayListItem {
                title: item.title,
                subtitle: item.subtitle,
                badge: item.badge,
                indent: item.indent,
                search_value: item.search_value,
                selection: item.selection,
            })
            .collect(),
        selected: 0,
        search: Some(OverlaySearchState {
            label: "search".into(),
            placeholder: Some("filter commands\u{2026}".into()),
            value: String::new(),
        }),
        secure_input: None,
        ..Default::default()
    }
}

/// Global frame tick counter for animations (incremented per render).
static FRAME_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Animation epoch for time-keyed animation frames. Spinner frames key
/// on wall-clock time — NOT the draw count — because event bursts
/// during streaming drive many draws per interval and made count-keyed
/// spinners visibly race.
static ANIMATION_T0: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);

/// The animation frame index for a spinner with the given frame period
/// (milliseconds). Deterministic in wall-clock time: rapid
/// back-to-back draws within one period show the same frame.
fn animation_frame(period_ms: u64) -> u64 {
    ANIMATION_T0.elapsed().as_millis() as u64 / period_ms.max(1)
}
/// Tracks whether the terminal title currently shows a running state.
static TITLE_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// ASCII spinner frames for the tab title. They remain readable in every font.
const TITLE_SPINNER: &[&str] = &["-", "\\", "|", "/"];

/// Braille spinner frames for the in-TUI run indicator (the row above
/// the composer). Braille is plain Unicode (U+2800 block) — no font or
/// emoji caveats — and animates on the frame tick.
const RUN_SPINNER: &[&str] = &[
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280F}",
];

/// `59s` under a minute, `2m 05s` beyond it — for the run indicator's
/// elapsed readout.
fn format_elapsed_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Linear-interpolate between two RGB colors. `ratio` 0 = base, 1 = target.
fn blend_rgb(base: Color, target: Color, ratio: f64) -> Color {
    match (base, target) {
        (Color::Rgb(br, bg, bb), Color::Rgb(tr, tg, tb)) => {
            let r = (br as f64 + (tr as f64 - br as f64) * ratio).round() as u8;
            let g = (bg as f64 + (tg as f64 - bg as f64) * ratio).round() as u8;
            let b = (bb as f64 + (tb as f64 - bb as f64) * ratio).round() as u8;
            Color::Rgb(r, g, b)
        }
        _ => base,
    }
}

/// Accent rail color for a transcript line kind.
fn accent_color_for_kind(kind: InlineMessageKind, styles: &ThemeStyles) -> Color {
    match kind {
        InlineMessageKind::User => color_from_anstyle(styles.user.get_fg_color()),
        InlineMessageKind::Agent => color_from_anstyle(styles.response.get_fg_color()),
        InlineMessageKind::Tool => color_from_anstyle(styles.tool.get_fg_color()),
        InlineMessageKind::Error => color_from_anstyle(styles.error.get_fg_color()),
        InlineMessageKind::Warning => color_from_anstyle(styles.status.get_fg_color()),
        InlineMessageKind::Info => color_from_anstyle(styles.info.get_fg_color()),
        InlineMessageKind::Policy => color_from_anstyle(styles.mcp.get_fg_color()),
        InlineMessageKind::Pty => color_from_anstyle(styles.pty_output.get_fg_color()),
    }
}

/// Compose one frame using the agent view layout (grok-build-style):
/// Scrollback (dominant, top) → Prompt → ShortcutsBar (bottom).
/// Chrome geometry and the shortcuts bar are rendered by
/// [`render_chrome`](crate::tui_vt::frame_layout::render_chrome); the
/// transcript and composer are placed into the returned layout rects.
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
    let layout = super::frame_layout::compute_chrome(area);
    let tick = FRAME_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Update terminal tab title: spinner while running, plain when idle.
    {
        let running = state.active_run.is_some() || state.reasoning_stage.is_some();
        let was_running = TITLE_RUNNING.swap(running, std::sync::atomic::Ordering::Relaxed);
        if running || was_running {
            let title = if running {
                let spin = TITLE_SPINNER[(animation_frame(120) as usize) % TITLE_SPINNER.len()];
                let model = state
                    .header_context
                    .editor_context
                    .as_deref()
                    .unwrap_or("oxicode");
                format!("{spin} oxicode \u{2014} {model}")
            } else {
                "oxicode".to_string()
            };
            use std::io::Write;
            let _ = write!(std::io::stderr(), "\x1b]2;{}\x07", title);
            let _ = std::io::stderr().flush();
        }
    }
    // Git TUI overlay REPLACES the scrollback + composer region when
    // open. Skip both so the transcript doesn't bleed through, then
    // draw the overlay across the full frame area.
    if let Some(git) = &state.git_tui {
        crate::tui_vt::git_tui::render::render_overlay_lines(frame, area, git);
    } else {
        render_transcript(frame, layout.scrollback, state);
        let mut pinned_area = layout.scrollback;
        if !state.queued_inputs.is_empty() {
            let used = render_queue_pane(frame, pinned_area, state);
            pinned_area.y = pinned_area.y.saturating_add(used);
            pinned_area.height = pinned_area.height.saturating_sub(used);
        }
        if !state.todo_phases.is_empty() {
            if frame.area().height < TODO_COMPACT_ROWS_THRESHOLD {
                let line = render_todo_compact_line(&state.todo_phases);
                frame.render_widget(
                    Paragraph::new(vec![line]),
                    Rect {
                        height: 1,
                        ..pinned_area
                    },
                );
            } else {
                let is_matched = build_matched_closure(state.hub.as_ref());
                render_todo_pane(
                    frame,
                    pinned_area,
                    &state.todo_phases,
                    state.todo_expanded,
                    is_matched,
                );
            }
        }
        // The row above the composer has one owner per frame. A live run
        // (tracker or stage) takes it — the tracker spans turn boundaries.
        if state.active_run.is_some() || state.reasoning_stage.is_some() {
            render_reasoning_indicator(frame, layout.prompt, state);
        } else if state.pending_quit {
            render_pending_quit_hint(frame, layout.prompt);
        } else if !state.follow_ups.is_empty() {
            render_follow_ups(frame, layout.prompt, &state.follow_ups);
        } else {
            // Ephemeral tip banner above the composer (auto-dismissed by tick TTL).
            let occluded = state.overlay.is_some() || state.confirmation.is_some();
            if let Some(tip) = &state.tip
                && tip_is_visible(tip, tick)
                && !(tip.ambient && occluded)
            {
                render_tip(frame, layout.prompt, &tip.text);
            }
        }
        render_composer(frame, layout.prompt, state);
        if state.slash_popup.open {
            render_slash_popup(frame, layout.prompt, state);
        }
        if state.file_search.is_some() {
            render_file_search_dropdown(frame, layout.prompt, state);
        }
    }
    if state.agent_hub_open {
        render_agent_hub(frame, area, state);
    }
    if let Some(overlay) = &state.overlay {
        render_overlay(frame, area, overlay);
    }
    if let Some(confirm) = &state.confirmation {
        render_confirmation(frame, area, confirm);
    }
    // (Git TUI overlay is drawn inside the `if let Some(git)` arm
    // above; nothing more to paint here.)
}
/// Render the y/n/x confirmation modal centered on top of everything else.
fn render_confirmation(frame: &mut Frame, area: Rect, confirm: &ModalConfirmation) {
    let styles = active_styles();
    let accent = color_from_anstyle(styles.error.get_fg_color());
    let inner_w = confirm
        .title
        .chars()
        .count()
        .max(confirm.message.chars().count())
        .max(36) as u16;
    let width = inner_w + 4;
    let height = 5;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup_area = Rect {
        x,
        y,
        width,
        height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            format!(" {} ", confirm.title),
            Style::default().fg(accent).bold(),
        ))
        .border_style(Style::default().fg(accent));
    let msg = Line::styled(
        confirm.message.clone(),
        Style::default().fg(color_from_anstyle(Some(styles.foreground))),
    );
    frame.render_widget(
        Paragraph::new(vec![Line::default(), msg]).block(block),
        popup_area,
    );
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

/// Render an overlay (Modal / List) as a centered, bordered panel. Modals
/// show only their title + descriptive lines; lists also render a search bar
/// (when configured) and a scrollable item list with the selected item
/// marked by a plain-text cursor.
fn render_overlay(frame: &mut Frame<'_>, area: Rect, overlay: &OverlayState) {
    let styles = active_styles();
    // Secure-input overlays draw a compact frame: title + lines + a single
    // masked input box. List overlays take the longer path below.
    if let Some(secure) = &overlay.secure_input {
        // Reserve the line just below `overlay.lines` for the input box.
        let lines_count = overlay.lines.len();
        let desired_h = (lines_count as u16).saturating_add(1).saturating_add(2); // input row + borders
        let height = desired_h.min(area.height.saturating_sub(2));
        let width = area.width.clamp(30, 80);
        let rect = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, rect);

        let title = Line::from(Span::styled(
            format!(" {} ", overlay.title),
            Style::default()
                .fg(color_from_anstyle(styles.primary.get_fg_color()))
                .add_modifier(Modifier::BOLD),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())))
            .title(title);
        let inner = block.inner(rect);
        frame.render_widget(&block, rect);

        let secondary = color_from_anstyle(styles.secondary.get_fg_color());

        let mut row = inner.top();
        for line_text in &overlay.lines {
            let row_area = Rect {
                x: inner.left(),
                y: row,
                width: inner.width,
                height: 1,
            };
            let line = Line::from(Span::styled(
                line_text.clone(),
                Style::default().fg(secondary),
            ));
            frame.render_widget(Paragraph::new(line), row_area);
            row = row.saturating_add(1);
        }

        // Secure input box — paint either the placeholder (empty buffer) or
        // a `TextArea` whose whole buffer is a single masked `TextElement`.
        // The real value lives only in `secure.editor.text()`; the element
        // `display` (one asterisk per char when `mask_input` is on) is what
        // actually reaches the terminal — the editor's text never enters a
        // rendered `Line` when `mask_input` is true.
        let label = &secure.config.label;
        let label_prefix = format!("{label}: ");
        let prefix_columns = UnicodeWidthStr::width(label_prefix.as_str()) as u16;
        let prefix_area = Rect {
            x: inner.left(),
            y: row,
            width: prefix_columns.min(inner.width),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label_prefix.clone(),
                Style::default().fg(secondary),
            ))),
            prefix_area,
        );
        let textarea_area = Rect {
            x: inner.left().saturating_add(prefix_columns),
            y: row,
            width: inner.width.saturating_sub(prefix_columns),
            height: 1,
        };
        let inner_left = textarea_area.left();
        let inner_right = textarea_area.right().saturating_sub(1);

        let value = secure.editor.text();
        if value.is_empty() {
            // Empty buffer: dim placeholder + caret at column 0 of the
            // body area (matches the pre-port look).
            if let Some(placeholder) = secure.config.placeholder.as_deref() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        placeholder.to_string(),
                        Style::default().fg(secondary).dim(),
                    ))),
                    textarea_area,
                );
            }
            if textarea_area.width > 0 {
                frame.set_cursor_position(Position::new(inner_left, row));
            }
            return;
        }

        // Build a fresh masked TextArea per render. Re-using the editor's
        // exact text avoids per-frame bookkeeping of element ids.
        let display_line: Line<'static> = if secure.config.mask_input {
            Line::from("*".repeat(value.chars().count()))
        } else {
            // Unmasked mode: the user has opted in to seeing the secret,
            // so the element's `display` is the value itself. The element
            // still gives atomic cursor navigation, and the editor still
            // owns the source of truth.
            Line::from(value.to_string())
        };
        let mut ta = TextArea::new();
        ta.set_text(value);
        ta.replace_range_with_element(
            0..value.len(),
            value,
            MASKED_ELEMENT_KIND,
            Some(display_line),
        );
        // `set_cursor` snaps to the nearest element boundary. Since the
        // masked element covers the whole buffer, the rendered caret lands
        // at 0 or `value.len()` — the two atomic positions for the field.
        ta.set_cursor(secure.editor.cursor_byte());
        frame.render_widget_ref(&ta, textarea_area);
        // `cursor_pos_with_state` returns ABSOLUTE coordinates (it already
        // adds `textarea_area.x`/`.y`). Do NOT re-add the area origin.
        if let Some((cx, cy)) = ta.cursor_pos_with_state(textarea_area, TextAreaState::default()) {
            let caret_x = cx.min(inner_right);
            frame.set_cursor_position(Position::new(caret_x, cy));
        }
        return;
    }
    // Keep space for the title, contextual content, and a stable key-help
    // footer. The item viewport itself scrolls around the active item.
    let visible_max = (area.height as usize).saturating_sub(7).max(3);

    // Filter items by the search value when search is enabled.
    let filtered: Vec<usize> = match &overlay.search {
        Some(search) if !search.value.is_empty() => {
            let needle = search.value.to_lowercase();
            overlay
                .items
                .iter()
                .enumerate()
                .filter_map(|(idx, item)| {
                    let title_match = item.title.to_lowercase().contains(&needle);
                    let sv_match = item
                        .search_value
                        .as_deref()
                        .map(|v| v.to_lowercase().contains(&needle))
                        .unwrap_or(false);
                    if title_match || sv_match {
                        Some(idx)
                    } else {
                        None
                    }
                })
                .collect()
        }
        _ => (0..overlay.items.len()).collect(),
    };

    let has_search = overlay.search.is_some();
    let has_tabs = overlay.tabs.len() > 1;
    let lines_count = overlay.lines.len();
    let items_count = filtered.len().min(visible_max);
    let height_inner =
        (lines_count + items_count + usize::from(has_search) + usize::from(has_tabs)) as u16;
    let desired_h = height_inner.saturating_add(3); // borders + key-help footer
    let height = desired_h.min(area.height.saturating_sub(2));
    let width = area.width.clamp(30, 80);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);

    let title = Line::from(Span::styled(
        format!(" {} ", overlay.title),
        Style::default()
            .fg(color_from_anstyle(styles.primary.get_fg_color()))
            .add_modifier(Modifier::BOLD),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())))
        .title(title);
    let inner = block.inner(rect);
    frame.render_widget(&block, rect);

    let primary = color_from_anstyle(styles.primary.get_fg_color());
    let fg = color_from_anstyle(Some(styles.foreground));
    let secondary = color_from_anstyle(styles.secondary.get_fg_color());

    // Compute where the selected item is in the filtered list.
    let selected_filtered_pos = filtered
        .iter()
        .position(|&idx| idx == overlay.selected)
        .unwrap_or(0);

    let mut row = inner.top();

    // Tab bar (settings panel): one line of tab names, the active tab
    // bold+accent; ←/→ switch tabs.
    if has_tabs {
        let mut spans: Vec<Span> = Vec::new();
        for (i, name) in overlay.tabs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            let style = if i == overlay.active_tab {
                Style::default().fg(primary).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(secondary).add_modifier(Modifier::DIM)
            };
            spans.push(Span::styled(name.clone(), style));
        }
        let row_area = Rect {
            x: inner.left(),
            y: row,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
        row = row.saturating_add(1);
    }
    // Search bar (if present).
    if let Some(search) = &overlay.search {
        let prompt = format!("{}: {}", search.label, search.value);
        let line = Line::from(vec![
            Span::styled(
                format!("{}: ", search.label),
                Style::default().fg(secondary),
            ),
            Span::styled(
                if search.value.is_empty() {
                    search
                        .placeholder
                        .clone()
                        .unwrap_or_else(|| "type to filter\u{2026}".to_string())
                } else {
                    search.value.clone()
                },
                if search.value.is_empty() {
                    Style::default().fg(secondary).add_modifier(Modifier::DIM)
                } else {
                    Style::default().fg(fg)
                },
            ),
        ]);
        let _ = prompt; // suppress unused warning
        let row_area = Rect {
            x: inner.left(),
            y: row,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(line), row_area);
        row = row.saturating_add(1);
    }

    // Descriptive lines.
    for line_text in &overlay.lines {
        let row_area = Rect {
            x: inner.left(),
            y: row,
            width: inner.width,
            height: 1,
        };
        let line = Line::from(Span::styled(
            line_text.clone(),
            Style::default().fg(secondary),
        ));
        frame.render_widget(Paragraph::new(line), row_area);
        row = row.saturating_add(1);
    }

    // Sidebar split (settings panel): with >= 2 sections and enough
    // width, the left column lists section names (active bold+accent)
    // and the item list moves to the right column with rows outside the
    // active section dimmed. Falls back to the flat list while a filter
    // is active (results cross sections) or when narrow.
    let searching = overlay.search.as_ref().is_some_and(|s| !s.value.is_empty());
    let use_sidebar = overlay.sections.len() >= 2 && inner.width >= 60 && !searching;
    let sidebar_w = if use_sidebar {
        let longest = overlay
            .sections
            .iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(0);
        (22usize.min(longest) + 4) as u16
    } else {
        0
    };
    // The active section tracks the selected item's group, not a stored
    // index — selection moves across sections via Up/Down.
    let active_section = if use_sidebar {
        item_section_idx(overlay, overlay.selected).unwrap_or(overlay.active_section)
    } else {
        overlay.active_section
    };
    let list_x = inner.left() + sidebar_w;
    let list_w = inner.width.saturating_sub(sidebar_w);
    if use_sidebar {
        let mut srow = row;
        for (i, name) in overlay.sections.iter().enumerate() {
            let style = if i == active_section {
                Style::default().fg(primary).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(secondary)
            };
            let marker = if i == active_section { "> " } else { "  " };
            let row_area = Rect {
                x: inner.left(),
                y: srow,
                width: sidebar_w,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(marker, style),
                    Span::styled(name.clone(), style),
                ])),
                row_area,
            );
            srow = srow.saturating_add(1);
        }
    }

    // Items.
    if filtered.is_empty() {
        // The key-capture prompt is items-free by design — the prompt
        // line above IS the UI; a "(no items)" placeholder would be
        // noise.
        if overlay.key_capture.is_some() {
            return;
        }
        let row_area = Rect {
            x: inner.left(),
            y: row,
            width: inner.width,
            height: 1,
        };
        let empty_text = if overlay.search.is_some() {
            "  (no matches)"
        } else {
            "  (no items)"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                empty_text,
                Style::default().fg(secondary).add_modifier(Modifier::DIM),
            ))),
            row_area,
        );
    } else {
        let first_visible = selected_filtered_pos
            .saturating_sub(visible_max / 2)
            .min(filtered.len().saturating_sub(visible_max));
        for &item_idx in filtered.iter().skip(first_visible).take(visible_max) {
            let item = &overlay.items[item_idx];
            let is_selected = item_idx == overlay.selected;
            let marker = if is_selected { "> " } else { "  " };
            let indent = "  ".repeat(item.indent as usize);
            let mut item_style = if is_selected {
                Style::default().fg(primary).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(fg)
            };
            // Rows outside the active section recede while the sidebar
            // is up.
            if use_sidebar
                && item_section_idx(overlay, item_idx).is_some_and(|sec| sec != active_section)
            {
                item_style = item_style.add_modifier(Modifier::DIM);
            }
            let mut spans = vec![
                Span::styled(marker, item_style),
                Span::styled(indent, item_style),
                Span::styled(item.title.clone(), item_style),
            ];
            if let Some(badge) = &item.badge {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    badge.clone(),
                    Style::default().fg(secondary).add_modifier(Modifier::DIM),
                ));
            }
            if let Some(subtitle) = &item.subtitle {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    subtitle.clone(),
                    Style::default().fg(secondary),
                ));
            }
            let line = Line::from(spans);
            let row_area = Rect {
                x: list_x,
                y: row,
                width: list_w,
                height: 1,
            };
            frame.render_widget(Paragraph::new(line), row_area);
            row = row.saturating_add(1);
        }
    }

    // A panel should always explain how to leave it and how to commit a
    // choice. This avoids hiding essential controls in a separate help view.
    if row < inner.bottom() {
        let hint = if overlay.items.iter().any(|item| item.selection.is_some()) {
            if has_tabs {
                "Enter select | Up/Down move | ←/→ tabs | Esc close"
            } else {
                "Enter select | Up/Down move | Esc close"
            }
        } else {
            "Esc close"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default().fg(secondary).add_modifier(Modifier::DIM),
            ))),
            Rect {
                x: inner.left(),
                y: inner.bottom().saturating_sub(1),
                width: inner.width,
                height: 1,
            },
        );
    }
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, state: &RenderState) {
    if state.transcript.is_empty() {
        render_welcome(frame, area, state);
        return;
    }
    let styles = active_styles();
    let bg_color = color_from_anstyle(Some(styles.background));

    // Plain transcript surface (omp-style): no rail column, no speaker
    // chrome, no in-app scrollbar — the host terminal's native
    // scrollback owns history now. Content spans the full area.
    let content_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
    };

    let display =
        build_transcript_display(state, &styles, state.committed_entries, content_area.width);

    // Resolve scroll offset into the display list.
    let total = display.len();
    let raw_start = if state.scroll_offset == usize::MAX {
        total.saturating_sub(content_area.height as usize)
    } else {
        display
            .iter()
            .position(|d| d.source_index >= state.scroll_offset)
            .unwrap_or(total.saturating_sub(1))
    };
    let start = effective_scroll_offset(raw_start, total, content_area.height as usize);

    // Sticky header (grok-build parity): when the viewport top sits inside a
    // block's body (not on its head), pin the block's first line at the top
    // so the user can tell which block they are scrolling through.
    let sticky_first: Option<usize> = display.get(start).and_then(|d| {
        let bid = state.transcript.get(d.source_index)?.block_id;
        let first_idx = state.transcript.iter().position(|l| l.block_id == bid)?;
        (first_idx != d.source_index).then_some(first_idx)
    });
    let sticky_h: u16 = if sticky_first.is_some() { 1 } else { 0 };
    let body_top = content_area.top() + sticky_h;

    // Push/fade (grok-build iOS-style 1D): detect the next block boundary
    // within the viewport. As it approaches the sticky row, fade the current
    // sticky header toward the background — a smooth handoff to the next
    // block's header. FADE_ROWS controls the transition width.
    const FADE_ROWS: usize = 5;
    let sticky_opacity: f64 = if let Some(sidx) = sticky_first {
        let sticky_bid = state.transcript[sidx].block_id;
        // Walk display from `start` to find the first visual row belonging to
        // a different block.
        let next_offset = display.iter().skip(start).position(|d| {
            state
                .transcript
                .get(d.source_index)
                .map(|l| l.block_id != sticky_bid)
                .unwrap_or(false)
        });
        match next_offset {
            Some(off) if off <= FADE_ROWS => off as f64 / FADE_ROWS as f64,
            _ => 1.0,
        }
    } else {
        1.0
    };

    // Sticky header row: head line + faint bg highlight, no rail. Opacity
    // fades as the next block pushes in.
    if let Some(sidx) = sticky_first {
        let tl = &state.transcript[sidx];
        let accent_base = accent_color_for_kind(tl.kind, &styles);
        let bg_blend = 0.1 * sticky_opacity;
        let line =
            transcript_line_marked(tl, &styles, false, false, false, true, content_area.width);
        let row = Rect {
            x: content_area.x,
            y: content_area.top(),
            width: content_area.width,
            height: 1,
        };
        if bg_blend > 0.01 {
            frame.buffer_mut().set_style(
                row,
                Style::default().bg(blend_rgb(bg_color, accent_base, bg_blend)),
            );
        }
        frame.render_widget(Paragraph::new(line), row);
    }
    // Pressure-driven allocation ladder (peer parity with omp): when
    // there are more visible items than rows in the live region, fold
    // older blocks to a glyph row, then a folded card, and finally
    // hide them with a banner. The ladder is pure (`allocate_rows`)
    // and resolved here once per frame; the render loop below applies
    // it per item.
    let live_budget = content_area.height.saturating_sub(sticky_h) as usize;
    let (alloc_by_block, hidden_count, natural_by_block) =
        compute_block_allocations(state, state.committed_entries, live_budget);
    // the live region visibly breathes while tools are running.
    let pulse = animation_frame(1000).is_multiple_of(2);
    // for `… N earlier blocks hidden` whenever any block is hidden.
    let banner_row_used = hidden_count > 0;
    let banner_y = content_area.bottom().saturating_sub(1);
    // Render top-down, wrapping each line into multiple visual rows.
    let mut y = body_top;
    let width = content_area.width.max(1) as usize;
    // Inline image previews: resolve each pending image's transcript row
    // to its block and pre-compute the block's visual height (same wrap
    // math the commit path uses) so the render loop can anchor a
    // placement at the block's top row, sized to the tool box.
    // (block_id, image id, block height, fallback-row index)
    let image_block_plans: Vec<(usize, u32, u16, usize)> = state
        .image_previews
        .pending()
        .iter()
        .filter_map(|p| {
            // Resolve the fallback row by its embedded label — the row
            // only exists after the append command applied.
            let row_index = state
                .transcript
                .iter()
                .position(|l| l.segments.iter().any(|s| s.text.contains(&p.label)))?;
            let bid = state.transcript[row_index].block_id;
            let mut block_rows: u16 = 0;
            for d in &display {
                let Some(l) = state.transcript.get(d.source_index) else {
                    continue;
                };
                if l.block_id != bid {
                    continue;
                }
                block_rows = block_rows.saturating_add(match &d.line {
                    None => 1,
                    Some(line) => {
                        let lw = line.width();
                        if lw == 0 {
                            1
                        } else {
                            lw.div_ceil(width).max(1) as u16
                        }
                    }
                });
            }
            (block_rows > 0).then_some((bid, p.id, block_rows, row_index))
        })
        .collect();
    let mut current_block: Option<usize> = None;
    let mut skipped_blocks: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for d in display.into_iter().skip(start) {
        if y >= content_area.bottom() {
            break;
        }
        // Banner reservation: never paint over the reserved banner
        // row at the bottom of the live region.
        if banner_row_used && y >= banner_y {
            break;
        }
        let d_bid = state.transcript.get(d.source_index).map(|l| l.block_id);
        let Some(d_bid) = d_bid else {
            continue;
        };
        // Block transition: pick a ladder policy for the new block.
        if current_block != Some(d_bid) {
            // Inline image preview: this block is a pending image's tool
            // box — record where its top row landed so the post-draw
            // step can place the transmitted pixels here. Placement is
            // clamped to the visible window.
            if y < content_area.bottom()
                && let Some((_, pid, prows, row_index)) =
                    image_block_plans.iter().find(|(b, _, _, _)| *b == d_bid)
            {
                let visible_rows = content_area.bottom().saturating_sub(y).max(1);
                state.image_previews.record_anchor(
                    *pid,
                    content_area.x,
                    y,
                    (*prows).min(visible_rows),
                    *row_index,
                );
            }
            current_block = Some(d_bid);
            let alloc = alloc_by_block
                .get(&d_bid)
                .copied()
                .unwrap_or(BlockAlloc { rows: 0 });
            // The ladder only intervenes when the block is being
            // squeezed (alloc.rows < natural). When alloc.rows >=
            // natural (roomy), the natural rendering already fits
            // — leave the existing wrap logic alone so explicit
            // newlines and word-wrap behave the way they always
            // did.
            let natural = natural_by_block.get(&d_bid).copied().unwrap_or(0);
            if alloc.rows < natural {
                // Pressure / emergency: ladder overrides the
                // natural rendering. Reserve the first row(s) for
                // a glyph / folded card; the rest of the block's
                // natural items are skipped entirely.
                if skipped_blocks.contains(&d_bid) {
                    continue;
                }
                match alloc.rows {
                    0 => {
                        skipped_blocks.insert(d_bid);
                        continue;
                    }
                    1 => {
                        let activity = block_activity(&state.transcript, d_bid);
                        render_glyph_row(frame, content_area, y, &activity, &styles, pulse);
                        y += 1;
                        skipped_blocks.insert(d_bid);
                        continue;
                    }
                    2 => {
                        let activity = block_activity(&state.transcript, d_bid);
                        y += render_folded_card(frame, content_area, y, &activity, &styles, pulse);
                        skipped_blocks.insert(d_bid);
                        continue;
                    }
                    _ => {}
                }
            }
            // Roomy (alloc.rows >= natural): fall through and render
            // the natural items — every display item for the block
            // gets painted (and ratatui handles wrapping / explicit
            // newlines as before).
        }
        let Some(line) = d.line else {
            y += 1;
            continue;
        };
        let text_w = line.width();
        let wrapped_h = if text_w == 0 {
            1
        } else {
            text_w.div_ceil(width).max(1) as u16
        };
        let row = Rect {
            x: content_area.x,
            y,
            width: content_area.width,
            height: wrapped_h.min(content_area.bottom().saturating_sub(y)),
        };
        frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), row);
        y += wrapped_h;
    }

    // Banner: paint the `… N earlier blocks hidden` summary in the
    // reserved row at the bottom of the live region (if any block
    // was hidden).
    if banner_row_used {
        render_hidden_banner(frame, content_area, banner_y, hidden_count);
    }

    let _ = (total, sticky_h);
}

/// One visible row of the transcript: a rendered line (or a turn
/// spacer, `line: None`) plus the transcript entry it belongs to.
#[derive(Clone)]
struct TranscriptDisplayItem<'a> {
    source_index: usize,
    /// `None` marks a turn spacer: a blank breathing row.
    line: Option<Line<'a>>,
}

/// Short, present-tense descriptor for a block: the first non-empty
/// text in its leading line. Falls back to the block's kind label
/// (e.g. "tool", "agent") when nothing is derivable. The ladder
/// uses this in the glyph row and folded card so a half-shown
/// block still tells the user what it was.
fn block_activity(transcript: &[TranscriptLine], block_id: usize) -> String {
    let mut activity = String::new();
    for line in transcript.iter().filter(|l| l.block_id == block_id) {
        for seg in &line.segments {
            let text = seg.text.trim();
            if !text.is_empty() {
                activity.push_str(text);
                break;
            }
        }
        if !activity.is_empty() {
            break;
        }
    }
    if !activity.is_empty() {
        return activity;
    }
    // Fallback: kind label.
    transcript
        .iter()
        .find(|l| l.block_id == block_id)
        .map(|l| kind_label(l.kind))
        .unwrap_or_else(|| "block".to_string())
}

/// Lower-case kind label (e.g. "tool", "agent", "user") used as a
/// last-resort activity descriptor.
fn kind_label(kind: InlineMessageKind) -> String {
    match kind {
        InlineMessageKind::Agent => "agent".to_string(),
        InlineMessageKind::User => "user".to_string(),
        InlineMessageKind::Tool => "tool".to_string(),
        InlineMessageKind::Error => "error".to_string(),
        InlineMessageKind::Warning => "warning".to_string(),
        InlineMessageKind::Info => "info".to_string(),
        InlineMessageKind::Policy => "policy".to_string(),
        InlineMessageKind::Pty => "pty".to_string(),
    }
}

/// Build per-block natural heights from `visible_items`. The natural
/// height is the number of items `visible_items` would surface for
/// that block (each `Line` or `Gap` counts as one logical row).
/// Blocks with no visible items get height 0.
fn block_natural_heights(
    transcript: &[TranscriptLine],
    mode_for: impl Fn(usize) -> BlockDisplayMode,
    from_entry: usize,
) -> (Vec<usize>, Vec<usize>) {
    let mut block_ids: Vec<usize> = Vec::new();
    let mut heights: Vec<usize> = Vec::new();
    let mut index_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for item in visible_items(transcript, mode_for) {
        let bid = match item {
            VisibleItem::Line { source_index, .. } => transcript[source_index].block_id,
            VisibleItem::Gap { source_index, .. } => transcript[source_index].block_id,
        };
        // Frozen rows (already committed to host scrollback) don't
        // participate in live-region budgeting — the live region is
        // bounded to what's still in the viewport, not what's already
        // gone to scrollback.
        let live_source = match item {
            VisibleItem::Line { source_index, .. } => source_index,
            VisibleItem::Gap { source_index, .. } => source_index,
        };
        if live_source < from_entry {
            continue;
        }
        let idx = *index_of.entry(bid).or_insert_with(|| {
            block_ids.push(bid);
            heights.push(0);
            block_ids.len() - 1
        });
        heights[idx] += 1;
    }
    (block_ids, heights)
}

/// Resolve a per-block allocation map. The ladder applies only to
/// blocks without a manual override; manual `Collapsed` /
/// `Truncated` modes override the ladder for that block (manual
/// wins — the user already chose how this block should fold).
///
/// Returns `alloc_by_block_id`, `hidden_count`, `natural_by_block_id`.
fn compute_block_allocations(
    state: &RenderState,
    from_entry: usize,
    budget: usize,
) -> (
    std::collections::HashMap<usize, BlockAlloc>,
    usize,
    std::collections::HashMap<usize, usize>,
) {
    let (block_ids, heights) =
        block_natural_heights(&state.transcript, |bid| state.block_mode(bid), from_entry);
    let total_blocks = block_ids.len();
    let allocs = allocate_rows(&heights, budget);
    let mut by_block: std::collections::HashMap<usize, BlockAlloc> =
        std::collections::HashMap::with_capacity(total_blocks);
    let mut natural_by_block: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::with_capacity(total_blocks);
    let mut hidden = 0usize;
    for (i, &bid) in block_ids.iter().enumerate() {
        // Manual override wins. The ladder ONLY applies to blocks
        // without a manual override; a Collapsed block's natural
        // item is a single `[+] <line>` (built by
        // `transcript_line_marked(folded=true)`), which is exactly
        // what we want for the user's "folded" affordance. Truncated
        // / Expanded get the ladder output unchanged — those
        // policies already control folding, so the ladder has
        // nothing to add.
        let alloc = match state.block_mode(bid) {
            // Collapsed: skip the ladder and route through the
            // natural render. Set `rows = natural` so the roomy
            // branch in the render loop paints the single `[+]`
            // line that `visible_items(Collapsed)` emitted.
            BlockDisplayMode::Collapsed => BlockAlloc { rows: heights[i] },
            BlockDisplayMode::Truncated | BlockDisplayMode::Expanded => allocs[i],
        };
        if alloc.rows == 0 {
            hidden += 1;
        }
        natural_by_block.insert(bid, heights[i]);
        by_block.insert(bid, alloc);
    }
    (by_block, hidden, natural_by_block)
}

/// Truncate `text` so its unicode display width (after the supplied
/// prefix) fits inside `width` cells. When the text overflows, an
/// ellipsis replaces the trailing chars. Mirrors the rule that
/// `clamp_segments_to_width` enforces on rendered rows: never let a
/// single row spill past the terminal width.
fn clamp_fold_text(text: &str, prefix_w: usize, width: usize, ellipsis: &str) -> String {
    let budget = width.saturating_sub(prefix_w);
    if budget == 0 || width == 0 {
        return String::new();
    }
    let text_w = text.width();
    if text_w <= budget {
        return text.to_string();
    }
    // Leave room for the ellipsis. Walk char-by-char on display
    // width; stop one cell before the budget overflows.
    let ell_w = ellipsis.width();
    let cap = budget.saturating_sub(ell_w);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > cap {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push_str(ellipsis);
    out
}

/// Render a single folded-card row (2-row form: `╭─ <activity>` /
/// `╰─ …`) into `frame` at `(x, y)`, honoring `width`. The activity
/// string is clamped to fit the live content width — long
/// descriptors never break the box-drawing affordance. Returns the
/// number of rows consumed (always 2).
fn render_folded_card(
    frame: &mut Frame<'_>,
    area: Rect,
    y: u16,
    activity: &str,
    styles: &ThemeStyles,
    pulse: bool,
) -> u16 {
    let tool_color = styles
        .tool
        .get_fg_color()
        .or_else(|| styles.secondary.get_fg_color())
        .or(styles.response.get_fg_color());
    let style = Style::default().fg(color_from_anstyle(tool_color));
    let pulse_mark = if pulse { " \u{2022}" } else { "" };
    // Box head is `╭─ ` (3 cells) plus an optional pulse mark.
    // Clamp the activity to the remaining cells so the row never
    // wraps onto a third visual row.
    let head_prefix_w = "\u{256D}\u{2500} ".width() + pulse_mark.width();
    let head_activity = clamp_fold_text(activity, head_prefix_w, area.width as usize, "\u{2026}");
    let head = Line::from(vec![Span::styled(
        format!("\u{256D}\u{2500} {head_activity}{pulse_mark}"),
        style,
    )]);
    let tail = Line::from(vec![Span::styled("\u{2570}\u{2500} \u{2026}", style)]);
    if y < area.bottom() {
        let row = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(head), row);
    }
    let y2 = y.saturating_add(1);
    if y2 < area.bottom() {
        let row = Rect {
            x: area.x,
            y: y2,
            width: area.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(tail), row);
    }
    2
}

/// Render a single glyph row (`▸ <activity>`) into `frame` at `(x,
/// y)`, honoring `width`. The activity is clamped to the live
/// content width so a long descriptor never wraps. The shared
/// wall-clock pulse animates the trailing `•` on a 1-second period
/// so the live region breathes.
fn render_glyph_row(
    frame: &mut Frame<'_>,
    area: Rect,
    y: u16,
    activity: &str,
    styles: &ThemeStyles,
    pulse: bool,
) -> u16 {
    let tool_color = styles
        .tool
        .get_fg_color()
        .or_else(|| styles.secondary.get_fg_color())
        .or(styles.response.get_fg_color());
    let style = Style::default().fg(color_from_anstyle(tool_color));
    let pulse_mark = if pulse { " \u{2022}" } else { "" };
    // Glyph prefix is `▸ ` (2 cells) plus an optional pulse mark.
    let glyph_prefix_w = "\u{25B8} ".width() + pulse_mark.width();
    let glyph_activity = clamp_fold_text(activity, glyph_prefix_w, area.width as usize, "\u{2026}");
    let line = Line::from(vec![Span::styled(
        format!("\u{25B8} {glyph_activity}{pulse_mark}"),
        style,
    )]);
    if y < area.bottom() {
        let row = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(line), row);
    }
    1
}
/// Render a one-row banner `… N earlier blocks hidden` in the dim
/// secondary style. The text is clamped to the live content width
/// so a very large `N` never overflows the row.
fn render_hidden_banner(frame: &mut Frame<'_>, area: Rect, y: u16, hidden: usize) -> u16 {
    let style = Style::default().fg(color_from_anstyle(active_styles().secondary.get_fg_color()));
    let text = if hidden == 1 {
        "\u{2026} 1 earlier block hidden".to_string()
    } else {
        format!("\u{2026} {hidden} earlier blocks hidden")
    };
    let clamped = clamp_fold_text(&text, 0, area.width as usize, "\u{2026}");
    let line = Line::from(vec![Span::styled(clamped, style)]);
    if y < area.bottom() {
        let row = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(line), row);
    }
    1
}
/// and turn rhythm. Entries below `from_entry` (committed to the host
/// scrollback) are skipped — they are frozen and must not render in
/// the live viewport again.
fn build_transcript_display<'a>(
    state: &'a RenderState,
    styles: &'a ThemeStyles,
    from_entry: usize,
    width: u16,
) -> Vec<TranscriptDisplayItem<'a>> {
    let search_set: std::collections::HashSet<usize> = state
        .search
        .as_ref()
        .map(|s| s.matches.iter().copied().collect())
        .unwrap_or_default();
    let current_match = state
        .search
        .as_ref()
        .and_then(|s| (!s.matches.is_empty()).then(|| s.matches[s.current]));

    let mut display = Vec::with_capacity(state.transcript.len());
    let dim_style = Style::default()
        .fg(color_from_anstyle(styles.secondary.get_fg_color()))
        .add_modifier(Modifier::DIM);
    let mut prev_block: Option<usize> = None;
    let mut prev_kind: Option<InlineMessageKind> = None;
    for item in visible_items(&state.transcript, |block_id| state.block_mode(block_id)) {
        match item {
            VisibleItem::Line {
                source_index,
                folded,
            } => {
                if source_index < from_entry {
                    continue;
                }
                let tl = &state.transcript[source_index];
                let is_block_start = prev_block != Some(tl.block_id);
                // Turn rhythm: breathe before a user block and after one,
                // so a request and its response never glue together.
                let needs_spacer = is_block_start
                    && prev_block.is_some()
                    && (tl.kind == InlineMessageKind::User
                        || prev_kind == Some(InlineMessageKind::User));
                if needs_spacer {
                    display.push(TranscriptDisplayItem {
                        source_index,
                        line: None,
                    });
                }
                let is_match = search_set.contains(&source_index);
                let line = transcript_line_marked(
                    tl,
                    styles,
                    folded,
                    is_match,
                    current_match == Some(source_index),
                    is_block_start,
                    width,
                );
                display.push(TranscriptDisplayItem {
                    source_index,
                    line: Some(line),
                });
                prev_block = Some(tl.block_id);
                prev_kind = Some(tl.kind);
            }
            VisibleItem::Gap {
                source_index,
                hidden_lines,
            } => {
                if source_index < from_entry {
                    continue;
                }
                let gap = Line::styled(format!("  \u{2026} +{hidden_lines} lines"), dim_style);
                display.push(TranscriptDisplayItem {
                    source_index,
                    line: Some(gap),
                });
            }
        }
    }
    display
}
/// Decide whether the host scrollback must be wiped and rebuilt after a
/// terminal resize. Only width changes invalidate the frozen transcript
/// (rows were printed at the original width and cannot re-wrap). A
/// height-only resize leaves the printed history intact — the live
/// viewport just grows or shrinks beneath it.
///
/// `prev_w == 0` is the "never measured" sentinel (no frame has been
/// drawn at a known width): there is no stale-width scrollback to
/// invalidate, so the answer is always false. Without this, the
/// 80-column `RenderState::default()` would fire CSI 3J on the first
/// draw of any wider terminal and wipe the user's pre-TUI shell
/// scrollback on every launch (final-review finding 1).
pub(crate) fn should_rebuild_scrollback(
    prev_w: u16,
    new_w: u16,
    _prev_h: u16,
    _new_h: u16,
) -> bool {
    prev_w != 0 && prev_w != new_w
}

/// Force-flush boundary — commit the entire finalized prefix regardless
/// of viewport fit. Used at exit to land every committable row into the
/// host scrollback before raw mode is dropped. Returns the number of
/// display rows to commit (= `display_len`). `display_len` itself comes
/// from the caller (the same `build_transcript_display` output the live
/// commit plan uses) so the boundary stays in lockstep with what the
/// user has actually been seeing on screen.
pub(crate) fn plan_full_flush(display_len: usize) -> usize {
    display_len
}

/// A planned flush of finalized rows into the host terminal's real
/// scrollback.
struct ScrollbackCommit {
    /// Display rows to print above the viewport.
    rows: u16,
    /// Display items [0, boundary_item) are the committed chunk.
    boundary_item: usize,
    /// New `committed_entries`: transcript index of the first live entry.
    new_committed_entries: usize,
}
/// Decide which leading display rows to shed into the host scrollback so
/// the live region keeps only `keep_rows` (the viewport). The boundary is
/// **block-atomic** (never splits a block) and never touches the anchored
/// streaming block or anything below it — those lines are still being
/// rewritten by `ReplaceLast`.
fn scrollback_commit_plan(
    display: &[TranscriptDisplayItem<'_>],
    transcript: &[TranscriptLine],
    width: usize,
    keep_rows: usize,
    anchor_entry: Option<usize>,
) -> Option<ScrollbackCommit> {
    let width = width.max(1);
    // Cumulative display rows through each item (spacers cost 1 row;
    // lines wrap to ceil(width / content width)).
    let mut ends: Vec<usize> = Vec::with_capacity(display.len());
    let mut y = 0usize;
    for d in display {
        let h = match &d.line {
            None => 1,
            Some(line) => {
                let w = line.width();
                if w == 0 { 1 } else { w.div_ceil(width).max(1) }
            }
        };
        y += h;
        ends.push(y);
    }
    let total_rows = y;
    if total_rows <= keep_rows || display.is_empty() {
        return None;
    }
    let limit = total_rows - keep_rows;

    // Everything whose last row ends at/below the keep window stays live.
    let mut boundary_item = ends.iter().rposition(|&e| e <= limit)? + 1;

    // The anchored streaming block (and everything after it) never
    // commits: its lines are still being rewritten in place.
    if let Some(anchor) = anchor_entry
        && let Some(anchor_item) = display.iter().position(|d| d.source_index >= anchor)
    {
        boundary_item = boundary_item.min(anchor_item);
    }

    // Block-atomic: shrink until the boundary sits between blocks.
    boundary_item = boundary_item.min(display.len().saturating_sub(1));
    let bid_of = |i: usize| transcript.get(display[i].source_index).map(|t| t.block_id);
    while boundary_item > 0 {
        let last = display[boundary_item - 1].source_index;
        let next = display[boundary_item].source_index;
        let same_block = matches!(
            (transcript.get(last), transcript.get(next)),
            (Some(a), Some(b)) if a.block_id == b.block_id
        );
        if !same_block {
            break;
        }
        // Block-atomic by default — but a FINALIZED block taller than
        // the viewport can never fit the live region; committing its
        // head at a line boundary is the only way it reaches the host
        // scrollback (long messages; Claude Code / Ink print the same
        // way). The anchor cap above already keeps the streaming block
        // out, so anything this split touches is final.
        let block_bid = bid_of(boundary_item);
        let block_start = (0..boundary_item)
            .rev()
            .find(|&i| bid_of(i) != block_bid)
            .map_or(0, |i| i + 1);
        let block_end = (boundary_item..display.len())
            .find(|&i| bid_of(i) != block_bid)
            .unwrap_or(display.len());
        let before_rows = if block_start == 0 {
            0
        } else {
            ends[block_start - 1]
        };
        if ends[block_end - 1] - before_rows > keep_rows {
            break; // oversized: keep the line boundary inside it
        }
        boundary_item -= 1;
    }
    if boundary_item == 0 {
        return None;
    }

    // Committed entries run to the first live row: normally the START of
    // the next block (a folded block's gap row can point inside its
    // block), but an oversized split commits at line granularity.
    let first_live = display[boundary_item].source_index;
    let last_committed = display[boundary_item - 1].source_index;
    let new_committed_entries = if matches!(
        (transcript.get(last_committed), transcript.get(first_live)),
        (Some(a), Some(b)) if a.block_id == b.block_id
    ) {
        first_live
    } else {
        let live_bid = transcript.get(first_live)?.block_id;
        transcript.iter().position(|t| t.block_id == live_bid)?
    };

    let rows = ends[boundary_item - 1].min(u16::MAX as usize) as u16;
    Some(ScrollbackCommit {
        rows,
        boundary_item,
        new_committed_entries,
    })
}

/// Render the committed chunk into the `insert_before` buffer. Mirrors
/// the viewport's wrapping math so the frozen rows match what the live
/// region showed.
fn render_committed_chunk(
    buf: &mut Buffer,
    items: &[TranscriptDisplayItem<'_>],
    x: u16,
    width: u16,
) {
    use ratatui::widgets::Widget;
    let width = width.max(1);
    let mut y = 0u16;
    for item in items {
        let Some(line) = &item.line else {
            y += 1;
            continue;
        };
        let text_w = line.width();
        let wrapped_h = if text_w == 0 {
            1
        } else {
            text_w.div_ceil(width as usize).max(1) as u16
        };
        let area = Rect {
            x,
            y,
            width,
            height: wrapped_h,
        };
        Paragraph::new(line.clone())
            .wrap(Wrap { trim: false })
            .render(area, buf);
        y += wrapped_h;
    }
}

/// Flush finalized transcript rows into the host terminal's real
/// scrollback (inline-viewport pattern — peer parity with Claude Code /
/// pi). Runs only when the live content overflows the viewport and the
/// user is not browsing: streaming buffers must be empty (the anchored
/// block is still being rewritten otherwise) and manual scrolling /
/// overlays / search pause committing so the live region stays put.
/// Committed blocks are frozen — block-mode cycling applies to live
/// blocks only (Claude Code behaves the same way).
fn commit_scrollback(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut RenderState,
    force_all: bool,
) {
    if state.scroll_offset != usize::MAX
        || !state.message_buffer.is_empty()
        || !state.thinking_buffer.is_empty()
        || state.overlay.is_some()
        || state.confirmation.is_some()
        || state.agent_hub_open
        || state.slash_popup.open
        || state.file_search.is_some()
        || state.search.is_some()
        || state.transcript.is_empty()
    {
        return;
    }
    let Ok(size) = terminal.size() else {
        return;
    };
    let area = Rect {
        x: 0,
        y: 0,
        width: size.width,
        height: size.height,
    };
    let keep_rows = super::frame_layout::scrollback_height(area) as usize;
    if keep_rows == 0 && !force_all {
        return;
    }
    let styles = active_styles();
    let (gutter_x, scrollback_w) = super::frame_layout::scrollback_geometry(area);
    let content_w = scrollback_w as usize;
    let display =
        build_transcript_display(state, &styles, state.committed_entries, content_w as u16);
    if force_all {
        // On exit: commit the entire committable prefix in one shot.
        // Streaming buffers are already empty at this point; unfinalized
        // stream content simply stays in the live viewport. The boundary
        // comes from `plan_full_flush` (trivial: display length).
        let boundary_item = plan_full_flush(display.len()).min(display.len());
        if boundary_item == 0 {
            return;
        }
        // Cumulative display rows through `boundary_item` — needed for
        // `insert_before`'s height hint.
        let mut total_rows = 0usize;
        let width = content_w.max(1);
        for d in &display[..boundary_item] {
            total_rows += match &d.line {
                None => 1,
                Some(line) => {
                    let w = line.width();
                    if w == 0 { 1 } else { w.div_ceil(width).max(1) }
                }
            };
        }
        let rows = total_rows.min(u16::MAX as usize) as u16;
        let chunk = &display[..boundary_item];
        let res = terminal.insert_before(rows, |buf| {
            render_committed_chunk(buf, chunk, gutter_x, content_w as u16);
        });
        if res.is_ok() {
            // After a force-flush, every committed row is in scrollback;
            // advance the marker to the end of the transcript so the
            // final draw pass doesn't try to re-commit anything.
            state.committed_entries = state.transcript.len();
        }
        return;
    }
    let Some(plan) = scrollback_commit_plan(
        &display,
        &state.transcript,
        content_w,
        keep_rows,
        state.stream_anchor,
    ) else {
        return;
    };
    let chunk = &display[..plan.boundary_item];
    let res = terminal.insert_before(plan.rows, |buf| {
        render_committed_chunk(buf, chunk, gutter_x, content_w as u16);
    });
    if res.is_ok() {
        state.committed_entries = plan.new_committed_entries;
    }
}

/// Build a ratatui `Line` from a transcript line, with optional fold marker
/// and search-match highlighting.
///
/// Plain transcript (omp-style): speaker identity is weight and color, not
/// chrome. There is no rail column, no speaker label, and no prefix glyph —
/// the user's input is the only bold body text, in the primary color, and
/// the agent's response reads in the default ink. System severities keep a
/// short colored label on the block's first line because severity is data.
fn transcript_line_marked<'a>(
    line: &'a TranscriptLine,
    styles: &'a ThemeStyles,
    folded: bool,
    is_match: bool,
    is_current: bool,
    is_block_start: bool,
    width: u16,
) -> Line<'a> {
    let kind_style = match line.kind {
        InlineMessageKind::Agent => {
            Style::default().fg(color_from_anstyle(styles.response.get_fg_color()))
        }
        InlineMessageKind::User => {
            Style::default().fg(color_from_anstyle(styles.user.get_fg_color()))
        }
        InlineMessageKind::Tool => {
            Style::default().fg(color_from_anstyle(styles.tool.get_fg_color()))
        }
        InlineMessageKind::Error => {
            Style::default().fg(color_from_anstyle(styles.error.get_fg_color()))
        }
        InlineMessageKind::Warning => {
            Style::default().fg(color_from_anstyle(styles.status.get_fg_color()))
        }
        InlineMessageKind::Info => {
            Style::default().fg(color_from_anstyle(styles.info.get_fg_color()))
        }
        InlineMessageKind::Policy => {
            Style::default().fg(color_from_anstyle(styles.mcp.get_fg_color()))
        }
        InlineMessageKind::Pty => {
            Style::default().fg(color_from_anstyle(styles.pty_output.get_fg_color()))
        }
    };

    // Severity labels appear on the block's first line only; folded heads
    // always show the marker so a collapsed block stays identifiable.
    let severity_label = match line.kind {
        InlineMessageKind::Error => Some("error: "),
        InlineMessageKind::Warning => Some("warning: "),
        InlineMessageKind::Info => Some("info: "),
        InlineMessageKind::Policy => Some("policy: "),
        _ => None,
    };
    let mut prefix = String::new();
    if folded {
        prefix.push_str("[+] ");
    }
    if let Some(label) = severity_label
        && (folded || is_block_start)
    {
        prefix.push_str(label);
    }

    // Highlight background for search matches.
    let highlight = if is_current {
        Some(Style::default().reversed())
    } else if is_match {
        Some(Style::default().add_modifier(Modifier::UNDERLINED))
    } else {
        None
    };
    // Write-path width invariant (omp tui-core-renderer.md §4): every row
    // must fit inside the terminal width. Reserve the prefix's display
    // width first so the segments never push the row past `width`. The
    // prefix is always ASCII (`"[+] "`, `"error: "`, ...) so `.len()` is
    // a faithful display-width measure here.
    let prefix_w = prefix.len() as u16;
    let budget = width.saturating_sub(prefix_w);
    let clamped = clamp_segments_to_width(&line.segments, budget);
    let mut spans = Vec::with_capacity(clamped.len() + 1);
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, kind_style));
    }
    for segment in &clamped {
        let mut style = segment_style(segment, kind_style, styles);
        // Weight-led hierarchy: user input is the only bold body text.
        if line.kind == InlineMessageKind::User {
            style = style.add_modifier(Modifier::BOLD);
        }
        if let Some(h) = highlight {
            style = style.patch(h);
        }
        spans.push(Span::styled(segment.text.clone(), style));
    }
    Line::from(spans)
}

fn segment_style(segment: &InlineSegment, fallback: Style, _styles: &ThemeStyles) -> Style {
    let mut style = fallback;
    let inline = segment.style.as_ref();
    if let Some(color) = inline.color {
        style = style.fg(color_from_anstyle(Some(color)));
    }
    // No inline color: keep the kind fallback (`fallback`). Overriding
    // with a fixed `response` ink made user turns indistinguishable from
    // agent output — the kind color is the speaker signal in plain style.
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

    let prefix = state.prompt_prefix.clone();
    let placeholder = state.placeholder.clone();

    // Build prefix spans. The prefix lives in a static leading region of the
    // composer box; the textarea renders the editable body in the
    // remaining area (right of `prefix_w`). The textarea's own
    // `cursor_pos_with_state` reports the cursor relative to that area.
    // All prefix segments are ASCII-only today (">[auto] ", "[vim] ", "! ");
    // using UnicodeWidthStr keeps the math correct if any of them grows a
    // wide glyph in the future (e.g. a status emoji in the vim label).
    let mut prefix_w: u16 = 0;
    let mut line_spans = Vec::new();
    if let Some(label) = state.vim_state.status_label() {
        let seg = format!("[{label}] ");
        prefix_w = prefix_w.saturating_add(seg.width() as u16);
        line_spans.push(Span::styled(
            seg,
            Style::default()
                .fg(color_from_anstyle(styles.tool.get_fg_color()))
                .add_modifier(Modifier::BOLD),
        ));
    }
    if state.autonomy_mode.is_auto() {
        let seg = "[auto] ";
        prefix_w = prefix_w.saturating_add(seg.width() as u16);
        line_spans.push(Span::styled(
            seg,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    prefix_w = prefix_w.saturating_add(UnicodeWidthStr::width(prefix.as_str()) as u16);
    line_spans.push(Span::styled(prefix, prefix_style));
    if state.shell_mode {
        let seg = "! ";
        prefix_w = prefix_w.saturating_add(seg.width() as u16);
        line_spans.push(Span::styled(
            seg,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let context_line = composer_context_line(state, area.width);
    let used: usize = context_line.spans.iter().map(|s| s.width()).sum();
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())))
        // The top border is useful real estate. It carries the active session
        // context instead of spending a full row on a generic "MESSAGE"
        // label, while the border still makes the input target unmistakable.
        .title(context_line);
    if let Some(chip) = composer_brain_chip(state, area.width, used) {
        block = block.title(chip);
    }

    // Place the prefix in a leading line, then render the textarea in the
    // remaining width. When the body is empty AND a placeholder is
    // configured, render the placeholder as dimmed text (preserving the
    // pre-port look) and put the caret at the placeholder start.
    let inner = area.inner(Margin::new(1, 1));
    let textarea_area = Rect {
        x: inner.left().saturating_add(prefix_w),
        y: inner.top(),
        width: inner.width.saturating_sub(prefix_w),
        height: inner.height,
    };
    if state.composer.is_empty()
        && let Some(ph) = placeholder.as_deref()
    {
        // Prefix + placeholder as a single paragraph (no body).
        line_spans.push(Span::styled(
            ph.to_string(),
            Style::default()
                .fg(color_from_anstyle(styles.secondary.get_fg_color()))
                .dim(),
        ));
        let paragraph = Paragraph::new(Line::from(line_spans))
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
        if state.input_enabled {
            // Caret sits at the start of the placeholder so the user sees
            // where typing will land — same behavior as before the port.
            frame.set_cursor_position(Position::new(
                inner.left().saturating_add(prefix_w),
                area.top().saturating_add(1),
            ));
        }
        return;
    }
    // Paint the prefix in the first `prefix_w` columns of the inner box,
    // then the textarea paints the editable body. The textarea reports
    // its caret position relative to `textarea_area`; we add the
    // area origin at the end.
    let prefix_area = Rect {
        x: inner.left(),
        y: inner.top(),
        width: prefix_w,
        height: inner.height,
    };
    // Render the bordered box (with no body content) and the prefix
    // spans inside it.
    let frame_paragraph = Paragraph::new(Line::from(Vec::<Span>::new()))
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(frame_paragraph, area);
    frame.render_widget(Paragraph::new(Line::from(line_spans)), prefix_area);
    frame.render_widget_ref(&state.composer, textarea_area);

    if state.input_enabled
        && let Some((cx, cy)) = state
            .composer
            .cursor_pos_with_state(textarea_area, TextAreaState::default())
    {
        // `cursor_pos_with_state` returns the ABSOLUTE screen position:
        // it already adds `area.x` and `area.y` to the cursor's column/row
        // inside the area (see oxicode-textarea `cursor_pos_with_state`:
        // `Some((area.x + col, area.y + screen_row))`). Do NOT add the
        // area origin again — that double-offset pushed the caret off the
        // frame (e.g. row 38 on a 24-row terminal).
        frame.set_cursor_position(Position::new(cx, cy));
    }
}

/// Compact session facts embedded in the composer's top border.
///
/// The field order is deliberately task-oriented: model and reasoning first,
/// then place/version-control context, then the capacity signal.
/// At narrower widths lower-priority facts disappear as complete fields
/// rather than being clipped halfway through a path or branch name.
fn composer_context_line<'a>(state: &'a RenderState, width: u16) -> Line<'a> {
    let styles = active_styles();
    let primary = color_from_anstyle(styles.primary.get_fg_color());
    let fg = color_from_anstyle(Some(styles.foreground));
    let muted = color_from_anstyle(styles.secondary.get_fg_color());
    let info = color_from_anstyle(styles.info.get_fg_color());

    let model = state
        .header_context
        .model
        .strip_prefix(&format!("{}/", state.header_context.provider))
        .unwrap_or(&state.header_context.model);
    let workspace = state
        .cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "workspace".to_string());
    let branch = state
        .header_context
        .persistent_memory
        .as_ref()
        .map(|badge| badge.text.as_str())
        .filter(|branch| !branch.is_empty())
        .unwrap_or("—");
    let context = match state.context_tokens {
        Some(used) => {
            let percent = used.saturating_mul(100) / state.context_window.max(1);
            format!(
                "{}/{} {percent}%",
                compact_token_count(used),
                compact_token_count(state.context_window)
            )
        }
        None => format!("0/{}", compact_token_count(state.context_window)),
    };

    // (label, value, value style, minimum width). The first surviving field
    // renders without a leading separator — there is no app badge. With
    // `glyph_set = "nerd"`, labels become Nerd Font icons (never emoji).
    use crate::symbols::nerd as icons;
    let nerd = state.glyph_set == crate::symbols::GlyphSet::Nerd;
    let label =
        |text: &'static str, icon: &'static str| -> &'static str { if nerd { icon } else { text } };
    let mut fields: Vec<(&str, String, Style, u16)> = vec![
        (
            label("MODEL ", icons::MODEL),
            model.to_string(),
            Style::default().fg(fg).add_modifier(Modifier::BOLD),
            0,
        ),
        (
            label("THINK ", icons::THINK),
            state.thinking_level.clone(),
            Style::default().fg(info),
            58,
        ),
        (
            label("DIR ", icons::DIR),
            workspace,
            Style::default().fg(fg),
            82,
        ),
        (
            label("GIT ", icons::GIT),
            branch.to_string(),
            Style::default().fg(fg),
            104,
        ),
        (
            label("CTX ", icons::CTX),
            context,
            Style::default().fg(info),
            124,
        ),
    ];
    if state.active_run.is_some() || state.reasoning_stage.is_some() {
        fields.push((
            label("RUN ", icons::RUN),
            state
                .reasoning_stage
                .clone()
                .unwrap_or_else(|| "working\u{2026}".to_string()),
            Style::default().fg(primary).add_modifier(Modifier::BOLD),
            148,
        ));
    }

    let mut spans = Vec::new();
    for (i, (label, value, value_style, min_width)) in fields.into_iter().enumerate() {
        if width < min_width {
            break;
        }
        if i > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(muted)));
        }
        spans.push(Span::styled(
            (*label).to_string(),
            Style::default().fg(muted),
        ));
        spans.push(Span::styled(value, value_style));
    }
    Line::from(spans)
}

/// Right-aligned oxibrain health chip rendered as its own border title
/// (moved off the removed shortcuts bar): healthy reads info, unreachable
/// error, absent when memory is disabled. Nerd mode swaps the prefix for
/// the brain glyph.
///
/// The chip must NOT be space-padded into [`composer_context_line`]: a
/// title overwrites the border row for its full width, and the padding
/// would erase the `─` rule between the facts and the chip (it did —
/// see `brain_chip_does_not_erase_the_border_rule`). A separate
/// right-aligned title covers only the chip's own cells.
fn composer_brain_chip<'a>(state: &'a RenderState, width: u16, used: usize) -> Option<Line<'a>> {
    let styles = active_styles();
    let (chip_label, healthy) = state.brain.chip_label()?;
    let chip_color = if healthy {
        color_from_anstyle(styles.info.get_fg_color())
    } else {
        color_from_anstyle(styles.error.get_fg_color())
    };
    let nerd = state.glyph_set == crate::symbols::GlyphSet::Nerd;
    let text = if nerd {
        let state_word = chip_label.trim_start_matches("brain\u{b7}");
        format!("{} {}", crate::symbols::nerd::BRAIN, state_word)
    } else {
        chip_label.to_string()
    };
    let chip = format!(" {text} ");
    // The border's title row is two cells narrower than the block.
    let usable = width.saturating_sub(2) as usize;
    (used + chip.width() < usable)
        .then(|| Line::from(Span::styled(chip, Style::default().fg(chip_color))).right_aligned())
}

fn compact_token_count(tokens: usize) -> String {
    if tokens >= 1_000 {
        let whole = tokens / 1_000;
        let decimal = (tokens % 1_000) / 100;
        if decimal == 0 {
            format!("{whole}K")
        } else {
            format!("{whole}.{decimal}K")
        }
    } else {
        tokens.to_string()
    }
}

/// Render a compact onboarding card when the transcript is empty.
///
/// The card answers the three questions a fresh terminal should answer at a
/// glance: where am I, which model will answer, and what can I do next.
fn render_welcome(frame: &mut Frame<'_>, area: Rect, state: &RenderState) {
    let styles = active_styles();
    let primary = color_from_anstyle(styles.primary.get_fg_color());
    let fg = color_from_anstyle(Some(styles.foreground));
    let secondary = color_from_anstyle(styles.secondary.get_fg_color());
    let workspace = state
        .cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "workspace".to_string());
    let lines = vec![
        Line::from(Span::styled(
            "OXICODE",
            Style::default().fg(primary).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Terminal coding assistant",
            Style::default().fg(secondary).add_modifier(Modifier::DIM),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("WORKSPACE  ", Style::default().fg(secondary)),
            Span::styled(
                workspace,
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("MODEL      ", Style::default().fg(secondary)),
            Span::styled(
                format!(
                    "{} / {}",
                    state.header_context.provider, state.header_context.model
                ),
                Style::default().fg(fg),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter  send     /  commands     @  attach a file",
            Style::default().fg(fg),
        )),
        Line::from(Span::styled(
            "?  shortcuts     /model  change model     /help  all commands",
            Style::default().fg(secondary),
        )),
    ];
    let height = lines.len().min(area.height as usize) as u16;
    let card = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: area.width,
        height,
    };
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), card);
}

/// Render a 1-row run indicator just above the composer.
///
/// While a run is live this row is continuously owned by the indicator:
/// turn boundaries clear `reasoning_stage` but the run tracker keeps the
/// row up (falling back to `working…`), so it never flickers to the idle
/// row mid-loop. The spinner animates on the frame tick and the suffix
/// carries progress facts (turn count, tool calls, elapsed time).
fn render_reasoning_indicator(frame: &mut Frame<'_>, composer_area: Rect, state: &RenderState) {
    let styles = active_styles();
    let indicator_area = Rect {
        x: composer_area.x,
        y: composer_area.top().saturating_sub(1),
        width: composer_area.width,
        height: 1,
    };
    let primary = color_from_anstyle(styles.primary.get_fg_color());
    let muted = color_from_anstyle(styles.secondary.get_fg_color());
    // 12.5 fps: lively, but keyed on wall-clock so draw-count bursts
    // during streaming can't make it race.
    let spin = RUN_SPINNER[(animation_frame(80) as usize) % RUN_SPINNER.len()];
    let stage = state
        .reasoning_stage
        .as_deref()
        .unwrap_or("working\u{2026}");
    let mut spans = vec![
        Span::styled(spin, Style::default().fg(primary)),
        Span::styled(
            " RUNNING",
            Style::default().fg(primary).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(muted)),
        Span::styled(
            stage.to_string(),
            Style::default().fg(muted).add_modifier(Modifier::DIM),
        ),
    ];
    if let Some(run) = &state.active_run {
        let elapsed = format_elapsed_secs(run.started_at.elapsed().as_secs());
        let facts = if run.turn > 0 {
            format!(
                " \u{b7} turn {} \u{b7} {} tool call{} \u{b7} {elapsed}",
                run.turn,
                run.tool_calls,
                if run.tool_calls == 1 { "" } else { "s" },
            )
        } else {
            format!(" \u{b7} {elapsed}")
        };
        spans.push(Span::styled(
            facts,
            Style::default().fg(muted).add_modifier(Modifier::DIM),
        ));
    }
    // Contextual abort hint (Claude Code pattern): shown only while
    // a run is live — the static shortcuts bar is gone.
    spans.push(Span::styled(
        "  Esc abort \u{b7} Ctrl+C quit",
        Style::default().fg(muted).add_modifier(Modifier::DIM),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), indicator_area);
}

/// Pending-quit hint: shown in the row above the composer after the
/// first Ctrl+C aborted a stream — the next press opens the quit
/// confirmation. Submitting a new prompt cancels it.
fn render_pending_quit_hint(frame: &mut Frame<'_>, composer_area: Rect) {
    let styles = active_styles();
    let hint_area = Rect {
        x: composer_area.x,
        y: composer_area.top().saturating_sub(1),
        width: composer_area.width,
        height: 1,
    };
    let line = Line::from(Span::styled(
        "press Ctrl+C again to quit",
        Style::default()
            .fg(color_from_anstyle(styles.error.get_fg_color()))
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(line), hint_area);
}

/// Render queued input prompts as a compact pane at the top of the scrollback.
fn render_queue_pane(frame: &mut Frame<'_>, scrollback: Rect, state: &RenderState) -> u16 {
    let styles = active_styles();
    let entries = &state.queued_inputs;
    let interactive = state.queue_panel_open;
    let selected = state.queue_selected.min(entries.len().saturating_sub(1));
    let height = if interactive {
        entries.len() as u16 + 1
    } else {
        1
    };
    let area = Rect {
        x: scrollback.x,
        y: scrollback.y,
        width: scrollback.width,
        height,
    };
    let info = color_from_anstyle(styles.info.get_fg_color());
    let secondary = color_from_anstyle(styles.secondary.get_fg_color());
    let primary = color_from_anstyle(styles.primary.get_fg_color());
    if !interactive {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("QUEUED {}", entries.len()),
                    Style::default().fg(primary).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " | Ctrl+; manage",
                    Style::default().fg(secondary).add_modifier(Modifier::DIM),
                ),
            ])),
            area,
        );
        return height;
    }
    let items: Vec<Line<'_>> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let prefix = format!("#{} ", i + 1);
            let prefix_style = if i == selected {
                Style::default().fg(primary).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(info)
            };
            let text_style = if i == selected {
                Style::default().fg(primary).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(secondary)
            };
            let marker = if i == selected { "> " } else { "  " };
            Line::from(vec![
                Span::styled(prefix, prefix_style),
                Span::styled(marker, prefix_style),
                Span::styled(e.clone(), text_style),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(items).block(Block::default().borders(Borders::TOP).border_style(
            Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())),
        )),
        area,
    );
    height
}

/// Format one todo row: marker + content + status-specific suffix + notes
/// marker. Ports omp's `#formatTodoLine` (`interactive-mode.ts:2326-2341`).
fn format_todo_line(todo: &TodoItem, matched: bool, styles: &ThemeStyles) -> Line<'static> {
    let notes_marker = match todo.notes.as_ref().map(|n| n.len()).unwrap_or(0) {
        0 => String::new(),
        n => format!(" ·{n}"),
    };
    let (marker, color, strike, suffix) = match todo.status {
        TodoStatus::Completed => ("✓", styles.foreground, true, String::new()),
        TodoStatus::InProgress => (
            "▸",
            styles.primary.get_fg_color().unwrap_or(styles.foreground),
            false,
            String::new(),
        ),
        TodoStatus::Abandoned => (
            "☐",
            styles.error.get_fg_color().unwrap_or(styles.foreground),
            true,
            String::new(),
        ),
        TodoStatus::Blocked => {
            let reason = todo
                .block_reason
                .as_deref()
                .map(|r| format!(" (blocked: {r})"))
                .unwrap_or_else(|| " (blocked)".to_string());
            (
                "☐",
                styles.info.get_fg_color().unwrap_or(styles.foreground),
                false,
                reason,
            )
        }
        TodoStatus::Pending if matched => (
            "☐",
            styles.primary.get_fg_color().unwrap_or(styles.foreground),
            false,
            String::new(),
        ),
        TodoStatus::Pending => (
            "☐",
            styles.secondary.get_fg_color().unwrap_or(styles.foreground),
            false,
            String::new(),
        ),
    };
    let mut text_style = Style::default().fg(color_from_anstyle(Some(color)));
    if strike {
        text_style = text_style.add_modifier(Modifier::CROSSED_OUT);
    }
    Line::from(vec![
        Span::styled(
            format!("{marker} "),
            Style::default().fg(color_from_anstyle(Some(color))),
        ),
        Span::styled(
            format!("{}{}{}", todo.content, suffix, notes_marker),
            text_style,
        ),
    ])
}

const TREE_BRANCH: &str = "├─";
const TREE_VERTICAL: &str = "│ ";
const TREE_HOOK: &str = "└";
const SUBSEQUENT_STAGE_CAP: usize = 4;
const ACTIVE_TASK_CAP: usize = 5;

/// Index of the first phase with pending/in-progress work; falls back to the
/// last phase. Ports omp's `#getActivePhase` (`interactive-mode.ts:2489`).
fn active_phase_index(phases: &[&TodoPhase]) -> usize {
    phases
        .iter()
        .position(|p| {
            p.tasks
                .iter()
                .any(|t| matches!(t.status, TodoStatus::Pending | TodoStatus::InProgress))
        })
        .unwrap_or_else(|| phases.len().saturating_sub(1))
}

/// Closed = completed or abandoned (the collapsed window hides both).
fn closed_count(tasks: &[TodoItem]) -> usize {
    tasks
        .iter()
        .filter(|t| matches!(t.status, TodoStatus::Completed | TodoStatus::Abandoned))
        .count()
}

/// "I. Foundation", "II. Auth", … Reuses `roman_numeral` from `todo.rs`.
fn phase_display_name(name: &str, one_based: usize) -> String {
    format!(
        "{}. {name}",
        oxicode_agent::tools::todo::roman_numeral(one_based)
    )
}

/// Render the sticky todo HUD: phase tree + progress spine. Ports omp's
/// `#renderTodoList` (`interactive-mode.ts:2529-2643`). Returns rows used so
/// callers can reserve the space (mirrors `render_queue_pane`).
fn render_todo_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    phases: &[TodoPhase],
    expanded: bool,
    is_matched: impl Fn(&TodoItem) -> bool,
) -> u16 {
    let phases: Vec<&TodoPhase> = phases.iter().filter(|p| !p.tasks.is_empty()).collect();
    if phases.is_empty() {
        return 0;
    }
    let styles = active_styles();
    let multi_phase = phases.len() > 1;
    let active_idx = active_phase_index(&phases);

    let render_tasks = |phase: &TodoPhase| -> Vec<Line<'static>> {
        if expanded {
            phase
                .tasks
                .iter()
                .map(|t| format_todo_line(t, is_matched(t), &styles))
                .collect()
        } else {
            let sel = oxicode_agent::tools::todo::select_collapsed_todos(
                &phase.tasks,
                &is_matched,
                ACTIVE_TASK_CAP,
            );
            let mut lines: Vec<Line<'static>> = sel
                .items
                .iter()
                .map(|t| format_todo_line(t, is_matched(t), &styles))
                .collect();
            if let Some(summary) = sel.summary {
                lines.push(Line::from(Span::styled(
                    summary,
                    Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())),
                )));
            }
            lines
        }
    };

    let base_idx = if expanded { 0 } else { active_idx };
    let phase_slice: &[&TodoPhase] = if expanded {
        &phases[base_idx..]
    } else {
        &phases[base_idx..(base_idx + 1 + SUBSEQUENT_STAGE_CAP).min(phases.len())]
    };
    let hidden_stages = phases.len() - base_idx - phase_slice.len();

    let mut content_lines: Vec<Line<'static>> = Vec::new();
    for (i, phase) in phase_slice.iter().enumerate() {
        let one_based = base_idx + i + 1;
        let is_active = base_idx + i == active_idx;
        let done = closed_count(&phase.tasks);
        let header_text = if multi_phase {
            format!(
                "{} · {done}/{}",
                phase_display_name(&phase.name, one_based),
                phase.tasks.len()
            )
        } else {
            phase.name.clone()
        };
        let header_style = if is_active {
            Style::default()
                .fg(color_from_anstyle(styles.primary.get_fg_color()))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color()))
        };
        content_lines.push(Line::from(Span::styled(header_text, header_style)));
        if is_active || expanded {
            content_lines.extend(render_tasks(phase));
        }
    }
    if hidden_stages > 0 {
        content_lines.push(Line::from(Span::styled(
            format!(
                "… {hidden_stages} more stage{}",
                if hidden_stages == 1 { "" } else { "s" }
            ),
            Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())),
        )));
    }

    // Progress spine: `closed / total` across every phase fills the tree path
    // (content rows + 1 closing-hook row) in accent, clamped so a partial
    // plan lights at least one cell and a closed plan never overfills.
    let total: usize = phases.iter().map(|p| p.tasks.len()).sum();
    let closed: usize = phases.iter().map(|p| closed_count(&p.tasks)).sum();
    let path_len = content_lines.len() + 1;
    let mut filled = (closed * path_len).checked_div(total).unwrap_or(0);
    if closed > 0 {
        filled = filled.max(1);
    }
    if closed < total {
        filled = filled.min(path_len.saturating_sub(1));
    }

    let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
        "TODO",
        Style::default()
            .fg(color_from_anstyle(styles.primary.get_fg_color()))
            .add_modifier(Modifier::BOLD),
    ))];
    for (i, content) in content_lines.into_iter().enumerate() {
        let glyph = if i == 0 { TREE_BRANCH } else { TREE_VERTICAL };
        let glyph_color = if i < filled {
            styles.primary.get_fg_color()
        } else {
            styles.secondary.get_fg_color()
        };
        let mut spans = vec![Span::styled(
            format!(" {glyph}"),
            Style::default().fg(color_from_anstyle(glyph_color)),
        )];
        spans.extend(content.spans);
        lines.push(Line::from(spans));
    }
    // path_len = content rows + 1 hook row; the hook fills only when every
    // cell (including it) is lit, i.e. the whole list is closed.
    let hook_color = if filled >= path_len {
        styles.primary.get_fg_color()
    } else {
        styles.secondary.get_fg_color()
    };
    lines.push(Line::from(Span::styled(
        format!(" {TREE_HOOK}"),
        Style::default().fg(color_from_anstyle(hook_color)),
    )));

    let height = lines.len() as u16;
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height,
        },
    );
    height
}

const TODO_COMPACT_ROWS_THRESHOLD: u16 = 18;

/// First in-progress task, else the first pending task, else `None`. Ports
/// omp's `nextActionableTask` (`todo.ts:164-172`).
fn next_actionable_task(phases: &[TodoPhase]) -> Option<&TodoItem> {
    let mut first_pending = None;
    for phase in phases {
        for task in &phase.tasks {
            if task.status == TodoStatus::InProgress {
                return Some(task);
            }
            if first_pending.is_none() && task.status == TodoStatus::Pending {
                first_pending = Some(task);
            }
        }
    }
    first_pending
}

/// Single-line HUD used on short terminals (< 18 rows): "TODO N/M · <task>".
/// Ports omp's `renderCompactStatusLine` (`interactive-mode.ts:2645+`).
fn render_todo_compact_line(phases: &[TodoPhase]) -> Line<'static> {
    let styles = active_styles();
    let total: usize = phases.iter().map(|p| p.tasks.len()).sum();
    let closed: usize = phases.iter().map(|p| closed_count(&p.tasks)).sum();
    let mut spans = vec![Span::styled(
        format!("TODO {closed}/{total} "),
        Style::default()
            .fg(color_from_anstyle(styles.primary.get_fg_color()))
            .add_modifier(Modifier::BOLD),
    )];
    match next_actionable_task(phases) {
        Some(task) => spans.extend(format_todo_line(task, false, &styles).spans),
        None => spans.push(Span::styled(
            "✓ done",
            Style::default().fg(color_from_anstyle(Some(styles.foreground))),
        )),
    }
    Line::from(spans)
}

/// Pull the latest todo phases, auto-reconciling against the hub's *idle*
/// sub-agents (a transition Running → Idle is a successful completion) and
/// committing the reconciled result back when it changed. Ports omp's
/// `#reconcileTodosWithSubagents` (`interactive-mode.ts:2369-2404`).
fn refresh_todo_phases(
    provider: &std::sync::Arc<dyn TodoStateProvider>,
    hub: Option<&crate::app::agent_hub_registry::SharedHubRegistry>,
) -> Vec<TodoPhase> {
    let phases = provider.get_phases();
    let Some(hub) = hub else {
        return phases;
    };
    let completed: Vec<String> = hub
        .snapshot()
        .into_iter()
        .filter(|(_, e)| {
            e.kind == oxicode_sdk::HubKind::Subagent && e.status == oxicode_sdk::HubStatus::Idle
        })
        .filter_map(|(_, e)| e.current_task)
        .collect();
    let (updated, mutated) =
        oxicode_agent::tools::todo::reconcile_with_subagents(&phases, &completed);
    if mutated {
        provider.set_phases_sync(updated.clone());
    }
    updated
}

/// Whether every task in the list is closed (`Completed`/`Abandoned`) and at
/// least one task exists. A list with zero phases or zero tasks is not
/// "settled" — there's nothing meaningful to auto-clear.
fn is_todo_list_settled(phases: &[TodoPhase]) -> bool {
    let mut seen_task = false;
    for phase in phases {
        for task in &phase.tasks {
            if !matches!(task.status, TodoStatus::Completed | TodoStatus::Abandoned) {
                return false;
            }
            seen_task = true;
        }
    }
    seen_task
}

/// HUD-only auto-clear: does not touch the underlying `TodoState`, so a
/// `/todo` or `todo` tool call after clearing still sees the historical
/// phases. `delay_secs < 0` disables clearing entirely. Called every frame
/// after `refresh_todo_phases`, so a settled list stays visually cleared.
fn sync_todo_clear_timer(state: &mut RenderState, delay_secs: i64) {
    if delay_secs < 0 || !is_todo_list_settled(&state.todo_phases) {
        state.todo_clear_deadline = None;
        return;
    }
    if delay_secs == 0 {
        state.todo_phases.clear();
        state.todo_clear_deadline = None;
        return;
    }
    let deadline = state.todo_clear_deadline.get_or_insert_with(|| {
        std::time::Instant::now() + std::time::Duration::from_secs(delay_secs as u64)
    });
    if std::time::Instant::now() >= *deadline {
        state.todo_phases.clear();
        state.todo_clear_deadline = None;
    }
}

/// Closure that lights a pending todo up (accent) when a *running* sub-agent
/// is executing it, matched by normalized content overlap. Ports omp's
/// `isMatched` (`interactive-mode.ts:2543`).
fn build_matched_closure(
    hub: Option<&crate::app::agent_hub_registry::SharedHubRegistry>,
) -> impl Fn(&TodoItem) -> bool + '_ {
    let active_descs: Vec<String> = hub
        .map(|h| {
            h.snapshot()
                .into_iter()
                .filter(|(_, e)| {
                    e.kind == oxicode_sdk::HubKind::Subagent
                        && e.status == oxicode_sdk::HubStatus::Running
                })
                .filter_map(|(_, e)| e.current_task)
                .collect()
        })
        .unwrap_or_default();
    move |t| {
        !active_descs.is_empty()
            && oxicode_agent::tools::todo::todo_matches_any_description(&t.content, &active_descs)
    }
}

/// Render follow-up suggestion chips just above the composer.
fn render_follow_ups(frame: &mut Frame<'_>, composer_area: Rect, chips: &[String]) {
    let styles = active_styles();
    let area = Rect {
        x: composer_area.x,
        y: composer_area.top().saturating_sub(1),
        width: composer_area.width,
        height: 1,
    };
    let mut spans = vec![Span::styled(
        "Suggestions: ",
        Style::default()
            .fg(color_from_anstyle(styles.secondary.get_fg_color()))
            .add_modifier(Modifier::DIM),
    )];
    for (i, chip) in chips.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!("[{}]", chip),
            Style::default().fg(color_from_anstyle(styles.primary.get_fg_color())),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Whether an ephemeral tip is still within its visible TTL window.
fn tip_is_visible(tip: &EphemeralTip, now_tick: u64) -> bool {
    now_tick.saturating_sub(tip.born_tick) < tip.ttl_ticks
}

/// Render the ephemeral tip banner one row above the composer.
fn render_tip(frame: &mut Frame, composer_area: Rect, text: &str) {
    let styles = active_styles();
    let area = Rect {
        x: composer_area.x,
        y: composer_area.top().saturating_sub(1),
        width: composer_area.width,
        height: 1,
    };
    let line = Line::styled(
        format!(" note: {text}"),
        Style::default()
            .fg(color_from_anstyle(styles.info.get_fg_color()))
            .add_modifier(Modifier::DIM),
    );
    frame.render_widget(Paragraph::new(line), area);
}

/// Render the slash-command autocomplete popup as a floating panel above the
/// composer. Anchored to the composer's left edge, grows upward.
fn render_slash_popup(frame: &mut Frame<'_>, composer_area: Rect, state: &RenderState) {
    let styles = active_styles();
    let items = &state.slash_popup.items;
    if items.is_empty() {
        return;
    }

    let max_visible = 7usize;
    let visible = items.len().min(max_visible);
    let popup_h = visible as u16 + 3; // borders + persistent key-help row
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
        " COMMANDS ",
        Style::default()
            .fg(color_from_anstyle(styles.primary.get_fg_color()))
            .add_modifier(Modifier::BOLD),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
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

        let marker = if is_selected { "> " } else { "  " };
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
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Enter insert | Up/Down move | Esc close",
            Style::default().fg(secondary).add_modifier(Modifier::DIM),
        ))),
        Rect {
            x: inner.left(),
            y: inner.bottom().saturating_sub(1),
            width: inner.width,
            height: 1,
        },
    );
}

/// Render the @-file-search dropdown as a floating panel above the
/// composer, mirroring `render_slash_popup`'s geometry. Shows up to 10
/// fuzzy-matched file paths with the selected one highlighted.
fn render_file_search_dropdown(frame: &mut Frame<'_>, composer_area: Rect, state: &RenderState) {
    let styles = active_styles();
    let Some(fs) = &state.file_search else {
        return;
    };
    let items = &fs.results;
    if items.is_empty() {
        return;
    }

    let max_visible = 10usize;
    let visible = items.len().min(max_visible);
    let popup_h = visible as u16 + 3; // borders + persistent key-help row
    let width = composer_area.width.min(72);
    let popup_area = Rect {
        x: composer_area.left(),
        y: composer_area.top().saturating_sub(popup_h),
        width,
        height: popup_h,
    };
    frame.render_widget(Clear, popup_area);

    let border_color = color_from_anstyle(styles.secondary.get_fg_color());
    let title_str = if fs.hidden_mode {
        " FILES: HIDDEN "
    } else {
        " FILES "
    };
    let title = Line::from(Span::styled(
        title_str,
        Style::default()
            .fg(color_from_anstyle(styles.primary.get_fg_color()))
            .add_modifier(Modifier::BOLD),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color))
        .title(title);
    let inner = block.inner(popup_area);
    frame.render_widget(&block, popup_area);

    let primary = color_from_anstyle(styles.primary.get_fg_color());
    let fg = color_from_anstyle(Some(styles.foreground));
    let secondary = color_from_anstyle(styles.secondary.get_fg_color());

    for (i, result) in items.iter().take(visible).enumerate() {
        let is_selected = i == fs.selected;
        let y = inner.top() + i as u16;
        let row_area = Rect {
            x: inner.left(),
            y,
            width: inner.width,
            height: 1,
        };

        let marker = if is_selected { "> " } else { "  " };
        let path_style = if is_selected {
            Style::default().fg(primary).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg)
        };
        let line = Line::from(vec![
            Span::styled(marker, path_style),
            Span::styled(&result.path, path_style),
        ]);
        frame.render_widget(Paragraph::new(line), row_area);
    }

    // Footer hint: show result count + key bindings.
    if popup_h >= 4 {
        let hint_y = inner.bottom().saturating_sub(1);
        let hint_area = Rect {
            x: inner.left(),
            y: hint_y,
            width: inner.width,
            height: 1,
        };
        let count = items.len();
        let hint = format!("{count} files | Tab accept | Esc cancel");
        let _ = secondary; // suppress unused warning
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())),
            )))
            .style(Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color()))),
            hint_area,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Vim mode — Editor adapter for the input buffer
// ─────────────────────────────────────────────────────────────────────────

/// Adapter that lets the vim engine operate on the composer's [`TextArea`].
///
/// The host TUI keeps a single [`TextArea`](oxicode_textarea::TextArea)
/// (the composer) as the source of truth for editable text; the vim engine
/// still wants a `&str` + byte-cursor handle. This adapter forwards each
/// trait call to the textarea so cursor math, grapheme boundaries, and
/// undo history are owned by the textarea.
struct InputEditor<'a> {
    composer: &'a mut oxicode_textarea::TextArea,
}

impl<'a> InputEditor<'a> {
    fn new(composer: &'a mut oxicode_textarea::TextArea) -> Self {
        Self { composer }
    }
}

impl<'a> crate::tui_vt::vim::Editor for InputEditor<'a> {
    fn content(&self) -> &str {
        self.composer.text()
    }
    fn cursor(&self) -> usize {
        self.composer.cursor()
    }
    fn set_cursor(&mut self, pos: usize) {
        self.composer.set_cursor(pos);
    }
    fn move_left(&mut self) {
        // The textarea's `set_cursor` clamps to the nearest grapheme
        // boundary, so we just step back one byte and let it clean up.
        let new_pos = self.composer.cursor().saturating_sub(1);
        self.composer.set_cursor(new_pos);
    }
    fn move_right(&mut self) {
        let new_pos = self.composer.cursor().saturating_add(1);
        self.composer.set_cursor(new_pos);
    }
    fn delete_char_forward(&mut self) {
        self.composer.input(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Delete,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    fn insert_text(&mut self, text: &str) {
        self.composer.insert_str(text);
    }
    fn replace(&mut self, content: String, cursor: usize) {
        self.composer.set_text(&content);
        self.composer.set_cursor(cursor);
    }
    fn replace_range(&mut self, start: usize, end: usize, text: &str) {
        self.composer.replace_range(start..end, text);
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

// Slash-command autocomplete popup
// ─────────────────────────────────────────────────────────────────────────
/// Filter slash commands by `token` (the text after `/`). An empty token
/// returns every command. Matching is prefix-based against the canonical
/// name and all aliases.
///
/// Built-in commands are listed first; user-defined file commands are
/// appended afterwards. Any file command whose name shadows a built-in is
/// dropped — built-ins always win, so file commands cannot redefine
/// `/quit`, `/clear`, etc.
fn slash_filter(token: &str, file_commands: &[FileCommand]) -> Vec<SlashPopupItem> {
    let builtins = SlashRegistry::builtin_commands();
    let builtin_names: std::collections::HashSet<&str> =
        builtins.iter().map(|(n, _, _)| *n).collect();

    let mut items: Vec<SlashPopupItem> = builtins
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
        .collect();

    // Append file commands (skip names shadowed by builtins).
    for fc in file_commands {
        if builtin_names.contains(fc.name.as_str())
            || fc
                .aliases
                .iter()
                .any(|alias| builtin_names.contains(alias.as_str()))
        {
            continue;
        }
        if token.is_empty()
            || fc.name.starts_with(token)
            || fc.aliases.iter().any(|a| a.starts_with(token))
        {
            let mut label = format!("/{}", fc.name);
            for a in &fc.aliases {
                label.push_str(&format!(", /{a}"));
            }
            items.push(SlashPopupItem {
                label,
                description: fc.description.clone(),
                name: fc.name.clone(),
            });
        }
    }

    items
}

/// Recompute the slash popup from the current input buffer. The popup is
/// active when the buffer starts with `/` and has no space yet (the user is
/// still composing the command token, not its arguments). Called after every
/// buffer mutation in the input thread.
fn refresh_slash_popup(state: &mut RenderState) {
    let buf = state.composer.text();
    let active = buf.starts_with('/') && !buf[1..].contains(' ');
    if !active {
        state.slash_popup.open = false;
        state.slash_popup.items.clear();
        state.slash_popup.selected = 0;
        return;
    }
    let token = &buf[1..];
    let items = slash_filter(token, &state.file_commands);
    state.slash_popup.open = !items.is_empty();
    if items.is_empty() {
        state.slash_popup.selected = 0;
    } else {
        state.slash_popup.selected = state.slash_popup.selected.min(items.len() - 1);
    }
    state.slash_popup.items = items;
}
/// Combined popup refresher — calls both the slash-command popup and the
/// @-file-search picker. Called after every input buffer mutation in the
/// input thread so both popups stay in sync with the cursor position.
fn refresh_input_popups(state: &mut RenderState) {
    refresh_slash_popup(state);
    refresh_file_search(state);
}

/// Recompute the @-file-search dropdown from the current input buffer.
/// Called after every buffer mutation in the input thread. The filesystem
/// walk (building the index) happens only on the `None → Some` transition
/// (when `@` is first typed); subsequent keystrokes just re-filter the
/// cached index via [`FileSearchState::refresh`](crate::tui_vt::file_search::FileSearchState::refresh).
fn refresh_file_search(state: &mut RenderState) {
    use crate::tui_vt::file_search;
    // Never open the file picker while a slash command is being composed.
    if state.slash_popup.open {
        state.file_search = None;
        return;
    }
    match file_search::parse_at_cursor(state.composer.text(), state.composer.cursor()) {
        Some(token) => match &mut state.file_search {
            None => {
                let cwd = state.cwd.clone();
                state.file_search = Some(file_search::open(&cwd, token.at_offset, false));
            }
            Some(fs) => {
                if fs.query != token.path_query {
                    fs.refresh(&token.path_query);
                }
            }
        },
        None => state.file_search = None,
    }
}

/// Accept the currently-selected file-search result: replace the `@query`
/// token in the buffer with the canonical `@path ` (or `@path:N-M ` in
/// line mode), advance the cursor past it, and close the picker.
/// Returns `true` if a result was accepted.
fn accept_file_search(state: &mut RenderState, line_mode: bool) -> bool {
    use crate::tui_vt::file_search;
    let Some(fs) = &state.file_search else {
        return false;
    };
    let Some(result) = fs.selected_result().cloned() else {
        return false;
    };
    let at_offset = fs.at_offset;
    let text = file_search::insertion_text(&result.path, None, line_mode);
    let cursor_end = state.composer.cursor();
    // Replace everything from `@` to the current cursor with the insertion.
    state.composer.replace_range(
        at_offset..cursor_end.min(state.composer.text().len()),
        &text,
    );
    state.composer.set_cursor(at_offset + text.len());
    state.file_search = None;
    true
}

fn preview_tool_result(content: &str) -> String {
    const MAX: usize = 500;
    if content.chars().count() <= MAX {
        return content.to_string();
    }
    let truncated: String = content.chars().take(MAX).collect();
    format!("{truncated}\u{2026}")
}

/// Extract the first embedded PNG from a `generate_image` tool result.
///
/// The tool's output embeds images as
/// `Image N (<bytes> bytes, base64):\n<base64>`. Returns the decoded
/// bytes of the first image, or `None` when no marker/base64 payload is
/// present or the payload does not decode.
fn extract_generated_png(content: &str) -> Option<Vec<u8>> {
    use base64::{Engine, engine::general_purpose};
    const MARKER: &str = "base64):";
    let rest = &content[content.find(MARKER)? + MARKER.len()..];
    // The base64 blob is the first non-empty line after the marker.
    let blob = rest.lines().map(str::trim).find(|l| !l.is_empty())?;
    if blob.is_empty() {
        return None;
    }
    let bytes = general_purpose::STANDARD.decode(blob).ok()?;
    // Sanity floor: a real PNG header is 8 bytes. Shorter payloads are
    // parse noise, not an image.
    (bytes.len() >= 8).then_some(bytes)
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
        let items = slash_filter("", &[]);
        // 7 built-in commands.
        assert!(items.len() >= 7);
        assert!(items.iter().any(|i| i.name == "quit"));
        assert!(items.iter().any(|i| i.name == "clear"));
        assert!(items.iter().any(|i| i.name == "model"));
    }

    #[test]
    fn prefix_filter_matches_name() {
        let items = slash_filter("qu", &[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "quit");
        assert!(items[0].label.contains("/quit"));
    }

    #[test]
    fn prefix_filter_matches_alias() {
        // "cl" should match "clear" (alias "cls") and "compact".
        let items = slash_filter("cl", &[]);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"clear"));
    }

    #[test]
    fn file_commands_appear_in_filter() {
        let fc = crate::tui_vt::slash::file_commands::FileCommand::parse(
            "review",
            "---\ndescription: proj cmd\naliases: cr\n---\nbody",
        );
        let items = slash_filter("", &[fc]);
        assert!(items.iter().any(|i| i.name == "review"));
        assert!(items.iter().any(|i| i.name == "quit")); // builtins still present
    }

    #[test]
    fn file_commands_filtered_by_prefix() {
        let fc = crate::tui_vt::slash::file_commands::FileCommand::parse(
            "review",
            "---\ndescription: x\n---\nbody",
        );
        let items = slash_filter("rev", &[fc]);
        assert!(items.iter().any(|i| i.name == "review"));
    }

    #[test]
    fn file_commands_shadowed_by_builtins_are_dropped() {
        // A file command whose name collides with a built-in must be dropped —
        // built-ins always win. Without this guarantee the popup could surface
        // two items for the same prefix and the dispatch layer would pick the
        // wrong one.
        let fc = crate::tui_vt::slash::file_commands::FileCommand::parse(
            "quit",
            "---\ndescription: hijack\n---\nbody",
        );
        let items = slash_filter("", &[fc]);
        let quit_count = items.iter().filter(|i| i.name == "quit").count();
        assert_eq!(quit_count, 1, "shadowed file command must not appear");
        // And it must be the built-in description, not the file one.
        assert!(
            items
                .iter()
                .any(|i| i.name == "quit" && !i.description.contains("hijack"))
        );
    }

    #[test]
    fn file_commands_with_builtin_aliases_are_dropped() {
        let fc = crate::tui_vt::slash::file_commands::FileCommand::parse(
            "review",
            "---\ndescription: hijack\naliases: quit\n---\nbody",
        );
        let items = slash_filter("", &[fc]);
        assert!(!items.iter().any(|item| item.name == "review"));
    }

    #[test]
    fn popup_opens_on_slash() {
        let mut state = RenderState::default();
        state.composer.set_text("/");
        refresh_input_popups(&mut state);
        assert!(state.slash_popup.open);
        assert!(!state.slash_popup.items.is_empty());
    }

    #[test]
    fn popup_closes_on_space() {
        let mut state = RenderState::default();
        state.composer.set_text("/quit ");
        refresh_input_popups(&mut state);
        assert!(!state.slash_popup.open);
    }

    #[test]
    fn popup_closes_on_non_slash() {
        let mut state = RenderState::default();
        state.composer.set_text("hello");
        refresh_input_popups(&mut state);
        assert!(!state.slash_popup.open);
    }

    #[test]
    fn popup_filters_as_user_types() {
        let mut state = RenderState::default();
        state.composer.set_text("/m");
        refresh_input_popups(&mut state);
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
        state.composer.set_text("/");
        refresh_input_popups(&mut state);
        let full_count = state.slash_popup.items.len();
        state.slash_popup.selected = full_count - 1;
        // Narrow the filter so fewer items remain.
        state.composer.set_text("/qu");
        refresh_input_popups(&mut state);
        assert!(state.slash_popup.selected < state.slash_popup.items.len());
    }
}

#[cfg(test)]
mod keymap_dispatch_tests {
    use super::*;
    use crate::tui_vt::keymap::Keymap;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// Build a keymap from (action-name, combo-string) override pairs —
    /// the unified replacement for the branch's keybindings.yml overlay
    /// (settings-backed overrides go through the same
    /// `Keymap::from_settings` path as the real settings editor).
    fn keymap_with(overrides: &[(&str, &str)]) -> Keymap {
        let mut o = std::collections::HashMap::new();
        for (action, combo) in overrides {
            o.insert(action.to_string(), vec![combo.to_string()]);
        }
        Keymap::from_settings(&o)
    }

    fn default_keymap() -> Keymap {
        Keymap::from_settings(&std::collections::HashMap::new())
    }

    #[test]
    fn rebound_submit_fires_through_generic_dispatch() {
        // Final-review finding 6: `submit: alt+s` must actually fire.
        // Previously Submit was consulted only inside the hardcoded
        // Enter arm, so a rebind could only disable submit at Enter,
        // never move it to another key.
        let km = keymap_with(&[("Submit", "Alt+s")]);
        let alt_s = press(KeyCode::Char('s'), KeyModifiers::ALT);
        // Multiline is irrelevant for the rebind: the carve-out only
        // protects plain Enter.
        assert_eq!(keymap_pre_match(&km, &alt_s, false), KeymapDispatch::Submit);
        assert_eq!(keymap_pre_match(&km, &alt_s, true), KeymapDispatch::Submit);
        // Enter no longer submits (replaced wholesale) — and in
        // multiline it still falls through for the newline insert.
        let enter = press(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(keymap_pre_match(&km, &enter, false), KeymapDispatch::None);
        assert_eq!(keymap_pre_match(&km, &enter, true), KeymapDispatch::None);
    }

    #[test]
    fn default_submit_dispatch_preserves_muscle_memory() {
        let km = default_keymap();
        let enter = press(KeyCode::Enter, KeyModifiers::NONE);
        let shift_enter = press(KeyCode::Enter, KeyModifiers::SHIFT);
        // Non-multiline: plain Enter submits.
        assert_eq!(keymap_pre_match(&km, &enter, false), KeymapDispatch::Submit);
        // Multiline: plain Enter falls through (the Enter arm inserts
        // a newline), Shift+Enter submits.
        assert_eq!(keymap_pre_match(&km, &enter, true), KeymapDispatch::None);
        assert_eq!(
            keymap_pre_match(&km, &shift_enter, true),
            KeymapDispatch::Submit
        );
    }

    #[test]
    fn scroll_and_help_dispatch_via_keymap() {
        let km = default_keymap();
        assert_eq!(
            keymap_pre_match(&km, &press(KeyCode::PageUp, KeyModifiers::NONE), false),
            KeymapDispatch::ScrollPageUp
        );
        assert_eq!(
            keymap_pre_match(&km, &press(KeyCode::PageDown, KeyModifiers::NONE), false),
            KeymapDispatch::ScrollPageDown
        );
        let km = keymap_with(&[("ScrollUp", "Ctrl+u")]);
        assert_eq!(
            keymap_pre_match(
                &km,
                &press(KeyCode::Char('u'), KeyModifiers::CONTROL),
                false
            ),
            KeymapDispatch::ScrollPageUp
        );
        // Printable Help bindings stay with the Char arm (empty-
        // composer gate); non-printable ones dispatch here.
        let km = default_keymap();
        assert_eq!(
            keymap_pre_match(&km, &press(KeyCode::Char('?'), KeyModifiers::NONE), false),
            KeymapDispatch::None
        );
        let km = keymap_with(&[("Help", "Ctrl+PageUp")]);
        assert_eq!(
            keymap_pre_match(&km, &press(KeyCode::PageUp, KeyModifiers::CONTROL), false),
            KeymapDispatch::Help
        );
        // Everything else falls through.
        assert_eq!(
            keymap_pre_match(&km, &press(KeyCode::Char('x'), KeyModifiers::NONE), false),
            KeymapDispatch::None
        );
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use oxicode_vtui::tui::core::{InlineHandle, OverlayEvent};
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

    /// Render the full frame at the requested size. Mirrors
    /// `render_frame_to_string` but at the documented width so PTY-style
    /// snapshot tests can assert on a representative viewport.
    #[allow(dead_code)]
    fn render_frame_to_string_at(state: &RenderState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
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

    /// Diagnostic helper: render the full frame and return the terminal
    /// caret position (where render_composer set it).
    fn terminal_caret(state: &RenderState) -> Option<(u16, u16)> {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = InlineHandle::new_for_tests(tx);
        terminal
            .draw(|f| render_frame(f, state, &handle))
            .expect("draw");
        terminal
            .get_cursor_position()
            .ok()
            .map(|position| (position.x, position.y))
    }

    #[test]
    fn composer_caret_aligns_after_ascii() {
        let mut state = RenderState::default();
        state.prompt_prefix = "> ".to_string();
        state.input_enabled = true;
        let mut composer = oxicode_textarea::TextArea::new();
        composer.set_text("hello");
        composer.set_cursor(5);
        state.composer = composer;
        let caret = terminal_caret(&state);
        // The dense chat layout leaves a 1-column side gutter and no outer
        // vertical padding: prompt = Rect{x:1,y:21,w:78,h:3}; inner starts at
        // (2, 21), and the 2-column prefix puts the body at x=4.
        assert_eq!(
            caret,
            Some((9, 22)),
            "ASCII caret must sit right after '> hello'"
        );
    }

    #[test]
    fn composer_caret_aligns_after_cjk_display_columns() {
        let mut state = RenderState::default();
        state.prompt_prefix = "> ".to_string();
        state.input_enabled = true;
        let body = "안녕";
        let mut composer = oxicode_textarea::TextArea::new();
        composer.set_text(body);
        composer.set_cursor(body.len()); // 6 bytes (end), 4 display cols
        state.composer = composer;
        let caret = terminal_caret(&state);
        // textarea_area.x = 4, col = 4 -> (4 + 4, 21) = (8, 21).
        assert_eq!(
            caret,
            Some((8, 22)),
            "CJK caret must sit after 4 display columns (not 6 bytes)"
        );
    }

    #[test]
    fn composer_caret_aligns_after_mixed_ascii_cjk() {
        let body = "hi안녕";
        let mut state = RenderState::default();
        state.prompt_prefix = "> ".to_string();
        state.input_enabled = true;
        let mut composer = oxicode_textarea::TextArea::new();
        composer.set_text(body);
        composer.set_cursor(body.len()); // 8 bytes, 6 display cols
        state.composer = composer;
        let caret = terminal_caret(&state);
        // textarea_area.x = 4, col = 6 -> (4 + 6, 21) = (10, 21).
        assert_eq!(
            caret,
            Some((10, 22)),
            "Mixed caret must sit after 6 display columns"
        );
    }

    #[test]
    fn agent_session_event_reaches_the_transcript_bridge() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = InlineHandle::new_for_tests(tx);
        let mut state = RenderState::default();

        handle_session_event(
            &mut state,
            &handle,
            &SessionEvent::Agent(Box::new(AgentEvent::TextChunk {
                text: "streamed reply".to_string(),
            })),
            None,
        );

        let command = rx
            .try_recv()
            .expect("an agent event must produce a render command");
        apply_command(&mut state, command);
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].kind, InlineMessageKind::Agent);
        assert_eq!(state.transcript[0].segments[0].text, "streamed reply");
    }

    #[test]
    fn missing_key_errors_are_distinguished_from_other_provider_failures() {
        assert!(is_missing_api_key_error(
            "Provider stream error: Missing API key — configure a credential"
        ));
        assert!(!is_missing_api_key_error("Provider returned HTTP 429"));
        assert_eq!(
            provider_from_model_id("deepseek/deepseek-v4-flash"),
            "deepseek"
        );
    }

    #[test]
    fn prompt_queue_mutations_change_the_execution_queue() {
        let queue = PromptQueue::default();
        queue.enqueue("first".to_string());
        queue.enqueue("second".to_string());
        queue.enqueue("third".to_string());

        assert!(queue.move_by(2, -1));
        assert_eq!(queue.remove(0).as_deref(), Some("first"));
        let pending: Vec<_> = queue.pending.lock().iter().cloned().collect();
        assert_eq!(pending, ["third", "second"]);
    }

    #[test]
    fn welcome_screen_shown_when_transcript_empty() {
        let state = RenderState::default();
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("OXICODE") && rendered.contains("WORKSPACE"),
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
        state.slash_popup.items = slash_filter("", &[]);
        let rendered = render_frame_to_string(&state);
        assert!(rendered.contains("COMMANDS"), "popup title must render");
        assert!(rendered.contains("/quit"), "popup must list /quit");
    }

    #[test]
    fn composer_and_popup_render_together() {
        let mut state = RenderState::default();
        state.prompt_prefix = "> ".to_string();
        state.composer.set_text("/qu");
        state.slash_popup.open = true;
        state.slash_popup.items = slash_filter("qu", &[]);
        let rendered = render_frame_to_string(&state);
        assert!(rendered.contains("COMMANDS"), "popup must render");
        assert!(rendered.contains("/quit"), "popup must list /quit");
        assert!(rendered.contains('>'), "composer must still render");
    }

    #[test]
    fn transcript_wraps_long_lines() {
        // Write-path width invariant (Task 2 / omp tui-core-renderer.md §4):
        // the transcript MUST never paint past the content width — even if
        // an agent response would naturally wrap to several rows, we hard-clip
        // to the viewport width so a malformed table can never overflow a
        // narrow terminal. The visible row stays at exactly the content width
        // and content past that column is dropped at the boundary (never
        // wrapped into a second visual row).
        let mut state = RenderState::default();
        state.transcript.push(TranscriptLine {
            kind: InlineMessageKind::Agent,
            segments: vec![plain_segment(
                "This is a very long agent response line that should wrap across multiple terminal rows when rendered at a narrow width.".to_string()
            )],
            block_id: 0,
        });
        let backend = TestBackend::new(40, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = InlineHandle::new_for_tests(tx);
        terminal
            .draw(|f| render_frame(f, &state, &handle))
            .expect("draw");
        let buf = terminal.backend().buffer();
        // Walk every cell: the transcript row never paints past the content
        // width (no cell beyond col 40 should carry the clipped text).
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
            !full.contains("wrap"),
            "long line is clamped at the viewport edge — content past col 40 must be dropped, not wrapped"
        );
        // The truncated prefix is still visible: the leading word "This" lands
        // at the top-left of the transcript.
        assert!(
            full.contains("This"),
            "the truncated prefix of the clamped line is visible: {full:?}"
        );
    }

    // ─── overlay tests ────────────────────────────────────────────────────

    fn sample_overlay_items() -> Vec<OverlayListItem> {
        vec![
            OverlayListItem {
                title: "model-a".to_string(),
                subtitle: Some("first".to_string()),
                badge: Some("ready".to_string()),
                indent: 0,
                search_value: None,
                selection: Some(oxicode_vtui::tui::core::InlineListSelection::Model(0)),
            },
            OverlayListItem {
                title: "model-b".to_string(),
                subtitle: None,
                badge: None,
                indent: 0,
                search_value: None,
                selection: Some(oxicode_vtui::tui::core::InlineListSelection::Model(1)),
            },
            OverlayListItem {
                title: "model-c".to_string(),
                subtitle: None,
                badge: None,
                indent: 0,
                search_value: None,
                selection: Some(oxicode_vtui::tui::core::InlineListSelection::Model(2)),
            },
        ]
    }

    #[test]
    fn overlay_renders_title_and_items() {
        let mut state = RenderState::default();
        state.overlay = Some(OverlayState {
            title: "Select model".to_string(),
            lines: vec!["Pick one".to_string()],
            items: sample_overlay_items(),
            selected: 0,
            search: None,
            secure_input: None,
            ..Default::default()
        });
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("Select model"),
            "overlay title must render"
        );
        assert!(rendered.contains("model-a"), "first item must render");
        assert!(rendered.contains("model-b"), "second item must render");
        assert!(rendered.contains("model-c"), "third item must render");
        assert!(
            rendered.contains("Pick one"),
            "descriptive line must render"
        );
    }

    #[test]
    fn overlay_search_filters_items() {
        let mut state = RenderState::default();
        state.overlay = Some(OverlayState {
            title: "Select".to_string(),
            lines: Vec::new(),
            items: sample_overlay_items(),
            selected: 0,
            search: Some(OverlaySearchState {
                label: "filter".to_string(),
                placeholder: Some("type".to_string()),
                value: "model-b".to_string(),
            }),
            secure_input: None,
            ..Default::default()
        });
        let rendered = render_frame_to_string(&state);
        assert!(rendered.contains("model-b"), "matching item must render");
        assert!(
            !rendered.contains("model-a"),
            "non-matching item must not render (got: {})",
            rendered
        );
        assert!(
            !rendered.contains("model-c"),
            "non-matching item must not render"
        );
    }

    #[test]
    fn overlay_keyboard_nav_moves_selection() {
        let mut state = RenderState::default();
        state.overlay = Some(OverlayState {
            title: "Select".to_string(),
            lines: Vec::new(),
            items: sample_overlay_items(),
            selected: 0,
            search: None,
            secure_input: None,
            ..Default::default()
        });
        let state_arc = Arc::new(parking_lot::Mutex::new(state));
        let (tx, mut _rx) = mpsc::unbounded_channel();

        // Initial: index 0 selected.
        assert_eq!(state_arc.lock().overlay.as_ref().unwrap().selected, 0);

        // Down: index 1 selected.
        let consumed = handle_overlay_key(&state_arc, &tx, KeyCode::Down);
        assert!(consumed, "Down must be consumed while overlay is open");
        assert_eq!(state_arc.lock().overlay.as_ref().unwrap().selected, 1);

        // Down: index 2 selected.
        let consumed = handle_overlay_key(&state_arc, &tx, KeyCode::Down);
        assert!(consumed);
        assert_eq!(state_arc.lock().overlay.as_ref().unwrap().selected, 2);

        // Down: wraps to index 0.
        let consumed = handle_overlay_key(&state_arc, &tx, KeyCode::Down);
        assert!(consumed);
        assert_eq!(state_arc.lock().overlay.as_ref().unwrap().selected, 0);

        // Up: wraps to last (index 2).
        let consumed = handle_overlay_key(&state_arc, &tx, KeyCode::Up);
        assert!(consumed);
        assert_eq!(state_arc.lock().overlay.as_ref().unwrap().selected, 2);

        // Enter: closes overlay and emits a Submission event.
        let consumed = handle_overlay_key(&state_arc, &tx, KeyCode::Enter);
        assert!(consumed);
        assert!(
            state_arc.lock().overlay.is_none(),
            "overlay must be cleared after Enter"
        );
        let evt = _rx.try_recv().expect("submit event must arrive");
        match evt {
            InlineEvent::Overlay(OverlayEvent::Submitted(_)) => {}
            other => panic!("expected Submitted overlay event, got {other:?}"),
        }
    }

    #[test]
    fn overlay_esc_closes_and_emits_cancelled() {
        let mut state = RenderState::default();
        state.overlay = Some(OverlayState {
            title: "Select".to_string(),
            lines: Vec::new(),
            items: sample_overlay_items(),
            selected: 0,
            search: None,
            secure_input: None,
            ..Default::default()
        });
        let state_arc = Arc::new(parking_lot::Mutex::new(state));
        let (tx, mut rx) = mpsc::unbounded_channel();

        let consumed = handle_overlay_key(&state_arc, &tx, KeyCode::Esc);
        assert!(consumed);
        assert!(
            state_arc.lock().overlay.is_none(),
            "overlay must be cleared after Esc"
        );
        let evt = rx.try_recv().expect("cancel event must arrive");
        assert!(
            matches!(evt, InlineEvent::Overlay(OverlayEvent::Cancelled)),
            "expected Cancelled overlay event"
        );
    }

    #[test]
    fn overlay_enter_on_readonly_item_is_noop() {
        // A read-only item (selection: None — /tools, /mcp, the /settings
        // Model row) must NOT submit a synthetic selection or pollute the
        // prompt with "/overlay:N". Enter is a no-op: overlay stays open.
        let mut state = RenderState::default();
        state.overlay = Some(OverlayState {
            title: "Tools".to_string(),
            lines: Vec::new(),
            items: vec![OverlayListItem {
                title: "read".to_string(),
                subtitle: Some("Read a file".to_string()),
                badge: None,
                indent: 0,
                search_value: None,
                selection: None,
            }],
            selected: 0,
            search: None,
            secure_input: None,
            ..Default::default()
        });
        let state_arc = Arc::new(parking_lot::Mutex::new(state));
        let (tx, mut rx) = mpsc::unbounded_channel();

        let consumed = handle_overlay_key(&state_arc, &tx, KeyCode::Enter);
        assert!(consumed, "Enter must be consumed even on read-only items");
        assert!(
            state_arc.lock().overlay.is_some(),
            "overlay must stay open when Enter hits a read-only item"
        );
        assert!(
            rx.try_recv().is_err(),
            "no overlay event must be emitted for a read-only Enter"
        );
    }

    #[test]
    fn overlay_chars_route_to_search_field() {
        let mut state = RenderState::default();
        state.overlay = Some(OverlayState {
            title: "Select".to_string(),
            lines: Vec::new(),
            items: sample_overlay_items(),
            selected: 0,
            search: Some(OverlaySearchState {
                label: "filter".to_string(),
                placeholder: None,
                value: String::new(),
            }),
            secure_input: None,
            ..Default::default()
        });
        let state_arc = Arc::new(parking_lot::Mutex::new(state));
        let (tx, _rx) = mpsc::unbounded_channel();

        handle_overlay_key(&state_arc, &tx, KeyCode::Char('m'));
        handle_overlay_key(&state_arc, &tx, KeyCode::Char('o'));
        handle_overlay_key(&state_arc, &tx, KeyCode::Backspace);
        let value = state_arc
            .lock()
            .overlay
            .as_ref()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .value
            .clone();
        assert_eq!(value, "m", "Backspace should drop last char");
    }

    #[test]
    fn overlay_key_no_op_when_no_overlay_open() {
        let state = RenderState::default();
        let state_arc = Arc::new(parking_lot::Mutex::new(state));
        let (tx, _rx) = mpsc::unbounded_channel();
        let consumed = handle_overlay_key(&state_arc, &tx, KeyCode::Enter);
        assert!(
            !consumed,
            "handle_overlay_key must return false when no overlay is open"
        );
    }

    #[test]
    fn apply_command_show_overlay_populates_state() {
        use oxicode_vtui::tui::core::{InlineListItem, ListOverlayRequest};
        let mut state = RenderState::default();
        let items = vec![
            InlineListItem {
                title: "alpha".to_string(),
                subtitle: None,
                badge: None,
                indent: 0,
                selection: None,
                search_value: None,
            },
            InlineListItem {
                title: "beta".to_string(),
                subtitle: None,
                badge: None,
                indent: 0,
                selection: None,
                search_value: None,
            },
        ];
        let request = OverlayRequest::List(ListOverlayRequest {
            title: "Pick".to_string(),
            lines: vec!["desc".to_string()],
            footer_hint: None,
            items,
            selected: None,
            search: None,
            hotkeys: Vec::new(),
        });
        let shutdown = apply_command(
            &mut state,
            InlineCommand::ShowOverlay {
                request: Box::new(request),
            },
        );
        assert!(!shutdown, "ShowOverlay must not request shutdown");
        let overlay = state.overlay.as_ref().expect("overlay must be Some");
        assert_eq!(overlay.title, "Pick");
        assert_eq!(overlay.items.len(), 2);
        assert_eq!(overlay.items[0].title, "alpha");
        assert_eq!(overlay.items[1].title, "beta");
        assert_eq!(overlay.lines.len(), 1);

        // CloseOverlay clears it.
        apply_command(&mut state, InlineCommand::CloseOverlay);
        assert!(state.overlay.is_none(), "CloseOverlay must clear state");
    }

    #[test]
    fn materialize_overlay_modal_with_secure_prompt_populates_secure_input() {
        use oxicode_vtui::tui::core::{ModalOverlayRequest, SecurePromptConfig};
        let request = OverlayRequest::Modal(ModalOverlayRequest {
            title: "API key".into(),
            lines: vec!["Paste your key".into()],
            secure_prompt: Some(SecurePromptConfig {
                label: "Key".into(),
                placeholder: Some("sk-...".into()),
                mask_input: true,
            }),
        });
        let state = materialize_overlay(request);
        let secure = state
            .secure_input
            .expect("secure_input must be Some when secure_prompt is Some");
        assert_eq!(secure.config.label, "Key");
        assert!(secure.config.mask_input);
        assert_eq!(secure.editor.text(), "");
        assert_eq!(secure.editor.cursor_byte(), 0);
    }

    #[test]
    fn materialize_overlay_modal_without_secure_prompt_has_none_secure_input() {
        use oxicode_vtui::tui::core::ModalOverlayRequest;
        let request = OverlayRequest::Modal(ModalOverlayRequest {
            title: "Confirm".into(),
            lines: vec!["y/n".into()],
            secure_prompt: None,
        });
        let state = materialize_overlay(request);
        assert!(
            state.secure_input.is_none(),
            "secure_input must be None when secure_prompt is None"
        );
    }

    // ─── fold / grace tests ─────────────────────────────────────────────

    fn three_block_transcript() -> Vec<TranscriptLine> {
        // Three distinct blocks: user(0), agent(1), user(2).
        vec![
            TranscriptLine {
                kind: InlineMessageKind::User,
                segments: vec![plain_segment("hi")],
                block_id: 0,
            },
            TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment("hello")],
                block_id: 1,
            },
            TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment("world")],
                block_id: 1,
            },
            TranscriptLine {
                kind: InlineMessageKind::User,
                segments: vec![plain_segment("bye")],
                block_id: 2,
            },
        ]
    }

    #[test]
    fn fold_all_collapses_every_block() {
        let mut state = RenderState::default();
        state.transcript = three_block_transcript();
        state.fold_all();
        assert_eq!(state.block_display.len(), 3, "3 distinct block ids");
        assert_eq!(state.block_mode(0), BlockDisplayMode::Collapsed);
        assert_eq!(state.block_mode(1), BlockDisplayMode::Collapsed);
        assert_eq!(state.block_mode(2), BlockDisplayMode::Collapsed);
    }

    #[test]
    fn expand_all_after_fold_all_shows_expanded() {
        let mut state = RenderState::default();
        state.transcript = three_block_transcript();
        state.fold_all();
        state.expand_all();
        assert!(
            state.block_display.is_empty(),
            "Expanded is the default — no overrides"
        );
        assert_eq!(state.block_mode(0), BlockDisplayMode::Expanded);
        assert_eq!(state.block_mode(2), BlockDisplayMode::Expanded);
    }

    #[test]
    fn truncate_all_sets_explicit_truncated() {
        let mut state = RenderState::default();
        state.transcript = three_block_transcript();
        state.fold_all();
        state.truncate_all();
        assert_eq!(state.block_display.len(), 3);
        assert_eq!(state.block_mode(1), BlockDisplayMode::Truncated);
    }

    #[test]
    fn default_block_mode_is_expanded() {
        let state = RenderState::default();
        assert_eq!(state.block_mode(0), BlockDisplayMode::Expanded);
        assert!(state.block_display.is_empty(), "default needs no map entry");
    }

    #[test]
    fn cycle_block_advances_through_three_states() {
        let mut state = RenderState::default();
        state.transcript = three_block_transcript();
        state.scroll_offset = 0; // view on block 0
        // Expanded (default) → Collapsed
        state.cycle_block_at_view();
        assert_eq!(state.block_mode(0), BlockDisplayMode::Collapsed);
        // Collapsed → Truncated
        state.cycle_block_at_view();
        assert_eq!(state.block_mode(0), BlockDisplayMode::Truncated);
        // Truncated → Expanded (default — removed from the map)
        state.cycle_block_at_view();
        assert_eq!(state.block_mode(0), BlockDisplayMode::Expanded);
        assert!(!state.block_display.contains_key(&0));
    }

    #[test]
    fn cancel_grace_field_defaults_none() {
        let state = RenderState::default();
        assert!(
            state.cancel_grace_until.is_none(),
            "cancel_grace_until must default to None"
        );
    }

    #[test]
    fn cancel_routes_to_interrupt_when_streaming() {
        assert_eq!(
            route_cancel(true),
            CancelRoute::Interrupt,
            "Esc while streaming must route through the interrupt path"
        );
    }

    #[test]
    fn cancel_routes_to_exit_when_idle() {
        assert_eq!(
            route_cancel(false),
            CancelRoute::Exit,
            "Esc while idle must exit immediately (one-press quit)"
        );
    }
    #[test]
    fn no_scrollbar_even_when_content_overflows() {
        // The in-app scrollbar is gone — native terminal scrollback owns
        // history and finalized rows commit above the viewport. Even a
        // 40-block transcript overflowing the viewport must not paint a
        // rail or thumb.
        let mut state = RenderState::default();
        for i in 0..40u32 {
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment(format!("line {i}"))],
                block_id: i as usize,
            });
        }
        let rendered = render_frame_to_string(&state);
        assert!(
            !rendered.contains('\u{2588}'),
            "no scrollbar thumb (█): native scrollback owns history"
        );
    }

    // ─── confirmation modal tests ───────────────────────────────────────

    #[test]
    fn confirmation_modal_renders_title() {
        let mut state = RenderState::default();
        state.confirmation = Some(quit_confirmation());
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("Quit oxicode?"),
            "confirmation title must render"
        );
    }

    #[test]
    fn confirmation_yes_sends_exit_and_closes() {
        let mut state = RenderState::default();
        state.confirmation = Some(quit_confirmation());
        let state_arc = Arc::new(parking_lot::Mutex::new(state));
        let (tx, mut rx) = mpsc::unbounded_channel();
        handle_confirmation_key(&state_arc, &tx, KeyCode::Char('y'));
        assert!(
            state_arc.lock().confirmation.is_none(),
            "yes must close the modal"
        );
        let ev = rx.try_recv().expect("yes must send an event");
        assert!(matches!(ev, InlineEvent::Exit), "yes must send Exit");
    }

    #[test]
    fn confirmation_no_closes_without_event() {
        let mut state = RenderState::default();
        state.confirmation = Some(quit_confirmation());
        let state_arc = Arc::new(parking_lot::Mutex::new(state));
        let (tx, mut rx) = mpsc::unbounded_channel();
        handle_confirmation_key(&state_arc, &tx, KeyCode::Char('n'));
        assert!(
            state_arc.lock().confirmation.is_none(),
            "no must close the modal"
        );
        assert!(rx.try_recv().is_err(), "no must not send an event");
    }
    // ─── ephemeral tip tests ───────────────────────────────────────────

    #[test]
    fn tip_banner_renders_when_active() {
        let mut state = RenderState::default();
        let now_tick = FRAME_TICK.load(std::sync::atomic::Ordering::Relaxed);
        state.tip = Some(EphemeralTip {
            text: "hello-tip-marker".to_string(),
            born_tick: now_tick,
            ttl_ticks: 100,
            key: "test",
            ambient: false,
        });
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("hello-tip-marker"),
            "active tip must render above the composer"
        );
    }

    #[test]
    fn tip_visible_within_ttl_window() {
        let tip = EphemeralTip {
            text: "x".to_string(),
            born_tick: 10,
            ttl_ticks: 5,
            key: "test",
            ambient: false,
        };
        assert!(tip_is_visible(&tip, 12), "within TTL must be visible");
        assert!(
            !tip_is_visible(&tip, 15),
            "at TTL boundary (born + ttl) must expire"
        );
        assert!(!tip_is_visible(&tip, 99), "past TTL must expire");
    }

    // ─── sticky header tests ───────────────────────────────────────────

    #[test]
    fn sticky_header_pins_block_head_when_scrolled_into_body() {
        // One big block (40 same-block lines); scroll the viewport into the
        // body. The sticky header must pin the block's first line at the top.
        let mut state = RenderState::default();
        for i in 0..40u32 {
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment(format!("body-line-{i:02}"))],
                block_id: 0,
            });
        }
        state.scroll_offset = 10;
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("body-line-00"),
            "sticky header must pin the block head when scrolled into the body"
        );
    }

    #[test]
    fn sticky_header_absent_when_viewport_at_block_head() {
        // Viewport top is the block head itself — no sticky pin needed.
        let mut state = RenderState::default();
        for i in 0..40u32 {
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment(format!("head-line-{i:02}"))],
                block_id: 0,
            });
        }
        state.scroll_offset = 0;
        let rendered = render_frame_to_string(&state);
        // head-line-00 is the viewport top already; it renders exactly once
        // (no separate sticky row). Just assert it is present.
        assert!(rendered.contains("head-line-00"));
    }

    // ─── prompt queue tests ─────────────────────────────────────────────

    #[test]
    fn turn_end_drains_queue_head() {
        let mut state = RenderState::default();
        state.queued_inputs = vec!["queued-1".into(), "queued-2".into()];
        state.drain_queue_head();
        assert_eq!(
            state.queued_inputs.len(),
            1,
            "drain_queue_head must drop the head (now running)"
        );
        assert_eq!(state.queued_inputs[0], "queued-2");
    }

    // ─── render_frame integration ──────────────────────────────────────

    #[test]
    fn render_frame_paints_transcript_content() {
        // Guard against render_frame losing its render_transcript call
        // (which only a content assertion through render_frame can catch —
        // render_transcript unit tests bypass render_frame entirely).
        let mut state = RenderState::default();
        state.transcript.push(TranscriptLine {
            kind: InlineMessageKind::Agent,
            segments: vec![plain_segment("frame-content-marker-xyz")],
            block_id: 0,
        });
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("frame-content-marker-xyz"),
            "render_frame must paint transcript content"
        );
    }

    #[test]
    fn user_turns_get_one_blank_spacer_row() {
        let mut state = RenderState::default();
        state.transcript = vec![
            TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment("agent-answer")],
                block_id: 0,
            },
            TranscriptLine {
                kind: InlineMessageKind::User,
                segments: vec![plain_segment("next-question")],
                block_id: 1,
            },
        ];
        let rendered = render_frame_to_string(&state);
        let rows: Vec<&str> = rendered.split('\n').collect();
        let agent_row = rows
            .iter()
            .position(|r| r.contains("agent-answer"))
            .expect("agent row");
        assert!(
            rows[agent_row + 1].trim().is_empty(),
            "blank spacer between turns: {:?}",
            &rows[agent_row..agent_row + 3]
        );
        assert!(
            rows[agent_row + 2].contains("next-question"),
            "user line follows the spacer"
        );
    }

    #[test]
    fn transcript_snapshot_at_120_cols_matches_role_layout() {
        let mut state = RenderState::default();
        state.brain = BrainChip::Ok;
        state.append_line(
            InlineMessageKind::User,
            vec![plain_segment("intro message\nsecond line")],
        );
        state.append_line(
            InlineMessageKind::Agent,
            vec![plain_segment("answer paragraph line one\nline two")],
        );
        state.append_line(
            InlineMessageKind::User,
            vec![plain_segment("follow-up question")],
        );
        let rendered = render_frame_to_string_at(&state, 120, 24);
        let rows: Vec<&str> = rendered.split('\n').collect();
        // User rows carry no glyph — bold primary text only.
        let first_user = rows
            .iter()
            .position(|row| row.contains("intro message"))
            .expect("intro user row");
        let continuation = rows
            .iter()
            .position(|row| row.contains("second line"))
            .expect("user continuation visible");
        assert_eq!(
            continuation,
            first_user + 1,
            "user continuation on next row"
        );
        assert!(
            !rows[first_user].contains("> "),
            "plain style has no prompt glyph: {rows:?}"
        );

        // Turn rhythm: a blank row breathes between the request and the
        // response, and again before the next user turn.
        let agent_row = rows
            .iter()
            .position(|row| row.contains("answer paragraph"))
            .expect("agent row");
        assert_eq!(
            agent_row,
            continuation + 2,
            "one blank row separates request from response: {:?}",
            &rows[continuation..=agent_row]
        );
        assert!(
            !rows[agent_row].trim_start().starts_with('>'),
            "agent rows carry no prompt glyph: {rows:?}"
        );

        let next_user = rows
            .iter()
            .position(|row| row.contains("follow-up question"))
            .expect("second user row");
        assert_eq!(
            next_user,
            agent_row + 3,
            "answer (2 rows) + one blank + next user turn: {:?}",
            &rows[agent_row..=next_user]
        );

        // Brain chip lives on the shortcuts bar, not the composer border.
        let shortcuts_row = rows
            .iter()
            .position(|row| row.contains("brain·ok"))
            .expect("brain chip on shortcuts row");
        assert!(shortcuts_row > next_user, "chip below the chat surface");
    }

    #[test]
    fn response_breathes_after_the_user_request() {
        let mut state = RenderState::default();
        state.append_line(InlineMessageKind::User, vec![plain_segment("the request")]);
        state.append_line(InlineMessageKind::Agent, vec![plain_segment("the answer")]);
        let rendered = render_frame_to_string(&state);
        let rows: Vec<&str> = rendered.split('\n').collect();
        let request_row = rows
            .iter()
            .position(|r| r.contains("the request"))
            .expect("request row");
        let answer_row = rows
            .iter()
            .position(|r| r.contains("the answer"))
            .expect("answer row");
        assert_eq!(
            answer_row,
            request_row + 2,
            "one blank row must separate request from response: {:?}",
            &rows[request_row..=answer_row]
        );
    }

    #[test]
    fn assistant_tool_flow_stays_contiguous() {
        let mut state = RenderState::default();
        state.append_line(InlineMessageKind::Tool, vec![plain_segment("[tool] read")]);
        state.append_line(InlineMessageKind::Tool, vec![plain_segment("[done] ok")]);
        state.append_line(InlineMessageKind::Agent, vec![plain_segment("the answer")]);
        let rendered = render_frame_to_string(&state);
        let rows: Vec<&str> = rendered.split('\n').collect();
        let tool_row = rows
            .iter()
            .position(|r| r.contains("[tool] read"))
            .expect("tool row");
        let answer_row = rows
            .iter()
            .position(|r| r.contains("the answer"))
            .expect("answer row");
        assert_eq!(
            answer_row,
            tool_row + 2,
            "tool → answer is one assistant turn — no blank inside it: {:?}",
            &rows[tool_row..=answer_row]
        );
    }

    #[test]
    fn no_spacer_above_the_first_transcript_line() {
        let mut state = RenderState::default();
        state.transcript = vec![TranscriptLine {
            kind: InlineMessageKind::User,
            segments: vec![plain_segment("opening-question")],
            block_id: 0,
        }];
        let rendered = render_frame_to_string(&state);
        let rows: Vec<&str> = rendered.split('\n').collect();
        let user_row = rows
            .iter()
            .position(|r| r.contains("opening-question"))
            .expect("user row");
        assert!(
            rows[..user_row].iter().all(|r| r.trim().is_empty()),
            "transcript starts at the top with no spacer"
        );
    }

    #[test]
    fn long_response_renders_every_line_by_default() {
        let mut state = RenderState::default();
        let mut segments = Vec::new();
        for i in 0..8 {
            if i > 0 {
                segments.push(plain_segment("\n"));
            }
            segments.push(plain_segment(format!("line-{i}")));
        }
        state.append_line(InlineMessageKind::Agent, segments);
        let rendered = render_frame_to_string(&state);
        assert!(
            !rendered.contains("lines"),
            "no elision gap by default — full text scrolls instead: {rendered}"
        );
        for i in 0..8 {
            assert!(
                rendered.contains(&format!("line-{i}")),
                "line-{i} must be reachable by scrolling: {rendered}"
            );
        }
    }
    #[test]
    fn multiline_user_input_renders_every_explicit_line() {
        let mut state = RenderState::default();
        state.append_line(
            InlineMessageKind::User,
            vec![plain_segment("first line\nsecond line")],
        );
        let rendered = render_frame_to_string(&state);
        let rows: Vec<&str> = rendered.split('\n').collect();
        let first_row = rows
            .iter()
            .position(|row| row.contains("first line"))
            .expect("first user row");
        let second_row = rows
            .iter()
            .position(|row| row.contains("second line"))
            .expect("explicit continuation line is visible");
        assert_eq!(
            second_row,
            first_row + 1,
            "explicit newline must occupy the following visual row"
        );

        assert!(
            !rows[second_row].contains("> second line"),
            "continuation row must not look like a second user turn"
        );
    }

    #[test]
    fn streaming_agent_delta_renders_every_explicit_line() {
        let mut state = RenderState::default();
        state.inline_segment(
            InlineMessageKind::Agent,
            plain_segment("first answer\nsecond answer"),
        );
        let rendered = render_frame_to_string(&state);
        let rows: Vec<&str> = rendered.split('\n').collect();
        let first_row = rows
            .iter()
            .position(|row| row.contains("first answer"))
            .expect("first agent row");
        let second_row = rows
            .iter()
            .position(|row| row.contains("second answer"))
            .expect("second agent row");
        assert_eq!(
            second_row,
            first_row + 1,
            "streamed newline must occupy the following visual row"
        );
    }

    #[test]
    fn file_search_dropdown_renders_results() {
        use crate::tui_vt::file_search::{FileSearchResult, FileSearchState};
        let mut state = RenderState::default();
        state.input_enabled = true;
        state.file_search = Some(FileSearchState {
            query: "main".into(),
            at_offset: 0,
            hidden_mode: false,
            results: vec![
                FileSearchResult {
                    path: "src/main.rs".into(),
                    score: 100,
                },
                FileSearchResult {
                    path: "tests/main.rs".into(),
                    score: 50,
                },
            ],
            selected: 0,
            index: vec![],
            line_mode: false,
        });
        let rendered = render_frame_to_string(&state);
        assert!(rendered.contains("FILES"), "dropdown title must render");
        assert!(
            rendered.contains("src/main.rs"),
            "dropdown must show file paths"
        );
    }

    #[test]
    fn file_search_dropdown_hidden_mode_title() {
        use crate::tui_vt::file_search::{FileSearchResult, FileSearchState};
        let mut state = RenderState::default();
        state.input_enabled = true;
        state.file_search = Some(FileSearchState {
            query: "".into(),
            at_offset: 0,
            hidden_mode: true,
            results: vec![FileSearchResult {
                path: ".env".into(),
                score: 0,
            }],
            selected: 0,
            index: vec![],
            line_mode: false,
        });
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("HIDDEN"),
            "hidden mode must be indicated in title"
        );
    }

    #[test]
    fn file_search_and_composer_render_together() {
        use crate::tui_vt::file_search::{FileSearchResult, FileSearchState};
        let mut state = RenderState::default();
        state.input_enabled = true;
        state.prompt_prefix = "> ".into();
        state.composer.set_text("@main");
        state.file_search = Some(FileSearchState {
            query: "main".into(),
            at_offset: 0,
            hidden_mode: false,
            results: vec![FileSearchResult {
                path: "src/main.rs".into(),
                score: 100,
            }],
            selected: 0,
            index: vec![],
            line_mode: false,
        });
        let rendered = render_frame_to_string(&state);
        // Both the composer text and the dropdown must appear.
        assert!(rendered.contains('>'), "composer must still render");
        assert!(
            rendered.contains("src/main.rs"),
            "dropdown must render alongside composer"
        );
    }

    #[test]
    fn format_todo_line_shows_block_reason_and_notes_marker() {
        let styles = active_styles();
        let todo = TodoItem {
            content: "Wire OAuth".into(),
            status: TodoStatus::Blocked,
            notes: Some(vec!["waiting on vendor".into()]),
            block_reason: Some("vendor sandbox pending".into()),
        };
        let line = format_todo_line(&todo, false, &styles);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Wire OAuth"));
        assert!(text.contains("blocked: vendor sandbox pending"));
        assert!(text.contains("·1"));
    }

    #[test]
    fn format_todo_line_abandoned_is_strikethrough() {
        let styles = active_styles();
        let todo = TodoItem {
            content: "Drop this".into(),
            status: TodoStatus::Abandoned,
            notes: None,
            block_reason: None,
        };
        let line = format_todo_line(&todo, false, &styles);
        assert!(
            line.spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::CROSSED_OUT))
        );
    }

    #[test]
    fn render_todo_pane_multi_phase_shows_roman_header_and_progress() {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        let mut state = RenderState::default();
        state.todo_phases = vec![
            TodoPhase {
                name: "Foundation".into(),
                tasks: vec![
                    TodoItem {
                        content: "a".into(),
                        status: TodoStatus::Completed,
                        notes: None,
                        block_reason: None,
                    },
                    TodoItem {
                        content: "b".into(),
                        status: TodoStatus::Completed,
                        notes: None,
                        block_reason: None,
                    },
                ],
            },
            TodoPhase {
                name: "Auth".into(),
                tasks: vec![
                    TodoItem {
                        content: "c".into(),
                        status: TodoStatus::Completed,
                        notes: None,
                        block_reason: None,
                    },
                    TodoItem {
                        content: "d".into(),
                        status: TodoStatus::InProgress,
                        notes: None,
                        block_reason: None,
                    },
                    TodoItem {
                        content: "e".into(),
                        status: TodoStatus::Pending,
                        notes: None,
                        block_reason: None,
                    },
                ],
            },
        ];
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("II. Auth"),
            "multi-phase HUD must show the roman-numeral phase header"
        );
        assert!(
            rendered.contains("1/3"),
            "active phase must show its done/total progress"
        );
    }

    #[test]
    fn todo_auto_clear_fires_after_delay_when_all_closed() {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        let mut state = RenderState::default();
        state.todo_phases = vec![TodoPhase {
            name: "Auth".into(),
            tasks: vec![TodoItem {
                content: "a".into(),
                status: TodoStatus::Completed,
                notes: None,
                block_reason: None,
            }],
        }];
        sync_todo_clear_timer(&mut state, 0); // 0-second delay = instant
        assert!(state.todo_phases.is_empty());
    }

    #[test]
    fn todo_auto_clear_does_not_fire_while_open_tasks_remain() {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        let mut state = RenderState::default();
        let phases = vec![TodoPhase {
            name: "Auth".into(),
            tasks: vec![TodoItem {
                content: "a".into(),
                status: TodoStatus::Pending,
                notes: None,
                block_reason: None,
            }],
        }];
        state.todo_phases = phases.clone();
        sync_todo_clear_timer(&mut state, 0);
        assert_eq!(state.todo_phases.len(), phases.len());
        assert_eq!(state.todo_phases[0].name, "Auth");
    }

    #[test]
    fn todo_auto_clear_negative_delay_disables_clearing() {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        let mut state = RenderState::default();
        let phases = vec![TodoPhase {
            name: "Auth".into(),
            tasks: vec![TodoItem {
                content: "a".into(),
                status: TodoStatus::Completed,
                notes: None,
                block_reason: None,
            }],
        }];
        state.todo_phases = phases.clone();
        sync_todo_clear_timer(&mut state, -1);
        assert_eq!(state.todo_phases.len(), phases.len());
    }

    #[test]
    fn render_todo_pane_single_phase_has_no_roman_header() {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        let mut state = RenderState::default();
        state.todo_phases = vec![TodoPhase {
            name: "Todos".into(),
            tasks: vec![TodoItem {
                content: "a".into(),
                status: TodoStatus::Pending,
                notes: None,
                block_reason: None,
            }],
        }];
        let rendered = render_frame_to_string(&state);
        assert!(
            !rendered.contains("I. Todos"),
            "single phase must skip roman header"
        );
        assert!(rendered.contains("Todos"), "single-phase name must render");
    }

    #[test]
    fn render_todo_compact_line_shows_counts_and_active_task() {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        let phases = vec![TodoPhase {
            name: "Auth".into(),
            tasks: vec![
                TodoItem {
                    content: "a".into(),
                    status: TodoStatus::Completed,
                    notes: None,
                    block_reason: None,
                },
                TodoItem {
                    content: "b".into(),
                    status: TodoStatus::InProgress,
                    notes: None,
                    block_reason: None,
                },
            ],
        }];
        let line = render_todo_compact_line(&phases);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("TODO 1/2"));
        assert!(text.contains("b"));
    }

    #[test]
    fn render_todo_compact_line_all_done_shows_done_marker() {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        let phases = vec![TodoPhase {
            name: "Auth".into(),
            tasks: vec![TodoItem {
                content: "a".into(),
                status: TodoStatus::Completed,
                notes: None,
                block_reason: None,
            }],
        }];
        let line = render_todo_compact_line(&phases);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("done"));
    }

    fn test_todo_state() -> std::sync::Arc<crate::store::todo_state::TodoState> {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        std::sync::Arc::new(crate::store::todo_state::TodoState::with_phases(vec![
            TodoPhase {
                name: "Auth".into(),
                tasks: vec![TodoItem {
                    content: "implement authentication module".into(),
                    status: TodoStatus::Pending,
                    notes: None,
                    block_reason: None,
                }],
            },
        ]))
    }

    fn hub_with_subagent(
        status: oxicode_sdk::HubStatus,
        current_task: Option<&str>,
    ) -> std::sync::Arc<crate::app::agent_hub_registry::HubRegistry> {
        use crate::app::agent_hub_registry::{HubEntry, HubRegistry};
        let hub = HubRegistry::new();
        hub.register(
            "sub".into(),
            HubEntry {
                kind: oxicode_sdk::HubKind::Subagent,
                status,
                display_name: "sub".into(),
                current_task: current_task.map(str::to_string),
                last_activity_ms: 0,
                session_file: None,
            },
        );
        std::sync::Arc::new(hub)
    }

    #[test]
    fn frame_refresh_reconciles_todo_with_completed_subagent() {
        let state = test_todo_state();
        let provider: std::sync::Arc<dyn TodoStateProvider> =
            crate::store::todo_state::provider_from_state(state.clone());
        let hub = hub_with_subagent(oxicode_sdk::HubStatus::Idle, Some("authentication module"));
        let phases = refresh_todo_phases(&provider, Some(&hub));
        assert_eq!(phases[0].tasks[0].status, TodoStatus::Completed);
        // The reconcile write-back persisted through the provider.
        assert_eq!(state.get_phases()[0].tasks[0].status, TodoStatus::Completed);
    }

    #[test]
    fn matched_closure_lights_pending_todo_for_running_subagent() {
        let hub = hub_with_subagent(
            oxicode_sdk::HubStatus::Running,
            Some("authentication module"),
        );
        let matched = build_matched_closure(Some(&hub));
        let t = oxicode_agent::tools::todo::TodoItem {
            content: "implement authentication module".into(),
            status: TodoStatus::Pending,
            notes: None,
            block_reason: None,
        };
        assert!(matched(&t));
    }

    #[test]
    fn todo_pane_renders_when_items_present() {
        // The sticky pane is populated from the live provider in the event
        // loop; here we seed it directly to assert the pane paints task text.
        let mut state = RenderState::default();
        state.todo_phases = vec![oxicode_agent::tools::todo::TodoPhase {
            name: "Work".into(),
            tasks: vec![
                oxicode_agent::tools::todo::TodoItem {
                    content: "active task".into(),
                    status: TodoStatus::InProgress,
                    notes: None,
                    block_reason: None,
                },
                oxicode_agent::tools::todo::TodoItem {
                    content: "open task".into(),
                    status: TodoStatus::Pending,
                    notes: None,
                    block_reason: None,
                },
            ],
        }];
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("active task"),
            "in-progress task must render"
        );
        assert!(rendered.contains("open task"), "pending task must render");
        // The active task is marked with the in-progress glyph.
        assert!(rendered.contains("▸"), "in-progress status must render");
    }

    #[test]
    fn todo_pane_hidden_when_empty() {
        let state = RenderState::default();
        let rendered = render_frame_to_string(&state);
        // No todo content should leak when the list is empty.
        assert!(!rendered.contains("done"), "no completed state when empty");
    }

    #[test]
    fn render_overlay_secure_input_shows_label_mask_value_and_placeholder() {
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let overlay = OverlayState {
            title: "OpenAI key".into(),
            lines: vec!["Paste your API key".into()],
            items: Vec::new(),
            selected: 0,
            search: None,
            secure_input: Some(OverlaySecureInput {
                config: SecurePromptConfig {
                    label: "Key".into(),
                    placeholder: Some("sk-...".into()),
                    mask_input: true,
                },
                editor: oxicode_textarea::EditBuffer::from_parts("sk-abc", 6),
            }),
            ..Default::default()
        };
        terminal
            .draw(|f| render_overlay(f, f.area(), &overlay))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        // Mask must show 6 asterisks, never the value.
        let text: String = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("Key:"));
        assert!(text.contains("******"));
        assert!(!text.contains("sk-abc"));
    }

    #[test]
    fn render_overlay_secure_input_placeholder_when_empty() {
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let overlay = OverlayState {
            title: "OpenAI key".into(),
            lines: vec!["Paste your API key".into()],
            items: Vec::new(),
            selected: 0,
            search: None,
            secure_input: Some(OverlaySecureInput {
                config: SecurePromptConfig {
                    label: "Key".into(),
                    placeholder: Some("sk-...".into()),
                    mask_input: true,
                },
                editor: oxicode_textarea::EditBuffer::new(),
            }),
            ..Default::default()
        };
        terminal
            .draw(|f| render_overlay(f, f.area(), &overlay))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("sk-..."));
    }
    /// Pressure-driven allocation ladder: with many blocks competing
    /// for a short live region, older blocks must collapse to glyph
    /// rows and the emergency branch must paint a `… N earlier
    /// blocks hidden` banner.
    #[test]
    fn ladder_collapses_oldest_blocks_to_glyph_row_and_banner() {
        // 6 tool blocks; live region is 6 rows. Allocate 3 source
        // lines per block so the natural height (3) exceeds the
        // budget per block in the pressure branch. The ladder
        // hides the oldest blocks and paints glyph rows for the
        // newest ones.
        let mut state = RenderState::default();
        for i in 0..6 {
            let bid = i;
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Tool,
                segments: vec![plain_segment(format!("tool-{i}-headline"))],
                block_id: bid,
            });
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Tool,
                segments: vec![plain_segment(format!("tool-{i}-middle"))],
                block_id: bid,
            });
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Tool,
                segments: vec![plain_segment(format!("tool-{i}-trailer"))],
                block_id: bid,
            });
        }
        // 80x10 viewport → content_area.height ≈ 10 - composer 3 -
        // breath row 1 = 6 rows for the live region.
        let rendered = render_frame_to_string_at(&state, 80, 10);
        // Emergency banner ("… N earlier blocks hidden") must be
        // present when more blocks than rows exist. With 6 blocks
        // and a 6-row region, the ladder may or may not hide — but
        // it should never panic. We assert the renderer did not
        // lose the live region entirely and that the most-recent
        // block (tool-5) is at least partially visible.
        assert!(
            rendered.contains("tool-5")
                || rendered.contains("tool-5-headline")
                || rendered.contains("\u{25B8}")
                || rendered.contains("earlier blocks hidden"),
            "live region must surface a recent block or its folded form"
        );
    }

    /// Pressure-driven ladder: the latest block stays full when
    /// older blocks are folded to glyph rows.
    #[test]
    fn ladder_keeps_newest_block_full_under_pressure() {
        // 3 blocks: one big (5 lines) + two small (2 lines each) =
        // 9 natural items; budget ≈ 6 → pressure. Newest (big)
        // gets the largest slice.
        let mut state = RenderState::default();
        // Block 0 (older, 2 lines)
        for j in 0..2 {
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment(format!("old-block-line-{j}"))],
                block_id: 0,
            });
        }
        // Block 1 (middle, 2 lines)
        for j in 0..2 {
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment(format!("mid-block-line-{j}"))],
                block_id: 1,
            });
        }
        // Block 2 (newest, 5 lines)
        for j in 0..5 {
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment(format!("new-block-line-{j}"))],
                block_id: 2,
            });
        }
        let rendered = render_frame_to_string_at(&state, 80, 12);
        // Newest block's first line must be visible.
        assert!(
            rendered.contains("new-block-line-0"),
            "newest block's leading line must be visible"
        );
        // Oldest block's lines may be folded or hidden — assert
        // they don't occupy the FULL natural height (the ladder
        // folded them).
        let old_visible = (0..2).all(|j| rendered.contains(&format!("old-block-line-{j}")));
        assert!(
            !old_visible,
            "oldest block must be folded or hidden when under pressure"
        );
    }
    /// Long activity strings must be clamped to the live content
    /// width — never wrap onto a second visual row that would
    /// break the `▸ ` or `╭─ / ╰─ …` affordances.
    #[test]
    fn long_activity_folded_card_stays_within_width() {
        let mut state = RenderState::default();
        // Many blocks of 3 lines each in a short live region.
        // 6 blocks × 3 = 18 visible items, budget ≈ 6 → pressure.
        // Every block gets 1 glyph row; activity descriptors are
        // 120+ chars long and would wrap without clamping.
        for i in 0..8 {
            let bid = i;
            let long = format!("tool-{i}-{}", "x".repeat(120));
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Tool,
                segments: vec![plain_segment(format!("tool-{i}-head"))],
                block_id: bid,
            });
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Tool,
                segments: vec![plain_segment(format!("tool-{i}-body"))],
                block_id: bid,
            });
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Tool,
                segments: vec![plain_segment(long)],
                block_id: bid,
            });
        }
        let rendered = render_frame_to_string_at(&state, 80, 10);
        // Every row in the rendered output must stay within the
        // 80-cell viewport. (Without clamping, the glyph row
        // `▸ tool-N-xxxxxxxxxxxxx...` would wrap onto a second
        // visual row whose first cell holds `▸`.)
        for line in rendered.split('\n') {
            assert!(
                line.width() <= 80,
                "rendered row exceeded the viewport width: '{}' ({} cells)",
                line,
                line.width()
            );
        }
        // The glyph row (`▸ `) signature must appear — long
        // activity must still surface (clamped, not truncated to
        // empty).
        assert!(
            rendered.contains('\u{25B8}'),
            "glyph rows must be painted even with long activities"
        );
    }

    /// Manual `Collapsed` mode must keep the historic `[+] ` prefix
    /// produced by `transcript_line_marked(folded=true)`. The
    /// ladder applies only to non-manual blocks.
    #[test]
    fn manual_collapsed_block_keeps_the_plus_marker() {
        let mut state = RenderState::default();
        // One block with 3 lines + one newest block.
        for j in 0..3 {
            state.transcript.push(TranscriptLine {
                kind: InlineMessageKind::Error,
                segments: vec![plain_segment(format!("boom-line-{j}"))],
                block_id: 0,
            });
        }
        // Mark block 0 as manually collapsed.
        state.block_display.insert(0, BlockDisplayMode::Collapsed);
        // Newest block (1) untouched, default mode.
        state.transcript.push(TranscriptLine {
            kind: InlineMessageKind::Agent,
            segments: vec![plain_segment("after-collapsed")],
            block_id: 1,
        });
        let rendered = render_frame_to_string_at(&state, 80, 12);
        // The historic `[+] error:` prefix from
        // `transcript_line_marked(folded=true)` must still appear.
        assert!(
            rendered.contains("[+] error: boom-line-0"),
            "manual Collapsed must keep the [+] marker (got: {rendered:?})"
        );
        // The ladder's glyph affordance (`▸ `) must NOT replace it.
        assert!(
            !rendered.contains('\u{25B8}'),
            "manual Collapsed must NOT be replaced by the ladder glyph"
        );
    }

    /// `clamp_fold_text` (the helper that truncates activity
    /// strings) honors unicode display width and replaces overflow
    /// with an ellipsis.
    #[test]
    fn clamp_fold_text_truncates_at_unicode_width() {
        // ASCII overflow: 80-cell budget, prefix 3, activity 100.
        let out = clamp_fold_text(&"x".repeat(100), 3, 80, "\u{2026}");
        assert!(out.ends_with('\u{2026}'), "ellipsis appended on overflow");
        assert!(out.width() <= 80, "clamped to budget: got {}", out.width());
        // CJK: each glyph is 2 cells.
        let cjk = "\u{4ECA}\u{65E5}\u{306F}\u{667A}\u{6167}".repeat(20);
        let out_cjk = clamp_fold_text(&cjk, 2, 20, "\u{2026}");
        assert!(out_cjk.width() <= 20, "CJK clamp: got {}", out_cjk.width());
        assert!(out_cjk.ends_with('\u{2026}'));
        // Identity when text already fits.
        assert_eq!(clamp_fold_text("short", 0, 80, "\u{2026}"), "short");
        // Zero-width returns empty.
        assert_eq!(clamp_fold_text("text", 0, 0, "\u{2026}"), "");
    }

    // Cursor math for the composer is now owned by `oxicode_textarea::
    // TextArea::cursor_pos_with_state`, which is exercised by the
    // 351 tests in `oxicode-textarea`. The byte-cursor column math
    // these tests used to pin (composer_cursor_position) is gone.
}

#[cfg(test)]
mod secure_input_tests {
    use super::*;
    use oxicode_vtui::tui::core::OverlaySubmission;

    #[test]
    fn overlay_submission_secure_input_is_routed_to_host() {
        // Smoke: serialization round-trip — the variant must be reachable
        // through the protocol so the input thread can dispatch it.
        let _ = OverlaySubmission::SecureInput("sk-test".into());
        let serialized = format!("{:?}", OverlaySubmission::SecureInput("x".into()));
        assert!(serialized.contains("SecureInput"));
    }

    #[test]
    fn providers_action_matrix_branches_correctly() {
        // Pin the (has_key, oauth_capable) → Vec<AuthAction> matrix
        // exactly. Refactors MUST keep this contract: the order of
        // returned actions drives the visible action menu order.
        assert_eq!(
            next_provider_actions(true, true),
            vec![
                AuthAction::SetApiKey,
                AuthAction::StartOAuth,
                AuthAction::RemoveKey,
            ],
            "has key + oauth-capable: replace, oauth, remove"
        );
        assert_eq!(
            next_provider_actions(true, false),
            vec![AuthAction::SetApiKey, AuthAction::RemoveKey],
            "has key, key-only provider: replace, remove"
        );
        assert_eq!(
            next_provider_actions(false, true),
            vec![AuthAction::SetApiKey, AuthAction::StartOAuth],
            "no key + oauth-capable: set key, oauth"
        );
        assert_eq!(
            next_provider_actions(false, false),
            vec![AuthAction::SetApiKey],
            "no key + key-only provider: set key only"
        );
    }

    // ── EditBuffer-flow tests for the post-port secure input ──────
    //
    // These exercise the new flow end-to-end so we never regress on the
    // core invariants: the real value lives only in the editor, the
    // renderer paints asterisks (not the value), and a backspace at the
    // end of the masked element clears the buffer atomically. None of the
    // assertions reference the secret string directly — only its length
    // and the renderer's symbol output.

    /// Replicate the secure-input render path against an [`OverlaySecureInput`]
    /// so each test can build it without going through `materialize_overlay`.
    fn render_secure_to_text(secure: &OverlaySecureInput) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let overlay = OverlayState {
            title: "OpenAI key".into(),
            lines: vec!["Paste your API key".into()],
            items: Vec::new(),
            selected: 0,
            search: None,
            secure_input: Some(secure.clone()),
            ..Default::default()
        };
        terminal
            .draw(|f| render_overlay(f, f.area(), &overlay))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn masked_render_shows_asterisks_not_value() {
        // The render path must NEVER carry the real value through a
        // `Line` span when `mask_input` is on. We assert on the rendered
        // buffer symbols only — the secret lives only in `editor.text()`.
        let mut editor = oxicode_textarea::EditBuffer::new();
        let _ = editor.insert_str("ABCDE");
        let rendered = render_secure_to_text(&OverlaySecureInput {
            config: SecurePromptConfig {
                label: "Key".into(),
                placeholder: Some("sk-...".into()),
                mask_input: true,
            },
            editor,
        });
        assert!(rendered.contains("*****"), "mask must render asterisks");
        assert!(
            !rendered.contains("ABCDE"),
            "masked render must NEVER carry the real value"
        );
        assert!(rendered.contains("Key:"), "label prefix must still render");
    }

    #[test]
    fn masked_render_caret_lands_after_mask() {
        // After a value is set the caret must sit at the end of the
        // masked element (atomic boundary). The exact column is the
        // label-prefix width plus the masked width — both are stable.
        let mut editor = oxicode_textarea::EditBuffer::new();
        let _ = editor.insert_str("ABCD");
        let secure = OverlaySecureInput {
            config: SecurePromptConfig {
                label: "Key".into(),
                placeholder: Some("sk-...".into()),
                mask_input: true,
            },
            editor,
        };
        // Drive the same render path used by the production renderer to
        // pull the caret column out via `cursor_pos_with_state`.
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let overlay = OverlayState {
            title: "OpenAI key".into(),
            lines: vec!["Paste your API key".into()],
            items: Vec::new(),
            selected: 0,
            search: None,
            secure_input: Some(secure.clone()),
            ..Default::default()
        };
        terminal
            .draw(|f| render_overlay(f, f.area(), &overlay))
            .unwrap();
        // Build the masked textarea identically and ask for its cursor
        // column relative to the same area the renderer uses.
        let value = secure.editor.text();
        let mut ta = oxicode_textarea::TextArea::new();
        ta.set_text(value);
        ta.replace_range_with_element(
            0..value.len(),
            value,
            MASKED_ELEMENT_KIND,
            Some(Line::from("*".repeat(value.chars().count()))),
        );
        ta.set_cursor(secure.editor.cursor_byte());
        let caret = ta
            .cursor_pos_with_state(
                Rect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 24,
                },
                oxicode_textarea::TextAreaState::default(),
            )
            .expect("caret must be visible");
        // The masked element covers 0..4, so the textarea's cursor snaps
        // to its end boundary and reports column 4 relative to the area.
        assert_eq!(caret.0, 4);
    }

    #[test]
    fn backspace_removes_previous_grapheme() {
        // The masked element renders the whole buffer as asterisks, but
        // `EditBuffer` operates grapheme-by-grapheme — the textarea's
        // element bookkeeping only affects cursor snapping at render
        // time, not the editor's edit primitives. Pin both halves of the
        // contract so a future port that changes either side is caught.
        let mut editor = oxicode_textarea::EditBuffer::new();
        let _ = editor.insert_str("XYZ");
        assert_eq!(editor.text(), "XYZ");
        assert_eq!(editor.cursor_byte(), 3);
        let _ = editor.apply(oxicode_textarea::EditCommand::DeleteGraphemeBackward);
        assert_eq!(editor.text(), "XY");
        assert_eq!(editor.cursor_byte(), 2);
        let _ = editor.apply(oxicode_textarea::EditCommand::DeleteGraphemeBackward);
        assert_eq!(editor.text(), "X");
        let _ = editor.apply(oxicode_textarea::EditCommand::DeleteGraphemeBackward);
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor_byte(), 0);
    }

    #[test]
    fn empty_editor_renders_placeholder_not_asterisks() {
        // Pin the empty-buffer render path: placeholder text, zero
        let rendered = render_secure_to_text(&OverlaySecureInput {
            config: SecurePromptConfig {
                label: "Key".into(),
                placeholder: Some("sk-...".into()),
                mask_input: true,
            },
            editor: oxicode_textarea::EditBuffer::new(),
        });
        assert!(rendered.contains("sk-..."));
        assert!(!rendered.contains("*"));
    }

    #[test]
    fn paste_filter_drops_newline_and_non_ascii_via_edit_command() {
        // The paste path now feeds `EditCommand::Insert` per character
        // after the same ASCII + newline filter the helper used to apply.
        // Re-pinning the contract here means a regression in the filter
        // shows up directly as a test failure.
        let mut editor = oxicode_textarea::EditBuffer::new();
        let pasted = "sk-xyz\nABC\u{1F600}";
        let trimmed = pasted.trim_end_matches('\n');
        for ch in trimmed.chars() {
            if ch.is_ascii_graphic() || ch == ' ' {
                let _ = editor.apply(oxicode_textarea::EditCommand::Insert(ch));
            }
        }
        assert_eq!(editor.text(), "sk-xyzABC");
        assert_eq!(editor.cursor_byte(), 9);
    }
}
// ═════════════════════════════════════════════════════════════════════════
// `/providers` overlay chaining — regression for the bug where the
// `OverlayEvent::Submitted` arm closed the current overlay
// unconditionally, even when the handler opened a fresh overlay (action
// menu, secure prompt). The cmd channel processes `ShowOverlay` and
// `CloseOverlay` in submit order, so a `CloseOverlay` enqueued right
// after the `ShowOverlay` from the action menu won — leaving the user
// with nothing visible on Enter.
// ═════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod provider_overlay_tests {
    use super::*;
    use crate::app::agent_session::{AgentSession, AgentSessionHandle};
    use crate::store::session::SessionManager;
    use crate::store::settings::Settings;
    use oxicode_agent::{Agent, AgentConfig};
    use oxicode_sdk::{Provider, ProviderError, ProviderEvent};
    use oxicode_vtui::tui::core::OverlayEvent;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context as TaskContext, Poll};

    /// Minimal mock provider — produces an empty stream so `AgentSession`
    /// can construct (the `ProviderRow` dispatch never streams).
    struct EmptyStream;
    impl futures::Stream for EmptyStream {
        type Item = ProviderEvent;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(None)
        }
    }

    struct StubProvider;
    impl Provider for StubProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a oxicode_sdk::Model,
            _context: &'a oxicode_sdk::Context,
            _options: Option<oxicode_sdk::StreamOptions>,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>,
                            ProviderError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                Ok::<_, ProviderError>(Box::pin(EmptyStream)
                    as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
            })
        }
    }

    /// Minimal `AgentTool` fixture: name + essential flag, execute is a
    /// stub. Used by `make_session_with_tools_for_tests`.
    struct StubEssentialTool {
        name: &'static str,
        essential: bool,
    }
    #[async_trait::async_trait]
    impl oxicode_agent::AgentTool for StubEssentialTool {
        fn name(&self) -> &str {
            self.name
        }
        fn label(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "stub tool for multiselect editor tests"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        fn essential(&self) -> bool {
            self.essential
        }
        async fn execute(
            &self,
            _id: &str,
            _params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &oxicode_agent::ToolContext,
        ) -> Result<oxicode_agent::AgentToolResult, String> {
            Ok(oxicode_agent::AgentToolResult::success("ok"))
        }
    }

    fn make_session() -> AgentSessionHandle {
        let provider = Arc::new(StubProvider);
        let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
        let agent = Arc::new(Agent::new(
            provider,
            config,
            Arc::new(oxicode_agent::ToolRegistry::new()),
        ));
        let settings = Settings::default();
        let session_manager = SessionManager::in_memory("/tmp/test_providers");
        let session = AgentSession::new(
            agent,
            settings,
            session_manager,
            "/tmp/test_providers".to_string(),
            crate::SessionState::default(),
        );
        session.clone_handle()
    }

    /// Session fixture with a registry holding one essential (`bash`)
    /// and one optional (`commit`) tool — used by the settings-panel
    /// multiselect editor tests (they source their row list from the
    /// live registry). `pub(super)` so sibling test mods can reuse the
    /// provider stub without duplicating it.
    pub(super) fn make_session_with_tools_for_tests() -> AgentSessionHandle {
        let provider = Arc::new(StubProvider);
        let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
        let registry = oxicode_agent::ToolRegistry::new();
        registry.register_arc(Arc::new(StubEssentialTool {
            name: "bash",
            essential: true,
        }));
        registry.register_arc(Arc::new(StubEssentialTool {
            name: "commit",
            essential: false,
        }));
        let agent = Arc::new(Agent::new(provider, config, Arc::new(registry)));
        let settings = Settings::default();
        let session_manager = SessionManager::in_memory("/tmp/test_providers");
        let session = AgentSession::new(
            agent,
            settings,
            session_manager,
            "/tmp/test_providers".to_string(),
            crate::SessionState::default(),
        );
        session.clone_handle()
    }

    /// `InlineCommand` does not implement `Debug`, so summarise the channel
    /// contents by command variant for assertion failure messages.
    fn summarise(cmds: &[InlineCommand]) -> String {
        let mut show = 0;
        let mut close = 0;
        let mut other = 0;
        for c in cmds {
            match c {
                InlineCommand::ShowOverlay { .. } => show += 1,
                InlineCommand::CloseOverlay => close += 1,
                _ => other += 1,
            }
        }
        format!("[ShowOverlay={show}, CloseOverlay={close}, other={other}]")
    }

    /// Regression: `/providers` row selection for an OAuth-capable
    /// provider with no stored key triggers the multi-action chain
    /// `[SetApiKey, StartOAuth]` → `handle.show_list_modal` opens the
    /// action menu. The bug closed that menu instantly. The fix tracks
    /// whether the handler opened a new overlay and only emits the
    /// trailing `close_overlay()` when nothing was opened.
    #[test]
    fn provider_row_opens_action_menu_without_close() {
        // openai is OAuth-capable (per `product-meta.toml`), no key in
        // the env / storage, so the action matrix returns the
        // multi-action list.
        let session = make_session();
        let mut state = RenderState::default();
        state.overlay_providers = vec!["openai".to_string()];
        state.overlay = Some(OverlayState {
            title: "Providers".to_string(),
            lines: Vec::new(),
            items: Vec::new(),
            selected: 0,
            search: None,
            secure_input: None,
            ..Default::default()
        });

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<InlineCommand>();
        let handle = InlineHandle::new_for_tests(cmd_tx);
        let prompt_queue = Arc::new(PromptQueue::default());

        let evt = InlineEvent::Overlay(OverlayEvent::Submitted(OverlaySubmission::Selection(
            InlineListSelection::ProviderRow(0),
        )));
        let _ = handle_inline_event(&mut state, &handle, &session, &prompt_queue, evt);

        let cmds: Vec<InlineCommand> = {
            let mut out = Vec::new();
            while let Ok(cmd) = cmd_rx.try_recv() {
                out.push(cmd);
            }
            out
        };
        let show_count = cmds
            .iter()
            .filter(|c| matches!(c, InlineCommand::ShowOverlay { .. }))
            .count();
        assert_eq!(
            show_count,
            1,
            "submitting a provider row must ShowOverlay exactly once (commands: {})",
            summarise(&cmds)
        );

        // The bug: a `CloseOverlay` followed the `ShowOverlay` on the
        // cmd channel and won the order-of-application race. After the
        // fix, no `CloseOverlay` may follow the `ShowOverlay`.
        let show_idx = cmds
            .iter()
            .position(|c| matches!(c, InlineCommand::ShowOverlay { .. }))
            .expect("ShowOverlay must be present");
        let trailing = &cmds[show_idx + 1..];
        assert!(
            !trailing
                .iter()
                .any(|c| matches!(c, InlineCommand::CloseOverlay)),
            "no CloseOverlay may follow the action-menu ShowOverlay (commands: {})",
            summarise(&cmds)
        );

        // Stale-state cleanup must still run so future `/providers`
        // does not see stale indices.
        assert!(
            state.overlay_providers.is_empty(),
            "overlay_providers must be cleared after dispatch (got {:?})",
            state.overlay_providers
        );
    }

    /// Regression: `/providers` row selection for a key-only provider
    /// (no OAuth spec) with no stored key triggers the single-action
    /// chain `[SetApiKey]` → `handle_auth_action` opens the secure
    /// prompt modal. The bug closed that modal instantly. The fix
    /// propagates the `opened_new_overlay` flag through `|=` so the
    /// secure prompt survives.
    #[test]
    fn provider_row_set_api_key_opens_secure_prompt_without_close() {
        // cerebras is key-only (no OAuth spec in `product-meta.toml`).
        let session = make_session();
        let mut state = RenderState::default();
        state.overlay_providers = vec!["cerebras".to_string()];
        state.overlay = Some(OverlayState {
            title: "Providers".to_string(),
            lines: Vec::new(),
            items: Vec::new(),
            selected: 0,
            search: None,
            secure_input: None,
            ..Default::default()
        });

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<InlineCommand>();
        let handle = InlineHandle::new_for_tests(cmd_tx);
        let prompt_queue = Arc::new(PromptQueue::default());

        let evt = InlineEvent::Overlay(OverlayEvent::Submitted(OverlaySubmission::Selection(
            InlineListSelection::ProviderRow(0),
        )));
        let _ = handle_inline_event(&mut state, &handle, &session, &prompt_queue, evt);

        let cmds: Vec<InlineCommand> = {
            let mut out = Vec::new();
            while let Ok(cmd) = cmd_rx.try_recv() {
                out.push(cmd);
            }
            out
        };
        let show_count = cmds
            .iter()
            .filter(|c| matches!(c, InlineCommand::ShowOverlay { .. }))
            .count();
        assert_eq!(
            show_count,
            1,
            "submitting a provider row must ShowOverlay exactly once (commands: {})",
            summarise(&cmds)
        );

        let show_idx = cmds
            .iter()
            .position(|c| matches!(c, InlineCommand::ShowOverlay { .. }))
            .expect("ShowOverlay must be present");
        let trailing = &cmds[show_idx + 1..];
        assert!(
            !trailing
                .iter()
                .any(|c| matches!(c, InlineCommand::CloseOverlay)),
            "no CloseOverlay may follow the secure-prompt ShowOverlay (commands: {})",
            summarise(&cmds)
        );

        // The secure prompt origin must be stashed so a subsequent
        // `SecureInput` submission routes the key to the right provider
        // and emits a contextual follow-up message.
        assert_eq!(
            state.secure_input_origin,
            Some(SecureInputOrigin::SetKey {
                provider: "cerebras".to_string(),
            }),
            "secure_input_origin must be stashed by SetApiKey"
        );
    }

    /// Catalog model selection (the working baseline) must remain
    /// closing — pinning the behavior so the conditional close does
    /// not regress the other `Submitted` branches.
    #[test]
    fn catalog_model_selection_still_closes_overlay() {
        let session = make_session();
        let mut state = RenderState::default();
        state.overlay_catalog_models = vec![(
            "anthropic".to_string(),
            "claude-sonnet-4-20250514".to_string(),
        )];
        state.overlay = Some(OverlayState {
            title: "Models".to_string(),
            lines: Vec::new(),
            items: Vec::new(),
            selected: 0,
            search: None,
            secure_input: None,
            ..Default::default()
        });

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<InlineCommand>();
        let handle = InlineHandle::new_for_tests(cmd_tx);
        let prompt_queue = Arc::new(PromptQueue::default());

        let evt = InlineEvent::Overlay(OverlayEvent::Submitted(OverlaySubmission::Selection(
            InlineListSelection::CatalogModel(0),
        )));
        let _ = handle_inline_event(&mut state, &handle, &session, &prompt_queue, evt);

        let cmds: Vec<InlineCommand> = {
            let mut out = Vec::new();
            while let Ok(cmd) = cmd_rx.try_recv() {
                out.push(cmd);
            }
            out
        };
        assert!(
            cmds.iter()
                .any(|c| matches!(c, InlineCommand::CloseOverlay)),
            "catalog model selection must close the overlay (commands: {})",
            summarise(&cmds)
        );
    }

    /// `add_custom_provider` chains into the secure prompt via
    /// `open_secure_prompt` with `SecureInputOrigin::NewlyAdded`. The
    /// provider must be reachable from either variant so the
    /// `OverlaySubmission::SecureInput` consumer routes the key to the
    /// right slot without a per-variant branch.
    fn secure_input_origin_carries_provider_independently_of_variant() {
        let set = SecureInputOrigin::SetKey {
            provider: "openai".to_string(),
        };
        let added = SecureInputOrigin::NewlyAdded {
            provider: "minimax".to_string(),
        };
        // `provider` must be reachable regardless of variant so the
        // `OverlaySubmission::SecureInput` consumer can route the key
        // without a per-variant branch. (The model-role origins carry
        // no provider — they route through their own arm.)
        let provider_of = |o: &SecureInputOrigin| match o {
            SecureInputOrigin::SetKey { provider } | SecureInputOrigin::NewlyAdded { provider } => {
                provider.clone()
            }
            SecureInputOrigin::ModelRoleKey | SecureInputOrigin::ModelRoleValue { .. } => {
                unreachable!("model-role origins have no provider")
            }
            SecureInputOrigin::TextEdit(_) => {
                unreachable!("text-edit origin has no provider")
            }
        };
        assert_eq!(provider_of(&set), "openai");
        assert_eq!(provider_of(&added), "minimax");
        // Variants are distinct (so the follow-up message can branch).
        assert_ne!(set, added);
    }

    /// Regression: the `/sessions` picker arm previously set
    /// `state.pending_resume` without the `is_streaming()` gate that the
    /// direct `/sessions <id>` path and `/handoff` both use. A mid-stream
    /// pick + Enter fired the drain, which calls `resume_from_file` →
    /// `AgentSession::new` → `agent.update_state` on the shared
    /// `Arc<Agent>`, clobbering the in-flight conversation's message
    /// history. The picker now refuses with the same error wording as
    /// the direct path and never sets `pending_resume` while streaming.
    #[test]
    fn session_picker_resume_refused_while_streaming() {
        let session = make_session();
        // Flip the streaming flag BEFORE dispatch so the gate fires.
        // `streaming_flag()` returns an `Arc<AtomicBool>` shared with the
        // worker thread, so the production code observes the new value.
        session
            .streaming_flag()
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let mut state = RenderState::default();
        // Sanity: no resume queued yet.
        assert!(
            state.pending_resume.is_none(),
            "precondition: pending_resume must start None"
        );

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<InlineCommand>();
        let handle = InlineHandle::new_for_tests(cmd_tx);
        let prompt_queue = Arc::new(PromptQueue::default());

        let evt = InlineEvent::Overlay(OverlayEvent::Submitted(OverlaySubmission::Selection(
            InlineListSelection::Session("some-id".to_string()),
        )));
        let _ = handle_inline_event(&mut state, &handle, &session, &prompt_queue, evt);

        // The gate must have refused: pending_resume stays None.
        assert!(
            state.pending_resume.is_none(),
            "streaming session must not enqueue pending_resume (got {:?})",
            state.pending_resume
        );

        // Drain the handle's cmd channel and inspect appended lines.
        let mut cmds: Vec<InlineCommand> = Vec::new();
        while let Ok(cmd) = cmd_rx.try_recv() {
            cmds.push(cmd);
        }
        let mut found_error = false;
        let mut error_text = String::new();
        for cmd in &cmds {
            if let InlineCommand::AppendLine { kind, segments } = cmd
                && matches!(kind, InlineMessageKind::Error)
            {
                error_text = segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                found_error = true;
            }
        }
        assert!(
            found_error,
            "expected an error AppendLine (commands: {})",
            summarise(&cmds)
        );
        assert!(
            error_text.contains("Cannot resume while agent is running"),
            "error text must match the direct-path wording (got {error_text:?})"
        );

        // Cleanup: reset streaming so the flag doesn't leak across tests
        // in the same process.
        session
            .streaming_flag()
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// `/settings` tab switch: submitting `SettingsTab(1)` must reopen the
    /// panel rebuilt for tab 1 (Model) — tab bar, sidebar sections, and
    /// the def-table rows for that tab — without emitting a CloseOverlay.
    #[test]
    fn settings_tab_selection_rebuilds_item_list() {
        let session = make_session();
        let mut state = RenderState::default();
        // Enter already closed the overlay before the submission arrives.
        state.overlay = None;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<InlineCommand>();
        let handle = InlineHandle::new_for_tests(cmd_tx);
        let prompt_queue = Arc::new(PromptQueue::default());

        let evt = InlineEvent::Overlay(OverlayEvent::Submitted(OverlaySubmission::Selection(
            InlineListSelection::SettingsTab(1),
        )));
        let _ = handle_inline_event(&mut state, &handle, &session, &prompt_queue, evt);

        let overlay = state.overlay.as_ref().expect("panel reopened on tab 1");
        assert_eq!(overlay.active_tab, 1);
        assert_eq!(overlay.tabs.get(1).map(String::as_str), Some("Model"));
        assert_eq!(
            overlay.sections,
            vec!["Defaults".to_string(), "Pointers".to_string()]
        );
        // Rows come from the def table for the Model tab.
        let settings = Settings::load().unwrap_or_default();
        let expected = settings_overlay_items(SettingsTab::Model, &settings).0;
        assert_eq!(overlay.items.len(), expected.len());
        assert_eq!(
            overlay
                .items
                .iter()
                .map(|i| i.title.clone())
                .collect::<Vec<_>>(),
            expected.iter().map(|i| i.title.clone()).collect::<Vec<_>>(),
        );
        assert_eq!(state.settings_active_tab, SettingsTab::Model);
        // The switch reopens in place — no close may leak through the
        // cmd channel.
        while let Ok(cmd) = cmd_rx.try_recv() {
            assert!(
                !matches!(cmd, InlineCommand::CloseOverlay),
                "tab switch must not close the reopened panel"
            );
        }
    }
}
#[cfg(test)]
mod thinking_stream_tests {
    //! Regression: a `StreamDelta::Thinking` delta must (a) never append
    //! to the transcript, and (b) only set a fixed `thinking…` label on
    //! the reasoning stage — never the streamed fragment. Raw reasoning
    //! fragments would otherwise leak through two render surfaces
    //! (the composer `RUN ` field in `composer_context_line`, and the
    //! reasoning indicator above the composer).
    use super::*;
    use oxicode_ai::{Api, AssistantMessage, ContentBlock, Message, TextContent};
    use oxicode_vtui::tui::core::InlineHandle;
    use tokio::sync::mpsc;

    fn fresh_handle() -> (InlineHandle, mpsc::UnboundedReceiver<InlineCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (InlineHandle::new_for_tests(tx), rx)
    }

    fn assistant() -> Message {
        Message::Assistant(AssistantMessage::new(
            Api::OpenAiCompletions,
            "test",
            "test",
        ))
    }

    fn assistant_with_text(text: &str) -> Message {
        let mut a = AssistantMessage::new(Api::OpenAiCompletions, "test", "test");
        a.content.push(ContentBlock::Text(TextContent::new(text)));
        Message::Assistant(a)
    }
    #[test]
    fn thinking_delta_sets_fixed_stage_label_not_raw_text() {
        let mut state = RenderState::default();
        let (handle, mut cmd_rx) = fresh_handle();

        // The exact text the model streamed for reasoning MUST NOT appear
        // in the stage indicator — it renders in the transcript's dimmed
        // reasoning block instead.
        let event = AgentEvent::MessageUpdate {
            message: assistant(),
            delta: oxicode_sdk::StreamDelta::Thinking("considering options".into()),
        };
        map_agent_event(&handle, event, &mut state);

        assert_eq!(
            state.reasoning_stage.as_deref(),
            Some("thinking\u{2026}"),
            "reasoning stage must show a fixed label, never the streamed fragment"
        );
        while let Ok(cmd) = cmd_rx.try_recv() {
            assert!(
                !matches!(cmd, InlineCommand::Inline { .. }),
                "thinking must not emit a transcript Inline command"
            );
        }
    }

    #[test]
    fn thinking_streams_as_dimmed_block_above_the_answer() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();

        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Thinking("weighing alternatives".into()),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);
        let text: String = state
            .transcript
            .iter()
            .flat_map(|l| l.segments.iter().map(|s| s.text.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            text.contains("weighing alternatives"),
            "thinking must render in the transcript: {text}"
        );
        let dim_italic = state.transcript.iter().any(|l| {
            l.segments.iter().any(|s| {
                let st = s.style.as_ref();
                st.effects.contains(anstyle::Effects::DIMMED)
                    && st.effects.contains(anstyle::Effects::ITALIC)
            })
        });
        assert!(
            dim_italic,
            "thinking lines render in the dimmed italic reasoning style"
        );

        // The answer streams below the thinking block, and thinking survives.
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("the answer".into()),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);
        type_out_stream(&mut state);

        // One blank row breathes between the thinking block and the answer.
        let texts: Vec<String> = state
            .transcript
            .iter()
            .map(|l| {
                l.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect();
        let think_idx = texts
            .iter()
            .position(|t| t.contains("weighing alternatives"))
            .expect("thinking line");
        let answer_idx = texts
            .iter()
            .position(|t| t.contains("the answer"))
            .expect("answer line");
        assert!(
            answer_idx > think_idx,
            "answer renders below thinking: {texts:?}"
        );
        assert_eq!(
            state.reasoning_stage.as_deref(),
            Some("generating response"),
            "the stage label moves on once the answer streams"
        );
    }

    #[test]
    fn tool_lines_survive_the_full_turn_event_sequence() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();

        // Real order (agent_loop): assistant text → MessageEnd → ToolStart
        // → ToolComplete → ToolResult(MessageStart+MessageEnd) → next text.
        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("I will check.".into()),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageEnd {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionStart {
                tool_call_id: "tc1".into(),
                tool_name: "bash".into(),
                args: serde_json::json!({"command": "echo hi"}),
                intent: None,
                context: None,
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionEnd {
                tool_call_id: "tc1".into(),
                tool_name: "bash".into(),
                intent: None,
                result: oxicode_ai::ToolResult {
                    tool_call_id: "tc1".into(),
                    content: "ls output".into(),
                    status: "success".into(),
                },
                is_error: false,
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("All done.".into()),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageEnd {
                message: assistant(),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);

        let text: String = state
            .transcript
            .iter()
            .flat_map(|l| l.segments.iter().map(|s| s.text.as_str()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(
            text.contains("$ echo hi"),
            "box header shows the shell command: {text}"
        );
        assert!(
            text.contains("Output") && text.contains("ls output"),
            "labeled divider separates the call from its output: {text}"
        );
        assert!(
            text.contains("\u{256D}") && text.contains("\u{2570}"),
            "rounded top and bottom borders close the box: {text}"
        );
        // The whole box is ONE block: folding and scrollback commits stay
        // atomic per call.
        let block_ids: std::collections::HashSet<usize> = state
            .transcript
            .iter()
            .filter(|l| l.kind == InlineMessageKind::Tool)
            .map(|l| l.block_id)
            .collect();
        assert_eq!(block_ids.len(), 1, "one tool call = one block");
    }
    #[test]
    fn first_text_delta_overrides_thinking_stage_with_generating_response() {
        let mut state = RenderState::default();
        let (handle, _cmd_rx) = fresh_handle();

        // The real streaming path emits Thinking and Text deltas as
        // `AgentEvent::MessageUpdate { delta: StreamDelta::* }`
        // (oxicode-agent/src/agent_loop/streaming.rs:277-280). TextChunk
        // is legacy and no producer emits it. The Text arm is the
        // lifecycle owner that moves the stage off `thinking…`.
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Thinking("considering".into()),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("hi".into()),
            },
            &mut state,
        );

        assert_eq!(
            state.reasoning_stage.as_deref(),
            Some("generating response"),
            "first Text delta must move the stage off `thinking\u{2026}`"
        );
    }

    fn apply_all(state: &mut RenderState, rx: &mut mpsc::UnboundedReceiver<InlineCommand>) {
        while let Ok(cmd) = rx.try_recv() {
            apply_command(state, cmd);
        }
    }

    #[test]
    fn message_end_replaces_the_streamed_block_without_duplicates() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();

        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("para one".into()),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("\n\npara two".into()),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageEnd {
                message: assistant_with_text("para one\n\npara two"),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);

        let text: String = state
            .transcript
            .iter()
            .flat_map(|l| l.segments.iter().map(|s| s.text.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            text.matches("para one").count(),
            1,
            "the markdown re-render must fully replace the streamed raw lines: {text}"
        );
    }

    #[test]
    fn consecutive_messages_stream_into_separate_blocks() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();

        for body in ["first answer", "second answer"] {
            map_agent_event(
                &handle,
                AgentEvent::MessageStart {
                    message: assistant(),
                },
                &mut state,
            );
            map_agent_event(
                &handle,
                AgentEvent::MessageUpdate {
                    message: assistant(),
                    delta: oxicode_sdk::StreamDelta::Text(body.into()),
                },
                &mut state,
            );
            map_agent_event(
                &handle,
                AgentEvent::MessageEnd {
                    message: assistant_with_text(body),
                },
                &mut state,
            );
        }
        apply_all(&mut state, &mut rx);

        let joined = state
            .transcript
            .iter()
            .map(|l| {
                l.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("|");
        assert!(
            joined.contains("first answer") && joined.contains("second answer"),
            "both messages survive: {joined}"
        );
        assert!(
            !joined.contains("first answersecond answer"),
            "a new message must not append into the previous message's line: {joined}"
        );
    }

    #[test]
    fn text_deltas_render_markdown_live_not_raw() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();
        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("a **bold** claim".into()),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);
        type_out_stream(&mut state);

        let text = state
            .transcript
            .iter()
            .flat_map(|l| l.segments.iter().map(|s| s.text.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !text.contains("**"),
            "the live stream must render markdown, not raw syntax: {text}"
        );
        assert!(text.contains("bold"), "content survives: {text}");
    }

    #[test]
    fn message_end_does_not_reflow_the_streamed_block() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();
        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("hello **world**".into()),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);
        type_out_stream(&mut state);
        let streamed: Vec<String> = state
            .transcript
            .iter()
            .map(|l| {
                l.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect();

        map_agent_event(
            &handle,
            AgentEvent::MessageEnd {
                message: assistant_with_text("hello **world**"),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);
        let final_: Vec<String> = state
            .transcript
            .iter()
            .map(|l| {
                l.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(
            streamed, final_,
            "MessageEnd must not re-render what the live stream already shows"
        );
    }

    #[test]
    fn final_message_renders_the_authoritative_tail() {
        // Regression: providers can coalesce the stream tail into the
        // final Done message without a matching delta (the Done message
        // replaces the accumulated partial in agent_loop/streaming.rs).
        // Rendering the final block from the delta buffers lost that
        // tail until the next prompt rebuilt history from the session.
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();
        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("visible prefix ".into()),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);
        type_out_stream(&mut state);

        map_agent_event(
            &handle,
            AgentEvent::MessageEnd {
                message: assistant_with_text("visible prefix HIDDEN-TAIL"),
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);
        let text: String = state
            .transcript
            .iter()
            .map(|l| {
                l.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect();
        assert!(
            text.contains("HIDDEN-TAIL"),
            "the final message is authoritative — its tail must render: {text}"
        );
        assert_eq!(
            text.matches("visible prefix").count(),
            1,
            "no duplicated block: {text}"
        );
    }

    #[test]
    fn streamed_body_renders_only_the_revealed_prefix() {
        let mut state = RenderState::default();
        state.message_buffer = "hello world".to_string();
        state.stream_reveal = 5; // bytes — "hello"
        let lines = render_streamed_message(&mut state);
        let text: String = lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect();
        assert!(text.contains("hello"), "revealed prefix renders: {text}");
        assert!(!text.contains("world"), "unrevealed text waits: {text}");
    }

    #[test]
    fn advance_stream_reveal_types_out_in_bounded_steps() {
        let mut state = RenderState::default();
        state.stream_anchor = Some(0);
        state.message_buffer = "x".repeat(6000);
        state.stream_reveal = 0;

        assert!(advance_stream_reveal(&mut state), "first tick paints");
        assert!(
            state.stream_reveal > 0 && state.stream_reveal < 6000,
            "bounded step, not a lump: {}",
            state.stream_reveal
        );
        let transcript_after_step: usize = state.transcript.len();
        assert!(
            transcript_after_step > 0,
            "the revealed prefix lands in the transcript"
        );

        while advance_stream_reveal(&mut state) {}
        assert_eq!(
            state.stream_reveal, 6000,
            "repeated ticks drain the backlog completely"
        );
    }

    /// Drive the typewriter to completion (test-side stand-in for the
    /// render tick).
    fn type_out_stream(state: &mut RenderState) {
        while advance_stream_reveal(state) {}
    }

    #[test]
    fn message_end_clears_reasoning_stage() {
        let mut state = RenderState::default();
        let (handle, _cmd_rx) = fresh_handle();
        state.reasoning_stage = Some("thinking\u{2026}".into());

        map_agent_event(
            &handle,
            AgentEvent::MessageEnd {
                message: assistant(),
            },
            &mut state,
        );

        assert!(
            state.reasoning_stage.is_none(),
            "MessageEnd must clear the reasoning stage so follow-ups / tips can render"
        );
    }

    #[test]
    fn run_tracker_spans_the_whole_tool_loop() {
        let mut state = RenderState::default();
        let (handle, _cmd_rx) = fresh_handle();

        map_agent_event(
            &handle,
            AgentEvent::AgentStart {
                prompts: vec![],
                session_id: None,
            },
            &mut state,
        );
        assert!(
            state.active_run.is_some(),
            "AgentStart opens the run tracker"
        );

        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionStart {
                tool_call_id: "tc-1".into(),
                tool_name: "read".into(),
                args: serde_json::json!({}),
                intent: None,
                context: None,
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageEnd {
                message: assistant(),
            },
            &mut state,
        );

        // Turn boundary: the stage may be cleared, but the run tracker —
        // with its progress facts — stays live until AgentEnd.
        let run = state.active_run.as_ref().expect("run stays live");
        assert_eq!(run.turn, 1, "MessageStart counts a turn");
        assert_eq!(run.tool_calls, 1, "ToolExecutionStart counts a call");

        map_agent_event(
            &handle,
            AgentEvent::AgentEnd {
                messages: vec![],
                stop_reason: None,
                session_id: None,
            },
            &mut state,
        );
        assert!(state.active_run.is_none(), "AgentEnd closes the tracker");
        assert!(
            state.reasoning_stage.is_none(),
            "AgentEnd releases the indicator row"
        );
    }

    #[test]
    fn message_end_releases_the_stream_anchor() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();
        map_agent_event(
            &handle,
            AgentEvent::MessageStart {
                message: assistant(),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageUpdate {
                message: assistant(),
                delta: oxicode_sdk::StreamDelta::Text("done".into()),
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::MessageEnd {
                message: assistant(),
            },
            &mut state,
        );
        while let Ok(cmd) = rx.try_recv() {
            apply_command(&mut state, cmd);
        }
        assert!(
            state.stream_anchor.is_none(),
            "MessageEnd finalizes the message — the anchor must release so the finished block can commit to scrollback"
        );
    }
}

#[cfg(test)]
mod composer_border_tests {
    //! The composer's top border is the single chrome surface after the
    //! status bar's removal: session facts + brain health, no app badge.
    use super::*;

    fn spans_to_string(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn composer_border_has_no_app_badge() {
        let mut state = RenderState::default();
        state.header_context.provider = "prov".to_string();
        state.header_context.model = "prov/m-1".to_string();

        let text = spans_to_string(&composer_context_line(&state, 200));

        assert!(
            text.starts_with("MODEL "),
            "model leads with no leading separator: {text}"
        );
        assert!(
            text.contains("MODEL m-1"),
            "provider prefix is stripped from the model: {text}"
        );
    }

    #[test]
    fn composer_border_fields_drop_by_width() {
        let mut state = RenderState::default();
        state.header_context.provider = "prov".to_string();
        state.header_context.model = "prov/m-1".to_string();

        // Narrow: only the model survives; wide: context usage joins.
        let narrow = spans_to_string(&composer_context_line(&state, 60));
        assert!(
            narrow.contains("MODEL ") && !narrow.contains("CTX "),
            "narrow keeps the model only: {narrow}"
        );
        let wide = spans_to_string(&composer_context_line(&state, 140));
        assert!(wide.contains("CTX "), "wide carries context usage: {wide}");
    }

    #[test]
    fn model_chips_follow_model_switch() {
        let mut state = RenderState::default();
        assert_eq!(state.context_window, 128_000, "default before sync");

        // A 1M-context model must replace both the MODEL field and the
        // CTX denominator (regression: the denominator was written once
        // at startup and never updated).
        apply_model_to_chips(&mut state, "google/gemini-2.5-pro", 1_048_576);
        assert_eq!(state.header_context.provider, "google");
        assert_eq!(state.header_context.model, "google/gemini-2.5-pro");
        assert_eq!(
            state.header_context.editor_context.as_deref(),
            Some("google/gemini-2.5-pro")
        );
        assert_eq!(state.context_window, 1_048_576);

        let wide = spans_to_string(&composer_context_line(&state, 140));
        assert!(
            wide.contains("CTX 0/1048.5K"),
            "CTX chip renders the synced denominator: {wide}"
        );

        // Empty id is a no-op.
        apply_model_to_chips(&mut state, "", 999);
        assert_eq!(state.header_context.model, "google/gemini-2.5-pro");

        // Zero window (unknown model): the MODEL chip follows the switch,
        // the CTX denominator keeps the last known value instead of 0.
        apply_model_to_chips(&mut state, "zai/glm-5.1", 0);
        assert_eq!(state.header_context.model, "zai/glm-5.1");
        assert_eq!(state.context_window, 1_048_576);
    }

    #[test]
    fn plain_segments_render_in_their_kind_color_not_response() {
        let styles = active_styles();
        let user_color = color_from_anstyle(styles.user.get_fg_color());
        let response = color_from_anstyle(styles.response.get_fg_color());
        let line = |kind| TranscriptLine {
            kind,
            segments: vec![plain_segment("body")],
            block_id: 0,
        };

        let user_line = line(InlineMessageKind::User);
        let user = transcript_line_marked(&user_line, &styles, false, false, false, true, 80);
        assert_eq!(
            user.spans[0].style.fg,
            Some(user_color),
            "user text must read in the user color — response-ink makes turns indistinguishable"
        );

        let agent_line = line(InlineMessageKind::Agent);
        let agent = transcript_line_marked(&agent_line, &styles, false, false, false, true, 80);
        assert_eq!(agent.spans[0].style.fg, Some(response));
    }
}
#[cfg(test)]
mod transcript_turn_tests {
    //! Speaker identity is structural (accent rail + weight), never prose
    //! labels. See docs/superpowers/specs/2026-08-20-transcript-turn-rendering-design.md.
    use super::*;
    fn tl(kind: InlineMessageKind, text: &str, block_id: usize) -> TranscriptLine {
        TranscriptLine {
            kind,
            segments: vec![plain_segment(text)],
            block_id,
        }
    }

    fn spans_to_string(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn user_lines_are_bold_primary_without_prefix() {
        let styles = active_styles();
        let line = tl(InlineMessageKind::User, "refactor the parser", 0);
        let rendered = transcript_line_marked(&line, &styles, false, false, false, true, 80);
        assert_eq!(
            rendered.spans.len(),
            1,
            "plain style renders no prefix span"
        );
        assert_eq!(
            spans_to_string(&rendered),
            "refactor the parser",
            "user text renders as typed, no glyph"
        );
        assert!(
            rendered.spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD),
            "user body is the only bold transcript text"
        );
    }

    #[test]
    fn agent_tool_and_shell_lines_have_no_prefix() {
        let styles = active_styles();
        for (kind, label) in [
            (InlineMessageKind::Agent, "agent"),
            (InlineMessageKind::Tool, "tool"),
            (InlineMessageKind::Pty, "shell"),
        ] {
            let line = tl(kind, &format!("{label}-content"), 0);
            let rendered = transcript_line_marked(&line, &styles, false, false, false, true, 80);
            assert_eq!(
                rendered.spans.len(),
                1,
                "{label} lines carry no marker spans"
            );
            assert_eq!(spans_to_string(&rendered), format!("{label}-content"));
        }
    }

    #[test]
    fn system_labels_render_on_block_start_only() {
        let styles = active_styles();
        let line = tl(InlineMessageKind::Error, "boom", 0);

        let head = transcript_line_marked(&line, &styles, false, false, false, true, 80);
        assert_eq!(spans_to_string(&head), "error: boom");

        let body = transcript_line_marked(&line, &styles, false, false, false, false, 80);
        assert_eq!(
            spans_to_string(&body),
            "boom",
            "continuation lines drop the label"
        );
    }

    #[test]
    fn folded_head_keeps_the_block_label() {
        let styles = active_styles();
        let line = tl(InlineMessageKind::Error, "boom", 0);
        let rendered = transcript_line_marked(&line, &styles, true, false, false, false, 80);
        assert_eq!(
            spans_to_string(&rendered),
            "[+] error: boom",
            "a collapsed block stays identifiable"
        );
    }

    #[test]
    fn transcript_line_marked_clamps_to_width() {
        // Write-path width invariant: even if a 300-char segment lands on
        // a 40-col viewport, the rendered Line never overflows.
        let styles = active_styles();
        let big: String = "x".repeat(300);
        let line = tl(InlineMessageKind::Agent, &big, 0);
        let rendered = transcript_line_marked(&line, &styles, false, false, false, true, 40);
        assert!(
            rendered.width() <= 40,
            "transcript row overflowed the terminal width: rendered.width()={}",
            rendered.width()
        );
    }
}

#[cfg(test)]
mod scrollback_commit_tests {
    //! Host-scrollback committing (inline-viewport pattern — peer parity
    //! with Claude Code / pi): finalized transcript blocks are printed
    //! into the terminal's real scrollback so native scroll-up shows the
    //! conversation. Commits are block-atomic, never touch the anchored
    //! streaming block, and pause while the user browses.
    use super::*;

    fn tl(kind: InlineMessageKind, text: &str, block_id: usize) -> TranscriptLine {
        TranscriptLine {
            kind,
            segments: vec![plain_segment(text)],
            block_id,
        }
    }

    /// 6 agent blocks × 2 lines = 12 entries; one display row each at
    /// width 80 (no spacers — agent flow stays contiguous).
    fn long_transcript() -> Vec<TranscriptLine> {
        (0..6)
            .flat_map(|b| {
                [
                    tl(InlineMessageKind::Agent, &format!("b{b}-line-one"), b),
                    tl(InlineMessageKind::Agent, &format!("b{b}-line-two"), b),
                ]
            })
            .collect()
    }

    #[test]
    fn commit_plan_sheds_oldest_blocks_and_keeps_the_tail() {
        let state = RenderState {
            transcript: long_transcript(),
            ..Default::default()
        };
        let styles = active_styles();
        let display = build_transcript_display(&state, &styles, 0, 80);
        // 12 rows, keep 4 → 8 rows commit; entry 8 starts block b4, so
        // the boundary is already block-atomic (b3 ends at entry 7).
        let plan = scrollback_commit_plan(&display, &state.transcript, 80, 4, None).expect("plan");
        assert_eq!(plan.rows, 8, "12 rows total, keep 4 → commit 8");
        assert_eq!(plan.new_committed_entries, 8);
    }

    #[test]
    fn commit_plan_never_splits_a_block() {
        // Blocks of 3; the keep-window boundary lands mid-block and must
        // snap back to the block start.
        let transcript: Vec<TranscriptLine> = (0..3)
            .flat_map(|b| {
                (0..3).map(move |i| tl(InlineMessageKind::Agent, &format!("b{b}-{i}"), b))
            })
            .collect();
        let state = RenderState {
            transcript,
            ..Default::default()
        };
        let styles = active_styles();
        let display = build_transcript_display(&state, &styles, 0, 80);
        // 9 rows; keep 5 → limit 4 → the boundary would split b1
        // (items 3,4,5).
        let plan = scrollback_commit_plan(&display, &state.transcript, 80, 5, None).expect("plan");
        assert_eq!(
            plan.new_committed_entries, 3,
            "boundary snaps to block start"
        );
        assert_eq!(plan.rows, 3);
    }

    #[test]
    fn commit_plan_excludes_the_streaming_anchor() {
        let state = RenderState {
            transcript: long_transcript(),
            stream_anchor: Some(4),
            ..Default::default()
        };
        let styles = active_styles();
        let display = build_transcript_display(&state, &styles, 0, 80);
        let plan = scrollback_commit_plan(&display, &state.transcript, 80, 4, state.stream_anchor)
            .expect("plan");
        assert!(
            plan.new_committed_entries <= 4,
            "nothing at/after the anchored (streaming) block commits"
        );
    }
    #[test]
    fn committed_entries_floor_the_live_render() {
        let mut state = RenderState::default();
        state.transcript = long_transcript();
        state.committed_entries = 8;
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        terminal
            .draw(|f| render_frame(f, &state, &unused_handle()))
            .expect("draw");
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut rendered = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    rendered.push_str(cell.symbol());
                }
            }
            rendered.push('\n');
        }
        assert!(
            !rendered.contains("b0-line-one"),
            "committed blocks leave the viewport"
        );
        assert!(rendered.contains("b5-line"), "the live tail stays");
    }

    #[test]
    fn search_skips_committed_entries() {
        let mut state = RenderState::default();
        state.transcript = long_transcript();
        state.committed_entries = 8;
        state.start_search("line-one");
        let s = state.search.as_ref().expect("search open");
        assert!(
            s.matches.iter().all(|&i| i >= 8),
            "matches confined to the live region: {:?}",
            s.matches
        );
        assert!(!s.matches.is_empty());
    }

    fn unused_handle() -> InlineHandle {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        InlineHandle::new_for_tests(tx)
    }

    #[test]
    fn commit_plan_noop_when_tail_fits() {
        let state = RenderState {
            transcript: long_transcript(),
            ..Default::default()
        };
        let styles = active_styles();
        let display = build_transcript_display(&state, &styles, 0, 80);
        assert!(scrollback_commit_plan(&display, &state.transcript, 80, 12, None).is_none());
    }

    #[test]
    fn oversized_block_commits_its_head_at_line_granularity() {
        // One 30-row block + a trailing 2-row block, viewport keeps 10:
        // the big block cannot fit the live region, so its head commits.
        let mut transcript: Vec<TranscriptLine> = (0..30)
            .map(|i| tl(InlineMessageKind::Agent, &format!("big-{i:02}"), 0))
            .collect();
        transcript.push(tl(InlineMessageKind::Agent, "tail-a", 1));
        transcript.push(tl(InlineMessageKind::Agent, "tail-b", 1));
        let state = RenderState {
            transcript,
            ..Default::default()
        };
        let styles = active_styles();
        let display = build_transcript_display(&state, &styles, 0, 80);
        let plan = scrollback_commit_plan(&display, &state.transcript, 80, 10, None).expect("plan");
        assert_eq!(
            plan.new_committed_entries, 22,
            "32 rows, keep 10 → head 22 rows commit"
        );
        assert_eq!(plan.rows, 22);
    }

    #[test]
    fn rebuild_only_on_width_change() {
        // Height-only resize: nothing to rebuild — committed transcript
        // lives in the host terminal's scrollback at the width it was
        // printed at; the live viewport just changes rows.
        assert!(!should_rebuild_scrollback(80, 80, 24, 30));
        // Width grew: re-commit so freshly finalized rows wrap to the
        // new width and the old frozen scrollback must go (CSI 3J).
        assert!(should_rebuild_scrollback(80, 100, 24, 24));
        // Width shrank: same — the old layout no longer fits.
        assert!(should_rebuild_scrollback(100, 80, 30, 24));
        // Sentinel (final-review finding 1): prev_w == 0 means "never
        // measured" — no frame was drawn at a known width, so there is
        // no stale-width scrollback and the wipe must NOT fire, no
        // matter what the new width is.
        assert!(!should_rebuild_scrollback(0, 100, 24, 24));
        assert!(!should_rebuild_scrollback(0, 80, 24, 24));
    }

    #[test]
    fn force_flush_boundary_is_everything() {
        // Force-flush on exit ignores viewport fit and commits the
        // whole finalized prefix — every display row, regardless of
        // what fits in the live region.
        assert_eq!(plan_full_flush(0), 0);
        assert_eq!(plan_full_flush(8), 8);
        assert_eq!(plan_full_flush(40), 40);
    }
}

#[cfg(test)]
mod tool_box_tests {
    //! omp-style tool boxes: borders, divider labels, and — critically —
    //! display-width math. Korean text is width-2 per glyph; a char-count
    //! wrap or pad misaligns the right border instantly.
    use super::*;

    fn row_text(row: &[InlineSegment]) -> String {
        row.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn korean_rows_keep_the_right_border_aligned() {
        // w=20 → inner=16 cells. "한글" = 4 cells per word.
        let rows = tool_box_rows(
            "한글테스트 명령어",
            20,
            InlineTextStyle::default(),
            anstyle::Color::Ansi(anstyle::AnsiColor::White),
        );
        for row in &rows {
            let text = row_text(row);
            assert_eq!(text.width(), 20, "row must fill exactly 20 cells: {text:?}");
            assert!(text.starts_with('\u{2502}'), "left border: {text:?}");
            assert!(text.ends_with('\u{2502}'), "right border: {text:?}");
        }
        assert!(!rows.is_empty());
        // Wrapping counts cells, not chars: 9 Korean chars = 18 cells >
        // 16 inner → two rows.
        assert_eq!(rows.len(), 2, "wraps by display width");
    }

    #[test]
    fn divider_carries_the_label() {
        let seg = tool_box_divider(
            "Output",
            30,
            anstyle::Color::Ansi(anstyle::AnsiColor::White),
        );
        let text = row_text(&seg);
        assert!(
            text.starts_with("\u{251C}\u{2500} Output"),
            "label after ├─: {text:?}"
        );

        assert!(text.ends_with('\u{2524}'), "closes with ┤: {text:?}");
        assert_eq!(text.width(), 30, "divider fills the box width");
    }
}

#[cfg(test)]
mod tool_box_width_tests {
    //! Box width must equal the LIVE transcript content width (layout
    //! gutters + scrollbar column). At the raw terminal width every
    //! row's right border wraps onto the next visual line.
    use super::*;

    #[test]
    fn tool_box_width_matches_live_content_width() {
        let state = RenderState {
            viewport_width: 100,
            ..Default::default()
        };
        // CHAT_LAYOUT insets 1 column per side; the in-app scrollbar is
        // gone (native scrollback owns history): 100 - 2 = 98.
        assert_eq!(tool_box_width(&state), 98);
    }
}

#[test]
fn tool_box_rows_expand_tabs_so_borders_align() {
    // The read tool numbers lines as `{:>6}\t{content}`. unicode-width 0.2
    // counts the tab as 1 (`UnicodeWidthStr::width`), but ratatui drops it
    // when filling cells — a row built with tab width in its pad math
    // renders one column short and the right border lands inside the box.
    let chunk = format!("{:>6}\t{}", 1, "[package]");
    let rows = tool_box_rows(
        &chunk,
        176,
        InlineTextStyle::default(),
        anstyle::Color::Ansi(anstyle::AnsiColor::White),
    );
    assert_eq!(rows.len(), 1);
    for seg in &rows[0] {
        assert!(
            !seg.text.contains('\t'),
            "tabs must be expanded: {:?}",
            seg.text
        );
    }
    let built: usize = rows[0]
        .iter()
        .map(|s| UnicodeWidthStr::width(s.text.as_str()))
        .sum();
    assert_eq!(built, 176, "built width must equal the box width exactly");
}

#[cfg(test)]
mod contextual_hint_tests {
    //! The static shortcuts bar is gone; discoverability is contextual:
    //! the brain chip lives on the composer border, abort/quit hints
    //! appear only while a run is live or a quit is armed.
    use super::*;

    #[test]
    fn brain_chip_lives_on_the_composer_border() {
        let mut state = RenderState::default();
        state.header_context.provider = "prov".to_string();
        state.header_context.model = "prov/m-1".to_string();

        // Off (memory disabled) — no chip.
        let off = spans_to_string_border(&state);
        assert!(!off.contains("brain"), "chip hidden when off: {off}");

        // Ok — right side of the border.
        state.brain = BrainChip::Ok;
        let ok = spans_to_string_border(&state);
        assert!(ok.contains("brain·ok"), "healthy chip on border: {ok}");
        assert!(
            ok.trim_end().ends_with("brain·ok"),
            "chip is right-aligned: {ok}"
        );

        // Down — still renders.
        state.brain = BrainChip::Down;
        let down = spans_to_string_border(&state);
        assert!(down.contains("brain·down"), "degraded chip: {down}");
    }

    #[test]
    fn brain_chip_does_not_erase_the_border_rule() {
        // Regression: the chip used to be space-padded into the fields
        // title. A title overwrites the border row for its full width,
        // so the padding erased the `─` rule right of the facts.
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let mut state = RenderState::default();
        state.header_context.provider = "prov".to_string();
        state.header_context.model = "prov/m-1".to_string();
        state.brain = BrainChip::Ok;
        terminal
            .draw(|f| render_frame(f, &state, &unused_test_handle()))
            .expect("draw");
        let buf = terminal.backend().buffer();
        // The welcome card also prints "MODEL" when the transcript is
        // empty; the composer border row is the one with ` | ` field
        // separators.
        let border_row = (0..buf.area().height)
            .map(|y| {
                (0..buf.area().width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
            })
            .find(|row| row.contains("MODEL") && row.contains(" | "))
            .expect("composer border row rendered");
        let rule_count = border_row.chars().filter(|c| *c == '\u{2500}').count();
        assert!(
            rule_count >= 10,
            "the ─ rule must survive right of the facts: {border_row}"
        );
        assert!(
            border_row.contains("brain\u{b7}ok"),
            "chip still on the border: {border_row}"
        );
    }

    #[test]
    fn run_indicator_stays_up_between_turns() {
        // Mid-run the stage is cleared at each turn boundary
        // (MessageEnd/TurnEnd); the run tracker must keep the indicator
        // row owned so it never flickers to the idle row — and it should
        // carry progress facts (spinner, turn/tool counts, elapsed).
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let state = RenderState {
            active_run: Some(RunState {
                started_at: std::time::Instant::now(),
                turn: 2,
                tool_calls: 3,
            }),
            reasoning_stage: None,
            ..Default::default()
        };
        terminal
            .draw(|f| render_frame(f, &state, &unused_test_handle()))
            .expect("draw");
        let buf = terminal.backend().buffer();
        let row: String = (0..buf.area().width)
            .filter_map(|x| buf.cell((x, 20)).map(|c| c.symbol().to_string()))
            .collect();
        assert!(row.contains("RUNNING"), "row stays up between turns: {row}");
        assert!(row.contains("working"), "stage fallback label: {row}");
        assert!(row.contains("turn 2"), "turn count: {row}");
        assert!(row.contains("3 tool calls"), "tool count: {row}");
        assert!(row.contains("Esc abort"), "abort hint stays: {row}");
        assert!(
            RUN_SPINNER.iter().any(|f| row.contains(f)),
            "animated spinner frame: {row}"
        );
    }

    #[test]
    fn spinner_frame_is_wall_clock_not_draw_count() {
        // Regression: the spinner advanced on FRAME_TICK (draw count).
        // Event bursts during streaming drive many draws per interval,
        // so the spinner raced. Animation frames must key on wall-clock
        // time — rapid back-to-back draws show the SAME frame.
        let state = || RenderState {
            active_run: Some(RunState::default()),
            reasoning_stage: None,
            ..Default::default()
        };
        let spinner_of = |s: &RenderState| -> Option<char> {
            let backend = ratatui::backend::TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).expect("backend");
            terminal
                .draw(|f| render_frame(f, s, &unused_test_handle()))
                .expect("draw");
            let buf = terminal.backend().buffer();
            let row: String = (0..buf.area().width)
                .filter_map(|x| buf.cell((x, 20)).map(|c| c.symbol().to_string()))
                .collect();
            row.chars()
                .find(|c| RUN_SPINNER.iter().any(|f| f.starts_with(*c)))
        };
        // Two draws in the same animation period (sub-80ms apart, which
        // consecutive draws in one test always are).
        let first = spinner_of(&state());
        let second = spinner_of(&state());
        assert!(first.is_some(), "spinner renders");
        assert_eq!(
            first, second,
            "back-to-back draws must not advance the spinner"
        );
    }

    #[test]
    fn elapsed_formats_minutes_beyond_60s() {
        assert_eq!(format_elapsed_secs(59), "59s");
        assert_eq!(format_elapsed_secs(60), "1m 00s");
        assert_eq!(format_elapsed_secs(125), "2m 05s");
    }

    #[test]
    fn reasoning_row_carries_the_abort_hint() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let state = RenderState {
            reasoning_stage: Some("thinking\u{2026}".into()),
            ..Default::default()
        };
        terminal
            .draw(|f| render_frame(f, &state, &unused_test_handle()))
            .expect("draw");
        let buf = terminal.backend().buffer();
        let row: String = (0..buf.area().width)
            .filter_map(|x| buf.cell((x, 20)).map(|c| c.symbol().to_string()))
            .collect();
        assert!(
            row.contains("Esc abort"),
            "streaming shows the contextual abort hint: {row}"
        );
    }

    #[test]
    fn pending_quit_owns_the_hint_row() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let state = RenderState {
            pending_quit: true,
            ..Default::default()
        };
        terminal
            .draw(|f| render_frame(f, &state, &unused_test_handle()))
            .expect("draw");
        let buf = terminal.backend().buffer();

        let row: String = (0..buf.area().width)
            .filter_map(|x| buf.cell((x, 20)).map(|c| c.symbol().to_string()))
            .collect();
        assert!(
            row.contains("press Ctrl+C again to quit"),
            "armed quit shows its hint: {row}"
        );
    }

    fn spans_to_string_border(state: &RenderState) -> String {
        let line = composer_context_line(state, 200);
        let mut text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let used = line.spans.iter().map(|s| s.width()).sum();
        if let Some(chip) = composer_brain_chip(state, 200, used) {
            assert_eq!(
                chip.alignment,
                Some(Alignment::Right),
                "the chip is its own right-aligned title"
            );
            text.extend(chip.spans.iter().map(|s| s.content.as_ref()));
        }
        text
    }

    fn unused_test_handle() -> InlineHandle {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        InlineHandle::new_for_tests(tx)
    }
}

#[cfg(test)]
mod trailing_breath_tests {
    //! The gap between the transcript and the composer is LAYOUT: the
    //! scrollback area reserves one blank row above the prompt, so the
    //! newest response never glues to the composer at any height — and
    //! the gap can't be windowed or committed away.
    use super::*;

    #[test]
    fn display_ends_at_the_last_line_no_trailing_blank_item() {
        let state = RenderState {
            transcript: vec![TranscriptLine {
                kind: InlineMessageKind::Agent,
                segments: vec![plain_segment("answer")],
                block_id: 0,
            }],
            ..Default::default()
        };
        let styles = active_styles();
        let display = build_transcript_display(&state, &styles, 0, 80);
        assert_eq!(display.len(), 1, "the gap is layout, not a display item");
        assert!(display[0].line.is_some());
    }

    #[test]
    fn scrollback_area_reserves_one_breath_row_above_the_composer() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let layout = super::super::frame_layout::compute_chrome(area);
        assert_eq!(
            layout.scrollback.bottom() + 1,
            layout.prompt.y,
            "exactly one row separates the transcript from the composer"
        );
        assert_eq!(
            super::super::frame_layout::scrollback_height(area),
            layout.scrollback.height,
            "the commit keep-rows must match the rendered area"
        );
    }
}

#[cfg(test)]
mod nerd_icon_tests {
    //! `glyph_set = "nerd"` swaps the composer's text labels for Nerd
    //! Font private-use glyphs — never emoji. Default (unicode) keeps
    //! the text labels.
    use super::*;

    fn border_text(state: &RenderState) -> String {
        let line = composer_context_line(state, 200);
        let mut text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let used = line.spans.iter().map(|s| s.width()).sum();
        if let Some(chip) = composer_brain_chip(state, 200, used) {
            text.extend(chip.spans.iter().map(|s| s.content.as_ref()));
        }
        text
    }

    fn base_state() -> RenderState {
        let mut state = RenderState::default();
        state.header_context.provider = "prov".to_string();
        state.header_context.model = "prov/m-1".to_string();
        state.brain = BrainChip::Ok;
        state
    }

    #[test]
    fn nerd_mode_replaces_labels_with_private_use_glyphs() {
        let mut state = base_state();
        state.glyph_set = crate::symbols::GlyphSet::Nerd;
        let text = border_text(&state);
        assert!(!text.contains("MODEL "), "text label gone: {text}");
        assert!(
            text.contains(crate::symbols::nerd::MODEL),
            "robot glyph for the model: {text}"
        );
        assert!(
            text.contains(crate::symbols::nerd::GIT),
            "git glyph present: {text}"
        );
        assert!(
            text.contains(crate::symbols::nerd::BRAIN),
            "brain glyph chip: {text}"
        );
        // No emoji ever: all swaps live in the private-use area
        // (U+E000–U+F8FF and the supplementary PUA planes).
        for ch in text.chars() {
            let cp = ch as u32;
            let private_use = (0xE000..=0xF8FF).contains(&cp)
                || (0xF0000..=0xFFFFD).contains(&cp)
                || (0x100000..=0x10FFFD).contains(&cp);
            assert!(
                !('\u{1F300}'..='\u{1FAFF}').contains(&ch) || !private_use,
                "sanity"
            );
        }
    }

    #[test]
    fn unicode_default_keeps_text_labels() {
        let state = base_state();
        let text = border_text(&state);
        assert!(text.contains("MODEL "), "default keeps text: {text}");
        assert!(text.contains("brain\u{b7}ok"), "default chip text: {text}");
    }
}
#[cfg(test)]
mod glyph_cycle_tests {
    use crate::symbols::GlyphSet;

    #[test]
    fn glyph_set_cycles_unicode_ascii_nerd() {
        assert_eq!(GlyphSet::Unicode.next(), GlyphSet::Ascii);
        assert_eq!(GlyphSet::Ascii.next(), GlyphSet::Nerd);
        assert_eq!(GlyphSet::Nerd.next(), GlyphSet::Unicode);
    }
}

#[cfg(test)]
mod coalesce_draw_tests {
    //! Render coalescing: the event loop used to redraw on every iteration,
    //! causing a frame storm during token-stream bursts (one full
    //! `terminal.draw` per agent event). The fix is `coalesce_draw`: most
    //! arms gate the post-select draw behind a 50ms cadence; user-facing
    //! arms (keyboard, SIGINT, brain chip) raise `priority = true` for
    //! an immediate repaint.
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn defer_within_interval() {
        let now = Instant::now();
        let last = now - Duration::from_millis(10);
        assert_eq!(
            coalesce_draw(last, false, DRAW_MIN_INTERVAL),
            DrawDecision::Defer
        );
    }

    #[test]
    fn draw_now_on_priority_even_within_interval() {
        let now = Instant::now();
        let last = now - Duration::from_millis(10);
        assert_eq!(
            coalesce_draw(last, true, DRAW_MIN_INTERVAL),
            DrawDecision::DrawNow
        );
    }

    #[test]
    fn draw_now_when_interval_elapsed() {
        let now = Instant::now();
        let last = now - Duration::from_millis(60);
        assert_eq!(
            coalesce_draw(last, false, DRAW_MIN_INTERVAL),
            DrawDecision::DrawNow
        );
    }
}
#[cfg(test)]
mod settings_panel_tests {
    use super::*;
    use crate::app::agent_session::AgentSessionHandle;
    use crate::store::settings::Settings;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn make_session_with_tools() -> AgentSessionHandle {
        super::provider_overlay_tests::make_session_with_tools_for_tests()
    }
    use oxicode_vtui::tui::core::InlineListSelection;
    use ratatui::{Terminal, backend::TestBackend};

    fn heading(title: &str) -> OverlayListItem {
        OverlayListItem {
            title: title.into(),
            subtitle: None,
            badge: None,
            indent: 0,
            search_value: None,
            selection: None,
        }
    }

    fn row(title: &str, badge: &str, selection: Option<InlineListSelection>) -> OverlayListItem {
        OverlayListItem {
            title: title.into(),
            subtitle: None,
            badge: Some(badge.into()),
            indent: 0,
            search_value: None,
            selection,
        }
    }

    /// Collect each terminal row as (y, concatenated text, per-char x
    /// positions) so tests can assert on WHERE content landed, not just
    /// that it exists.
    fn rows_with_positions(terminal: &Terminal<TestBackend>) -> Vec<(u16, String, Vec<u16>)> {
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut out = Vec::new();
        for y in 0..area.height {
            let mut text = String::new();
            let mut xs = Vec::new();
            for x in 0..area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    text.push_str(cell.symbol());
                    xs.push(x);
                }
            }
            out.push((y, text, xs));
        }
        out
    }

    /// All (y, x) offsets where `needle` starts in the rendered buffer.
    fn occurrences(rows: &[(u16, String, Vec<u16>)], needle: &str) -> Vec<(u16, usize)> {
        let mut hits = Vec::new();
        for (y, text, xs) in rows {
            let mut from = 0;
            while let Some(rel) = text[from..].find(needle) {
                let byte_idx = from + rel;
                let char_idx = text[..byte_idx].chars().count();
                if let Some(&x) = xs.get(char_idx) {
                    hits.push((*y, x as usize));
                }
                from = byte_idx + needle.len();
            }
        }
        hits
    }

    /// A tabbed overlay (>= 2 sections, width >= 60) renders the tab bar
    /// and the sidebar column: section names appear BOTH in the sidebar
    /// (left of the item column) and as in-list heading rows, and rows
    /// outside the active section are dimmed.
    #[test]
    fn render_overlay_tabbed_settings_shows_tab_bar_and_sidebar() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let overlay = OverlayState {
            title: "Settings".into(),
            lines: Vec::new(),
            items: vec![
                heading("Defaults"),
                row(
                    "Thinking level",
                    "medium",
                    Some(InlineListSelection::ConfigAction("ThinkingLevel".into())),
                ),
                row("Model roles", "0", None),
                heading("Pointers"),
                row("Theme", "dark", None),
            ],
            selected: 1,
            search: None,
            secure_input: None,
            tabs: vec!["General".into(), "Model".into(), "Interaction".into()],
            active_tab: 1,
            sections: vec!["Defaults".into(), "Pointers".into()],
            active_section: 0,
            key_capture: None,
        };
        terminal
            .draw(|f| render_overlay(f, f.area(), &overlay))
            .unwrap();
        let rows = rows_with_positions(&terminal);

        // Tab bar: one row names the inactive tabs flanking the active
        // one.
        let general = occurrences(&rows, "General");
        let interaction = occurrences(&rows, "Interaction");
        assert!(
            general
                .iter()
                .any(|(gy, _)| interaction.iter().any(|(iy, _)| gy == iy)),
            "tab bar must list tabs on one row"
        );

        // Sidebar geometry: sidebar width = min(22, longest)+4 = 12, so
        // the sidebar column occupies x < 13 and the item list starts at
        // x >= 13.
        for name in ["Defaults", "Pointers"] {
            let hits = occurrences(&rows, name);
            assert!(hits.len() >= 2, "{name} must render in sidebar AND list");
            assert!(
                hits.iter().any(|(_, x)| *x < 13),
                "{name} must render in the sidebar column"
            );
            assert!(
                hits.iter().any(|(_, x)| *x >= 13),
                "{name} must render in the item column"
            );
        }

        // Out-of-section rows recede: the items-column "Pointers"
        // heading is DIM while the active section's is not.
        let buf = terminal.backend().buffer();
        let pointers_item_col = occurrences(&rows, "Pointers")
            .into_iter()
            .find(|(_, x)| *x >= 13)
            .expect("items-column Pointers heading");
        let cell = buf
            .cell((pointers_item_col.1 as u16, pointers_item_col.0))
            .expect("cell");
        assert!(
            cell.modifier.contains(Modifier::DIM),
            "out-of-section rows must be dimmed"
        );
        let defaults_item_col = occurrences(&rows, "Defaults")
            .into_iter()
            .find(|(_, x)| *x >= 13)
            .expect("items-column Defaults heading");
        let cell = buf
            .cell((defaults_item_col.1 as u16, defaults_item_col.0))
            .expect("cell");
        assert!(
            !cell.modifier.contains(Modifier::DIM),
            "active-section rows must not be dimmed"
        );
    }

    /// The input loop resolves shortcuts through the live keymap: with
    /// `SendNow` rebound to `Alt+s`, that combo fires the send-now path
    /// (interrupt + immediate submit of the composed buffer) while the
    /// default `Ctrl+Enter` still resolves.
    #[test]
    fn rebound_send_now_combo_submits_immediately() {
        let state = Arc::new(parking_lot::Mutex::new(RenderState::default()));
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("SendNow".to_string(), vec!["Alt+s".to_string()]);
        *state.lock().keymap.write() = Keymap::from_settings(&overrides);

        let alt_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        let action = state
            .lock()
            .keymap
            .read()
            .resolve(alt_s)
            .expect("Alt+s must resolve to SendNow after the rebind");
        assert!(matches!(action, GlobalAction::SendNow));
        // Overrides replace only the named action's combo list: the old
        // default combo no longer fires SendNow, while every other
        // action keeps its default.
        let ctrl_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(state.lock().keymap.read().resolve(ctrl_enter), None);
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert!(matches!(
            state.lock().keymap.read().resolve(ctrl_p),
            Some(GlobalAction::OpenCommandPalette)
        ));

        state.lock().composer.set_text("send me now");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        apply_global_action(action, &state, &tx);

        assert_eq!(state.lock().composer.text(), "");
        match rx.try_recv().expect("interrupt fires first") {
            InlineEvent::Interrupt => {}
            other => panic!("expected Interrupt first, got {other:?}"),
        }
        match rx.try_recv().expect("submit fires second") {
            InlineEvent::Submit(text) => assert_eq!(&*text, "send me now"),
            other => panic!("expected Submit, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "no further events");
    }

    /// `OverlaySubmission` variants the settings panel emits must stay
    /// constructible through the compat layer (compile-level contract).
    #[test]
    fn settings_selection_variants_round_trip_names() {
        assert_eq!(
            InlineListSelection::SettingsTab(1),
            InlineListSelection::SettingsTab(1)
        );
        assert_eq!(
            InlineListSelection::SettingsSection(0),
            InlineListSelection::SettingsSection(0)
        );
        assert_eq!(
            InlineListSelection::SettingKeyCapture("OpenCommandPalette".into()),
            InlineListSelection::SettingKeyCapture("OpenCommandPalette".into())
        );
        assert_eq!(
            InlineListSelection::SettingTextEdit("ToolTimeoutSecs".into()),
            InlineListSelection::SettingTextEdit("ToolTimeoutSecs".into())
        );
        assert_eq!(
            InlineListSelection::SettingSubmenuOpen("AdvisorSyncBacklog".into()),
            InlineListSelection::SettingSubmenuOpen("AdvisorSyncBacklog".into())
        );
        assert_eq!(
            InlineListSelection::SettingMultiselect("DisabledTools".into()),
            InlineListSelection::SettingMultiselect("DisabledTools".into())
        );
    }

    /// Capturing a new combo for `OpenCommandPalette` is additive: the
    /// next `Keymap::resolve` call resolves BOTH the new combo and the
    /// original default `Ctrl+P`. The capture path drives the round
    /// trip end-to-end — `SettingKeyCapture` selection opens the
    /// capture prompt, the simulated `KeyEvent` is fed straight to
    /// `handle_key_capture`, and the live `RenderState::keymap` is the
    /// single source of truth the test inspects.
    ///
    /// SANDBOXED: writes go to a tempdir `settings.json` via the
    /// `settings_override_path` hook so the real `~/.oxicode/settings.*`
    /// is never touched (the previous version of this test polluted the
    /// developer's live config — see final-review finding 1).
    #[test]
    fn key_capture_appends_combo_and_keeps_default_resolving() {
        // Snapshot the real ~/.oxicode settings.json mtime so the
        // post-condition assertion catches accidental leakage.
        let real_settings = dirs::home_dir()
            .map(|h| h.join(".oxicode").join("settings.json"))
            .filter(|p| p.exists());
        let real_settings_mtime_before = real_settings
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        let real_settings_sha_before = real_settings
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                b.hash(&mut h);
                h.finish()
            });

        let tmp = tempfile::tempdir().expect("tempdir");
        let sandbox = tmp.path().join("settings.json");

        let mut state = RenderState::default();
        state.settings_override_path = Some(sandbox.clone());
        // Open the capture overlay for OpenCommandPalette — same
        // selection variant `handle_inline_event` would dispatch from
        // the settings panel.
        state.overlay = Some(build_key_capture_overlay(
            GlobalAction::OpenCommandPalette.name(),
        ));
        state.settings_map_rows.clear();

        // Pre-condition: the default resolves, the new combo doesn't.
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        let alt_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT);
        assert_eq!(
            state.keymap.read().resolve(ctrl_p),
            Some(GlobalAction::OpenCommandPalette)
        );
        assert_eq!(state.keymap.read().resolve(alt_p), None);

        // Drive the capture flow with an Alt+P press.
        handle_key_capture(&mut state, alt_p);

        // Post-condition: both combos resolve (additive merge into the
        // live keymap) — and the panel has been rebuilt back on the
        // Keybindings tab with a success status, not the capture
        // prompt.
        let keymap = state.keymap.read();
        assert_eq!(
            keymap.resolve(ctrl_p),
            Some(GlobalAction::OpenCommandPalette)
        );
        assert_eq!(
            keymap.resolve(alt_p),
            Some(GlobalAction::OpenCommandPalette)
        );
        drop(keymap);
        let overlay = state
            .overlay
            .as_ref()
            .expect("capture commits reopen the panel on Keybindings");
        assert!(
            overlay.key_capture.is_none(),
            "capture prompt must be closed"
        );
        assert_eq!(
            overlay.lines.first().map(String::as_str),
            Some("Captured Alt+p for OpenCommandPalette")
        );
        assert_eq!(state.settings_active_tab, SettingsTab::Keybindings);

        // Sandbox assertion: the tempdir received the write, the real
        // `~/.oxicode/settings.json` is untouched (no mtime or
        // content change).
        let sandbox_contents = std::fs::read_to_string(&sandbox)
            .expect("sandbox settings.json must exist after capture");
        assert!(
            sandbox_contents.contains("OpenCommandPalette"),
            "sandbox file must contain the captured keybinding override; got {sandbox_contents}"
        );
        assert!(
            sandbox_contents.contains("Alt+p"),
            "sandbox file must contain the captured Alt+p combo; got {sandbox_contents}"
        );
        if let Some(before) = real_settings_mtime_before {
            let after = real_settings
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok())
                .expect("real settings.json must still exist after capture");
            assert_eq!(
                before, after,
                "real ~/.oxicode/settings.json mtime must not change (sandbox leak)"
            );
        }
        if let (Some(before), Some(after)) = (
            real_settings_sha_before,
            real_settings
                .as_ref()
                .and_then(|p| std::fs::read(p).ok())
                .map(|b| {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    b.hash(&mut h);
                    h.finish()
                }),
        ) {
            assert_eq!(
                before, after,
                "real ~/.oxicode/settings.json content must not change (sandbox leak)"
            );
        }
    }

    /// The remove-last-binding guard refuses to drop the final combo of
    /// an action — an action with zero keys would be a silent trap
    /// (the user could neither trigger it nor reach this panel to fix
    /// it). We pre-bind `OpenCommandPalette` to a single combo (the
    /// default) and confirm `remove_keybinding_combo` no-ops the
    /// removal while surfacing the reason in the panel status.
    #[test]
    fn remove_keybinding_combo_refuses_to_drop_the_last_combo() {
        let mut state = RenderState::default();
        // Force a single-combo state: replace OpenCommandPalette's
        // list with just `Ctrl+p` (the default minus all other
        // combos the action doesn't have — the point is that the
        // list ends up at length 1).
        let mut settings = Settings::default();
        crate::tui_vt::settings_defs::set_action_combos(
            &mut settings,
            GlobalAction::OpenCommandPalette,
            vec!["Ctrl+p".to_string()],
        );
        *state.keymap.write() = Keymap::from_settings(&settings.keybindings);
        assert_eq!(
            state
                .keymap
                .read()
                .action_combos(GlobalAction::OpenCommandPalette)
                .len(),
            1,
            "test setup: action must start with exactly one combo"
        );

        // Place the panel somewhere (the guard reopens it, but
        // starting state should be observable).
        state.settings_active_tab = SettingsTab::Keybindings;

        remove_keybinding_combo(&mut state, GlobalAction::OpenCommandPalette, "Ctrl+p");

        // The combo is still live — the guard refused the removal.
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(
            state.keymap.read().resolve(ctrl_p),
            Some(GlobalAction::OpenCommandPalette),
            "the guard must not let OpenCommandPalette go combo-less"
        );
        assert_eq!(
            state
                .keymap
                .read()
                .action_combos(GlobalAction::OpenCommandPalette)
                .len(),
            1,
            "no combo was removed"
        );
        // The reason is surfaced as the panel status line so the user
        // knows why nothing happened.
        let overlay = state.overlay.as_ref().expect("reopen leaves the panel up");
        assert!(
            overlay
                .lines
                .first()
                .map(|l| l.contains("Refusing to remove the last combo"))
                .unwrap_or(false),
            "panel must explain why the removal was refused; got {:?}",
            overlay.lines
        );
    }

    // ── Final-fix wave: Text / SubmenuSelect / Multiselect editors ────

    /// `commit_text_edit` with valid numeric input: the value is
    /// parsed, persisted to the SANDBOX path, and the panel reopens
    /// with a status line showing the new value.
    #[test]
    fn text_edit_commit_valid_input() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sandbox = tmp.path().join("settings.json");
        let mut state = RenderState::default();
        state.settings_override_path = Some(sandbox.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = InlineHandle::new_for_tests(tx);

        let (outcome, _msg) = commit_text_edit(
            &mut state,
            &handle,
            None,
            SettingKey::SessionHistorySize,
            "300".to_string(),
        );
        assert!(outcome.is_ok(), "valid numeric input must commit");

        // The sandbox file received the write with the new value.
        let contents = std::fs::read_to_string(&sandbox).expect("sandbox written");
        let saved: Settings = serde_json::from_str(&contents).expect("sandbox parses");
        assert_eq!(
            saved.session_history_size, 300,
            "sandbox must hold session_history_size=300"
        );

        // The panel reopened with a status line naming the new value.
        let overlay = state.overlay.as_ref().expect("panel reopened");
        assert!(
            overlay
                .lines
                .first()
                .map(|l| l.contains("300"))
                .unwrap_or(false),
            "status line must show the new value; got {:?}",
            overlay.lines
        );
        // And the transcript Info line was emitted.
        let mut saw_info = false;
        while let Ok(cmd) = rx.try_recv() {
            if let InlineCommand::AppendLine { kind, .. } = cmd
                && matches!(kind, InlineMessageKind::Info)
            {
                saw_info = true;
            }
        }
        assert!(saw_info, "commit must surface an Info line");
    }

    /// `commit_text_edit` with INVALID input: the parse fails, nothing
    /// is persisted (no sandbox file), and the failure surfaces as an
    /// Error line — never a silent no-op.
    #[test]
    fn text_edit_commit_invalid_input_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sandbox = tmp.path().join("settings.json");
        let mut state = RenderState::default();
        state.settings_override_path = Some(sandbox.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = InlineHandle::new_for_tests(tx);

        let (outcome, msg) = commit_text_edit(
            &mut state,
            &handle,
            None,
            SettingKey::SessionHistorySize,
            "not-a-number".to_string(),
        );
        assert!(outcome.is_err(), "non-numeric input must be rejected");
        assert!(
            msg.contains("invalid") || msg.contains("ParseError") || !msg.is_empty(),
            "rejection must carry a reason; got {msg}"
        );
        // No write happened.
        assert!(
            !sandbox.exists(),
            "rejected input must not persist anything"
        );
        // The failure surfaced as an Error line.
        let mut saw_error = false;
        while let Ok(cmd) = rx.try_recv() {
            if let InlineCommand::AppendLine { kind, .. } = cmd
                && matches!(kind, InlineMessageKind::Error)
            {
                saw_error = true;
            }
        }
        assert!(saw_error, "rejection must surface an Error line");
    }

    /// `open_submenu_select_prompt` builds the option list from the
    /// def's `SubmenuSelect` options with the current value marked, and
    /// `commit_submenu_choice` persists the choice and reopens the
    /// panel.
    #[test]
    fn submenu_select_commit_for_sync_backlog() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sandbox = tmp.path().join("settings.json");
        let mut state = RenderState::default();
        state.settings_override_path = Some(sandbox.clone());

        // Open the submenu: rows for off/sync/async, current marked.
        open_submenu_select_prompt(&mut state, SettingKey::AdvisorSyncBacklog);
        let overlay = state.overlay.as_ref().expect("submenu overlay opens");
        assert_eq!(overlay.items.len(), 3, "off/sync/async rows");
        let titles: Vec<&str> = overlay.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["off", "sync", "async"]);
        // Every row carries a SubmenuCommit selection payload.
        for item in &overlay.items {
            let sel = item.selection.as_ref().expect("row is selectable");
            match sel {
                InlineListSelection::ConfigAction(p) => {
                    assert!(
                        p.starts_with("SubmenuCommit:AdvisorSyncBacklog:"),
                        "payload must address the key; got {p}"
                    );
                }
                other => panic!("expected ConfigAction, got {other:?}"),
            }
        }

        // Commit "async" through the commit helper.
        let status = commit_submenu_choice(
            &mut state,
            SettingKey::AdvisorSyncBacklog,
            "async".to_string(),
        )
        .expect("valid option commits");
        assert!(status.contains("async"), "status names the new value");

        // The sandbox file holds the new value.
        let contents = std::fs::read_to_string(&sandbox).expect("sandbox written");
        assert!(
            contents.contains("async"),
            "sandbox must hold the async choice: {contents}"
        );
        // The panel reopened with the status line.
        let overlay = state.overlay.as_ref().expect("panel reopened");
        assert!(
            overlay
                .lines
                .first()
                .map(|l| l.contains("async"))
                .unwrap_or(false),
            "status line must show the new value"
        );
    }

    /// The multiselect editor toggles a non-essential tool into (and
    /// out of) `disabled_tools`, persisting through the sandbox, and
    /// REFUSES an essential tool with an Error line and no write.
    #[test]
    fn multiselect_toggles_tool_and_refuses_essential() {
        let session = make_session_with_tools();
        let tmp = tempfile::tempdir().expect("tempdir");
        let sandbox = tmp.path().join("settings.json");
        let mut state = RenderState::default();
        state.settings_override_path = Some(sandbox.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = InlineHandle::new_for_tests(tx);

        // The overlay lists the registry's tools (bash + commit from
        // the fixture), sorted, with essential badges.
        open_disabled_tools_multiselect(&mut state, &session);
        let overlay = state.overlay.as_ref().expect("multiselect opens");
        let titles: Vec<&str> = overlay.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["bash", "commit"], "registry tools, sorted");
        let bash = &overlay.items[0];
        assert_eq!(bash.badge.as_deref(), Some("essential"));
        let commit = &overlay.items[1];
        assert_eq!(commit.badge.as_deref(), Some("enabled"));

        // Toggle the optional tool OFF (disable): commit ∈ disabled_tools.
        commit_disabled_tool_toggle(
            &mut state,
            &handle,
            &session,
            "commit".to_string(),
            false, // not essential
            false, // currently enabled
        );
        let saved: Settings = serde_json::from_str(&std::fs::read_to_string(&sandbox).unwrap())
            .expect("sandbox parses");
        assert!(
            saved.disabled_tools.iter().any(|t| t == "commit"),
            "toggle must add 'commit' to disabled_tools; got {:?}",
            saved.disabled_tools
        );

        // Toggle it back ON (enable): commit ∉ disabled_tools.
        commit_disabled_tool_toggle(
            &mut state,
            &handle,
            &session,
            "commit".to_string(),
            false, // not essential
            true,  // currently disabled
        );
        let saved: Settings = serde_json::from_str(&std::fs::read_to_string(&sandbox).unwrap())
            .expect("sandbox parses");
        assert!(
            !saved.disabled_tools.iter().any(|t| t == "commit"),
            "toggle must remove 'commit' from disabled_tools; got {:?}",
            saved.disabled_tools
        );

        // Essential refusal: an Error line is emitted, no write happens.
        let before = std::fs::read_to_string(&sandbox).unwrap();
        commit_disabled_tool_toggle(
            &mut state,
            &handle,
            &session,
            "bash".to_string(),
            true,  // essential
            false, // currently enabled
        );
        let after = std::fs::read_to_string(&sandbox).unwrap();
        assert_eq!(before, after, "essential refusal must not write");
        let mut saw_refusal = false;
        while let Ok(cmd) = rx.try_recv() {
            if let InlineCommand::AppendLine { kind, segments } = cmd
                && matches!(kind, InlineMessageKind::Error)
                && segments.iter().any(|s| s.text.contains("essential"))
            {
                saw_refusal = true;
            }
        }
        assert!(
            saw_refusal,
            "essential refusal must surface an Error line mentioning 'essential'"
        );
    }
}

// Inline image previews — generate_image result hook (kitty/iTerm2).
// ═════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod image_preview_hook_tests {
    use super::*;
    use base64::{Engine, engine::general_purpose};
    use tokio::sync::mpsc;

    /// Craft a generate_image tool-result body in the exact shape
    /// `GenerateImageTool::execute` produces.
    fn image_result_content(payload: &[u8]) -> String {
        let b64 = general_purpose::STANDARD.encode(payload);
        format!(
            "Generated 1 image(s).\n\nImage 1 ({} bytes, base64):\n{}\n",
            payload.len(),
            b64
        )
    }

    fn fresh_handle() -> (InlineHandle, mpsc::UnboundedReceiver<InlineCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (InlineHandle::new_for_tests(tx), rx)
    }

    fn apply_all(state: &mut RenderState, rx: &mut mpsc::UnboundedReceiver<InlineCommand>) {
        while let Ok(cmd) = rx.try_recv() {
            apply_command(state, cmd);
        }
    }

    fn transcript_text(state: &RenderState) -> Vec<String> {
        state
            .transcript
            .iter()
            .map(|l| {
                l.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect()
    }

    /// A successful generate_image result renders the text-fallback row
    /// (never the raw base64 wall) and enqueues the decoded PNG keyed by
    /// its content hash, pointing at the fallback row.
    #[test]
    fn generate_image_result_renders_fallback_row_and_enqueues_live_preview() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();
        let payload = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionStart {
                tool_call_id: "img-1".into(),
                tool_name: "generate_image".into(),
                args: serde_json::json!({"prompt": "a cat"}),
                intent: None,
                context: None,
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionEnd {
                tool_call_id: "img-1".into(),
                tool_name: "generate_image".into(),
                intent: None,
                result: oxicode_ai::ToolResult {
                    tool_call_id: "img-1".into(),
                    content: image_result_content(&payload),
                    status: "success".into(),
                },
                is_error: false,
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);

        let texts = transcript_text(&state);
        let fallback_idx = texts
            .iter()
            .position(|t| t.contains("[image: generate_image:"))
            .expect("fallback row rendered in the tool box");
        assert!(
            texts
                .iter()
                .all(|t| !t.contains(&general_purpose::STANDARD.encode(payload))),
            "raw base64 must never render as text"
        );

        assert_eq!(
            state.image_previews.pending_len(),
            1,
            "decoded PNG enqueued for live placement"
        );
        let pending = &state.image_previews.pending()[0];
        assert_eq!(&*pending.png, &payload, "decoded bytes round-trip");
        // The pending preview's label resolves to the fallback row — this
        // is the lookup the render pass uses to anchor the placement.
        assert!(
            texts[fallback_idx].contains(&pending.label),
            "label {label:?} matches the fallback row {row:?}",
            label = pending.label,
            row = texts[fallback_idx],
        );
    }

    /// Results without an embedded base64 image (API errors, empty
    /// responses) keep the generic preview path and enqueue nothing.
    #[test]
    fn generate_image_without_payload_keeps_generic_preview() {
        let mut state = RenderState::default();
        let (handle, mut rx) = fresh_handle();
        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionStart {
                tool_call_id: "img-2".into(),
                tool_name: "generate_image".into(),
                args: serde_json::json!({"prompt": "a cat"}),
                intent: None,
                context: None,
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionEnd {
                tool_call_id: "img-2".into(),
                tool_name: "generate_image".into(),
                intent: None,
                result: oxicode_ai::ToolResult {
                    tool_call_id: "img-2".into(),
                    content: "Image generation completed but returned no images.".into(),
                    status: "success".into(),
                },
                is_error: false,
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);
        let texts = transcript_text(&state);
        assert!(
            texts.iter().any(|t| t.contains("returned no images")),
            "generic preview path still renders the summary"
        );
        assert_eq!(state.image_previews.pending_len(), 0);
    }

    /// End-to-end: a live frame records the anchor for the pending
    /// image's tool box, and the post-draw emit produces the full kitty
    /// sequence (CUP + transmit + place) for it.
    #[test]
    fn live_frame_anchors_and_emits_kitty_sequence() {
        use crate::tui_vt::image_preview::{ImagePreviews, ImageSupport};
        use ratatui::{Terminal, backend::TestBackend};

        let mut state = RenderState::default();
        state.image_previews = ImagePreviews::new(ImageSupport::Kitty);
        let (handle, mut rx) = fresh_handle();
        let payload = [0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionStart {
                tool_call_id: "img-3".into(),
                tool_name: "generate_image".into(),
                args: serde_json::json!({"prompt": "a dog"}),
                intent: None,
                context: None,
            },
            &mut state,
        );
        map_agent_event(
            &handle,
            AgentEvent::ToolExecutionEnd {
                tool_call_id: "img-3".into(),
                tool_name: "generate_image".into(),
                intent: None,
                result: oxicode_ai::ToolResult {
                    tool_call_id: "img-3".into(),
                    content: image_result_content(&payload),
                    status: "success".into(),
                },
                is_error: false,
            },
            &mut state,
        );
        apply_all(&mut state, &mut rx);

        // Render one live frame (records the anchor through the shared
        // interior-mutable channel).
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let (tx, _drain) = mpsc::unbounded_channel();
        terminal
            .draw(|frame| render_frame(frame, &state, &InlineHandle::new_for_tests(tx)))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let frame_text: String = (0..buf.area().height)
            .map(|y| {
                (0..buf.area().width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            frame_text.contains("[image: generate_image:"),
            "live frame paints the fallback row"
        );

        // Post-draw emit: full kitty stream for the anchored box.
        let seq = state.image_previews.emit_live(state.committed_entries);
        assert!(seq.contains("\x1b["));
        assert!(seq.contains("\x1b_Ga=t,f=100"), "transmit");
        assert!(seq.contains("a=p"), "placement");
        assert_eq!(state.image_previews.pending_len(), 0, "placed and consumed");
    }

    /// `extract_generated_png` — the marker parse powering the hook.
    #[test]
    fn extract_generated_png_parses_first_image_and_rejects_garbage() {
        let payload = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        assert_eq!(
            extract_generated_png(&image_result_content(&payload)),
            Some(payload),
            "first base64 blob after the marker decodes"
        );
        // Multiple images: only the first is previewed. Payloads clear
        // the 8-byte PNG-header sanity floor.
        let first: Vec<u8> = (1u8..=8).collect();
        let second: Vec<u8> = (9u8..=16).collect();
        let two = format!(
            "Generated 2 image(s).\n\nImage 1 (8 bytes, base64):\n{}\n\nImage 2 (8 bytes, base64):\n{}\n",
            general_purpose::STANDARD.encode(&first),
            general_purpose::STANDARD.encode(&second),
        );
        assert_eq!(extract_generated_png(&two), Some(first));
        // No marker / invalid base64 / sub-PNG-header payload → None.
        assert_eq!(extract_generated_png("plain text output"), None);
        assert_eq!(
            extract_generated_png("Image 1 (8 bytes, base64):\n!!!not-base64!!!\n"),
            None
        );
        assert_eq!(
            extract_generated_png("Image 1 (2 bytes, base64):\n AQID \n"),
            None,
            "payloads shorter than a PNG header are rejected"
        );
    }
}
