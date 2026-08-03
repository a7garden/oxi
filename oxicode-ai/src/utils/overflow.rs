//! Context overflow detection utilities
//!
//! Detects when an LLM response indicates the input exceeded the model's context window.
//! Supports error-based detection (most providers) and silent overflow detection (z.ai, Xiaomi MiMo).

use crate::messages::AssistantMessage;

/// Regex-like patterns to detect context overflow errors from different providers.
/// Each entry is a substring pattern that indicates an overflow error.
const OVERFLOW_PATTERNS: &[&str] = &[
    "prompt is too long",                    // Anthropic token overflow
    "request_too_large",                     // Anthropic request byte-size overflow (HTTP 413)
    "input is too long for requested model", // Amazon Bedrock
    "exceeds the context window",            // OpenAI (Completions & Responses API)
    "exceeds the maximum number of tokens",  // Google (Gemini)
    "maximum prompt length",                 // xAI (Grok)
    "reduce the length of the messages",     // Groq
    "maximum context length",                // OpenRouter (all backends)
    "exceeds the limit of",                  // GitHub Copilot
    "exceeds the available context size",    // llama.cpp server
    "greater than the context length",       // LM Studio
    "context window exceeds limit",          // MiniMax
    "exceeded model token limit",            // Kimi For Coding
    "too large for model with",              // Mistral
    "model_context_window_exceeded",         // z.ai non-standard finish_reason
    "prompt too long",                       // Ollama explicit overflow error
    "context_length_exceeded",               // Generic (LiteLLM, etc.)
    "context length exceeded",               // Generic fallback
    "too many tokens",                       // Generic fallback
    "token limit exceeded",                  // Generic fallback
];

/// Patterns that indicate non-overflow errors (e.g., rate limiting, server errors).
/// Error messages matching any of these are excluded from overflow detection
/// even if they also match an OVERFLOW_PATTERN.
const NON_OVERFLOW_PATTERNS: &[&str] = &[
    "Throttling error:",    // AWS Bedrock non-overflow
    "Service unavailable:", // AWS Bedrock non-overflow
    "rate limit",           // Generic rate limiting
    "too many requests",    // Generic HTTP 429 style
];

/// Check if an assistant message represents a context overflow error.
///
/// This handles three cases:
/// 1. **Error-based overflow**: Most providers return `stop_reason = "error"` with a
///    specific error message pattern.
/// 2. **Silent overflow**: Some providers accept overflow requests and return
///    successfully. For these, check if `usage.input` exceeds the context window.
/// 3. **Length-stop overflow**: Some providers (Xiaomi MiMo) truncate input to fill
///    the context window, leaving no room for output. Returns `stop_reason = "length"`
///    with `output = 0` and input filling the context window.
///
/// # Arguments
/// * `message` - The assistant message to check
/// * `context_window` - Optional context window size for detecting silent overflow
///
/// # Returns
/// `true` if the message indicates a context overflow
pub fn is_context_overflow(message: &AssistantMessage, context_window: Option<usize>) -> bool {
    // Case 1: Check error message patterns
    if message.stop_reason == crate::types::StopReason::Error
        && let Some(ref error_msg) = message.error_message
    {
        // Skip messages matching known non-overflow patterns
        let is_non_overflow = NON_OVERFLOW_PATTERNS
            .iter()
            .any(|p: &&str| error_msg.contains(p));

        if !is_non_overflow {
            let is_overflow = OVERFLOW_PATTERNS
                .iter()
                .any(|p: &&str| error_msg.contains(p));

            if is_overflow {
                return true;
            }
        }

        // Special case: Cerebras returns "400 status code (no body)" or "413 status code (no body)"
        if (error_msg.contains("400") || error_msg.contains("413"))
            && (error_msg.contains("no body") || error_msg.trim().len() < 50)
        {
            return true;
        }
    }

    let Some(window) = context_window else {
        return false;
    };

    // Case 2: Silent overflow (z.ai style) - successful but usage exceeds context
    if message.stop_reason == crate::types::StopReason::Stop {
        let input_tokens = message.usage.input + message.usage.cache_read;
        if input_tokens > window {
            return true;
        }
    }

    // Case 3: Length-stop overflow (Xiaomi MiMo style)
    // Server truncates oversized input to fit context window, leaving no room for output.
    // Returns stopReason "length" with output=0 and input filling the context window.
    if message.stop_reason == crate::types::StopReason::Length && message.usage.output == 0 {
        let input_tokens = message.usage.input + message.usage.cache_read;
        // Use 99% threshold to account for rounding
        if input_tokens >= (window as f64 * 0.99) as usize {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Cost, StopReason, Usage};

    fn make_error_message(error: &str) -> AssistantMessage {
        let mut msg =
            AssistantMessage::new(crate::types::Api::OpenAiCompletions, "test", "test-model");
        msg.stop_reason = StopReason::Error;
        msg.error_message = Some(error.to_string());
        msg
    }

    fn make_success_message(input: usize, output: usize) -> AssistantMessage {
        let mut msg =
            AssistantMessage::new(crate::types::Api::OpenAiCompletions, "test", "test-model");
        msg.stop_reason = StopReason::Stop;
        msg.usage = Usage {
            input,
            output,
            total_tokens: input + output,
            cache_read: 0,
            cache_write: 0,
            cost: Cost::default(),
        };
        msg
    }

    fn make_length_message(input: usize, output: usize) -> AssistantMessage {
        let mut msg =
            AssistantMessage::new(crate::types::Api::OpenAiCompletions, "test", "test-model");
        msg.stop_reason = StopReason::Length;
        msg.usage = Usage {
            input,
            output,
            total_tokens: input + output,
            cache_read: 0,
            cache_write: 0,
            cost: Cost::default(),
        };
        msg
    }

    #[test]
    fn test_anthropic_overflow() {
        let msg = make_error_message("prompt is too long: 213462 tokens > 200000 maximum");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_anthropic_request_too_large() {
        let msg = make_error_message("request_too_large: Request exceeds maximum size");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_openai_overflow() {
        let msg = make_error_message("Your input exceeds the context window of this model");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_google_overflow() {
        let msg = make_error_message(
            "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_xai_overflow() {
        let msg = make_error_message(
            "This model's maximum prompt length is 131072 but the request contains 537812 tokens",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_groq_overflow() {
        let msg = make_error_message("Please reduce the length of the messages or completion");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_mistral_overflow() {
        let msg = make_error_message(
            "Prompt contains X tokens ... too large for model with Y maximum context length",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_non_overflow_rate_limit() {
        let msg = make_error_message("rate limit exceeded");
        assert!(!is_context_overflow(&msg, None));
    }

    #[test]
    fn test_non_overflow_throttling() {
        let msg = make_error_message("Throttling error: Too many tokens, please wait");
        // "too many tokens" matches an overflow pattern, but "Throttling error:" is a non-overflow pattern
        assert!(!is_context_overflow(&msg, None));
    }

    #[test]
    fn test_silent_overflow() {
        let msg = make_success_message(150_000, 500);
        assert!(is_context_overflow(&msg, Some(128_000)));
    }

    #[test]
    fn test_no_silent_overflow() {
        let msg = make_success_message(100_000, 500);
        assert!(!is_context_overflow(&msg, Some(128_000)));
    }

    #[test]
    fn test_length_stop_overflow() {
        let msg = make_length_message(127_500, 0);
        assert!(is_context_overflow(&msg, Some(128_000)));
    }

    #[test]
    fn test_length_stop_no_overflow() {
        let msg = make_length_message(100_000, 0);
        assert!(!is_context_overflow(&msg, Some(128_000)));
    }

    #[test]
    fn test_length_stop_with_output() {
        // Has output, so not a silent overflow
        let msg = make_length_message(100_000, 500);
        assert!(!is_context_overflow(&msg, Some(128_000)));
    }

    #[test]
    fn test_no_error_no_overflow() {
        let msg = make_success_message(100, 50);
        assert!(!is_context_overflow(&msg, None));
    }

    #[test]
    fn test_cerebras_overflow() {
        let msg = make_error_message("400 status code (no body)");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_bedrock_overflow() {
        let msg = make_error_message("input is too long for requested model");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_llamacpp_overflow() {
        let msg =
            make_error_message("the request exceeds the available context size, try increasing it");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_minimax_overflow() {
        let msg = make_error_message("invalid params, context window exceeds limit");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_kimi_overflow() {
        let msg = make_error_message(
            "Your request exceeded model token limit: 128000 (requested: 200000)",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_generic_context_length_exceeded() {
        let msg = make_error_message("context_length_exceeded");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_service_unavailable_not_overflow() {
        let msg = make_error_message("Service unavailable: too many tokens, try again later");
        assert!(!is_context_overflow(&msg, None));
    }
}
