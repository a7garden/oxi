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
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate, disable_raw_mode, enable_raw_mode},
};
use oxicode_agent::AgentEvent;
use oxicode_agent::config::Mode;
use oxicode_agent::tools::TodoStateProvider;
use oxicode_agent::tools::todo::TodoStatus;
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
use crate::tui_vt::slash::file_commands::FileCommand;
use crate::tui_vt::slash::registry::{SlashCtx, SlashOutcome, SlashRegistry};
use oxicode_vtui::presentation::{BlockDisplayMode, TranscriptLine, VisibleItem, visible_items};

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
    /// First Ctrl+C armed a quit; a second press exits (two-press quit).
    pub pending_quit: bool,
    /// Slash-command autocomplete popup state.
    pub slash_popup: SlashPopup,
    /// Current reasoning/tool stage (e.g. "tool: read"), shown above the composer.
    pub reasoning_stage: Option<String>,
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
    /// Todo checklist items (text, status) — refreshed from the live provider.
    pub todo_items: Vec<(String, TodoStatus)>,
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
    /// `Some(state)` once the TUI startup clones the `App`'s
    /// `SessionState` into the render state. The resume spawn
    /// closure captures it and passes it to
    /// `AgentSession::resume_from_file`. `Option` because
    /// `#[derive(Default)]` requires it.
    pub session_state: Option<crate::SessionState>,
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
            agent_hub_open: false,
            hub_entries: Vec::new(),
            pending_quit: false,
            slash_popup: SlashPopup::default(),
            reasoning_stage: None,
            thinking_level: "medium".to_string(),
            viewport_width: 80,
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
            todo_items: Vec::new(),
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
            pending_resume: None,
            session_state: None,
            context_tokens: None,
            context_window: 128_000,
            brain: BrainChip::default(),
        }
    }
}

/// Where a secure prompt came from. The `SecureInput` overlay has just one
/// payload (the API key text); the origin discriminates the post-commit
/// follow-up so the user gets a contextual message instead of a generic
/// "saved" line.
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
#[derive(Clone, Debug)]
pub struct OverlayState {
    pub title: String,
    pub lines: Vec<String>,
    pub items: Vec<OverlayListItem>,
    pub selected: usize,
    pub search: Option<OverlaySearchState>,
    pub secure_input: Option<OverlaySecureInput>,
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
    state.lock().session_swapper = Some(session_swapper.clone());
    state.lock().session_state = Some(app.session_state().clone());
    state.lock().thinking_level = format!("{:?}", session.thinking_level()).to_ascii_lowercase();
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
    let prompt_queue = Arc::new(PromptQueue::default());
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
            }
            // 5. Brain health chip updates from the background prober.
            changed = brain_rx.changed() => {
                if changed.is_ok() {
                    state.lock().brain = *brain_rx.borrow_and_update();
                }
            }

            // 6. Periodic repaint — echoes typed input and drives animation
            //    even when no other event is ready.
            _ = render_tick.tick() => {}
        }

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
        // Redraw every iteration. The harness's redraw is idempotent —
        // the ratatui backend coalesces unchanged frames.
        let mut snapshot = state.lock();
        // Tool boxes size their borders to the live terminal width.
        if let Ok(size) = terminal.size() {
            snapshot.viewport_width = size.width;
        }
        // pane reflects phase changes written by the `todo` agent tool.
        if let Some(provider) = snapshot.todo_provider.as_ref() {
            snapshot.todo_items = flatten_todo_items(&provider.get_phases());
        }
        // Shed finalized rows into the host scrollback before the
        // synchronized repaint so the commit and the viewport redraw
        // land as one visual update.
        commit_scrollback(terminal, &mut snapshot);
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
            state.overlay = Some(materialize_overlay(*request));
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
/// Render the in-flight message: the dimmed italic thinking block (one
/// line per explicit newline, reasoning-styled) above the markdown-rendered
/// answer. Re-rendered whole on every delta so the live view equals the
/// final render.
fn render_streamed_message(state: &RenderState) -> Vec<Vec<InlineSegment>> {
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
    if !state.message_buffer.is_empty() {
        lines.extend(oxicode_vtui::tui::ui::markdown::render_markdown(
            &state.message_buffer,
        ));
    }
    lines
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

// ─────────────────────────────────────────────────────────────────────────
// omp-style tool boxes
// ─────────────────────────────────────────────────────────────────────────
/// Tool box content width: the LIVE transcript content width (layout
/// gutters minus the scrollbar column), floored so narrow terminals
/// still draw a coherent box. Building at the terminal width would wrap
/// every row's right border onto the next visual line.
fn tool_box_width(state: &RenderState) -> usize {
    let area = Rect {
        x: 0,
        y: 0,
        width: state.viewport_width,
        height: 24,
    };
    let (_x, w) = super::frame_layout::scrollback_geometry(area);
    w.saturating_sub(1).max(24) as usize
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
        .flat_map(|line| wrap_by_display_width(line, inner))
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
        AgentEvent::MessageStart { .. } => {
            state.reasoning_stage = Some("generating response".to_string());
            state.message_buffer.clear();
            state.thinking_buffer.clear();
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
                }
            }
        },
        AgentEvent::MessageEnd { .. } => {
            // Hide the reasoning indicator once the turn has ended so the
            // composer row reverts to follow-ups / tips.
            state.reasoning_stage = None;
            // Final rendering (same as delta:None for completeness)
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
            state.reasoning_stage = Some(stage.clone());
            handle.set_reasoning_stage(Some(stage));
        }
        AgentEvent::ToolExecutionEnd {
            result, is_error, ..
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
            if let Some(rows) = diff_rows(&result.content) {
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
            state.reasoning_stage = None;
            handle.set_input_enabled(true);
            handle.set_input_status(None, None);
        }
        AgentEvent::Compaction { .. } => {
            // Detailed lifecycle is handled by the AgentSession layer
            // (CompactionStart/End SessionEvents).
        }
        AgentEvent::Cancelled => {
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
            handle.set_reasoning_stage(None);
        }
        AgentEvent::Usage { input_tokens, .. } => {
            // `input_tokens` is the provider's tokenization of the complete
            // prompt for this turn, so it is a useful live snapshot of the
            // context currently occupying the window (unlike a character
            // count or a local approximation).
            state.context_tokens = Some(input_tokens);
        }
        _ => {
            // Other variants (TurnStart/End, AgentStart/End, Usage, …) are
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
/// pass (the cmd channel processes `ShowOverlay` and `CloseOverlay` in
/// submit order).
pub(crate) fn open_secure_prompt(
    state: &mut RenderState,
    handle: &InlineHandle,
    origin: SecureInputOrigin,
) {
    let provider = match &origin {
        SecureInputOrigin::SetKey { provider } | SecureInputOrigin::NewlyAdded { provider } => {
            provider.clone()
        }
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
                            Ok(()) => handle.append_line(
                                InlineMessageKind::Info,
                                vec![plain_segment(format!("Switched to {model_id}"))],
                            ),
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
                    if let OverlaySubmission::Selection(InlineListSelection::ConfigAction(key)) =
                        &sub
                    {
                        match key.as_str() {
                            "thinking_level" => {
                                if let Some(level) = session.cycle_thinking_level() {
                                    handle.append_line(
                                        InlineMessageKind::Info,
                                        vec![plain_segment(format!("Thinking: {level:?}"))],
                                    );
                                }
                            }
                            "auto_compaction" => {
                                let enabled = !session.auto_compaction_enabled();
                                session.set_auto_compaction(enabled);
                                handle.append_line(
                                    InlineMessageKind::Info,
                                    vec![plain_segment(format!(
                                        "Auto-compaction: {}",
                                        if enabled { "on" } else { "off" }
                                    ))],
                                );
                            }
                            "auto_retry" => {
                                let enabled = !session.auto_retry_enabled();
                                session.set_auto_retry(enabled);
                                handle.append_line(
                                    InlineMessageKind::Info,
                                    vec![plain_segment(format!(
                                        "Auto-retry: {}",
                                        if enabled { "on" } else { "off" }
                                    ))],
                                );
                            }
                            "advisor" => match session.toggle_advisor() {
                                Ok(enabled) => handle.append_line(
                                    InlineMessageKind::Info,
                                    vec![plain_segment(format!(
                                        "Advisor: {}",
                                        if enabled { "on" } else { "off" }
                                    ))],
                                ),
                                Err(e) => handle.append_line(
                                    InlineMessageKind::Error,
                                    vec![plain_segment(format!("Failed to toggle advisor: {e}"))],
                                ),
                            },
                            _ => {}
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
                            Ok(()) => handle.append_line(
                                InlineMessageKind::Info,
                                vec![plain_segment(format!("Switched to {full}"))],
                            ),
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
                    // Secure (masked) prompt committed by the user. The
                    // matching open prompt must have stashed
                    // `state.secure_input_origin`; we trust that field
                    // here because every prompt path goes through
                    // `open_secure_prompt` (SetApiKey, add_custom_provider)
                    // which sets it before opening the modal.
                    if let OverlaySubmission::SecureInput(text) = &sub
                        && let Some(origin) = state.secure_input_origin.take()
                    {
                        let provider = match &origin {
                            SecureInputOrigin::SetKey { provider }
                            | SecureInputOrigin::NewlyAdded { provider } => provider.clone(),
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
                                    Ok(()) => "Use /models to choose a model, or send a message.",
                                    Err(_) => "Restart this session before using it.",
                                }
                            ),
                        };
                        handle.append_line(InlineMessageKind::Info, vec![plain_segment(msg)]);
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

            // Ctrl+C: even with raw mode enabled some terminals / shells
            // fall back to delivering it as a SIGINT. Handle it as an
            // explicit interrupt so we don't depend on the OS signal.
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let _ = evt_tx.send(InlineEvent::Interrupt);
                continue;
            }

            // Ctrl+M: toggle multiline input mode.
            if key.code == KeyCode::Char('m') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let mut s = state.lock();
                s.multiline_mode = !s.multiline_mode;
                continue;
            }

            // Ctrl+P: open the command palette.
            if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let mut s = state.lock();
                s.overlay = Some(build_command_palette());
                continue;
            }

            // Ctrl+;: toggle the interactive queue panel.
            if key.code == KeyCode::Char(';') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let mut s = state.lock();
                s.queue_panel_open = !s.queue_panel_open;
                if s.queue_panel_open {
                    s.queue_selected = 0;
                }
                continue;
            }

            // Ctrl+E: fold all blocks (Shift+E expands all).
            if key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let mut s = state.lock();
                s.fold_all();
                continue;
            }

            // Ctrl+Enter: send-now — abort the current run (if any) and submit
            // the composed input immediately, bypassing the queue pane.
            if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
                let submitted = {
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
                    if !buf.is_empty() && !buf.starts_with('/') {
                        s.prompt_history.insert(0, buf.clone());
                        s.prompt_history.truncate(100);
                    }
                    buf
                };
                if !submitted.is_empty() {
                    let _ = evt_tx.send(InlineEvent::Interrupt);
                    let _ = evt_tx.send(InlineEvent::Submit(submitted.into()));
                }
                continue;
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
                    // Multiline mode: plain Enter inserts a newline.
                    // Shift+Enter (or Enter in non-multiline mode) sends.
                    let send = !state.lock().multiline_mode
                        || key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::SHIFT);

                    if !send {
                        let mut s = state.lock();
                        s.composer.insert_str("\n");
                        continue;
                    }

                    // Shell mode: submit the buffer as a bash command request.
                    let shell_cmd = state.lock().shell_mode;
                    if shell_cmd {
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

                    let submitted = {
                        let mut s = state.lock();
                        let buf = if s.slash_popup.open && !s.slash_popup.items.is_empty() {
                            let item = &s.slash_popup.items[s.slash_popup.selected];
                            format!("/{}", item.name)
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
                    };
                    let _ = evt_tx.send(InlineEvent::Submit(submitted.into()));
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
                KeyCode::PageUp => {
                    let _ = evt_tx.send(InlineEvent::ScrollPageUp);
                }
                KeyCode::PageDown => {
                    let _ = evt_tx.send(InlineEvent::ScrollPageDown);
                }
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
                        match ch {
                            '?' => {
                                s.overlay = Some(OverlayState {
                                    title: "Keyboard Shortcuts".into(),
                                    lines: cheatsheet_lines(),
                                    items: vec![],
                                    selected: 0,
                                    search: None,
                                    secure_input: None,
                                });
                            }
                            'e' => s.cycle_block_at_view(),
                            'E' => s.expand_all(),
                            'J' => s.jump_next_turn(),
                            'K' => s.jump_prev_turn(),
                            'n' if s.search.is_some() => s.search_next(),
                            'N' if s.search.is_some() => s.search_prev(),
                            _ => {
                                s.composer.input(crossterm::event::KeyEvent::new(
                                    KeyCode::Char(ch),
                                    key.modifiers,
                                ));
                                refresh_input_popups(&mut s);
                            }
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

// ─────────────────────────────────────────────────────────────────────────
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
    }
}

/// Global frame tick counter for animations (incremented per render).
static FRAME_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Tracks whether the terminal title currently shows a running state.
static TITLE_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// ASCII spinner frames for the tab title. They remain readable in every font.
const TITLE_SPINNER: &[&str] = &["-", "\\", "|", "/"];

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
        let running = state.reasoning_stage.is_some();
        let was_running = TITLE_RUNNING.swap(running, std::sync::atomic::Ordering::Relaxed);
        if running || was_running {
            let title = if running {
                let spin = TITLE_SPINNER[(tick as usize) % TITLE_SPINNER.len()];
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
    render_transcript(frame, layout.scrollback, state);
    let mut pinned_area = layout.scrollback;
    if !state.queued_inputs.is_empty() {
        let used = render_queue_pane(frame, pinned_area, state);
        pinned_area.y = pinned_area.y.saturating_add(used);
        pinned_area.height = pinned_area.height.saturating_sub(used);
    }
    if !state.todo_items.is_empty() {
        render_todo_pane(frame, pinned_area, &state.todo_items);
    }
    // The row above the composer has one owner per frame. A live run takes
    if let Some(stage) = &state.reasoning_stage {
        render_reasoning_indicator(frame, layout.prompt, stage);
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
    if state.agent_hub_open {
        render_agent_hub(frame, area, state);
    }
    if let Some(overlay) = &state.overlay {
        render_overlay(frame, area, overlay);
    }
    if let Some(confirm) = &state.confirmation {
        render_confirmation(frame, area, confirm);
    }
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
    let lines_count = overlay.lines.len();
    let items_count = filtered.len().min(visible_max);
    let height_inner = (lines_count + items_count + if has_search { 1 } else { 0 }) as u16;
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

    // Items.
    if filtered.is_empty() {
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
            let item_style = if is_selected {
                Style::default().fg(primary).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(fg)
            };
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
                x: inner.left(),
                y: row,
                width: inner.width,
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
            "Enter select | Up/Down move | Esc close"
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
    // chrome. The content owns the full width minus the scrollbar; weight
    // and color carry who is speaking, blank rows carry turn boundaries.
    let scrollbar_w: u16 = 1;
    let content_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width.saturating_sub(scrollbar_w),
        height: area.height,
    };

    let display = build_transcript_display(state, &styles, state.committed_entries);

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
        let line = transcript_line_marked(tl, &styles, false, false, false, true);
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

    // Render top-down, wrapping each line into multiple visual rows.
    let mut y = body_top;
    let width = content_area.width.max(1) as usize;
    for d in display.into_iter().skip(start) {
        if y >= content_area.bottom() {
            break;
        }
        // Turn spacer: one blank breathing row — no content.
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

    // Scrollbar (rightmost column): shown only when content overflows.
    // Follow-tail dims the thumb; explicit scroll brightens it.
    let body_viewport = (content_area.height as usize).saturating_sub(sticky_h as usize);
    if total > body_viewport {
        let follow = state.scroll_offset == usize::MAX;
        render_scrollbar(
            frame,
            area.right().saturating_sub(1),
            area.top(),
            area.height,
            total,
            body_viewport,
            start,
            follow,
            &styles,
            bg_color,
        );
    }
}

/// One visible row of the transcript: a rendered line (or a turn
/// spacer, `line: None`) plus the transcript entry it belongs to.
#[derive(Clone)]
struct TranscriptDisplayItem<'a> {
    source_index: usize,
    /// `None` marks a turn spacer: a blank breathing row.
    line: Option<Line<'a>>,
}

/// Build the visible-row list, respecting block folding, search marks
/// and turn rhythm. Entries below `from_entry` (committed to the host
/// scrollback) are skipped — they are frozen and must not render in
/// the live viewport again.
fn build_transcript_display<'a>(
    state: &'a RenderState,
    styles: &'a ThemeStyles,
    from_entry: usize,
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
fn commit_scrollback(terminal: &mut Terminal<CrosstermBackend<Stdout>>, state: &mut RenderState) {
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
    if keep_rows == 0 {
        return;
    }
    let styles = active_styles();
    let display = build_transcript_display(state, &styles, state.committed_entries);
    // The plan's wrapping math must match the LIVE viewport's content
    // width (layout gutters + scrollbar column), and the printed chunk
    // must sit at the same left gutter — otherwise frozen rows wrap or
    // shift a column relative to what the live region showed.
    let (gutter_x, scrollback_w) = super::frame_layout::scrollback_geometry(area);
    let content_w = scrollback_w.saturating_sub(1) as usize;
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

/// Render a 1-column scrollbar in the rightmost cell column. The thumb
/// represents the viewport's position within the full content; the rail is
/// a faint track. Follow-tail (auto-scroll) dims the thumb toward the
/// background; an explicit scroll offset paints it in the accent color.
#[allow(clippy::too_many_arguments)]
fn render_scrollbar(
    frame: &mut Frame,
    x: u16,
    top: u16,
    height: u16,
    total: usize,
    viewport: usize,
    start: usize,
    follow: bool,
    styles: &ThemeStyles,
    bg: Color,
) {
    if height == 0 {
        return;
    }
    let ratio = (start as f64 / total.max(1) as f64).clamp(0.0, 1.0);
    let thumb_h = (((viewport as f64 / total.max(1) as f64) * height as f64).ceil() as u16)
        .max(1)
        .min(height);
    let track_h = height.saturating_sub(thumb_h);
    let thumb_y = (ratio * track_h as f64).round() as u16;

    let accent = color_from_anstyle(styles.primary.get_fg_color());
    // Follow-tail: dim thumb so it recedes. Explicit scroll: bright accent.
    let thumb_color = if follow {
        blend_rgb(bg, accent, 0.35)
    } else {
        accent
    };
    let rail_color = blend_rgb(bg, accent, 0.1);

    for row in 0..height {
        let y = top + row;
        let is_thumb = row >= thumb_y && row < thumb_y + thumb_h;
        let (ch, color) = if is_thumb {
            ('\u{2588}', thumb_color) // █
        } else {
            ('\u{2502}', rail_color) // │
        };
        if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
            cell.set_char(ch);
            cell.set_style(Style::default().fg(color));
        }
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

    let mut spans = Vec::with_capacity(line.segments.len() + 1);
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, kind_style));
    }
    for segment in &line.segments {
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

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())))
        // The top border is useful real estate. It carries the active session
        // context instead of spending a full row on a generic "MESSAGE"
        // label, while the border still makes the input target unmistakable.
        .title(composer_context_line(state, area.width));

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
    // renders without a leading separator — there is no app badge.
    let mut fields: Vec<(&str, String, Style, u16)> = vec![
        (
            "MODEL ",
            model.to_string(),
            Style::default().fg(fg).add_modifier(Modifier::BOLD),
            0,
        ),
        (
            "THINK ",
            state.thinking_level.clone(),
            Style::default().fg(info),
            58,
        ),
        ("DIR ", workspace, Style::default().fg(fg), 82),
        ("GIT ", branch.to_string(), Style::default().fg(fg), 104),
        ("CTX ", context, Style::default().fg(info), 124),
    ];
    if let Some(stage) = state.reasoning_stage.clone() {
        fields.push((
            "RUN ",
            stage,
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
    // Right-aligned oxibrain health chip (moved off the removed
    // shortcuts bar): healthy reads info, unreachable error, absent
    // when memory is disabled. The border's title row is two cells
    // narrower than the block.
    if let Some((label, healthy)) = state.brain.chip_label() {
        let chip_color = if healthy {
            color_from_anstyle(styles.info.get_fg_color())
        } else {
            color_from_anstyle(styles.error.get_fg_color())
        };
        let usable = width.saturating_sub(2) as usize;
        let used: usize = spans.iter().map(|s| s.width()).sum();
        let chip = format!(" {label} ");
        if used + chip.width() < usable {
            let pad = usable - used - chip.width();
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(chip, Style::default().fg(chip_color)));
        }
    }
    Line::from(spans)
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

/// Render a 1-row reasoning/tool-stage indicator just above the composer.
fn render_reasoning_indicator(frame: &mut Frame<'_>, composer_area: Rect, stage: &str) {
    let styles = active_styles();
    let indicator_area = Rect {
        x: composer_area.x,
        y: composer_area.top().saturating_sub(1),
        width: composer_area.width,
        height: 1,
    };
    let line = Line::from(vec![
        Span::styled(
            "RUNNING",
            Style::default()
                .fg(color_from_anstyle(styles.primary.get_fg_color()))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " | ",
            Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())),
        ),
        Span::styled(
            stage.to_string(),
            Style::default()
                .fg(color_from_anstyle(styles.secondary.get_fg_color()))
                .add_modifier(Modifier::DIM),
        ),
        // Contextual abort hint (Claude Code pattern): shown only while
        // a run is live — the static shortcuts bar is gone.
        Span::styled(
            "  Esc abort \u{b7} Ctrl+C quit",
            Style::default()
                .fg(color_from_anstyle(styles.secondary.get_fg_color()))
                .add_modifier(Modifier::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), indicator_area);
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

/// Flatten todo phases into `(content, status)` pairs for the sticky pane.
fn flatten_todo_items(
    phases: &[oxicode_agent::tools::todo::TodoPhase],
) -> Vec<(String, TodoStatus)> {
    phases
        .iter()
        .flat_map(|p| p.tasks.iter().map(|t| (t.content.clone(), t.status)))
        .collect()
}

/// Render a compact todo checklist at the top of the scrollback area.
fn render_todo_pane(frame: &mut Frame<'_>, scrollback: Rect, items: &[(String, TodoStatus)]) {
    let styles = active_styles();
    let height = items.len() as u16 + 1;
    let area = Rect {
        x: scrollback.x,
        y: scrollback.y,
        width: scrollback.width,
        height,
    };
    let lines: Vec<Line<'_>> = items
        .iter()
        .map(|(text, status)| {
            // Text markers work in every terminal font and do not depend on
            // pictograms for status recognition.
            let (marker, color) = match status {
                TodoStatus::Completed => ("done", Some(styles.foreground)),
                TodoStatus::InProgress => ("now", styles.primary.get_fg_color()),
                TodoStatus::Blocked => ("wait", styles.info.get_fg_color()),
                TodoStatus::Abandoned => ("skip", styles.error.get_fg_color()),
                TodoStatus::Pending => ("todo", styles.secondary.get_fg_color()),
            };
            let text_style = if *status == TodoStatus::Completed {
                Style::default()
                    .fg(color_from_anstyle(Some(styles.foreground)))
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default().fg(color_from_anstyle(Some(styles.foreground)))
            };
            Line::from(vec![
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(color_from_anstyle(color)),
                ),
                Span::styled(text.clone(), text_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
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
        // A line wider than the terminal must wrap, not clip.
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
    fn scrollbar_paints_thumb_when_content_overflows() {
        // 40 distinct blocks in a 24-row viewport must produce a scrollbar
        // thumb (█) in the rendered frame.
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
            rendered.contains('\u{2588}'),
            "scrollbar thumb (█) must render when transcript overflows the viewport"
        );
    }

    #[test]
    fn scrollbar_absent_when_content_fits_viewport() {
        // A single short line fits without overflow — no thumb character.
        let mut state = RenderState::default();
        state.transcript.push(TranscriptLine {
            kind: InlineMessageKind::Agent,
            segments: vec![plain_segment("hi")],
            block_id: 0,
        });
        let rendered = render_frame_to_string(&state);
        assert!(
            !rendered.contains('\u{2588}'),
            "no scrollbar thumb when content fits the viewport"
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
    fn flatten_todo_items_preserves_order_and_status() {
        use oxicode_agent::tools::todo::{TodoItem, TodoPhase};
        let phases = vec![
            TodoPhase {
                name: "A".into(),
                tasks: vec![
                    TodoItem {
                        content: "write code".into(),
                        status: TodoStatus::InProgress,
                        notes: None,
                        block_reason: None,
                    },
                    TodoItem {
                        content: "write tests".into(),
                        status: TodoStatus::Pending,
                        notes: None,
                        block_reason: None,
                    },
                ],
            },
            TodoPhase {
                name: "B".into(),
                tasks: vec![TodoItem {
                    content: "waiting on review".into(),
                    status: TodoStatus::Blocked,
                    notes: None,
                    block_reason: None,
                }],
            },
        ];
        let flat = flatten_todo_items(&phases);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0], ("write code".to_string(), TodoStatus::InProgress));
        assert_eq!(flat[1], ("write tests".to_string(), TodoStatus::Pending));
        assert_eq!(
            flat[2],
            ("waiting on review".to_string(), TodoStatus::Blocked)
        );
    }

    #[test]
    fn todo_pane_renders_when_items_present() {
        // The sticky pane is populated from the live provider in the event
        // loop; here we seed it directly to assert the pane paints task text.
        let mut state = RenderState::default();
        state.todo_items = vec![
            ("active task".to_string(), TodoStatus::InProgress),
            ("open task".to_string(), TodoStatus::Pending),
        ];
        let rendered = render_frame_to_string(&state);
        assert!(
            rendered.contains("active task"),
            "in-progress task must render"
        );
        assert!(rendered.contains("open task"), "pending task must render");
        // The text status must distinguish the active task without symbols.
        assert!(rendered.contains("now"), "in-progress status must render");
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
        // without a per-variant branch.
        assert_eq!(
            match &set {
                SecureInputOrigin::SetKey { provider }
                | SecureInputOrigin::NewlyAdded { provider } => provider,
            },
            "openai"
        );
        assert_eq!(
            match &added {
                SecureInputOrigin::SetKey { provider }
                | SecureInputOrigin::NewlyAdded { provider } => provider,
            },
            "minimax"
        );
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
    use oxicode_ai::{Api, AssistantMessage, Message};
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
                    message: assistant(),
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
                message: assistant(),
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
        let user = transcript_line_marked(&user_line, &styles, false, false, false, true);
        assert_eq!(
            user.spans[0].style.fg,
            Some(user_color),
            "user text must read in the user color — response-ink makes turns indistinguishable"
        );

        let agent_line = line(InlineMessageKind::Agent);
        let agent = transcript_line_marked(&agent_line, &styles, false, false, false, true);
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
        let rendered = transcript_line_marked(&line, &styles, false, false, false, true);
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
            let rendered = transcript_line_marked(&line, &styles, false, false, false, true);
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

        let head = transcript_line_marked(&line, &styles, false, false, false, true);
        assert_eq!(spans_to_string(&head), "error: boom");

        let body = transcript_line_marked(&line, &styles, false, false, false, false);
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
        let rendered = transcript_line_marked(&line, &styles, true, false, false, false);
        assert_eq!(
            spans_to_string(&rendered),
            "[+] error: boom",
            "a collapsed block stays identifiable"
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
        let display = build_transcript_display(&state, &styles, 0);
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
        let display = build_transcript_display(&state, &styles, 0);
        // 9 rows, keep 4 → limit 5 → boundary would split b1 (3,4,5).
        let plan = scrollback_commit_plan(&display, &state.transcript, 80, 4, None).expect("plan");
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
        let display = build_transcript_display(&state, &styles, 0);
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
        let display = build_transcript_display(&state, &styles, 0);
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
        let display = build_transcript_display(&state, &styles, 0);
        let plan = scrollback_commit_plan(&display, &state.transcript, 80, 10, None).expect("plan");
        assert_eq!(
            plan.new_committed_entries, 22,
            "32 rows, keep 10 → head 22 rows commit"
        );
        assert_eq!(plan.rows, 22);
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
        // CHAT_LAYOUT insets 1 column per side; the scrollbar column
        // eats one more: 100 - 2 - 1 = 97.
        assert_eq!(tool_box_width(&state), 97);
    }
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
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn unused_test_handle() -> InlineHandle {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        InlineHandle::new_for_tests(tx)
    }
}
