//! Utility modules for AI API handling
//!
//! These utilities provide robust handling for common AI API edge cases:
//! - Unicode sanitization for safe JSON serialization
//! - JSON parsing with repair for malformed streaming responses
//! - Context overflow detection across providers

pub mod json_parse;
pub mod overflow;
pub mod sanitize_unicode;
