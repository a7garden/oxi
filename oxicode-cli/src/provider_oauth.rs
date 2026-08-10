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
}
