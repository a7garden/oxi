use oxi_ai::Model;
use oxi_ai::{get_model, lookup_model};

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
    get_model(provider, &model_id_part).cloned()
}
