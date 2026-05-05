//! Core domain types for oxi-ai

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

/// Provider API identifier.
///
/// Selects the wire-format / protocol dialect spoken to a particular LLM provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Api {
    /// OpenAI Chat Completions API.
    #[serde(rename = "openai-completions")]
    OpenAiCompletions,
    /// OpenAI Responses API.
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// Anthropic Messages API.
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
    /// Google Generative AI (Gemini) API.
    #[serde(rename = "google-generative-ai")]
    GoogleGenerativeAi,
    /// Google Vertex AI endpoint.
    #[serde(rename = "google-vertex")]
    GoogleVertex,
    /// Mistral Conversations API.
    #[serde(rename = "mistral-conversations")]
    MistralConversations,
    /// Azure OpenAI Responses API.
    #[serde(rename = "azure-openai-responses")]
    AzureOpenAiResponses,
    /// AWS Bedrock Converse Stream API.
    #[serde(rename = "bedrock-converse-stream")]
    BedrockConverseStream,
}

impl fmt::Display for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Api::OpenAiCompletions => write!(f, "openai-completions"),
            Api::OpenAiResponses => write!(f, "openai-responses"),
            Api::AnthropicMessages => write!(f, "anthropic-messages"),
            Api::GoogleGenerativeAi => write!(f, "google-generative-ai"),
            Api::GoogleVertex => write!(f, "google-vertex"),
            Api::MistralConversations => write!(f, "mistral-conversations"),
            Api::AzureOpenAiResponses => write!(f, "azure-openai-responses"),
            Api::BedrockConverseStream => write!(f, "bedrock-converse-stream"),
        }
    }
}

/// Cache retention preference
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    #[default]
/// none variant.
    None,
/// short variant.
    Short,
/// long variant.
    Long,
}

/// Model thinking/reasoning level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ThinkingLevel {
    #[default]
/// off variant.
    Off,
/// minimal variant.
    Minimal,
/// low variant.
    Low,
/// medium variant.
    Medium,
/// high variant.
    High,
/// x high variant.
    XHigh,
}

impl ThinkingLevel {
/// TODO: document this function.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ThinkingLevel::Off => None,
            ThinkingLevel::Minimal => Some("minimal"),
            ThinkingLevel::Low => Some("low"),
            ThinkingLevel::Medium => Some("medium"),
            ThinkingLevel::High => Some("high"),
            ThinkingLevel::XHigh => Some("xhigh"),
        }
    }
}

/// Input modalities
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum InputModality {
/// text variant.
    Text,
/// image variant.
    Image,
}

/// Cost structure – prices per million tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cost {
    /// Input token cost ($/M tokens).
    #[serde(default)]
    pub input: f64,
    /// Output token cost ($/M tokens).
    #[serde(default)]
    pub output: f64,
    /// Cached-input read cost ($/M tokens).
    #[serde(default)]
    pub cache_read: f64,
    /// Cache write cost ($/M tokens).
    #[serde(default)]
    pub cache_write: f64,
}

impl Cost {
    /// Sum of all cost components.
    pub fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// Stop reason – why the model finished generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum StopReason {
    /// Normal stop – the model finished its response.
    Stop,
    /// Hit the maximum output token limit.
    Length,
    /// Stopped to invoke a tool.
    ToolUse,
    /// An error occurred during generation.
    Error,
    /// Generation was aborted by the client.
    Aborted,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Number of input (prompt) tokens.
    #[serde(default)]
    pub input: usize,
    /// Number of output (completion) tokens.
    #[serde(default)]
    pub output: usize,
    /// Number of tokens read from cache.
    #[serde(default)]
    pub cache_read: usize,
    /// Number of tokens written to cache.
    #[serde(default)]
    pub cache_write: usize,
    /// Total tokens (input + output + cache).
    #[serde(default)]
    pub total_tokens: usize,
    /// Computed cost in dollars.
    #[serde(default)]
    pub cost: Cost,
}

impl Usage {
    /// Recalculate `total_tokens` and per-component costs from raw token counts.
    pub fn calculate_cost(&mut self) {
        self.total_tokens = self.input + self.output + self.cache_read + self.cache_write;
        self.cost.input = (self.input as f64) / 1_000_000.0;
        self.cost.output = (self.output as f64) / 1_000_000.0;
        self.cost.cache_read = (self.cache_read as f64) / 1_000_000.0;
        self.cost.cache_write = (self.cache_write as f64) / 1_000_000.0;
    }
}

/// Compatibility settings for OpenAI-compatible APIs.
///
/// Not every OpenAI-compatible provider supports every feature.
/// These flags let the streaming layer adapt its request shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatSettings {
    /// Whether the provider supports the `store` parameter.
    #[serde(default = "default_true")]
    pub supports_store: bool,
    /// Whether the provider recognises the `developer` role.
    #[serde(default = "default_true")]
    pub supports_developer_role: bool,
    /// Whether the provider supports `reasoning_effort`.
    #[serde(default = "default_true")]
    pub supports_reasoning_effort: bool,
    /// Whether the provider returns usage data in streaming responses.
    #[serde(default = "default_true")]
    pub supports_usage_in_streaming: bool,
    /// Which JSON field name to use for the max-tokens parameter.
    #[serde(default)]
    pub max_tokens_field: Option<MaxTokensField>,
    /// Whether tool results must include the tool name.
    #[serde(default = "default_false")]
    pub requires_tool_result_name: bool,
    /// Whether an assistant message must follow every tool result.
    #[serde(default = "default_false")]
    pub requires_assistant_after_tool_result: bool,
    /// Whether thinking should be sent as plain text.
    #[serde(default = "default_false")]
    pub requires_thinking_as_text: bool,
    /// Provider-specific thinking wire-format.
    #[serde(default)]
    pub thinking_format: Option<ThinkingFormat>,
}

fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}

/// Which JSON field to use for the maximum output token count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaxTokensField {
    /// Use `max_completion_tokens`.
    MaxCompletionTokens,
    /// Use `max_tokens`.
    MaxTokens,
}

/// Provider-specific wire format for extended thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingFormat {
    /// OpenAI native thinking format.
    OpenAI,
    /// OpenRouter thinking format.
    OpenRouter,
    /// DeepSeek thinking format.
    DeepSeek,
    /// Zai thinking format.
    Zai,
    /// Qwen API thinking format.
    Qwen,
    /// Qwen chat-template thinking format.
    QwenChatTemplate,
}

/// Tool result returned by agent tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// ID of the tool call this result corresponds to.
    pub tool_call_id: String,
    /// Human-readable result or error text.
    pub content: String,
    /// `"success"` or `"error"`.
    pub status: String,
}

impl ToolResult {
    /// Create a successful tool result.
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            status: "success".to_string(),
        }
    }

    /// Create an error tool result.
    pub fn error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            status: "error".to_string(),
        }
    }

    /// Returns `true` if this result represents an error.
    pub fn is_error(&self) -> bool {
        self.status == "error"
    }
}

/// LLM model definition.
///
/// Describes a model's capabilities, endpoint, and cost structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Unique model identifier (e.g. `"gpt-4o"`, `"claude-3-5-sonnet"`).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Which API dialect this model speaks.
    pub api: Api,
    /// Provider name (e.g. `"openai"`, `"anthropic"`).
    pub provider: String,
    /// Base URL for the provider API.
    pub base_url: String,
    /// Whether this model supports extended reasoning / thinking.
    #[serde(default)]
    pub reasoning: bool,
    /// Supported input modalities.
    #[serde(default)]
    pub input: Vec<InputModality>,
    /// Pricing information.
    #[serde(default)]
    pub cost: Cost,
    /// Maximum context window in tokens.
    pub context_window: usize,
    /// Maximum output tokens per request.
    pub max_tokens: usize,
    /// Extra HTTP headers to send with every request.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Compatibility tweaks for non-standard providers.
    #[serde(default)]
    pub compat: Option<CompatSettings>,
}

impl Model {
    /// Create a new model with sensible defaults.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        api: Api,
        provider: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            api,
            provider: provider.into(),
            base_url: base_url.into(),
            reasoning: false,
            input: vec![InputModality::Text],
            cost: Cost::default(),
            context_window: 128_000,
            max_tokens: 32_000,
            headers: HashMap::new(),
            compat: None,
        }
    }

    /// Returns `true` if the model accepts image inputs.
    pub fn supports_vision(&self) -> bool {
        self.input.contains(&InputModality::Image)
    }

    /// Returns `true` if the model supports extended reasoning.
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_roundtrip() {
        let mut model = Model::new(
            "gpt-4o",
            "GPT-4o",
            Api::OpenAiCompletions,
            "openai",
            "https://api.openai.com/v1",
        );
        model.reasoning = true;
        model.input.push(InputModality::Image);
        model.cost = Cost {
            input: 5.0,
            output: 15.0,
            cache_read: 2.5,
            cache_write: 0.0,
        };
        model.compat = Some(CompatSettings::default());

        let json = serde_json::to_string(&model).unwrap();
        let deserialized: Model = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "gpt-4o");
        assert_eq!(deserialized.name, "GPT-4o");
        assert_eq!(deserialized.api, Api::OpenAiCompletions);
        assert_eq!(deserialized.provider, "openai");
        assert!(deserialized.reasoning);
        assert!(deserialized.supports_vision());
        assert!(deserialized.supports_reasoning());
        assert_eq!(deserialized.cost.input, 5.0);
        assert_eq!(deserialized.cost.output, 15.0);
    }

    #[test]
    fn usage_calculate_cost() {
        let mut usage = Usage {
            input: 1_000_000,
            output: 500_000,
            cache_read: 200_000,
            cache_write: 100_000,
            ..Default::default()
        };
        usage.calculate_cost();

        assert_eq!(usage.total_tokens, 1_800_000);
        assert_eq!(usage.cost.input, 1.0);
        assert_eq!(usage.cost.output, 0.5);
        assert_eq!(usage.cost.cache_read, 0.2);
        assert_eq!(usage.cost.cache_write, 0.1);
    }

    #[test]
    fn cost_total() {
        let cost = Cost {
            input: 3.0,
            output: 6.0,
            cache_read: 1.0,
            cache_write: 0.5,
        };
        assert!((cost.total() - 10.5).abs() < f64::EPSILON);

        let default_cost = Cost::default();
        assert_eq!(default_cost.total(), 0.0);
    }

    #[test]
    fn api_display() {
        assert_eq!(Api::OpenAiCompletions.to_string(), "openai-completions");
        assert_eq!(Api::OpenAiResponses.to_string(), "openai-responses");
        assert_eq!(Api::AnthropicMessages.to_string(), "anthropic-messages");
        assert_eq!(Api::GoogleGenerativeAi.to_string(), "google-generative-ai");
        assert_eq!(Api::GoogleVertex.to_string(), "google-vertex");
        assert_eq!(Api::MistralConversations.to_string(), "mistral-conversations");
        assert_eq!(Api::AzureOpenAiResponses.to_string(), "azure-openai-responses");
        assert_eq!(Api::BedrockConverseStream.to_string(), "bedrock-converse-stream");
    }

    #[test]
    fn api_serde_roundtrip() {
        for api in [
            Api::OpenAiCompletions,
            Api::OpenAiResponses,
            Api::AnthropicMessages,
            Api::GoogleGenerativeAi,
            Api::GoogleVertex,
            Api::MistralConversations,
            Api::AzureOpenAiResponses,
            Api::BedrockConverseStream,
        ] {
            let json = serde_json::to_string(&api).unwrap();
            let back: Api = serde_json::from_str(&json).unwrap();
            assert_eq!(api, back);
        }
    }

    #[test]
    fn thinking_level_serde() {
        for level in [
            ThinkingLevel::Off,
            ThinkingLevel::Minimal,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::XHigh,
        ] {
            let json = serde_json::to_string(&level).unwrap();
            let back: ThinkingLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(level, back);
        }
        // Verify default
        assert_eq!(ThinkingLevel::default(), ThinkingLevel::Off);
        // Verify rename values
        assert_eq!(serde_json::to_string(&ThinkingLevel::High).unwrap(), "\"high\"");
        assert_eq!(serde_json::to_string(&ThinkingLevel::Off).unwrap(), "\"off\"");
        // as_str
        assert!(ThinkingLevel::Off.as_str().is_none());
        assert_eq!(ThinkingLevel::High.as_str(), Some("high"));
        assert_eq!(ThinkingLevel::XHigh.as_str(), Some("xhigh"));
    }

    #[test]
    fn stop_reason_serde() {
        assert_eq!(
            serde_json::to_string(&StopReason::ToolUse).unwrap(),
            "\"toolUse\""
        );
        let back: StopReason = serde_json::from_str("\"toolUse\"").unwrap();
        assert_eq!(back, StopReason::ToolUse);
    }

    #[test]
    fn tool_result_helpers() {
        let success = ToolResult::success("call_1", "result text");
        assert_eq!(success.tool_call_id, "call_1");
        assert_eq!(success.content, "result text");
        assert_eq!(success.status, "success");
        assert!(!success.is_error());

        let error = ToolResult::error("call_2", "something failed");
        assert!(error.is_error());
        assert_eq!(error.status, "error");
    }

    #[test]
    fn cache_retention_default() {
        assert_eq!(CacheRetention::default(), CacheRetention::None);
    }
}
