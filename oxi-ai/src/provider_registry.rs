//! Provider authentication registry
//!
//! Manages authentication for multiple providers with a unified interface.
//! Supports API keys, OAuth tokens, and ambient credentials (AWS IAM, Google ADC).

use crate::env_api_keys;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

/// Information about an OAuth token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenInfo {
    /// Access token
    pub access_token: String,
    /// Refresh token (if available)
    pub refresh_token: Option<String>,
    /// Expiration timestamp (Unix epoch seconds)
    pub expires_at: i64,
    /// Token type (usually "Bearer")
    pub token_type: String,
}

impl OAuthTokenInfo {
    /// Check if the token is expired
    pub fn is_expired(&self) -> bool {
        let now = Utc::now().timestamp();
        now >= self.expires_at
    }

    /// Check if the token needs refresh (within 5 minutes of expiration)
    pub fn needs_refresh(&self) -> bool {
        let now = Utc::now().timestamp();
        now >= (self.expires_at - 300) // 5 minutes buffer
    }

    /// Create a new OAuth token info
    pub fn new(access_token: String, refresh_token: Option<String>, expires_in_secs: i64) -> Self {
        let now = Utc::now().timestamp();
        Self {
            access_token,
            refresh_token,
            expires_at: now + expires_in_secs,
            token_type: "Bearer".to_string(),
        }
    }
}

/// Trait for provider-specific authentication
pub trait ProviderAuth: Send + Sync {
    /// Get the provider name
    fn provider_name(&self) -> &str;

    /// Check if auth is configured (API key, OAuth, or ambient)
    fn is_configured(&self) -> bool;

    /// Get the current API key (may involve refresh for OAuth)
    fn get_api_key(&self) -> Option<String>;

    /// Check if OAuth is configured and needs refresh
    fn needs_oauth_refresh(&self) -> bool;

    /// Get OAuth token info
    fn get_oauth_token(&self) -> Option<OAuthTokenInfo>;

    /// Set OAuth token
    fn set_oauth_token(&mut self, token: OAuthTokenInfo);

    /// Set API key directly
    fn set_api_key(&mut self, api_key: String);
}

/// API key based authentication
#[derive(Debug, Clone)]
pub struct ApiKeyAuth {
    api_key: Option<String>,
    source: AuthSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthSource {
    Stored,
    Runtime,
    Environment,
    Ambient,
}

impl ApiKeyAuth {
    pub fn new(api_key: Option<String>, source: AuthSource) -> Self {
        Self { api_key, source }
    }
}

impl ProviderAuth for ApiKeyAuth {
    fn provider_name(&self) -> &str {
        "api_key"
    }

    fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    fn get_api_key(&self) -> Option<String> {
        self.api_key.clone()
    }

    fn needs_oauth_refresh(&self) -> bool {
        false
    }

    fn get_oauth_token(&self) -> Option<OAuthTokenInfo> {
        None
    }

    fn set_oauth_token(&mut self, _token: OAuthTokenInfo) {
        // Not applicable for API key auth
    }

    fn set_api_key(&mut self, api_key: String) {
        self.api_key = Some(api_key);
        self.source = AuthSource::Stored;
    }
}

/// OAuth-based authentication with auto-refresh support
pub struct OAuthAuth {
    provider_name: String,
    token: Option<OAuthTokenInfo>,
    on_refresh: Option<Box<dyn Fn(&OAuthTokenInfo) + Send + Sync>>,
}

impl OAuthAuth {
    pub fn new(provider_name: &str) -> Self {
        Self {
            provider_name: provider_name.to_string(),
            token: None,
            on_refresh: None,
        }
    }

    pub fn with_token(provider_name: &str, token: OAuthTokenInfo) -> Self {
        Self {
            provider_name: provider_name.to_string(),
            token: Some(token),
            on_refresh: None,
        }
    }

    /// Set a callback to be called when the token is refreshed
    pub fn on_token_refresh<F>(&mut self, callback: F)
    where
        F: Fn(&OAuthTokenInfo) + Send + Sync + 'static,
    {
        self.on_refresh = Some(Box::new(callback));
    }
}

impl ProviderAuth for OAuthAuth {
    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    fn is_configured(&self) -> bool {
        self.token.is_some()
    }

    fn get_api_key(&self) -> Option<String> {
        self.token.as_ref().map(|t| t.access_token.clone())
    }

    fn needs_oauth_refresh(&self) -> bool {
        self.token
            .as_ref()
            .map(|t| t.needs_refresh())
            .unwrap_or(true)
    }

    fn get_oauth_token(&self) -> Option<OAuthTokenInfo> {
        self.token.clone()
    }

    fn set_oauth_token(&mut self, token: OAuthTokenInfo) {
        if let Some(ref callback) = self.on_refresh {
            callback(&token);
        }
        self.token = Some(token);
    }

    fn set_api_key(&mut self, _api_key: String) {
        // OAuth auth doesn't use direct API keys
    }
}

/// Ambient credential authentication (AWS IAM, Google ADC, etc.)
pub struct AmbientAuth {
    provider_name: String,
    check_fn: Box<dyn Fn() -> bool + Send + Sync>,
}

impl AmbientAuth {
    pub fn new<F>(provider_name: &str, check_fn: F) -> Self
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        Self {
            provider_name: provider_name.to_string(),
            check_fn: Box::new(check_fn),
        }
    }
}

impl ProviderAuth for AmbientAuth {
    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    fn is_configured(&self) -> bool {
        (self.check_fn)()
    }

    fn get_api_key(&self) -> Option<String> {
        if (self.check_fn)() {
            Some("<authenticated>".to_string())
        } else {
            None
        }
    }

    fn needs_oauth_refresh(&self) -> bool {
        false
    }

    fn get_oauth_token(&self) -> Option<OAuthTokenInfo> {
        None
    }

    fn set_oauth_token(&mut self, _token: OAuthTokenInfo) {
        // Not applicable for ambient auth
    }

    fn set_api_key(&mut self, _api_key: String) {
        // Ambient auth doesn't use direct API keys
    }
}

/// Provider authentication registry
///
/// Manages authentication for multiple providers with priority-based resolution:
/// 1. Runtime override (CLI --api-key)
/// 2. Stored credential (auth.json)
/// 3. OAuth token (with auto-refresh)
/// 4. Environment variable
/// 5. Ambient credentials (AWS IAM, Google ADC)
pub struct ProviderAuthRegistry {
    providers: HashMap<String, Box<dyn ProviderAuth>>,
    runtime_overrides: RwLock<HashMap<String, String>>,
    fallback_resolver: RwLock<Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>>,
}

impl Default for ProviderAuthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAuthRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            runtime_overrides: RwLock::new(HashMap::new()),
            fallback_resolver: RwLock::new(None),
        }
    }

    /// Create a registry with default providers pre-registered
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register_defaults();
        registry
    }

    /// Register default providers
    pub fn register_defaults(&mut self) {
        // Register ambient auth providers
        self.register_ambient("vertex", || env_api_keys::has_vertex_adc_full());
        self.register_ambient("google-vertex", || env_api_keys::has_vertex_adc_full());
        self.register_ambient("bedrock", || env_api_keys::has_bedrock_creds());
        self.register_ambient("amazon-bedrock", || env_api_keys::has_bedrock_creds());
        self.register_ambient("aws-bedrock", || env_api_keys::has_bedrock_creds());
    }

    /// Register an API key provider
    pub fn register_api_key(&mut self, provider: &str, api_key: Option<String>) {
        self.providers.insert(
            provider.to_string(),
            Box::new(ApiKeyAuth::new(api_key, AuthSource::Stored)),
        );
    }

    /// Register an OAuth provider
    pub fn register_oauth(&mut self, provider: &str, token: OAuthTokenInfo) {
        self.providers.insert(
            provider.to_string(),
            Box::new(OAuthAuth::with_token(provider, token)),
        );
    }

    /// Register an ambient credential provider
    pub fn register_ambient<F>(&mut self, provider: &str, check_fn: F)
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        self.providers.insert(
            provider.to_string(),
            Box::new(AmbientAuth::new(provider, check_fn)),
        );
    }

    /// Register a custom provider auth implementation
    pub fn register<P: ProviderAuth + 'static>(&mut self, provider: &str, auth: P) {
        self.providers.insert(provider.to_string(), Box::new(auth));
    }

    /// Set a runtime API key override
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

    /// Set a fallback resolver for providers not in the registry
    pub fn set_fallback_resolver<F>(&self, resolver: F)
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        *self.fallback_resolver.write().unwrap() = Some(Box::new(resolver));
    }

    /// Remove fallback resolver
    pub fn clear_fallback_resolver(&self) {
        *self.fallback_resolver.write().unwrap() = None;
    }

    /// Get API key for a provider using the resolution chain
    ///
    /// Priority:
    /// 1. Runtime override
    /// 2. Stored credential (API key or OAuth)
    /// 3. Environment variable
    /// 4. Fallback resolver
    pub fn get_api_key(&self, provider: &str) -> Option<String> {
        // 1. Runtime override takes highest priority
        {
            let overrides = self.runtime_overrides.read().unwrap();
            if let Some(key) = overrides.get(provider) {
                return Some(key.clone());
            }
        }

        // 2. Check registered provider
        if let Some(auth) = self.providers.get(provider) {
            if let Some(key) = auth.get_api_key() {
                return Some(key);
            }
        }

        // 3. Check environment variable
        if let Some(key) = env_api_keys::get_env_api_key(provider) {
            return Some(key);
        }

        // 4. Fallback resolver
        {
            let resolver = self.fallback_resolver.read().unwrap();
            if let Some(ref fallback) = *resolver {
                return fallback(provider);
            }
        }

        None
    }

    /// Check if any auth is configured for a provider
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

        // Check registered provider
        if let Some(auth) = self.providers.get(provider) {
            if auth.is_configured() {
                return true;
            }
        }

        // Check environment variable
        if env_api_keys::has_env_key(provider) {
            return true;
        }

        false
    }

    /// Check if an OAuth provider needs refresh
    pub fn needs_oauth_refresh(&self, provider: &str) -> bool {
        self.providers
            .get(provider)
            .map(|auth| auth.needs_oauth_refresh())
            .unwrap_or(false)
    }

    /// Update OAuth token for a provider
    pub fn set_oauth_token(&mut self, provider: &str, token: OAuthTokenInfo) {
        if let Some(auth) = self.providers.get_mut(provider) {
            auth.set_oauth_token(token);
        }
    }

    /// Update API key directly
    pub fn set_api_key(&mut self, provider: &str, api_key: String) {
        if let Some(auth) = self.providers.get_mut(provider) {
            auth.set_api_key(api_key);
        } else {
            // Create new API key auth
            self.register_api_key(provider, Some(api_key));
        }
    }

    /// List all providers with configured auth
    pub fn list_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self
            .providers
            .iter()
            .filter(|(_, auth)| auth.is_configured())
            .map(|(name, _)| name.clone())
            .collect();

        // Add runtime overrides
        let overrides: Vec<String> = self
            .runtime_overrides
            .read()
            .unwrap()
            .keys()
            .cloned()
            .collect();

        for key in overrides {
            if !providers.contains(&key) {
                providers.push(key);
            }
        }

        providers.sort();
        providers.dedup();
        providers
    }

    /// Get auth status for a provider
    pub fn get_auth_status(&self, provider: &str) -> AuthStatus {
        if self
            .runtime_overrides
            .read()
            .unwrap()
            .contains_key(provider)
        {
            return AuthStatus {
                configured: true,
                source: AuthSource::Runtime,
                label: Some("--api-key".to_string()),
            };
        }

        if let Some(auth) = self.providers.get(provider) {
            if auth.is_configured() {
                return AuthStatus {
                    configured: true,
                    source: AuthSource::Stored,
                    label: None,
                };
            }
        }

        if env_api_keys::has_env_key(provider) {
            return AuthStatus {
                configured: false,
                source: AuthSource::Environment,
                label: Some(provider.to_string()),
            };
        }

        if let Some(keys) = env_api_keys::find_env_keys(provider) {
            return AuthStatus {
                configured: false,
                source: AuthSource::Environment,
                label: Some(keys[0].to_string()),
            };
        }

        AuthStatus {
            configured: false,
            source: AuthSource::Ambient,
            label: None,
        }
    }
}

/// Authentication status
#[derive(Debug, Clone)]
pub struct AuthStatus {
    pub configured: bool,
    pub source: AuthSource,
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_token_info_expired() {
        // Create token that expires immediately
        let token = OAuthTokenInfo::new(
            "access".to_string(),
            Some("refresh".to_string()),
            -1, // Already expired
        );

        assert!(token.is_expired());
        assert!(token.needs_refresh());
    }

    #[test]
    fn test_oauth_token_info_valid() {
        // Create token that expires in 1 hour
        let token = OAuthTokenInfo::new("access".to_string(), Some("refresh".to_string()), 3600);

        assert!(!token.is_expired());
        assert!(!token.needs_refresh());
    }

    #[test]
    fn test_oauth_token_info_needs_refresh_buffer() {
        // Create token that expires in 2 minutes (within 5 min buffer)
        let token = OAuthTokenInfo::new("access".to_string(), Some("refresh".to_string()), 120);

        assert!(!token.is_expired());
        assert!(token.needs_refresh());
    }

    #[test]
    fn test_api_key_auth() {
        let auth = ApiKeyAuth::new(Some("sk-test".to_string()), AuthSource::Stored);

        assert!(auth.is_configured());
        assert_eq!(auth.get_api_key(), Some("sk-test".to_string()));
        assert!(!auth.needs_oauth_refresh());
    }

    #[test]
    fn test_api_key_auth_not_configured() {
        let auth = ApiKeyAuth::new(None, AuthSource::Environment);

        assert!(!auth.is_configured());
        assert!(auth.get_api_key().is_none());
    }

    #[test]
    fn test_oauth_auth() {
        let token = OAuthTokenInfo::new(
            "access_token".to_string(),
            Some("refresh_token".to_string()),
            3600,
        );
        let auth = OAuthAuth::with_token("anthropic", token);

        assert!(auth.is_configured());
        assert_eq!(auth.get_api_key(), Some("access_token".to_string()));
        assert_eq!(auth.provider_name(), "anthropic");
    }

    #[test]
    fn test_oauth_auth_refresh_callback() {
        let mut auth = OAuthAuth::new("anthropic");

        let refreshed = std::sync::Arc::new(std::sync::Mutex::new(false));
        let refreshed_clone = refreshed.clone();
        auth.on_token_refresh(move |_| {
            *refreshed_clone.lock().unwrap() = true;
        });

        let new_token = OAuthTokenInfo::new("new_access".to_string(), None, 3600);
        auth.set_oauth_token(new_token);

        assert!(*refreshed.lock().unwrap());
        assert_eq!(auth.get_api_key(), Some("new_access".to_string()));
    }

    #[test]
    fn test_ambient_auth() {
        let auth = AmbientAuth::new("bedrock", || true);

        assert!(auth.is_configured());
        assert_eq!(auth.get_api_key(), Some("<authenticated>".to_string()));
    }

    #[test]
    fn test_ambient_auth_not_configured() {
        let auth = AmbientAuth::new("bedrock", || false);

        assert!(!auth.is_configured());
        assert!(auth.get_api_key().is_none());
    }

    #[test]
    fn test_registry_new() {
        let registry = ProviderAuthRegistry::new();

        assert!(registry.list_providers().is_empty());
        assert!(!registry.has_auth("openai"));
    }

    #[test]
    fn test_registry_with_defaults() {
        let registry = ProviderAuthRegistry::with_defaults();

        // Ambient providers should be registered
        assert!(registry.providers.contains_key("bedrock"));
        assert!(registry.providers.contains_key("vertex"));
    }

    #[test]
    fn test_registry_runtime_override() {
        let registry = ProviderAuthRegistry::new();

        // Set runtime override
        registry.set_runtime_key("openai", "sk-runtime".to_string());

        // Runtime should take priority
        assert_eq!(
            registry.get_api_key("openai"),
            Some("sk-runtime".to_string())
        );
        assert!(registry.has_auth("openai"));
    }

    #[test]
    fn test_registry_remove_runtime_key() {
        let registry = ProviderAuthRegistry::new();

        registry.set_runtime_key("openai", "sk-runtime".to_string());
        assert_eq!(
            registry.get_api_key("openai"),
            Some("sk-runtime".to_string())
        );

        registry.remove_runtime_key("openai");
        assert!(registry.get_api_key("openai").is_none());
    }

    #[test]
    fn test_registry_register_api_key() {
        let mut registry = ProviderAuthRegistry::new();
        registry.register_api_key("anthropic", Some("sk-stored".to_string()));

        assert!(registry.has_auth("anthropic"));
        assert_eq!(
            registry.get_api_key("anthropic"),
            Some("sk-stored".to_string())
        );
    }

    #[test]
    fn test_registry_register_oauth() {
        let mut registry = ProviderAuthRegistry::new();
        let token = OAuthTokenInfo::new(
            "oauth-access".to_string(),
            Some("refresh".to_string()),
            3600,
        );
        registry.register_oauth("anthropic", token);

        assert!(registry.has_auth("anthropic"));
        assert_eq!(
            registry.get_api_key("anthropic"),
            Some("oauth-access".to_string())
        );
    }

    #[test]
    fn test_registry_env_key_fallback() {
        std::env::set_var("OPENAI_API_KEY", "sk-env-key");

        let registry = ProviderAuthRegistry::new();
        // No explicit provider registered

        // Should fall back to environment
        assert_eq!(
            registry.get_api_key("openai"),
            Some("sk-env-key".to_string())
        );

        std::env::remove_var("OPENAI_API_KEY");
    }

    #[test]
    fn test_registry_fallback_resolver() {
        let registry = ProviderAuthRegistry::new();

        registry.set_fallback_resolver(|provider| {
            if provider == "custom" {
                Some("custom-key".to_string())
            } else {
                None
            }
        });

        assert_eq!(
            registry.get_api_key("custom"),
            Some("custom-key".to_string())
        );
        assert!(registry.get_api_key("unknown").is_none());
    }

    #[test]
    fn test_registry_priority() {
        std::env::set_var("ANTHROPIC_API_KEY", "sk-env");

        let mut registry = ProviderAuthRegistry::new();

        // Register stored key
        registry.register_api_key("anthropic", Some("sk-stored".to_string()));
        registry.set_runtime_key("anthropic", "sk-runtime".to_string());

        // Runtime should win
        assert_eq!(
            registry.get_api_key("anthropic"),
            Some("sk-runtime".to_string())
        );

        // Remove runtime
        registry.remove_runtime_key("anthropic");

        // Stored should win
        assert_eq!(
            registry.get_api_key("anthropic"),
            Some("sk-stored".to_string())
        );

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn test_registry_list_providers() {
        let mut registry = ProviderAuthRegistry::new();

        registry.register_api_key("openai", Some("key1".to_string()));
        registry.register_oauth(
            "anthropic",
            OAuthTokenInfo::new("access".to_string(), None, 3600),
        );
        registry.set_runtime_key("google", "runtime-key".to_string());

        let providers = registry.list_providers();
        assert!(providers.contains(&"openai".to_string()));
        assert!(providers.contains(&"anthropic".to_string()));
        assert!(providers.contains(&"google".to_string()));
    }

    #[test]
    fn test_registry_get_auth_status() {
        let registry = ProviderAuthRegistry::new();

        registry.set_runtime_key("openai", "key".to_string());

        let status = registry.get_auth_status("openai");
        assert!(status.configured);
        assert_eq!(status.source, AuthSource::Runtime);
        assert_eq!(status.label, Some("--api-key".to_string()));
    }

    #[test]
    fn test_registry_env_source_status() {
        std::env::set_var("DEEPSEEK_API_KEY", "sk-test");

        let registry = ProviderAuthRegistry::new();

        let status = registry.get_auth_status("deepseek");
        assert!(!status.configured); // Env vars don't count as "configured"
        assert_eq!(status.source, AuthSource::Environment);

        std::env::remove_var("DEEPSEEK_API_KEY");
    }

    #[test]
    fn test_registry_update_oauth_token() {
        let mut registry = ProviderAuthRegistry::new();
        let token = OAuthTokenInfo::new("old".to_string(), None, 0);
        registry.register_oauth("anthropic", token);

        // Token should be expired
        assert!(registry.needs_oauth_refresh("anthropic"));

        // Update with fresh token
        let new_token = OAuthTokenInfo::new("new".to_string(), None, 3600);
        registry.set_oauth_token("anthropic", new_token);

        assert!(!registry.needs_oauth_refresh("anthropic"));
        assert_eq!(registry.get_api_key("anthropic"), Some("new".to_string()));
    }
}
