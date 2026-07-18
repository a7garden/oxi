//! File-based `AuthProvider` — single JSON file holding API keys and OAuth
//! tokens per provider.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::SdkError;
use crate::ports::{AuthProvider, OAuthToken};

/// On-disk schema for `auth.json`.
///
/// ```json
/// {
///   "version": 1,
///   "providers": {
///     "anthropic": { "api_key": "sk-ant-..." },
///     "openai":    { "api_key": "sk-..." },
///     "google":    { "oauth": { "access_token": "...", "refresh_token": "..." } }
///   }
/// }
/// ```
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct AuthFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    providers: HashMap<String, ProviderEntry>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ProviderEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth: Option<OAuthToken>,
}

impl AuthFile {
    fn load(path: &std::path::Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).expect("serializable");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// File-based `AuthProvider`.
///
/// Concurrency: a single `Mutex` guards the in-memory cache. Read-heavy
/// workloads should consider `Arc<AuthProvider>` + cloning; writes are
/// rare (interactive login) so a coarse lock is fine.
pub struct FileAuthProvider {
    path: PathBuf,
    state: Mutex<AuthFile>,
}

impl std::fmt::Debug for FileAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileAuthProvider")
            .field("path", &self.path)
            .finish()
    }
}

impl FileAuthProvider {
    /// Create a new provider that reads/writes `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let state = AuthFile::load(&path);
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    /// Resolve an API key with this priority:
    /// 1. The given in-memory state (loaded from `path`)
    /// 2. The `OXI_API_KEY_<UPPER>` env var
    /// 3. The provider's standard env var (e.g. `ANTHROPIC_API_KEY`)
    fn resolve_with(state: &AuthFile, provider: &str) -> Option<String> {
        if let Some(k) = state
            .providers
            .get(provider)
            .and_then(|e| e.api_key.clone())
        {
            return Some(k);
        }
        Self::env_fallback(provider)
    }

    /// Environment-variable fallback used after both the in-memory cache
    /// and (for the sync path) a fresh disk read miss.
    fn env_fallback(provider: &str) -> Option<String> {
        let upper = provider.to_uppercase();
        if let Ok(k) = std::env::var(format!("OXI_API_KEY_{upper}"))
            && !k.is_empty()
        {
            return Some(k);
        }
        // Standard provider env vars (best-effort, optional).
        let conventional = match provider {
            "anthropic" => "ANTHROPIC_API_KEY",
            "openai" => "OPENAI_API_KEY",
            "google" | "gemini" => "GOOGLE_API_KEY",
            "deepseek" => "DEEPSEEK_API_KEY",
            _ => return None,
        };
        std::env::var(conventional).ok().filter(|s| !s.is_empty())
    }

    /// Resolve an API key against the in-memory boot-time cache.
    ///
    /// Use [`get_api_key_sync`](AuthProvider::get_api_key_sync) for the
    /// dynamic path that re-reads `path` from disk — that's the one
    /// `Oxi::create_provider` consults, so it picks up writes from
    /// external singletons (e.g. the CLI's `shared_auth_storage()`).
    pub fn resolve_api_key(&self, provider: &str) -> Option<String> {
        let state = self.state.lock();
        Self::resolve_with(&state, provider)
    }
}

impl AuthProvider for FileAuthProvider {
    fn get_api_key(
        &self,
        provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, SdkError>> + Send + '_>> {
        let result = self.resolve_api_key(provider);
        Box::pin(async move { Ok(result) })
    }

    /// Sync fast-path — **re-reads `path` from disk** on every call so
    /// external writers (e.g. the CLI's `shared_auth_storage()` singleton
    /// at oxi-cli/src/store/auth_storage.rs:1135, which maintains its own
    /// independent in-memory cache) are reflected without restart. This is
    /// the credential source `Oxi::create_provider` consults at build /
    /// `switch_model` / `refresh_credentials` time; per-call file I/O is
    /// negligible on those paths. Issue #40.
    fn get_api_key_sync(&self, provider: &str) -> Result<Option<String>, SdkError> {
        let fresh = AuthFile::load(&self.path);
        Ok(Self::resolve_with(&fresh, provider))
    }

    fn set_api_key(
        &self,
        provider: &str,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        let mut s = self.state.lock();
        s.providers.entry(provider.to_string()).or_default().api_key = Some(key.to_string());
        let result = s.save(&self.path).map_err(|e| SdkError::Internal(e.into()));
        Box::pin(async { result })
    }

    fn delete_api_key(
        &self,
        provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        let mut s = self.state.lock();
        if let Some(entry) = s.providers.get_mut(provider) {
            entry.api_key = None;
            if entry.oauth.is_none() {
                s.providers.remove(provider);
            }
        }
        let result = s.save(&self.path).map_err(|e| SdkError::Internal(e.into()));
        Box::pin(async { result })
    }

    fn get_oauth(
        &self,
        provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<OAuthToken>, SdkError>> + Send + '_>> {
        let s = self.state.lock();
        let result = s.providers.get(provider).and_then(|e| e.oauth.clone());
        Box::pin(async move { Ok(result) })
    }

    fn set_oauth(
        &self,
        provider: &str,
        token: OAuthToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        let mut s = self.state.lock();
        s.providers.entry(provider.to_string()).or_default().oauth = Some(token);
        let result = s.save(&self.path).map_err(|e| SdkError::Internal(e.into()));
        Box::pin(async { result })
    }

    fn list_providers(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SdkError>> + Send + '_>> {
        let s = self.state.lock();
        let result = s.providers.keys().cloned().collect();
        Box::pin(async move { Ok(result) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn set_then_get_api_key() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("auth.json");
        let auth = FileAuthProvider::new(&p);
        auth.set_api_key("anthropic", "sk-ant-test").await.unwrap();
        let got = auth.get_api_key("anthropic").await.unwrap();
        assert_eq!(got.as_deref(), Some("sk-ant-test"));
        assert!(p.exists());
    }

    #[tokio::test]
    async fn delete_api_key_removes_entry() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("auth.json");
        let auth = FileAuthProvider::new(&p);
        auth.set_api_key("openai", "k").await.unwrap();
        auth.delete_api_key("openai").await.unwrap();
        assert!(auth.get_api_key("openai").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oauth_round_trip() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("auth.json");
        let auth = FileAuthProvider::new(&p);
        let token = OAuthToken::bearer("ya29.test");
        auth.set_oauth("google", token.clone()).await.unwrap();
        let got = auth.get_oauth("google").await.unwrap().unwrap();
        assert_eq!(got.access_token, "ya29.test");
    }

    #[tokio::test]
    async fn env_var_fallback_when_no_file_entry() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("auth.json");
        let auth = FileAuthProvider::new(&p);
        // No file entry. Check that resolve_api_key returns None for an
        // unrecognised provider.
        assert!(auth.resolve_api_key("nonexistent-xyz").is_none());
    }

    /// Regression (#40 advisor follow-up): `get_api_key_sync` MUST re-read
    /// `path` from disk on every call. Without this, an external writer
    /// updating `auth.json` (e.g. the CLI's `shared_auth_storage()`
    /// singleton, which owns a separate in-memory cache) would not be
    /// visible to `Oxi::create_provider`, silently breaking mid-session
    /// credential refresh from the TUI provider overlay.
    #[test]
    fn get_api_key_sync_re_reads_disk_after_external_write() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("auth.json");
        let auth = FileAuthProvider::new(&p);

        // Initially empty.
        assert_eq!(
            auth.get_api_key_sync("anthropic").unwrap().as_deref(),
            None,
            "fresh provider should have no key"
        );

        // External write — simulate `shared_auth_storage().set_api_key(...)`
        std::fs::write(
            &p,
            r#"{"version":1,"providers":{"anthropic":{"api_key":"sk-external"}}}"#,
        )
        .unwrap();

        // Sync fast-path must observe the new key WITHOUT re-instantiating
        // the FileAuthProvider — proves it re-reads `path` from disk.
        assert_eq!(
            auth.get_api_key_sync("anthropic").unwrap().as_deref(),
            Some("sk-external"),
            "get_api_key_sync must re-read path on every call; \
             FileAuthProvider's own in-memory cache is still empty here"
        );

        // Sanity: the cache-only `resolve_api_key` does NOT observe the
        // external write — proving the test is exercising the new path
        // (not just the existing cache).
        assert_eq!(
            auth.resolve_api_key("anthropic").as_deref(),
            None,
            "cache-only resolve_api_key must stay stale; \
             this assertion locks the dual-cache contract"
        );
    }
}
