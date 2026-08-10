use std::path::PathBuf;
use std::sync::Arc;

pub use crate::tui::config::KeyboardProtocolConfig;
pub use crate::tui::core_tui::types::protocol::{
    InlineCommand, InlineEvent, InlineHandle, InlineSession,
};
pub use crate::tui::core_tui::types::{
    AuthAction, ContentPart, FocusChangeCallback, InlineEventCallback, InlineHeaderContext,
    InlineHeaderHighlight, InlineHeaderStatusBadge, InlineHeaderStatusTone, InlineLinkRange,
    InlineLinkTarget, InlineListItem, InlineListSearchConfig, InlineListSelection,
    InlineMessageKind, InlineSegment, InlineTextStyle, InlineTheme, ListOverlayRequest,
    ModalOverlayRequest, OverlayEvent, OverlayHotkey, OverlayHotkeyAction, OverlayHotkeyKey,
    OverlayRequest, OverlaySelectionChange, OverlaySubmission, RewindAction, SecurePromptConfig,
    SubmittedInput, WizardModalMode, WizardOverlayRequest, WizardStep,
};
pub use crate::tui::options::{
    FullscreenInteractionSettings, KeyboardProtocolSettings, SessionSurface,
};

pub type CoreCommand = InlineCommand;
pub type CoreEvent = InlineEvent;
pub type CoreHandle = InlineHandle;
pub type CoreSession = InlineSession;

/// Core session launch options for reusable TUI integrations.
#[derive(Clone)]
pub struct CoreSessionOptions {
    pub placeholder: Option<String>,
    pub surface_preference: SessionSurface,
    pub inline_rows: u16,
    pub event_callback: Option<InlineEventCallback>,
    pub focus_callback: Option<FocusChangeCallback>,
    pub workspace_root: Option<PathBuf>,
    pub app_name: String,
    pub non_interactive_hint: Option<String>,
}

impl Default for CoreSessionOptions {
    fn default() -> Self {
        Self {
            placeholder: None,
            surface_preference: SessionSurface::default(),
            inline_rows: 24,
            event_callback: None,
            focus_callback: None,
            workspace_root: None,
            app_name: "oxicode".to_string(),
            non_interactive_hint: None,
        }
    }
}

/// Spawn a core TUI session. Returns the session handle and event receiver.
/// The actual TUI event loop is implemented in oxicode-cli, not here.
pub fn spawn_core_session(
    _theme: InlineTheme,
    _options: CoreSessionOptions,
) -> anyhow::Result<(
    InlineHandle,
    tokio::sync::mpsc::UnboundedReceiver<InlineEvent>,
)> {
    // The real implementation lives in oxicode-cli's TUI harness.
    // This is a placeholder that the harness replaces.
    unimplemented!("spawn_core_session is implemented in oxicode-cli's TUI harness")
}

/// Commonly used core TUI API items.
pub mod prelude {
    pub use super::{
        CoreCommand, CoreEvent, CoreHandle, CoreSession, CoreSessionOptions,
        FullscreenInteractionSettings, InlineHeaderContext, InlineHeaderHighlight,
        InlineHeaderStatusBadge, InlineHeaderStatusTone, InlineMessageKind, InlineSegment,
        InlineTextStyle, InlineTheme, KeyboardProtocolSettings, ListOverlayRequest,
        ModalOverlayRequest, OverlayEvent, OverlayHotkey, OverlayHotkeyAction, OverlayHotkeyKey,
        OverlayRequest, OverlaySelectionChange, OverlaySubmission, SessionSurface, WizardModalMode,
        WizardOverlayRequest, WizardStep, spawn_core_session,
    };
}
