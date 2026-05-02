//! Agent configuration

use serde::{Deserialize, Serialize};

/// Agent runtime configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent name
    pub name: String,
    /// Agent description
    pub description: Option<String>,
    /// Model ID to use
    pub model_id: String,
    /// System prompt
    pub system_prompt: Option<String>,
    /// Maximum number of agent iterations
    pub max_iterations: usize,
    /// Timeout in seconds for the entire agent run
    pub timeout_seconds: u64,
    /// Temperature for generation (0.0 to 1.0)
    pub temperature: Option<f64>,
    /// Maximum tokens to generate
    pub max_tokens: Option<usize>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: "oxi-agent".to_string(),
            description: None,
            model_id: "claude-sonnet-4-20250514".to_string(),
            system_prompt: None,
            max_iterations: 10,
            timeout_seconds: 300,
            temperature: None,
            max_tokens: None,
        }
    }
}

impl AgentConfig {
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            ..Default::default()
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }
}
