//! models.dev → oxicode catalog materialization.
//!
//! Converts a [`MdCatalog`] (from models.dev `api.json`) into
//! [`BuiltinProviderEntry`] and [`BuiltinModelEntry`] vectors.
//!
//! This is the shared entry point for both **SNAP** (compile-time embedded
//! snapshot) and **LIVE** (runtime cache) sources.
//!
//! # Protocol resolution
//!
//! The [`protocol_for`] function maps a models.dev `npm` string to oxicode's
//! [`crate::Api`] enum + [`crate::catalog::provider::AuthMethod`]. This is the **only** protocol knowledge
//! oxicode has — 7 match arms covering all known API protocols. Unknown npm
//! values default to OpenAI-compatible (`OpenAiCompletions` + `Bearer`).
//!
//! # Model-level overrides
//!
//! Per-model [`crate::catalog::models_dev::MdModelProvider`] can override three things:
//! - `npm` → protocol + auth (via `protocol_for`)
//! - `api` → base URL (55+ models as of 2026-06-17)
//!
//! # Attribution
//!
//! Model data © [models.dev](https://models.dev) (MIT).

use crate::catalog::BuiltinModelEntry;
use crate::catalog::models_dev::{MdCatalog, protocol_for};
use crate::catalog::override_::{OverrideFile, apply_model_overrides, apply_provider_overrides};
use crate::catalog::provider::BuiltinProviderEntry;
use std::collections::BTreeMap;

/// Convert a [`MdCatalog`] into oxicode's internal provider + model entries.
///
/// Applies product-meta extra headers and user overrides (Layer 2) after
/// the base conversion.
///
/// Returns `(providers, models)` sorted by provider id.
pub fn materialize(
    catalog: &MdCatalog,
    product_meta: &ProductMeta,
    user_overrides: &OverrideFile,
) -> (
    Vec<BuiltinProviderEntry>,
    BTreeMap<String, Vec<BuiltinModelEntry>>,
) {
    let mut providers = Vec::new();
    let mut models: BTreeMap<String, Vec<BuiltinModelEntry>> = BTreeMap::new();

    for (pid, mdprov) in &catalog.0 {
        let (api, auth) = protocol_for(mdprov.npm.as_deref().unwrap_or(""));
        let extra = product_meta
            .extra_headers
            .get(pid)
            .cloned()
            .unwrap_or_default();

        providers.push(BuiltinProviderEntry {
            id: pid.clone(),
            display_name: mdprov.name.clone(),
            aliases: vec![], // models.dev id만 사용, 호환성 버림
            api: api.to_string(),
            env_key: mdprov.env.first().cloned().unwrap_or_default(),
            extra_env_keys: mdprov.env[1..].to_vec(),
            base_url: mdprov.api.clone().unwrap_or_default(),
            auth_method: auth,
            extra_headers: extra,
            category: String::new(),    // 제거됨 (Phase 2에서 최종 제거)
            description: String::new(), // 제품 메타 불필요
            default_enabled: true,
        });

        for (mid, mdmodel) in &mdprov.models {
            // Model-level override (v3): protocol + auth + base_url
            let model_prov = mdmodel.provider.as_ref();
            let model_npm = model_prov
                .and_then(|p| p.npm.as_deref())
                .unwrap_or_else(|| mdprov.npm.as_deref().unwrap_or(""));
            let (model_api, model_auth) = protocol_for(model_npm);
            let model_base_url = model_prov.and_then(|p| p.api.clone());

            models
                .entry(pid.clone())
                .or_default()
                .push(BuiltinModelEntry {
                    id: mid.clone(),
                    name: mdmodel.name.clone(),
                    api: model_api.to_string(),
                    provider: pid.clone(),
                    reasoning: mdmodel.reasoning,
                    auth_method: model_auth,
                    base_url: model_base_url,
                    input: normalize_modalities(&mdmodel.modalities),
                    cost_input: mdmodel.cost.as_ref().map(|c| c.input).unwrap_or(0.0),
                    cost_output: mdmodel.cost.as_ref().map(|c| c.output).unwrap_or(0.0),
                    cost_cache_read: mdmodel
                        .cost
                        .as_ref()
                        .and_then(|c| c.cache_read)
                        .unwrap_or(0.0),
                    cost_cache_write: mdmodel
                        .cost
                        .as_ref()
                        .and_then(|c| c.cache_write)
                        .unwrap_or(0.0),
                    context_window: mdmodel.limit.context as u32,
                    max_tokens: mdmodel.limit.output as u32,
                });
        }
    }

    // Apply Layer 2 user overrides (highest precedence)
    apply_provider_overrides(&mut providers, &user_overrides.provider);
    apply_model_overrides(&mut models, &user_overrides.model);

    (providers, models)
}

/// Raw gzip bytes of the embedded models.dev snapshot.
///
/// Single source of truth for the compiled-in catalog snapshot. Sibling
/// crates (notably `oxicode-sdk`) reference these bytes through this accessor
/// instead of their own `include_bytes!`, so the snapshot file is packaged
/// exactly once — here, inside `oxicode-ai` — and never escapes the crate root
/// in a published artifact. (A cross-crate `include_bytes!` in `oxicode-sdk`
/// pointed at `oxicode-ai/data/...` and made the published `oxicode-sdk` crate
/// uncompilable for downstream consumers — see 0.37.1.)
///
/// The path is relative to this source file and stays within `oxicode-ai`'s
/// own package, so it resolves correctly both in-tree and when `oxicode-ai` is
/// downloaded from crates.io.
pub fn snapshot_gzip_bytes() -> &'static [u8] {
    include_bytes!("../../data/catalog/_snapshot.json.gz")
}

/// Load the embedded SNAP snapshot, decompress it, and parse as `MdCatalog`.
///
/// This is the single source for both the provider registry and the model
/// database. No network access — the snapshot is embedded at compile time.
pub fn load_snapshot_catalog() -> Option<MdCatalog> {
    use std::io::Read;
    let compressed: &[u8] = snapshot_gzip_bytes();
    let mut decoder = flate2::read::GzDecoder::new(compressed);
    let mut json = String::new();
    decoder.read_to_string(&mut json).ok()?;
    serde_json::from_str::<MdCatalog>(&json).ok()
}

/// Materialize providers from the embedded snapshot.
///
/// Returns the full provider list (145+ providers from models.dev) after
/// applying product-meta overrides and Layer 2 user overrides.
pub fn materialize_providers() -> Vec<BuiltinProviderEntry> {
    let Some(catalog) = load_snapshot_catalog() else {
        return Vec::new();
    };
    let product_meta = ProductMeta::builtin();
    let overrides = crate::catalog::load_overrides().unwrap_or_default();
    let (providers, _models) = materialize(&catalog, &product_meta, &overrides);
    providers
}

/// Product-specific metadata: extra HTTP headers for a subset of providers.
///
/// Models.dev does not know oxicode-specific HTTP headers (e.g. `HTTP-Referer`
/// for OpenRouter). These are maintained in `data/catalog/product-meta.toml`.
#[derive(Default)]
pub struct ProductMeta {
    /// Extra HTTP headers keyed by models.dev provider id.
    pub extra_headers: std::collections::HashMap<String, Vec<(String, String)>>,
}

impl ProductMeta {
    /// Parse from the built-in product-meta.toml.
    pub fn builtin() -> Self {
        include_str!("../../data/catalog/product-meta.toml")
            .parse()
            .unwrap_or_default()
    }
}

impl std::str::FromStr for ProductMeta {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default)]
            provider: Vec<RawProvider>,
        }
        #[derive(serde::Deserialize)]
        struct RawProvider {
            id: String,
            #[serde(default)]
            extra_headers: Vec<(String, String)>,
        }
        let raw: Raw = toml::from_str(s).map_err(|e| e.to_string())?;
        let mut extra_headers = std::collections::HashMap::new();
        for p in raw.provider {
            if !p.extra_headers.is_empty() {
                extra_headers.insert(p.id, p.extra_headers);
            }
        }
        Ok(Self { extra_headers })
    }
}

/// Normalize models.dev modalities to oxicode input string list.
fn normalize_modalities(md: &Option<crate::catalog::models_dev::MdModalities>) -> Vec<String> {
    match md {
        Some(m) => match &m.input {
            Some(input) if !input.is_empty() => input.clone(),
            _ => vec!["text".to_string()],
        },
        _ => vec!["text".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Api;

    #[test]
    fn protocol_for_anthropic() {
        let (api, auth) = protocol_for("@ai-sdk/anthropic");
        assert_eq!(api, Api::AnthropicMessages);
        assert_eq!(auth, crate::catalog::provider::AuthMethod::XApiKey);
    }

    #[test]
    fn protocol_for_google() {
        let (api, auth) = protocol_for("@ai-sdk/google");
        assert_eq!(api, Api::GoogleGenerativeAi);
        assert_eq!(auth, crate::catalog::provider::AuthMethod::None);
    }

    #[test]
    fn protocol_for_openai_compatible() {
        let (api, auth) = protocol_for("@ai-sdk/openai-compatible");
        assert_eq!(api, Api::OpenAiCompletions);
        assert_eq!(auth, crate::catalog::provider::AuthMethod::Bearer);
    }

    #[test]
    fn protocol_for_unknown() {
        let (api, auth) = protocol_for("some-new-sdk");
        assert_eq!(api, Api::OpenAiCompletions);
        assert_eq!(auth, crate::catalog::provider::AuthMethod::Bearer);
    }

    #[test]
    fn protocol_for_empty() {
        let (api, auth) = protocol_for("");
        assert_eq!(api, Api::OpenAiCompletions);
        assert_eq!(auth, crate::catalog::provider::AuthMethod::Bearer);
    }

    #[test]
    fn protocol_for_mistral() {
        // omp treats Mistral as openai-completions-compatible (no separate
        // dialect); it falls through to the default Bearer/openai-completions.
        let (api, auth) = protocol_for("@ai-sdk/mistral");
        assert_eq!(api, Api::OpenAiCompletions);
        assert_eq!(auth, crate::catalog::provider::AuthMethod::Bearer);
    }

    #[test]
    fn protocol_for_azure() {
        let (api, auth) = protocol_for("@ai-sdk/azure");
        assert_eq!(api, Api::AzureOpenAiResponses);
        assert_eq!(auth, crate::catalog::provider::AuthMethod::ApiKey);
    }

    #[test]
    fn protocol_for_amazon_bedrock() {
        let (api, auth) = protocol_for("@ai-sdk/amazon-bedrock");
        assert_eq!(api, Api::BedrockConverseStream);
        assert_eq!(auth, crate::catalog::provider::AuthMethod::None);
    }

    #[test]
    fn product_meta_parses() {
        let meta = ProductMeta::builtin();
        // OpenRouter must have its HTTP-Referer header
        let openrouter_headers = meta.extra_headers.get("openrouter");
        assert!(
            openrouter_headers.is_some(),
            "product-meta.toml should include openrouter"
        );
        if let Some(headers) = openrouter_headers {
            assert!(headers.iter().any(|(k, _)| k == "HTTP-Referer"));
        }
    }

    #[test]
    fn materialize_snapshot_counts() {
        use std::io::Read;

        // Decompress the embedded SNAP snapshot
        let compressed = include_bytes!("../../data/catalog/_snapshot.json.gz");
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        let mut json = String::new();
        decoder.read_to_string(&mut json).unwrap();

        let catalog: crate::catalog::MdCatalog = serde_json::from_str(&json).unwrap();
        let (providers, models) =
            super::materialize(&catalog, &ProductMeta::default(), &Default::default());

        assert!(!providers.is_empty(), "providers should not be empty");
        assert_eq!(providers.len(), 145, "expected 145 providers");
        let model_count: usize = models.values().map(|v| v.len()).sum();
        assert_eq!(model_count, 5277, "expected 5277 models");
    }
}
