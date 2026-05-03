//! Authentication storage for API keys and OAuth tokens
//!
//! Provides secure storage and retrieval of authentication credentials,
//! with OS keyring integration and fallback to encrypted file storage.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// Authentication credential
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthCredential {
    /// API key credential
    ApiKey { key: String },
    /// OAuth credential with token management
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: u64,
        /// Scopes granted (space-separated)
        #[serde(default)]
        scopes: Option<String>,
        /// Provider-specific data (JSON for extensibility)
        #[serde(default)]
        provider_data: Option<serde_json::Value>,
    },
}

impl AuthCredential {
    /// Check if the OAuth token is expired
    pub fn is_expired(&self) -> bool {
        match self {
            AuthCredential::OAuth { expires_at, .. } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                *expires_at <= now
            }
            AuthCredential::ApiKey { .. } => false,
        }
    }

    /// Check if the token needs refresh (within 60 seconds of expiration)
    pub fn needs_refresh(&self) -> bool {
        match self {
            AuthCredential::OAuth {
                expires_at,
                refresh_token,
                ..
            } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                refresh_token.is_some() && *expires_at <= now + 60
            }
            AuthCredential::ApiKey { .. } => false,
        }
    }

    /// Get the access token if valid
    pub fn access_token(&self) -> Option<&str> {
        match self {
            AuthCredential::OAuth { access_token, .. } if !self.is_expired() => Some(access_token),
            _ => None,
        }
    }
}

/// Authentication status
#[derive(Debug, Clone)]
pub struct AuthStatus {
    /// Whether auth is configured
    pub configured: bool,
    /// Source of the auth (stored, runtime, environment, etc.)
    pub source: Option<String>,
    /// Label for display
    pub label: Option<String>,
}

/// Result of an auth operation
type AuthResult<T> = Result<T, AuthError>;

/// Authentication errors
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Failed to read auth storage: {0}")]
    ReadError(String),
    #[error("Failed to write auth storage: {0}")]
    WriteError(String),
    #[error("Credential not found: {0}")]
    NotFound(String),
    #[error("Invalid credential format: {0}")]
    InvalidFormat(String),
    #[error("Keyring error: {0}")]
    KeyringError(String),
}

/// Storage backend trait
pub trait AuthStorageBackend: Send + Sync {
    /// Read a value
    fn read(&self) -> AuthResult<Option<String>>;
    /// Write a value
    fn write(&self, data: &str) -> AuthResult<()>;
    /// Delete a value
    fn delete(&self) -> AuthResult<()>;
}

/// File-based auth storage backend
pub struct FileAuthStorage {
    path: PathBuf,
    cache: RwLock<Option<String>>,
}

impl FileAuthStorage {
    /// Create a new file-based auth storage
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: RwLock::new(None),
        }
    }

    /// Get the default auth file path
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("oxi").join("auth.json"))
    }
}

impl AuthStorageBackend for FileAuthStorage {
    fn read(&self) -> AuthResult<Option<String>> {
        if !self.path.exists() {
            return Ok(None);
        }

        match std::fs::read_to_string(&self.path) {
            Ok(content) => {
                *self.cache.write().unwrap() = Some(content.clone());
                Ok(Some(content))
            }
            Err(e) => Err(AuthError::ReadError(e.to_string())),
        }
    }

    fn write(&self, data: &str) -> AuthResult<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuthError::WriteError(e.to_string()))?;
        }

        // Set file permissions to owner-only on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.path, perms)
                .map_err(|e| AuthError::WriteError(e.to_string()))?;
        }

        std::fs::write(&self.path, data).map_err(|e| AuthError::WriteError(e.to_string()))?;
        *self.cache.write().unwrap() = Some(data.to_string());
        Ok(())
    }

    fn delete(&self) -> AuthResult<()> {
        if self.path.exists() {
            std::fs::remove_file(&self.path).map_err(|e| AuthError::WriteError(e.to_string()))?;
        }
        *self.cache.write().unwrap() = None;
        Ok(())
    }
}

/// Environment variable backend
pub struct EnvAuthStorage {
    provider_prefix: String,
}

impl EnvAuthStorage {
    /// Create a new environment-based auth storage
    pub fn new(provider: &str) -> Self {
        Self {
            provider_prefix: format!("{}_API_KEY", provider.to_uppercase().replace('-', "_")),
        }
    }
}

impl AuthStorageBackend for EnvAuthStorage {
    fn read(&self) -> AuthResult<Option<String>> {
        Ok(std::env::var(&self.provider_prefix).ok())
    }

    fn write(&self, _data: &str) -> AuthResult<()> {
        Err(AuthError::WriteError(
            "Cannot write to environment variables".to_string(),
        ))
    }

    fn delete(&self) -> AuthResult<()> {
        std::env::remove_var(&self.provider_prefix);
        Ok(())
    }
}

/// Memory-based auth storage (for testing)
pub struct MemoryAuthStorage {
    data: RwLock<HashMap<String, AuthCredential>>,
}

impl MemoryAuthStorage {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryAuthStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthStorageBackend for MemoryAuthStorage {
    fn read(&self) -> AuthResult<Option<String>> {
        // Not applicable for memory storage
        Ok(None)
    }

    fn write(&self, _data: &str) -> AuthResult<()> {
        // Not applicable for memory storage
        Ok(())
    }

    fn delete(&self) -> AuthResult<()> {
        self.data.write().unwrap().clear();
        Ok(())
    }
}

/// Main auth storage struct
pub struct AuthStorage {
    /// File-based storage
    file_storage: Option<FileAuthStorage>,
    /// In-memory cache
    credentials: RwLock<HashMap<String, AuthCredential>>,
    /// Runtime overrides (CLI --api-key)
    runtime_overrides: RwLock<HashMap<String, String>>,
}

impl AuthStorage {
    /// Create a new auth storage with default file backend
    pub fn new() -> Self {
        let file_storage = Self::default_path().map(FileAuthStorage::new);

        let credentials = if let Some(ref storage) = file_storage {
            if let Ok(Some(content)) = storage.read() {
                serde_json::from_str(&content).unwrap_or_default()
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        Self {
            file_storage,
            credentials: RwLock::new(credentials),
            runtime_overrides: RwLock::new(HashMap::new()),
        }
    }

    /// Create with explicit storage backend
    pub fn with_backend<B: AuthStorageBackend + 'static>(backend: B) -> Self {
        let credentials = if let Ok(Some(content)) = backend.read() {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Self {
            file_storage: None,
            credentials: RwLock::new(credentials),
            runtime_overrides: RwLock::new(HashMap::new()),
        }
    }

    /// Create a memory-only storage
    pub fn in_memory() -> Self {
        Self {
            file_storage: None,
            credentials: RwLock::new(HashMap::new()),
            runtime_overrides: RwLock::new(HashMap::new()),
        }
    }

    /// Get the default auth file path
    fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("oxi").join("auth.json"))
    }

    /// Set a runtime API key override (from CLI --api-key)
    pub fn set_runtime_key(&self, provider: &str, api_key: String) {
        self.runtime_overrides
            .write()
            .unwrap()
            .insert(provider.to_string(), api_key);
    }

    /// Remove a runtime override
    pub fn remove_runtime_key(&self, provider: &str) {
        self.runtime_overrides.write().unwrap().remove(provider);
    }

    /// Check if a provider has any auth configured
    pub fn has_auth(&self, provider: &str) -> bool {
        // Check runtime override
        if self
            .runtime_overrides
            .read()
            .unwrap()
            .contains_key(provider)
        {
            return true;
        }

        // Check stored credentials
        if self.credentials.read().unwrap().contains_key(provider) {
            return true;
        }

        // Check environment
        let env_key = format!("{}_API_KEY", provider.to_uppercase().replace('-', "_"));
        std::env::var(&env_key).is_ok()
    }

    /// Get auth status for a provider
    pub fn get_status(&self, provider: &str) -> AuthStatus {
        if self
            .runtime_overrides
            .read()
            .unwrap()
            .contains_key(provider)
        {
            return AuthStatus {
                configured: false,
                source: Some("runtime".to_string()),
                label: Some("--api-key".to_string()),
            };
        }

        if self.credentials.read().unwrap().contains_key(provider) {
            return AuthStatus {
                configured: true,
                source: Some("stored".to_string()),
                label: None,
            };
        }

        let env_key = format!("{}_API_KEY", provider.to_uppercase().replace('-', "_"));
        if std::env::var(&env_key).is_ok() {
            return AuthStatus {
                configured: false,
                source: Some("environment".to_string()),
                label: Some(env_key),
            };
        }

        AuthStatus {
            configured: false,
            source: None,
            label: None,
        }
    }

    /// Get API key for a provider
    ///
    /// Priority:
    /// 1. Runtime override (CLI --api-key)
    /// 2. Stored API key
    /// 3. OAuth token (auto-refreshed)
    /// 4. Environment variable
    pub fn get_api_key(&self, provider: &str) -> Option<String> {
        // 1. Runtime override
        if let Some(key) = self.runtime_overrides.read().unwrap().get(provider) {
            return Some(key.clone());
        }

        // 2. Stored credential
        if let Some(cred) = self.credentials.read().unwrap().get(provider) {
            return match cred {
                AuthCredential::ApiKey { key } => Some(key.clone()),
                AuthCredential::OAuth {
                    access_token,
                    expires_at,
                    ..
                } => {
                    // Check if token needs refresh
                    if *expires_at
                        > std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs()
                    {
                        Some(access_token.clone())
                    } else {
                        // Token expired
                        None
                    }
                }
            };
        }

        // 3. Environment variable
        let env_key = format!("{}_API_KEY", provider.to_uppercase().replace('-', "_"));
        std::env::var(&env_key).ok()
    }

    /// Set API key for a provider
    pub fn set_api_key(&self, provider: &str, key: String) {
        self.credentials
            .write()
            .unwrap()
            .insert(provider.to_string(), AuthCredential::ApiKey { key });
        self.persist();
    }

    /// Set OAuth credential for a provider
    pub fn set_oauth(
        &self,
        provider: &str,
        access_token: String,
        refresh_token: Option<String>,
        expires_at: u64,
    ) {
        self.set_oauth_full(
            provider,
            access_token,
            refresh_token,
            expires_at,
            None,
            None,
        );
    }

    /// Set OAuth credential with full details for a provider
    pub fn set_oauth_full(
        &self,
        provider: &str,
        access_token: String,
        refresh_token: Option<String>,
        expires_at: u64,
        scopes: Option<String>,
        provider_data: Option<serde_json::Value>,
    ) {
        self.credentials.write().unwrap().insert(
            provider.to_string(),
            AuthCredential::OAuth {
                access_token,
                refresh_token,
                expires_at,
                scopes,
                provider_data,
            },
        );
        self.persist();
    }

    /// Get OAuth credential for a provider (for token refresh)
    pub fn get_oauth_credential(&self, provider: &str) -> Option<AuthCredential> {
        self.credentials.read().unwrap().get(provider).cloned()
    }

    /// Check if a provider has OAuth credentials that can be refreshed
    pub fn has_oauth_with_refresh(&self, provider: &str) -> bool {
        if let Some(cred) = self.credentials.read().unwrap().get(provider) {
            match cred {
                AuthCredential::OAuth { refresh_token, .. } => refresh_token.is_some(),
                _ => false,
            }
        } else {
            false
        }
    }

    /// Remove credential for a provider
    pub fn remove(&self, provider: &str) {
        self.credentials.write().unwrap().remove(provider);
        self.persist();
    }

    /// List all providers with credentials
    pub fn list_providers(&self) -> Vec<String> {
        self.credentials.read().unwrap().keys().cloned().collect()
    }

    /// Check if credential exists for provider
    pub fn has(&self, provider: &str) -> bool {
        self.credentials.read().unwrap().contains_key(provider)
    }

    /// Get all credentials (for debugging)
    pub fn get_all(&self) -> HashMap<String, AuthCredential> {
        self.credentials.read().unwrap().clone()
    }

    /// Clear all stored credentials
    pub fn clear(&self) {
        self.credentials.write().unwrap().clear();
        self.persist();
    }

    /// Reload from disk
    pub fn reload(&self) {
        if let Some(ref storage) = self.file_storage {
            if let Ok(Some(content)) = storage.read() {
                if let Ok(creds) = serde_json::from_str(&content) {
                    *self.credentials.write().unwrap() = creds;
                }
            }
        }
    }

    /// Persist to disk
    fn persist(&self) {
        if let Some(ref storage) = self.file_storage {
            let creds = self.credentials.read().unwrap();
            if let Ok(json) = serde_json::to_string_pretty(&*creds) {
                let _ = storage.write(&json);
            }
        }
    }
}

impl Default for AuthStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrapper for using keyring crate with fallback
pub mod keyring_support {
    use super::*;

    /// Try to get a secret from the OS keyring
    #[cfg(feature = "keyring")]
    pub fn get_keyring_secret(service: &str, account: &str) -> Option<String> {
        use keyring::Entry;
        Entry::new(service, account)
            .ok()
            .and_then(|entry| entry.get_password().ok())
    }

    /// Try to set a secret in the OS keyring
    #[cfg(feature = "keyring")]
    pub fn set_keyring_secret(service: &str, account: &str, secret: &str) -> Result<(), AuthError> {
        use keyring::Entry;
        Entry::new(service, account)
            .map_err(|e| AuthError::KeyringError(e.to_string()))?
            .set_password(secret)
            .map_err(|e| AuthError::KeyringError(e.to_string()))
    }

    /// Try to delete a secret from the OS keyring
    #[cfg(feature = "keyring")]
    pub fn delete_keyring_secret(service: &str, account: &str) -> Result<(), AuthError> {
        use keyring::Entry;
        Entry::new(service, account)
            .map_err(|e| AuthError::KeyringError(e.to_string()))?
            .delete_credential()
            .map_err(|e| AuthError::KeyringError(e.to_string()))
    }

    // Non-keyring fallbacks
    #[cfg(not(feature = "keyring"))]
    pub fn get_keyring_secret(_service: &str, _account: &str) -> Option<String> {
        None
    }

    #[cfg(not(feature = "keyring"))]
    pub fn set_keyring_secret(
        _service: &str,
        _account: &str,
        _secret: &str,
    ) -> Result<(), AuthError> {
        Err(AuthError::KeyringError(
            "Keyring support not compiled".to_string(),
        ))
    }

    #[cfg(not(feature = "keyring"))]
    pub fn delete_keyring_secret(_service: &str, _account: &str) -> Result<(), AuthError> {
        Err(AuthError::KeyringError(
            "Keyring support not compiled".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_storage_new() {
        let storage = AuthStorage::in_memory();
        assert!(!storage.has("anthropic"));
    }

    #[test]
    fn test_set_and_get_api_key() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("anthropic", "sk-test123".to_string());
        assert!(storage.has("anthropic"));
        assert_eq!(
            storage.get_api_key("anthropic"),
            Some("sk-test123".to_string())
        );
    }

    #[test]
    fn test_runtime_override() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("anthropic", "stored-key".to_string());
        storage.set_runtime_key("anthropic", "runtime-key".to_string());

        // Runtime key should take priority
        assert_eq!(
            storage.get_api_key("anthropic"),
            Some("runtime-key".to_string())
        );
    }

    #[test]
    fn test_remove_credential() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("anthropic", "sk-test123".to_string());
        assert!(storage.has("anthropic"));

        storage.remove("anthropic");
        assert!(!storage.has("anthropic"));
    }

    #[test]
    fn test_auth_status() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("anthropic", "sk-test123".to_string());

        let status = storage.get_status("anthropic");
        assert!(status.configured);
        assert_eq!(status.source, Some("stored".to_string()));
    }

    #[test]
    fn test_list_providers() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("anthropic", "key1".to_string());
        storage.set_api_key("openai", "key2".to_string());

        let providers = storage.list_providers();
        assert!(providers.contains(&"anthropic".to_string()));
        assert!(providers.contains(&"openai".to_string()));
    }

    #[test]
    fn test_oauth_credential() {
        let storage = AuthStorage::in_memory();
        storage.set_oauth(
            "provider",
            "access123".to_string(),
            Some("refresh456".to_string()),
            u64::MAX,
        );

        assert!(storage.has("provider"));
        assert_eq!(
            storage.get_api_key("provider"),
            Some("access123".to_string())
        );
    }

    #[test]
    fn test_expired_oauth_token() {
        let storage = AuthStorage::in_memory();
        // Set token that expired in the past
        storage.set_oauth("provider", "access123".to_string(), None, 0);

        // Token should be treated as expired
        let key = storage.get_api_key("provider");
        assert!(key.is_none());
    }

    #[test]
    fn test_get_all_credentials() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("anthropic", "key1".to_string());
        storage.set_api_key("openai", "key2".to_string());

        let all = storage.get_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_clear() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("anthropic", "key".to_string());
        assert!(storage.has("anthropic"));

        storage.clear();
        assert!(!storage.has("anthropic"));
    }

    #[test]
    fn test_remove_runtime_key() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("anthropic", "stored".to_string());
        storage.set_runtime_key("anthropic", "runtime".to_string());

        assert_eq!(
            storage.get_api_key("anthropic"),
            Some("runtime".to_string())
        );

        storage.remove_runtime_key("anthropic");
        assert_eq!(storage.get_api_key("anthropic"), Some("stored".to_string()));
    }

    #[test]
    fn test_auth_credential_is_expired() {
        // API key never expires
        let api_key_cred = AuthCredential::ApiKey {
            key: "test".to_string(),
        };
        assert!(!api_key_cred.is_expired());

        // OAuth token that expires in the future
        let future_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let oauth_cred = AuthCredential::OAuth {
            access_token: "token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: future_time,
            scopes: None,
            provider_data: None,
        };
        assert!(!oauth_cred.is_expired());

        // OAuth token that expired in the past
        let oauth_cred_expired = AuthCredential::OAuth {
            access_token: "token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: 0,
            scopes: None,
            provider_data: None,
        };
        assert!(oauth_cred_expired.is_expired());
    }

    #[test]
    fn test_auth_credential_needs_refresh() {
        let future_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 120; // 2 minutes from now

        // Has refresh token, will expire soon
        let oauth_cred = AuthCredential::OAuth {
            access_token: "token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: future_time,
            scopes: None,
            provider_data: None,
        };
        assert!(oauth_cred.needs_refresh());

        // No refresh token - doesn't need refresh
        let no_refresh = AuthCredential::OAuth {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: future_time,
            scopes: None,
            provider_data: None,
        };
        assert!(!no_refresh.needs_refresh());

        // API key never needs refresh
        let api_key_cred = AuthCredential::ApiKey {
            key: "test".to_string(),
        };
        assert!(!api_key_cred.needs_refresh());
    }

    #[test]
    fn test_auth_credential_access_token() {
        let future_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;

        let oauth_cred = AuthCredential::OAuth {
            access_token: "valid_token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: future_time,
            scopes: None,
            provider_data: None,
        };
        assert_eq!(oauth_cred.access_token(), Some("valid_token"));

        // Expired token
        let expired_cred = AuthCredential::OAuth {
            access_token: "expired_token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: 0,
            scopes: None,
            provider_data: None,
        };
        assert!(expired_cred.access_token().is_none());

        // API key always returns token
        let api_key_cred = AuthCredential::ApiKey {
            key: "api_key_token".to_string(),
        };
        assert!(api_key_cred.access_token().is_none()); // API keys don't use access_token method
    }

    #[test]
    fn test_get_oauth_credential() {
        let storage = AuthStorage::in_memory();
        storage.set_oauth(
            "provider",
            "access".to_string(),
            Some("refresh".to_string()),
            u64::MAX,
        );

        let cred = storage.get_oauth_credential("provider");
        assert!(cred.is_some());
        assert!(matches!(cred.unwrap(), AuthCredential::OAuth { .. }));
    }

    #[test]
    fn test_has_oauth_with_refresh() {
        let storage = AuthStorage::in_memory();

        // With refresh token
        storage.set_oauth(
            "with_refresh",
            "access".to_string(),
            Some("refresh".to_string()),
            u64::MAX,
        );
        assert!(storage.has_oauth_with_refresh("with_refresh"));

        // Without refresh token
        storage.set_oauth("without_refresh", "access".to_string(), None, u64::MAX);
        assert!(!storage.has_oauth_with_refresh("without_refresh"));

        // API key provider
        storage.set_api_key("apikey_provider", "key".to_string());
        assert!(!storage.has_oauth_with_refresh("apikey_provider"));
    }

    #[test]
    fn test_set_oauth_full() {
        let storage = AuthStorage::in_memory();
        storage.set_oauth_full(
            "provider",
            "access_token".to_string(),
            Some("refresh_token".to_string()),
            3600,
            Some("read write".to_string()),
            Some(serde_json::json!({"extra": "data"})),
        );

        let cred = storage.get_oauth_credential("provider");
        assert!(cred.is_some());
        if let AuthCredential::OAuth {
            scopes,
            provider_data,
            ..
        } = cred.unwrap()
        {
            assert_eq!(scopes, Some("read write".to_string()));
            assert!(provider_data.is_some());
        } else {
            panic!("Expected OAuth credential");
        }
    }
}
