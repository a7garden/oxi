//! Shim of `xai_grok_shared::session::FeedbackTerminalInfo`.

/// Terminal info attached to a feedback session report.
#[derive(Debug, Clone, Default)]
pub struct FeedbackTerminalInfo {
    pub term: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}
