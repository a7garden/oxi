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

pub mod prelude {
    pub use crate::types::*;
    pub use crate::messages::*;
    pub use crate::context::Context;
    pub use crate::tools::{Tool, validate_args};
    pub use crate::providers::{Provider, ProviderEvent, StreamOptions};
    pub use crate::error::{Error, Result};
}

// Re-export main types
pub use types::*;
pub use messages::*;
pub use context::Context;
pub use tools::{Tool, validate_args, ValidationError};
pub use providers::{Provider, ProviderEvent, StreamOptions, get_provider};
pub use providers::CacheRetention;
pub use crate::error::ProviderError;
pub use error::{Error, Result};

// High-level API
pub use high_level::{complete, estimate_tokens};
pub use high_level::tokens::{estimate, estimate_words, context_usage};

// Model registry
mod model_registry;
pub use model_registry::{ModelRegistry, get_model, get_providers, get_models};

/// Re-export AssistantMessage from messages
pub use messages::AssistantMessage;