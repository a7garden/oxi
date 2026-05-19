//! Tool call renderer — formats tool calls and results for display.
//!
//! Provides specialized formatting for built-in tools (edit, bash, read, etc.)
//! based on the tool name, arguments JSON, and result text.
//!
//! Auto-detection: if a result looks like a unified diff, it's rendered
//! as a colored diff view even without explicit hints.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::text::truncate_to_width;
use crate::theme::ThemeStyles;

// ── Constants ────────────────────────────────────────────────────────────

/// Maximum lines to show in result preview
const RESULT_PREVIEW_LINES: usize = 5;
/// Maximum lines to show in diff view
const DIFF_PREVIEW_LINES: usize = 8;
/// Maximum lines to show for read output
const READ_PREVIEW_LINES: usize = 4;
/// Maximum content lines for generic results
const GENERIC_PREVIEW_LINES: usize = 4;

// ── Path utilities ───────────────────────────────────────────────────────

/// Shorten a file path by replacing home directory with ~.
pub fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str: String = home.to_string_lossy().into_owned();
        if let Some(rest) = path.strip_prefix(&*home_str) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

// ── JSON parsing helpers ──────────────────────────────────────────────────

/// Parse arguments JSON to extract common fields.
pub fn parse_tool_args(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or(Value::Null)
}

/// Extract a string field from parsed arguments.
pub fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)?.as_str()
}

/// Extract an optional path field from arguments (handles path/file_path).
pub fn get_path(args: &Value) -> Option<String> {
    args.get("path")
        .or_else(|| args.get("file_path"))
        .and_then(|v| v.as_str())
        .map(shorten_path)
}

/// Extract an integer field from arguments.
pub fn get_int(args: &Value, key: &str) -> Option<i64> {
    args.get(key)?.as_i64()
}

// ── Auto-detection ────────────────────────────────────────────────────────

/// Check if result text looks like a unified diff.
pub fn looks_like_diff(text: &str) -> bool {
    text.lines().take(5).any(|l| {
        l.starts_with("@@ -")
            || (l.starts_with('-') && !l.starts_with("--"))
            || (l.starts_with('+') && !l.starts_with("++"))
    })
}

/// Check if result indicates a command exit status.
pub fn has_exit_status(text: &str) -> bool {
    text.contains("Command exited with code")
        || text.contains("Command timed out")
        || text.contains("Command aborted")
}

/// Count diff stats from a unified diff text.
pub fn count_diff_stats(diff: &str) -> (u32, u32) {
    let mut added = 0u32;
    let mut removed = 0u32;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("--") {
            removed += 1;
        }
    }
    (added, removed)
}

// ── Call formatters ───────────────────────────────────────────────────────

/// Format a tool call header line.
fn format_call_header(name: &str, extra: &str, styles: &ThemeStyles) -> Line<'static> {
    let name_style = styles.accent.add_modifier(Modifier::BOLD);
    let extra_style = styles.muted;
    Line::from(vec![
        Span::styled(format!("{} ", name), name_style),
        Span::styled(extra.to_string(), extra_style),
    ])
}

/// Format the edit tool call.
pub fn format_edit_call(args: &Value, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let path = get_path(args);
    let path_display = path.unwrap_or_else(|| "?".to_string());

    // Count edits
    let edit_count = args
        .get("edits")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or_else(|| {
            // Legacy mode: check oldText
            if args.get("oldText").or(args.get("oldText")).is_some() {
                1
            } else {
                0
            }
        });

    let extra = if edit_count > 0 {
        format!(
            "{} ({} replacement{})",
            path_display,
            edit_count,
            if edit_count == 1 { "" } else { "s" }
        )
    } else {
        path_display
    };

    vec![format_call_header("edit", &extra, styles)]
}

/// Format the bash tool call.
pub fn format_bash_call(
    args: &Value,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let command = get_str(args, "command").unwrap_or("...");
    // Account for "$ " prefix (2 chars)
    let command_display = truncate_to_width(command, max_width.saturating_sub(2).max(20));

    let mut lines = vec![Line::from(vec![
        Span::styled("$ ", styles.accent.add_modifier(Modifier::BOLD)),
        Span::styled(command_display, styles.normal),
    ])];

    // Add timeout if present
    if let Some(timeout) = get_int(args, "timeout") {
        lines.push(Line::from(Span::styled(
            format!("  (timeout {}s)", timeout),
            styles.muted,
        )));
    }

    lines
}

/// Format the read tool call.
pub fn format_read_call(args: &Value, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let path = get_path(args).unwrap_or_else(|| "?".to_string());

    // Line range
    let offset = get_int(args, "offset");
    let limit = get_int(args, "limit");

    let extra = match (offset, limit) {
        (Some(o), Some(l)) => format!("{}:{}-{}", path, o, o + l - 1),
        (Some(o), None) => format!("{}:{}", path, o),
        (None, Some(l)) => format!("{}:1-{}", path, l),
        (None, None) => path,
    };

    vec![format_call_header("read", &extra, styles)]
}

/// Format the write tool call.
pub fn format_write_call(args: &Value, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let path = get_path(args).unwrap_or_else(|| "?".to_string());
    let extra = format!("{} (new)", path);
    vec![format_call_header("write", &extra, styles)]
}

/// Format search tools (grep, find, ls).
pub fn format_search_call(name: &str, args: &Value, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let icon = match name {
        "grep" => "⌕",
        "find" => "🔍",
        "ls" => "📁",
        _ => "○",
    };

    let path = get_path(args).unwrap_or_else(|| ".".to_string());

    // Pattern for grep
    let pattern = get_str(args, "pattern")
        .or_else(|| get_str(args, "query"))
        .map(|p| format!(" \"{}\"", truncate_to_width(p, 30)))
        .unwrap_or_default();

    let extra = format!("{}{} {}", icon, pattern, path);
    vec![Line::from(vec![
        Span::styled(
            format!("{} ", name),
            styles.accent.add_modifier(Modifier::BOLD),
        ),
        Span::styled(extra, styles.muted),
    ])]
}

/// Format a generic tool call (fallback for unknown tools).
pub fn format_generic_call(
    name: &str,
    args: &Value,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let name_style = styles.accent.add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("{} ", name),
        name_style,
    )])];

    // Show first few args as key: value
    if let Some(obj) = args.as_object() {
        for (key, v) in obj.iter().take(3) {
            let val_str = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let prefix_len = 2 + UnicodeWidthStr::width(key.as_str()) + 2;
            let avail = max_width.saturating_sub(prefix_len);
            let display = truncate_to_width(&val_str, avail);
            lines.push(Line::from(vec![
                Span::styled(format!("  {}", key), styles.muted),
                Span::styled(": ", styles.muted),
                Span::styled(display, styles.normal),
            ]));
        }
    }

    lines
}

/// Format a tool call by tool name.
pub fn format_tool_call(
    name: &str,
    arguments: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let args = parse_tool_args(arguments);

    match name {
        "edit" => format_edit_call(&args, styles),
        "bash" => format_bash_call(&args, max_width, styles),
        "read" => format_read_call(&args, styles),
        "write" => format_write_call(&args, styles),
        "grep" | "find" | "ls" => format_search_call(name, &args, styles),
        _ => format_generic_call(name, &args, max_width, styles),
    }
}

// ── Result formatters ─────────────────────────────────────────────────────

/// Format an error result.
pub fn format_error_result(
    error: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for line in error.lines().take(4) {
        let display = truncate_to_width(line, max_width);
        lines.push(Line::from(Span::styled(
            format!("  {}", display),
            styles.error,
        )));
    }

    if error.lines().count() > 4 {
        lines.push(Line::from(Span::styled("  \u{2026}", styles.muted)));
    }

    lines
}

/// Format a unified diff result with colors.
pub fn format_diff_result(
    diff: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let total_lines = diff.lines().count();

    for raw_line in diff.lines().take(DIFF_PREVIEW_LINES) {
        let line = if raw_line.starts_with("@@") {
            // Hunk header
            Line::from(Span::styled(
                truncate_to_width(raw_line, max_width),
                styles.muted,
            ))
        } else if raw_line.starts_with('-') && !raw_line.starts_with("--") {
            // Removed line — red
            Line::from(Span::styled(
                format!(
                    " {}",
                    truncate_to_width(raw_line, max_width.saturating_sub(1))
                ),
                styles.error,
            ))
        } else if raw_line.starts_with('+') && !raw_line.starts_with("++") {
            // Added line — green
            Line::from(Span::styled(
                format!(
                    " {}",
                    truncate_to_width(raw_line, max_width.saturating_sub(1))
                ),
                styles.success,
            ))
        } else {
            // Context line
            Line::from(Span::styled(
                format!(
                    " {}",
                    truncate_to_width(raw_line, max_width.saturating_sub(1))
                ),
                styles.muted,
            ))
        };
        lines.push(line);
    }

    // Show diff stats if significant
    let (added, removed) = count_diff_stats(diff);
    if total_lines > DIFF_PREVIEW_LINES {
        lines.push(Line::from(Span::styled(
            format!(
                "  \u{2026} ({} more lines)",
                total_lines - DIFF_PREVIEW_LINES
            ),
            styles.muted,
        )));
    } else if added > 0 || removed > 0 {
        lines.push(Line::from(Span::styled(
            format!("  [+{} / -{}]", added, removed),
            styles.muted,
        )));
    }

    lines
}

/// Format a bash command result.
pub fn format_bash_result(
    result: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Show last N lines of output
    let all_lines: Vec<&str> = result.lines().collect();
    let _preview_lines = if all_lines.len() > RESULT_PREVIEW_LINES {
        // Show last N lines
        let start = all_lines.len() - RESULT_PREVIEW_LINES;
        for line in &all_lines[start..] {
            let display = truncate_to_width(line, max_width.saturating_sub(2));
            lines.push(Line::from(Span::styled(
                format!("  {}", display),
                styles.normal,
            )));
        }
        if start > 0 {
            lines.insert(
                0,
                Line::from(Span::styled(
                    format!("  … ({} earlier lines)", start),
                    styles.muted,
                )),
            );
        }
        all_lines.len()
    } else {
        for line in &all_lines {
            let display = truncate_to_width(line, max_width.saturating_sub(2));
            lines.push(Line::from(Span::styled(
                format!("  {}", display),
                styles.normal,
            )));
        }
        all_lines.len()
    };

    // Note: "Full output:" truncation info is intentionally NOT appended
    // as a separate line here. When the bash output is large, the tool
    // result already includes this text, and it will appear in the preview
    // naturally. Adding it separately caused a height measurement mismatch
    // (format_bash_result produced more lines than measure_result_height
    // expected), leading to content clipping in bordered tool boxes.

    lines
}

/// Format a read tool result (file content preview).
pub fn format_read_result(
    result: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let all_lines: Vec<&str> = result.lines().collect();
    let total = all_lines.len();

    // Show first N lines with line numbers
    for (i, line) in all_lines.iter().take(READ_PREVIEW_LINES).enumerate() {
        let line_num = i + 1;
        let display = truncate_to_width(line, max_width.saturating_sub(10));
        lines.push(Line::from(vec![
            Span::styled(format!("{:4} ", line_num), styles.muted),
            Span::styled(display, styles.normal),
        ]));
    }

    if total > READ_PREVIEW_LINES {
        let remaining = total - READ_PREVIEW_LINES;
        lines.push(Line::from(Span::styled(
            format!("  \u{2026} ({} more lines, {} total)", remaining, total),
            styles.muted,
        )));
    } else if total > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ({} lines)", total),
            styles.muted,
        )));
    }

    lines
}

/// Format a generic tool result (fallback).
pub fn format_generic_result(
    result: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let all_lines: Vec<&str> = result.lines().collect();

    for line in all_lines.iter().take(GENERIC_PREVIEW_LINES) {
        let display = truncate_to_width(line, max_width.saturating_sub(2));
        lines.push(Line::from(Span::styled(
            format!("  {}", display),
            styles.normal,
        )));
    }

    if all_lines.len() > GENERIC_PREVIEW_LINES {
        lines.push(Line::from(Span::styled(
            format!(
                "  \u{2026} ({} more lines)",
                all_lines.len() - GENERIC_PREVIEW_LINES
            ),
            styles.muted,
        )));
    }

    lines
}

/// Format a tool result by tool name.
pub fn format_tool_result(
    name: &str,
    result: &str,
    is_error: bool,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    if is_error {
        return format_error_result(result, max_width, styles);
    }

    // Auto-detect diff if result contains diff markers
    if looks_like_diff(result) {
        return format_diff_result(result, max_width, styles);
    }

    match name {
        "edit" => format_diff_result(result, max_width, styles),
        "bash" => format_bash_result(result, max_width, styles),
        "read" => format_read_result(result, max_width, styles),
        _ => format_generic_result(result, max_width, styles),
    }
}

// ── Height calculation ─────────────────────────────────────────────────────

/// Calculate the rendered height for a tool call.
pub fn measure_call_height(name: &str, arguments: &str, max_width: usize) -> u16 {
    let args = parse_tool_args(arguments);

    match name {
        "edit" => format_edit_call(&args, &ThemeStyles::default()).len() as u16,
        "bash" => format_bash_call(&args, max_width, &ThemeStyles::default()).len() as u16,
        "read" => 1, // Always 1 line for read
        "write" => 1,
        "grep" | "find" | "ls" => 1,
        _ => {
            // Generic: 1 header + up to 3 args
            let args_count = args.as_object().map(|o| o.len().min(3)).unwrap_or(0);
            1 + args_count as u16
        }
    }
}

/// Calculate the rendered height for a tool result.
pub fn measure_result_height(name: &str, result: &str, is_error: bool) -> u16 {
    if is_error {
        let total = result.lines().count();
        let shown = total.min(4);
        return shown as u16 + if total > 4 { 1 } else { 0 };
    }

    if looks_like_diff(result) {
        let total = result.lines().count();
        let shown = total.min(DIFF_PREVIEW_LINES);
        let extra = if total > DIFF_PREVIEW_LINES
            || count_diff_stats(result).0 > 0
            || count_diff_stats(result).1 > 0
        {
            1
        } else {
            0
        };
        return shown as u16 + extra;
    }

    match name {
        "edit" => {
            let total = result.lines().count();
            let shown = total.min(DIFF_PREVIEW_LINES);
            let has_more = total > DIFF_PREVIEW_LINES;
            let has_stats =
                !has_more && (count_diff_stats(result).0 > 0 || count_diff_stats(result).1 > 0);
            let extra = if has_more || has_stats { 1 } else { 0 };
            shown as u16 + extra
        }
        "bash" => {
            let total = result.lines().count();
            let shown = total.min(RESULT_PREVIEW_LINES);
            let extra = if total > RESULT_PREVIEW_LINES { 1 } else { 0 };
            shown as u16 + extra
        }
        "read" => {
            let total = result.lines().count();
            let shown = total.min(READ_PREVIEW_LINES);
            let extra: u16 = 1; // Always show count
            shown as u16 + extra
        }
        _ => {
            let total = result.lines().count();
            let shown = total.min(GENERIC_PREVIEW_LINES);
            let extra = if total > GENERIC_PREVIEW_LINES { 1 } else { 0 };
            shown as u16 + extra
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorten_path() {
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        let path = format!("{}/src/main.rs", home);
        assert_eq!(shorten_path(&path), "~/src/main.rs");
    }

    #[test]
    fn test_parse_args() {
        // Use actual home directory so shorten_path works
        let home = dirs::home_dir().unwrap();
        let path = format!("{}/file.rs", home.to_string_lossy());
        let args = format!(r#"{{"path":"{}","offset":10,"limit":50}}"#, path);
        let parsed = parse_tool_args(&args);
        assert_eq!(get_path(&parsed), Some("~/file.rs".to_string()));
        assert_eq!(get_int(&parsed, "offset"), Some(10));
        assert_eq!(get_int(&parsed, "limit"), Some(50));
    }

    #[test]
    fn test_looks_like_diff() {
        assert!(looks_like_diff("@@ -10,3 +10,4 @@\n-old\n+new"));
        assert!(looks_like_diff("-removed\n+added"));
        assert!(!looks_like_diff("hello world\njust text"));
    }

    #[test]
    fn test_count_diff_stats() {
        let diff = "-removed\n-old\n+added\n+new";
        let (added, removed) = count_diff_stats(diff);
        assert_eq!(added, 2);
        assert_eq!(removed, 2);
    }

    #[test]
    fn test_truncate_to_width() {
        let text = "hello world";
        assert_eq!(truncate_to_width(text, 100), "hello world");
        // For max_width=5: "hello" (5 chars) overflows, so truncate to 4 + ellipsis
        // This matches the behavior of the original truncate_str in chat.rs
        assert_eq!(truncate_to_width(text, 5), "hell…");
        assert_eq!(truncate_to_width(text, 0), "");
    }

    #[test]
    fn test_format_edit_call() {
        let args = serde_json::json!({
            "path": "/home/user/src/main.rs",
            "edits": [{"oldText": "foo", "newText": "bar"}]
        });
        let lines = format_edit_call(&args, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("edit"));
        assert!(lines[0].to_string().contains("main.rs"));
    }

    #[test]
    fn test_format_bash_call() {
        let args = serde_json::json!({
            "command": "cargo build --release",
            "timeout": 120
        });
        let lines = format_bash_call(&args, 80, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("$ cargo build"));
    }

    #[test]
    fn test_format_read_call() {
        let args = serde_json::json!({
            "path": "/home/user/src/main.rs",
            "offset": 10,
            "limit": 50
        });
        let lines = format_read_call(&args, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("read"));
        assert!(lines[0].to_string().contains("main.rs:10-59"));
    }

    #[test]
    fn test_format_diff_result() {
        let diff = "@@ -10,3 +10,4 @@\n-old line\n context\n+new line\n+extra line";
        let lines = format_diff_result(diff, 80, &ThemeStyles::default());
        // Should have hunk header + lines + stats
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_format_bash_result() {
        let output = "Compiling mycrate v0.1.0\nFinished release [optimized]";
        let lines = format_bash_result(output, 80, &ThemeStyles::default());
        assert!(!lines.is_empty());
        assert!(lines[0].to_string().contains("Compiling"));
    }

    #[test]
    fn test_format_read_result() {
        let content = "use std::io;\n\nfn main() {\n    println!(\"hello\");\n}\n";
        let lines = format_read_result(content, 80, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("1"));
        assert!(lines[0].to_string().contains("use std"));
    }

    #[test]
    fn test_format_generic_call() {
        let args = serde_json::json!({
            "param1": "value1",
            "param2": "value2"
        });
        let lines = format_generic_call("my_tool", &args, 80, &ThemeStyles::default());
        assert!(!lines.is_empty());
        assert!(lines[0].to_string().contains("my_tool"));
    }

    #[test]
    fn test_has_exit_status() {
        assert!(has_exit_status("Command exited with code 1"));
        assert!(has_exit_status("Command timed out after 120 seconds"));
        assert!(!has_exit_status("Everything is fine"));
    }
}
