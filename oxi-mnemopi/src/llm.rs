//! LLM backend — ported from omp `packages/mnemopi/src/core/{llm-backends,local-llm}.ts`.
//!
//! Provides a synchronous [`LlmBackend`] trait plus an OpenAI-compatible
//! remote implementation (behind the `remote-llm` feature, reusing the
//! existing `reqwest::blocking::Client` pattern from [`crate::embeddings`]).
//!
//! ## Lifetime and blocking
//!
//! Implementations are expected to be blocking; callers use `spawn_blocking`
//! from async contexts, mirroring [`crate::embeddings::EmbeddingProvider`].
//!
//! ## Why synchronous
//!
//! `oxi-mnemopi` is a library: the host (typically `oxi-cli`) owns the
//! async runtime and decides when to block. A sync API keeps the surface
//! small and matches the rest of the crate's public I/O shape
//! (`remember`, `recall`, `sleep`).

use std::time::Duration;

use crate::error::{MnemopiError, Result};

/// Options for a single LLM completion call.
#[derive(Debug, Clone, Default)]
pub struct CompleteOptions {
    /// Maximum tokens the model may emit. `None` lets the backend choose
    /// its default.
    pub max_tokens: Option<u32>,
    /// Sampling temperature in `[0.0, 2.0]`. `None` = backend default.
    pub temperature: Option<f32>,
    /// Per-call timeout. `None` = backend default (typically 30 s).
    ///
    /// Honored via `reqwest::blocking::RequestBuilder::timeout` in
    /// [`RemoteLlmBackend`]; client-level timeouts act only as a ceiling.
    pub timeout: Option<Duration>,
}

/// Synchronous LLM backend — produces a single completion for a prompt.
///
/// The contract is intentionally minimal: implementations may use any
/// transport (remote HTTP API, local GGUF via external process, host
/// callback) provided they block until the result is ready.
///
/// Implementations should be cheap to clone (`Arc`-internal) so callers
/// can stash them in long-lived configs.
pub trait LlmBackend: Send + Sync {
    /// Produce a completion for `prompt`.
    ///
    /// Returns the raw text response. Implementations are encouraged to
    /// strip leading/trailing whitespace and any common code-fence
    /// wrappers before returning, so downstream parsers can be lenient.
    ///
    /// Per-call [`CompleteOptions::timeout`] is honored — client-level
    /// timeouts act only as a ceiling.
    fn complete(&self, prompt: &str, options: &CompleteOptions) -> Result<String>;

    /// Human-readable backend identifier (model name + transport).
    /// Used for diagnostics and metadata.
    fn backend_name(&self) -> &str;
}

/// No-op backend: `complete` always returns [`MnemopiError::Llm`] with a
/// clear message.
///
/// Equivalent to [`crate::embeddings::NoopEmbeddingProvider`]: makes the
/// "LLM disabled" state explicit and avoids `Option<Arc<dyn ...>>`
/// branching at every call site.
#[derive(Debug, Clone, Default)]
pub struct NoopLlmBackend;

impl LlmBackend for NoopLlmBackend {
    fn complete(&self, _prompt: &str, _options: &CompleteOptions) -> Result<String> {
        Err(MnemopiError::Llm(
            "no LLM backend configured (set MnemopiConfig.llm_backend)".into(),
        ))
    }

    fn backend_name(&self) -> &str {
        "noop"
    }
}

// ── Remote (OpenAI-compatible /v1/chat/completions) ───────────────────────

/// OpenAI-compatible remote LLM backend.
///
/// Calls `POST {base_url}/v1/chat/completions` with the configured API
/// key. Works with OpenAI, Azure OpenAI, OpenRouter, local servers
/// (ollama, vLLM, llama.cpp's `--server` mode), etc.
///
/// Behind the `remote-llm` feature flag to keep the default binary small
/// (mirrors `remote-embeddings`).
#[cfg(feature = "remote-llm")]
pub struct RemoteLlmBackend {
    base_url: String,
    api_key: String,
    model: String,
    default_max_tokens: u32,
    default_temperature: f32,
    default_timeout: Duration,
    client: reqwest::blocking::Client,
}

#[cfg(feature = "remote-llm")]
impl std::fmt::Debug for RemoteLlmBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteLlmBackend")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("default_max_tokens", &self.default_max_tokens)
            .field("default_temperature", &self.default_temperature)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "remote-llm")]
impl RemoteLlmBackend {
    /// Create a new remote LLM backend.
    ///
    /// - `base_url`: e.g. `https://api.openai.com` (no trailing slash).
    /// - `api_key`: Bearer token for the chat-completions endpoint.
    /// - `model`: e.g. `gpt-4.1-mini`, `claude-3-5-sonnet` (on Anthropic-
    ///   compatible proxies), `qwen2.5:3b` (on ollama), etc.
    ///
    /// Defaults: `max_tokens=2048`, `temperature=0.0`, `timeout=30s`. Use
    /// the `with_*` builders to override.
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            default_max_tokens: 2048,
            default_temperature: 0.0,
            default_timeout: Duration::from_secs(30),
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::blocking::Client::new()),
        }
    }

    /// Override the default `max_tokens`.
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.default_max_tokens = tokens;
        self
    }

    /// Override the default sampling temperature (clamped to `[0.0, 2.0]`).
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.default_temperature = temp.clamp(0.0, 2.0);
        self
    }

    /// Override the default per-call timeout. Also rebuilds the inner
    /// `reqwest` client so connection-level timeouts match.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self.client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        self
    }
}

#[cfg(feature = "remote-llm")]
impl LlmBackend for RemoteLlmBackend {
    fn complete(&self, prompt: &str, options: &CompleteOptions) -> Result<String> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let max_tokens = options.max_tokens.unwrap_or(self.default_max_tokens);
        let temperature = options.temperature.unwrap_or(self.default_temperature);
        // Per-call timeout overrides the client-level default. reqwest's
        // `RequestBuilder::timeout` is the documented mechanism; the
        // client-level timeout remains as a ceiling for safety.
        let per_call_timeout = options.timeout.unwrap_or(self.default_timeout);

        // OpenAI / OpenRouter / Azure / ollama / vLLM all speak this shape.
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "user", "content": prompt }
            ],
            "max_tokens": max_tokens,
            "temperature": temperature,
            "stream": false,
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(per_call_timeout)
            .send()
            .map_err(|e| MnemopiError::Llm(format!("HTTP request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(MnemopiError::Llm(format!("API returned {status}: {text}")));
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| MnemopiError::Llm(format!("JSON parse failed: {e}")))?;

        // Extract `choices[0].message.content`. Some servers emit
        // `choices[0].text` instead (legacy completions API); fall back.
        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|first| {
                first
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|t| t.as_str())
                    .or_else(|| first.get("text").and_then(|t| t.as_str()))
            })
            .ok_or_else(|| {
                MnemopiError::Llm("response missing choices[0].message.content".into())
            })?;

        Ok(content.trim().to_string())
    }

    fn backend_name(&self) -> &str {
        &self.model
    }
}

/// A stub backend that returns a fixed string. Useful in downstream
/// tests of fact extraction / consolidation that need an LLM but
/// should not hit the network.
#[derive(Debug, Clone)]
pub struct StubLlmBackend {
    /// The string returned by every `complete` call.
    pub response: String,
    /// Value returned by `backend_name`.
    pub name: String,
}

impl LlmBackend for StubLlmBackend {
    fn complete(&self, _prompt: &str, _options: &CompleteOptions) -> Result<String> {
        Ok(self.response.clone())
    }

    fn backend_name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_backend_returns_llm_error() {
        let backend = NoopLlmBackend;
        let err = backend
            .complete("hi", &CompleteOptions::default())
            .unwrap_err();
        assert!(matches!(err, MnemopiError::Llm(_)));
        assert_eq!(backend.backend_name(), "noop");
    }

    #[test]
    fn complete_options_default_is_empty() {
        let opts = CompleteOptions::default();
        assert!(opts.max_tokens.is_none());
        assert!(opts.temperature.is_none());
        assert!(opts.timeout.is_none());
    }

    #[cfg(feature = "remote-llm")]
    #[test]
    fn remote_backend_constructs_with_expected_defaults() {
        let backend = RemoteLlmBackend::new("https://api.openai.com", "sk-test", "gpt-4.1-mini");
        assert_eq!(backend.backend_name(), "gpt-4.1-mini");
        assert_eq!(backend.default_max_tokens, 2048);
        assert_eq!(backend.default_temperature, 0.0);
    }

    #[cfg(feature = "remote-llm")]
    #[test]
    fn remote_backend_with_temperature_clamps() {
        let backend = RemoteLlmBackend::new("https://x", "k", "m").with_temperature(5.0);
        // 5.0 clamps to 2.0.
        assert_eq!(backend.default_temperature, 2.0);
    }

    #[cfg(feature = "remote-llm")]
    #[test]
    fn remote_backend_with_max_tokens_applies() {
        let backend = RemoteLlmBackend::new("https://x", "k", "m").with_max_tokens(512);
        assert_eq!(backend.default_max_tokens, 512);
    }

    #[test]
    fn stub_backend_returns_response() {
        let backend = StubLlmBackend {
            response: "fixed output".into(),
            name: "stub".into(),
        };
        let out = backend
            .complete("anything", &CompleteOptions::default())
            .unwrap();
        assert_eq!(out, "fixed output");
        assert_eq!(backend.backend_name(), "stub");
    }
}
