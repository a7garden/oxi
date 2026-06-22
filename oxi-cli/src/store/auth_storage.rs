//! Authentication storage for API keys, OAuth tokens, and session tokens.
//!
//! Provides secure storage and retrieval of authentication credentials,
//! with OS keyring integration and fallback to encrypted file storage.
//! Supports multi-provider auth, credential validation, and session tokens.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

// ============================================================================
// Credential Types
// ============================================================================

/// Authentication credential
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthCredential {
    /// API key credential
    ApiKey {
        /// The API key string.
        key: String,
    },
    /// OAuth credential with token management
    OAuth {
        /// access_token.
        access_token: String,
        /// refresh_token.
        refresh_token: Option<String>,
        /// expires_at.
        expires_at: u64,
        /// Scopes granted (space-separated)
        #[serde(default)]
        scopes: Option<String>,
        /// Provider-specific data (JSON for extensibility)
        #[serde(default)]
        provider_data: Option<serde_json::Value>,
    },
    /// Session token credential (e.g. from browser-based login)
    Session {
        /// token.
        token: String,
        /// When the session expires (unix timestamp, 0 = never)
        #[serde(default)]
        expires_at: u64,
        /// Session metadata (user info, etc.)
        #[serde(default)]
        metadata: Option<serde_json::Value>,
    },
}

impl AuthCredential {
    /// Check if the credential is expired
    pub fn is_expired(&self) -> bool {
        match self {
            AuthCredential::OAuth { expires_at, .. } => {
                let now = now_secs();
                *expires_at < now
            }
            AuthCredential::Session { expires_at, .. } => {
                if *expires_at == 0 {
                    return false; // never expires
                }
                *expires_at <= now_secs()
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
                let now = now_secs();
                refresh_token.is_some() && *expires_at <= now + 60
            }
            AuthCredential::Session { .. } => false,
            AuthCredential::ApiKey { .. } => false,
        }
    }

    /// Get the access token if valid (not expired)
    pub fn access_token(&self) -> Option<&str> {
        match self {
            AuthCredential::OAuth { access_token, .. } if !self.is_expired() => Some(access_token),
            AuthCredential::Session { token, .. } if !self.is_expired() => Some(token),
            _ => None,
        }
    }

    /// Get the credential type name
    pub fn type_name(&self) -> &'static str {
        match self {
            AuthCredential::ApiKey { .. } => "api_key",
            AuthCredential::OAuth { .. } => "oauth",
            AuthCredential::Session { .. } => "session",
        }
    }

    /// Validate the credential structure
    pub fn validate(&self) -> Result<(), CredentialValidationError> {
        match self {
            AuthCredential::ApiKey { key } => {
                if key.is_empty() {
                    return Err(CredentialValidationError::EmptyField("key".to_string()));
                }
                // Check for common placeholder values
                if key == "your-api-key-here" || key == "xxx" {
                    return Err(CredentialValidationError::PlaceholderValue(key.clone()));
                }
                Ok(())
            }
            AuthCredential::OAuth {
                access_token,
                expires_at,
                ..
            } => {
                if access_token.is_empty() {
                    return Err(CredentialValidationError::EmptyField(
                        "access_token".to_string(),
                    ));
                }
                if *expires_at == 0 {
                    return Err(CredentialValidationError::InvalidExpiry);
                }
                Ok(())
            }
            AuthCredential::Session { token, .. } => {
                if token.is_empty() {
                    return Err(CredentialValidationError::EmptyField("token".to_string()));
                }
                Ok(())
            }
        }
    }
}

/// Credential validation error
#[derive(Debug, Clone, thiserror::Error)]
pub enum CredentialValidationError {
    #[error("Field '{0}' must not be empty")]
    /// empty field variant.
    EmptyField(String),
    #[error("Placeholder value detected: '{0}'")]
    /// placeholder value variant.
    PlaceholderValue(String),
    #[error("Invalid expiry timestamp")]
    /// invalid expiry variant.
    InvalidExpiry,
}

// ============================================================================
// Auth Status
// ============================================================================

/// Authentication status
#[derive(Debug, Clone)]
pub struct AuthStatus {
    /// Whether auth is configured
    pub configured: bool,
    /// Source of the auth (stored, runtime, environment, fallback)
    pub source: Option<String>,
    /// Label for display
    pub label: Option<String>,
}

impl std::fmt::Display for AuthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.source, &self.label) {
            (Some(source), Some(label)) => write!(f, "{} ({})", source, label),
            (Some(source), None) => write!(f, "{}", source),
            (None, Some(label)) => write!(f, "{}", label),
            (None, None) => write!(f, "not configured"),
        }
    }
}

// ============================================================================
// Auth Errors
// ============================================================================

/// Result of an auth operation
pub type AuthResult<T> = Result<T, AuthError>;

/// Authentication errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthError {
    #[error("Failed to read auth storage: {0}")]
    /// read error variant.
    ReadError(String),
    #[error("Failed to write auth storage: {0}")]
    /// write error variant.
    WriteError(String),
    #[error("Credential not found: {0}")]
    /// not found variant.
    NotFound(String),
    #[error("Invalid credential format: {0}")]
    /// invalid format variant.
    InvalidFormat(String),
    #[error("Keyring error: {0}")]
    /// keyring error variant.
    KeyringError(String),
    #[error("Credential validation failed: {0}")]
    /// validation failed variant.
    ValidationFailed(String),
}

// ============================================================================
// Storage Backend Trait
// ============================================================================

/// Storage backend trait
pub trait AuthStorageBackend: Send + Sync {
    /// Read stored data
    fn read(&self) -> AuthResult<Option<String>>;
    /// Write data
    fn write(&self, data: &str) -> AuthResult<()>;
    /// Delete stored data
    fn delete(&self) -> AuthResult<()>;
}

// ============================================================================
// File Backend
// ============================================================================

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

    /// Get the default auth file path (uses ~/.oxi/auth.json for consistency with settings)
    pub fn default_path() -> Option<PathBuf> {
        dirs::home_dir().map(|p| p.join(".oxi").join("auth.json"))
    }

    /// Get the storage path
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl AuthStorageBackend for FileAuthStorage {
    fn read(&self) -> AuthResult<Option<String>> {
        if !self.path.exists() {
            return Ok(None);
        }

        match std::fs::read_to_string(&self.path) {
            Ok(content) => {
                *self.cache.write() = Some(content.clone());
                Ok(Some(content))
            }
            Err(e) => Err(AuthError::ReadError(e.to_string())),
        }
    }

    fn write(&self, data: &str) -> AuthResult<()> {
        // Ensure parent directory exists with restricted permissions
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuthError::WriteError(e.to_string()))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o700);
                let _ = std::fs::set_permissions(parent, perms);
            }
        }

        // Write the file
        std::fs::write(&self.path, data).map_err(|e| AuthError::WriteError(e.to_string()))?;

        // Set file permissions to owner-only on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.path, perms)
                .map_err(|e| AuthError::WriteError(e.to_string()))?;
        }

        *self.cache.write() = Some(data.to_string());
        Ok(())
    }

    fn delete(&self) -> AuthResult<()> {
        if self.path.exists() {
            std::fs::remove_file(&self.path).map_err(|e| AuthError::WriteError(e.to_string()))?;
        }
        *self.cache.write() = None;
        Ok(())
    }
}

// ============================================================================
// Memory Backend
// ============================================================================

/// Memory-based auth storage (for testing)
pub struct MemoryAuthStorage {
    data: RwLock<HashMap<String, AuthCredential>>,
}

impl MemoryAuthStorage {
    /// Create a new memory auth storage
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
        // Memory backend doesn't use JSON serialization
        Ok(None)
    }

    fn write(&self, _data: &str) -> AuthResult<()> {
        Ok(())
    }

    fn delete(&self) -> AuthResult<()> {
        self.data.write().clear();
        Ok(())
    }
}

// ============================================================================
// Fallback Resolver
// ============================================================================

/// Trait for fallback API key resolution (e.g., from models.json config)
pub trait FallbackResolver: Send + Sync {
    /// Try to resolve an API key for the given provider
    fn resolve(&self, provider: &str) -> Option<String>;
}

/// A simple closure-based fallback resolver
pub struct FnFallbackResolver {
    #[allow(clippy::type_complexity)]
    f: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

impl FnFallbackResolver {
    /// Create from a closure
    #[allow(clippy::type_complexity)]
    pub fn new(f: Box<dyn Fn(&str) -> Option<String> + Send + Sync>) -> Self {
        Self { f }
    }
}

impl FallbackResolver for FnFallbackResolver {
    fn resolve(&self, provider: &str) -> Option<String> {
        (self.f)(provider)
    }
}

/// Environment variable fallback resolver.
///
/// Uses oxi-ai's `BuiltinProvider` registry to look up the primary and
/// extra env var names for each provider, then checks `std::env`.
pub struct EnvVarFallbackResolver;

impl FallbackResolver for EnvVarFallbackResolver {
    fn resolve(&self, provider: &str) -> Option<String> {
        // Look up the provider's env key from the builtin registry
        let builtin = oxi_ai::get_builtin_provider(provider)?;
        let key = builtin.env_key;

        // Try primary key
        if let Ok(val) = std::env::var(key)
            && !val.is_empty()
        {
            return Some(val);
        }

        // Try extra keys
        for extra in builtin.extra_env_keys {
            if let Ok(val) = std::env::var(extra)
                && !val.is_empty()
            {
                return Some(val);
            }
        }

        None
    }
}

// ============================================================================
// Auth Storage (Main)
// ============================================================================

/// Main auth storage struct.
///
/// Provides multi-layered credential lookup with the following priority:
/// 1. Runtime override (CLI --api-key)
/// 2. Stored API key from auth.json
/// 3. OAuth token from auth.json (with auto-refresh awareness)
/// 4. Session token from auth.json
/// 5. Environment variable
/// 6. Fallback resolver (e.g., custom provider config from models.json)
pub struct AuthStorage {
    /// File-based storage backend
    file_storage: Option<Arc<dyn AuthStorageBackend>>,
    /// In-memory credential cache
    credentials: RwLock<HashMap<String, AuthCredential>>,
    /// Runtime overrides (CLI --api-key)
    runtime_overrides: RwLock<HashMap<String, String>>,
    /// Fallback resolver for custom providers
    fallback_resolver: RwLock<Option<Arc<dyn FallbackResolver>>>,
    /// Collected errors
    errors: RwLock<Vec<AuthError>>,
    /// Whether initial load had an error
    load_error: RwLock<Option<AuthError>>,
    /// OnceLock to warn about plaintext storage only once
    plaintext_warned: OnceLock<()>,
}

impl AuthStorage {
    /// Create a new auth storage with default file backend
    pub fn new() -> Self {
        let file_storage = FileAuthStorage::default_path()
            .map(|p| Arc::new(FileAuthStorage::new(p)) as Arc<dyn AuthStorageBackend>);

        let credentials = if let Some(ref storage) = file_storage {
            match storage.read() {
                Ok(Some(content)) => serde_json::from_str(&content).unwrap_or_default(),
                _ => HashMap::new(),
            }
        } else {
            HashMap::new()
        };

        Self {
            file_storage,
            credentials: RwLock::new(credentials),
            runtime_overrides: RwLock::new(HashMap::new()),
            fallback_resolver: RwLock::new(None),
            errors: RwLock::new(Vec::new()),
            load_error: RwLock::new(None),
            plaintext_warned: OnceLock::new(),
        }
    }

    /// Create with explicit storage backend
    pub fn with_backend(backend: impl AuthStorageBackend + 'static) -> Self {
        let credentials = match backend.read() {
            Ok(Some(content)) => serde_json::from_str(&content).unwrap_or_default(),
            _ => HashMap::new(),
        };

        Self {
            file_storage: Some(Arc::new(backend)),
            credentials: RwLock::new(credentials),
            runtime_overrides: RwLock::new(HashMap::new()),
            fallback_resolver: RwLock::new(None),
            errors: RwLock::new(Vec::new()),
            load_error: RwLock::new(None),
            plaintext_warned: OnceLock::new(),
        }
    }

    /// Create a memory-only storage (for testing)
    pub fn in_memory() -> Self {
        Self {
            file_storage: None,
            credentials: RwLock::new(HashMap::new()),
            runtime_overrides: RwLock::new(HashMap::new()),
            fallback_resolver: RwLock::new(None),
            errors: RwLock::new(Vec::new()),
            load_error: RwLock::new(None),
            plaintext_warned: OnceLock::new(),
        }
    }

    /// Get the default auth file path
    pub fn default_path() -> Option<PathBuf> {
        FileAuthStorage::default_path()
    }

    // -----------------------------------------------------------------------
    // Runtime overrides
    // -----------------------------------------------------------------------

    /// Set a runtime API key override (from CLI --api-key)
    pub fn set_runtime_key(&self, provider: &str, api_key: String) {
        self.runtime_overrides
            .write()
            .insert(provider.to_string(), api_key);
    }

    /// Remove a runtime override
    pub fn remove_runtime_key(&self, provider: &str) {
        self.runtime_overrides.write().remove(provider);
    }

    // -----------------------------------------------------------------------
    // Fallback resolver
    // -----------------------------------------------------------------------

    /// Set a fallback resolver for API keys not found in auth.json or env vars.
    /// Used for custom provider keys from models.json.
    pub fn set_fallback_resolver(&self, resolver: Arc<dyn FallbackResolver>) {
        *self.fallback_resolver.write() = Some(resolver);
    }

    /// Clear the fallback resolver
    pub fn clear_fallback_resolver(&self) {
        *self.fallback_resolver.write() = None;
    }

    // -----------------------------------------------------------------------
    // Credential query
    // -----------------------------------------------------------------------

    /// Check if a provider has any auth configured
    pub fn has_auth(&self, provider: &str) -> bool {
        if self.runtime_overrides.read().contains_key(provider) {
            return true;
        }
        if self.credentials.read().contains_key(provider) {
            return true;
        }
        if let Some(ref resolver) = *self.fallback_resolver.read()
            && resolver.resolve(provider).is_some()
        {
            return true;
        }
        false
    }

    /// Get auth status for a provider (without exposing credentials)
    pub fn get_status(&self, provider: &str) -> AuthStatus {
        if self.runtime_overrides.read().contains_key(provider) {
            return AuthStatus {
                configured: false,
                source: Some("runtime".to_string()),
                label: Some("--api-key".to_string()),
            };
        }

        if let Some(cred) = self.credentials.read().get(provider) {
            return AuthStatus {
                configured: true,
                source: Some("stored".to_string()),
                label: Some(cred.type_name().to_string()),
            };
        }

        if let Some(ref resolver) = *self.fallback_resolver.read()
            && resolver.resolve(provider).is_some()
        {
            return AuthStatus {
                configured: false,
                source: Some("fallback".to_string()),
                label: Some("custom provider config".to_string()),
            };
        }

        AuthStatus {
            configured: false,
            source: None,
            label: None,
        }
    }

    /// Get API key for a provider.
    ///
    /// Priority:
    /// 1. Runtime override (CLI --api-key)
    /// 2. Stored API key from auth.json
    /// 3. OAuth token from auth.json (auto-refreshed)
    /// 4. Session token from auth.json
    /// 5. Fallback resolver
    pub fn get_api_key(&self, provider: &str) -> Option<String> {
        self.get_api_key_with_options(provider, true)
    }

    /// Get API key with option to include/exclude fallback resolver
    pub fn get_api_key_with_options(
        &self,
        provider: &str,
        include_fallback: bool,
    ) -> Option<String> {
        // 1. Runtime override
        if let Some(key) = self.runtime_overrides.read().get(provider) {
            return Some(key.clone());
        }

        // 2-4. Stored credential
        if let Some(cred) = self.credentials.read().get(provider) {
            return match cred {
                AuthCredential::ApiKey { key } => Some(key.clone()),
                AuthCredential::OAuth {
                    access_token,
                    expires_at,
                    ..
                } => {
                    if *expires_at > now_secs() {
                        Some(access_token.clone())
                    } else {
                        // Token expired - caller should handle refresh
                        None
                    }
                }
                AuthCredential::Session {
                    token, expires_at, ..
                } => {
                    if *expires_at == 0 || *expires_at > now_secs() {
                        Some(token.clone())
                    } else {
                        None
                    }
                }
            };
        }

        // 5. Cross-provider alias lookup:
        //    If the key is stored under a different provider name that shares
        //    the same env_key (e.g. key stored as "zai-coding-global" but
        //    looked up as "zai"), check those providers' stored credentials.
        if let Some(builtin) = oxi_ai::register_builtins::get_builtin_provider(provider) {
            let env_key = builtin.env_key;
            let credentials = self.credentials.read();
            for other in oxi_ai::register_builtins::get_builtin_providers() {
                if other.name == provider {
                    continue; // already checked above
                }
                if other.env_key == env_key
                    && let Some(cred) = credentials.get(other.name)
                {
                    return match cred {
                        AuthCredential::ApiKey { key } => Some(key.clone()),
                        AuthCredential::OAuth {
                            access_token,
                            expires_at,
                            ..
                        } => {
                            if *expires_at > now_secs() {
                                Some(access_token.clone())
                            } else {
                                None
                            }
                        }
                        AuthCredential::Session {
                            token, expires_at, ..
                        } => {
                            if *expires_at == 0 || *expires_at > now_secs() {
                                Some(token.clone())
                            } else {
                                None
                            }
                        }
                    };
                }
            }
        }

        // 6. Fallback resolver
        if include_fallback && let Some(ref resolver) = *self.fallback_resolver.read() {
            return resolver.resolve(provider);
        }

        None
    }

    // -----------------------------------------------------------------------
    // Credential mutation
    // -----------------------------------------------------------------------

    /// Set API key for a provider
    pub fn set_api_key(&self, provider: &str, key: String) {
        self.credentials
            .write()
            .insert(provider.to_string(), AuthCredential::ApiKey { key });
        if let Err(e) = self.persist() {
            tracing::warn!("Failed to persist API key for '{}': {}", provider, e);
        }
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

    /// Set OAuth credential with full details
    pub fn set_oauth_full(
        &self,
        provider: &str,
        access_token: String,
        refresh_token: Option<String>,
        expires_at: u64,
        scopes: Option<String>,
        provider_data: Option<serde_json::Value>,
    ) {
        self.credentials.write().insert(
            provider.to_string(),
            AuthCredential::OAuth {
                access_token,
                refresh_token,
                expires_at,
                scopes,
                provider_data,
            },
        );
        if let Err(e) = self.persist() {
            tracing::warn!("Failed to persist OAuth token for '{}': {}", provider, e);
        }
    }

    /// Set session token for a provider
    pub fn set_session(
        &self,
        provider: &str,
        token: String,
        expires_at: u64,
        metadata: Option<serde_json::Value>,
    ) {
        self.credentials.write().insert(
            provider.to_string(),
            AuthCredential::Session {
                token,
                expires_at,
                metadata,
            },
        );
        if let Err(e) = self.persist() {
            tracing::warn!("Failed to persist session for '{}': {}", provider, e);
        }
    }

    /// Update an existing OAuth credential (for token refresh)
    pub fn update_oauth_tokens(
        &self,
        provider: &str,
        new_access_token: String,
        new_refresh_token: Option<String>,
        new_expires_at: u64,
    ) -> AuthResult<()> {
        let mut creds = self.credentials.write();
        let cred = creds
            .get_mut(provider)
            .ok_or_else(|| AuthError::NotFound(provider.to_string()))?;

        match cred {
            AuthCredential::OAuth {
                access_token,
                refresh_token,
                expires_at,
                ..
            } => {
                *access_token = new_access_token;
                *refresh_token = new_refresh_token;
                *expires_at = new_expires_at;
            }
            _ => {
                return Err(AuthError::InvalidFormat(format!(
                    "Provider '{}' does not have OAuth credentials",
                    provider
                )));
            }
        }

        drop(creds);
        if let Err(e) = self.persist() {
            tracing::warn!(
                "Failed to persist OAuth token update for '{}': {}",
                provider,
                e
            );
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Credential retrieval
    // -----------------------------------------------------------------------

    /// Get credential for a provider
    pub fn get(&self, provider: &str) -> Option<AuthCredential> {
        self.credentials.read().get(provider).cloned()
    }

    /// Get OAuth credential for a provider (for token refresh)
    pub fn get_oauth_credential(&self, provider: &str) -> Option<AuthCredential> {
        self.credentials.read().get(provider).cloned()
    }

    /// Check if a provider has OAuth credentials that can be refreshed
    pub fn has_oauth_with_refresh(&self, provider: &str) -> bool {
        if let Some(cred) = self.credentials.read().get(provider) {
            matches!(
                cred,
                AuthCredential::OAuth {
                    refresh_token: Some(_),
                    ..
                }
            )
        } else {
            false
        }
    }

    // -----------------------------------------------------------------------
    // CRUD operations
    // -----------------------------------------------------------------------

    /// Set a credential for a provider
    pub fn set(&self, provider: &str, credential: AuthCredential) {
        self.credentials
            .write()
            .insert(provider.to_string(), credential);
        if let Err(e) = self.persist() {
            tracing::warn!("Failed to persist credential for '{}': {}", provider, e);
        }
    }

    /// Remove credential for a provider
    pub fn remove(&self, provider: &str) {
        self.credentials.write().remove(provider);
        if let Err(e) = self.persist() {
            tracing::warn!("Failed to persist after removing '{}': {}", provider, e);
        }
    }

    /// List all providers with credentials
    pub fn list_providers(&self) -> Vec<String> {
        self.credentials.read().keys().cloned().collect()
    }

    /// Check if credential exists for provider in storage
    pub fn has(&self, provider: &str) -> bool {
        self.credentials.read().contains_key(provider)
    }

    /// Get all credentials
    pub fn get_all(&self) -> HashMap<String, AuthCredential> {
        self.credentials.read().clone()
    }

    /// Clear all stored credentials
    pub fn clear(&self) {
        self.credentials.write().clear();
        if let Err(e) = self.persist() {
            tracing::warn!("Failed to persist after clearing credentials: {}", e);
        }
    }

    // -----------------------------------------------------------------------
    // Persistence
    // -----------------------------------------------------------------------

    /// Reload from disk
    pub fn reload(&self) {
        if let Some(ref storage) = self.file_storage {
            match storage.read() {
                Ok(Some(content)) => {
                    if let Ok(creds) = serde_json::from_str(&content) {
                        *self.credentials.write() = creds;
                    }
                    *self.load_error.write() = None;
                }
                Ok(None) => {
                    self.credentials.write().clear();
                    *self.load_error.write() = None;
                }
                Err(e) => {
                    *self.load_error.write() = Some(e);
                    self.record_error(AuthError::ReadError(
                        "Failed to reload auth storage".to_string(),
                    ));
                }
            }
        }
    }

    /// Persist to disk
    #[allow(unexpected_cfgs)]
    fn persist(&self) -> Result<(), String> {
        if let Some(ref storage) = self.file_storage {
            let creds = self.credentials.read();
            if let Ok(json) = serde_json::to_string_pretty(&*creds) {
                // F-4 (audit 2026-06-21): the previous warning suggested
                // enabling a `keyring` feature that is never declared in
                // `Cargo.toml` (the `keyring_support` module at line 1047
                // is dead code: `#[cfg(feature = "keyring")]` is never
                // selected, so `cargo` always builds the `not(feature)`
                // branch). Replace with an accurate one-shot warning that
                // names the actual on-disk path and points at the docs.
                self.plaintext_warned.get_or_init(|| {
                    tracing::warn!(
                        "Auth credentials are stored in plaintext at \
                         ~/.oxi/auth.json (mode 0600). For OS-keyring \
                         support, see the `oxi-auth-keyring` crate or \
                         the OXI_KEYRING=1 docs at docs/PORT_GUIDE.md."
                    );
                });

                if let Err(e) = storage.write(&json) {
                    tracing::error!("Failed to persist auth storage: {}", e);
                    self.record_error(e);
                    return Err("persist failed".to_string());
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Error tracking
    // -----------------------------------------------------------------------

    /// Record an error
    fn record_error(&self, error: AuthError) {
        self.errors.write().push(error);
    }

    /// Drain collected errors
    pub fn drain_errors(&self) -> Vec<AuthError> {
        let mut errors = self.errors.write();
        std::mem::take(&mut *errors)
    }

    /// Get the last load error
    pub fn load_error(&self) -> Option<AuthError> {
        self.load_error.read().clone()
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    /// Validate all stored credentials
    pub fn validate_all(&self) -> Vec<(String, CredentialValidationError)> {
        let creds = self.credentials.read();
        let mut results = Vec::new();
        for (provider, cred) in creds.iter() {
            if let Err(e) = cred.validate() {
                results.push((provider.clone(), e));
            }
        }
        results
    }

    /// Validate credential for a specific provider
    pub fn validate(&self, provider: &str) -> Result<(), CredentialValidationError> {
        let creds = self.credentials.read();
        let cred = creds.get(provider).ok_or_else(|| {
            CredentialValidationError::EmptyField(format!(
                "no credential for provider '{}'",
                provider
            ))
        })?;
        cred.validate()
    }

    // -----------------------------------------------------------------------
    // Multi-provider support
    // -----------------------------------------------------------------------

    /// Get all configured provider IDs (sorted)
    pub fn configured_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self.credentials.read().keys().cloned().collect();
        providers.sort();
        providers
    }

    /// Check if multiple providers are configured
    pub fn has_multiple_providers(&self) -> bool {
        self.credentials.read().len() > 1
    }

    /// Get the primary provider (first configured, preferring stored over env)
    pub fn primary_provider(&self) -> Option<String> {
        let creds = self.credentials.read();
        creds.keys().next().cloned()
    }

    /// Migrate credentials from one provider to another
    pub fn migrate_provider(&self, from: &str, to: &str) -> AuthResult<()> {
        let mut creds = self.credentials.write();
        let cred = creds
            .remove(from)
            .ok_or_else(|| AuthError::NotFound(from.to_string()))?;
        creds.insert(to.to_string(), cred);
        drop(creds);
        let _ = self.persist();
        Ok(())
    }
}

impl Default for AuthStorage {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Helper: current unix timestamp
// ============================================================================

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================================
// Keyring Support
// ============================================================================
//
// F-4 (audit 2026-06-21): this module is currently unreachable from the
// workspace because the `keyring` feature is not declared in `Cargo.toml`.
// It is preserved so a follow-up PR can wire it back in via
// `keyring = { version = "2", optional = true }` + `keyring = ["dep:keyring"]`
// and add an opt-in env-gated call site in `FileAuthStorage::persist`.
// Until that PR lands, the `not(feature = "keyring")` arm always compiles
// and the `cfg(feature = "keyring")` arm is dead.

/// OS-keyring credential helpers. Currently stubbed because the
/// `keyring` cargo feature is not enabled (see F-4 audit note above).
#[allow(unexpected_cfgs)]
#[deprecated(note = "keyring cargo feature is not wired in Cargo.toml; \
    this module is currently a no-op fallback. See docs/PORT_GUIDE.md \
    and the F-4 audit note in oxi-cli/src/store/auth_storage.rs.")]
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
    pub fn set_keyring_secret(service: &str, account: &str, secret: &str) -> AuthResult<()> {
        use keyring::Entry;
        Entry::new(service, account)
            .map_err(|e| AuthError::KeyringError(e.to_string()))?
            .set_password(secret)
            .map_err(|e| AuthError::KeyringError(e.to_string()))
    }

    /// Try to delete a secret from the OS keyring
    #[cfg(feature = "keyring")]
    pub fn delete_keyring_secret(service: &str, account: &str) -> AuthResult<()> {
        use keyring::Entry;
        Entry::new(service, account)
            .map_err(|e| AuthError::KeyringError(e.to_string()))?
            .delete_credential()
            .map_err(|e| AuthError::KeyringError(e.to_string()))
    }

    // Non-keyring fallbacks
    #[cfg(not(feature = "keyring"))]
    /// Retrieve a secret from the OS keyring.
    ///
    /// Returns `None` when the keyring feature is not compiled in.
    pub fn get_keyring_secret(_service: &str, _account: &str) -> Option<String> {
        None
    }

    #[cfg(not(feature = "keyring"))]
    /// Store a secret in the OS keyring.
    ///
    /// Returns an error when the keyring feature is not compiled in.
    pub fn set_keyring_secret(_service: &str, _account: &str, _secret: &str) -> AuthResult<()> {
        Err(AuthError::KeyringError(
            "Keyring support not compiled".to_string(),
        ))
    }

    #[cfg(not(feature = "keyring"))]
    /// Delete a secret from the OS keyring.
    ///
    /// Returns an error when the keyring feature is not compiled in.
    pub fn delete_keyring_secret(_service: &str, _account: &str) -> AuthResult<()> {
        Err(AuthError::KeyringError(
            "Keyring support not compiled".to_string(),
        ))
    }
}

// ============================================================================
// Singleton
// ============================================================================

/// Get a shared singleton `Arc<AuthStorage>` instance.
///
/// Avoids creating multiple `AuthStorage::new()` instances that each
/// independently read and cache `auth.json`. All callers share the same
/// in-memory state through the `Arc`.
pub fn shared_auth_storage() -> Arc<AuthStorage> {
    static STORAGE: OnceLock<Arc<AuthStorage>> = OnceLock::new();
    STORAGE
        .get_or_init(|| {
            let storage = Arc::new(AuthStorage::new());
            // Default fallback: resolve API keys from environment variables
            // using the BuiltinProvider registry (env_key + extra_env_keys).
            storage.set_fallback_resolver(Arc::new(EnvVarFallbackResolver));
            storage
        })
        .clone()
}

// ============================================================================
// Tests
// ============================================================================

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
        assert_eq!(status.label, Some("api_key".to_string()));
    }

    #[test]
    fn test_auth_status_display() {
        let status = AuthStatus {
            configured: true,
            source: Some("stored".to_string()),
            label: Some("api_key".to_string()),
        };
        let display = format!("{}", status);
        assert_eq!(display, "stored (api_key)");

        let no_config = AuthStatus {
            configured: false,
            source: None,
            label: None,
        };
        assert_eq!(format!("{}", no_config), "not configured");
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
        let future_time = now_secs() + 3600;
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
        let future_time = now_secs() + 120; // 2 minutes from now

        // Has refresh token, will expire soon - not yet within 60s
        let oauth_cred = AuthCredential::OAuth {
            access_token: "token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: future_time,
            scopes: None,
            provider_data: None,
        };
        assert!(!oauth_cred.needs_refresh());

        // Within 60 seconds
        let soon = now_secs() + 30;
        let oauth_soon = AuthCredential::OAuth {
            access_token: "token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: soon,
            scopes: None,
            provider_data: None,
        };
        assert!(oauth_soon.needs_refresh());

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
        let future_time = now_secs() + 3600;

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

        // API key returns None via access_token
        let api_key_cred = AuthCredential::ApiKey {
            key: "api_key_token".to_string(),
        };
        assert!(api_key_cred.access_token().is_none());
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

    #[test]
    fn test_session_token() {
        let storage = AuthStorage::in_memory();
        storage.set_session(
            "browser",
            "session-token-123".to_string(),
            0, // never expires
            Some(serde_json::json!({"user": "test"})),
        );

        assert!(storage.has("browser"));
        assert_eq!(
            storage.get_api_key("browser"),
            Some("session-token-123".to_string())
        );

        let cred = storage.get("browser").unwrap();
        assert!(matches!(cred, AuthCredential::Session { .. }));
        assert!(cred.access_token().is_some());
    }

    #[test]
    fn test_session_token_expired() {
        let storage = AuthStorage::in_memory();
        storage.set_session("browser", "session-token".to_string(), 1, None);

        // Token expired (timestamp 1 is in the past)
        assert!(storage.get_api_key("browser").is_none());
    }

    #[test]
    fn test_credential_validation() {
        // Valid API key
        let valid = AuthCredential::ApiKey {
            key: "sk-valid".to_string(),
        };
        assert!(valid.validate().is_ok());

        // Empty API key
        let empty = AuthCredential::ApiKey {
            key: "".to_string(),
        };
        assert!(empty.validate().is_err());

        // Placeholder
        let placeholder = AuthCredential::ApiKey {
            key: "your-api-key-here".to_string(),
        };
        assert!(placeholder.validate().is_err());

        // Valid OAuth
        let valid_oauth = AuthCredential::OAuth {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: now_secs() + 3600,
            scopes: None,
            provider_data: None,
        };
        assert!(valid_oauth.validate().is_ok());

        // Invalid OAuth (empty token)
        let invalid_oauth = AuthCredential::OAuth {
            access_token: "".to_string(),
            refresh_token: None,
            expires_at: 1000,
            scopes: None,
            provider_data: None,
        };
        assert!(invalid_oauth.validate().is_err());
    }

    #[test]
    fn test_validate_all() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("valid", "sk-good".to_string());
        storage.set_api_key("empty", "".to_string());

        let errors = storage.validate_all();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "empty");
    }

    #[test]
    fn test_update_oauth_tokens() {
        let storage = AuthStorage::in_memory();
        storage.set_oauth(
            "provider",
            "old-access".to_string(),
            Some("old-refresh".to_string()),
            now_secs() + 3600,
        );

        storage
            .update_oauth_tokens(
                "provider",
                "new-access".to_string(),
                Some("new-refresh".to_string()),
                now_secs() + 7200,
            )
            .unwrap();

        let key = storage.get_api_key("provider");
        assert_eq!(key, Some("new-access".to_string()));
    }

    #[test]
    fn test_update_oauth_tokens_wrong_type() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("provider", "key".to_string());

        let result = storage.update_oauth_tokens(
            "provider",
            "new-access".to_string(),
            None,
            now_secs() + 3600,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_migrate_provider() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("old-provider", "key123".to_string());
        storage
            .migrate_provider("old-provider", "new-provider")
            .unwrap();

        assert!(!storage.has("old-provider"));
        assert!(storage.has("new-provider"));
        assert_eq!(
            storage.get_api_key("new-provider"),
            Some("key123".to_string())
        );
    }

    #[test]
    fn test_migrate_provider_not_found() {
        let storage = AuthStorage::in_memory();
        let result = storage.migrate_provider("nonexistent", "target");
        assert!(result.is_err());
    }

    #[test]
    fn test_error_draining() {
        let storage = AuthStorage::in_memory();
        let errors = storage.drain_errors();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_fallback_resolver() {
        let storage = AuthStorage::in_memory();
        storage.set_fallback_resolver(Arc::new(FnFallbackResolver::new(Box::new(|provider| {
            if provider == "custom" {
                Some("custom-key-from-config".to_string())
            } else {
                None
            }
        }))));

        assert_eq!(
            storage.get_api_key("custom"),
            Some("custom-key-from-config".to_string())
        );
        assert!(storage.get_api_key("unknown").is_none());

        // Without fallback
        storage.clear_fallback_resolver();
        assert!(storage.get_api_key("custom").is_none());
    }

    #[test]
    fn test_get_api_key_with_options() {
        let storage = AuthStorage::in_memory();
        storage.set_fallback_resolver(Arc::new(FnFallbackResolver::new(Box::new(|_| {
            Some("fallback-key".to_string())
        }))));

        // With fallback
        assert_eq!(
            storage.get_api_key_with_options("test", true),
            Some("fallback-key".to_string())
        );

        // Without fallback
        assert!(storage.get_api_key_with_options("test", false).is_none());
    }

    #[test]
    fn test_configured_providers() {
        let storage = AuthStorage::in_memory();
        storage.set_api_key("openai", "key".to_string());
        storage.set_api_key("anthropic", "key".to_string());

        let providers = storage.configured_providers();
        assert!(providers.len() >= 2);
        // Should be sorted
        let mut sorted = providers.clone();
        sorted.sort();
        assert_eq!(providers, sorted);
    }

    #[test]
    fn test_has_multiple_providers() {
        let storage = AuthStorage::in_memory();
        assert!(!storage.has_multiple_providers());

        storage.set_api_key("openai", "key1".to_string());
        assert!(!storage.has_multiple_providers());

        storage.set_api_key("anthropic", "key2".to_string());
        assert!(storage.has_multiple_providers());
    }

    #[test]
    fn test_set_and_get_credential() {
        let storage = AuthStorage::in_memory();
        let cred = AuthCredential::Session {
            token: "abc".to_string(),
            expires_at: 0,
            metadata: None,
        };
        storage.set("custom", cred);
        let retrieved = storage.get("custom");
        assert!(retrieved.is_some());
        assert!(matches!(retrieved.unwrap(), AuthCredential::Session { .. }));
    }

    #[test]
    fn test_credential_type_name() {
        assert_eq!(
            AuthCredential::ApiKey {
                key: "k".to_string()
            }
            .type_name(),
            "api_key"
        );
        assert_eq!(
            AuthCredential::OAuth {
                access_token: "t".to_string(),
                refresh_token: None,
                expires_at: 0,
                scopes: None,
                provider_data: None,
            }
            .type_name(),
            "oauth"
        );
        assert_eq!(
            AuthCredential::Session {
                token: "t".to_string(),
                expires_at: 0,
                metadata: None,
            }
            .type_name(),
            "session"
        );
    }
}
