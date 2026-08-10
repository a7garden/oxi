//! OAuth `authorization_code` support for LLM providers.
//!
//! Specs are loaded from `oxicode-catalog/data/catalog/product-meta.toml`
//! (`[providers.<name>.oauth]` tables). Empty/missing table = key-only.
//!
//! The catalog file is embedded at compile time so the cached loader does not
//! depend on filesystem layout at runtime. A runtime [`load_meta`] helper is
//! also exposed for callers that want to source the file from a custom
//! location (tests, user overrides).

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// One provider's OAuth configuration parsed from `product-meta.toml`.
///
/// Public PKCE clients only — there is no `client_secret` field by design.
/// `deny_unknown_fields` guards against accidentally adding one: a TOML entry
/// carrying `client_secret = "..."` will fail to parse instead of silently
/// loading a confidential-client flow this CLI cannot support.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderOAuthSpec {
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub redirect_path: String,
    #[serde(default = "default_pkce")]
    pub use_pkce: bool,
}

fn default_pkce() -> bool {
    true
}

/// Container for every provider's OAuth spec parsed from `product-meta.toml`.
#[derive(Clone, Debug, Default)]
pub struct OAuthMeta {
    pub specs: HashMap<String, ProviderOAuthSpec>,
}

/// The catalog file embedded into the binary at compile time.
///
/// Path is resolved relative to this module because the layout of
/// `oxicode-cli/src/` and `oxicode-catalog/data/` is a workspace invariant.
const PRODUCT_META_TOML: &str =
    include_str!("../../oxicode-catalog/data/catalog/product-meta.toml");

/// Process-wide cache of the parsed `product-meta.toml` OAuth sections.
///
/// Populated lazily on first call to [`oauth_meta`] (or [`spec_for`]).
static META: OnceLock<OAuthMeta> = OnceLock::new();

/// Parse the OAuth sections from raw TOML.
///
/// The wrapping shape is:
///
/// ```toml
/// [providers.<name>.oauth]
/// client_id = "..."
/// # ...
/// ```
///
/// Providers without an `[providers.<name>.oauth]` table are dropped from
/// the result — empty/missing block = key-only provider.
pub fn load_meta_from_str(content: &str) -> Result<OAuthMeta, toml::de::Error> {
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        providers: HashMap<String, ProviderToml>,
    }
    #[derive(Deserialize)]
    struct ProviderToml {
        #[serde(default)]
        oauth: Option<ProviderOAuthSpec>,
    }

    let root: Root = toml::from_str(content)?;
    let specs = root
        .providers
        .into_iter()
        .filter_map(|(name, p)| p.oauth.map(|spec| (name, spec)))
        .collect();
    Ok(OAuthMeta { specs })
}

/// Load and parse `path` as a `product-meta.toml` containing OAuth sections.
///
/// A parse failure is reported as `io::ErrorKind::InvalidData` so callers can
/// branch on `ErrorKind` rather than threading TOML's error type.
pub fn load_meta(path: &Path) -> std::io::Result<OAuthMeta> {
    let content = std::fs::read_to_string(path)?;
    load_meta_from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Return the cached process-wide `OAuthMeta`, parsing the embedded
/// `product-meta.toml` on first call.
///
/// Missing or malformed sections degrade to an empty map so a broken catalog
/// file does not panic the CLI; downstream code treats "no spec" as
/// "key-only provider".
pub fn oauth_meta() -> &'static OAuthMeta {
    META.get_or_init(|| load_meta_from_str(PRODUCT_META_TOML).unwrap_or_default())
}

/// Look up the OAuth spec for `provider` (e.g. `"openai"`, `"anthropic"`).
///
/// Returns `None` when the provider has no `[providers.<name>.oauth]`
/// block in `product-meta.toml` (i.e. it is key-only).
pub fn spec_for(provider: &str) -> Option<ProviderOAuthSpec> {
    oauth_meta().specs.get(provider).cloned()
}
/// Tokens returned from a successful OAuth token exchange.
///
/// `expires_at` is an absolute Unix epoch in seconds (`now + expires_in`),
/// matching how callers want to compare against the system clock.
#[derive(Debug, Clone)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    pub scopes: Vec<String>,
}

/// Generate a random PKCE verifier and matching S256 code challenge
/// (RFC 7636 §4.1, §4.2).
///
/// The verifier is 32 bytes of CSPRNG output encoded as base64url-no-pad,
/// landing at 43 ASCII characters — within the RFC's 43..=128 range.
/// The challenge is `BASE64URL-NO-PAD(SHA256(verifier))`.
pub fn pkce_pair() -> (String, String) {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use rand::TryRngCore;
    use sha2::{Digest, Sha256};
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OsRng is infallible");
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
    (verifier, challenge)
}

/// Build the authorization URL for the OAuth `authorization_code` flow.
///
/// `port` is the local-loopback port the CLI has bound to capture the
/// provider's redirect — providers will reject the call if the
/// `redirect_uri` host:port does not match a registered value, so it must
/// be threaded through (not assumed to be 0).
pub fn build_auth_url(
    spec: &ProviderOAuthSpec,
    port: u16,
    state: &str,
    code_challenge: &str,
) -> String {
    let redirect_uri = format!("http://127.0.0.1:{port}{}", spec.redirect_path);
    let mut url = url::Url::parse(&spec.auth_url).expect("auth_url must be valid");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", &spec.client_id);
        q.append_pair("redirect_uri", &redirect_uri);
        q.append_pair("scope", &spec.scopes.join(" "));
        q.append_pair("state", state);
        if spec.use_pkce {
            q.append_pair("code_challenge", code_challenge);
            q.append_pair("code_challenge_method", "S256");
        }
    }
    url.to_string()
}

/// Exchange an authorization code for tokens at `spec.token_url`.
///
/// Sends the PKCE verifier (`code_verifier`) and the same `redirect_uri`
/// that was used in the authorization request, so the provider can pair
/// them up. Returns the parsed [`OAuthTokens`].
pub async fn exchange_code(
    spec: &ProviderOAuthSpec,
    port: u16,
    code: &str,
    verifier: &str,
) -> anyhow::Result<OAuthTokens> {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default)]
        expires_in: Option<i64>,
        #[serde(default)]
        scope: Option<String>,
    }

    #[derive(Deserialize)]
    struct TokenError {
        error: String,
        #[serde(default)]
        error_description: Option<String>,
    }

    let redirect_uri = format!("http://127.0.0.1:{port}{}", spec.redirect_path);
    let client = reqwest::Client::new();
    let response = client
        .post(&spec.token_url)
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", spec.client_id.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if status.is_success() {
        let parsed: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("malformed token response: {e}; body={body}"))?;
        let expires_in = parsed.expires_in.unwrap_or(0);
        let scopes = parsed
            .scope
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        let expires_at = chrono::Utc::now().timestamp() + expires_in;
        Ok(OAuthTokens {
            access_token: parsed.access_token,
            refresh_token: parsed.refresh_token,
            expires_at,
            scopes,
        })
    } else {
        // Surface the provider's OAuth error code so callers can branch on
        // `invalid_grant`, `invalid_request`, etc.
        match serde_json::from_str::<TokenError>(&body) {
            Ok(err) => Err(anyhow::anyhow!(
                "token exchange failed (status {status}): {} — {}",
                err.error,
                err.error_description.unwrap_or_default()
            )),
            Err(_) => Err(anyhow::anyhow!(
                "token exchange failed (status {status}): {body}"
            )),
        }
    }
}

/// Hand a URL to the OS so the user's default browser opens it.
///
/// Validates the URL is parseable before handing it off — we never want
/// a malformed string reaching `xdg-open`/`open`/the Windows shell.
pub fn open_browser(url: &str) -> anyhow::Result<()> {
    let parsed = url::Url::parse(url)
        .map_err(|e| anyhow::anyhow!("open_browser: invalid URL {url:?}: {e}"))?;
    // Only http(s) schemes are safe to launch externally.
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(anyhow::anyhow!(
                "open_browser: refusing to launch non-web scheme {other:?}"
            ));
        }
    }
    open::that_detached(url).map_err(|e| anyhow::anyhow!("open_browser: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn loads_openai_and_anthropic_specs() {
        let meta = load_meta_from_str(
            r#"
            [providers.openai.oauth]
            client_id = "app-x"
            auth_url = "https://auth.openai.com/oauth/authorize"
            token_url = "https://auth.openai.com/oauth/token"
            scopes = ["openid"]
            redirect_path = "/callback"
            use_pkce = true

            [providers.anthropic.oauth]
            client_id = "oxicode"
            auth_url = "https://console.anthropic.com/oauth/authorize"
            token_url = "https://console.anthropic.com/oauth/token"
            scopes = ["user:profile"]
            redirect_path = "/callback"
            use_pkce = true
            "#,
        )
        .expect("parse must succeed");
        let openai = meta.specs.get("openai").expect("openai present");
        assert_eq!(openai.client_id, "app-x");
        assert!(openai.use_pkce);
        let anthropic = meta.specs.get("anthropic").expect("anthropic present");
        assert_eq!(anthropic.scopes, vec!["user:profile".to_string()]);
    }

    #[test]
    fn missing_oauth_table_means_provider_is_key_only() {
        let meta = load_meta_from_str(
            r#"
            [providers.google.some_other_block]
            foo = "bar"
            "#,
        )
        .expect("parse must succeed");
        assert!(!meta.specs.contains_key("google"));
    }

    /// Embedded `product-meta.toml` must yield at least the two seeded
    /// providers so `spec_for("openai")` / `spec_for("anthropic")` work
    /// without any runtime IO.
    #[test]
    fn embedded_catalog_yields_openai_and_anthropic() {
        let openai = spec_for("openai").expect("openai spec present");
        assert_eq!(openai.auth_url, "https://auth.openai.com/oauth/authorize");
        assert!(openai.use_pkce);

        let anthropic = spec_for("anthropic").expect("anthropic spec present");
        assert_eq!(
            anthropic.token_url,
            "https://console.anthropic.com/oauth/token"
        );

        // A key-only provider (no oauth block) must not be present.
        assert!(!oauth_meta().specs.contains_key("openrouter"));
    }

    #[test]
    fn pkce_pair_verifier_is_43_to_128_chars_and_challenge_is_s256() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};

        let (verifier, challenge) = pkce_pair();
        assert!(
            verifier.len() >= 43 && verifier.len() <= 128,
            "verifier length {} out of RFC 7636 range",
            verifier.len()
        );
        // Recompute the challenge from the verifier and compare.
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(challenge, expected);
    }

    #[test]
    fn build_auth_url_includes_pkce_state_and_redirect_uri() {
        let spec = ProviderOAuthSpec {
            client_id: "app-x".into(),
            auth_url: "https://auth.openai.com/oauth/authorize".into(),
            token_url: "https://auth.openai.com/oauth/token".into(),
            scopes: vec!["openid".into(), "offline_access".into()],
            redirect_path: "/callback".into(),
            use_pkce: true,
        };
        let url = build_auth_url(&spec, 12345, "ST", "CC");
        let parsed = url::Url::parse(&url).expect("must be a valid URL");
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("auth.openai.com"));
        assert_eq!(parsed.path(), "/oauth/authorize");
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(q.get("client_id").map(String::as_str), Some("app-x"));
        assert_eq!(
            q.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:12345/callback")
        );
        assert_eq!(q.get("state").map(String::as_str), Some("ST"));
        assert_eq!(q.get("code_challenge").map(String::as_str), Some("CC"));
        assert_eq!(
            q.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            q.get("scope").map(String::as_str),
            Some("openid offline_access")
        );
    }

    #[tokio::test]
    async fn exchange_code_parses_200_response() {
        use httpmock::MockServer;
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST).path("/oauth/token");
            then.status(200).json_body(serde_json::json!({
                "access_token": "AT",
                "refresh_token": "RT",
                "expires_in": 3600,
                "scope": "openid"
            }));
        });
        let spec = ProviderOAuthSpec {
            client_id: "app-x".into(),
            auth_url: "https://auth.example.com/authorize".into(),
            token_url: format!("{}/oauth/token", server.base_url()),
            scopes: vec!["openid".into()],
            redirect_path: "/callback".into(),
            use_pkce: true,
        };
        let tokens = exchange_code(&spec, 12345, "code-1", "verifier")
            .await
            .expect("token exchange should succeed");
        assert_eq!(tokens.access_token, "AT");
        assert_eq!(tokens.refresh_token.as_deref(), Some("RT"));
        assert!(tokens.expires_at > 0);
        assert_eq!(tokens.scopes, vec!["openid".to_string()]);
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn exchange_code_returns_error_on_4xx() {
        use httpmock::MockServer;
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST).path("/oauth/token");
            then.status(400).json_body(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "code already redeemed"
            }));
        });
        let spec = ProviderOAuthSpec {
            client_id: "app-x".into(),
            auth_url: "https://example.com/authorize".into(),
            token_url: format!("{}/oauth/token", server.base_url()),
            scopes: vec![],
            redirect_path: "/callback".into(),
            use_pkce: true,
        };
        let err = exchange_code(&spec, 12345, "code-1", "v")
            .await
            .expect_err("4xx must surface as error");
        assert!(
            format!("{err}").contains("invalid_grant"),
            "error must include provider's error code: {err}"
        );
        mock.assert_hits(1);
    }

    /// Smoke test: the function must exist, accept a `&str`, and return a
    /// `Result` compatible with `anyhow::Error`. The brief notes that
    /// actually invoking `open_browser` would spawn a real browser, so the
    /// test only pins the signature without calling it.
    #[test]
    fn open_browser_accepts_a_well_formed_url() {
        let f: fn(&str) -> anyhow::Result<()> = open_browser;
        // The signature is what we care about — referencing `f` keeps it
        // live so dead-code elimination does not strip it.
        let _ = f;
    }
}
