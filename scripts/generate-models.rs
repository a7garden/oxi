//! Model DB Generator
//!
//! Reads `scripts/models.json` and prints `model_db.rs` to stdout.
//!
//! Usage:
//!   cargo run --manifest-path scripts/Cargo.toml --bin generate-models > oxi-ai/src/model_db.rs

use serde::Deserialize;
use std::io::{self, Read};

// ── JSON schema types ──────────────────────────────────────────────

#[derive(Deserialize)]
struct ModelDb {
    providers: Vec<Provider>,
}

#[derive(Deserialize)]
struct Provider {
    name: String,
    api: String,
    models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    name: String,
    /// Per-model API override. If absent, use the provider's default API.
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    reasoning: bool,
    input: Vec<String>,
    cost_input: f64,
    cost_output: f64,
    #[serde(default)]
    cost_cache_read: f64,
    #[serde(default)]
    cost_cache_write: f64,
    context_window: u32,
    max_tokens: u32,
}

// ── API string → Rust enum variant ────────────────────────────────

fn api_variant(api: &str) -> &'static str {
    match api {
        "openai-completions" => "Api::OpenAiCompletions",
        "openai-responses" => "Api::OpenAiResponses",
        "anthropic-messages" => "Api::AnthropicMessages",
        "google-generative-ai" => "Api::GoogleGenerativeAi",
        "google-vertex" => "Api::GoogleVertex",
        "mistral-conversations" => "Api::MistralConversations",
        "azure-openai-responses" => "Api::AzureOpenAiResponses",
        "bedrock-converse-stream" => "Api::BedrockConverseStream",
        _ => panic!("Unknown API variant: {}", api),
    }
}

// ── Input modality string → Rust enum ─────────────────────────────

fn input_modality(m: &str) -> &'static str {
    match m {
        "text" => "InputModality::Text",
        "image" => "InputModality::Image",
        _ => panic!("Unknown input modality: {}", m),
    }
}

// ── Static array name from provider name ──────────────────────────

fn static_name(provider: &str) -> String {
    let upper = provider.to_uppercase().replace('-', "_");
    format!("{}_MODELS", upper)
}

// ── Code generation ───────────────────────────────────────────────

fn main() {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf).expect("read stdin");
    let db: ModelDb = serde_json::from_str(&buf)
        .unwrap_or_else(|e| {
            // Try reading from file if stdin was empty or just whitespace
            if buf.trim().is_empty() {
                let json_str = std::fs::read_to_string("scripts/models.json")
                    .expect("Could not read scripts/models.json");
                serde_json::from_str(&json_str).expect("Parse models.json")
            } else {
                panic!("Failed to parse JSON from stdin: {}", e);
            }
        });

    let total_models: usize = db.providers.iter().map(|p| p.models.len()).sum();
    let total_providers = db.providers.len();

    // Count providers that have more than one api across their models
    // (for accurate doc comment if someone uses per-model api overrides)
    let mut out = String::new();

    // ── Module doc comment ──
    out.push_str(&format!(
        "//! Comprehensive model database for oxi-ai\n\
         //!\n\
         //! Contains {} models across {} providers.\n\
         //!\n\
         //! # Usage\n\
         //!\n\
         //! ```ignore\n\
         //! use oxi_ai::model_db::{{get_model_entry, get_provider_models, get_all_models}};\n\
         //!\n\
         //! // Look up a specific model\n\
         //! let entry = get_model_entry(\"anthropic\", \"claude-sonnet-4-20250514\");\n\
         //! assert!(entry.is_some());\n\
         //!\n\
         //! // Get all models for a provider\n\
         //! let anthropic_models = get_provider_models(\"anthropic\");\n\
         //! assert!(!anthropic_models.is_empty());\n\
         //!\n\
         //! // Iterate all models\n\
         //! let all = get_all_models();\n\
         //! assert!(all.len() > {});\n\
         //! ```\n\n",
        total_models, total_providers, total_models.saturating_sub(10),
    ));

    // ── Imports ──
    out.push_str("use crate::{Api, InputModality};\n\n");

    // ── ModelEntry struct ──
    out.push_str(
        "/// A static model entry in the database.\n\
         ///\n\
         /// Uses `&'static str` references for zero-allocation lookups.\n\
         #[derive(Debug, Clone, Copy, PartialEq)]\n\
         pub struct ModelEntry {\n\
         \x20   /// Model identifier (e.g., \"claude-sonnet-4-20250514\")\n\
         \x20   pub id: &'static str,\n\
         \x20   /// Human-readable model name (e.g., \"Claude Sonnet 4\")\n\
         \x20   pub name: &'static str,\n\
         \x20   /// API protocol to use\n\
         \x20   pub api: Api,\n\
         \x20   /// Provider name (e.g., \"anthropic\", \"openai\")\n\
         \x20   pub provider: &'static str,\n\
         \x20   /// Whether this model supports reasoning/thinking\n\
         \x20   pub reasoning: bool,\n\
         \x20   /// Supported input modalities\n\
         \x20   pub input: &'static [InputModality],\n\
         \x20   /// Cost per million input tokens (USD)\n\
         \x20   pub cost_input: f64,\n\
         \x20   /// Cost per million output tokens (USD)\n\
         \x20   pub cost_output: f64,\n\
         \x20   /// Cost per million cached read tokens (USD)\n\
         \x20   pub cost_cache_read: f64,\n\
         \x20   /// Cost per million cached write tokens (USD)\n\
         \x20   pub cost_cache_write: f64,\n\
         \x20   /// Maximum context window in tokens\n\
         \x20   pub context_window: u32,\n\
         \x20   /// Maximum output tokens\n\
         \x20   pub max_tokens: u32,\n\
         }\n\n",
    );

    // ── ModelEntry impl block ──
    out.push_str(
        "impl ModelEntry {\n\
         \x20   /// Check if this model supports image/vision input\n\
         \x20   pub fn supports_vision(&self) -> bool {\n\
         \x20\x20\x20\x20\x20\x20\x20self.input.contains(&InputModality::Image)\n\
         \x20   }\n\n\
         \x20   /// Check if this model supports reasoning/thinking\n\
         \x20   pub fn supports_reasoning(&self) -> bool {\n\
         \x20\x20\x20\x20\x20\x20\x20self.reasoning\n\
         \x20   }\n\n\
         \x20   /// Calculate the cost for a given token usage\n\
         \x20   pub fn calculate_cost(\n\
         \x20\x20\x20\x20\x20\x20\x20&self,\n\
         \x20\x20\x20\x20\x20\x20\x20input_tokens: u64,\n\
         \x20\x20\x20\x20\x20\x20\x20output_tokens: u64,\n\
         \x20\x20\x20\x20\x20\x20\x20cache_read: u64,\n\
         \x20\x20\x20\x20\x20\x20\x20cache_write: u64,\n\
         \x20   ) -> f64 {\n\
         \x20\x20\x20\x20\x20\x20\x20let in_cost = (input_tokens as f64 / 1_000_000.0) * self.cost_input;\n\
         \x20\x20\x20\x20\x20\x20\x20let out_cost = (output_tokens as f64 / 1_000_000.0) * self.cost_output;\n\
         \x20\x20\x20\x20\x20\x20\x20let cr_cost = (cache_read as f64 / 1_000_000.0) * self.cost_cache_read;\n\
         \x20\x20\x20\x20\x20\x20\x20let cw_cost = (cache_write as f64 / 1_000_000.0) * self.cost_cache_write;\n\
         \x20\x20\x20\x20\x20\x20\x20in_cost + out_cost + cr_cost + cw_cost\n\
         \x20   }\n\
         }\n\n",
    );

    // ── Per-provider static arrays ──
    for provider in &db.providers {
        let array_name = static_name(&provider.name);
        let count = provider.models.len();
        out.push_str(&format!(
            "/// {} models ({} entries)\nstatic {}: &[ModelEntry] = &[\n",
            provider.name, count, array_name,
        ));

        for model in &provider.models {
            // Build the input modality slice
            let input_slice = if model.input.len() == 1 {
                format!("&[{}]", input_modality(&model.input[0]))
            } else {
                let items: Vec<_> = model.input.iter().map(|m| input_modality(m)).collect();
                format!("&[{}]", items.join(", "))
            };

            // Format cost values — use 0.0 for zero, otherwise full precision
            let fmt_cost = |v: f64| -> String {
                if v == 0.0 {
                    "0.0".to_string()
                } else if v == (v as u64) as f64 {
                    format!("{}.0", v as u64)
                } else {
                    format!("{}", v)
                }
            };

            out.push_str(&format!(
                "    ModelEntry {{\n\
                 \x20\x20\x20\x20\x20\x20\x20id: \"{}\",\n\
                 \x20\x20\x20\x20\x20\x20\x20name: \"{}\",\n\
                 \x20\x20\x20\x20\x20\x20\x20api: {},\n\
                 \x20\x20\x20\x20\x20\x20\x20provider: \"{}\",\n\
                 \x20\x20\x20\x20\x20\x20\x20reasoning: {},\n\
                 \x20\x20\x20\x20\x20\x20\x20input: {},\n\
                 \x20\x20\x20\x20\x20\x20\x20cost_input: {},\n\
                 \x20\x20\x20\x20\x20\x20\x20cost_output: {},\n\
                 \x20\x20\x20\x20\x20\x20\x20cost_cache_read: {},\n\
                 \x20\x20\x20\x20\x20\x20\x20cost_cache_write: {},\n\
                 \x20\x20\x20\x20\x20\x20\x20context_window: {},\n\
                 \x20\x20\x20\x20\x20\x20\x20max_tokens: {},\n\
                 \x20\x20\x20\x20}},\n",
                model.id,
                model.name,
                api_variant(model.api.as_deref().unwrap_or(&provider.api)),
                provider.name,
                model.reasoning,
                input_slice,
                fmt_cost(model.cost_input),
                fmt_cost(model.cost_output),
                fmt_cost(model.cost_cache_read),
                fmt_cost(model.cost_cache_write),
                model.context_window,
                model.max_tokens,
            ));
        }

        out.push_str("];\n\n");
    }

    // ── ALL_PROVIDER_MODELS index ──
    out.push_str("/// All model arrays indexed by provider\n");
    out.push_str("static ALL_PROVIDER_MODELS: &[(&str, &[ModelEntry])] = &[\n");
    for provider in &db.providers {
        let array_name = static_name(&provider.name);
        out.push_str(&format!(
            "    (\"{}\", {}),\n",
            provider.name, array_name
        ));
    }
    out.push_str("];\n\n");

    // ── Helper functions ──
    out.push_str(
        "/// Look up a specific model entry by provider and model ID.\n\
         ///\n\
         /// # Arguments\n\
         /// * `provider` - The provider name (e.g., \"anthropic\", \"openai\")\n\
         /// * `id` - The model ID (e.g., \"claude-sonnet-4-20250514\")\n\
         ///\n\
         /// # Returns\n\
         /// `Some(&ModelEntry)` if found, `None` otherwise.\n\
         ///\n\
         /// # Example\n\
         /// ```ignore\n\
         /// use oxi_ai::model_db::get_model_entry;\n\
         /// let m = get_model_entry(\"openai\", \"gpt-4o\").unwrap();\n\
         /// assert_eq!(m.name, \"GPT-4o\");\n\
         /// ```\n\
         pub fn get_model_entry(provider: &str, id: &str) -> Option<&'static ModelEntry> {\n\
         \x20   for (prov, models) in ALL_PROVIDER_MODELS.iter() {\n\
         \x20\x20\x20\x20\x20\x20\x20if *prov == provider {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20return models.iter().find(|m| m.id == id);\n\
         \x20\x20\x20\x20\x20\x20\x20}\n\
         \x20   }\n\
         \x20   None\n\
         }\n\n",
    );

    out.push_str(
        "/// Get all model entries for a given provider.\n\
         ///\n\
         /// # Arguments\n\
         /// * `provider` - The provider name (e.g., \"anthropic\", \"openai\")\n\
         ///\n\
         /// # Returns\n\
         /// A slice of `ModelEntry` for the provider, or an empty slice if not found.\n\
         pub fn get_provider_models(provider: &str) -> &'static [ModelEntry] {\n\
         \x20   for (prov, models) in ALL_PROVIDER_MODELS.iter() {\n\
         \x20\x20\x20\x20\x20\x20\x20if *prov == provider {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20return models;\n\
         \x20\x20\x20\x20\x20\x20\x20}\n\
         \x20   }\n\
         \x20   &[]\n\
         }\n\n",
    );

    out.push_str(
        "/// Get all model entries across all providers.\n\
         ///\n\
         /// Returns a flat iterator over every `ModelEntry` in the database.\n\
         pub fn get_all_models() -> impl Iterator<Item = &'static ModelEntry> {\n\
         \x20   ALL_PROVIDER_MODELS\n\
         \x20\x20\x20\x20\x20\x20\x20.iter()\n\
         \x20\x20\x20\x20\x20\x20\x20.flat_map(|(_, models)| models.iter())\n\
         }\n\n",
    );

    out.push_str(
        "/// Get the total number of models in the database.\n\
         pub fn model_count() -> usize {\n\
         \x20   ALL_PROVIDER_MODELS.iter().map(|(_, m)| m.len()).sum()\n\
         }\n\n",
    );

    out.push_str(
        "/// Get all known provider names.\n\
         pub fn get_providers() -> Vec<&'static str> {\n\
         \x20   ALL_PROVIDER_MODELS.iter().map(|(name, _)| *name).collect()\n\
         }\n\n",
    );

    out.push_str(
        "/// Search models by name or ID pattern (case-insensitive).\n\
         pub fn search_models(pattern: &str) -> Vec<&'static ModelEntry> {\n\
         \x20   let lower = pattern.to_lowercase();\n\
         \x20   get_all_models()\n\
         \x20\x20\x20\x20\x20\x20\x20.filter(|m| m.id.to_lowercase().contains(&lower) || m.name.to_lowercase().contains(&lower))\n\
         \x20\x20\x20\x20\x20\x20\x20.collect()\n\
         }\n\n",
    );

    out.push_str(
        "/// Find models that support reasoning/thinking.\n\
         pub fn get_reasoning_models() -> Vec<&'static ModelEntry> {\n\
         \x20   get_all_models().filter(|m| m.reasoning).collect()\n\
         }\n\n",
    );

    out.push_str(
        "/// Find models that support image/vision input.\n\
         pub fn get_vision_models() -> Vec<&'static ModelEntry> {\n\
         \x20   get_all_models().filter(|m| m.supports_vision()).collect()\n\
         }\n\n",
    );

    out.push_str(
        "/// Find the cheapest models by input cost, returning up to `limit` results.\n\
         pub fn get_cheapest_models(limit: usize) -> Vec<&'static ModelEntry> {\n\
         \x20   let mut all: Vec<_> = get_all_models().collect();\n\
         \x20   all.sort_by(|a, b| {\n\
         \x20\x20\x20\x20\x20\x20\x20a.cost_input\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20.partial_cmp(&b.cost_input)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20.unwrap_or(std::cmp::Ordering::Equal)\n\
         \x20   });\n\
         \x20   all.truncate(limit);\n\
         \x20   all\n\
         }\n\n",
    );

    // ── Tests ──
    out.push_str(
        "#[cfg(test)]\n\
         mod tests {\n\
         \x20   use super::*;\n\n",
    );

    out.push_str(&format!(
        "    #[test]\n\
         \x20   fn test_total_model_count() {{\n\
         \x20\x20\x20\x20\x20\x20\x20let count = model_count();\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(count >= {}, \"Expected at least {} models, got {{}}\", count);\n\
         \x20   }}\n\n",
        total_models, total_models,
    ));

    // Find an anthropic model for test
    let anthropic_provider = db.providers.iter().find(|p| p.name == "anthropic");
    if let Some(ap) = anthropic_provider {
        if let Some(sample) = ap.models.iter().find(|m| m.id.contains("sonnet")) {
            out.push_str(&format!(
                "    #[test]\n\
                 \x20   fn test_get_anthropic_model() {{\n\
                 \x20\x20\x20\x20\x20\x20\x20let m = get_model_entry(\"anthropic\", \"{}\");\n\
                 \x20\x20\x20\x20\x20\x20\x20assert!(m.is_some(), \"{} should exist\");\n\
                 \x20\x20\x20\x20\x20\x20\x20let m = m.unwrap();\n\
                 \x20\x20\x20\x20\x20\x20\x20assert_eq!(m.provider, \"anthropic\");\n\
                 \x20\x20\x20\x20\x20\x20\x20assert!(m.context_window >= 200_000);\n\
                 \x20   }}\n\n",
                sample.id, sample.name,
            ));
        }
    }

    // Find an openai model for test
    let openai_provider = db.providers.iter().find(|p| p.name == "openai");
    if let Some(op) = openai_provider {
        if let Some(sample) = op.models.iter().find(|m| m.id == "gpt-4o") {
            out.push_str(&format!(
                "    #[test]\n\
                 \x20   fn test_get_openai_model() {{\n\
                 \x20\x20\x20\x20\x20\x20\x20let m = get_model_entry(\"openai\", \"{}\");\n\
                 \x20\x20\x20\x20\x20\x20\x20assert!(m.is_some(), \"{} should exist\");\n\
                 \x20\x20\x20\x20\x20\x20\x20let m = m.unwrap();\n\
                 \x20\x20\x20\x20\x20\x20\x20assert_eq!(m.provider, \"openai\");\n\
                 \x20   }}\n\n",
                sample.id, sample.name,
            ));
        }
    }

    out.push_str(
        "    #[test]\n\
         \x20   fn test_provider_models() {\n\
         \x20\x20\x20\x20\x20\x20\x20let anthropic = get_provider_models(\"anthropic\");\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(!anthropic.is_empty(), \"Anthropic should have models\");\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(anthropic.iter().all(|m| m.provider == \"anthropic\"));\n\n\
         \x20\x20\x20\x20\x20\x20\x20let unknown = get_provider_models(\"nonexistent-provider\");\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(unknown.is_empty());\n\
         \x20   }\n\n",
    );

    out.push_str(
        "    #[test]\n\
         \x20   fn test_search_models() {\n\
         \x20\x20\x20\x20\x20\x20\x20let results = search_models(\"claude\");\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(!results.is_empty(), \"Should find Claude models\");\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(results\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20.iter()\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20.all(|m| m.name.to_lowercase().contains(\"claude\")\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20|| m.id.to_lowercase().contains(\"claude\")));\n\
         \x20   }\n\n",
    );

    out.push_str(
        "    #[test]\n\
         \x20   fn test_all_providers() {\n\
         \x20\x20\x20\x20\x20\x20\x20let providers = get_providers();\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(providers.contains(&\"openai\"), \"Should have openai\");\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(providers.contains(&\"anthropic\"), \"Should have anthropic\");\n\
         \x20   }\n\n",
    );

    out.push_str(
        "    #[test]\n\
         \x20   fn test_reasoning_models() {\n\
         \x20\x20\x20\x20\x20\x20\x20let reasoning = get_reasoning_models();\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(!reasoning.is_empty(), \"Should have reasoning models\");\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(reasoning.iter().all(|m| m.reasoning));\n\
         \x20   }\n\n",
    );

    out.push_str(
        "    #[test]\n\
         \x20   fn test_vision_models() {\n\
         \x20\x20\x20\x20\x20\x20\x20let vision = get_vision_models();\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(!vision.is_empty(), \"Should have vision models\");\n\
         \x20\x20\x20\x20\x20\x20\x20assert!(vision.iter().all(|m| m.supports_vision()));\n\
         \x20   }\n\n",
    );

    out.push_str(
        "    #[test]\n\
         \x20   fn test_cheapest_models() {\n\
         \x20\x20\x20\x20\x20\x20\x20let cheapest = get_cheapest_models(5);\n\
         \x20\x20\x20\x20\x20\x20\x20assert_eq!(cheapest.len(), 5.min(model_count()));\n\
         \x20\x20\x20\x20\x20\x20\x20for i in 1..cheapest.len() {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20assert!(cheapest[i].cost_input >= cheapest[i - 1].cost_input);\n\
         \x20\x20\x20\x20\x20\x20\x20}\n\
         \x20   }\n",
    );

    out.push_str("}\n");

    print!("{}", out);
}
