//! Core domain types for oxi-ai

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

/// Provider API identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Api {
    #[serde(rename = "openai-completions")]
    OpenAiCompletions,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
    #[serde(rename = "google-generative-ai")]
    GoogleGenerativeAi,
    #[serde(rename = "google-vertex")]
    GoogleVertex,
    #[serde(rename = "mistral-conversations")]
    MistralConversations,
    #[serde(rename = "azure-openai-responses")]
    AzureOpenAiResponses,
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
    None,
    Short,
    Long,
}

/// Model thinking/reasoning level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ThinkingLevel {
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
    Text,
    Image,
}

/// Cost structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cost {
    #[serde(default)]
    pub input: f64, // $/million tokens
    #[serde(default)]
    pub output: f64, // $/million tokens
    #[serde(default)]
    pub cache_read: f64, // $/million tokens
    #[serde(default)]
    pub cache_write: f64, // $/million tokens
}

impl Cost {
    pub fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// Stop reason
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

/// Token usage
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input: usize,
    #[serde(default)]
    pub output: usize,
    #[serde(default)]
    pub cache_read: usize,
    #[serde(default)]
    pub cache_write: usize,
    #[serde(default)]
    pub total_tokens: usize,
    #[serde(default)]
    pub cost: Cost,
}

impl Usage {
    pub fn calculate_cost(&mut self) {
        self.total_tokens = self.input + self.output + self.cache_read + self.cache_write;
        self.cost.input = (self.input as f64) / 1_000_000.0;
        self.cost.output = (self.output as f64) / 1_000_000.0;
        self.cost.cache_read = (self.cache_read as f64) / 1_000_000.0;
        self.cost.cache_write = (self.cache_write as f64) / 1_000_000.0;
    }
}

/// Compatibility settings for OpenAI-compatible APIs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatSettings {
    #[serde(default = "default_true")]
    pub supports_store: bool,
    #[serde(default = "default_true")]
    pub supports_developer_role: bool,
    #[serde(default = "default_true")]
    pub supports_reasoning_effort: bool,
    #[serde(default = "default_true")]
    pub supports_usage_in_streaming: bool,
    #[serde(default)]
    pub max_tokens_field: Option<MaxTokensField>,
    #[serde(default = "default_false")]
    pub requires_tool_result_name: bool,
    #[serde(default = "default_false")]
    pub requires_assistant_after_tool_result: bool,
    #[serde(default = "default_false")]
    pub requires_thinking_as_text: bool,
    #[serde(default)]
    pub thinking_format: Option<ThinkingFormat>,
}

fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaxTokensField {
    MaxCompletionTokens,
    MaxTokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingFormat {
    OpenAI,
    OpenRouter,
    DeepSeek,
    Zai,
    Qwen,
    QwenChatTemplate,
}

// Tool result (for agent tool execution results)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub status: String,
}

impl ToolResult {
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            status: "success".to_string(),
        }
    }

    pub fn error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            status: "error".to_string(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.status == "error"
    }
}

/// LLM model definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: String,
    pub base_url: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub input: Vec<InputModality>,
    #[serde(default)]
    pub cost: Cost,
    pub context_window: usize,
    pub max_tokens: usize,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub compat: Option<CompatSettings>,
}

impl Model {
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

    pub fn supports_vision(&self) -> bool {
        self.input.contains(&InputModality::Image)
    }

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
