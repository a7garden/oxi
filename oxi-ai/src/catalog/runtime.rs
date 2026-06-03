//! Runtime model discovery (Layer 3 of the 3-tier catalog).
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
pub async fn discover_models(
    provider_id: &str,
    api_type: &str,
    base_url: &str,
) -> Vec<BuiltinModelEntry> {
    if base_url.is_empty() {
        return Vec::new();
    }
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let result = client.get(&url).send().await;
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
            reasoning: false,    // unknown at runtime
            input: vec!["text".into()], // most local servers default to text
            cost_input: 0.0,     // local = free
            cost_output: 0.0,
            cost_cache_read: 0.0,
            cost_cache_write: 0.0,
            context_window: 0,   // unknown
            max_tokens: 0,
        })
        .collect()
}

/// Discover models from all known local-runtime providers in parallel.
///
/// This is the Layer 3 entry point. It runs once at startup (or on user
/// command) and merges results into the catalog.
///
/// The default set of providers scanned:
/// - ollama (http://localhost:11434/v1)
/// - lmstudio (http://localhost:1234/v1)
/// - vllm (http://localhost:8000/v1)
/// - sglang (http://localhost:30000/v1)
///
/// Each provider is queried in parallel. Total wall time is bounded by
/// `DISCOVERY_TIMEOUT` (~5s).
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
            async move {
                let models = discover_models(id, api, url).await;
                if !models.is_empty() {
                    tracing::info!(provider = id, count = models.len(),
                        "Discovered local models");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discover_empty_url_returns_empty() {
        let result = discover_models("test", "openai-completions", "").await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn discover_unreachable_returns_empty() {
        // Use an unroutable address to ensure the request fails fast.
        let result = discover_models(
            "test",
            "openai-completions",
            "http://127.0.0.1:1/v1", // port 1 is privileged and unused
        )
        .await;
        assert!(result.is_empty());
    }
}
