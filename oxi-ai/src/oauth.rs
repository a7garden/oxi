//! OAuth authentication system for oxi-ai
//!
//! Supports PKCE-based OAuth flows for:
//! - Anthropic (authorization code + PKCE)
//! - OpenAI Codex (authorization code + PKCE)
//! - GitHub Copilot (device flow)
//!
//! Token persistence to `<product-home>/auth.json` (`$OXI_HOME` or `~/.oxi`) with secure file permissions.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors produced by the OAuth subsystem.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("IO error: {0}")]
    /// io variant.
    Io(#[from] io::Error),

    #[error("HTTP request failed: {0}")]
    /// http variant.
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    /// json variant.
    Json(#[from] serde_json::Error),

    #[error("Token expired and no refresh_token available")]
    /// no refresh token variant.
    NoRefreshToken,

    #[error("Token refresh failed: {0}")]
    /// refresh failed variant.
    RefreshFailed(String),

    #[error("Device flow polling timed out after {0}s")]
    /// device flow timeout variant.
    DeviceFlowTimeout(u64),

    #[error("Device flow authorization pending")]
    /// device flow pending variant.
    DeviceFlowPending,

    #[error("Device flow rejected by user")]
    /// device flow rejected variant.
    DeviceFlowRejected,

    #[error("Missing environment variable: {0}")]
    /// missing env variant.
    MissingEnv(String),

    #[error("Invalid state: {0}")]
    /// invalid state variant.
    InvalidState(String),
}

type Result<T> = std::result::Result<T, OAuthError>;

// ---------------------------------------------------------------------------
// Token data model
// ---------------------------------------------------------------------------

/// An OAuth token bundle as stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBundle {
    /// The bearer access token.
    pub access_token: String,
    /// Optional refresh token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Token type, e.g. "Bearer".
    #[serde(default = "default_token_type")]
    pub token_type: String,
    /// Time at which the token was obtained.
    pub obtained_at: DateTime<Utc>,
    /// Seconds until expiry (0 = unknown / never expires).
    #[serde(default)]
    pub expires_in: u64,
    /// Granted scopes (space-separated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl TokenBundle {
    /// Returns true when the token is expired (with a 60-second safety margin).
    pub fn is_expired(&self) -> bool {
        if self.expires_in == 0 {
            return false; // never expires / unknown
        }
        let expires_at = self.obtained_at + chrono::Duration::seconds(self.expires_in as i64);
        Utc::now() >= expires_at - chrono::Duration::seconds(60)
    }
}

/// The on-disk auth file structure: a map from provider name to token bundle.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AuthStore {
    /// Map from provider name to token bundle.
    #[serde(flatten)]
    pub tokens: HashMap<String, TokenBundle>,
}

// ---------------------------------------------------------------------------
// File-based token persistence
// ---------------------------------------------------------------------------

/// Returns the default path for the auth store.
///
/// Resolves to `<product-home>/auth.json`, where the product home is
/// `$OXI_HOME` or `~/.oxi` (see [`crate::product_env`]). The parent
/// directory is created on demand.
pub fn default_auth_path() -> Result<PathBuf> {
    let path = crate::product_env::auth_path().ok_or_else(|| {
        OAuthError::InvalidState(
            "Cannot determine product home directory (neither OXI_HOME nor HOME is set)".into(),
        )
    })?;
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(path)
}

/// Load the auth store from disk.
pub fn load_auth_store() -> Result<AuthStore> {
    let path = default_auth_path()?;
    if !path.exists() {
        return Ok(AuthStore::default());
    }
    let data = fs::read_to_string(&path)?;
    let store: AuthStore = serde_json::from_str(&data)?;
    Ok(store)
}

/// Persist the auth store to disk with mode 0o600 where possible.
pub fn save_auth_store(store: &AuthStore) -> Result<()> {
    let path = default_auth_path()?;
    let json = serde_json::to_string_pretty(store)?;

    // Write atomically: temp file → rename.
    let tmp_path = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.flush()?;
        // Set permissions on unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&tmp_path, perms)?;
        }
    }
    fs::rename(&tmp_path, &path)?;
    Ok(())
}

/// Convenience: load a token for a provider key (e.g. "anthropic").
pub fn load_token(provider: &str) -> Result<Option<TokenBundle>> {
    let store = load_auth_store()?;
    Ok(store.tokens.get(provider).cloned())
}

/// Convenience: save a token for a provider key.
pub fn save_token(provider: &str, token: &TokenBundle) -> Result<()> {
    let mut store = load_auth_store()?;
    store.tokens.insert(provider.to_string(), token.clone());
    save_auth_store(&store)
}

/// Remove a stored token.
pub fn remove_token(provider: &str) -> Result<()> {
    let mut store = load_auth_store()?;
    store.tokens.remove(provider);
    save_auth_store(&store)
}

// ---------------------------------------------------------------------------
// PKCE helpers
// ---------------------------------------------------------------------------

/// Generate a cryptographically-random code_verifier (43 chars, RFC 7636 §4.1).
pub fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 32]; // 32 bytes → 43 base64url chars
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Derive the code_challenge from a code_verifier using S256 (SHA-256 + base64url).
pub fn derive_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

// ---------------------------------------------------------------------------
// Provider configuration
// ---------------------------------------------------------------------------

/// Configuration for a PKCE-based authorization-code OAuth provider.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// OAuth authorization endpoint URL.
    pub authorization_endpoint: String,
    /// OAuth token endpoint URL.
    pub token_endpoint: String,
    /// OAuth client identifier.
    pub client_id: String,
    /// OAuth redirect URI.
    pub redirect_uri: String,
    /// Space-separated scopes.
    pub scopes: String,
}

/// Anthropic OAuth configuration.
pub fn anthropic_config() -> Result<OAuthConfig> {
    let client_id = std::env::var("ANTHROPIC_OAUTH_CLIENT_ID")
        .map_err(|_| OAuthError::MissingEnv("ANTHROPIC_OAUTH_CLIENT_ID".into()))?;
    Ok(OAuthConfig {
        authorization_endpoint: "https://console.anthropic.com/api/oauth".into(),
        token_endpoint: "https://console.anthropic.com/api/oauth/token".into(),
        client_id,
        redirect_uri: "http://localhost:8787/callback".into(),
        scopes: "org.api.read org.api.write".into(),
    })
}

/// OpenAI Codex OAuth configuration.
pub fn openai_codex_config() -> Result<OAuthConfig> {
    let client_id = std::env::var("OPENAI_OAUTH_CLIENT_ID")
        .map_err(|_| OAuthError::MissingEnv("OPENAI_OAUTH_CLIENT_ID".into()))?;
    Ok(OAuthConfig {
        authorization_endpoint: "https://auth.openai.com/authorize".into(),
        token_endpoint: "https://auth.openai.com/oauth/token".into(),
        client_id,
        redirect_uri: "http://localhost:8787/callback".into(),
        scopes: "".into(),
    })
}

// ---------------------------------------------------------------------------
// Authorization URL builder (PKCE)
// ---------------------------------------------------------------------------

/// PKCE state produced when starting an authorization flow.
#[derive(Debug, Clone)]
pub struct PkceState {
    /// PKCE code verifier.
    pub code_verifier: String,
    /// PKCE code challenge (S256).
    pub code_challenge: String,
    /// Full authorization URL to redirect the user to.
    pub authorization_url: String,
    /// Opaque state parameter for CSRF protection.
    pub state: String,
}

/// Build a PKCE authorization URL for the given provider config.
pub fn build_authorization_url(config: &OAuthConfig) -> PkceState {
    let code_verifier = generate_code_verifier();
    let code_challenge = derive_code_challenge(&code_verifier);
    let state = generate_state_token();

    let mut url =
        url::Url::parse(&config.authorization_endpoint).expect("invalid authorization endpoint");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    if !config.scopes.is_empty() {
        url.query_pairs_mut().append_pair("scope", &config.scopes);
    }

    PkceState {
        code_verifier,
        code_challenge,
        authorization_url: url.to_string(),
        state,
    }
}

/// Generate an opaque state parameter (22 random base64url chars).
fn generate_state_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// ---------------------------------------------------------------------------
// Token exchange (authorization code → access token)
// ---------------------------------------------------------------------------

/// Exchange an authorization code for a token bundle.
pub async fn exchange_code(
    client: &reqwest::Client,
    config: &OAuthConfig,
    pkce: &PkceState,
    code: &str,
) -> Result<TokenBundle> {
    #[derive(Serialize)]
    struct TokenRequest {
        grant_type: String,
        code: String,
        redirect_uri: String,
        client_id: String,
        code_verifier: String,
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default = "default_token_type")]
        token_type: String,
        #[serde(default)]
        expires_in: u64,
        #[serde(default)]
        scope: Option<String>,
    }

    let body = TokenRequest {
        grant_type: "authorization_code".into(),
        code: code.into(),
        redirect_uri: config.redirect_uri.clone(),
        client_id: config.client_id.clone(),
        code_verifier: pkce.code_verifier.clone(),
    };

    let resp = client
        .post(&config.token_endpoint)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OAuthError::RefreshFailed(format!(
            "Token exchange failed ({status}): {text}"
        )));
    }

    let tr: TokenResponse = resp.json().await?;
    Ok(TokenBundle {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token,
        token_type: tr.token_type,
        obtained_at: Utc::now(),
        expires_in: tr.expires_in,
        scope: tr.scope,
    })
}

// ---------------------------------------------------------------------------
// Token refresh
// ---------------------------------------------------------------------------

/// Attempt to refresh an expired token.
pub async fn refresh_token(
    client: &reqwest::Client,
    config: &OAuthConfig,
    bundle: &TokenBundle,
) -> Result<TokenBundle> {
    let refresh = bundle
        .refresh_token
        .as_ref()
        .ok_or(OAuthError::NoRefreshToken)?;

    #[derive(Serialize)]
    struct RefreshRequest {
        grant_type: String,
        refresh_token: String,
        client_id: String,
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default = "default_token_type")]
        token_type: String,
        #[serde(default)]
        expires_in: u64,
        #[serde(default)]
        scope: Option<String>,
    }

    let body = RefreshRequest {
        grant_type: "refresh_token".into(),
        refresh_token: refresh.clone(),
        client_id: config.client_id.clone(),
    };

    let resp = client
        .post(&config.token_endpoint)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OAuthError::RefreshFailed(format!(
            "Refresh failed ({status}): {text}"
        )));
    }

    let tr: TokenResponse = resp.json().await?;
    Ok(TokenBundle {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token.or_else(|| Some(refresh.clone())),
        token_type: tr.token_type,
        obtained_at: Utc::now(),
        expires_in: tr.expires_in,
        scope: tr.scope,
    })
}

/// Ensure a valid token is available, refreshing if necessary.
/// Returns a token bundle (possibly refreshed).
pub async fn ensure_valid_token(
    client: &reqwest::Client,
    config: &OAuthConfig,
    provider_key: &str,
) -> Result<TokenBundle> {
    let bundle = load_token(provider_key)?.ok_or(OAuthError::InvalidState(format!(
        "No token stored for {provider_key}"
    )))?;

    if !bundle.is_expired() {
        return Ok(bundle);
    }

    let refreshed = refresh_token(client, config, &bundle).await?;
    save_token(provider_key, &refreshed)?;
    Ok(refreshed)
}

// ---------------------------------------------------------------------------
// GitHub device flow
// ---------------------------------------------------------------------------

/// Response from GitHub's device-code endpoint.
#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    /// Device verification code.
    pub device_code: String,
    /// User-visible code to enter on the verification page.
    pub user_code: String,
    /// URL the user should visit to authorize.
    pub verification_uri: String,
    /// Optional complete verification URL (includes user code).
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    /// Polling interval in seconds.
    pub interval: u64,
    /// Seconds until the device code expires.
    pub expires_in: u64,
}

/// Result of the device-flow token polling.
#[derive(Debug)]
pub enum DeviceFlowResult {
    /// success variant.
    Success(TokenBundle),
    /// pending variant.
    Pending,
    /// rejected variant.
    Rejected,
    /// timeout variant.
    Timeout(u64),
}

/// Step 1: Request a device code from GitHub.
pub async fn github_request_device_code(
    client: &reqwest::Client,
    client_id: &str,
    scope: &str,
) -> Result<DeviceCodeResponse> {
    #[derive(Serialize)]
    struct Body {
        client_id: String,
        scope: String,
    }

    let resp = client
        .post("https://github.com/login/device/code")
        .header("accept", "application/json")
        .json(&Body {
            client_id: client_id.into(),
            scope: scope.into(),
        })
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OAuthError::RefreshFailed(format!(
            "Device code request failed ({status}): {text}"
        )));
    }

    Ok(resp.json().await?)
}

/// Step 2: Poll GitHub for the access token.
///
/// `timeout_secs` caps total polling duration. Returns `DeviceFlowResult::Success`
/// once the user authorizes, `Pending` if still waiting, or `Rejected` on error.
pub async fn github_poll_for_token(
    client: &reqwest::Client,
    client_id: &str,
    device_code: &str,
    timeout_secs: u64,
) -> Result<DeviceFlowResult> {
    #[derive(Serialize)]
    struct Body {
        client_id: String,
        device_code: String,
        grant_type: String,
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        #[serde(default)]
        access_token: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        token_type: Option<String>,
        #[serde(default)]
        scope: Option<String>,
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        if std::time::Instant::now() > deadline {
            return Ok(DeviceFlowResult::Timeout(timeout_secs));
        }

        let resp = client
            .post("https://github.com/login/oauth/access_token")
            .header("accept", "application/json")
            .json(&Body {
                client_id: client_id.into(),
                device_code: device_code.into(),
                grant_type: "urn:ietf:params:oauth:grant-type:device_code".into(),
            })
            .send()
            .await?;

        let tr: TokenResponse = resp.json().await?;

        if let Some(token) = tr.access_token {
            return Ok(DeviceFlowResult::Success(TokenBundle {
                access_token: token,
                refresh_token: None,
                token_type: tr.token_type.unwrap_or_else(|| "Bearer".into()),
                obtained_at: Utc::now(),
                expires_in: 0, // GitHub tokens don't expire by default
                scope: tr.scope,
            }));
        }

        match tr.error.as_deref() {
            Some("authorization_pending") => {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            Some("slow_down") => {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                continue;
            }
            Some("expired_token") => return Ok(DeviceFlowResult::Rejected),
            Some("access_denied") => return Ok(DeviceFlowResult::Rejected),
            Some(other) => {
                return Err(OAuthError::RefreshFailed(format!(
                    "Device flow error: {other}"
                )));
            }
            None => {
                // No token and no error — keep polling briefly then bail.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        }
    }
}

/// High-level convenience: run the full GitHub device flow and persist the token.
pub async fn github_device_flow(
    client: &reqwest::Client,
    client_id: &str,
    scope: &str,
    timeout_secs: u64,
) -> Result<TokenBundle> {
    let dc = github_request_device_code(client, client_id, scope).await?;

    println!();
    println!("=== GitHub Device Authorization ===");
    println!("  1. Open: {}", dc.verification_uri);
    println!("  2. Enter code: {}", dc.user_code);
    if let Some(ref url) = dc.verification_uri_complete {
        println!("  Or visit: {url}");
    }
    println!();

    let result = github_poll_for_token(client, client_id, &dc.device_code, timeout_secs).await?;

    match result {
        DeviceFlowResult::Success(token) => {
            save_token("github", &token)?;
            println!("✓ GitHub authentication successful.");
            Ok(token)
        }
        DeviceFlowResult::Pending => Err(OAuthError::DeviceFlowPending),
        DeviceFlowResult::Rejected => Err(OAuthError::DeviceFlowRejected),
        DeviceFlowResult::Timeout(s) => Err(OAuthError::DeviceFlowTimeout(s)),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---- PKCE tests ----

    #[test]
    fn test_code_verifier_length() {
        let v = generate_code_verifier();
        assert!((43..=128).contains(&v.len()), "verifier length {}", v.len());
    }

    #[test]
    fn test_code_verifier_is_base64url() {
        let v = generate_code_verifier();
        // base64url chars: A-Z a-z 0-9 - _
        assert!(
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn test_code_verifier_uniqueness() {
        let a = generate_code_verifier();
        let b = generate_code_verifier();
        assert_ne!(a, b, "two verifiers should differ");
    }

    #[test]
    fn test_code_challenge_deterministic() {
        let v = generate_code_verifier();
        let c1 = derive_code_challenge(&v);
        let c2 = derive_code_challenge(&v);
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_code_challenge_differs_from_verifier() {
        let v = generate_code_verifier();
        let c = derive_code_challenge(&v);
        assert_ne!(v, c);
    }

    #[test]
    fn test_code_challenge_is_base64url() {
        let v = generate_code_verifier();
        let c = derive_code_challenge(&v);
        assert!(
            c.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        );
    }

    #[test]
    fn test_known_pkce_vector() {
        // RFC 7636 Appendix B reference vector
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = derive_code_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    // ---- TokenBundle / AuthStore tests ----

    #[test]
    fn test_token_bundle_not_expired_when_no_expiry() {
        let bundle = TokenBundle {
            access_token: "abc".into(),
            refresh_token: None,
            token_type: "Bearer".into(),
            obtained_at: Utc::now(),
            expires_in: 0,
            scope: None,
        };
        assert!(!bundle.is_expired());
    }

    #[test]
    fn test_token_bundle_expired() {
        let bundle = TokenBundle {
            access_token: "abc".into(),
            refresh_token: None,
            token_type: "Bearer".into(),
            obtained_at: Utc::now() - chrono::Duration::seconds(3600),
            expires_in: 1800, // expired 30 min ago
            scope: None,
        };
        assert!(bundle.is_expired());
    }

    #[test]
    fn test_token_bundle_not_yet_expired() {
        let bundle = TokenBundle {
            access_token: "abc".into(),
            refresh_token: None,
            token_type: "Bearer".into(),
            obtained_at: Utc::now(),
            expires_in: 3600, // expires in 1 hour
            scope: None,
        };
        assert!(!bundle.is_expired());
    }

    // ---- Auth store round-trip tests ----

    fn setup_temp_store() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn with_temp_auth_store<F>(f: F)
    where
        F: FnOnce(&PathBuf),
    {
        let dir = setup_temp_store();
        let path = dir.path().join("auth.json");
        // Monkey-patch default_auth_path by using save/load directly with a known path.
        // For tests we directly exercise the serde round-trip.
        let mut store = AuthStore::default();
        store.tokens.insert(
            "test-provider".into(),
            TokenBundle {
                access_token: "tok_abc123".into(),
                refresh_token: Some("ref_xyz".into()),
                token_type: "Bearer".into(),
                obtained_at: Utc::now(),
                expires_in: 3600,
                scope: Some("read write".into()),
            },
        );
        let json = serde_json::to_string_pretty(&store).unwrap();
        fs::write(&path, &json).unwrap();

        f(&path);

        // Verify round-trip
        let loaded: AuthStore = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.tokens["test-provider"].access_token, "tok_abc123");
        assert_eq!(
            loaded.tokens["test-provider"].refresh_token.as_deref(),
            Some("ref_xyz")
        );
    }

    #[test]
    fn test_auth_store_round_trip() {
        with_temp_auth_store(|_| {});
    }

    #[test]
    fn test_auth_store_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        assert!(!path.exists());
        // Loading from a missing file should yield an empty store via serde
        let result = fs::read_to_string(&path);
        assert!(result.is_err());
    }

    // ---- Authorization URL tests ----

    #[test]
    fn test_build_authorization_url_contains_pkce_params() {
        let config = OAuthConfig {
            authorization_endpoint: "https://example.com/authorize".into(),
            token_endpoint: "https://example.com/token".into(),
            client_id: "my-client".into(),
            redirect_uri: "http://localhost:8787/callback".into(),
            scopes: "read write".into(),
        };
        let pkce = build_authorization_url(&config);

        assert!(pkce.authorization_url.contains("code_challenge="));
        assert!(
            pkce.authorization_url
                .contains("code_challenge_method=S256")
        );
        assert!(pkce.authorization_url.contains("client_id=my-client"));
        assert!(pkce.authorization_url.contains("response_type=code"));
        assert!(pkce.authorization_url.contains("state="));
        assert!(pkce.authorization_url.contains("scope="));
        assert_eq!(pkce.code_verifier.len(), 43);
    }

    #[test]
    fn test_state_token_length() {
        let state = generate_state_token();
        assert!(state.len() >= 16, "state token should be at least 16 chars");
    }

    // ---- Serialization tests ----

    #[test]
    fn test_token_bundle_serialize_deserialize() {
        let bundle = TokenBundle {
            access_token: "at_123".into(),
            refresh_token: None,
            token_type: "Bearer".into(),
            obtained_at: "2025-01-01T00:00:00Z".parse().unwrap(),
            expires_in: 3600,
            scope: Some("org.api.read".into()),
        };
        let json = serde_json::to_string(&bundle).unwrap();
        let back: TokenBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.access_token, "at_123");
        assert!(back.refresh_token.is_none());
        assert_eq!(back.expires_in, 3600);
    }

    #[test]
    fn test_auth_store_multiple_providers() {
        let mut store = AuthStore::default();
        for name in &["anthropic", "openai", "github"] {
            store.tokens.insert(
                (*name).into(),
                TokenBundle {
                    access_token: format!("tok_{name}"),
                    refresh_token: None,
                    token_type: "Bearer".into(),
                    obtained_at: Utc::now(),
                    expires_in: 0,
                    scope: None,
                },
            );
        }
        let json = serde_json::to_string(&store).unwrap();
        let back: AuthStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tokens.len(), 3);
        assert_eq!(back.tokens["openai"].access_token, "tok_openai");
    }
}
