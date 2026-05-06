//! Welcome banner formatting.

pub(crate) fn format_welcome(session_id: &str, model_id: &str) -> String {
    let line = "─".repeat(33);
    format!(
        "  ╭{line}╮\n  │  ◈ oxi — AI Coding Assistant   │\n  ╰{line}╯\n\n  Session  {session_id}\n  Model    {model_id}\n\n  /help for commands · Enter to send",
        line = line,
        session_id = session_id,
        model_id = model_id,
    )
}
