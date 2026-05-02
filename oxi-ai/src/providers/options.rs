//! Stream options for providers

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use super::{CacheRetention, ThinkingLevel};

/// Options for streaming requests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamOptions {
    /// Sampling temperature (0.0 to 2.0)
    #[serde(default)]
    pub temperature: Option<f64>,
    
    /// Maximum tokens to generate
    #[serde(default)]
    pub max_tokens: Option<usize>,
    
    /// API key (overrides environment variable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    
    /// Cache retention preference
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_retention: Option<CacheRetention>,
    
    /// Session ID for providers that support session-based caching
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    
    /// Custom HTTP headers to include
    #[serde(default)]
    pub headers: HashMap<String, String>,
    
    /// Thinking/reasoning level
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
    
    /// Custom token budgets for thinking levels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budgets: Option<ThinkingBudgets>,
}

impl StreamOptions {
    /// Create new stream options
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set temperature
    pub fn temperature(mut self, temp: f64) -> Self {
        self.temperature = Some(temp);
        self
    }
    
    /// Set max tokens
    pub fn max_tokens(mut self, tokens: usize) -> Self {
        self.max_tokens = Some(tokens);
        self
    }
    
    /// Set API key
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }
    
    /// Set cache retention
    pub fn cache_retention(mut self, retention: CacheRetention) -> Self {
        self.cache_retention = Some(retention);
        self
    }
    
    /// Set session ID
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }
    
    /// Set thinking level
    pub fn thinking_level(mut self, level: ThinkingLevel) -> Self {
        self.thinking_level = Some(level);
        self
    }
}

/// Token budgets for thinking levels
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    #[serde(default)]
    pub minimal: Option<usize>,
    #[serde(default)]
    pub low: Option<usize>,
    #[serde(default)]
    pub medium: Option<usize>,
    #[serde(default)]
    pub high: Option<usize>,
}

impl ThinkingBudgets {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn minimal(mut self, tokens: usize) -> Self {
        self.minimal = Some(tokens);
        self
    }
    
    pub fn low(mut self, tokens: usize) -> Self {
        self.low = Some(tokens);
        self
    }
    
    pub fn medium(mut self, tokens: usize) -> Self {
        self.medium = Some(tokens);
        self
    }
    
    pub fn high(mut self, tokens: usize) -> Self {
        self.high = Some(tokens);
        self
    }
}

pub use super::CacheRetention;
