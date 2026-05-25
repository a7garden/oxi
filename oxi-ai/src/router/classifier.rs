//! LLM-based classifier for routing decisions (stub).

use anyhow::Result;

/// Placeholder classifier — full implementation in Phase 2.
#[derive(Debug, Clone, Default)]
pub struct LlmClassifier {
    /// The model to use for classification (not yet used).
    pub model: Option<String>,
}

impl LlmClassifier {
    /// Create a new classifier stub.
    pub fn new(model: Option<String>) -> Self {
        Self { model }
    }

    /// Classify a user message to determine routing tier.
    pub async fn classify(&self, _prompt: &str) -> Result<f64> {
        anyhow::bail!("LlmClassifier is not ready — model: {:?}", self.model)
    }
}
