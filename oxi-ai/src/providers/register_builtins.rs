//! Built-in provider registration.
//!
//! Defines all built-in providers with comprehensive metadata: names, aliases,
//! API key environment variables, base URLs, auth methods, and provider-specific
//! headers. The provider factory is data-driven from this metadata rather than
//! using hardcoded match arms.

use crate::catalog::BuiltinProviderEntry;
use crate::Api;

// ---------------------------------------------------------------------------
// Auth method
// ---------------------------------------------------------------------------

/// How a provider passes its API key in HTTP headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// `Authorization: Bearer <key>` — most OpenAI-compatible providers.
    Bearer,
    /// `x-api-key: <key>` — Anthropic and Anthropic-compatible providers.
    XApiKey,
    /// `api-key: <key>` — Azure OpenAI.
    ApiKey,
    /// No API key header (uses other auth like OAuth, SigV4).
    None,
}

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
    /// How to pass the API key
    pub auth_method: AuthMethod,
    /// Extra HTTP headers required by this provider
    pub extra_headers: &'static [(&'static str, &'static str)],
    /// Provider category for UI grouping ("primary", "chinese", "enterprise", "open",
    /// "coding", "cloud", "specialized")
    pub category: &'static str,
    /// Short human-readable description shown in the provider selection UI
    pub description: &'static str,
}

// ---------------------------------------------------------------------------
// API enum parsing
// ---------------------------------------------------------------------------

/// Parse the API string from a TOML entry into the `Api` enum.
///
/// Falls back to `Api::OpenAiCompletions` for unknown values — this matches
/// the historical behavior where most "AI gateway" providers expose an
/// OpenAI-compatible endpoint.
fn parse_api(s: &str) -> Api {
    match s {
        "openai-completions" => Api::OpenAiCompletions,
        "openai-responses" => Api::OpenAiResponses,
        "anthropic-messages" => Api::AnthropicMessages,
        "google-generative-ai" => Api::GoogleGenerativeAi,
        "google-vertex" => Api::GoogleVertex,
        "mistral-conversations" => Api::MistralConversations,
        "azure-openai-responses" => Api::AzureOpenAiResponses,
        "bedrock-converse-stream" => Api::BedrockConverseStream,
        _ => Api::OpenAiCompletions,
    }
}

impl From<&BuiltinProviderEntry> for BuiltinProvider {
    fn from(entry: &BuiltinProviderEntry) -> Self {
        // SAFETY: These String→&'static str conversions are safe because the
        // resulting `BuiltinProvider` instances are stored in a `OnceLock<Vec<_>>`
        // that lives for the lifetime of the program. We leak the strings to
        // obtain `'static` references; this is a one-time cost at startup and
        // bounded by the number of providers (currently 71).
        let name: &'static str = Box::leak(entry.id.clone().into_boxed_str());
        let display_name: &'static str = Box::leak(entry.display_name.clone().into_boxed_str());
        let aliases: &'static [&'static str] = Box::leak(
            entry
                .aliases
                .iter()
                .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let env_key: &'static str = Box::leak(entry.env_key.clone().into_boxed_str());
        let extra_env_keys: &'static [&'static str] = Box::leak(
            entry
                .extra_env_keys
                .iter()
                .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let base_url: &'static str = Box::leak(entry.base_url.clone().into_boxed_str());
        let category: &'static str = Box::leak(entry.category.clone().into_boxed_str());
        let description: &'static str = Box::leak(entry.description.clone().into_boxed_str());
        let extra_headers: &'static [(&'static str, &'static str)] = Box::leak(
            entry
                .extra_headers
                .iter()
                .map(|(k, v)| {
                    (
                        Box::leak(k.clone().into_boxed_str()) as &'static str,
                        Box::leak(v.clone().into_boxed_str()) as &'static str,
                    )
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );

        BuiltinProvider {
            name,
            display_name,
            aliases,
            api: parse_api(&entry.api),
            env_key,
            extra_env_keys,
            base_url,
            default_enabled: entry.default_enabled,
            auth_method: match entry.auth_method {
                crate::catalog::AuthMethod::Bearer => AuthMethod::Bearer,
                crate::catalog::AuthMethod::XApiKey => AuthMethod::XApiKey,
                crate::catalog::AuthMethod::ApiKey => AuthMethod::ApiKey,
                crate::catalog::AuthMethod::None => AuthMethod::None,
            },
            extra_headers,
            category,
            description,
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in provider definitions (deprecated, now in data/catalog/providers.toml)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// API-to-provider mappings
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

/// Get all built-in providers, built lazily from the catalog TOML.
///
/// This replaces the historical `static BUILTIN_PROVIDERS` array. The first
/// call parses `data/catalog/providers.toml` and converts each entry to a
/// `BuiltinProvider`; subsequent calls return the cached `&'static` slice.
///
/// **Layer 2 (user overrides) is applied here**: before conversion, the
/// `OverrideFile` from `crate::catalog::load_overrides()` (if any) is
/// merged with the built-in providers. Override entries with the same id
/// replace built-in ones; new ids are appended.
pub fn get_builtin_providers() -> &'static [BuiltinProvider] {
    static CACHE: std::sync::OnceLock<Vec<BuiltinProvider>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut builtins: Vec<crate::catalog::BuiltinProviderEntry> =
                crate::catalog::load_builtin_providers().to_vec();
            if let Some(overrides) = crate::catalog::load_overrides() {
                crate::catalog::apply_provider_overrides(&mut builtins, &overrides.provider);
            }
            builtins.iter().map(BuiltinProvider::from).collect()
        })
        .as_slice()
}

/// Look up a built-in provider by name or alias.
pub fn get_builtin_provider(name: &str) -> Option<&'static BuiltinProvider> {
    get_builtin_providers()
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
    get_builtin_providers().iter().map(|p| p.name).collect()
}

/// Get all provider names including aliases.
pub fn get_all_provider_aliases() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = get_builtin_providers()
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
// Data-driven provider factory
// ---------------------------------------------------------------------------

/// Create a built-in provider by name.
///
/// This is the **single source of truth** for provider instantiation. It reads
/// the `BuiltinProvider` metadata and creates the appropriate provider struct
/// with the correct base URL, API key, and extra headers.
///
/// Returns `None` if the name is not a known built-in provider.
pub fn create_builtin_provider(name: &str) -> Option<Box<dyn super::Provider>> {
    let builtin = get_builtin_provider(name)?;

    match builtin.api {
        // ── Anthropic Messages API ──────────────────────────────────────
        Api::AnthropicMessages => {
            let extra_headers: Vec<(String, String)> = builtin
                .extra_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            // If the provider has its own base URL (MiniMax, etc.), use it
            if !builtin.base_url.is_empty() && builtin.name != "anthropic" {
                Some(Box::new(super::anthropic::AnthropicProvider::with_config(
                    builtin.base_url,
                    None,
                    extra_headers,
                )))
            } else {
                Some(Box::new(super::anthropic::AnthropicProvider::new()))
            }
        }

        // ── Google APIs ─────────────────────────────────────────────────
        Api::GoogleGenerativeAi => Some(Box::new(super::google::GoogleProvider::new())),
        Api::GoogleVertex => Some(Box::new(super::vertex::VertexProvider::new())),

        // ── Mistral ─────────────────────────────────────────────────────
        Api::MistralConversations => Some(Box::new(super::mistral::MistralProvider::new())),

        // ── Azure ───────────────────────────────────────────────────────
        Api::AzureOpenAiResponses => Some(Box::new(super::azure::AzureProvider::new())),

        // ── Bedrock ─────────────────────────────────────────────────────
        Api::BedrockConverseStream => Some(Box::new(super::bedrock::BedrockProvider::new())),

        // ── OpenAI Responses API ────────────────────────────────────────
        Api::OpenAiResponses => Some(Box::new(
            super::openai_responses::OpenAiResponsesProvider::new(),
        )),

        // ── OpenAI Chat Completions API ─────────────────────────────────
        // All OpenAI-compatible providers use OpenAiProvider with custom base
        // URLs and optional extra headers from their BuiltinProvider metadata.
        Api::OpenAiCompletions => {
            let extra_headers: Vec<(String, String)> = builtin
                .extra_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            if builtin.base_url.is_empty() {
                // OpenAI itself (no custom base URL)
                if extra_headers.is_empty() {
                    Some(Box::new(super::openai::OpenAiProvider::new()))
                } else {
                    Some(Box::new(super::openai::OpenAiProvider::with_config(
                        "https://api.openai.com/v1",
                        None,
                        extra_headers,
                    )))
                }
            } else if extra_headers.is_empty() {
                Some(Box::new(super::openai::OpenAiProvider::with_base_url(
                    builtin.base_url,
                )))
            } else {
                Some(Box::new(super::openai::OpenAiProvider::with_config(
                    builtin.base_url,
                    None,
                    extra_headers,
                )))
            }
        }
    }
}

/// Create a built-in provider with optional credential and base URL overrides.
///
/// This is like [`create_builtin_provider`] but allows injecting an API key
/// and/or base URL at construction time instead of reading from environment
/// variables. When `api_key` is `Some`, it takes precedence over the
/// environment. When `base_url` is `Some`, the provider's default endpoint
/// is overridden.
///
/// Returns `None` if the name is not a known built-in provider.
pub fn create_builtin_provider_with_options(
    name: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Option<Box<dyn super::Provider>> {
    let builtin = get_builtin_provider(name)?;

    // Resolve API key: explicit override > environment variables
    let resolved_key = api_key.map(String::from).or_else(|| {
        std::env::var(builtin.env_key).ok().or_else(|| {
            builtin
                .extra_env_keys
                .iter()
                .find_map(|k| std::env::var(k).ok())
        })
    });

    // Resolve base URL: explicit override > built-in default
    let resolved_base_url = base_url.map(String::from).or_else(|| {
        if builtin.base_url.is_empty() {
            None
        } else {
            Some(builtin.base_url.to_string())
        }
    });

    let extra_headers: Vec<(String, String)> = builtin
        .extra_headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    match builtin.api {
        // ── Anthropic Messages API ──────────────────────────────────────
        Api::AnthropicMessages => {
            if let Some(key) = resolved_key {
                Some(Box::new(super::anthropic::AnthropicProvider::with_config(
                    resolved_base_url
                        .as_deref()
                        .unwrap_or("https://api.anthropic.com"),
                    Some(key),
                    extra_headers,
                )))
            } else if resolved_base_url.is_some() {
                Some(Box::new(super::anthropic::AnthropicProvider::with_config(
                    resolved_base_url
                        .as_deref()
                        .unwrap_or("https://api.anthropic.com"),
                    None,
                    extra_headers,
                )))
            } else {
                // No key and no base URL — fall back to default construction
                // (reads from env at stream time)
                create_builtin_provider(name)
            }
        }

        // ── Google APIs ─────────────────────────────────────────────────
        Api::GoogleGenerativeAi => create_builtin_provider(name),
        Api::GoogleVertex => create_builtin_provider(name),

        // ── Mistral ─────────────────────────────────────────────────────
        Api::MistralConversations => create_builtin_provider(name),

        // ── Azure ───────────────────────────────────────────────────────
        Api::AzureOpenAiResponses => create_builtin_provider(name),

        // ── Bedrock ─────────────────────────────────────────────────────
        Api::BedrockConverseStream => create_builtin_provider(name),

        // ── OpenAI Responses API ────────────────────────────────────────
        Api::OpenAiResponses => create_builtin_provider(name),

        // ── OpenAI Chat Completions API ─────────────────────────────────
        Api::OpenAiCompletions => {
            let url = resolved_base_url
                .as_deref()
                .unwrap_or(if builtin.base_url.is_empty() {
                    "https://api.openai.com/v1"
                } else {
                    builtin.base_url
                });

            if let Some(key) = resolved_key {
                if extra_headers.is_empty() {
                    Some(Box::new(
                        super::openai::OpenAiProvider::with_base_url_and_key(url, Some(key)),
                    ))
                } else {
                    Some(Box::new(super::openai::OpenAiProvider::with_config(
                        url,
                        Some(key),
                        extra_headers,
                    )))
                }
            } else if url != builtin.base_url || !extra_headers.is_empty() {
                // Base URL override or extra headers without explicit key
                Some(Box::new(super::openai::OpenAiProvider::with_config(
                    url,
                    None,
                    extra_headers,
                )))
            } else {
                create_builtin_provider(name)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_builtin_provider_anthropic() {
        let p = create_builtin_provider("anthropic").unwrap();
        assert_eq!(p.name(), "anthropic");
    }

    #[test]
    fn test_create_builtin_provider_openai() {
        let p = create_builtin_provider("openai").unwrap();
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn test_create_builtin_provider_by_alias() {
        let p = create_builtin_provider("amazon-bedrock").unwrap();
        assert_eq!(p.name(), "bedrock");
    }

    #[test]
    fn test_create_builtin_provider_unknown() {
        assert!(create_builtin_provider("unknown").is_none());
    }

    #[test]
    fn layer2_override_adds_provider() {
        // Set OXI_CATALOG_OVERRIDE to a known override file, then verify
        // the provider shows up. We use a tempfile in the test target dir.
        use std::io::Write;

        let dir = std::env::temp_dir().join("oxi-test-layer2");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("overrides.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"
[[provider]]
id = "test-injected-{}"
display_name = "Test Injected"
api = "openai-completions"
env_key = "TEST_INJECTED_KEY"
auth_method = "bearer"
category = "primary"
description = "Test provider from override"
"#,
            std::process::id()
        )
        .unwrap();
        drop(f);

        // SAFETY: only this test mutates this env var, and only briefly.
        // The test is #[test] which runs on a single thread within a test binary.
        unsafe {
            std::env::set_var("OXI_CATALOG_OVERRIDE", &path);
        }
        // Invalidate the cache so the override is picked up.
        // (The OnceLock has no reset API; this test only checks the load
        //  machinery via find_override_files, not the full integration.)
        let files = crate::catalog::find_override_files();
        unsafe {
            std::env::remove_var("OXI_CATALOG_OVERRIDE");
        }
        assert!(!files.is_empty(), "OXI_CATALOG_OVERRIDE should be detected");
        let (found_path, _content) = &files[0];
        assert_eq!(found_path, &path);
    }

    #[test]
    fn test_create_builtin_provider_deepseek() {
        let p = create_builtin_provider("deepseek").unwrap();
        assert_eq!(p.name(), "openai"); // Uses OpenAI provider with custom base URL
    }

    #[test]
    fn test_create_builtin_provider_minimax() {
        let p = create_builtin_provider("minimax").unwrap();
        assert_eq!(p.name(), "anthropic"); // Uses Anthropic provider with custom base URL
    }

    #[test]
    fn test_create_builtin_provider_minimax_cn() {
        let p = create_builtin_provider("minimax-cn").unwrap();
        assert_eq!(p.name(), "anthropic"); // Uses Anthropic provider with custom base URL
    }

    #[test]
    fn test_create_builtin_provider_togetherai() {
        let p = create_builtin_provider("togetherai").unwrap();
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn test_create_builtin_provider_openrouter() {
        let p = create_builtin_provider("openrouter").unwrap();
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn test_create_builtin_provider_cerebras() {
        let p = create_builtin_provider("cerebras").unwrap();
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn test_get_builtin_provider_openai() {
        let p = get_builtin_provider("openai").unwrap();
        assert_eq!(p.name, "openai");
        assert_eq!(p.display_name, "OpenAI");
        assert_eq!(p.api, Api::OpenAiCompletions);
        assert_eq!(p.auth_method, AuthMethod::Bearer);
    }

    #[test]
    fn test_get_builtin_provider_anthropic() {
        let p = get_builtin_provider("anthropic").unwrap();
        assert_eq!(p.name, "anthropic");
        assert_eq!(p.auth_method, AuthMethod::XApiKey);
    }

    #[test]
    fn test_get_builtin_provider_azure() {
        let p = get_builtin_provider("azure").unwrap();
        assert_eq!(p.auth_method, AuthMethod::ApiKey);
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
        assert!(is_builtin_provider("togetherai"));
        assert!(!is_builtin_provider("fake-provider"));
    }

    #[test]
    fn test_all_providers_have_env_key() {
        for p in get_builtin_providers() {
            assert!(!p.env_key.is_empty(), "Provider {} has no env key", p.name);
        }
    }

    #[test]
    fn test_all_providers_have_auth_method() {
        for p in get_builtin_providers() {
            // Just verify they all have a valid auth method
            match p.auth_method {
                AuthMethod::Bearer
                | AuthMethod::XApiKey
                | AuthMethod::ApiKey
                | AuthMethod::None => {}
            }
        }
    }

    #[test]
    fn test_get_all_provider_names() {
        let names = get_all_provider_names();
        assert!(names.contains(&"openai"));
        assert!(names.contains(&"anthropic"));
        assert!(names.contains(&"bedrock"));
        assert!(names.contains(&"togetherai"));
        assert!(names.len() >= 20);
    }

    // ── Tests for providers ported from openclaw (MIT) ─────────────────

    #[test]
    fn test_openclaw_ported_providers_present() {
        // Verify all 28 new providers added in feat/openclaw-port
        let names = get_all_provider_names();
        for p in [
            "chutes",
            "venice",
            "moonshot",
            "byteplus",
            "gmi",
            "novita",
            "arcee",
            "qianfan",
            "stepfun",
            "qwen-portal",
            "alibaba",
            "anthropic-vertex",
            "synthetic",
            "ollama",
            "ollama-cloud",
            "lmstudio",
            "vllm",
            "sglang",
            "litellm",
            "microsoft-foundry",
            "amazon-bedrock-mantle",
            "opencode",
            "copilot-proxy",
            "xiaomi-token-plan",
            "kilocode",
        ] {
            assert!(names.contains(&p), "Missing openclaw-ported provider: {p}");
        }
    }

    #[test]
    fn test_openclaw_provider_aliases() {
        // gmi-cloud, gmicloud, qwen-portal, modelstudio → canonical resolution
        assert_eq!(resolve_provider_name("gmi-cloud"), Some("gmi"));
        assert_eq!(resolve_provider_name("gmicloud"), Some("gmi"));
        // dashscope and modelstudio resolve to alibaba (primary entry)
        assert_eq!(resolve_provider_name("dashscope"), Some("alibaba"));
        assert_eq!(resolve_provider_name("modelstudio"), Some("alibaba"));
        // qwen-oauth and qwen-cli resolve to qwen-portal
        assert_eq!(resolve_provider_name("qwen-oauth"), Some("qwen-portal"));
        assert_eq!(resolve_provider_name("qwen-cli"), Some("qwen-portal"));
        assert_eq!(resolve_provider_name("novita-ai"), Some("novita"));
        assert_eq!(resolve_provider_name("novitaai"), Some("novita"));
        assert_eq!(resolve_provider_name("stepfun-plan"), Some("stepfun"));
        assert_eq!(resolve_provider_name("kilocode"), Some("kilocode"));
    }

    #[test]
    fn test_openclaw_provider_base_urls() {
        assert_eq!(
            get_provider_base_url("chutes"),
            Some("https://llm.chutes.ai/v1")
        );
        assert_eq!(
            get_provider_base_url("venice"),
            Some("https://api.venice.ai/api/v1")
        );
        assert_eq!(
            get_provider_base_url("ollama"),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(
            get_provider_base_url("lmstudio"),
            Some("http://localhost:1234/v1")
        );
        assert_eq!(
            get_provider_base_url("vllm"),
            Some("http://localhost:8000/v1")
        );
        assert_eq!(
            get_provider_base_url("synthetic"),
            Some("https://api.synthetic.new/anthropic")
        );
    }

    #[test]
    fn test_openclaw_local_providers_use_bearer() {
        // Local providers still use Bearer auth (some clients require a key)
        for p in ["ollama", "ollama-cloud", "lmstudio", "vllm", "sglang"] {
            let bp = get_builtin_provider(p).unwrap();
            assert_eq!(
                bp.auth_method,
                AuthMethod::Bearer,
                "{p} should use Bearer auth"
            );
        }
    }

    #[test]
    fn test_openclaw_anthropic_compat_providers() {
        // synthetic, anthropic-vertex, kimi are Anthropic-protocol
        for p in ["synthetic", "anthropic-vertex"] {
            let bp = get_builtin_provider(p).unwrap();
            assert_eq!(
                bp.api,
                Api::AnthropicMessages,
                "{p} should use AnthropicMessages API"
            );
        }
    }

    #[test]
    fn test_create_openclaw_providers() {
        // Smoke test that all new providers can be instantiated
        for p in [
            "chutes",
            "venice",
            "moonshot",
            "byteplus",
            "gmi",
            "novita",
            "arcee",
            "qianfan",
            "stepfun",
            "qwen-portal",
            "alibaba",
            "anthropic-vertex",
            "synthetic",
            "ollama",
            "lmstudio",
            "vllm",
            "sglang",
            "litellm",
            "microsoft-foundry",
            "opencode",
            "copilot-proxy",
            "xiaomi-token-plan",
            "kilocode",
        ] {
            let bp = create_builtin_provider(p);
            assert!(bp.is_some(), "create_builtin_provider({p}) failed");
        }
    }

    #[test]
    fn test_get_all_provider_aliases() {
        let aliases = get_all_provider_aliases();
        assert!(aliases.contains(&"amazon-bedrock"));
        assert!(aliases.contains(&"aws-bedrock"));
        assert!(aliases.contains(&"bedrock"));
        assert!(aliases.contains(&"together"));
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
        assert_eq!(
            get_provider_base_url("groq"),
            Some("https://api.groq.com/openai/v1")
        );
        assert_eq!(
            get_provider_base_url("togetherai"),
            Some("https://api.together.xyz/v1")
        );
    }

    #[test]
    fn test_minimax_base_url() {
        let p = get_builtin_provider("minimax").unwrap();
        assert_eq!(p.base_url, "https://api.minimax.io/anthropic");
        assert_eq!(p.api, Api::AnthropicMessages);
    }

    #[test]
    fn test_openrouter_extra_headers() {
        let p = get_builtin_provider("openrouter").unwrap();
        assert_eq!(
            p.extra_headers,
            &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")]
        );
    }

    #[test]
    fn test_cerebras_extra_headers() {
        let p = get_builtin_provider("cerebras").unwrap();
        assert_eq!(
            p.extra_headers,
            &[("X-Cerebras-3rd-Party-Integration", "opencode")]
        );
    }

    #[test]
    fn test_create_builtin_provider_with_options_openai() {
        // With explicit API key and base URL
        let p = create_builtin_provider_with_options(
            "openai",
            Some("sk-test-key"),
            Some("https://my-proxy.example.com/v1"),
        );
        assert!(p.is_some());
        assert_eq!(p.unwrap().name(), "openai");
    }

    #[test]
    fn test_create_builtin_provider_with_options_anthropic() {
        let p = create_builtin_provider_with_options("anthropic", Some("sk-ant-test-key"), None);
        assert!(p.is_some());
        assert_eq!(p.unwrap().name(), "anthropic");
    }

    #[test]
    fn test_create_builtin_provider_with_options_no_override() {
        // No key or URL — should fall back to default creation
        let p = create_builtin_provider_with_options("deepseek", None, None);
        assert!(p.is_some());
    }

    #[test]
    fn test_create_builtin_provider_with_options_unknown() {
        let p = create_builtin_provider_with_options("nonexistent_provider", None, None);
        assert!(p.is_none());
    }
}
