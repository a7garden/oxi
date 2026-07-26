//! Typed errors for MCP operations.
//!
//! Introduced for the consent gate (F-2, code audit 2026-07-25): new MCP
//! code paths use a typed error enum instead of opaque `anyhow` strings, so
//! callers can match on [`McpError::ConsentDenied`] without downcasting.
//!
//! Scope note: the rest of the `mcp` module still uses `anyhow::Result` for
//! historical reasons (tracked as finding F-7). This module is the typed
//! beachhead — new error-bearing code should extend it rather than reach for
//! `anyhow::anyhow!`.

use thiserror::Error;

/// Errors raised by the MCP manager's consent-aware paths.
#[derive(Debug, Error)]
pub enum McpError {
    /// Server spawn/connect denied because the server is not in the consent
    /// allow-list (its stored [`crate::mcp::ConsentState`] is `Ask` for an
    /// unknown server, or `Deny` after an explicit revocation).
    ///
    /// Actionable: the message names the `oxi mcp trust <server>` command so
    /// the user knows exactly how to proceed.
    #[error(
        "MCP server {server:?} is not trusted — run `oxi mcp trust {server}` to allow, \
         or remove the entry from mcp.json if you did not configure it"
    )]
    ConsentDenied {
        /// The server name as it appears in `mcp.json` / `mcpServers`.
        server: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_denied_display_names_server_and_command() {
        let e = McpError::ConsentDenied {
            server: "github".to_string(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("github"), "message must name the server");
        assert!(
            msg.contains("oxi mcp trust github"),
            "message must surface the exact remediation command, got: {msg}"
        );
    }

    #[test]
    fn consent_denied_is_send_and_sync() {
        // Ensures the error can travel through anyhow::Result across async
        // boundaries (tokio requires Send).
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<McpError>();
    }
}
