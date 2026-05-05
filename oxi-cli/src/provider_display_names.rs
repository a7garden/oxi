//! Provider ID to display name mappings.
//!
//! Human-readable display names for supported LLM providers.

use std::collections::HashMap;
use once_cell::sync::Lazy;

/// Built-in mapping of provider IDs to human-readable display names.
pub static BUILT_IN_PROVIDER_DISPLAY_NAMES: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("anthropic", "Anthropic");
    m.insert("amazon-bedrock", "Amazon Bedrock");
    m.insert("azure-openai-responses", "Azure OpenAI Responses");
    m.insert("cerebras", "Cerebras");
    m.insert("cloudflare-ai-gateway", "Cloudflare AI Gateway");
    m.insert("cloudflare-workers-ai", "Cloudflare Workers AI");
    m.insert("deepseek", "DeepSeek");
    m.insert("fireworks", "Fireworks");
    m.insert("google", "Google Gemini");
    m.insert("google-vertex", "Google Vertex AI");
    m.insert("groq", "Groq");
    m.insert("huggingface", "Hugging Face");
    m.insert("kimi-coding", "Kimi For Coding");
    m.insert("mistral", "Mistral");
    m.insert("minimax", "MiniMax");
    m.insert("minimax-cn", "MiniMax (China)");
    m.insert("moonshotai", "Moonshot AI");
    m.insert("moonshotai-cn", "Moonshot AI (China)");
    m.insert("opencode", "OpenCode Zen");
    m.insert("opencode-go", "OpenCode Go");
    m.insert("openai", "OpenAI");
    m.insert("openrouter", "OpenRouter");
    m.insert("vercel-ai-gateway", "Vercel AI Gateway");
    m.insert("xai", "xAI");
    m.insert("zai", "ZAI");
    m.insert("xiaomi", "Xiaomi MiMo Token Plan");
    m
});

/// Look up a display name for a provider ID.
///
/// Falls back to returning the raw `provider_id` if no mapping exists.
pub fn provider_display_name(provider_id: &str) -> &str {
    BUILT_IN_PROVIDER_DISPLAY_NAMES
        .get(provider_id)
        .copied()
        .unwrap_or(provider_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_provider() {
        assert_eq!(provider_display_name("anthropic"), "Anthropic");
        assert_eq!(provider_display_name("google"), "Google Gemini");
    }

    #[test]
    fn unknown_provider_falls_back() {
        assert_eq!(provider_display_name("unknown-provider"), "unknown-provider");
    }
}
