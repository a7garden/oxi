//! File-backed MCP credential provider (v2.2).
//!
//! Implements [`McpCredentialProvider`] using OAuth2 client_credentials.
//! For each server with an [`OAuthConfig`] in `mcp.json`, the provider
//! POSTs to `token_url` to exchange `client_id`/`client_secret` for
//! an access token, caches it in `~/.config/oxi/mcp-tokens.json`, and
//! returns it on [`McpCredentialProvider::access_token`]. Tokens are
//! refreshed when missing or expired (based on `expires_in`).
//!
//! Browser-based authorization-code flow (with a local callback server)
//! is **not** implemented in v2.2 — clients without a public `client_id`
//! / `client_secret` (e.g. MCP servers that require user-interactive
//! consent) cannot be auto-authenticated. Use the server's own hosted
//! login and configure `oauth` to a machine-client for now.
//!
//! Trigger a manual refresh via `/mcp reauth <server>` in the TUI.

use anyhow::{Context, Result};
use oxi_agent::mcp::auth::{Credential, McpCredentialProvider};
use oxi_agent::mcp::types::OAuthConfig;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TOKEN_FILE: &str = "mcp-tokens.json";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoredToken {
    access_token: String,
    /// Unix seconds at which the token is known to be expired.
    /// `None` means unknown / never expires (rely on refresh errors).
    expires_at: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TokenStore {
    #[serde(default)]
    tokens: HashMap<String, StoredToken>,
}

/// File-backed credential provider. Constructed once in the bootstrap
/// and shared with [`McpManager`] via `Arc<dyn McpCredentialProvider>`.
pub struct FileMcpCredentialProvider {
    /// Per-server OAuth client config (from `mcp.json`).
    oauth: HashMap<String, OAuthConfig>,
    /// Cached tokens, loaded from disk on construction and saved
    /// after every successful refresh (atomic temp + rename).
    store: RwLock<TokenStore>,
    store_path: PathBuf,
    http: reqwest::Client,
}

impl FileMcpCredentialProvider {
    /// Create a new file-backed credential provider.
    ///
    /// Loads any cached tokens from `config_dir` (or starts with an empty
    /// store) and configures a 15-second-timeout HTTP client for refreshes.
    pub fn new(oauth: HashMap<String, OAuthConfig>, config_dir: PathBuf) -> Result<Arc<Self>> {
        let store_path = config_dir.join(TOKEN_FILE);
        let store = if store_path.exists() {
            match std::fs::read_to_string(&store_path) {
                Ok(s) => serde_json::from_str::<TokenStore>(&s).unwrap_or_default(),
                Err(_) => TokenStore::default(),
            }
        } else {
            TokenStore::default()
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .context("build reqwest client for MCP credential provider")?;
        Ok(Arc::new(Self {
            oauth,
            store: RwLock::new(store),
            store_path,
            http,
        }))
    }

    /// Force a refresh for `server` and persist the new token. Used by
    /// `/mcp reauth <server>`.
    pub async fn force_refresh(&self, server: &str) -> Result<()> {
        let new = self.do_refresh(server).await?;
        self.store_token(server, &new);
        Ok(())
    }

    async fn do_refresh(&self, server: &str) -> Result<StoredToken> {
        let cfg = self.oauth.get(server).cloned().ok_or_else(|| {
            anyhow::anyhow!("Server '{}' has no OAuth config in mcp.json", server)
        })?;
        let mut form = vec![
            ("grant_type", "client_credentials".to_string()),
            ("client_id", cfg.client_id),
            ("client_secret", cfg.client_secret),
        ];
        if let Some(scope) = cfg.scope.as_deref() {
            form.push(("scope", scope.to_string()));
        }
        let resp = self
            .http
            .post(&cfg.token_url)
            .header("Accept", "application/json")
            .form(&form)
            .send()
            .await
            .with_context(|| format!("OAuth token request to {} failed", cfg.token_url))?;
        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .context("OAuth token response was not JSON")?;
        if !status.is_success() {
            anyhow::bail!(
                "OAuth token endpoint returned {}: {}",
                status.as_u16(),
                body
            );
        }
        let access_token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("OAuth response missing access_token"))?
            .to_string();
        let expires_at = body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .and_then(|secs| now_secs().checked_add(secs));
        Ok(StoredToken {
            access_token,
            expires_at,
        })
    }

    fn store_token(&self, server: &str, token: &StoredToken) {
        {
            let mut s = self.store.write();
            s.tokens.insert(server.to_string(), token.clone());
        }
        // Best-effort atomic write.
        let snapshot = self.store.read().clone();
        if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
            if let Some(parent) = self.store_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = self.store_path.with_extension("json.tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.store_path);
            }
        }
    }

    fn token_is_fresh(&self, server: &str) -> Option<Credential> {
        let s = self.store.read();
        let t = s.tokens.get(server)?;
        if let Some(exp) = t.expires_at {
            // Treat tokens within 30s of expiry as stale so a request
            // doesn't use a token that will expire mid-flight.
            if now_secs() + 30 >= exp {
                return None;
            }
        }
        Some(Credential {
            access_token: t.access_token.clone(),
        })
    }
}

#[async_trait::async_trait]
impl McpCredentialProvider for FileMcpCredentialProvider {
    async fn access_token(&self, server: &str, _url: &str) -> Option<Credential> {
        if let Some(c) = self.token_is_fresh(server) {
            return Some(c);
        }
        // Try to refresh; if refresh fails, return None (no auth).
        match self.do_refresh(server).await {
            Ok(token) => {
                self.store_token(server, &token);
                Some(Credential {
                    access_token: token.access_token,
                })
            }
            Err(e) => {
                tracing::warn!("MCP credential refresh for '{}' failed: {}", server, e);
                None
            }
        }
    }

    async fn refresh(&self, server: &str, _url: &str) -> Option<Credential> {
        match self.do_refresh(server).await {
            Ok(token) => {
                self.store_token(server, &token);
                Some(Credential {
                    access_token: token.access_token,
                })
            }
            Err(e) => {
                tracing::warn!("MCP credential refresh for '{}' failed: {}", server, e);
                None
            }
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
