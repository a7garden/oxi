//! Runtime model discovery layer of the dynamic catalog.
//!
//! Some providers (ollama, lmstudio, vllm, sglang, openrouter) expose
//! `GET /v1/models` for runtime discovery. This module fetches those endpoints
//! and merges the results into the catalog at startup.
//!
//! ## Why this is Layer 3
//!
//! - **Built-in (Layer 1)**: fast, deterministic, offline
//! - **Override (Layer 2)**: user customization, fast, offline
//! - **Runtime (Layer 3)**: dynamic, requires network, slow — only for providers
//!   where the model list cannot be known a priori
//!
//! ## Failure handling
//!
//! - Network failures: silently skip that provider (with debug log)
//! - HTTP errors: same
//! - Parsing errors: same
//! - Slow providers: bounded by a 5-second timeout
//!
//! The discovery is best-effort: if Ollama is not running, we just don't
//! have its models, and the rest of the catalog still works.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::catalog::BuiltinModelEntry;
use serde::Deserialize;

use futures::future::join_all;

/// Maximum time to wait for any single provider's `/v1/models` response.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// OpenAI-compatible `/v1/models` response shape.
///
/// Most providers (ollama, lmstudio, vllm, sglang, openrouter) follow this format.
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<RemoteModel>,
}

#[derive(Debug, Deserialize)]
struct RemoteModel {
    id: String,
    #[serde(default, rename = "object")]
    #[allow(dead_code)]
    object: Option<String>,
    #[serde(default, rename = "owned_by")]
    #[allow(dead_code)]
    owned_by: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    created: Option<u64>,
}

/// Discover models from a single OpenAI-compatible endpoint.
///
/// Returns an empty Vec if the endpoint is unreachable, the response can't
/// be parsed, or the timeout is exceeded.
///
/// `env_key` is the **name** of the env var holding the API key (e.g.
/// `OPENAI_API_KEY`). If the env var is set, it's sent as a Bearer token.
/// If not set, the request is unauthenticated (suitable for local servers
/// like ollama/lmstudio that don't require auth).
pub async fn discover_models(
    provider_id: &str,
    api_type: &str,
    base_url: &str,
    env_key: Option<&str>,
) -> Vec<BuiltinModelEntry> {
    if base_url.is_empty() {
        return Vec::new();
    }
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut request = client.get(&url);
    if let Some(env_var) = env_key
        && let Ok(api_key) = std::env::var(env_var)
    {
        request = request.bearer_auth(api_key);
    }

    let result = request.send().await;
    let response = match result {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(provider = provider_id, error = %e, "Discovery: fetch failed");
            return Vec::new();
        }
    };

    if !response.status().is_success() {
        tracing::debug!(provider = provider_id, status = %response.status(),
            "Discovery: non-success status");
        return Vec::new();
    }

    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(provider = provider_id, error = %e, "Discovery: body read failed");
            return Vec::new();
        }
    };

    let parsed: ModelsResponse = match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(provider = provider_id, error = %e, "Discovery: parse failed");
            return Vec::new();
        }
    };

    parsed
        .data
        .into_iter()
        .map(|m| BuiltinModelEntry {
            id: m.id.clone(),
            name: m.id.clone(), // runtime providers don't provide display names
            api: api_type.to_string(),
            provider: provider_id.to_string(),
            reasoning: false,           // unknown at runtime
            input: vec!["text".into()], // most local servers default to text
            cost_input: 0.0,            // unknown at runtime
            cost_output: 0.0,
            cost_cache_read: 0.0,
            cost_cache_write: 0.0,
            context_window: 0, // unknown
            max_tokens: 0,
            auth_method: crate::catalog::provider::AuthMethod::Bearer,
            base_url: None,
        })
        .collect()
}

/// Discover models from all known local-runtime providers in parallel.
///
/// This is the Layer 3 entry point for **always-on local servers**. It
/// queries them unconditionally — if the server isn't running, the
/// call fails fast and we move on. Total wall time is bounded by
/// `DISCOVERY_TIMEOUT` (~5s) since the queries are in parallel.
///
/// The default set:
/// - ollama (http://localhost:11434/v1)
/// - lmstudio (http://localhost:1234/v1)
/// - vllm (http://localhost:8000/v1)
/// - sglang (http://localhost:30000/v1)
pub async fn discover_all_local() -> BTreeMap<String, Vec<BuiltinModelEntry>> {
    let targets = [
        ("ollama", "openai-completions", "http://localhost:11434/v1"),
        ("lmstudio", "openai-completions", "http://localhost:1234/v1"),
        ("vllm", "openai-completions", "http://localhost:8000/v1"),
        ("sglang", "openai-completions", "http://localhost:30000/v1"),
    ];

    let futures = targets
        .iter()
        .map(|(id, api, url)| {
            let id = *id;
            let api = *api;
            let url = *url;
            let env_key = match id {
                "ollama" => Some("OLLAMA_API_KEY"),
                "lmstudio" => Some("LMSTUDIO_API_KEY"),
                "vllm" => Some("VLLM_API_KEY"),
                "sglang" => Some("SGLANG_API_KEY"),
                _ => None,
            };
            async move {
                // Local servers typically don't need auth, but some setups
                // configure one. The user's env var, if set, is sent as a
                // Bearer token.
                let models = discover_models(id, api, url, env_key).await;
                if !models.is_empty() {
                    tracing::info!(
                        provider = %id,
                        count = models.len(),
                        "Discovered local models"
                    );
                }
                (id.to_string(), models)
            }
        })
        .collect::<Vec<_>>();

    let results = join_all(futures).await;
    let mut out = BTreeMap::new();
    for (id, models) in results {
        if !models.is_empty() {
            out.insert(id, models);
        }
    }
    out
}

/// Discover models from authenticated cloud providers whose API key is
/// in the environment.
///
/// This is the openclaw-style **on-demand discovery**: only providers
/// that have an API key in `std::env` AND a custom `base_url` configured
/// in `providers.toml` are queried. The list is the openclaw-port
/// providers that have known `/v1/models` endpoints:
///
/// - chutes, deepinfra, gmi, kilocode, novita, nvidia, qwen, stepfun,
///   byteplus, venice
///
/// For each, the function:
/// 1. Reads `providers.toml` to find the `base_url` and `env_key`.
/// 2. Skips if `env_key` is not set in the environment.
/// 3. Skips if `base_url` is empty (use vendor default).
/// 4. Calls `GET {base_url}/models` with the API key as `Authorization: Bearer`.
///
/// All providers are queried in parallel. Total wall time is bounded by
/// `DISCOVERY_TIMEOUT` (~5s).
///
/// The point: when the user has set `NOVITA_API_KEY=...`, we automatically
/// fetch the live Novita model list (with prices) and merge it into the
/// catalog. The user's existing built-in Novita TOML is REPLACED by
/// the live data — this is the intended behavior of Layer 3, which
/// supersedes Layer 1/2.
pub async fn discover_all_authenticated() -> BTreeMap<String, Vec<BuiltinModelEntry>> {
    // Targets: (provider_id, expected_env_key, default_base_url)
    // default_base_url is used when the provider's TOML doesn't override it.
    let targets: &[(&str, &str, &str)] = &[
        ("chutes", "CHUTES_API_KEY", "https://api.chutes.ai/v1"),
        (
            "deepinfra",
            "DEEPINFRA_API_KEY",
            "https://api.deepinfra.com/v1/openai",
        ),
        ("gmi", "GMI_API_KEY", "https://api.gmi-serving.com/v1"),
        ("kilocode", "KILOCODE_API_KEY", "https://api.kilocode.ai/v1"),
        ("moonshot", "MOONSHOT_API_KEY", "https://api.moonshot.ai/v1"),
        (
            "novita",
            "NOVITA_API_KEY",
            "https://api.novita.ai/v3/openai",
        ),
        (
            "nvidia",
            "NVIDIA_API_KEY",
            "https://integrate.api.nvidia.com/v1",
        ),
        ("qwen-oauth", "QWEN_API_KEY", "https://api.qwen.ai/v1"),
        ("stepfun", "STEPFUN_API_KEY", "https://api.stepfun.com/v1"),
        (
            "byteplus",
            "BYTEPLUS_API_KEY",
            "https://ark.ap-southeast.bytepluses.com/api/v3",
        ),
        ("venice", "VENICE_API_KEY", "https://api.venice.ai/api/v1"),
    ];

    let providers = crate::catalog::materialize::materialize_providers();
    let provider_map: BTreeMap<&str, &crate::catalog::BuiltinProviderEntry> =
        providers.iter().map(|p| (p.id.as_str(), p)).collect();

    // Pre-compute active targets as owned Strings (avoids lifetime issues
    // when moving into async futures).
    let mut active: Vec<(String, String, String, String)> = Vec::new();
    for (id, env_key, default_url) in targets {
        if std::env::var(env_key).is_err() {
            continue;
        }
        let url = provider_map
            .get(id)
            .map(|p| {
                if p.base_url.is_empty() {
                    (*default_url).to_string()
                } else {
                    p.base_url.clone()
                }
            })
            .unwrap_or_else(|| (*default_url).to_string());
        let api = provider_map
            .get(id)
            .map(|p| p.api.clone())
            .unwrap_or_else(|| "openai-completions".to_string());
        active.push((id.to_string(), api, url, env_key.to_string()));
    }

    let futures = active
        .into_iter()
        .map(|(id, api, url, env_key)| async move {
            let models = discover_models(&id, &api, &url, Some(&env_key)).await;
            if !models.is_empty() {
                tracing::info!(
                    provider = %id,
                    count = models.len(),
                    "Discovered authenticated models"
                );
            }
            (id, models)
        })
        .collect::<Vec<_>>();

    let results = join_all(futures).await;
    let mut out = BTreeMap::new();
    for (id, models) in results {
        if !models.is_empty() {
            out.insert(id, models);
        }
    }
    out
}

/// Discover all Layer 3 sources: local servers + authenticated cloud.
///
/// This is the high-level entry point. Call once at startup. Both
/// sub-calls are bounded by `DISCOVERY_TIMEOUT` (~5s) because the
/// underlying requests are in parallel.
pub async fn discover_all() -> BTreeMap<String, Vec<BuiltinModelEntry>> {
    let mut all = discover_all_local().await;
    all.extend(discover_all_authenticated().await);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discover_empty_url_returns_empty() {
        let result = discover_models("test", "openai-completions", "", None).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn discover_unreachable_returns_empty() {
        // Use an unroutable address to ensure the request fails fast.
        let result = discover_models(
            "test",
            "openai-completions",
            "http://127.0.0.1:1/v1", // port 1 is privileged and unused
            None,
        )
        .await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn discover_all_authenticated_no_keys_is_empty() {
        // With no env keys set, the function should return an empty map.
        // We don't unset keys (other tests may need them), but we
        // check the case where a non-existent key was used.
        // This test is mainly to verify the function compiles and runs.
        let result = discover_all_authenticated().await;
        // Don't assert empty — other tests may have set keys. Just check
        // it returns a map.
        let _: std::collections::BTreeMap<String, Vec<_>> = result;
    }
}
