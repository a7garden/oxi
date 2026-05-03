//! Output truncation utilities for tool results
//!
//! Limits tool output by two independent limits (lines + bytes), whichever hits first.
//! UTF-8 safe: truncates at valid character boundaries.

/// Default max output lines
pub const DEFAULT_MAX_LINES: usize = 2000;
/// Default max output bytes (50KB)
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;
/// Max line length for grep output
pub const GREP_MAX_LINE_LENGTH: usize = 500;

/// Result of a truncation operation
#[derive(Debug, Clone)]
pub struct TruncationResult {
    /// The truncated content
    pub content: String,
    /// Whether truncation occurred
    pub truncated: bool,
    /// How truncation was applied
    pub truncated_by: TruncatedBy,
    /// Total lines in original content
    pub total_lines: usize,
    /// Total bytes in original content
    pub total_bytes: usize,
    /// Lines in output
    pub output_lines: usize,
    /// Bytes in output
    pub output_bytes: usize,
    /// Whether the first line of output is a partial line
    pub last_line_partial: bool,
    /// Whether the first line alone exceeds the byte limit
    pub first_line_exceeds_limit: bool,
}

/// How truncation was applied
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncatedBy {
    /// No truncation needed
    None,
    /// Truncated by line count
    Lines,
    /// Truncated by byte count
    Bytes,
}

/// Options for truncation
#[derive(Debug, Clone)]
pub struct TruncationOptions {
    /// Maximum number of lines to keep
    pub max_lines: Option<usize>,
    /// Maximum number of bytes to keep
    pub max_bytes: Option<usize>,
}

impl Default for TruncationOptions {
    fn default() -> Self {
        Self {
            max_lines: Some(DEFAULT_MAX_LINES),
            max_bytes: Some(DEFAULT_MAX_BYTES),
        }
    }
}

/// Truncate content from the head (keep first N lines/bytes).
/// Never produces partial lines unless the first line itself exceeds byte limit.
pub fn truncate_head(content: &str, options: &TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(usize::MAX);
    let max_bytes = options.max_bytes.unwrap_or(usize::MAX);
    
    let total_bytes = content.len();
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    
    // Check if already within limits
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: TruncatedBy::None,
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
        };
    }
    
    // Check if first line exceeds byte limit
    let first_line_bytes = lines.first().map(|l| l.len()).unwrap_or(0);
    let first_line_exceeds_limit = first_line_bytes > max_bytes;
    
    if first_line_exceeds_limit {
        // Can only include a partial first line
        let truncated_line = truncate_to_bytes(lines[0], max_bytes);
        let output_bytes = truncated_line.len();
        return TruncationResult {
            content: format!("{}\n\n... [truncated: first line exceeds byte limit]", truncated_line),
            truncated: true,
            truncated_by: TruncatedBy::Bytes,
            total_lines,
            total_bytes,
            output_lines: 1,
            output_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: true,
        };
    }
    
    // Accumulate lines until we hit either limit
    let mut output_lines_vec: Vec<&str> = Vec::new();
    let mut output_bytes = 0;
    let mut truncated_by = TruncatedBy::None;
    
    for line in &lines {
        let line_bytes = line.len() + 1; // +1 for newline
        if output_lines_vec.len() >= max_lines {
            truncated_by = TruncatedBy::Lines;
            break;
        }
        if output_bytes + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output_lines_vec.push(line);
        output_bytes += line_bytes;
    }
    
    let output_lines_count = output_lines_vec.len();
    let mut content = output_lines_vec.join("\n");
    
    // Add truncation notice
    let remaining_lines = total_lines.saturating_sub(output_lines_count);
    let remaining_bytes = total_bytes.saturating_sub(output_bytes);
    content.push_str(&format!(
        "\n\n... [truncated: {} lines, {} bytes remaining]",
        remaining_lines,
        format_bytes(remaining_bytes)
    ));
    
    TruncationResult {
        content,
        truncated: true,
        truncated_by,
        total_lines,
        total_bytes,
        output_lines: output_lines_count,
        output_bytes,
        last_line_partial: false,
        first_line_exceeds_limit,
    }
}

/// Truncate content from the tail (keep last N lines/bytes).
/// May produce a partial first line if the last line of original exceeds byte limit.
pub fn truncate_tail(content: &str, options: &TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(usize::MAX);
    let max_bytes = options.max_bytes.unwrap_or(usize::MAX);
    
    let total_bytes = content.len();
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    
    // Check if already within limits
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: TruncatedBy::None,
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
        };
    }
    
    // Take lines from the end
    let start = total_lines.saturating_sub(max_lines);
    let mut output_lines_vec: Vec<&str> = lines[start..].to_vec();
    let mut output_bytes: usize = output_lines_vec.iter().map(|l| l.len() + 1).sum();
    
    // If still over byte limit, trim from the front
    let mut last_line_partial = false;
    while output_bytes > max_bytes && !output_lines_vec.is_empty() {
        let first = output_lines_vec.first().unwrap();
        let first_bytes = first.len() + 1;
        
        if output_bytes - first_bytes <= max_bytes {
            // Partial first line - truncate it
            let keep_bytes = max_bytes.saturating_sub(output_bytes - first_bytes);
            if keep_bytes > 0 {
                let truncated = truncate_string_to_bytes_from_end(first, keep_bytes);
                output_lines_vec[0] = ""; // Will be replaced
                let content = format!(
                    "... [truncated]\n{}\n{}",
                    truncated,
                    output_lines_vec[1..].join("\n")
                );
                let output_lines = output_lines_vec.len();
                return TruncationResult {
                    content,
                    truncated: true,
                    truncated_by: TruncatedBy::Bytes,
                    total_lines,
                    total_bytes,
                    output_lines,
                    output_bytes: output_bytes.saturating_sub(first_bytes) + truncated.len(),
                    last_line_partial: true,
                    first_line_exceeds_limit: false,
                };
            }
        }
        
        output_bytes -= first_bytes;
        output_lines_vec.remove(0);
        last_line_partial = true;
    }
    
    let output_lines_count = output_lines_vec.len();
    let mut result = output_lines_vec.join("\n");
    
    if last_line_partial || start > 0 {
        result = format!("... [truncated]\n{}", result);
    }
    
    TruncationResult {
        content: result,
        truncated: true,
        truncated_by: if output_bytes >= max_bytes { TruncatedBy::Bytes } else { TruncatedBy::Lines },
        total_lines,
        total_bytes,
        output_lines: output_lines_count,
        output_bytes,
        last_line_partial,
        first_line_exceeds_limit: false,
    }
}

/// Truncate a single line to a maximum character length.
/// Appends "... [truncated]" if truncated.
pub fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    
    let truncated: String = line.chars().take(max_chars).collect();
    format!("{}... [truncated]", truncated)
}

/// Truncate a string to a maximum number of bytes, respecting UTF-8 boundaries.
fn truncate_to_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Truncate string to bytes from the end (keeping the tail portion).
fn truncate_string_to_bytes_from_end(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    
    let start = s.len() - max_bytes;
    // Find valid UTF-8 boundary
    let mut start = start;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// Format bytes for human-readable display
fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_head_within_limits() {
        let content = "line1\nline2\nline3";
        let result = truncate_head(content, &TruncationOptions {
            max_lines: Some(10),
            max_bytes: Some(1000),
        });
        assert!(!result.truncated);
        assert_eq!(result.total_lines, 3);
    }

    #[test]
    fn test_truncate_head_by_lines() {
        let content = "line1\nline2\nline3\nline4\nline5";
        let result = truncate_head(content, &TruncationOptions {
            max_lines: Some(2),
            max_bytes: Some(1000),
        });
        assert!(result.truncated);
        assert_eq!(result.output_lines, 2);
        assert!(result.content.contains("truncated"));
    }

    #[test]
    fn test_truncate_head_by_bytes() {
        let content = "short\nthis is a much longer line that should push us over the byte limit";
        let result = truncate_head(content, &TruncationOptions {
            max_lines: Some(100),
            max_bytes: Some(20),
        });
        assert!(result.truncated);
    }

    #[test]
    fn test_truncate_line_short() {
        assert_eq!(truncate_line("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_line_long() {
        let result = truncate_line("hello world this is a very long line", 11);
        assert_eq!(result, "hello world... [truncated]");
    }

    #[test]
    fn test_truncate_to_bytes_utf8() {
        // Korean characters are multi-byte
        let s = "안녕하세요";
        let truncated = truncate_to_bytes(s, 6); // 2 Korean chars = 6 bytes
        assert_eq!(truncated, "안녕");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500B");
        assert_eq!(format_bytes(1024), "1.0KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0MB");
    }

    #[test]
    fn test_truncate_tail_within_limits() {
        let content = "line1\nline2\nline3";
        let result = truncate_tail(content, &TruncationOptions {
            max_lines: Some(10),
            max_bytes: Some(1000),
        });
        assert!(!result.truncated);
    }

    #[test]
    fn test_truncate_tail_by_lines() {
        let content = "line1\nline2\nline3\nline4\nline5";
        let result = truncate_tail(content, &TruncationOptions {
            max_lines: Some(2),
            max_bytes: Some(1000),
        });
        assert!(result.truncated);
        assert!(result.content.contains("line4"));
        assert!(result.content.contains("line5"));
    }
}
