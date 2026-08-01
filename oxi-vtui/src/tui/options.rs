#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FullscreenInteractionSettings {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyboardProtocolSettings {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionSurface {
    #[default]
    Full,
}

impl From<SessionSurface> for crate::tui::config::types::UiSurfacePreference {
    fn from(_: SessionSurface) -> Self {
        crate::tui::config::types::UiSurfacePreference::default()
    }
}
