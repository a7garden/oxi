//! One-time legacy compatibility import.
//!
//! While the migration is enabled (`OXICODE_FOUNDATION_MIGRATION=1`),
//! oxicode reads a single legacy profile from a host-provided
//! compatibility shim, writes a structured migration marker, and
//! resolves the profile through the same [`resolve_profile`]
//! decision function as a normal Foundation profile.
//!
//! Defaults: disabled. The importer never reads from `~/.oxicode/auth.json`
//! on its own — the user explicitly acknowledges the import via
//! `oxicode memory migrate-brain` / `oxicode config migrate-foundation`.

use std::path::Path;

use crate::foundation::FoundationError;
use crate::foundation::profiles::{CompatibilityImport, Profile};

/// `true` when the migration flag is set.
pub fn migration_enabled() -> bool {
    matches!(
        std::env::var("OXICODE_FOUNDATION_MIGRATION").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

/// Read the legacy compatibility shim. The shim is a small JSON file
/// that lives under `~/.oxi/foundation/v1/compatibility.json` and is
/// produced by the host's compatibility installer. oxicode never
/// fabricates one.
pub fn read_compatibility_shim(
    path: &Path,
) -> Result<Option<CompatibilityImport>, FoundationError> {
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let profile: Profile = serde_json::from_str(&raw)?;
    profile.credential.validate()?;
    Ok(Some(CompatibilityImport { profile }))
}

/// Write a migration marker. The marker is a JSON file under the
/// foundation root that records who/what was migrated. The marker is
/// for human auditing, not for runtime decisions.
pub fn write_migration_marker(root: &Path, profile_id: &str) -> Result<(), FoundationError> {
    let path = root.join("migration.marker.json");
    let body = serde_json::json!({
        "profile_id": profile_id,
        "migrated_at": chrono::Utc::now().to_rfc3339(),
        "migrated_by": "oxicode",
    });
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&body).map_err(|e| FoundationError::Parse(e.to_string()))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::profiles::CredentialLocator;

    #[test]
    fn migration_disabled_by_default() {
        // The test runner may have the env var set; tolerate that.
        let original = std::env::var("OXICODE_FOUNDATION_MIGRATION").ok();
        unsafe {
            std::env::remove_var("OXICODE_FOUNDATION_MIGRATION");
        }
        assert!(!migration_enabled());
        if let Some(value) = original {
            unsafe {
                std::env::set_var("OXICODE_FOUNDATION_MIGRATION", value);
            }
        }
    }

    #[test]
    fn migrate_marker_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        write_migration_marker(tmp.path(), "legacy").unwrap();
        let body = std::fs::read_to_string(tmp.path().join("migration.marker.json")).unwrap();
        assert!(body.contains("legacy"));
    }

    #[test]
    fn shim_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let opt = read_compatibility_shim(&tmp.path().join("missing.json")).unwrap();
        assert!(opt.is_none());
    }

    #[test]
    fn shim_parses_minimal_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("compatibility.json");
        let raw = r#"{
            "id": "legacy",
            "provider": "anthropic",
            "model": "claude-sonnet",
            "roles": ["coding.primary"],
            "credential": { "service": "dev.oxi.foundation", "account": "legacy" }
        }"#;
        std::fs::write(&path, raw).unwrap();
        let import = read_compatibility_shim(&path).unwrap().unwrap();
        assert_eq!(import.profile.provider, "anthropic");
        assert_eq!(
            import.profile.credential,
            CredentialLocator {
                service: "dev.oxi.foundation".to_string(),
                account: "legacy".to_string(),
            }
        );
    }
}
