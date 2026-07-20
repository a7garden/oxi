//! Shim of `xai_grok_shared::ui_config::UiConfig`.

/// UI configuration — theme, font size, etc.
#[derive(Debug, Clone, Default)]
pub struct UiConfig {
    pub theme: Option<String>,
    pub font_size: Option<u16>,
}
