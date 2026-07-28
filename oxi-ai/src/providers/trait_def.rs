//! Provider trait definition

use crate::error::ProviderError;
use crate::{Context, Model, ProviderEvent, StreamOptions};
use futures::Stream;
use std::future::Future;
use std::pin::Pin;

/// Stream result type alias
pub type StreamResult = Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;

/// LLM provider trait
///
/// Implement this trait to add support for new LLM providers.
pub trait Provider: Send + Sync + 'static {
    /// Stream assistant message events
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>>;
}
