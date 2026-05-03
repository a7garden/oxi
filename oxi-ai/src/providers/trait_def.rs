//! Provider trait definition

use crate::error::ProviderError;
use crate::{Context, Model, ProviderEvent, StreamOptions};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

/// LLM provider trait
///
/// Implement this trait to add support for new LLM providers.
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Stream assistant message events
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;

    /// Get the provider name
    fn name(&self) -> &str;
}
