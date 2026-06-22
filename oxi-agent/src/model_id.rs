use std::collections::HashMap;
use oxi_ai::Model;
use oxi_ai::{get_model, get_model_entry, lookup_model};
use oxi_ai::register_builtins::get_builtin_provider;

/// Parse a model ID in "provider/model" or plain "model" format.
/// Uses >= 2 segments (handles provider/org/model format).
pub fn resolve_model_from_id(model_id: &str) -> Option<Model> {
    let parts: Vec<&str> = model_id.split('/').collect();
    let provider = if parts.len() >= 2 {
        parts[0]
    } else {
        "anthropic"
    };
    let model_id_part = if parts.len() >= 2 {
        parts[1..].join("/")
    } else {
        parts[0].to_string()
    };

    // Check dynamic registry first (includes router/auto and custom provider models).
    if let Some(m) = lookup_model(provider, &model_id_part) {
        return Some(m);
    }
    // Fall back to static registry.
    if let Some(m) = get_model(provider, &model_id_part) {
        return Some(m.clone());
    }
    // Fallback: construct from ModelEntry catalog (materialized from models.dev snapshot).
    // This handles catalog-only providers (e.g. "zai-coding-plan/glm-5-turbo") that
    // aren't in the hand-written STATIC_MODELS registry.
    if let Some(entry) = get_model_entry(provider, &model_id_part) {
        return Some(model_from_entry(provider, entry));
    }
    None
}
/// Construct a `Model` from a `ModelEntry` catalog entry + `BuiltinProvider` metadata.
///
/// Used as fallback when a model is in the materialized models.dev catalog
/// but not in the static `STATIC_MODELS` registry (e.g. `zai-coding-plan/glm-5-turbo`).
fn model_from_entry(provider: &str, entry: &oxi_ai::model_db::ModelEntry) -> Model {
    let builtin = get_builtin_provider(provider);
    let base_url = builtin
        .map(|b| b.base_url)
        .unwrap_or("")
        .to_string();
    let headers: HashMap<String, String> = builtin
        .map(|b| {
            b.extra_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let compat = match provider {
        "zai" => Some(oxi_ai::CompatSettings {
            thinking_format: Some(oxi_ai::ThinkingFormat::Zai),
            ..Default::default()
        }),
        _ => None,
    };

    Model {
        id: entry.id.to_string(),
        name: entry.name.to_string(),
        api: entry.api,
        provider: provider.to_string(),
        base_url,
        reasoning: entry.reasoning,
        input: entry.input.to_vec(),
        cost: oxi_ai::Cost {
            input: entry.cost_input,
            output: entry.cost_output,
            cache_read: entry.cost_cache_read,
            cache_write: entry.cost_cache_write,
        },
        context_window: entry.context_window as usize,
        max_tokens: entry.max_tokens as usize,
        headers,
        compat,
    }
}
