// PagerState — single source of truth for the pager.
//
// Wrapped in `Arc<RwLock<...>>` for sharing between the main loop
// (writer) and the render path (reader). All sub-states have `Default`
// derived so `PagerState::default()` is a valid initial state.

use parking_lot::RwLock;
use std::sync::Arc;

/// Top-level pager state.
#[derive(Default)]
pub struct PagerState {
    pub scrollback: ScrollbackState,
    pub prompt: PromptState,
    pub status: StatusState,
    pub agent_meta: AgentMetaState,
    pub sticky_panels: StickyPanelState,
    /// Active modal — `None` when no overlay is open. Filled in PR-6.
    pub modal: Option<ModalKind>,
}

/// Shared pager state handle: `Arc<parking_lot::RwLock<PagerState>>`.
///
/// The pager's main loop holds the writer, the render path holds a
/// reader. `parking_lot::MutexGuard` is `!Send` — drop the guard before
/// any `.await` (AGENTS.md pitfall).
pub type SharedState = Arc<RwLock<PagerState>>;

/// Scrolled message history. Stub for PR-2; filled in PR-5.
#[derive(Default, Debug, Clone)]
pub struct ScrollbackState {}

/// User prompt input state.
#[derive(Default, Debug, Clone)]
pub struct PromptState {
    pub text: String,
    pub cursor: usize,
}

/// Footer / status bar state.
#[derive(Default, Debug, Clone)]
pub struct StatusState {
    pub spinner_phase: u8,
    pub last_error: Option<String>,
}

/// Agent session metadata.
#[derive(Default, Debug, Clone)]
pub struct AgentMetaState {
    pub session_id: Option<String>,
    pub model: Option<String>,
}

/// Sticky panel visibility (toggled via Ctrl+T / Ctrl+I / Ctrl+H / Ctrl+L
/// per spec §5.6).
#[derive(Default, Debug, Clone)]
pub struct StickyPanelState {
    pub todo: bool,
    pub issues: bool,
    pub hub: bool,
    pub lsp: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ModalKind {
    #[default]
    None,
    Ask,
    ModelSelect,
    ProviderSelect,
    Settings,
    Extensions,
    McpDashboard,
    McpConfig,
    Issues,
    Roles,
    Router,
    Skill,
    ToolConfirm,
}

