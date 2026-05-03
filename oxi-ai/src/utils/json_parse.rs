//! Robust JSON parsing utilities
//!
//! Handles malformed JSON from streaming LLM responses:
//! - Escapes raw control characters inside strings
//! - Doubles backslashes before invalid escape characters
//! - Repairs incomplete JSON from streaming responses

use crate::types::AssistantMessage;

/// Characters that are valid after a backslash in JSON strings
const VALID_JSON_ESCAPES: &[char] = &['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];

/// Check if a character is a control character (U+0000 to U+001F)
fn is_control_character(ch: char) -> bool {
    ch as u32 <= 0x1F
}

/// Escape a control character for JSON
fn escape_control_character(ch: char) -> String {
    match ch {
        '\u{0008}' => "\\b".to_string(),
        '\u{000C}' => "\\f".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        _ => format!("\\u{:04x}", ch as u32),
    }
}

/// Repairs malformed JSON string literals by:
/// - Escaping raw control characters inside strings
/// - Doubling backslashes before invalid escape characters
pub fn repair_json(json: &str) -> String {
    let mut repaired = String::with_capacity(json.len());
    let mut in_string = false;
    let chars: Vec<char> = json.chars().collect();
    let len = chars.len();
    let mut index = 0;

    while index < len {
        let ch = chars[index];

        if !in_string {
            repaired.push(ch);
            if ch == '"' {
                in_string = true;
            }
            index += 1;
            continue;
        }

        // We're inside a string
        if ch == '"' {
            repaired.push(ch);
            in_string = false;
            index += 1;
            continue;
        }

        if ch == '\\' {
            // Check next character
            if index + 1 >= len {
                // Trailing backslash at end - escape it
                repaired.push_str("\\\\");
                index += 1;
                continue;
            }

            let next_ch = chars[index + 1];

            if next_ch == 'u' {
                // Unicode escape - check if valid
                let unicode_digits: String = chars[index + 2..std::cmp::min(index + 6, len)]
                    .iter()
                    .collect();
                if unicode_digits.len() == 4
                    && unicode_digits.chars().all(|c| c.is_ascii_hexdigit())
                {
                    repaired.push_str(&format!("\\u{}", unicode_digits));
                    index += 6;
                    continue;
                }
            }

            if VALID_JSON_ESCAPES.contains(&next_ch) {
                repaired.push('\\');
                repaired.push(next_ch);
                index += 2;
                continue;
            }

            // Invalid escape - double the backslash
            repaired.push_str("\\\\");
            index += 1;
            continue;
        }

        // Regular character in string - escape control characters
        if is_control_character(ch) {
            repaired.push_str(&escape_control_character(ch));
        } else {
            repaired.push(ch);
        }
        index += 1;
    }

    repaired
}

/// Parse JSON with automatic repair of common malformations.
///
/// First tries standard parsing. If that fails, repairs the JSON and retries.
pub fn parse_json_with_repair<T: serde::de::DeserializeOwned>(
    json: &str,
) -> Result<T, serde_json::Error> {
    match serde_json::from_str(json) {
        Ok(result) => Ok(result),
        Err(original_error) => {
            let repaired = repair_json(json);
            if repaired != json {
                match serde_json::from_str(&repaired) {
                    Ok(result) => Ok(result),
                    Err(_) => Err(original_error),
                }
            } else {
                Err(original_error)
            }
        }
    }
}

/// Attempts to parse potentially incomplete JSON from a streaming response.
///
/// Tries multiple strategies:
/// 1. Direct parse
/// 2. Parse with repair
/// 3. Truncate at last valid position and retry
///
/// Always returns a valid value, using `default` as fallback.
pub fn parse_streaming_json<T: serde::de::DeserializeOwned + Default>(
    json: &str,
) -> T {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return T::default();
    }

    // Strategy 1: Direct parse
    if let Ok(result) = serde_json::from_str(trimmed) {
        return result;
    }

    // Strategy 2: Parse with repair
    if let Ok(result) = parse_json_with_repair(trimmed) {
        return result;
    }

    // Strategy 3: Try to parse as partial by finding last complete object
    if let Some(result) = parse_partial_json(trimmed) {
        return result;
    }

    // Strategy 4: Repair then parse partial
    let repaired = repair_json(trimmed);
    if repaired != trimmed {
        if let Some(result) = parse_partial_json(&repaired) {
            return result;
        }
    }

    T::default()
}

/// Try to parse partial JSON by progressively truncating from the end
/// until we find valid JSON.
fn parse_partial_json<T: serde::de::DeserializeOwned>(json: &str) -> Option<T> {
    // Only try this for objects/arrays
    let trimmed = json.trim();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return None;
    }

    let _close_char = if trimmed.starts_with('{') {
        '}'
    } else {
        ']'
    };
    let _open_char = if trimmed.starts_with('{') {
        '{'
    } else {
        '['
    };

    // Track nesting depth
    let mut depth = 0;
    let mut in_string = false;
    let mut last_valid_close = None;
    let bytes = trimmed.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if b == b'"' {
                in_string = false;
            } else if b == b'\\' {
                // Skip next char (escape)
                continue;
            }
            continue;
        }

        match b {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    last_valid_close = Some(i);
                }
            }
            _ => {}
        }
    }

    // If we found a valid closing position, try to parse up to it
    if let Some(pos) = last_valid_close {
        let candidate = &trimmed[..=pos];
        if let Ok(result) = serde_json::from_str(candidate) {
            return Some(result);
        }
    }

    None
}

/// Parse a streaming SSE data field as JSON, with robust error handling.
/// Returns `None` for non-data lines or unparseable content.
pub fn parse_sse_data<T: serde::de::DeserializeOwned + Default>(
    line: &str,
) -> Option<T> {
    let line = line.trim();

    if !line.starts_with("data: ") {
        return None;
    }

    let data = &line[6..];

    if data.is_empty() || data == "[DONE]" {
        return None;
    }

    Some(parse_streaming_json(data))
}

/// Extract the error message from an assistant message that may have
/// malformed JSON in its error field.
pub fn extract_error_message(message: &AssistantMessage) -> String {
    message
        .error_message
        .clone()
        .unwrap_or_else(|| "Unknown error".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq, Default)]
    struct TestObj {
        name: String,
        value: Option<i64>,
    }

    #[test]
    fn test_repair_json_valid() {
        let json = r#"{"name": "test"}"#;
        assert_eq!(repair_json(json), json);
    }

    #[test]
    fn test_repair_json_control_chars() {
        let json = "{\"name\": \"hello\nworld\"}";
        let repaired = repair_json(json);
        assert!(repaired.contains("\\n"));
        assert!(!repaired.contains("hello\nworld"));
    }

    #[test]
    fn test_repair_json_tab() {
        let json = "{\"name\": \"hello\tworld\"}";
        let repaired = repair_json(json);
        assert!(repaired.contains("\\t"));
    }

    #[test]
    fn test_repair_json_invalid_escape() {
        let json = r#"{"name": "hello\qworld"}"#;
        let repaired = repair_json(json);
        assert!(repaired.contains("\\\\q") || repaired.contains(r#"\\q"#));
    }

    #[test]
    fn test_repair_json_trailing_backslash() {
        let json = r#"{"name": "test\"#;
        let repaired = repair_json(json);
        assert!(repaired.contains("\\\\"));
    }

    #[test]
    fn test_repair_json_valid_escapes_preserved() {
        let json = r#"{"name": "hello\nworld"}"#;
        let repaired = repair_json(json);
        assert_eq!(repaired, json);
    }

    #[test]
    fn test_repair_json_unicode_escape_preserved() {
        let json = r#"{"name": "\u0041"}"#;
        let repaired = repair_json(json);
        assert_eq!(repaired, json);
    }

    #[test]
    fn test_parse_json_with_repair_valid() {
        let result: TestObj = parse_json_with_repair(r#"{"name": "test", "value": 42}"#).unwrap();
        assert_eq!(result.name, "test");
        assert_eq!(result.value, Some(42));
    }

    #[test]
    fn test_parse_json_with_repair_control_chars() {
        let json = "{\"name\": \"hello\nworld\"}";
        let result: TestObj = parse_json_with_repair(json).unwrap();
        assert_eq!(result.name, "hello\nworld");
    }

    #[test]
    fn test_parse_streaming_json_valid() {
        let result: TestObj = parse_streaming_json(r#"{"name": "test"}"#);
        assert_eq!(result.name, "test");
    }

    #[test]
    fn test_parse_streaming_json_empty() {
        let result: TestObj = parse_streaming_json("");
        assert_eq!(result, TestObj::default());
    }

    #[test]
    fn test_parse_streaming_json_whitespace() {
        let result: TestObj = parse_streaming_json("   ");
        assert_eq!(result, TestObj::default());
    }

    #[test]
    fn test_parse_streaming_json_partial() {
        let result: TestObj = parse_streaming_json(r#"{"name": "test"}, "extra""#);
        assert_eq!(result.name, "test");
    }

    #[test]
    fn test_parse_sse_data_valid() {
        let result: TestObj = parse_sse_data(r#"data: {"name": "test"}"#).unwrap();
        assert_eq!(result.name, "test");
    }

    #[test]
    fn test_parse_sse_data_done() {
        let result: Option<TestObj> = parse_sse_data("data: [DONE]");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_data_not_data_line() {
        let result: Option<TestObj> = parse_sse_data("event: message");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_data_empty_data() {
        let result: Option<TestObj> = parse_sse_data("data: ");
        assert!(result.is_none());
    }

    #[test]
    fn test_escape_control_character_special() {
        assert_eq!(escape_control_character('\n'), "\\n");
        assert_eq!(escape_control_character('\r'), "\\r");
        assert_eq!(escape_control_character('\t'), "\\t");
        assert_eq!(escape_control_character('\u{0008}'), "\\b");
        assert_eq!(escape_control_character('\u{000C}'), "\\f");
    }

    #[test]
    fn test_escape_control_character_generic() {
        assert_eq!(escape_control_character('\u{0001}'), "\\u0001");
        assert_eq!(escape_control_character('\u{001F}'), "\\u001f");
    }
}
