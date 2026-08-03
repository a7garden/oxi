use crate::theme::ThemeDefinition;
use ratatui::style::Style;

#[derive(Default)]
pub struct ThemeStyles {
    pub primary: Style,
    pub secondary: Style,
    pub response: Style,
    pub agent: Style,
    pub user: Style,
    pub tool: Style,
    pub tool_output: Style,
    pub tool_detail: Style,
    pub output: Style,
    pub error: Style,
    pub warning: Style,
    pub info: Style,
    pub code: Style,
    pub background: Style,
    pub foreground: Style,
    pub panel: Style,
    pub status: Style,
    pub border: Style,
    pub highlight: Style,
    pub dim: Style,
    pub pty: Style,
    pub reasoning: Style,
    pub action: Style,
}

pub fn available_theme_suites() -> Vec<()> {
    vec![]
}
pub fn theme_suite_id(_: &()) -> &'static str {
    "default"
}
pub fn theme_suite_label(_: &()) -> &'static str {
    "Default"
}
pub type DiffColorPalette = oxicode_vtui_compat::styling::DiffColorPalette;
