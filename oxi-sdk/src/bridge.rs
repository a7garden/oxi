//! Bridge layer: convert catalog port entries into `oxi_ai` types.
//!
//! This module sits in `oxi-sdk` (not `oxi-ai`) to avoid a reverse
//! dependency: `oxi-ai` must not depend on `oxi-sdk`. The catalog port
//! owns the data; the bridge owns the conversion into the types that
//! `Provider` implementations consume.
//!
//! See `docs/designs/2026-06-17-catalog-port-design.md` §7.5.

use crate::ports::catalog::{CatalogModelEntry, ModelCatalog};
use oxi_ai::{Cost, InputModality, Model};

/// Convert a `CatalogModelEntry` into an `oxi_ai::Model`.
///
/// The `provider` argument is passed explicitly (rather than read from the
/// entry) because callers sometimes resolve with a different provider name
/// than the one stored in the catalog (e.g. aliases).
///
/// Base URL resolution: if the model entry has a `base_url` override it is
/// used; otherwise the caller should resolve the provider's base URL
/// separately. Here we default to an empty string when neither is available
/// — callers that need a provider base URL should use
/// [`provider_base_url`].
pub fn catalog_entry_to_model(provider: &str, entry: &CatalogModelEntry) -> Model {
    Model {
        id: entry.model_id.clone(),
        name: entry.name.clone(),
        api: entry.protocol.as_oxi_api(),
        provider: provider.to_string(),
        base_url: entry.base_url.clone().unwrap_or_default(),
        reasoning: entry.reasoning,
        input: modalities_from_catalog(&entry.input_modalities, entry.supports_vision),
        cost: Cost {
            input: entry.cost_input,
            output: entry.cost_output,
            cache_read: entry.cost_cache_read,
            cache_write: entry.cost_cache_write,
        },
        context_window: entry.context_window as usize,
        max_tokens: entry.max_tokens as usize,
        headers: HashMap::new(),
        compat: None,
    }
}

/// Resolve a provider's base URL from the catalog port (sync read).
///
/// Returns `None` if the provider is unknown or has no base URL (e.g.
/// providers that use environment-configured endpoints like Anthropic/OpenAI).
pub fn provider_base_url(catalog: &dyn ModelCatalog, provider: &str) -> Option<String> {
    catalog.get_provider_sync(provider).and_then(|p| p.base_url)
}

/// Convert catalog modalities (string list) into `InputModality`.
///
/// Falls back to `[Text]` if the list is empty. Adds `Image` if
/// `supports_vision` is true and it isn't already present.
fn modalities_from_catalog(modalities: &[String], supports_vision: bool) -> Vec<InputModality> {
    let mut out: Vec<InputModality> = if modalities.is_empty() {
        vec![InputModality::Text]
    } else {
        modalities
            .iter()
            .filter_map(|m| match m.to_lowercase().as_str() {
                "text" => Some(InputModality::Text),
                "image" | "images" | "video" | "audio" | "pdf" | "file" | "files" => {
                    // Currently only Text/Image are supported; treat
                    // multimedia as Image where vision is available.
                    Some(InputModality::Image)
                }
                _ => None,
            })
            .collect()
    };

    // Ensure at least Text
    if !out.iter().any(|m| matches!(m, InputModality::Text)) {
        out.insert(0, InputModality::Text);
    }

    // Vision support
    if supports_vision && !out.iter().any(|m| matches!(m, InputModality::Image)) {
        out.push(InputModality::Image);
    }

    out
}

use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::catalog::{CatalogProtocol, CatalogSource};
    use oxi_ai::Api;

    fn sample_entry() -> CatalogModelEntry {
        CatalogModelEntry {
            provider: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            protocol: CatalogProtocol::OpenAiCompletions,
            source: CatalogSource::Embedded,
            base_url: None,
            reasoning: false,
            supports_vision: true,
            cost_input: 2.5,
            cost_output: 10.0,
            cost_cache_read: 1.25,
            cost_cache_write: 0.0,
            context_window: 128_000,
            max_tokens: 16_384,
            input_modalities: vec!["text".to_string(), "image".to_string()],
            release_date: None,
            status: None,
        }
    }

    #[test]
    fn converts_basic_fields() {
        let entry = sample_entry();
        let model = catalog_entry_to_model("openai", &entry);
        assert_eq!(model.id, "gpt-4o");
        assert_eq!(model.name, "GPT-4o");
        assert_eq!(model.api, Api::OpenAiCompletions);
        assert_eq!(model.provider, "openai");
        assert_eq!(model.context_window, 128_000);
        assert_eq!(model.max_tokens, 16_384);
        assert!(!model.reasoning);
    }

    #[test]
    fn converts_pricing() {
        let entry = sample_entry();
        let model = catalog_entry_to_model("openai", &entry);
        assert!((model.cost.input - 2.5).abs() < f64::EPSILON);
        assert!((model.cost.output - 10.0).abs() < f64::EPSILON);
        assert!((model.cost.cache_read - 1.25).abs() < f64::EPSILON);
    }

    #[test]
    fn converts_modalities_with_vision() {
        let entry = sample_entry();
        let model = catalog_entry_to_model("openai", &entry);
        assert!(model.input.contains(&InputModality::Text));
        assert!(model.input.contains(&InputModality::Image));
    }

    #[test]
    fn adds_vision_from_flag() {
        let mut entry = sample_entry();
        entry.input_modalities = vec!["text".to_string()];
        entry.supports_vision = true;
        let model = catalog_entry_to_model("openai", &entry);
        assert!(model.input.contains(&InputModality::Image));
    }

    #[test]
    fn empty_modalities_defaults_to_text() {
        let mut entry = sample_entry();
        entry.input_modalities = vec![];
        entry.supports_vision = false;
        let model = catalog_entry_to_model("openai", &entry);
        assert_eq!(model.input, vec![InputModality::Text]);
    }

    #[test]
    fn base_url_override_used() {
        let mut entry = sample_entry();
        entry.base_url = Some("https://custom.api/v1".to_string());
        let model = catalog_entry_to_model("openai", &entry);
        assert_eq!(model.base_url, "https://custom.api/v1");
    }

    #[test]
    fn protocol_maps_correctly() {
        let cases = [
            (CatalogProtocol::OpenAiCompletions, Api::OpenAiCompletions),
            (CatalogProtocol::OpenAiResponses, Api::OpenAiResponses),
            (CatalogProtocol::AnthropicMessages, Api::AnthropicMessages),
            (CatalogProtocol::GoogleGenerativeAi, Api::GoogleGenerativeAi),
        ];
        for (proto, expected_api) in cases {
            let mut entry = sample_entry();
            entry.protocol = proto;
            let model = catalog_entry_to_model("test", &entry);
            assert_eq!(model.api, expected_api);
        }
    }
}
