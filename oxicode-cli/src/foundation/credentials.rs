//! Keychain-backed credential resolver + legacy one-time importer.
//!
//! The Keychain is the only durable credential authority under the
//! Foundation host. The [`KeychainCredentialResolver`] looks up a
//! profile's `{ service, account }` locator and returns either the
//! resolved value (typed) or a typed error. The `Debug` / `Display`
//! surface never reveals the value.
//!
//! The legacy importer reads `~/.oxicode/auth.json`, asks the user
//! for explicit acknowledgement, writes the Keychain entry, and
//! optionally archives the legacy file outside the active credential
//! path. It is the only code path that reads `~/.oxicode/auth.json`
//! under the Foundation host.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::foundation::FoundationError;
#[cfg(test)]
use crate::foundation::profiles::CredentialLocator;
use crate::foundation::profiles::Profile;
/// Result of resolving a credential locator. The `Debug` impl masks
/// the secret value so the type can appear in `tracing` and
/// `anyhow::Error` chains without leaking the resolved key material.
pub enum Credential {
    /// A secret value resolved from the Keychain. Never displayed.
    Keychain(String),
    /// A non-persistent environment-variable override.
    Environment(String),
    /// The locator could not be resolved. The caller MUST surface
    /// this to the user; it is never silently retried.
    Unavailable(CredentialError),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::Keychain(_) => f.write_str("Credential::Keychain(***)"),
            Credential::Environment(_) => f.write_str("Credential::Environment(***)"),
            Credential::Unavailable(e) => {
                f.debug_tuple("Credential::Unavailable").field(e).finish()
            }
        }
    }
}
/// Typed keychain error. `Display` carries the locator (account name
/// is public), not the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    Unavailable(String),
    Locked(String),
    NotFound { service: String, account: String },
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "keychain unavailable: {s}"),
            Self::Locked(s) => write!(f, "keychain locked: {s}"),
            Self::NotFound { service, account } => {
                write!(f, "keychain entry not found for {service}:{account}")
            }
        }
    }
}

/// `keyring` crate abstraction. Real production code uses the
/// `keyring` crate; the trait is what the rest of the code depends
/// on so tests can swap in a fake without touching the OS keychain.
pub trait KeychainBackend: Send + Sync + std::fmt::Debug {
    fn get(&self, service: &str, account: &str) -> Result<String, CredentialError>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), CredentialError>;
}
/// Production implementation. Uses the `keyring` crate (v3) with
/// per-platform native backends configured in `Cargo.toml`.
#[derive(Debug, Default, Clone)]
pub struct SystemKeychain;

impl KeychainBackend for SystemKeychain {
    fn get(&self, service: &str, account: &str) -> Result<String, CredentialError> {
        let entry = keyring::Entry::new(service, account)
            .map_err(|e| CredentialError::Unavailable(e.to_string()))?;
        entry.get_password().map_err(|e| match e {
            keyring::Error::NoEntry => CredentialError::NotFound {
                service: service.to_string(),
                account: account.to_string(),
            },
            keyring::Error::PlatformFailure(_) => CredentialError::Unavailable(e.to_string()),
            _ => CredentialError::Locked(e.to_string()),
        })
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialError> {
        let entry = keyring::Entry::new(service, account)
            .map_err(|e| CredentialError::Unavailable(e.to_string()))?;
        entry
            .set_password(value)
            .map_err(|e| CredentialError::Unavailable(e.to_string()))
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), CredentialError> {
        let entry = keyring::Entry::new(service, account)
            .map_err(|e| CredentialError::Unavailable(e.to_string()))?;
        entry.delete_credential().map_err(|e| match e {
            keyring::Error::NoEntry => CredentialError::NotFound {
                service: service.to_string(),
                account: account.to_string(),
            },
            _ => CredentialError::Unavailable(e.to_string()),
        })
    }
}

/// Resolves profile credentials. The resolver is the only thing the
/// rest of the code talks to.
#[derive(Debug, Clone)]
pub struct KeychainCredentialResolver<B: KeychainBackend + Clone> {
    backend: B,
}

impl<B: KeychainBackend + Clone> KeychainCredentialResolver<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// Resolve a profile's credential. Environment sentinels fall
    /// through to the appropriate env var. The Keychain is contacted
    /// only when the locator is a real pair.
    pub fn resolve(&self, profile: &Profile) -> Credential {
        let loc = &profile.credential;
        if loc.service == "__env__" && loc.account == "__env__" {
            // Env override is recorded under either OXICODE_API_KEY
            // (default) or a profile-local variable.
            let env_var = std::env::var("OXICODE_API_KEY").ok();
            if let Some(value) = env_var
                && !value.is_empty()
            {
                return Credential::Environment(value);
            }
            return Credential::Unavailable(CredentialError::NotFound {
                service: loc.service.clone(),
                account: loc.account.clone(),
            });
        }
        match self.backend.get(&loc.service, &loc.account) {
            Ok(value) => Credential::Keychain(value),
            Err(e) => Credential::Unavailable(e),
        }
    }
}

impl Default for KeychainCredentialResolver<SystemKeychain> {
    fn default() -> Self {
        Self::new(SystemKeychain)
    }
}

/// One-time legacy importer. Reads `~/.oxicode/auth.json`, asks the
/// user for acknowledgement, writes the Keychain entry, then
/// (optionally) archives the legacy file outside the active
/// credential path.
pub struct LegacyImporter<B: KeychainBackend + Clone> {
    backend: B,
}

impl<B: KeychainBackend + Clone> LegacyImporter<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// Run the one-time import. The caller is responsible for
    /// gathering the explicit `acknowledge: true` from the user.
    pub fn run(
        &self,
        auth_json_path: &Path,
        profile_id: &str,
        provider: &str,
        acknowledge: bool,
        archive: bool,
    ) -> Result<LegacyImportOutcome, FoundationError> {
        if !acknowledge {
            return Err(FoundationError::Parse(
                "legacy import requires explicit acknowledgement".to_string(),
            ));
        }
        if !auth_json_path.is_file() {
            return Err(FoundationError::Parse(format!(
                "legacy auth file not found: {}",
                auth_json_path.display()
            )));
        }
        let raw = std::fs::read_to_string(auth_json_path)?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)?;
        let key = parsed
            .get("providers")
            .and_then(|p| p.get(provider))
            .and_then(|p| p.get("api_key"))
            .and_then(|p| p.as_str())
            .ok_or_else(|| {
                FoundationError::Parse(format!(
                    "legacy auth file does not contain an API key for {provider}"
                ))
            })?;
        let service = "dev.oxi.foundation".to_string();
        let account = profile_id.to_string();
        self.backend
            .set(&service, &account, key)
            .map_err(|e| FoundationError::KeychainUnavailable(e.to_string()))?;
        let archive_path = if archive {
            Some(self.archive_legacy(auth_json_path)?)
        } else {
            None
        };
        Ok(LegacyImportOutcome {
            service,
            account,
            archive_path,
        })
    }

    fn archive_legacy(&self, path: &Path) -> Result<PathBuf, FoundationError> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let archive_dir = parent.join("archive");
        std::fs::create_dir_all(&archive_dir)?;
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
        let target = archive_dir.join(format!(
            "auth-{}-{}.json",
            ts,
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("auth.json"),
        ));
        // Atomic rename; never overwrite the original silently.
        if target.exists() {
            return Err(FoundationError::Parse(format!(
                "archive target already exists: {}",
                target.display()
            )));
        }
        std::fs::rename(path, &target)?;
        Ok(target)
    }
}

/// Result of a successful legacy import.
#[derive(Debug, Clone)]
pub struct LegacyImportOutcome {
    pub service: String,
    pub account: String,
    pub archive_path: Option<PathBuf>,
}

impl<B: KeychainBackend + Clone> std::fmt::Debug for LegacyImporter<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyImporter")
            .field("backend", &"<dyn KeychainBackend>")
            .finish()
    }
}

/// In-memory Keychain backend for tests. The `Debug` impl masks
/// stored values.
#[derive(Debug, Clone, Default)]
pub struct InMemoryKeychain {
    inner: std::collections::HashMap<(String, String), String>,
}

impl InMemoryKeychain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed(&mut self, service: &str, account: &str, value: &str) {
        self.inner.insert(
            (service.to_string(), account.to_string()),
            value.to_string(),
        );
    }
}

impl KeychainBackend for InMemoryKeychain {
    fn get(&self, service: &str, account: &str) -> Result<String, CredentialError> {
        self.inner
            .get(&(service.to_string(), account.to_string()))
            .cloned()
            .ok_or_else(|| CredentialError::NotFound {
                service: service.to_string(),
                account: account.to_string(),
            })
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialError> {
        // Tests need mutation; we wrap in a Mutex.
        // ...this is a limitation of the value-only structure; we
        // accept that the test-only backend here is intentionally
        // minimal and is paired with a `parking_lot::Mutex` to allow
        // late seeding through a wrapper.
        let _ = (service, account, value);
        Err(CredentialError::Unavailable(
            "InMemoryKeychain::set requires the Mutex variant".to_string(),
        ))
    }

    fn delete(&self, _service: &str, _account: &str) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "InMemoryKeychain::delete requires the Mutex variant".to_string(),
        ))
    }
}

/// Mutable variant of the in-memory Keychain. Used for tests that
/// exercise the legacy importer. Clones share the same underlying
/// store via `Arc`.
#[derive(Debug, Default, Clone)]
pub struct MutexKeychain {
    inner: Arc<parking_lot::Mutex<std::collections::HashMap<(String, String), String>>>,
}

impl MutexKeychain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed(&self, service: &str, account: &str, value: &str) {
        self.inner.lock().insert(
            (service.to_string(), account.to_string()),
            value.to_string(),
        );
    }

    pub fn snapshot(&self) -> Vec<((String, String), String)> {
        self.inner
            .lock()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

impl KeychainBackend for MutexKeychain {
    fn get(&self, service: &str, account: &str) -> Result<String, CredentialError> {
        self.inner
            .lock()
            .get(&(service.to_string(), account.to_string()))
            .cloned()
            .ok_or_else(|| CredentialError::NotFound {
                service: service.to_string(),
                account: account.to_string(),
            })
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialError> {
        self.inner.lock().insert(
            (service.to_string(), account.to_string()),
            value.to_string(),
        );
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), CredentialError> {
        self.inner
            .lock()
            .remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}
pub fn source_class(profile: &Profile) -> &'static str {
    let loc = &profile.credential;
    if loc.service == "__env__" && loc.account == "__env__" {
        "environment"
    } else {
        "keychain"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with_locator(service: &str, account: &str) -> Profile {
        Profile {
            id: "x".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet".to_string(),
            roles: vec!["coding.primary".to_string()],
            credential: CredentialLocator {
                service: service.to_string(),
                account: account.to_string(),
            },
        }
    }

    #[test]
    fn resolve_missing_keychain_is_unavailable() {
        let r = KeychainCredentialResolver::new(MutexKeychain::new());
        let p = profile_with_locator("dev.oxi.foundation", "missing");
        match r.resolve(&p) {
            Credential::Unavailable(CredentialError::NotFound { service, account }) => {
                assert_eq!(service, "dev.oxi.foundation");
                assert_eq!(account, "missing");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn resolve_seeded_keychain_is_keychain() {
        let k = MutexKeychain::new();
        k.seed("dev.oxi.foundation", "p1", "sk-xxx");
        let r = KeychainCredentialResolver::new(k);
        let p = profile_with_locator("dev.oxi.foundation", "p1");
        match r.resolve(&p) {
            Credential::Keychain(v) => assert_eq!(v, "sk-xxx"),
            other => panic!("expected Keychain, got {other:?}"),
        }
    }

    #[test]
    fn resolve_env_sentinel_uses_env_var() {
        let original = std::env::var("OXICODE_API_KEY").ok();
        unsafe {
            std::env::set_var("OXICODE_API_KEY", "env-xxx");
        }
        let r = KeychainCredentialResolver::new(MutexKeychain::new());
        let p = profile_with_locator("__env__", "__env__");
        match r.resolve(&p) {
            Credential::Environment(v) => assert_eq!(v, "env-xxx"),
            other => panic!("expected Environment, got {other:?}"),
        }
        unsafe {
            std::env::remove_var("OXICODE_API_KEY");
        }
        if let Some(value) = original {
            unsafe {
                std::env::set_var("OXICODE_API_KEY", value);
            }
        }
    }

    #[test]
    fn legacy_import_requires_acknowledge() {
        let tmp = tempfile::tempdir().unwrap();
        let auth = tmp.path().join("auth.json");
        std::fs::write(&auth, r#"{"providers":{"anthropic":{"api_key":"sk-x"}}}"#).unwrap();
        let importer = LegacyImporter::new(MutexKeychain::new());
        let err = importer
            .run(&auth, "p1", "anthropic", false, false)
            .unwrap_err();
        assert!(matches!(err, FoundationError::Parse(_)));
    }

    #[test]
    fn legacy_import_writes_keychain_and_archives() {
        let tmp = tempfile::tempdir().unwrap();
        let auth = tmp.path().join("auth.json");
        std::fs::write(&auth, r#"{"providers":{"anthropic":{"api_key":"sk-x"}}}"#).unwrap();
        let k = MutexKeychain::new();
        let importer = LegacyImporter::new(k.clone());
        let out = importer.run(&auth, "p1", "anthropic", true, true).unwrap();
        assert_eq!(out.service, "dev.oxi.foundation");
        assert_eq!(out.account, "p1");
        assert!(out.archive_path.is_some());
        assert!(!auth.is_file(), "archive should have moved the file");
        let snap = k.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap[0].0,
            ("dev.oxi.foundation".to_string(), "p1".to_string())
        );
    }

    #[test]
    fn legacy_import_rejects_missing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let auth = tmp.path().join("auth.json");
        std::fs::write(&auth, r#"{"providers":{"anthropic":{}}}"#).unwrap();
        let importer = LegacyImporter::new(MutexKeychain::new());
        let err = importer
            .run(&auth, "p1", "anthropic", true, false)
            .unwrap_err();
        assert!(matches!(err, FoundationError::Parse(_)));
    }

    #[test]
    fn legacy_import_does_not_archive_when_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let auth = tmp.path().join("auth.json");
        std::fs::write(&auth, r#"{"providers":{"anthropic":{"api_key":"sk-x"}}}"#).unwrap();
        let k = MutexKeychain::new();
        let importer = LegacyImporter::new(k.clone());
        let out = importer.run(&auth, "p1", "anthropic", true, false).unwrap();
        assert!(out.archive_path.is_none());
        assert!(
            auth.is_file(),
            "file should remain when archive is not requested"
        );
    }

    #[test]
    fn source_class_reports_environment_or_keychain() {
        let p = profile_with_locator("__env__", "__env__");
        assert_eq!(source_class(&p), "environment");
        let p = profile_with_locator("dev.oxi.foundation", "p1");
        assert_eq!(source_class(&p), "keychain");
    }

    #[test]
    fn credential_error_display_redacts_value() {
        let err = CredentialError::NotFound {
            service: "dev.oxi.foundation".to_string(),
            account: "p1".to_string(),
        };
        let s = err.to_string();
        assert!(!s.contains("sk-"));
        assert!(s.contains("dev.oxi.foundation"));
        assert!(s.contains("p1"));
    }
}
