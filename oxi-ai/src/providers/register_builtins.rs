#![allow(dead_code)]
//! Built-in provider registration.
//!
//! Ported from `register-builtins.ts`. Defines all built-in providers with their
//! metadata: names, aliases, API key environment variables, and provider-to-model
//! mappings. Also provides a central registry for looking up provider metadata.
//!
//! Unlike the TypeScript version which uses lazy dynamic imports, Rust's static
//! type system allows us to define all providers at compile time.

use crate::Api;

// ---------------------------------------------------------------------------
// Provider metadata
// ---------------------------------------------------------------------------

/// Metadata for a built-in provider.
#[derive(Debug, Clone)]
pub struct BuiltinProvider {
    /// Primary provider name (e.g. "openai")
    pub name: &'static str,
    /// Display name (e.g. "OpenAI")
    pub display_name: &'static str,
    /// Alternative names that resolve to this provider
    pub aliases: &'static [&'static str],
    /// API type used by this provider
    pub api: Api,
    /// Environment variable(s) that may hold the API key (in priority order)
    pub env_key: &'static str,
    /// Additional environment variables to check
    pub extra_env_keys: &'static [&'static str],
    /// Default base URL for the API
    pub base_url: &'static str,
    /// Whether this provider is enabled by default
    pub default_enabled: bool,
}

// ---------------------------------------------------------------------------
// Built-in provider definitions
// ---------------------------------------------------------------------------

/// All built-in providers, defined statically.
static BUILTIN_PROVIDERS: &[BuiltinProvider] = &[
    BuiltinProvider {
        name: "openai",
        display_name: "OpenAI",
        aliases: &["openai"],
        api: Api::OpenAiCompletions,
        env_key: "OPENAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.openai.com/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "openai-responses",
        display_name: "OpenAI Responses API",
        aliases: &["openai-responses"],
        api: Api::OpenAiResponses,
        env_key: "OPENAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.openai.com/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "openai-completions",
        display_name: "OpenAI Completions API (Legacy)",
        aliases: &["openai-completions", "completions"],
        api: Api::OpenAiCompletions,
        env_key: "OPENAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.openai.com/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "anthropic",
        display_name: "Anthropic",
        aliases: &["anthropic"],
        api: Api::AnthropicMessages,
        env_key: "ANTHROPIC_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.anthropic.com",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "google",
        display_name: "Google AI",
        aliases: &["google"],
        api: Api::GoogleGenerativeAi,
        env_key: "GOOGLE_API_KEY",
        extra_env_keys: &["GEMINI_API_KEY"],
        base_url: "https://generativelanguage.googleapis.com",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "vertex",
        display_name: "Google Vertex AI",
        aliases: &["vertex", "google-vertex"],
        api: Api::GoogleVertex,
        env_key: "GOOGLE_APPLICATION_CREDENTIALS",
        extra_env_keys: &["GOOGLE_CLOUD_PROJECT"],
        base_url: "https://us-central1-aiplatform.googleapis.com",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "mistral",
        display_name: "Mistral",
        aliases: &["mistral"],
        api: Api::MistralConversations,
        env_key: "MISTRAL_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.mistral.ai",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "azure",
        display_name: "Azure OpenAI",
        aliases: &["azure", "azure-openai"],
        api: Api::AzureOpenAiResponses,
        env_key: "AZURE_OPENAI_API_KEY",
        extra_env_keys: &["AZURE_API_KEY"],
        base_url: "", // Configured per deployment
        default_enabled: true,
    },
    BuiltinProvider {
        name: "bedrock",
        display_name: "Amazon Bedrock",
        aliases: &["bedrock", "amazon-bedrock", "aws-bedrock"],
        api: Api::BedrockConverseStream,
        env_key: "AWS_ACCESS_KEY_ID",
        extra_env_keys: &["AWS_PROFILE", "AWS_REGION"],
        base_url: "", // Uses AWS SDK resolution
        default_enabled: true,
    },
    BuiltinProvider {
        name: "deepseek",
        display_name: "DeepSeek",
        aliases: &["deepseek"],
        api: Api::OpenAiCompletions,
        env_key: "DEEPSEEK_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.deepseek.com",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "groq",
        display_name: "Groq",
        aliases: &["groq"],
        api: Api::OpenAiCompletions,
        env_key: "GROQ_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.groq.com/openai/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "cerebras",
        display_name: "Cerebras",
        aliases: &["cerebras"],
        api: Api::OpenAiCompletions,
        env_key: "CEREBRAS_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.cerebras.ai/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "xai",
        display_name: "xAI (Grok)",
        aliases: &["xai", "grok"],
        api: Api::OpenAiCompletions,
        env_key: "XAI_API_KEY",
        extra_env_keys: &["GROK_API_KEY"],
        base_url: "https://api.x.ai/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "openrouter",
        display_name: "OpenRouter",
        aliases: &["openrouter"],
        api: Api::OpenAiCompletions,
        env_key: "OPENROUTER_API_KEY",
        extra_env_keys: &[],
        base_url: "https://openrouter.ai/api/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "fireworks",
        display_name: "Fireworks AI",
        aliases: &["fireworks"],
        api: Api::OpenAiCompletions,
        env_key: "FIREWORKS_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.fireworks.ai/inference/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "cloudflare",
        display_name: "Cloudflare Workers AI",
        aliases: &["cloudflare", "workers-ai"],
        api: Api::OpenAiCompletions,
        env_key: "CLOUDFLARE_API_KEY",
        extra_env_keys: &["CF_API_KEY"],
        base_url: "https://api.cloudflare.com/client/v4/accounts",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "copilot",
        display_name: "GitHub Copilot",
        aliases: &["copilot", "github-copilot"],
        api: Api::OpenAiCompletions,
        env_key: "GITHUB_COPILOT_TOKEN",
        extra_env_keys: &["GITHUB_TOKEN"],
        base_url: "https://api.githubcopilot.com",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "codex",
        display_name: "GitHub Codex",
        aliases: &["codex", "github-codex", "copilot-codex"],
        api: Api::OpenAiCompletions,
        env_key: "GITHUB_COPILOT_TOKEN",
        extra_env_keys: &["GITHUB_TOKEN"],
        base_url: "https://api.githubcopilot.com",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "minimax",
        display_name: "MiniMax",
        aliases: &["minimax"],
        api: Api::OpenAiCompletions,
        env_key: "MINIMAX_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.minimax.chat/v1",
        default_enabled: true,
    },
    BuiltinProvider {
        name: "zai",
        display_name: "ZAI",
        aliases: &["zai"],
        api: Api::OpenAiCompletions,
        env_key: "ZAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.z.ai/api/coding/paas/v4",
        default_enabled: true,
    },
];

// ---------------------------------------------------------------------------
// Provider-to-API mappings (ported from register-builtins.ts API registration)
// ---------------------------------------------------------------------------

/// Mapping from API identifier to the primary provider name.
static API_TO_PROVIDER: &[(&str, Api)] = &[
    ("anthropic-messages", Api::AnthropicMessages),
    ("openai-completions", Api::OpenAiCompletions),
    ("mistral-conversations", Api::MistralConversations),
    ("openai-responses", Api::OpenAiResponses),
    ("azure-openai-responses", Api::AzureOpenAiResponses),
    ("google-generative-ai", Api::GoogleGenerativeAi),
    ("google-vertex", Api::GoogleVertex),
    ("bedrock-converse-stream", Api::BedrockConverseStream),
];

// ---------------------------------------------------------------------------
// Registry access functions
// ---------------------------------------------------------------------------

/// Get all built-in providers.
pub fn get_builtin_providers() -> &'static [BuiltinProvider] {
    BUILTIN_PROVIDERS
}

/// Look up a built-in provider by name or alias.
pub fn get_builtin_provider(name: &str) -> Option<&'static BuiltinProvider> {
    BUILTIN_PROVIDERS
        .iter()
        .find(|p| p.name == name || p.aliases.contains(&name))
}

/// Get the environment variable name for a provider.
pub fn get_provider_env_key(name: &str) -> Option<&'static str> {
    get_builtin_provider(name).map(|p| p.env_key)
}

/// Get all environment variable names for a provider (primary + extras).
pub fn get_provider_env_keys(name: &str) -> Vec<&'static str> {
    if let Some(p) = get_builtin_provider(name) {
        let mut keys = vec![p.env_key];
        keys.extend_from_slice(p.extra_env_keys);
        keys
    } else {
        vec![]
    }
}

/// Get the API type for a provider by name or alias.
pub fn get_provider_api(name: &str) -> Option<Api> {
    get_builtin_provider(name).map(|p| p.api)
}

/// Get the default base URL for a provider.
pub fn get_provider_base_url(name: &str) -> Option<&'static str> {
    get_builtin_provider(name).map(|p| p.base_url)
}

/// Get all API-to-provider mappings.
pub fn get_api_mappings() -> &'static [(&'static str, Api)] {
    API_TO_PROVIDER
}

/// Get all provider names (primary names only).
pub fn get_all_provider_names() -> Vec<&'static str> {
    BUILTIN_PROVIDERS.iter().map(|p| p.name).collect()
}

/// Get all provider names including aliases.
pub fn get_all_provider_aliases() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = BUILTIN_PROVIDERS
        .iter()
        .flat_map(|p| std::iter::once(p.name).chain(p.aliases.iter().copied()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Resolve a provider name/alias to its canonical name.
pub fn resolve_provider_name(name: &str) -> Option<&'static str> {
    get_builtin_provider(name).map(|p| p.name)
}

/// Check if a provider name or alias is a known built-in.
pub fn is_builtin_provider(name: &str) -> bool {
    get_builtin_provider(name).is_some()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_builtin_provider_openai() {
        let p = get_builtin_provider("openai").unwrap();
        assert_eq!(p.name, "openai");
        assert_eq!(p.display_name, "OpenAI");
        assert_eq!(p.api, Api::OpenAiCompletions);
    }

    #[test]
    fn test_get_builtin_provider_by_alias() {
        let p = get_builtin_provider("amazon-bedrock").unwrap();
        assert_eq!(p.name, "bedrock");
    }

    #[test]
    fn test_get_builtin_provider_unknown() {
        assert!(get_builtin_provider("unknown-provider").is_none());
    }

    #[test]
    fn test_get_provider_env_key() {
        assert_eq!(get_provider_env_key("openai"), Some("OPENAI_API_KEY"));
        assert_eq!(get_provider_env_key("anthropic"), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn test_get_provider_env_keys_with_extras() {
        let keys = get_provider_env_keys("google");
        assert!(keys.contains(&"GOOGLE_API_KEY"));
        assert!(keys.contains(&"GEMINI_API_KEY"));
    }

    #[test]
    fn test_get_provider_api() {
        assert_eq!(get_provider_api("anthropic"), Some(Api::AnthropicMessages));
        assert_eq!(get_provider_api("vertex"), Some(Api::GoogleVertex));
    }

    #[test]
    fn test_resolve_provider_name() {
        assert_eq!(resolve_provider_name("google-vertex"), Some("vertex"));
        assert_eq!(resolve_provider_name("aws-bedrock"), Some("bedrock"));
        assert_eq!(resolve_provider_name("openai"), Some("openai"));
    }

    #[test]
    fn test_is_builtin_provider() {
        assert!(is_builtin_provider("openai"));
        assert!(is_builtin_provider("deepseek"));
        assert!(!is_builtin_provider("fake-provider"));
    }

    #[test]
    fn test_all_providers_have_env_key() {
        for p in get_builtin_providers() {
            assert!(!p.env_key.is_empty(), "Provider {} has no env key", p.name);
        }
    }

    #[test]
    fn test_get_all_provider_names() {
        let names = get_all_provider_names();
        assert!(names.contains(&"openai"));
        assert!(names.contains(&"anthropic"));
        assert!(names.contains(&"bedrock"));
        assert!(names.len() >= 15);
    }

    #[test]
    fn test_get_all_provider_aliases() {
        let aliases = get_all_provider_aliases();
        assert!(aliases.contains(&"amazon-bedrock"));
        assert!(aliases.contains(&"aws-bedrock"));
        assert!(aliases.contains(&"bedrock"));
    }

    #[test]
    fn test_get_provider_base_url() {
        assert_eq!(
            get_provider_base_url("openai"),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            get_provider_base_url("anthropic"),
            Some("https://api.anthropic.com")
        );
    }
}
