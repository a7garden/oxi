#![allow(unused_doc_comments)]
#![warn(missing_docs)]
// Relax two test-idiom lints under `cfg(test)` so `cargo clippy --all-targets`
// stays clean without weakening the shipped library:
//   - `clippy::unwrap_used` — `unwrap()`/`unwrap_err()` are idiomatic in tests;
//     shipped (non-test) code still `warn`s on it (see the line below).
//   - `clippy::field_reassign_with_default` — the `let mut x = X::default();
//     x.f = ..;` test-setup pattern.
#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::field_reassign_with_default))]

//! oxi-ai: Unified LLM API for oxi
//!
//! This crate provides a unified interface for interacting with multiple LLM providers.
//! It handles streaming, tool calling, context management, and cross-provider handoffs.

// Catalog moved to the `oxi-catalog` crate (omp aligns `pi-catalog` as a
// separate package). Re-exported here for backward compatibility during the
// migration; new code should depend on `oxi-catalog` directly.
pub use oxi_catalog::catalog;
pub mod circuit_breaker;
pub mod compaction;
pub mod compaction_seam;
mod complexity_router;
mod context;
pub mod env_api_keys;
mod error;
mod high_level;
mod messages;
pub mod oauth;
/// Product home-directory resolution (`OXI_HOME` / `~/.oxi`).
// `product_env` moved to oxi-catalog (it owns catalog cache/override dirs);
// re-exported here so `oxi_ai::product_env` and `oxi_sdk::ports::fs::path`
// (which delegates here) keep working.
pub use oxi_catalog::product_env;
pub mod provider_pool;
pub mod provider_registry;
mod providers;

#[allow(missing_docs)]
pub mod register_builtins {
    pub use crate::providers::register_builtins::*;
}
pub mod router;
pub mod secret;
mod tools;
mod transform;
pub mod types;
pub mod utils;

/// Standard imports for oxi-ai usage.
pub mod prelude {
    pub use crate::compaction::generate_branch_summary;
    pub use crate::compaction::{
        CompactedContext, CompactionManager, CompactionStrategy, Compactor, LlmCompactor,
    };
    pub use crate::context::Context;
    pub use crate::error::{Error, Result};
    pub use crate::messages::*;
    pub use crate::providers::{Provider, ProviderEvent, StreamOptions, StreamResult};
    pub use crate::tools::{Tool, validate_args};
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

/// Provider trait, streaming options, and provider registry.
pub use providers::{
    Provider, ProviderEvent, ProviderOptions, ProviderRegistry, StreamOptions, StreamResult,
    custom_provider_names, get_provider, get_provider_arc, register_provider, unregister_provider,
};

/// Built-in provider helpers (re-exported from providers).
pub use providers::register_builtins::{
    create_builtin_provider, create_builtin_provider_with_options, get_all_provider_names,
    get_builtin_provider, get_provider_env_key, get_provider_env_keys, is_builtin_provider,
};

/// OpenAI-compatible provider implementation.
pub use providers::OpenAiProvider;

/// Anthropic provider implementation.
pub use providers::AnthropicProvider;
/// Azure OpenAI provider implementation.
pub use providers::AzureProvider;

/// Model fetching utilities (async and blocking).
pub use providers::model_fetch::{fetch_models_async, fetch_models_blocking};

/// OpenAI Responses API provider.
pub use providers::OpenAiResponsesProvider;

/// AWS Bedrock provider implementation.
pub use providers::BedrockProvider;
/// Google (Gemini) provider implementation.
pub use providers::GoogleProvider;
/// Mistral provider implementation.
pub use providers::MistralProvider;
/// Google Vertex AI provider implementation.
pub use providers::VertexProvider;

/// Provider-specific message normalization (empty content filtering, tool ID
/// scrubbing, reasoning injection, tool-use ordering fix).
pub use providers::normalize_messages;

/// Tool definition and argument validation.
pub use tools::{ProgressCallback, Tool, ToolValidationError, progress_callback, validate_args};

pub use compaction::generate_branch_summary;
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
    CompactedContext, CompactionManager, CompactionStrategy, Compactor, ContextTransformer,
    LlmCompactor, NoopContextTransformer,
};

// Complexity-based routing

/// Complexity-based model routing.
pub use complexity_router::{ComplexityRouter, DefaultRouter};

// Cross-provider message transformation

/// Message transformation between provider formats.
pub use transform::{
    TransformOptions, anthropic_to_google, anthropic_to_openai, google_to_openai,
    normalize_tool_call_id, openai_to_anthropic, transform_messages, transform_messages_for_model,
};

// Multi-provider routing

/// MultiProvider for intelligent routing with fallback support.
pub mod multi_provider;

/// Re-exports for multi_provider convenience.
pub use multi_provider::MultiProvider;

// Model registry (runtime mutable registry)
mod model_registry;

/// Runtime model registry for dynamically registered models.
///
/// Unlike the static `model_db`, this supports adding/removing models at runtime.
pub use model_registry::{
    ModelRegistry, dynamic_models, get_model, get_models, get_providers, lookup_model,
    register_model, unregister_model,
};

// Static model database (comprehensive)
pub mod model_db;

/// Static database of known models with cost and modality info.
///
/// Provides comprehensive model listings, filtering, and search capabilities.
pub use model_db::{
    ModelEntry, get_all_models, get_cheapest_models, get_model_entry, get_provider_models,
    get_reasoning_models, get_vision_models, model_count, search_models,
};

// Fallback chain for ordered model failover
pub mod fallback_chain;

/// Ordered fallback chain for model failover on failure.
pub use fallback_chain::{FallbackChain, FallbackChainError};

// Model roles — named model assignments (ported from omp)

/// Named model roles with `pi/<role>` alias resolution.
pub mod roles;

/// Re-exports for the roles module.
pub use roles::{
    ModelRole, RoleColor, RoleInfo, RoleRegistry, builtin_role_info, builtin_visible_ids,
};

// Role switching — signal-based role decision on top of the roles registry

/// Role-switching decision engine (signals -> role -> model).
pub mod role_switcher;

/// Re-exports for the role_switcher module.
pub use role_switcher::{
    DEFAULT_LONG_CONTEXT_THRESHOLD, RoleSignals, decide_role, resolve_role_to_model, role_for_tool,
};

/// Re-exports for the live role registry (UI <-> provider shared state).
pub use roles::{live_role_registry, set_live_role_registry};

// Role-routing provider — plugs role switching into the live agent loop

/// Provider wrapper that routes each request to the role-selected model.
pub mod role_routing;

/// Re-export the role-routing provider.
pub use role_routing::RoleRoutingProvider;

// Partial response for stream recovery
pub mod partial_response;

/// Partial response accumulator for stream recovery.
pub use partial_response::PartialResponse;

// Circuit breaker for provider health tracking
/// Per-provider circuit breaker for health tracking.
pub use circuit_breaker::{CircuitBreakerConfig, CircuitOpenError, ProviderCircuitBreaker};

/// Re-export AssistantMessage from messages
pub use messages::AssistantMessage;

// Environment-based API key resolution

/// Utilities for discovering API keys from the environment.
pub use env_api_keys::{find_env_keys, get_all_env_keys, get_env_api_key};

/// Product home-directory resolution (`OXI_HOME` → `~/.oxi`).
pub use product_env::home_dir as product_home_dir;

// Provider authentication registry

/// OAuth token and API key management for providers.
pub use provider_registry::{OAuthTokenInfo, ProviderAuth, ProviderAuthRegistry};
