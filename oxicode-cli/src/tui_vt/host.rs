//! Host adapter implementation — connects oxicode's application state to vtcode-ui's HostAdapter.

use oxicode_vtui::tui::host::{
    HostAdapter, HostSessionDefaults, NotificationProvider, ThemeProvider, WorkspaceInfoProvider,
};
use parking_lot::RwLock;
use std::sync::Arc;

/// Adapts oxicode's application state to vtcode-ui's host contract.
pub struct OxicodeHostAdapter {
    pub workspace_name: String,
    pub workspace_root: Option<std::path::PathBuf>,
    pub settings: Arc<RwLock<crate::store::settings::Settings>>,
}

impl OxicodeHostAdapter {
    pub fn new(
        workspace_root: Option<std::path::PathBuf>,
        settings: Arc<RwLock<crate::store::settings::Settings>>,
    ) -> Self {
        let workspace_name = workspace_root
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "oxicode".to_string());
        Self {
            workspace_name,
            workspace_root,
            settings,
        }
    }
}

impl WorkspaceInfoProvider for OxicodeHostAdapter {
    fn workspace_name(&self) -> String {
        self.workspace_name.clone()
    }

    fn workspace_root(&self) -> Option<std::path::PathBuf> {
        self.workspace_root.clone()
    }
}

impl NotificationProvider for OxicodeHostAdapter {
    fn set_terminal_focused(&self, _focused: bool) {
        // oxicode does not currently use focus notifications.
    }
}

impl ThemeProvider for OxicodeHostAdapter {
    fn available_themes(&self) -> Vec<String> {
        oxicode_vtui::theme::available_themes()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn active_theme_name(&self) -> Option<String> {
        let name = self.settings.read().get_theme_name();
        if name.is_empty() { None } else { Some(name) }
    }
}

impl HostAdapter for OxicodeHostAdapter {
    fn app_name(&self) -> String {
        "oxicode".into()
    }

    fn session_defaults(&self) -> HostSessionDefaults {
        HostSessionDefaults::default()
    }
}

/// Resolve and activate the theme from settings.
pub fn activate_theme(settings: &crate::store::settings::Settings) {
    let theme_id = settings.get_theme_name();
    let theme_id = if theme_id.is_empty() {
        oxicode_vtui::theme::DEFAULT_THEME_ID
    } else {
        theme_id.as_str()
    };
    let _ = oxicode_vtui::theme::set_active_theme(theme_id);
}
