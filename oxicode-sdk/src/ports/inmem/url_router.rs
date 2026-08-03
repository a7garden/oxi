//! Composite URL router — a multi-handler [`InternalUrlRouter`] implementation.
//!
//! Products register one [`ProtocolHandler`] per scheme and the router dispatches
//! by extracting the scheme from the URI. A no-handler state returns
//! [`SdkError::UnknownScheme`].
//!
//! Ported from omp `packages/coding-agent/src/internal-urls/router.ts`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[cfg(test)]
use async_trait::async_trait;
use parking_lot::RwLock;

use crate::ports::{InternalUrlRouter, ProtocolHandler, ResolveContext, ResolvedUrl, SdkError};

/// Multi-handler [`InternalUrlRouter`] that dispatches by URI scheme.
///
/// Thread-safe registration with internal [`RwLock`] — handlers can be
/// added or removed at runtime.
pub struct CompositeUrlRouter {
    handlers: RwLock<HashMap<String, Arc<dyn ProtocolHandler>>>,
}

impl CompositeUrlRouter {
    /// Create an empty router.
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a handler for its scheme. Replaces any existing handler for
    /// that scheme.
    pub fn register(&self, handler: Arc<dyn ProtocolHandler>) {
        self.handlers
            .write()
            .insert(handler.scheme().to_lowercase(), handler);
    }

    /// Remove a handler by scheme. Returns `true` if a handler was removed.
    pub fn unregister(&self, scheme: &str) -> bool {
        self.handlers
            .write()
            .remove(&scheme.to_lowercase())
            .is_some()
    }

    /// Check whether any handler is registered for the given scheme.
    pub fn can_resolve(&self, scheme: &str) -> bool {
        self.handlers.read().contains_key(&scheme.to_lowercase())
    }

    /// Parse the scheme from a raw URI. Returns `None` when the input does
    /// not look like an internal URL.
    fn parse_scheme(input: &str) -> Option<&str> {
        // ^([a-z][a-z0-9+.-]*)://
        let bytes = input.as_bytes();
        let len = bytes.len();
        if len < 3 {
            return None;
        }
        // First char must be a lower-case letter.
        if !bytes[0].is_ascii_lowercase() {
            return None;
        }
        // Scan scheme characters.
        for i in 1..len {
            match bytes[i] {
                b':' => {
                    // Must be followed by //
                    if i + 2 < len && bytes[i + 1] == b'/' && bytes[i + 2] == b'/' {
                        let scheme = &input[..i];
                        // http and https are web URLs, not internal.
                        if scheme.eq_ignore_ascii_case("http")
                            || scheme.eq_ignore_ascii_case("https")
                        {
                            return None;
                        }
                        return Some(scheme);
                    }
                    return None;
                }
                b if b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || b == b'+'
                    || b == b'.'
                    || b == b'-' => {}
                _ => return None,
            }
        }
        None
    }

    /// Split an internal URL into `(scheme, path)`. The `://` separator is
    /// consumed.
    fn split_uri(uri: &str) -> Option<(&str, &str)> {
        let scheme = Self::parse_scheme(uri)?;
        let path = &uri[scheme.len() + 3..]; // skip "://"
        Some((scheme, path))
    }
}

impl Default for CompositeUrlRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalUrlRouter for CompositeUrlRouter {
    fn schemes(&self) -> &[&str] {
        &[]
    }

    fn registered_schemes(&self) -> Vec<String> {
        self.handlers.read().keys().cloned().collect()
    }

    fn resolve<'a>(
        &'a self,
        uri: &'a str,
        ctx: &'a ResolveContext,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedUrl, SdkError>> + Send + 'a>> {
        Box::pin(async move {
            let (scheme, path) = Self::split_uri(uri).ok_or_else(|| SdkError::UnknownScheme {
                scheme: uri.to_string(),
            })?;

            let handler = self.handlers.read().get(scheme).cloned().ok_or_else(|| {
                SdkError::UnknownScheme {
                    scheme: scheme.to_string(),
                }
            })?;

            let mut resolved = handler.resolve(path, None, ctx).await?;
            resolved.immutable = resolved.immutable || handler.immutable();

            Ok(resolved)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scheme_valid() {
        assert_eq!(
            CompositeUrlRouter::parse_scheme("issue://1428"),
            Some("issue")
        );
        assert_eq!(
            CompositeUrlRouter::parse_scheme("pr://owner/repo/1428"),
            Some("pr")
        );
        assert_eq!(
            CompositeUrlRouter::parse_scheme("agent://sub1/output"),
            Some("agent")
        );
        assert_eq!(
            CompositeUrlRouter::parse_scheme("skill://my-skill/SKILL.md"),
            Some("skill")
        );
        assert_eq!(
            CompositeUrlRouter::parse_scheme("memory://session-abc"),
            Some("memory")
        );
    }

    #[test]
    fn test_parse_scheme_rejects_regular_paths() {
        assert_eq!(CompositeUrlRouter::parse_scheme("src/main.rs"), None);
        assert_eq!(CompositeUrlRouter::parse_scheme("/absolute/path"), None,);
        assert_eq!(CompositeUrlRouter::parse_scheme("relative/path"), None);
    }

    #[test]
    fn test_parse_scheme_rejects_http() {
        // http:// and https:// are not internal — they're web URLs.
        assert_eq!(
            CompositeUrlRouter::parse_scheme("https://example.com"),
            None,
        );
    }

    #[test]
    fn test_split_uri() {
        let (scheme, path) = CompositeUrlRouter::split_uri("pr://owner/repo/1428/diff/1").unwrap();
        assert_eq!(scheme, "pr");
        assert_eq!(path, "owner/repo/1428/diff/1");
    }

    #[test]
    fn test_can_resolve() {
        let router = CompositeUrlRouter::new();
        assert!(!router.can_resolve("issue"));

        // Register a dummy handler.
        struct DummyHandler;
        #[async_trait]
        impl ProtocolHandler for DummyHandler {
            fn scheme(&self) -> &str {
                "issue"
            }
            async fn resolve(
                &self,
                _: &str,
                _: Option<&str>,
                _: &ResolveContext,
            ) -> Result<ResolvedUrl, SdkError> {
                Ok(ResolvedUrl {
                    url: "issue://1".into(),
                    content: "test".into(),
                    content_type: "text/plain".into(),
                    size: None,
                    source_path: None,
                    notes: vec![],
                    immutable: false,
                })
            }
        }

        router.register(Arc::new(DummyHandler));
        assert!(router.can_resolve("issue"));
        assert!(!router.can_resolve("pr"));
    }

    #[test]
    fn test_unregister() {
        let router = CompositeUrlRouter::new();
        assert!(!router.unregister("issue"));

        struct DummyHandler;
        #[async_trait]
        impl ProtocolHandler for DummyHandler {
            fn scheme(&self) -> &str {
                "issue"
            }
            async fn resolve(
                &self,
                _: &str,
                _: Option<&str>,
                _: &ResolveContext,
            ) -> Result<ResolvedUrl, SdkError> {
                Ok(ResolvedUrl {
                    url: "issue://1".into(),
                    content: "test".into(),
                    content_type: "text/plain".into(),
                    size: None,
                    source_path: None,
                    notes: vec![],
                    immutable: false,
                })
            }
        }

        router.register(Arc::new(DummyHandler));
        assert!(router.unregister("issue"));
        assert!(!router.can_resolve("issue"));
    }
}
