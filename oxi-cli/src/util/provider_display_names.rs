//! Provider ID to display name mappings.
//!
//! Human-readable display names for supported LLM providers.
//! Uses the built-in provider registry from `oxi_ai::register_builtins`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_provider() {
        assert_eq!(super::provider_display_name("anthropic"), "Anthropic");
        assert_eq!(super::provider_display_name("google"), "Google AI");
        assert_eq!(super::provider_display_name("openai"), "OpenAI");
        assert_eq!(super::provider_display_name("bedrock"), "Amazon Bedrock");
    }

    #[test]
    fn known_provider_by_alias() {
        // aliases are also supported
        assert_eq!(super::provider_display_name("amazon-bedrock"), "Amazon Bedrock");
        assert_eq!(super::provider_display_name("google-vertex"), "Google Vertex AI");
    }

    #[test]
    fn unknown_provider_falls_back() {
        assert_eq!(super::provider_display_name("unknown-provider"), "unknown-provider");
    }
}