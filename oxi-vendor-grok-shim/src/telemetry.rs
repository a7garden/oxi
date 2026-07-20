//! Shim of `xai_grok_telemetry::events::TerminalTelemetry`.

/// Terminal telemetry data collected at startup.
#[derive(Debug, Clone, Default)]
pub struct TerminalTelemetry {
    pub terminal_emulator: Option<String>,
    pub terminal_version: Option<String>,
    pub color_depth: Option<u8>,
    pub supports_kitty_keyboard: bool,
    pub supports_synchronized_output: bool,
}
