//! The `Api` enum — the wire-format / protocol dialect spoken to a particular
//! LLM provider.
//!
//! Rust port of omp's `KnownApi` union (`packages/catalog/src/types.ts:8-22`).
//! Selects which streaming transport function handles a model; carries **no
//! provider identity** (identity lives in `oxicode-ai`'s `ProviderDefinition`
//! registry, mirroring omp's three-way split: transport / auth-login /
//! model-host metadata).
//!
//! Each enum variant maps to a concrete dispatch in
//! `oxicode-ai/src/providers/register_builtins::build_builtin_transport`. The
//! two remaining gap variants are `Api::OpenAiCodexResponses` and
//! `Api::GoogleGeminiCli`: Codex reuses the OpenAI Responses transport, and
//! Gemini CLI is a typed stub (`GeminiCliProvider` returning
//! `ProviderError::NotImplemented`) until a real protocol is integrated.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Provider API identifier.
///
/// Selects the wire-format / protocol dialect spoken to a particular LLM
/// provider. The canonical 14 `KnownApi` dialects from omp; `Mistral` is
/// intentionally absent — omp treats Mistral as `openai-completions`-compatible
/// (no separate dialect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Api {
    /// OpenAI Chat Completions API (also: Mistral, DeepSeek, Together, …).
    #[serde(rename = "openai-completions")]
    OpenAiCompletions,
    /// OpenAI Responses API.
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// OpenRouter.
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// OpenAI Codex Responses API.
    #[serde(rename = "openai-codex-responses")]
    OpenAiCodexResponses,
    /// Azure OpenAI Responses API.
    #[serde(rename = "azure-openai-responses")]
    AzureOpenAiResponses,
    /// Anthropic Messages API.
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
    /// AWS Bedrock Converse Stream API.
    #[serde(rename = "bedrock-converse-stream")]
    BedrockConverseStream,
    /// Google Generative AI (Gemini) API.
    #[serde(rename = "google-generative-ai")]
    GoogleGenerativeAi,
    /// Google Gemini CLI (remote-AGENT protocol).
    #[serde(rename = "google-gemini-cli")]
    GoogleGeminiCli,
    /// Google Vertex AI endpoint.
    #[serde(rename = "google-vertex")]
    GoogleVertex,
    /// Ollama chat API (local server).
    #[serde(rename = "ollama-chat")]
    OllamaChat,
    /// Cursor (remote-AGENT protocol).
    #[serde(rename = "cursor-agent")]
    CursorAgent,
    /// GitLab Duo REST proxy (AI Gateway — delegates to Anthropic/OpenAI).
    #[serde(rename = "gitlab-duo")]
    GitLabDuo,
    /// GitLab Duo Agent (WebSocket workflow protocol).
    #[serde(rename = "gitlab-duo-agent")]
    GitLabDuoAgent,
    /// Devin (remote-AGENT protocol).
    #[serde(rename = "devin-agent")]
    DevinAgent,
}

impl fmt::Display for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Api::OpenAiCompletions => "openai-completions",
            Api::OpenAiResponses => "openai-responses",
            Api::OpenRouter => "openrouter",
            Api::OpenAiCodexResponses => "openai-codex-responses",
            Api::AzureOpenAiResponses => "azure-openai-responses",
            Api::AnthropicMessages => "anthropic-messages",
            Api::BedrockConverseStream => "bedrock-converse-stream",
            Api::GoogleGenerativeAi => "google-generative-ai",
            Api::GoogleGeminiCli => "google-gemini-cli",
            Api::GoogleVertex => "google-vertex",
            Api::OllamaChat => "ollama-chat",
            Api::CursorAgent => "cursor-agent",
            Api::GitLabDuo => "gitlab-duo",
            Api::GitLabDuoAgent => "gitlab-duo-agent",
            Api::DevinAgent => "devin-agent",
        };
        f.write_str(s)
    }
}

impl Api {
    /// Parse a kebab-case dialect string (omp `KnownApi` serialization) into
    /// an `Api`. Returns `None` for unrecognized strings — the single
    /// authoritative parser (callers decide the fallback, e.g.
    /// OpenAI-compatible default for unknown gateways/aggregators).
    ///
    /// This exists so the dialect↔string mapping lives with the enum (not
    /// duplicated as stale `parse_api` matches across crates that miss new
    /// variants).
    pub fn from_kebab_str(s: &str) -> Option<Self> {
        Some(match s {
            "openai-completions" => Api::OpenAiCompletions,
            "openai-responses" => Api::OpenAiResponses,
            "openrouter" => Api::OpenRouter,
            "openai-codex-responses" => Api::OpenAiCodexResponses,
            "azure-openai-responses" => Api::AzureOpenAiResponses,
            "anthropic-messages" => Api::AnthropicMessages,
            "bedrock-converse-stream" => Api::BedrockConverseStream,
            "google-generative-ai" => Api::GoogleGenerativeAi,
            "google-gemini-cli" => Api::GoogleGeminiCli,
            "google-vertex" => Api::GoogleVertex,
            "ollama-chat" => Api::OllamaChat,
            "cursor-agent" => Api::CursorAgent,
            "gitlab-duo" => Api::GitLabDuo,
            "gitlab-duo-agent" => Api::GitLabDuoAgent,
            "devin-agent" => Api::DevinAgent,
            _ => return None,
        })
    }
}
