//! Utility modules for AI API handling
//!
//! These utilities provide robust handling for common AI API edge cases:
//! - Unicode sanitization for safe JSON serialization
//! - JSON parsing with repair for malformed streaming responses
//! - Context overflow detection across providers
//! - Tool call ID normalization for cross-provider compatibility

pub mod json_parse;
pub mod overflow;
pub mod sanitize_unicode;

/// Normalize a tool call ID for cross-provider compatibility.
///
/// OpenAI Responses API generates IDs that are 450+ chars with special characters like `|`.
/// Anthropic APIs require IDs matching `^[a-zA-Z0-9_-]+$` (max 64 chars).
///
/// This replaces non-alphanumeric (except `_` and `-`) chars with `_` and
/// truncates to 64 characters.
pub fn normalize_tool_call_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.len() > 64 {
        sanitized[..64].trim_end_matches('_').to_string()
    } else {
        sanitized.trim_end_matches('_').to_string()
    }
}
