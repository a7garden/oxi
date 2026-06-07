//! Structured output extraction and validation.
//!
//! Provides utilities for extracting structured JSON from agent responses
//! and optionally validating them against a JSON Schema.

use serde_json::Value;

/// Output mode for agent responses.
#[derive(Debug, Clone, Default)]
pub enum OutputMode {
    /// Return the raw text response as-is.
    #[default]
    Text,
    /// Extract JSON from the response.
    Json,
    /// Extract JSON and validate against a schema.
    ValidatedJson {
        /// JSON Schema to validate against.
        schema: Value,
    },
}

impl OutputMode {
    /// Check if this mode requires JSON extraction.
    pub fn requires_json(&self) -> bool {
        matches!(self, OutputMode::Json | OutputMode::ValidatedJson { .. })
    }
}

/// Structured output extractor.
pub struct StructuredOutput;

impl StructuredOutput {
    /// Extract structured output from agent response content.
    ///
    /// - `OutputMode::Text` → returns the content as a JSON string.
    /// - `OutputMode::Json` → extracts JSON from the content.
    /// - `OutputMode::ValidatedJson` → extracts and validates JSON.
    pub fn extract(content: &str, mode: &OutputMode) -> Result<Value, StructuredOutputError> {
        match mode {
            OutputMode::Text => Ok(Value::String(content.to_string())),
            OutputMode::Json => Self::extract_json(content),
            OutputMode::ValidatedJson { schema } => {
                let json = Self::extract_json(content)?;
                Self::validate(&json, schema)?;
                Ok(json)
            }
        }
    }

    /// Extract JSON from text content.
    ///
    /// Tries, in order:
    /// 1. Parse the entire content as JSON.
    /// 2. Extract from a ` ```json ... ``` ` code block.
    /// 3. Find a matching `{ ... }` or `[ ... ]` bracket pair.
    pub fn extract_json(content: &str) -> Result<Value, StructuredOutputError> {
        // 1. Entire content is JSON
        if let Ok(v) = serde_json::from_str::<Value>(content) {
            return Ok(v);
        }

        // 2. ```json ... ``` block
        if let Some(start) = content.find("```json") {
            let json_start = start + 7;
            if let Some(end) = content[json_start..].find("```") {
                let json_str = content[json_start..json_start + end].trim();
                return serde_json::from_str(json_str).map_err(|e| {
                    StructuredOutputError::ParseError(format!(
                        "JSON parse error in code block: {}",
                        e
                    ))
                });
            }
        }

        // 3. Find matching brackets
        for (open, close) in [('{', '}'), ('[', ']')] {
            if let Some(start) = content.find(open) {
                let substr = &content[start..];
                if let Some(end) = Self::find_matching_bracket(substr, open, close) {
                    let json_str = &substr[..=end];
                    if let Ok(v) = serde_json::from_str(json_str) {
                        return Ok(v);
                    }
                }
            }
        }

        Err(StructuredOutputError::NotFound(
            "No JSON found in response".into(),
        ))
    }

    /// Validate a JSON value against a JSON Schema.
    ///
    /// Uses basic type/required field validation. For full JSON Schema
    /// validation, enable the `jsonschema` feature (future work).
    pub fn validate(json: &Value, schema: &Value) -> Result<(), StructuredOutputError> {
        // Basic validation: check "type" keyword
        if let Some(expected_type) = schema.get("type").and_then(|t| t.as_str()) {
            let actual_matches = match expected_type {
                "object" => json.is_object(),
                "array" => json.is_array(),
                "string" => json.is_string(),
                "number" => json.is_number(),
                "integer" => json.is_i64() || json.is_u64(),
                "boolean" => json.is_boolean(),
                "null" => json.is_null(),
                _ => true,
            };
            if !actual_matches {
                return Err(StructuredOutputError::ValidationError(format!(
                    "Expected type '{}', got '{}'",
                    expected_type,
                    json_type_name(json)
                )));
            }
        }

        // Check "required" fields
        if let Some(required) = schema.get("required").and_then(|r| r.as_array())
            && let Some(obj) = json.as_object()
        {
            for field in required {
                if let Some(name) = field.as_str()
                    && !obj.contains_key(name)
                {
                    return Err(StructuredOutputError::ValidationError(format!(
                        "Missing required field: '{}'",
                        name
                    )));
                }
            }
        }

        Ok(())
    }

    /// Find the index of the closing bracket that matches the first opening bracket.
    fn find_matching_bracket(s: &str, open: char, close: char) -> Option<usize> {
        let mut depth = 0;
        let mut in_string = false;
        let mut escape_next = false;

        for (i, c) in s.char_indices() {
            if escape_next {
                escape_next = false;
                continue;
            }
            match c {
                '\\' if in_string => escape_next = true,
                '"' => in_string = !in_string,
                _ if in_string => {}
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

/// Errors during structured output extraction.
#[derive(Debug, thiserror::Error)]
pub enum StructuredOutputError {
    /// JSON could not be found in the response.
    #[error("JSON not found: {0}")]
    NotFound(String),

    /// JSON parsing failed.
    #[error("{0}")]
    ParseError(String),

    /// Schema validation failed.
    #[error("Validation error: {0}")]
    ValidationError(String),
}

/// Get a human-readable type name for a JSON value.
fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_text_mode() {
        let result = StructuredOutput::extract("hello world", &OutputMode::Text).unwrap();
        assert_eq!(result, Value::String("hello world".to_string()));
    }

    #[test]
    fn test_extract_pure_json() {
        let json = r#"{"name": "test", "value": 42}"#;
        let result = StructuredOutput::extract(json, &OutputMode::Json).unwrap();
        assert_eq!(result["name"], "test");
        assert_eq!(result["value"], 42);
    }

    #[test]
    fn test_extract_json_code_block() {
        let content = "Here is the result:\n```json\n{\"status\": \"ok\"}\n```\nDone.";
        let result = StructuredOutput::extract(content, &OutputMode::Json).unwrap();
        assert_eq!(result["status"], "ok");
    }

    #[test]
    fn test_extract_json_embedded_brackets() {
        let content = "The answer is {\"x\": 1, \"y\": 2} as shown above.";
        let result = StructuredOutput::extract(content, &OutputMode::Json).unwrap();
        assert_eq!(result["x"], 1);
    }

    #[test]
    fn test_extract_json_array() {
        let content = "Results: [1, 2, 3]";
        let result = StructuredOutput::extract(content, &OutputMode::Json).unwrap();
        assert_eq!(result, json!([1, 2, 3]));
    }

    #[test]
    fn test_extract_json_not_found() {
        let content = "No JSON here, just plain text.";
        let result = StructuredOutput::extract(content, &OutputMode::Json);
        assert!(result.is_err());
    }

    #[test]
    fn test_validated_json_success() {
        let schema = json!({
            "type": "object",
            "required": ["name"]
        });
        let content = r#"{"name": "test", "value": 42}"#;
        let result =
            StructuredOutput::extract(content, &OutputMode::ValidatedJson { schema }).unwrap();
        assert_eq!(result["name"], "test");
    }

    #[test]
    fn test_validated_json_wrong_type() {
        let schema = json!({"type": "array"});
        let content = r#"{"name": "test"}"#;
        let result = StructuredOutput::extract(content, &OutputMode::ValidatedJson { schema });
        assert!(result.is_err());
    }

    #[test]
    fn test_validated_json_missing_required() {
        let schema = json!({
            "type": "object",
            "required": ["name", "age"]
        });
        let content = r#"{"name": "test"}"#;
        let result = StructuredOutput::extract(content, &OutputMode::ValidatedJson { schema });
        assert!(result.is_err());
    }

    #[test]
    fn test_nested_brackets() {
        let content = r#"Result: {"a": {"b": [1, 2]}, "c": 3}"#;
        let result = StructuredOutput::extract_json(content).unwrap();
        assert_eq!(result["a"]["b"], json!([1, 2]));
        assert_eq!(result["c"], 3);
    }

    #[test]
    fn test_json_with_string_containing_brackets() {
        let content = r#"{"text": "hello {world}"}"#;
        let result = StructuredOutput::extract_json(content).unwrap();
        assert_eq!(result["text"], "hello {world}");
    }

    #[test]
    fn test_output_mode_requires_json() {
        assert!(!OutputMode::Text.requires_json());
        assert!(OutputMode::Json.requires_json());
        assert!(OutputMode::ValidatedJson { schema: json!({}) }.requires_json());
    }
}
