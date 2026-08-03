//! Fallback chain for sequential model failover.

#![allow(missing_docs)]

use crate::ProviderEvent;
use crate::context::Context;
use crate::error::ProviderError;
use crate::providers::ProviderRegistry;
use crate::providers::StreamOptions;
use crate::types::Model;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;

/// Stream type alias.
pub type BoxStream = Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>;

/// Ordered fallback chain that tries models sequentially.
#[derive(Debug, Clone, Default)]
pub struct FallbackChain {
    /// Ordered list of `"provider/model-id"` fallback targets.
    pub models: Vec<String>,
}

impl FallbackChain {
    /// Create a new fallback chain.
    pub fn new(models: Vec<String>) -> Self {
        Self { models }
    }

    /// Try each model in sequence using a ProviderRegistry.
    pub async fn try_models(
        &self,
        registry: &Arc<ProviderRegistry>,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<BoxStream, ProviderError> {
        let mut last_err = ProviderError::StreamError("no fallback models configured".to_string());

        for model_str in &self.models {
            let Some((provider_name, model_id)) = Self::parse_model(model_str) else {
                continue;
            };

            let Some(provider) = registry.get(&provider_name) else {
                last_err = ProviderError::UnknownProvider(provider_name.clone());
                continue;
            };

            let model = Self::build_model(&provider_name, &model_id);
            match provider.stream(&model, context, options.clone()).await {
                Ok(stream) => {
                    tracing::info!(model = model_str, "Fallback model succeeded");
                    return Ok(stream);
                }
                Err(e) => {
                    tracing::warn!(model = model_str, error = %e, "Fallback model failed");
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    /// Try each model using a generic resolver closure.
    pub async fn try_models_with_resolver<F>(
        &self,
        resolver: F,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<BoxStream, ProviderError>
    where
        F: Fn(&str) -> Option<Arc<dyn crate::providers::Provider>> + Sync,
    {
        let mut last_err = ProviderError::StreamError("no fallback models configured".to_string());

        for model_str in &self.models {
            let Some((provider_name, model_id)) = Self::parse_model(model_str) else {
                continue;
            };

            let Some(provider) = resolver(&provider_name) else {
                last_err = ProviderError::UnknownProvider(provider_name.clone());
                continue;
            };

            let model = Self::build_model(&provider_name, &model_id);
            match provider.stream(&model, context, options.clone()).await {
                Ok(stream) => {
                    tracing::info!(model = model_str, "Fallback model succeeded");
                    return Ok(stream);
                }
                Err(e) => {
                    tracing::warn!(model = model_str, error = %e, "Fallback model failed");
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    fn parse_model(s: &str) -> Option<(String, String)> {
        let (provider, model_id) = s.split_once('/')?;
        let provider = provider.trim().to_string();
        let model_id = model_id.trim().to_string();
        if provider.is_empty() || model_id.is_empty() {
            return None;
        }
        Some((provider, model_id))
    }

    fn build_model(provider: &str, model_id: &str) -> Model {
        Model::new(
            model_id,
            model_id,
            crate::Api::AnthropicMessages,
            provider,
            "",
        )
    }
}
