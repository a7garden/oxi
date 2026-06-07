//! Comprehensive model database for oxi-ai
//!
//! Contains 934 models across 29 providers.
//!
//! # Usage
//!
//! ```ignore
//! use oxi_ai::model_db::{get_model_entry, get_provider_models, get_all_models};
//!
//! // Look up a specific model
//! let entry = get_model_entry("anthropic", "claude-sonnet-4-20250514");
//! assert!(entry.is_some());
//!
//! // Get all models for a provider
//! let anthropic_models = get_provider_models("anthropic");
//! assert!(!anthropic_models.is_empty());
//!
//! // Iterate all models
//! let all = get_all_models();
//! assert!(all.len() > 926);
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::catalog::BuiltinModelEntry;
use crate::{Api, InputModality};

// ---------------------------------------------------------------------------
// TOML → ModelEntry bridge
// ---------------------------------------------------------------------------
//
// The canonical model data lives in `data/catalog/models/*.toml` (Layer 1 of
// the 3-tier catalog). The legacy `ModelEntry` struct here is `&'static str`
// based, so we need to convert each `BuiltinModelEntry` (String-based, from
// the TOML loader) to a `ModelEntry` once and cache the result in a
// `OnceLock`. String-to-`&'static str` is achieved via `Box::leak`, same
// pattern used by `register_builtins.rs`.

fn parse_api(s: &str) -> Api {
    match s {
        "anthropic-messages" => Api::AnthropicMessages,
        "openai-completions" => Api::OpenAiCompletions,
        "openai-responses" => Api::OpenAiResponses,
        "google-generative-ai" => Api::GoogleGenerativeAi,
        "google-vertex" => Api::GoogleVertex,
        "mistral-conversations" => Api::MistralConversations,
        "azure-openai-responses" => Api::AzureOpenAiResponses,
        "bedrock-converse-stream" => Api::BedrockConverseStream,
        _ => Api::OpenAiCompletions,
    }
}

fn parse_input_modality(s: &str) -> InputModality {
    match s {
        "text" | "Text" => InputModality::Text,
        "image" | "Image" => InputModality::Image,
        _ => InputModality::Text,
    }
}

impl From<&BuiltinModelEntry> for ModelEntry {
    fn from(e: &BuiltinModelEntry) -> Self {
        // Leak the strings to obtain `&'static str`. Bounded by total model
        // count (currently 1099) and amortized once at startup.
        let id: &'static str = Box::leak(e.id.clone().into_boxed_str());
        let name: &'static str = Box::leak(e.name.clone().into_boxed_str());
        let provider: &'static str = Box::leak(e.provider.clone().into_boxed_str());
        let input: &'static [InputModality] = Box::leak(
            e.input
                .iter()
                .map(|s| parse_input_modality(s))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        // Sentinel transform: openclaw-sourced models with cost = 0 are
        // NOT verified as free — they're unverified. We translate upstream
        // `0.0` to the sentinel `-1.0` only for openclaw-imported providers.
        // Oxi-original verified data is left at 0.0 (truly free).
        let (ci, co) = if is_openclaw_sourced(&e.provider) {
            (
                if e.cost_input == 0.0 {
                    UNVERIFIED_PRICE
                } else {
                    e.cost_input
                },
                if e.cost_output == 0.0 {
                    UNVERIFIED_PRICE
                } else {
                    e.cost_output
                },
            )
        } else {
            (e.cost_input, e.cost_output)
        };
        ModelEntry {
            id,
            name,
            api: parse_api(&e.api),
            provider,
            reasoning: e.reasoning,
            input,
            cost_input: ci,
            cost_output: co,
            cost_cache_read: e.cost_cache_read,
            cost_cache_write: e.cost_cache_write,
            context_window: e.context_window,
            max_tokens: e.max_tokens,
        }
    }
}

/// Sentinel value used when a model entry has no verified price.
///
/// Negative values are reserved for sentinel meanings. The two real
/// interpretations of `cost_input` are now:
///
/// - `cost_input < 0.0` — price is unverified, see [`ModelEntry::pricing_unverified`].
/// - `cost_input == 0.0` — verified as zero (truly free local model, etc.).
/// - `cost_input > 0.0` — verified price in USD per million tokens.
pub const UNVERIFIED_PRICE: f64 = -1.0;

/// Returns true if a provider id came from the openclaw port AND has
/// unverified pricing.
///
/// These are the providers whose `0.0` cost values in the openclaw
/// upstream are placeholder, not verified-free. The runtime sentinel
/// transformation (`0.0` → `-1.0`) applies to them.
///
/// Providers with **verified** prices (venice, novita) are NOT in this
/// set — their values are backfilled and treated as known.
///
/// See `data/catalog/README.md` for the data-quality breakdown.
fn is_openclaw_sourced(provider: &str) -> bool {
    matches!(
        provider,
        "gmi"
            | "kilocode"
            | "moonshot"
            | "nvidia"
            | "ollama-cloud"
            | "qianfan"
            | "qwen-oauth"
            | "stepfun"
            | "byteplus"
            | "chutes"
            | "deepinfra"
            // Variants that share a TOML file with a verified provider.
            // stepfun.toml has both "stepfun" and "stepfun-plan" models;
            // the latter is a paid tier whose price is also unknown.
            | "stepfun-plan"
            | "byteplus-plan"
    )
}

/// A static model entry in the database.
///
/// Uses `&'static str` references for zero-allocation lookups.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelEntry {
    /// Model identifier (e.g., "claude-sonnet-4-20250514")
    pub id: &'static str,
    /// Human-readable model name (e.g., "Claude Sonnet 4")
    pub name: &'static str,
    /// API protocol to use
    pub api: Api,
    /// Provider name (e.g., "anthropic", "openai")
    pub provider: &'static str,
    /// Whether this model supports reasoning/thinking
    pub reasoning: bool,
    /// Supported input modalities
    pub input: &'static [InputModality],
    /// Cost per million input tokens (USD)
    pub cost_input: f64,
    /// Cost per million output tokens (USD)
    pub cost_output: f64,
    /// Cost per million cached read tokens (USD)
    pub cost_cache_read: f64,
    /// Cost per million cached write tokens (USD)
    pub cost_cache_write: f64,
    /// Maximum context window in tokens
    pub context_window: u32,
    /// Maximum output tokens
    pub max_tokens: u32,
}

impl ModelEntry {
    /// Check if this model supports image/vision input
    pub fn supports_vision(&self) -> bool {
        self.input.contains(&InputModality::Image)
    }

    /// Check if this model supports reasoning/thinking
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning
    }

    /// Calculate the cost for a given token usage.
    ///
    /// Returns 0.0 for any field that is the unverified sentinel
    /// (`UNVERIFIED_PRICE`, i.e. negative). Callers that care about
    /// unverified prices should check [`ModelEntry::pricing_unverified`]
    /// first and warn the user.
    pub fn calculate_cost(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> f64 {
        let in_cost = (input_tokens as f64 / 1_000_000.0) * self.cost_input.max(0.0);
        let out_cost = (output_tokens as f64 / 1_000_000.0) * self.cost_output.max(0.0);
        let cr_cost = (cache_read as f64 / 1_000_000.0) * self.cost_cache_read.max(0.0);
        let cw_cost = (cache_write as f64 / 1_000_000.0) * self.cost_cache_write.max(0.0);
        in_cost + out_cost + cr_cost + cw_cost
    }

    /// Sentinel value indicating "price unknown" / "not verified".
    ///
    /// Distinguishes upstream-supplied zero (e.g. a free local model) from
    /// "we don't have the price yet, use with caution". The convention is:
    ///
    /// - `cost_input = -1.0` (or any negative) means: price is unverified.
    ///   UIs should warn the user. Cost calculations may return 0 or refuse.
    /// - `cost_input = 0.0` means: price is verified as zero (truly free).
    /// - `cost_input > 0.0` means: verified price per million tokens.
    ///
    /// The `BuiltinModelEntry → ModelEntry` converter applies this
    /// transformation: upstream `0.0` for a known paid provider becomes `-1.0`
    /// here. The `pricing_verified` method lets callers check.
    pub fn pricing_verified(&self) -> bool {
        self.cost_input >= 0.0 && self.cost_output >= 0.0
    }

    /// Returns true if either cost field is the unverified sentinel.
    pub fn pricing_unverified(&self) -> bool {
        self.cost_input < 0.0 || self.cost_output < 0.0
    }
}

/// Lazy, catalog-backed `(provider, models)` table.
///
/// Replaces the historical `static ALL_PROVIDER_MODELS` array. On first access,
/// this iterates the `BuiltinModelEntry` map from `crate::catalog`, converts
/// each entry via `From<&BuiltinModelEntry> for ModelEntry`, and stores the
/// result. Subsequent accesses return the cached `&'static` slice.
///
/// The string-to-`&'static str` conversions happen inside the `From` impl.
static ALL_PROVIDER_MODELS: OnceLock<Vec<(&'static str, &'static [ModelEntry])>> = OnceLock::new();

fn all_provider_models() -> &'static [(&'static str, &'static [ModelEntry])] {
    ALL_PROVIDER_MODELS
        .get_or_init(|| {
            let catalog = crate::catalog::CatalogRoot::get();
            // Group by the per-model `provider` field, not the file-level
            // top-level provider. The openclaw-anthropic file contains models
            // for both `anthropic` and `claude-cli` provider namespaces; they
            // must be indexed separately. We first flatten all entries, then
            // regroup by the per-entry provider field.
            use std::collections::BTreeMap;
            // Snapshot into owned BuiltinModelEntry vec so we can mutate.
            let mut all_builtins: Vec<crate::catalog::BuiltinModelEntry> = Vec::new();
            for (_file_pid, builtin_models) in catalog.models.iter() {
                for bm in builtin_models.iter() {
                    all_builtins.push(bm.clone());
                }
            }
            // Apply Layer 2 (user overrides): same (provider, id) replaces,
            // new entries append.
            if let Some(overrides) = crate::catalog::load_overrides() {
                // Build mutable BTreeMap so apply_model_overrides can mutate.
                let mut all_map: BTreeMap<String, Vec<crate::catalog::BuiltinModelEntry>> =
                    BTreeMap::new();
                for bm in all_builtins.into_iter() {
                    all_map.entry(bm.provider.clone()).or_default().push(bm);
                }
                crate::catalog::apply_model_overrides(&mut all_map, &overrides.model);
                all_builtins = all_map.into_values().flatten().collect();
            }
            let mut by_pid: BTreeMap<String, Vec<ModelEntry>> = BTreeMap::new();
            for bm in all_builtins.iter() {
                let entry = ModelEntry::from(bm);
                by_pid
                    .entry(entry.provider.to_string())
                    .or_default()
                    .push(entry);
            }
            let mut out: Vec<(&'static str, &'static [ModelEntry])> =
                Vec::with_capacity(by_pid.len());
            for (pid, mut entries) in by_pid {
                let pid_static: &'static str = Box::leak(pid.into_boxed_str());
                entries.sort_by(|a, b| a.id.cmp(b.id));
                let slice: &'static [ModelEntry] = Box::leak(entries.into_boxed_slice());
                out.push((pid_static, slice));
            }
            out
        })
        .as_slice()
}
// ── Lazy-initialized indexes for O(1) lookups ──────────────────────────

/// Maps `"provider/id"` → `&'static ModelEntry` for O(1) model lookups.
static MODEL_INDEX: OnceLock<HashMap<&'static str, &'static ModelEntry>> = OnceLock::new();

fn model_index() -> &'static HashMap<&'static str, &'static ModelEntry> {
    MODEL_INDEX.get_or_init(|| {
        let mut map = HashMap::with_capacity(model_count());
        for (provider, models) in all_provider_models().iter() {
            for model in models.iter() {
                let key = format!("{}/{}", provider, model.id);
                // Leak the formatted key to obtain `&'static str`.
                // This happens once at first access; the total leaked memory
                // is bounded by the model database size (~60 KiB).
                let key_static: &'static str = Box::leak(key.into_boxed_str());
                map.insert(key_static, model);
            }
        }
        map
    })
}

/// Maps provider name → its model slice for O(1) provider lookups.
static PROVIDER_INDEX: OnceLock<HashMap<&'static str, &'static [ModelEntry]>> = OnceLock::new();

fn provider_index() -> &'static HashMap<&'static str, &'static [ModelEntry]> {
    PROVIDER_INDEX.get_or_init(|| {
        let mut map = HashMap::with_capacity(all_provider_models().len());
        for (provider, models) in all_provider_models().iter() {
            map.insert(*provider, *models);
        }
        map
    })
}

// ── Public API ───────────────────────────────────────────────────────────

/// Look up a specific model entry by provider and model ID.
///
/// Uses an O(1) index internally. Falls back gracefully if not found.
///
/// # Arguments
/// * `provider` - The provider name (e.g., "anthropic", "openai")
/// * `id` - The model ID (e.g., "claude-sonnet-4-20250514")
///
/// # Returns
/// `Some(&ModelEntry)` if found, `None` otherwise.
///
/// # Example
/// ```ignore
/// use oxi_ai::model_db::get_model_entry;
/// let m = get_model_entry("openai", "gpt-4o").unwrap();
/// assert_eq!(m.name, "GPT-4o");
/// ```
pub fn get_model_entry(provider: &str, id: &str) -> Option<&'static ModelEntry> {
    let key = format!("{}/{}", provider, id);
    model_index().get(key.as_str()).copied()
}

/// Get all model entries for a given provider.
///
/// Uses an O(1) index internally.
///
/// # Arguments
/// * `provider` - The provider name (e.g., "anthropic", "openai")
///
/// # Returns
/// A slice of `ModelEntry` for the provider, or an empty slice if not found.
pub fn get_provider_models(provider: &str) -> &'static [ModelEntry] {
    provider_index().get(provider).copied().unwrap_or(&[])
}

/// Get all model entries across all providers.
///
/// Returns a flat iterator over every `ModelEntry` in the database.
pub fn get_all_models() -> impl Iterator<Item = &'static ModelEntry> {
    all_provider_models()
        .iter()
        .flat_map(|(_, models)| models.iter())
}

/// Get the total number of models in the database.
pub fn model_count() -> usize {
    all_provider_models().iter().map(|(_, m)| m.len()).sum()
}

/// Count of models with the unverified-pricing sentinel
/// (`cost_input < 0.0 || cost_output < 0.0`).
///
/// These are openclaw-sourced entries where the upstream shipped `0.0`
/// prices that we could not verify. The UI should display a warning.
pub fn builtin_model_count_sentinel() -> usize {
    get_all_models().filter(|m| m.pricing_unverified()).count()
}

/// Get all known provider names.
pub fn get_providers() -> Vec<&'static str> {
    all_provider_models()
        .iter()
        .map(|(name, _)| *name)
        .collect()
}

/// Search models by name or ID pattern (case-insensitive).
pub fn search_models(pattern: &str) -> Vec<&'static ModelEntry> {
    let lower = pattern.to_lowercase();
    get_all_models()
        .filter(|m| m.id.to_lowercase().contains(&lower) || m.name.to_lowercase().contains(&lower))
        .collect()
}

/// Find models that support reasoning/thinking.
pub fn get_reasoning_models() -> Vec<&'static ModelEntry> {
    get_all_models().filter(|m| m.reasoning).collect()
}

/// Find models that support image/vision input.
pub fn get_vision_models() -> Vec<&'static ModelEntry> {
    get_all_models().filter(|m| m.supports_vision()).collect()
}

/// Find the cheapest models by input cost, returning up to `limit` results.
pub fn get_cheapest_models(limit: usize) -> Vec<&'static ModelEntry> {
    let mut all: Vec<_> = get_all_models().collect();
    all.sort_by(|a, b| {
        a.cost_input
            .partial_cmp(&b.cost_input)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all.truncate(limit);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_model_count() {
        let count = model_count();
        assert!(count >= 934, "Expected at least 934 models, got {}", count);
    }

    #[test]
    fn test_get_anthropic_model() {
        let m = get_model_entry("anthropic", "claude-3-5-sonnet-20240620");
        assert!(m.is_some(), "Claude Sonnet 3.5 should exist");
        let m = m.unwrap();
        assert_eq!(m.provider, "anthropic");
        assert!(m.context_window >= 200_000);
    }

    #[test]
    fn test_get_openai_model() {
        let m = get_model_entry("openai", "gpt-4o");
        assert!(m.is_some(), "GPT-4o should exist");
        let m = m.unwrap();
        assert_eq!(m.provider, "openai");
    }

    #[test]
    fn test_provider_models() {
        let anthropic = get_provider_models("anthropic");
        assert!(!anthropic.is_empty(), "Anthropic should have models");
        assert!(anthropic.iter().all(|m| m.provider == "anthropic"));

        let unknown = get_provider_models("nonexistent-provider");
        assert!(unknown.is_empty());
    }

    #[test]
    fn test_search_models() {
        let results = search_models("claude");
        assert!(!results.is_empty(), "Should find Claude models");
        assert!(
            results
                .iter()
                .all(|m| m.name.to_lowercase().contains("claude")
                    || m.id.to_lowercase().contains("claude"))
        );
    }

    #[test]
    fn test_all_providers() {
        let providers = get_providers();
        assert!(providers.contains(&"openai"), "Should have openai");
        assert!(providers.contains(&"anthropic"), "Should have anthropic");
    }

    #[test]
    fn test_reasoning_models() {
        let reasoning = get_reasoning_models();
        assert!(!reasoning.is_empty(), "Should have reasoning models");
        assert!(reasoning.iter().all(|m| m.reasoning));
    }

    #[test]
    fn test_vision_models() {
        let vision = get_vision_models();
        assert!(!vision.is_empty(), "Should have vision models");
        assert!(vision.iter().all(|m| m.supports_vision()));
    }

    #[test]

    fn test_cheapest_models() {
        let cheapest = get_cheapest_models(5);
        assert_eq!(cheapest.len(), 5.min(model_count()));
        for i in 1..cheapest.len() {
            assert!(cheapest[i].cost_input >= cheapest[i - 1].cost_input);
        }
    }
}
