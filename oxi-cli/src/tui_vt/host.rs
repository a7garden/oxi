//! Host adapter implementation — connects oxi's application state to vtcode-ui's HostAdapter.

use oxi_vtui::tui::host::{
    HostAdapter, HostSessionDefaults, NotificationProvider, ThemeProvider, WorkspaceInfoProvider,
};
use parking_lot::RwLock;
use std::sync::Arc;

/// Adapts oxi's application state to vtcode-ui's host contract.
pub struct OxiHostAdapter {
    pub workspace_name: String,
    pub workspace_root: Option<std::path::PathBuf>,
    pub settings: Arc<RwLock<crate::store::settings::Settings>>,
}

impl OxiHostAdapter {
    pub fn new(
        workspace_root: Option<std::path::PathBuf>,
        settings: Arc<RwLock<crate::store::settings::Settings>>,
    ) -> Self {
        let workspace_name = workspace_root
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "oxi".to_string());
        Self {
            workspace_name,
            workspace_root,
            settings,
        }
    }
}

impl WorkspaceInfoProvider for OxiHostAdapter {
    fn workspace_name(&self) -> String {
        self.workspace_name.clone()
    }

    fn workspace_root(&self) -> Option<std::path::PathBuf> {
        self.workspace_root.clone()
    }
}

impl NotificationProvider for OxiHostAdapter {
    fn set_terminal_focused(&self, _focused: bool) {
        // oxi does not currently use focus notifications.
    }
}

impl ThemeProvider for OxiHostAdapter {
    fn available_themes(&self) -> Vec<String> {
        oxi_vtui::theme::available_themes()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn active_theme_name(&self) -> Option<String> {
        let name = self.settings.read().get_theme_name();
        if name.is_empty() { None } else { Some(name) }
    }
}

impl HostAdapter for OxiHostAdapter {
    fn app_name(&self) -> String {
        "oxi".into()
    }

    fn session_defaults(&self) -> HostSessionDefaults {
        HostSessionDefaults::default()
    }
}

/// Resolve and activate the theme from settings.
pub fn activate_theme(settings: &crate::store::settings::Settings) {
    let theme_id = settings.get_theme_name();
    let theme_id = if theme_id.is_empty() {
        "ciapre-dark"
    } else {
        theme_id.as_str()
    };
    let _ = oxi_vtui::theme::set_active_theme(theme_id);
}
