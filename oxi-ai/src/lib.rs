//! oxi-ai: Unified LLM API for oxi
//!
//! This crate provides a unified interface for interacting with multiple LLM providers.
//! It handles streaming, tool calling, context management, and cross-provider handoffs.

mod types;
mod messages;
mod context;
mod tools;
mod providers;
mod error;
mod high_level;
mod compaction;
mod transform;
pub mod oauth;
pub mod env_api_keys;
pub mod provider_registry;

pub mod prelude {
    pub use crate::types::*;
    pub use crate::messages::*;
    pub use crate::context::Context;
    pub use crate::tools::{Tool, validate_args};
    pub use crate::providers::{Provider, ProviderEvent, StreamOptions};
    pub use crate::error::{Error, Result};
    pub use crate::compaction::{Compactor, CompactionStrategy, CompactionManager, CompactedContext, LlmCompactor};
}

// Re-export main types
pub use types::*;
pub use messages::*;
pub use context::Context;
pub use tools::{Tool, validate_args, ValidationError, ProgressCallback, progress_callback};
pub use providers::{Provider, ProviderEvent, StreamOptions, get_provider};
pub use providers::CacheRetention;
pub use crate::error::ProviderError;
pub use error::{Error, Result};

// High-level API
pub use high_level::{complete, estimate_tokens};
pub use high_level::tokens::{estimate, estimate_words, context_usage};

// Context compaction
pub use compaction::{Compactor, CompactionStrategy, CompactionManager, CompactedContext, LlmCompactor};

// Cross-provider message transformation
pub use transform::{transform_messages, TransformOptions, anthropic_to_openai, openai_to_anthropic, google_to_openai, anthropic_to_google};

// Model registry (runtime mutable registry)
mod model_registry;
pub use model_registry::{ModelRegistry, get_model, get_providers, get_models};

// Static model database (comprehensive, auto-generated from pi-mono)
pub mod model_db;
pub use model_db::{ModelEntry, get_model_entry, get_provider_models, get_all_models, model_count, search_models, get_reasoning_models, get_vision_models, get_cheapest_models};

// Environment-based API key resolution
pub use env_api_keys::{get_env_api_key, find_env_keys, get_all_env_keys};

// Provider authentication registry
pub use provider_registry::{ProviderAuthRegistry, ProviderAuth, OAuthTokenInfo};

/// Re-export AssistantMessage from messages
pub use messages::AssistantMessage;