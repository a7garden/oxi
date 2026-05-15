//! OpenAI-compatible `/v1/models` endpoint model fetching
//!
//! Queries the `/v1/models` (or `{base_url}/models`) endpoint of any
//! OpenAI-compatible provider and returns the list of available model IDs.
//! Used during startup to auto-register models for custom providers.

use serde::Deserialize;

/// Response shape from the OpenAI `/v1/models` endpoint.
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

/// Individual model entry from the `/v1/models` response.
#[derive(Debug, Deserialize)]
struct ModelInfo {
    id: String,

    owned_by: Option<String>,
}

/// Fetch the model list from an OpenAI-compatible `/v1/models` endpoint
/// using the shared `reqwest::blocking` client.
///
/// `base_url` should be something like `"https://api.minimax.chat/v1"`.
/// The function appends `/models` and issues a `GET` with a `Bearer` token.
///
/// # Errors
///
/// Returns a human-readable error string on failure (network, auth, parse).
pub fn fetch_models_blocking(base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    // Build the URL: trim trailing slashes, then append "/models"
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .map_err(|e| format!("request to {} failed: {}", url, e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("{} returned {}: {}", url, status, body.trim()));
    }

    let parsed: ModelsResponse = response
        .json()
        .map_err(|e| format!("failed to parse models response: {}", e))?;

    Ok(parsed.data.into_iter().map(|m| m.id).collect())
}

/// Fetch the model list from an OpenAI-compatible `/v1/models` endpoint
/// asynchronously.
///
/// This is the async counterpart to [`fetch_models_blocking`].
/// Uses the shared async `reqwest::Client` from the provider module.
///
/// `base_url` should be something like `"https://api.openai.com/v1"`.
/// The function appends `/models` and issues a `GET` with a `Bearer` token.
///
/// # Errors
///
/// Returns a human-readable error string on failure (network, auth, parse).
pub async fn fetch_models_async(base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    use super::shared_client;

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = shared_client();

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("request to {} failed: {}", url, e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("{} returned {}: {}", url, status, body.trim()));
    }

    let parsed: ModelsResponse = response
        .json()
        .await
        .map_err(|e| format!("failed to parse models response: {}", e))?;

    Ok(parsed.data.into_iter().map(|m| m.id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetch_models_blocking_bad_url() {
        // Non-routable address → should fail with a network error
        let result = fetch_models_blocking("http://0.0.0.0:1/v1", "test-key");
        assert!(result.is_err());
    }
}
