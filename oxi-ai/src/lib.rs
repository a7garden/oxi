#![allow(unused_doc_comments)]
#![warn(missing_docs)]
#![warn(clippy::unwrap_used)]
#![allow(clippy::unwrap_used_in_tests)]

//! oxi-ai: Unified LLM API for oxi
//!
//! This crate provides a unified interface for interacting with multiple LLM providers.
//! It handles streaming, tool calling, context management, and cross-provider handoffs.

mod compaction;
mod context;
pub mod env_api_keys;
mod error;
mod high_level;
mod messages;
pub mod oauth;
pub mod provider_registry;
mod providers;
pub use providers::register_builtins;
pub mod secret;
mod tools;
mod transform;
mod types;
pub mod utils;

/// Standard imports for oxi-ai usage.
pub mod prelude {
    pub use crate::compaction::{
        CompactedContext, CompactionManager, CompactionStrategy, Compactor, LlmCompactor,
    };
    pub use crate::context::Context;
    pub use crate::error::{Error, Result};
    pub use crate::messages::*;
    pub use crate::providers::{Provider, ProviderEvent, StreamOptions};
    pub use crate::tools::{validate_args, Tool};
    pub use crate::types::*;
}

// Re-export main types

/// Provider-specific error type for LLM operations.
pub use crate::error::ProviderError;

/// Shared conversation context.
pub use context::Context;

/// Result type alias for oxi-ai operations.
pub use error::{Error, Result};

/// Message types for constructing conversations.
pub use messages::*;

/// Cache retention control for provider requests.
pub use providers::CacheRetention;

/// Provider trait and streaming options.
pub use providers::{get_provider, register_provider, unregister_provider, custom_provider_names, Provider, ProviderEvent, StreamOptions};

/// OpenAI-compatible provider implementation.
pub use providers::OpenAiProvider;

/// Model fetching utilities (async and blocking).
pub use providers::model_fetch::{fetch_models_async, fetch_models_blocking};

/// OpenAI Responses API provider.
pub use providers::OpenAiResponsesProvider;

/// Tool definition and argument validation.
pub use tools::{progress_callback, validate_args, ProgressCallback, Tool, ToolValidationError};

/// Core type definitions (tokens, cost, etc.).
pub use types::*;

// High-level API

/// Token estimation and context usage helpers.
pub use high_level::tokens::{context_usage, estimate, estimate_words};

/// High-level completion and token estimation.
pub use high_level::{complete, estimate_tokens};

// Context compaction

/// Compaction strategies and managers for long conversations.
pub use compaction::{
    CompactedContext, CompactionManager, CompactionStrategy, Compactor, LlmCompactor,
};

// Cross-provider message transformation

/// Message transformation between provider formats.
pub use transform::{
    anthropic_to_google, anthropic_to_openai, google_to_openai, normalize_tool_call_id,
    openai_to_anthropic, transform_messages, transform_messages_for_model, TransformOptions,
};

// Model registry (runtime mutable registry)
mod model_registry;

/// Runtime model registry for dynamically registered models.
///
/// Unlike the static `model_db`, this supports adding/removing models at runtime.
pub use model_registry::{get_model, get_models, get_providers, register_model, unregister_model, lookup_model, ModelRegistry};

// Static model database (comprehensive)
pub mod model_db;

/// Static database of known models with cost and modality info.
///
/// Provides comprehensive model listings, filtering, and search capabilities.
pub use model_db::{
    get_all_models, get_cheapest_models, get_model_entry, get_provider_models,
    get_reasoning_models, get_vision_models, model_count, search_models, ModelEntry,
};

/// Re-export AssistantMessage from messages
pub use messages::AssistantMessage;

// Environment-based API key resolution

/// Utilities for discovering API keys from the environment.
pub use env_api_keys::{find_env_keys, get_all_env_keys, get_env_api_key};

// Provider authentication registry

/// OAuth token and API key management for providers.
pub use provider_registry::{OAuthTokenInfo, ProviderAuth, ProviderAuthRegistry};
