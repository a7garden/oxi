//! Welcome banner formatting.

/// Format a clean startup message.
pub(crate) fn format_welcome(_session_id: &str, _model_id: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!("\n  oxi v{}\n\n", version)
}
