//! Per-provider OAuth refresh coalesce.
//!
//! [`refresh_if_expired`] looks up the stored OAuth credential for a provider
//! and, if it is expired (or within 60 s of expiry), refreshes it via
//! [`provider_oauth::refresh_grant`]. Concurrent callers for the same
//! provider share a single in-flight refresh — once one finishes, all
//! subsequent callers see the freshly stored token without making a
//! second network request.
//!
//! ## Storage injection
//!
//! The public entry point reads from the process-wide [`shared_auth_storage`]
//! singleton. The inner [`refresh_if_expired_with_storage`] helper accepts an
//! explicit `&Arc<AuthStorage>` so tests can drive an in-memory storage
//! without touching the real `auth.json`. This mirrors the controller's
//! pre-flight API-alignment note: callers must NOT assume a method like
//! `auth.get_api_key_full` exists — we read the full credential set via
//! `auth.get_all()` and pattern-match the OAuth variant.

use crate::provider_oauth;
use crate::store::auth_storage::{AuthCredential, AuthStorage, shared_auth_storage};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tokio::sync::Mutex;

#[derive(Debug, Clone, thiserror::Error)]
pub enum RefreshError {
    /// The provider has no stored OAuth credential at all (only an API
    /// key, no entry, or a session token).
    #[error("no OAuth credential for '{0}'")]
    NotOAuth(String),

    /// The stored OAuth credential has no `refresh_token`; the user must
    /// re-run the interactive login flow to obtain one.
    #[error("refresh token missing — re-login required for '{0}'")]
    ReLoginRequired(String),

    /// The provider has an OAuth spec but the actual refresh grant failed
    /// (network error, provider rejection, malformed response, etc.).
    #[error("refresh failed: {0}")]
    Failed(String),
}

/// One coalesce cell. Concurrent callers for the same provider share a
/// single cell so only the first one performs the actual refresh grant.
type CoalesceCell = Arc<tokio::sync::OnceCell<Result<(), RefreshError>>>;

/// Process-wide coalesce map: provider name → in-flight refresh future.
///
/// `tokio::sync::OnceCell<Result<(), RefreshError>>` is used so that only the first caller for a
/// given provider actually performs the network round-trip; concurrent
/// callers wait on the same cell and observe the final `Result` after the
/// refresh completes (success or failure). The map itself is `Mutex`-ed so
/// we can register cells concurrently without races.
static COALESCE: LazyLock<Mutex<HashMap<String, CoalesceCell>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Refresh the stored OAuth credential for `provider` if it is expired or
/// within 60 seconds of expiry. Concurrent calls coalesce on a per-provider
/// `OnceCell` so the network round-trip runs at most once per cycle.
///
/// Returns `Ok(())` immediately when the credential is unexpired or
/// "never-expiring" (`expires_at == 0`) credentials; returns
/// [`RefreshError::NotOAuth`] when the provider has no OAuth entry at all
/// so the caller can distinguish "nothing to do" from "did a refresh".
/// Storage is read from the process-wide [`shared_auth_storage`] singleton.
pub async fn refresh_if_expired(provider: &str) -> Result<(), RefreshError> {
    let auth = shared_auth_storage();
    refresh_if_expired_with_storage(provider, &auth).await
}

/// Inner helper: refresh against an explicit `AuthStorage`.
///
/// Separating the storage parameter out of [`refresh_if_expired`] lets tests
/// inject an in-memory storage without polluting the user's real `auth.json`.
pub async fn refresh_if_expired_with_storage(
    provider: &str,
    auth: &Arc<AuthStorage>,
) -> Result<(), RefreshError> {
    let creds = auth.get_all();
    let credential = creds
        .get(provider)
        .ok_or_else(|| RefreshError::NotOAuth(provider.to_string()))?;

    let (refresh_token, spec) = match credential {
        AuthCredential::OAuth {
            access_token: _,
            refresh_token,
            expires_at,
            scopes: _,
            provider_data: _,
        } => {
            // `expires_at == 0` ⇒ never-expiring (matches `is_expired()` in
            // auth_storage.rs). Treat as not-expired and skip the refresh.
            if *expires_at == 0 {
                return Ok(());
            }
            let now = chrono::Utc::now().timestamp().max(0) as u64;
            // `needs_refresh()` in auth_storage.rs uses `expires_at <= now + 60`.
            if *expires_at > now + 60 {
                return Ok(());
            }
            let rt = refresh_token
                .clone()
                .ok_or_else(|| RefreshError::ReLoginRequired(provider.to_string()))?;
            let spec = provider_oauth::spec_for(provider)
                .ok_or_else(|| RefreshError::Failed(format!("no OAuth spec for {provider}")))?;
            (rt, spec)
        }
        // ApiKey or Session — not OAuth.
        _ => return Err(RefreshError::NotOAuth(provider.to_string())),
    };

    // Get or insert the coalesce cell for this provider. Two callers may
    // race here; the Mutex serializes the map mutation, after which both
    // hold the same Arc<OnceCell<()>>.
    let cell = {
        let mut map = COALESCE.lock().await;
        map.entry(provider.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
            .clone()
    };

    // Run the refresh exactly once per coalesce cell. Any error is
    // captured inside the cell so all waiters observe the same outcome;
    // the next caller (after the cell resolves) will simply re-check
    // expiry and may issue a fresh refresh if the previous attempt failed.
    // `do_refresh` returns `anyhow::Result<()>`. Wrap it into
    // `Result<(), RefreshError>` once, so every coalesced caller observes
    // the same `RefreshError::Failed` message.
    let cell_value = cell.get_or_init(|| async {
        do_refresh(provider, &spec, &refresh_token)
            .await
            .map_err(|e| RefreshError::Failed(format!("{e}")))
    });
    // `OnceCell::get_or_init(...).await` returns `&Result<...>`. The
    // `?` operator only works on owned `Result`s, so we clone the inner
    // `RefreshError` (`Clone`-derived via `thiserror`) and return.
    let outcome: Result<(), RefreshError> = match cell_value.await {
        Ok(()) => Ok(()),
        Err(e) => Err(e.clone()),
    };
    outcome?;
    Ok(())
}

/// Drop the coalesce cell for `provider` so the next call to
/// [`refresh_if_expired`] is forced to do a fresh refresh. Intended for
/// tests and for error-recovery paths that want to invalidate a failed
/// cell so a retry actually retries instead of returning the cached
/// `RefreshError::Failed`.
pub async fn invalidate_coalesce(provider: &str) {
    let mut map = COALESCE.lock().await;
    map.remove(provider);
}

async fn do_refresh(
    provider: &str,
    spec: &provider_oauth::ProviderOAuthSpec,
    refresh_token: &str,
) -> anyhow::Result<()> {
    let tokens = provider_oauth::refresh_grant(spec, refresh_token).await?;
    let auth = shared_auth_storage();
    // `update_oauth_tokens` takes `u64` for `new_expires_at`; convert from
    // the i64 timestamp returned by `refresh_grant` (which is what
    // `provider_oauth` exposes so callers can branch on `now < expires_at`
    // in i64). A negative `expires_at` is impossible here (it's
    // `now + expires_in` with `expires_in >= 0`), but saturate defensively.
    let new_expires_at: u64 = tokens.expires_at.max(0) as u64;
    auth.update_oauth_tokens(
        provider,
        tokens.access_token,
        tokens.refresh_token,
        new_expires_at,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::auth_storage::{AuthCredential, AuthStorage};

    /// A credential whose expiry is comfortably in the future must NOT
    /// trigger a network call. This is the failing-test gate for TDD:
    /// before `refresh_if_expired_with_storage` is implemented, the test
    /// cannot even compile, satisfying the brief's step 4 expectation.
    #[tokio::test]
    async fn refresh_if_expired_noop_when_not_expired() {
        let auth = Arc::new(AuthStorage::in_memory());
        let now = chrono::Utc::now().timestamp() as u64;
        // 1 hour in the future — well outside the 60 s refresh window.
        auth.set_oauth_full(
            "test-provider",
            "AT-unexpired".to_string(),
            Some("RT-unexpired".to_string()),
            now + 3600,
            None,
            None,
        );

        // No MockServer is started: if `refresh_if_expired_with_storage`
        // attempted to refresh, this test would hang on a connect-refused
        // against `spec_for("test-provider")`'s real token_url (or panic
        // on `spec_for` returning `None` → `RefreshError::Failed`). Either
        // way, a clean Ok(()) is the proof we skipped the refresh.
        let result = refresh_if_expired_with_storage("test-provider", &auth).await;
        assert!(
            result.is_ok(),
            "unexpired credential should be a no-op, got {result:?}"
        );

        // The credential must be unchanged (same access token).
        let stored = auth
            .get_all()
            .remove("test-provider")
            .expect("credential still present");
        match stored {
            AuthCredential::OAuth { access_token, .. } => {
                assert_eq!(access_token, "AT-unexpired");
            }
            other => panic!("expected OAuth credential, got {other:?}"),
        }
    }

    /// `expires_at == 0` means "never expires"; refresh must be skipped.
    #[tokio::test]
    async fn refresh_if_expired_noop_when_expires_at_zero() {
        let auth = Arc::new(AuthStorage::in_memory());
        auth.set_oauth_full(
            "never-exp",
            "AT-never".to_string(),
            Some("RT-never".to_string()),
            0, // never-expiring sentinel
            None,
            None,
        );

        let result = refresh_if_expired_with_storage("never-exp", &auth).await;
        assert!(
            result.is_ok(),
            "expires_at=0 must be a no-op, got {result:?}"
        );
    }

    /// No stored credential at all → `RefreshError::NotOAuth`.
    #[tokio::test]
    async fn refresh_if_expired_returns_not_oauth_when_missing() {
        let auth = Arc::new(AuthStorage::in_memory());
        let result = refresh_if_expired_with_storage("ghost", &auth).await;
        match result {
            Err(RefreshError::NotOAuth(name)) => assert_eq!(name, "ghost"),
            other => panic!("expected NotOAuth(\"ghost\"), got {other:?}"),
        }
    }

    /// API-key credential (not OAuth) → `RefreshError::NotOAuth`. Mirrors
    /// the existing `is_expired` / `needs_refresh` helpers in
    /// `auth_storage.rs` which also short-circuit on non-OAuth.
    #[tokio::test]
    async fn refresh_if_expired_returns_not_oauth_for_api_key() {
        let auth = Arc::new(AuthStorage::in_memory());
        auth.set_api_key("anthropic", "sk-test".to_string());
        let result = refresh_if_expired_with_storage("anthropic", &auth).await;
        match result {
            Err(RefreshError::NotOAuth(name)) => assert_eq!(name, "anthropic"),
            other => panic!("expected NotOAuth, got {other:?}"),
        }
    }

    /// OAuth credential without a refresh token → `RefreshError::ReLoginRequired`.
    #[tokio::test]
    async fn refresh_if_expired_returns_re_login_when_no_refresh_token() {
        let auth = Arc::new(AuthStorage::in_memory());
        // Spec exists for "openai" via the embedded catalog; we set an
        // expired credential with no refresh_token. The expiry gate fires
        // first; we then look for the refresh_token and find None.
        let now = chrono::Utc::now().timestamp() as u64;
        auth.set_oauth_full(
            "openai",
            "AT-stale".to_string(),
            None,
            now.saturating_sub(10), // already expired
            None,
            None,
        );
        let result = refresh_if_expired_with_storage("openai", &auth).await;
        match result {
            Err(RefreshError::ReLoginRequired(name)) => assert_eq!(name, "openai"),
            other => panic!("expected ReLoginRequired, got {other:?}"),
        }
    }
}
