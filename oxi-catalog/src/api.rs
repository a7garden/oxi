//! The `Api` enum — the wire-format / protocol dialect spoken to a particular
//! LLM provider.
//!
//! This is the Rust port of omp's `KnownApi` union (`packages/catalog/src/types.ts`).
//! It selects which streaming transport function handles a model; it carries
//! **no provider identity** (identity lives in `oxi-ai`'s `ProviderDefinition`
//! registry, mirroring omp's three-way split: transport / auth-login /
//! model-host metadata).
//!
//! NOTE (P0 roadmap): this enum currently holds oxi's historical 8 variants.
//! `docs/superpowers/specs/2026-07-27-omp-realignment-design.md` (P0-C) expands
//! it to omp's 14 `KnownApi` dialects and removes the incorrect
//! `MistralConversations` variant (omp treats Mistral as `openai-completions`).

use serde::{Deserialize, Serialize};
use std::fmt;

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
