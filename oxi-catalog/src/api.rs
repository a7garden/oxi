//! The `Api` enum — the wire-format / protocol dialect spoken to a particular
//! LLM provider.
//!
//! Rust port of omp's `KnownApi` union (`packages/catalog/src/types.ts:8-22`).
//! Selects which streaming transport function handles a model; carries **no
//! provider identity** (identity lives in `oxi-ai`'s `ProviderDefinition`
//! registry, mirroring omp's three-way split: transport / auth-login /
//! model-host metadata).
//!
//! Variants without a wired transport yet route to `None` in the provider
//! factory's `_` match arm — they land as P0.5 ports each provider's stream
//! function (Ollama, Cursor, Devin, GitLab Duo are the remote-AGENT protocols).

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
    /// GitLab Duo (remote-AGENT protocol).
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
            Api::GitLabDuoAgent => "gitlab-duo-agent",
            Api::DevinAgent => "devin-agent",
        };
        f.write_str(s)
    }
}
