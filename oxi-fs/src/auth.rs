//! File-based `AuthProvider` — single JSON file holding API keys and OAuth
//! tokens per provider.

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use oxi_sdk::ports::{AuthProvider, OAuthToken};
use oxi_sdk::SdkError;

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
        f.debug_struct("FileAuthProvider").field("path", &self.path).finish()
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
    /// 1. The in-memory `state` (loaded from `path`)
    /// 2. The `OXI_API_KEY_<UPPER>` env var
    /// 3. The provider's standard env var (e.g. `ANTHROPIC_API_KEY`)
    pub fn resolve_api_key(&self, provider: &str) -> Option<String> {
        if let Some(k) = self.state.lock().providers.get(provider).and_then(|e| e.api_key.clone()) {
            return Some(k);
        }
        let upper = provider.to_uppercase();
        if let Ok(k) = std::env::var(format!("OXI_API_KEY_{upper}")) {
            if !k.is_empty() {
                return Some(k);
            }
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
}

#[async_trait]
impl AuthProvider for FileAuthProvider {
    async fn get_api_key(&self, provider: &str) -> Result<Option<String>, SdkError> {
        Ok(self.resolve_api_key(provider))
    }

    async fn set_api_key(&self, provider: &str, key: &str) -> Result<(), SdkError> {
        let mut s = self.state.lock();
        s.providers
            .entry(provider.to_string())
            .or_default()
            .api_key = Some(key.to_string());
        s.save(&self.path).map_err(|e| SdkError::Internal(e.into()))
    }

    async fn delete_api_key(&self, provider: &str) -> Result<(), SdkError> {
        let mut s = self.state.lock();
        if let Some(entry) = s.providers.get_mut(provider) {
            entry.api_key = None;
            if entry.oauth.is_none() {
                s.providers.remove(provider);
            }
        }
        s.save(&self.path).map_err(|e| SdkError::Internal(e.into()))
    }

    async fn get_oauth(&self, provider: &str) -> Result<Option<OAuthToken>, SdkError> {
        let s = self.state.lock();
        Ok(s.providers.get(provider).and_then(|e| e.oauth.clone()))
    }

    async fn set_oauth(&self, provider: &str, token: OAuthToken) -> Result<(), SdkError> {
        let mut s = self.state.lock();
        s.providers
            .entry(provider.to_string())
            .or_default()
            .oauth = Some(token);
        s.save(&self.path).map_err(|e| SdkError::Internal(e.into()))
    }

    async fn list_providers(&self) -> Result<Vec<String>, SdkError> {
        let s = self.state.lock();
        Ok(s.providers.keys().cloned().collect())
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
}
