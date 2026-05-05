use oxi_ai::get_model;
use oxi_ai::Model;

/// Parse a model ID in "provider/model" or plain "model" format.
/// Uses >= 2 segments (handles provider/org/model format).
pub fn resolve_model_from_id(model_id: &str) -> Option<Model> {
    let parts: Vec<&str> = model_id.split('/').collect();
    if parts.len() >= 2 {
        get_model(parts[0], &parts[1..].join("/")).cloned()
    } else {
        get_model("anthropic", model_id).cloned()
    }
}
