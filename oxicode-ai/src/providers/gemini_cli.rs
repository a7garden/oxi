//! Google Gemini CLI transport — explicit typed unsupported-provider path.
//!
//! `Api::GoogleGeminiCli` is a first-class dialect in `oxicode_catalog::api::Api`
//! and appears in the model catalog for the `google-gemini-cli` provider.
//! However, the Gemini CLI is a remote-AGENT protocol without a dedicated
//! transport (no `proto/`, no bundled SDK), and the upstream openclaw port
//! collapses it to `google-generative-ai` (`scripts/catalog/port-openclaw.py:48`).
//!
//! Before this module existed, the `build_builtin_transport` factory arm
//! for `Api::GoogleGeminiCli` fell through to `_ => None`, which silently
//! surfaced as `ProviderError::UnknownProvider("google-gemini-cli")` downstream.
//!
//! This transport keeps the factory dispatch non-`None` (so resolution does
//! not silently miss) while surfacing a typed `ProviderError::NotImplemented`
//! error at the moment the caller actually invokes `stream()`. No fake
//! success, no fabricated HTTP traffic.
//!
//! When a real Gemini CLI protocol is integrated (likely a local
//! subprocess / OAuth helper), replace the body of `Provider::stream` with
//! the real implementation — the rest of the dispatch path stays unchanged.

use std::future::Future;
use std::pin::Pin;

use crate::error::ProviderError;
use crate::{Context, Model, Provider, StreamOptions, StreamResult};

/// Thin transport for the `google-gemini-cli` dialect.
///
/// Carries no identity — the canonical catalog id is the registry key (see
/// `oxicode_ai::providers::register_builtins::create_builtin_provider`).
#[derive(Clone, Default)]
pub struct GeminiCliProvider;

impl GeminiCliProvider {
    /// Construct a new `GeminiCliProvider`.
    pub fn new() -> Self {
        Self
    }
}

impl Provider for GeminiCliProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a Model,
        _context: &'a Context,
        _options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            Err(ProviderError::NotImplemented(
                "google-gemini-cli: no dedicated transport in oxicode-ai; \
                 upstream collapses to google-generative-ai. \
                 Use the `google` provider instead."
                    .into(),
            ))
        })
    }
}
