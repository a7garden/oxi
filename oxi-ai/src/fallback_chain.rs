//! Fallback chain for ordered model failover.
//!
//! A `FallbackChain` manages an ordered list of models to try sequentially
//! when a request fails. This enables automatic failover to backup models
//! without requiring manual intervention.
//!
//! # Usage
//!
//! ```ignore
//! use oxi_ai::fallback_chain::FallbackChain;
//!
//! // Create from provider/model strings
//! let chain = FallbackChain::from_ids(&[
//!     "anthropic/claude-sonnet-4-20250514",
//!     "openai/gpt-4o",
//! ])?;
//!
//! // Iterate through models
//! for model in chain.iter() {
//!     println!("Trying: {}", model.name);
//! }
//!
//! // Get the next model after a failure
//! if let Some(next) = chain.next("anthropic/claude-sonnet-4-20250514") {
//!     println!("Fallback to: {}", next.name);
//! }
//! ```

use crate::model_db::{get_model_entry, ModelEntry};

/// An ordered chain of models for sequential fallback on failure.
///
/// When a model request fails, the chain allows easy iteration to the next
/// available model in priority order. This is useful for implementing
/// automatic failover strategies.
///
/// # Example
///
/// ```ignore
/// use oxi_ai::fallback_chain::FallbackChain;
///
/// // From string IDs
/// let chain = FallbackChain::from_ids(&["openai/gpt-4o", "google/gemini-2.0-flash"])?;
///
/// // Direct construction
/// let models = vec![model1, model2];
/// let chain = FallbackChain::new(models);
///
/// // Find next model after failure
/// if let Some(next) = chain.next("openai/gpt-4o") {
///     // Use next model...
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct FallbackChain {
    /// The ordered list of model entries.
    models: Vec<&'static ModelEntry>,
    /// The original provider/model strings for reference.
    names: Vec<String>,
}

impl Default for FallbackChain {
    /// Creates a default fallback chain with cheap, reliable models.
    ///
    /// The default chain includes models from multiple providers to ensure
    /// redundancy and cost efficiency. These are selected based on:
    /// - Low input cost
    /// - Wide context window
    /// - Vision support for versatility
    fn default() -> Self {
        // Default chain: prioritize cheap models from different providers
        // Order: cheapest first, then progressively more expensive
        let default_ids = [
            // Free/very cheap models
            "deepseek/deepseek-chat-v3-0324",
            "google/gemini-2.0-flash",
            "openai/gpt-4o-mini",
            "anthropic/claude-3-5-haiku-20241022",
            // Mid-tier reliable models
            "openai/gpt-4o",
            "anthropic/claude-sonnet-4-20250514",
            // Premium models as last resort
            "anthropic/claude-opus-4-20250514",
        ];

        Self::from_ids(&default_ids).expect("Default fallback chain should always be valid")
    }
}

impl FallbackChain {
    /// Creates a new fallback chain from an ordered list of models.
    ///
    /// # Arguments
    ///
    /// * `models` - A vector of model entries in priority order (first = highest priority)
    ///
    /// # Example
    ///
    /// ```ignore
    /// use oxi_ai::model_db::get_model_entry;
    ///
    /// let models = vec![
    ///     get_model_entry("openai", "gpt-4o").unwrap(),
    ///     get_model_entry("anthropic", "claude-sonnet-4-20250514").unwrap(),
    /// ];
    /// let chain = FallbackChain::new(models);
    /// ```
    pub fn new(models: Vec<&'static ModelEntry>) -> Self {
        let names: Vec<String> = models
            .iter()
            .map(|m| format!("{}/{}", m.provider, m.id))
            .collect();

        Self { models, names }
    }

    /// Creates a fallback chain from "provider/model" ID strings.
    ///
    /// Each string must be in the format `"provider/model-id"`, for example:
    /// - `"anthropic/claude-sonnet-4-20250514"`
    /// - `"openai/gpt-4o"`
    /// - `"google/gemini-2.0-flash"`
    ///
    /// # Arguments
    ///
    /// * `ids` - Slice of strings in `"provider/model"` format
    ///
    /// # Errors
    ///
    /// Returns a `FallbackChainError` if any model ID cannot be found in the database.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&[
    ///     "anthropic/claude-sonnet-4-20250514",
    ///     "openai/gpt-4o",
    /// ])?;
    /// ```
    pub fn from_ids(ids: &[&str]) -> Result<Self, FallbackChainError> {
        let mut models: Vec<&'static ModelEntry> = Vec::with_capacity(ids.len());
        let mut names: Vec<String> = Vec::with_capacity(ids.len());

        for id in ids {
            let (provider, model_id) = match id.split_once('/') {
                Some((p, m)) => (p, m),
                None => {
                    return Err(FallbackChainError::InvalidFormat {
                        id: id.to_string(),
                        reason: "Expected 'provider/model' format".to_string(),
                    });
                }
            };

            match get_model_entry(provider, model_id) {
                Some(entry) => {
                    models.push(entry);
                    names.push(id.to_string());
                }
                None => {
                    return Err(FallbackChainError::ModelNotFound {
                        id: id.to_string(),
                        provider: provider.to_string(),
                        model_id: model_id.to_string(),
                    });
                }
            }
        }

        Ok(Self { models, names })
    }

    /// Returns the next model in the chain after the current one.
    ///
    /// # Arguments
    ///
    /// * `current` - The current model ID in `"provider/model"` format
    ///
    /// # Returns
    ///
    /// * `Some(&ModelEntry)` - The next model in the chain
    /// * `None` - If the current model is not in the chain, or it's the last model
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b", "c"])?;
    ///
    /// assert_eq!(chain.next("a").map(|m| m.id), Some("b"));
    /// assert_eq!(chain.next("b").map(|m| m.id), Some("c"));
    /// assert_eq!(chain.next("c"), None); // Last in chain
    /// assert_eq!(chain.next("unknown"), None); // Not in chain
    /// ```
    pub fn next(&self, current: &str) -> Option<&'static ModelEntry> {
        let index = self.index_of(current)?;
        let next_index = index + 1;

        if next_index < self.models.len() {
            Some(self.models[next_index])
        } else {
            None
        }
    }

    /// Returns the index of a model in the chain.
    ///
    /// # Arguments
    ///
    /// * `model_id` - The model ID in `"provider/model"` format
    ///
    /// # Returns
    ///
    /// * `Some(usize)` - The zero-based position in the chain
    /// * `None` - If the model is not in the chain
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b", "c"])?;
    ///
    /// assert_eq!(chain.index_of("a"), Some(0));
    /// assert_eq!(chain.index_of("b"), Some(1));
    /// assert_eq!(chain.index_of("c"), Some(2));
    /// assert_eq!(chain.index_of("unknown"), None);
    /// ```
    pub fn index_of(&self, model_id: &str) -> Option<usize> {
        self.names.iter().position(|n| n == model_id)
    }

    /// Returns an iterator over the model entries in the chain.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b", "c"])?;
    ///
    /// for model in chain.iter() {
    ///     println!("Model: {} ({})", model.name, model.provider);
    /// }
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = &'static ModelEntry> {
        self.models.iter().copied()
    }

    /// Returns `true` if the chain contains no models.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::new(vec![]);
    /// assert!(chain.is_empty());
    ///
    /// let chain = FallbackChain::from_ids(&["a"])?;
    /// assert!(!chain.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Returns the number of models in the chain.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b", "c"])?;
    /// assert_eq!(chain.len(), 3);
    /// ```
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Returns a slice of all model entries.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b", "c"])?;
    /// let models: Vec<_> = chain.models();
    /// ```
    pub fn models(&self) -> &[&'static ModelEntry] {
        &self.models
    }

    /// Returns the model ID strings that were used to create the chain.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["openai/gpt-4o", "anthropic/claude-sonnet-4"])?;
    /// assert_eq!(chain.names(), &["openai/gpt-4o", "anthropic/claude-sonnet-4"]);
    /// ```
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Returns the first model in the chain, if any.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b"])?;
    /// assert_eq!(chain.first().map(|m| m.id), Some("a"));
    ///
    /// let empty: FallbackChain = FallbackChain::new(vec![]);
    /// assert_eq!(empty.first(), None);
    /// ```
    pub fn first(&self) -> Option<&'static ModelEntry> {
        self.models.first().copied()
    }

    /// Returns the last model in the chain, if any.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b"])?;
    /// assert_eq!(chain.last().map(|m| m.id), Some("b"));
    ///
    /// let empty: FallbackChain = FallbackChain::new(vec![]);
    /// assert_eq!(empty.last(), None);
    /// ```
    pub fn last(&self) -> Option<&'static ModelEntry> {
        self.models.last().copied()
    }

    /// Checks if the chain contains a specific model.
    ///
    /// # Arguments
    ///
    /// * `model_id` - The model ID in `"provider/model"` format
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b"])?;
    /// assert!(chain.contains("a"));
    /// assert!(!chain.contains("c"));
    /// ```
    pub fn contains(&self, model_id: &str) -> bool {
        self.index_of(model_id).is_some()
    }

    /// Creates a new chain with models after (and including) the given model.
    ///
    /// This is useful for continuing fallback after a model succeeds but you
    /// want to track the remaining options.
    ///
    /// # Arguments
    ///
    /// * `model_id` - The model ID to start from (inclusive)
    ///
    /// # Returns
    ///
    /// * `Some(FallbackChain)` - The remaining models from the starting point
    /// * `None` - If the model is not in the chain
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b", "c"])?;
    /// let remaining = chain.from_inclusive("b")?;
    /// assert_eq!(remaining.names(), &["b", "c"]);
    /// ```
    pub fn from_inclusive(&self, model_id: &str) -> Option<Self> {
        let start_index = self.index_of(model_id)?;

        let models: Vec<_> = self.models[start_index..].to_vec();
        let names: Vec<_> = self.names[start_index..].to_vec();

        Some(Self { models, names })
    }

    /// Creates a new chain with models after (excluding) the given model.
    ///
    /// # Arguments
    ///
    /// * `model_id` - The model ID to skip
    ///
    /// # Returns
    ///
    /// * `Some(FallbackChain)` - The remaining models after the given model
    /// * `None` - If the model is not in the chain or is the last model
    ///
    /// # Example
    ///
    /// ```ignore
    /// let chain = FallbackChain::from_ids(&["a", "b", "c"])?;
    /// let remaining = chain.from_after("b")?;
    /// assert_eq!(remaining.names(), &["c"]);
    /// ```
    pub fn from_after(&self, model_id: &str) -> Option<Self> {
        let start_index = self.index_of(model_id)?;
        let next_index = start_index + 1;

        if next_index >= self.models.len() {
            return None;
        }

        let models: Vec<_> = self.models[next_index..].to_vec();
        let names: Vec<_> = self.names[next_index..].to_vec();

        Some(Self { models, names })
    }
}

/// Errors that can occur when creating a fallback chain.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FallbackChainError {
    /// The model ID format is invalid (expected "provider/model").
    #[error("Invalid model ID format '{id}': {reason}")]
    InvalidFormat {
        /// The malformed model ID.
        id: String,
        /// Explanation of why the format is invalid.
        reason: String,
    },

    /// The model was not found in the model database.
    #[error("Model not found: {provider}/{model_id}")]
    ModelNotFound {
        /// The full model ID that was requested.
        id: String,
        /// The provider that was searched.
        provider: String,
        /// The model ID that was not found.
        model_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_db::get_model_entry;

    #[test]
    fn test_from_ids_valid() {
        let chain = FallbackChain::from_ids(&["anthropic/claude-sonnet-4-20250514"]).unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain.first().unwrap().id, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_from_ids_multiple() {
        let chain = FallbackChain::from_ids(&[
            "openai/gpt-4o",
            "anthropic/claude-sonnet-4-20250514",
            "google/gemini-2.0-flash",
        ])
        .unwrap();

        assert_eq!(chain.len(), 3);
        assert_eq!(chain.first().unwrap().id, "gpt-4o");
        assert_eq!(chain.last().unwrap().id, "gemini-2.0-flash");
    }

    #[test]
    fn test_from_ids_invalid_format() {
        let result = FallbackChain::from_ids(&["invalid-no-slash"]);
        assert!(matches!(result, Err(FallbackChainError::InvalidFormat { .. })));
    }

    #[test]
    fn test_from_ids_not_found() {
        let result = FallbackChain::from_ids(&["nonexistent-provider/nonexistent-model"]);
        assert!(matches!(result, Err(FallbackChainError::ModelNotFound { .. })));
    }

    #[test]
    fn test_new_direct() {
        let model = get_model_entry("openai", "gpt-4o").unwrap();
        let chain = FallbackChain::new(vec![model]);

        assert_eq!(chain.len(), 1);
        assert_eq!(chain.first().unwrap().id, "gpt-4o");
    }

    #[test]
    fn test_default_chain() {
        let chain = FallbackChain::default();

        // Default chain should have several models
        assert!(!chain.is_empty());
        assert!(chain.len() >= 3);

        // First model should be the highest priority
        let first = chain.first();
        assert!(first.is_some());
    }

    #[test]
    fn test_next() {
        let chain = FallbackChain::from_ids(&["a", "b", "c"]).unwrap();

        assert_eq!(chain.next("a").unwrap().id, "b");
        assert_eq!(chain.next("b").unwrap().id, "c");
        assert_eq!(chain.next("c"), None);
        assert_eq!(chain.next("unknown"), None);
    }

    #[test]
    fn test_index_of() {
        let chain = FallbackChain::from_ids(&["a", "b", "c"]).unwrap();

        assert_eq!(chain.index_of("a"), Some(0));
        assert_eq!(chain.index_of("b"), Some(1));
        assert_eq!(chain.index_of("c"), Some(2));
        assert_eq!(chain.index_of("unknown"), None);
    }

    #[test]
    fn test_contains() {
        let chain = FallbackChain::from_ids(&["a", "b"]).unwrap();

        assert!(chain.contains("a"));
        assert!(chain.contains("b"));
        assert!(!chain.contains("c"));
    }

    #[test]
    fn test_iter() {
        let chain = FallbackChain::from_ids(&["a", "b", "c"]).unwrap();
        let ids: Vec<_> = chain.iter().map(|m| m.id).collect();

        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_is_empty() {
        let empty: FallbackChain = FallbackChain::new(vec![]);
        assert!(empty.is_empty());

        let non_empty = FallbackChain::from_ids(&["a"]).unwrap();
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn test_models_and_names() {
        let chain = FallbackChain::from_ids(&["openai/gpt-4o"]).unwrap();

        assert_eq!(chain.models().len(), 1);
        assert_eq!(chain.names(), &["openai/gpt-4o"]);
    }

    #[test]
    fn test_from_inclusive() {
        let chain = FallbackChain::from_ids(&["a", "b", "c"]).unwrap();

        let remaining = chain.from_inclusive("b").unwrap();
        assert_eq!(remaining.names(), &["b", "c"]);

        assert!(chain.from_inclusive("unknown").is_none());
    }

    #[test]
    fn test_from_after() {
        let chain = FallbackChain::from_ids(&["a", "b", "c"]).unwrap();

        let remaining = chain.from_after("b").unwrap();
        assert_eq!(remaining.names(), &["c"]);

        assert!(chain.from_after("c").is_none()); // No model after last
        assert!(chain.from_after("unknown").is_none());
    }

    #[test]
    fn test_first_last() {
        let chain = FallbackChain::from_ids(&["a", "b", "c"]).unwrap();

        assert_eq!(chain.first().unwrap().id, "a");
        assert_eq!(chain.last().unwrap().id, "c");

        let empty: FallbackChain = FallbackChain::new(vec![]);
        assert_eq!(empty.first(), None);
        assert_eq!(empty.last(), None);
    }

    #[test]
    fn test_debug_format() {
        let chain = FallbackChain::from_ids(&["a"]).unwrap();
        let debug_str = format!("{:?}", chain);
        assert!(debug_str.contains("FallbackChain"));
    }
}