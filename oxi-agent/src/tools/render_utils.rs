/// Tool output rendering utilities
//!
/// Provides helpers for formatting tool results for display.

/// Shorten a file path by replacing the home directory prefix with `~`.
pub fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

/// Replace tabs with spaces for display.
pub fn replace_tabs(text: &str) -> &str {
    // Fast path: if no tabs, return the original string
    if !text.contains('\t') {
        return text;
    }
    // Caller should use the owned version if tabs exist
    text
}

/// Replace tabs with spaces, returning an owned String.
pub fn replace_tabs_owned(text: &str) -> String {
    text.replace('\t', "   ")
}

/// Normalize display text by removing carriage returns.
pub fn normalize_display_text(text: &str) -> String {
    text.replace('\r', "")
}

/// Sanitize binary output by replacing non-printable bytes.
/// Keeps newlines, tabs, and printable ASCII. Replaces everything else.
pub fn sanitize_binary_output(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\n' || ch == '\t' || ch.is_ascii_graphic() || ch == ' ' {
            result.push(ch);
        } else if ch == '\r' {
            // Skip carriage returns
        } else {
            result.push('�');
        }
    }
    result
}

/// Extract text output from tool result content, stripping ANSI codes.
pub fn get_text_output(content: &str) -> String {
    let sanitized = sanitize_binary_output(content);
    normalize_display_text(&sanitized)
}

/// Format a string as a truncated preview for tool output display.
pub fn truncate_output_preview(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();

    if lines.len() <= max_lines && text.len() <= max_bytes {
        return text.to_string();
    }

    let selected: Vec<&str> = lines.into_iter().take(max_lines).collect();
    let mut result = selected.join("\n");

    if result.len() > max_bytes {
        result.truncate(max_bytes);
        // Ensure we don't cut in the middle of a multi-byte character
        while !result.is_char_boundary(result.len()) {
            result.pop();
        }
    }

    result
}

/// Format an "invalid argument" indicator for tool output.
pub fn invalid_arg_text() -> &'static str {
    "[invalid arg]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorten_path_with_home() {
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy().to_string();
        let path = format!("{}/foo/bar.txt", home_str);
        assert_eq!(shorten_path(&path), "~/foo/bar.txt");
    }

    #[test]
    fn test_shorten_path_no_home() {
        assert_eq!(shorten_path("/tmp/foo.txt"), "/tmp/foo.txt");
    }

    #[test]
    fn test_replace_tabs_no_tabs() {
        assert_eq!(replace_tabs("hello world"), "hello world");
    }

    #[test]
    fn test_replace_tabs_owned_with_tabs() {
        assert_eq!(replace_tabs_owned("hello\tworld"), "hello   world");
    }

    #[test]
    fn test_normalize_display_text() {
        assert_eq!(normalize_display_text("hello\r\nworld"), "hello\nworld");
        assert_eq!(normalize_display_text("no-cr"), "no-cr");
    }

    #[test]
    fn test_sanitize_binary_output() {
        assert_eq!(sanitize_binary_output("hello\nworld"), "hello\nworld");
        assert_eq!(sanitize_binary_output("tab\there"), "tab\there");

        // Non-printable character gets replaced
        let with_control = "hello\x01world";
        let result = sanitize_binary_output(with_control);
        assert!(result.contains('�'));
    }

    #[test]
    fn test_get_text_output() {
        assert_eq!(get_text_output("hello\r\nworld"), "hello\nworld");
    }

    #[test]
    fn test_truncate_output_preview_within_limits() {
        let text = "line1\nline2\nline3";
        assert_eq!(truncate_output_preview(text, 10, 1000), text);
    }

    #[test]
    fn test_truncate_output_preview_by_lines() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let result = truncate_output_preview(text, 2, 1000);
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
        assert!(!result.contains("line5"));
    }

    #[test]
    fn test_invalid_arg_text() {
        assert_eq!(invalid_arg_text(), "[invalid arg]");
    }
}
