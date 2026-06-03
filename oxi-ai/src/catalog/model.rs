//! Model metadata structures — TOML ↔ Rust.

use serde::{Deserialize, Serialize};

/// A single built-in model entry, deserialized from `data/catalog/models/*.toml`.
///
/// Matches the shape of `model_db::ModelEntry` but uses owned `String` types
/// because it comes from a runtime-parsed TOML rather than static literals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinModelEntry {
    /// Model identifier (e.g., "claude-sonnet-4-20250514")
    pub id: String,
    /// Human-readable model name (e.g., "Claude Sonnet 4")
    pub name: String,
    /// API protocol to use
    pub api: String,
    /// Provider name (e.g., "anthropic", "openai")
    pub provider: String,
    /// Whether this model supports reasoning/thinking
    #[serde(default)]
    pub reasoning: bool,
    /// Supported input modalities
    #[serde(default)]
    pub input: Vec<String>,
    /// Cost per million input tokens (USD)
    #[serde(default)]
    pub cost_input: f64,
    /// Cost per million output tokens (USD)
    #[serde(default)]
    pub cost_output: f64,
    /// Cost per million cached read tokens (USD)
    #[serde(default)]
    pub cost_cache_read: f64,
    /// Cost per million cached write tokens (USD)
    #[serde(default)]
    pub cost_cache_write: f64,
    /// Maximum context window in tokens
    #[serde(default)]
    pub context_window: u32,
    /// Maximum output tokens
    #[serde(default)]
    pub max_tokens: u32,
}

impl BuiltinModelEntry {
    /// Check if this model supports image/vision input.
    pub fn supports_vision(&self) -> bool {
        self.input.iter().any(|m| m == "image" || m == "Image")
    }

    /// Check if this model supports reasoning/thinking.
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning
    }

    /// Calculate the cost for a given token usage.
    pub fn calculate_cost(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> f64 {
        let in_cost = (input_tokens as f64 / 1_000_000.0) * self.cost_input;
        let out_cost = (output_tokens as f64 / 1_000_000.0) * self.cost_output;
        let cr_cost = (cache_read as f64 / 1_000_000.0) * self.cost_cache_read;
        let cw_cost = (cache_write as f64 / 1_000_000.0) * self.cost_cache_write;
        in_cost + out_cost + cr_cost + cw_cost
    }
}

/// Load all built-in models from the bundled TOML files.
pub fn load_builtin_models() -> &'static std::collections::BTreeMap<String, Vec<BuiltinModelEntry>> {
    &crate::catalog::CatalogRoot::get().models
}

/// Number of built-in models (across all providers).
pub fn builtin_model_count() -> usize {
    load_builtin_models().values().map(|v| v.len()).sum()
}

/// Index of all built-in model TOML files.
///
/// This is a placeholder for now — model files will be added in
/// data/catalog/models/<provider>.toml. Each file declares
/// `provider = "..."` and a list of `[[model]]` entries.
///
/// The macro returns a list of `(provider_id, toml_str)` tuples. At build
/// time, `build.rs` should generate this from the directory contents.
///
/// For now, the index is empty — providers and their TOML data are loaded
/// in a follow-up PR.
pub fn models_index() -> &'static [(&'static str, &'static str)] {
    &[]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_models_index_is_valid() {
        // Sanity check until model files are added
        assert!(models_index().is_empty());
    }
}
