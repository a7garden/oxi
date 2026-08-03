//! MCP credential provider (v2.1).
//!
//! Abstracts how an MCP transport obtains the credentials it needs to
//! authenticate against a server (API key, OAuth bearer token, ...).
//! Implementations live outside `oxicode-agent` so that the storage backend
//! (e.g. `oxicode-cli`'s auth store) can be swapped without changing the
//! transport code.
//!
//! The trait is deliberately narrow — two methods, both infallible from
//! the transport's point of view (they return `Option<Credential>` and
//! the transport simply omits the `Authorization` header when no
//! credential is available). It is **not** promoted to a full SDK port
//! (see `docs/designs/2026-06-19-mcp-v2-conformance-transports.md` §D11
//! / §5.2): MCP is an agent feature, not infrastructure, and most
//! products are happy with the noop default plus a per-product
//! implementation injected via [`crate::mcp::McpManager::set_credential_provider`].

use async_trait::async_trait;

/// A single credential materialising an MCP server's authentication.
///
/// The transport treats this as opaque and only consumes [`Self::access_token`]
/// to populate the `Authorization: Bearer …` header. Refresh material
/// (e.g. OAuth refresh token) is handled by [`McpCredentialProvider::refresh`]
/// and is never read by the transport itself.
#[derive(Debug, Clone)]
pub struct Credential {
    /// OAuth bearer token (or equivalent) sent in `Authorization: Bearer`.
    pub access_token: String,
}

/// Source of authentication material for an MCP transport.
///
/// `server` is the configured server name (e.g. `"github"`) and `url`
/// is the MCP endpoint the transport is connecting to. Providers may
/// key storage on either or both. Implementations should be cheap to
/// call — `access_token` is consulted on every connect, `refresh` is
/// called at most once per request on a `401`/`403` response.
#[async_trait]
pub trait McpCredentialProvider: Send + Sync {
    /// Return a credential for `server`/`url` if one is known.
    /// Returning `None` means "no auth" — the transport connects
    /// without an `Authorization` header.
    async fn access_token(&self, server: &str, url: &str) -> Option<Credential>;

    /// Refresh the credential for `server`/`url` and return the new
    /// value. Called by the HTTP transport after a `401`/`403`
    /// response. Returning `None` means refresh failed; the transport
    /// surfaces the original error to the caller.
    async fn refresh(&self, server: &str, url: &str) -> Option<Credential>;
}

/// Default no-op provider. Returns `None` for every lookup, which tells
/// transports to connect without authentication. Used by
/// [`crate::mcp::McpManager`] when no real provider has been injected.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCredentialProvider;

#[async_trait]
impl McpCredentialProvider for NoopCredentialProvider {
    async fn access_token(&self, _server: &str, _url: &str) -> Option<Credential> {
        None
    }

    async fn refresh(&self, _server: &str, _url: &str) -> Option<Credential> {
        None
    }
}
