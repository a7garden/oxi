//! Adapter bridging the SDK's `InternalUrlRouter` port to oxicode-agent's
//! `UrlResolver` capability.
//!
//! When wired into `AgentConfig.url_resolver`, this enables the `read`,
//! `grep`, and `find` tools to resolve internal protocol URLs (`issue://`,
//! `pr://`, `memory://`, `skill://`, etc.) through the SDK's port system.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use oxicode_agent::tools::{ResolvedContent, UrlResolver};

use crate::ports::{InternalUrlRouter, ResolveContext};

/// Adapter wrapping the SDK's `InternalUrlRouter` port.
///
/// Constructed by the composition root (oxicode-cli's `App::from_oxicode`) and
/// injected into `AgentConfig.url_resolver` so the agent's `read`/`grep`/
/// `find` tools can dispatch internal URLs.
pub struct SdkUrlResolver {
    router: Arc<dyn InternalUrlRouter>,
}

impl SdkUrlResolver {
    /// Create a new adapter wrapping the SDK's URL router port.
    pub fn new(router: Arc<dyn InternalUrlRouter>) -> Self {
        Self { router }
    }
}

impl std::fmt::Debug for SdkUrlResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkUrlResolver")
            .field("schemes", &self.router.registered_schemes())
            .finish()
    }
}

impl UrlResolver for SdkUrlResolver {
    fn can_resolve(&self, input: &str) -> bool {
        let registered = self.router.registered_schemes();
        registered
            .iter()
            .any(|scheme| input.starts_with(&format!("{}://", scheme)))
    }

    fn resolve<'a>(
        &'a self,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedContent, String>> + Send + 'a>> {
        Box::pin(async move {
            let ctx = ResolveContext::default();
            let resolved = self
                .router
                .resolve(uri, &ctx)
                .await
                .map_err(|e| e.to_string())?;
            Ok(ResolvedContent {
                content: resolved.content,
                content_type: resolved.content_type,
                immutable: resolved.immutable,
            })
        })
    }
}
