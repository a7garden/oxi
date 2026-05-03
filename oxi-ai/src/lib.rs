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
mod tools;
mod transform;
mod types;

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
pub use crate::error::ProviderError;
pub use context::Context;
pub use error::{Error, Result};
pub use messages::*;
pub use providers::CacheRetention;
pub use providers::{get_provider, Provider, ProviderEvent, StreamOptions};
pub use tools::{progress_callback, validate_args, ProgressCallback, Tool, ValidationError};
pub use types::*;

// High-level API
pub use high_level::tokens::{context_usage, estimate, estimate_words};
pub use high_level::{complete, estimate_tokens};

// Context compaction
pub use compaction::{
    CompactedContext, CompactionManager, CompactionStrategy, Compactor, LlmCompactor,
};

// Cross-provider message transformation
pub use transform::{
    anthropic_to_google, anthropic_to_openai, google_to_openai, openai_to_anthropic,
    transform_messages, TransformOptions,
};

// Model registry (runtime mutable registry)
mod model_registry;
pub use model_registry::{get_model, get_models, get_providers, ModelRegistry};

// Static model database (comprehensive, auto-generated from pi-mono)
pub mod model_db;
pub use model_db::{
    get_all_models, get_cheapest_models, get_model_entry, get_provider_models,
    get_reasoning_models, get_vision_models, model_count, search_models, ModelEntry,
};

/// Re-export AssistantMessage from messages
pub use messages::AssistantMessage;

// Environment-based API key resolution
pub use env_api_keys::{find_env_keys, get_all_env_keys, get_env_api_key};

// Provider authentication registry
pub use provider_registry::{OAuthTokenInfo, ProviderAuth, ProviderAuthRegistry};
