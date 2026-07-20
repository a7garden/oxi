// ToolProgressCard — structured progress for in-flight tool calls.
use crate::scrollback::BlockKind;

/// Compact display of an in-progress tool call.
pub fn tool_progress_line(kind: &BlockKind, elapsed_ms: u64) -> String {
    match kind {
        BlockKind::ToolCall { name, .. } => {
            format!("🔧 {name}... {:.1}s", elapsed_ms as f64 / 1000.0)
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_progress_line_shows_name() {
        let kind = BlockKind::ToolCall {
            name: "read".into(),
            call_id: "c1".into(),
        };
        let line = tool_progress_line(&kind, 2500);
        assert!(line.contains("read"));
        assert!(line.contains("2.5s"));
    }
}
