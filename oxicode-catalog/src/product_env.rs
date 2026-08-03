//! Product home-directory resolution for oxicode-ai.
//!
//! oxicode-ai is a reusable library embedded by multiple products (oxicode-cli,
//! oxios, downstream forks, ...). Each product needs its own isolated home
//! namespace so that catalog overrides, runtime caches, and auth stores do
//! not collide. Without an explicit product home, every embedder silently
//! inherits the `oxicode` namespace (`~/.oxicode/`) — including a *different*
//! product's catalog overrides — which is a library-layer coupling smell.
//!
//! The product home directory is resolved via the `OXICODE_HOME` environment
//! variable, falling back to `$HOME/.oxicode`. This mirrors
//! `oxicode_sdk::ports::fs::home_dir`, which delegates here so the leaf
//! library and the SDK agree on a single resolution path.
//!
//! # Why env-var, not a typed global
//!
//! A product identity is process-global: one binary is one product, with one
//! home namespace, for its entire lifetime. An environment variable is the
//! natural representation — it is set at process spawn, before any library
//! code runs, and is readable inside any lazy initializer (including the
//! `OnceLock` that caches built-in providers). This avoids introducing a
//! second, mutable, init-ordered global alongside the already-established
//! `OXICODE_HOME` convention used by oxicode-sdk.
//!
//! Embedders isolate by setting one variable:
//!
//! ```text
//! OXICODE_HOME=~/.oxios   # oxios gets its own ~/.oxios/{catalog,cache,auth.json}
//! ```

use std::path::{Path, PathBuf};

/// Resolve a product home directory from explicit + user-home inputs.
///
/// Pure (no environment access) so it is trivially testable without racing
/// the process-global environment under parallel test runners.
///
/// - If `oxicode_home` is set and non-empty, it wins (treated as an absolute path).
/// - Otherwise `$user_home/.oxicode`.
/// - `None` if neither is available.
fn resolve_home(oxicode_home: Option<&str>, user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = oxicode_home.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    user_home.map(|h| h.join(".oxicode"))
}

/// The product home directory.
///
/// Resolution order:
/// 1. `OXICODE_HOME` environment variable — absolute path, if set and non-empty.
/// 2. `$HOME/.oxicode` via [`dirs::home_dir`].
///
/// Returns [`Err`] only when neither `OXICODE_HOME` nor a usable home directory
/// is available (extremely rare — no home directory at all).
///
/// # Examples
///
/// ```
/// # use oxicode_catalog::product_env::home_dir;
/// // In a normal environment this resolves to either $OXICODE_HOME or ~/.oxicode.
/// let _ = home_dir();
/// ```
pub fn home_dir() -> std::io::Result<PathBuf> {
    resolve_home(
        std::env::var("OXICODE_HOME").ok().as_deref(),
        dirs::home_dir().as_deref(),
    )
    .ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "neither OXICODE_HOME nor HOME is set",
        )
    })
}

/// The catalog overrides directory: `<product-home>/catalog/`.
///
/// `None` when the product home cannot be resolved.
pub fn catalog_override_dir() -> Option<PathBuf> {
    home_dir().ok().map(|h| h.join("catalog"))
}

/// The runtime cache directory: `<product-home>/cache/`.
///
/// `None` when the product home cannot be resolved. The models.dev
/// enrichment layer may still override the exact cache *file* via
/// `OXICODE_MODELS_DEV_CACHE_PATH`, which is more specific than this directory.
pub fn cache_dir() -> Option<PathBuf> {
    home_dir().ok().map(|h| h.join("cache"))
}

/// The auth store path: `<product-home>/auth.json`.
///
/// `None` when the product home cannot be resolved.
pub fn auth_path() -> Option<PathBuf> {
    home_dir().ok().map(|h| h.join("auth.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn oxicode_home_wins_when_set() {
        let got = resolve_home(
            Some("/custom/oxios"),
            Some(PathBuf::from("/home/u").as_path()),
        );
        assert_eq!(got, Some(PathBuf::from("/custom/oxios")));
    }

    #[test]
    fn empty_oxicode_home_falls_through() {
        let got = resolve_home(Some(""), Some(PathBuf::from("/home/u").as_path()));
        assert_eq!(got, Some(PathBuf::from("/home/u/.oxicode")));
    }

    #[test]
    fn defaults_to_user_home_dot_oxicode() {
        let got = resolve_home(None, Some(PathBuf::from("/home/u").as_path()));
        assert_eq!(got, Some(PathBuf::from("/home/u/.oxicode")));
    }

    #[test]
    fn none_when_both_absent() {
        assert_eq!(resolve_home(None, None), None);
    }

    #[test]
    fn subpaths_compose_from_resolved_home() {
        let home = resolve_home(Some("/x"), None).unwrap();
        assert_eq!(
            home.join("catalog").join("overrides.toml"),
            PathBuf::from("/x/catalog/overrides.toml")
        );
        assert_eq!(
            home.join("cache").join("models-dev.json"),
            PathBuf::from("/x/cache/models-dev.json")
        );
        assert_eq!(home.join("auth.json"), PathBuf::from("/x/auth.json"));
    }

    /// Smoke test: `home_dir()` resolves in any normal environment (where
    /// either `OXICODE_HOME` or `HOME` is set). Does not mutate the environment,
    /// so it is safe under parallel test runners.
    #[test]
    fn home_dir_resolves_in_ci() {
        let resolved = home_dir();
        // Either OXICODE_HOME or HOME is set in every CI/dev environment.
        assert!(resolved.is_ok(), "expected a resolvable home dir");
    }
}
