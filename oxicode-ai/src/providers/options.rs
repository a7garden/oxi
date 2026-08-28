//! Stream options for providers

use crate::{CacheRetention, ThinkingLevel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Per-provider options for fine-grained control.
///
/// Each field corresponds to a specific provider's native API option.
/// Only the relevant provider reads its section; others ignore it.
/// Mirrors opencode's `providerOptions` pattern where the request carries
/// a bag of per-provider knobs that the protocol layer reads selectively.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderOptions {
    /// Anthropic-specific options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<AnthropicOptions>,

    /// OpenAI-specific options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai: Option<OpenAiOptions>,

    /// Google/Gemini-specific options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google: Option<GoogleOptions>,

    /// Generic OpenAI-compatible provider options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_compatible: Option<OpenAiCompatibleOptions>,
}

/// Anthropic-specific options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicOptions {
    /// Extended thinking mode.
    /// - `"enabled"`: Fixed budget thinking
    /// - `"adaptive"`: Anthropic chooses budget based on effort
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_type: Option<String>,

    /// Token budget for thinking (when thinking_type is "enabled").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<usize>,

    /// Reasoning effort level (when thinking_type is "adaptive").
    /// Values: "low", "medium", "high", "xhigh", "max".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// OpenAI-specific options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiOptions {
    /// Whether to store the response for session continuity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,

    /// Reasoning effort: "low", "medium", "high", "xhigh".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,

    /// Whether to include reasoning summary in the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_summary: Option<String>,

    /// Whether to include encrypted reasoning content for session continuity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_encrypted_reasoning: Option<bool>,

    /// Text verbosity: "low", "medium", "high".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_verbosity: Option<String>,

    /// Prompt cache key for server-side caching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
}

/// Google/Gemini-specific options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleOptions {
    /// Whether to include thoughts in the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,

    /// Thinking level: "low", "medium", "high".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,

    /// Thinking budget in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<usize>,
}

/// Generic OpenAI-compatible provider options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiCompatibleOptions {
    /// Reasoning effort level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,

    /// Whether thinking is enabled (for providers like ZAI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,

    /// Cache control marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<String>,
}

/// Options for streaming requests
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct StreamOptions {
    /// Sampling temperature (0.0 to 2.0)
    #[serde(default)]
    pub temperature: Option<f64>,

    /// Maximum tokens to generate
    #[serde(default)]
    pub max_tokens: Option<usize>,

    /// API key (overrides environment variable)
    /// This field is excluded from serialization and Debug output to prevent leakage.
    #[serde(skip)]
    pub api_key: Option<String>,

    /// Cache retention preference
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_retention: Option<CacheRetention>,

    /// Session ID for providers that support session-based caching
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Custom HTTP headers to include
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Thinking/reasoning level
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,

    /// Custom token budgets for thinking levels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budgets: Option<ThinkingBudgets>,

    /// Forces the next assistant turn's tool choice. `None`/`Auto` = no
    /// change from today's behavior. See [`crate::tools::ToolChoice`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<crate::tools::ToolChoice>,

    /// Per-provider options for fine-grained control.
    ///
    /// Each provider reads only its own section. For example, the Anthropic
    /// provider reads `provider_options.anthropic`, OpenAI reads
    /// `provider_options.openai`. This allows a single request to carry
    /// options for multiple providers without conflicts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl fmt::Debug for StreamOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamOptions")
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("cache_retention", &self.cache_retention)
            .field("session_id", &self.session_id)
            .field("headers", &self.headers)
            .field("thinking_level", &self.thinking_level)
            .field("thinking_budgets", &self.thinking_budgets)
            .field("provider_options", &self.provider_options)
            .finish()
    }
}

impl StreamOptions {
    /// Create new stream options
    pub fn new() -> Self {
        Self::default()
    }

    /// Set temperature
    pub fn temperature(mut self, temp: f64) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set max tokens
    pub fn max_tokens(mut self, tokens: usize) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Set API key
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Set cache retention
    pub fn cache_retention(mut self, retention: CacheRetention) -> Self {
        self.cache_retention = Some(retention);
        self
    }

    /// Set session ID
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Set thinking level
    pub fn thinking_level(mut self, level: ThinkingLevel) -> Self {
        self.thinking_level = Some(level);
        self
    }
}

/// Token budgets for thinking levels
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    #[serde(default)]
    pub minimal: Option<usize>,
    #[serde(default)]
    pub low: Option<usize>,
    #[serde(default)]
    pub medium: Option<usize>,
    #[serde(default)]
    pub high: Option<usize>,
}

impl ThinkingBudgets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn minimal(mut self, tokens: usize) -> Self {
        self.minimal = Some(tokens);
        self
    }

    pub fn low(mut self, tokens: usize) -> Self {
        self.low = Some(tokens);
        self
    }

    pub fn medium(mut self, tokens: usize) -> Self {
        self.medium = Some(tokens);
        self
    }

    pub fn high(mut self, tokens: usize) -> Self {
        self.high = Some(tokens);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolChoice;

    #[test]
    fn stream_options_default_has_no_tool_choice() {
        let opts = StreamOptions::default();
        assert!(opts.tool_choice.is_none());
    }

    #[test]
    fn tool_choice_named_round_trips_through_serde() {
        let tc = ToolChoice::Named("todo".to_string());
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tc);
    }
}
