//! Welcome banner formatting.

/// Format a clean startup message like Pi.
pub(crate) fn format_welcome(session_id: &str, model_id: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        " oxi v{}\n\n escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash\n\n",
        version
    )
}
