// PagerState — single source of truth for the pager.

use crate::prompt::PromptState;
use crate::scrollback::ScrollbackState;
use crate::status::StatusState;
use parking_lot::RwLock;
use ratatui::widgets::ListState;
use std::sync::Arc;

/// Top-level pager state.
#[derive(Default)]
pub struct PagerState {
    pub scrollback: ScrollbackState,
    /// Scroll state for the chat list.
    pub list_state: ListState,
    pub prompt: PromptState,
    pub status: StatusState,
    pub sticky_panels: StickyPanelState,
    pub modal: Option<ModalKind>,
    /// When `Some(instant)`, user pressed quit once within the 2s window.
    #[doc(hidden)]
    pub confirm_quit: Option<std::time::Instant>,
}

pub type SharedState = Arc<RwLock<PagerState>>;

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
