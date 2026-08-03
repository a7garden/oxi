//! Shared code between OpenAI Responses API and Codex providers.
//!
//! Contains the streaming JSON parser used by multiple OpenAI-compatible providers.

use serde_json::Value as JsonValue;

// ---------------------------------------------------------------------------
// Streaming JSON parser
// ---------------------------------------------------------------------------

/// Parse a partial JSON string, returning the best-effort parsed value.
/// Returns an empty JSON object for completely invalid input.
pub fn parse_streaming_json(input: &str) -> JsonValue {
    if input.is_empty() {
        return serde_json::json!({});
    }

    // Try full parse first
    if let Ok(val) = serde_json::from_str::<JsonValue>(input) {
        return val;
    }

    // Try to complete the JSON by closing open brackets/braces
    let mut open_braces = 0i32;
    let mut open_brackets = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for ch in input.chars() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => open_braces += 1,
            '}' => open_braces -= 1,
            '[' => open_brackets += 1,
            ']' => open_brackets -= 1,
            _ => {}
        }
    }

    let mut completed = input.to_string();
    if in_string {
        completed.push('"');
    }
    for _ in 0..open_brackets {
        completed.push(']');
    }
    for _ in 0..open_braces {
        completed.push('}');
    }

    serde_json::from_str::<JsonValue>(&completed).unwrap_or(serde_json::json!({}))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_streaming_json_empty() {
        assert_eq!(parse_streaming_json(""), serde_json::json!({}));
    }

    #[test]
    fn test_parse_streaming_json_complete() {
        let result = parse_streaming_json(r#"{"key": "value"}"#);
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn test_parse_streaming_json_partial() {
        let result = parse_streaming_json(r#"{"key": "val"#);
        assert_eq!(result["key"], "val");
    }
}
