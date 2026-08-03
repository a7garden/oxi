//! Embedding providers — ported from omp `core/embeddings.ts`.
//!
//! Defines a synchronous [`EmbeddingProvider`] trait. Implementations:
//! - [`NoopEmbeddingProvider`] — always unavailable (default).
//! - `RemoteEmbeddingProvider` — OpenAI-compatible `/v1/embeddings` API
//!   (behind `remote-embeddings` feature).
//! - `LocalEmbeddingProvider` — local ONNX via fastembed-rs
//!   (behind `local-embeddings` feature).
//!
//! Callers wrap `embed()` in `spawn_blocking` from async contexts.

use crate::error::{MnemopiError, Result};
use crate::vector_math::cosine_similarity;

/// Synchronous embedding provider trait.
///
/// Mirrors omp's `EmbeddingProvider` interface. Implementations are
/// expected to be blocking; callers use `spawn_blocking` from async.
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a batch of texts, returning one vector per input.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Whether this provider is available (has valid config / model loaded).
    fn available(&self) -> bool {
        true
    }

    /// Model name for cache keying and storage metadata.
    fn model_name(&self) -> &str {
        "unknown"
    }

    /// Embedding dimensionality (0 = unknown until first embed).
    fn dim(&self) -> usize {
        0
    }
}

// ── Noop provider ────────────────────────────────────────────────────────

/// No-op embedding provider — always unavailable.
///
/// Used when embeddings are disabled. All operations return empty vectors.
#[derive(Debug, Clone, Default)]
pub struct NoopEmbeddingProvider;

impl EmbeddingProvider for NoopEmbeddingProvider {
    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(Vec::new())
    }

    fn available(&self) -> bool {
        false
    }
}

// ── Remote API provider ──────────────────────────────────────────────────

/// OpenAI-compatible remote embedding provider.
///
/// Calls `POST {base_url}/v1/embeddings` with the configured API key.
/// Works with OpenAI, Azure OpenAI, local servers (ollama, vLLM), etc.
#[cfg(feature = "remote-embeddings")]
pub struct RemoteEmbeddingProvider {
    base_url: String,
    api_key: String,
    model: String,
    dim_cache: parking_lot::Mutex<usize>,
    client: reqwest::blocking::Client,
}

#[cfg(feature = "remote-embeddings")]
impl std::fmt::Debug for RemoteEmbeddingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteEmbeddingProvider")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "remote-embeddings")]
impl RemoteEmbeddingProvider {
    /// Create a new remote embedding provider.
    ///
    /// - `base_url`: e.g. `https://api.openai.com` (no trailing slash).
    /// - `api_key`: Bearer token for the embeddings endpoint.
    /// - `model`: e.g. `text-embedding-3-small`.
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            dim_cache: parking_lot::Mutex::new(0),
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::blocking::Client::new()),
        }
    }
}

#[cfg(feature = "remote-embeddings")]
impl EmbeddingProvider for RemoteEmbeddingProvider {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/v1/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .map_err(|e| MnemopiError::Embedding(format!("HTTP request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(MnemopiError::Embedding(format!(
                "API returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| MnemopiError::Embedding(format!("JSON parse failed: {e}")))?;

        let data = json
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| MnemopiError::Embedding("missing 'data' field".into()))?;

        let mut result = Vec::with_capacity(data.len());
        for entry in data {
            let embedding = entry
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| MnemopiError::Embedding("missing 'embedding' field".into()))?;

            let vec: Vec<f32> = embedding
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            result.push(vec);
        }

        // Cache dimensionality
        if let Some(first) = result.first() {
            let mut dim = self.dim_cache.lock();
            *dim = first.len();
        }

        Ok(result)
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> usize {
        *self.dim_cache.lock()
    }
}

// ── Local ONNX provider (fastembed) ──────────────────────────────────────

/// Local embedding provider using fastembed-rs (ONNX Runtime).
///
/// Downloads model weights on first use from HuggingFace Hub.
/// Adds ~50MB to binary size.
#[cfg(feature = "local-embeddings")]
pub struct LocalEmbeddingProvider {
    model: parking_lot::Mutex<Option<fastembed::TextEmbedding>>,
    model_choice: fastembed::EmbeddingModel,
    model_name: String,
    dim: usize,
}

#[cfg(feature = "local-embeddings")]
impl std::fmt::Debug for LocalEmbeddingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEmbeddingProvider")
            .field("model_name", &self.model_name)
            .field("dim", &self.dim)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "local-embeddings")]
impl LocalEmbeddingProvider {
    /// Create a local embedding provider with the default model
    /// (`BAAI/bge-small-en-v1.5`, 384-dim).
    pub fn new() -> Result<Self> {
        Self::with_model(fastembed::EmbeddingModel::BGESmallENV15)
    }

    /// Create with a specific fastembed model.
    pub fn with_model(model: fastembed::EmbeddingModel) -> Result<Self> {
        let dim = match model {
            fastembed::EmbeddingModel::BGESmallENV15 => 384,
            fastembed::EmbeddingModel::AllMiniLML6V2 => 384,
            fastembed::EmbeddingModel::AllMiniLML12V2 => 384,
            fastembed::EmbeddingModel::BGEBaseENV15 => 768,
            fastembed::EmbeddingModel::BGELargeENV15 => 1024,
            fastembed::EmbeddingModel::NomicEmbedTextV1 => 768,
            _ => 384, // fallback
        };
        let name = format!("{model:?}");

        Ok(Self {
            model: parking_lot::Mutex::new(None), // lazy init
            model_choice: model,
            model_name: name,
            dim,
        })
    }

    fn ensure_model(&self) -> Result<()> {
        let mut guard = self.model.lock();
        if guard.is_none() {
            let init = fastembed::InitOptions::new(self.model_choice.clone());
            let model = fastembed::TextEmbedding::try_new(init)
                .map_err(|e| MnemopiError::Embedding(format!("fastembed init: {e}")))?;
            *guard = Some(model);
        }
        Ok(())
    }
}

#[cfg(feature = "local-embeddings")]
impl Default for LocalEmbeddingProvider {
    fn default() -> Self {
        Self::new().unwrap_or(Self {
            model: parking_lot::Mutex::new(None),
            model_choice: fastembed::EmbeddingModel::BGESmallENV15,
            model_name: "unavailable".to_string(),
            dim: 0,
        })
    }
}

#[cfg(feature = "local-embeddings")]
impl EmbeddingProvider for LocalEmbeddingProvider {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_model()?;

        let guard = self.model.lock();
        let model = guard
            .as_ref()
            .ok_or_else(|| MnemopiError::Embedding("model not loaded".into()))?;

        let embeddings = model
            .embed(texts.iter().map(|s| s.as_str()).collect::<Vec<_>>(), None)
            .map_err(|e| MnemopiError::Embedding(format!("fastembed embed: {e}")))?;

        Ok(embeddings.into_iter().map(|e| e.to_vec()).collect())
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Embed a single query string, returning its vector.
///
/// Convenience wrapper around [`EmbeddingProvider::embed`].
pub fn embed_query(provider: &dyn EmbeddingProvider, text: &str) -> Result<Vec<f32>> {
    if !provider.available() {
        return Err(MnemopiError::Embedding("provider not available".into()));
    }
    let mut result = provider.embed(&[text.to_string()])?;
    result
        .pop()
        .ok_or_else(|| MnemopiError::Embedding("empty embedding result".into()))
}

/// Compute similarity between a query vector and a stored vector.
pub fn similarity(query: &[f32], stored: &[f32]) -> f32 {
    cosine_similarity(query, stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_provider_unavailable() {
        let provider = NoopEmbeddingProvider;
        assert!(!provider.available());
        let result = provider.embed(&["test".to_string()]).unwrap();
        assert!(result.is_empty());
    }

    #[cfg(feature = "remote-embeddings")]
    #[test]
    fn remote_provider_creation() {
        let provider = RemoteEmbeddingProvider::new(
            "https://api.openai.com",
            "test-key",
            "text-embedding-3-small",
        );
        assert_eq!(provider.model_name(), "text-embedding-3-small");
    }
}
