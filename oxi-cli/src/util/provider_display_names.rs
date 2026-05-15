//! Provider ID to display name mappings.
//!
//! Human-readable display names for supported LLM providers.
//! Uses the built-in provider registry from `oxi_ai::register_builtins`.

/// Look up a display name for a provider ID.
///
/// Falls back to returning the raw `provider_id` if no mapping exists.
#[allow(dead_code)]
pub fn provider_display_name(provider_id: &str) -> &str {
    oxi_ai::register_builtins::get_builtin_provider(provider_id)
        .map(|p| p.display_name)
        .unwrap_or(provider_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_provider() {
        assert_eq!(provider_display_name("anthropic"), "Anthropic");
        assert_eq!(provider_display_name("google"), "Google AI");
        assert_eq!(provider_display_name("openai"), "OpenAI");
        assert_eq!(provider_display_name("bedrock"), "Amazon Bedrock");
    }

    #[test]
    fn known_provider_by_alias() {
        // aliases are also supported
        assert_eq!(provider_display_name("amazon-bedrock"), "Amazon Bedrock");
        assert_eq!(provider_display_name("google-vertex"), "Google Vertex AI");
    }

    #[test]
    fn unknown_provider_falls_back() {
        assert_eq!(provider_display_name("unknown-provider"), "unknown-provider");
    }
}