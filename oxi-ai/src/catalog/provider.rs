//! Provider metadata structures — TOML ↔ Rust.

use serde::{Deserialize, Serialize};

/// How a provider passes its API key in HTTP headers.
///
/// Maps to `register_builtins::AuthMethod` 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    /// `Authorization: Bearer <key>` — most OpenAI-compatible providers.
    Bearer,
    /// `x-api-key: <key>` — Anthropic and Anthropic-compatible providers.
    #[serde(rename = "x-api-key")]
    XApiKey,
    /// `api-key: <key>` — Azure OpenAI.
    #[serde(rename = "api-key")]
    ApiKey,
    /// No API key header (uses other auth like OAuth, SigV4).
    None,
}

/// A single built-in provider entry, deserialized from `data/catalog/providers.toml`.
///
/// All fields except `id`, `display_name`, `api`, `env_key`, `auth_method`,
/// `category`, `description` are optional with sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinProviderEntry {
    /// Primary provider name (e.g. "openai")
    pub id: String,
    /// Display name (e.g. "OpenAI")
    pub display_name: String,
    /// Alternative names that resolve to this provider
    #[serde(default)]
    pub aliases: Vec<String>,
    /// API type used by this provider
    pub api: String,
    /// Environment variable that may hold the API key
    pub env_key: String,
    /// Additional environment variables to check
    #[serde(default)]
    pub extra_env_keys: Vec<String>,
    /// Default base URL for the API (empty = computed at runtime)
    #[serde(default)]
    pub base_url: String,
    /// How to pass the API key
    pub auth_method: AuthMethod,
    /// Extra HTTP headers required by this provider
    #[serde(default)]
    pub extra_headers: Vec<(String, String)>,
    /// Provider category for UI grouping
    pub category: String,
    /// Short human-readable description
    pub description: String,
    /// Whether this provider is enabled by default
    #[serde(default = "default_enabled")]
    pub default_enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl BuiltinProviderEntry {
    /// Get all environment variable names for this provider (primary + extras).
    pub fn all_env_keys(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.env_key.as_str()).chain(self.extra_env_keys.iter().map(|s| s.as_str()))
    }

    /// Check if this is an OpenAI-compatible provider.
    pub fn is_openai_compatible(&self) -> bool {
        matches!(self.api.as_str(), "openai-completions" | "openai-responses")
    }
}

/// Load all built-in providers from the bundled TOML.
///
/// This is the public entry point for the catalog loader. It returns a
/// `'static` reference to the parsed providers, cached after first call.
pub fn load_builtin_providers() -> &'static [BuiltinProviderEntry] {
    crate::catalog::CatalogRoot::get().provider.as_slice()
}

/// Number of built-in providers.
pub fn builtin_providers_count() -> usize {
    load_builtin_providers().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_providers_have_valid_auth_method() {
        for p in load_builtin_providers() {
            // All four variants are valid; this just ensures none are missing
            match p.auth_method {
                AuthMethod::Bearer
                | AuthMethod::XApiKey
                | AuthMethod::ApiKey
                | AuthMethod::None => {}
            }
        }
    }

    #[test]
    fn all_providers_have_non_empty_env_key() {
        for p in load_builtin_providers() {
            assert!(!p.env_key.is_empty(), "Provider {} has empty env_key", p.id);
        }
    }

    #[test]
    fn openai_compatible_providers_use_bearer() {
        for p in load_builtin_providers() {
            if p.is_openai_compatible() {
                assert_eq!(
                    p.auth_method,
                    AuthMethod::Bearer,
                    "OpenAI-compatible provider {} should use Bearer auth",
                    p.id
                );
            }
        }
    }
}
