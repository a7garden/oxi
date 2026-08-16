//! `foundation.json` parsing and host-version negotiation.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::FoundationError;

const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Typed representation of `foundation.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoundationManifest {
    /// MUST be `1`.
    pub schema_version: u32,
    /// Host compatibility ranges. Unknown hosts are silently ignored.
    #[serde(default)]
    pub host_compatibility: BTreeMap<String, String>,
}

impl FoundationManifest {
    /// Validate that this `oxicode` build is compatible. The check
    /// uses a narrow semver range parser (we accept only the
    /// `>=x.y.z` form rather than the full semver grammar).
    pub fn validate_oxicode(&self) -> Result<(), FoundationError> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(FoundationError::UnsupportedSchema(self.schema_version));
        }
        let Some(spec) = self.host_compatibility.get("oxicode") else {
            // No requirement declared — accept by default.
            return Ok(());
        };
        let required = parse_minimum_version(spec).ok_or_else(|| {
            FoundationError::IncompatibleHost(format!("unparsable spec {spec:?}"))
        })?;
        let current = current_pkg_version();
        if current_compare(&current, &required) {
            Ok(())
        } else {
            Err(FoundationError::IncompatibleHost(format!(
                "oxicode {}.{}.{} < required {spec}",
                current.0, current.1, current.2
            )))
        }
    }
}

/// Read and validate `foundation.json`. Returns a typed error on
/// schema mismatch, malformed JSON, or host incompatibility.
pub fn read(path: &Path) -> Result<FoundationManifest, FoundationError> {
    let manifest: FoundationManifest = serde_json::from_slice(&std::fs::read(path)?)?;
    manifest.validate_oxicode()?;
    Ok(manifest)
}

/// Parse the minimal `>=x.y.z` form. Returns the lower bound.
fn parse_minimum_version(spec: &str) -> Option<(u64, u64, u64)> {
    let trimmed = spec.trim();
    let rest = trimmed.strip_prefix(">=")?.trim();
    let mut parts = rest.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Read the current oxicode version from `CARGO_PKG_VERSION`.
fn current_pkg_version() -> (u64, u64, u64) {
    let s = env!("CARGO_PKG_VERSION");
    let mut parts = s.split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// `current >= required` lexical comparison.
fn current_compare(current: &(u64, u64, u64), required: &(u64, u64, u64)) -> bool {
    current.0 > required.0
        || (current.0 == required.0 && current.1 > required.1)
        || (current.0 == required.0 && current.1 == required.1 && current.2 >= required.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_min_version_accepts_supported() {
        assert_eq!(parse_minimum_version(">=0.75.0"), Some((0, 75, 0)));
        assert_eq!(parse_minimum_version(">=1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_minimum_version(">=  0.75.0  "), Some((0, 75, 0)));
    }

    #[test]
    fn parse_min_version_rejects_other_forms() {
        assert_eq!(parse_minimum_version("^0.75.0"), None);
        assert_eq!(parse_minimum_version("~0.75.0"), None);
        assert_eq!(parse_minimum_version("0.75.0"), None);
        assert_eq!(parse_minimum_version(">=0.75"), None);
        assert_eq!(parse_minimum_version(">=0.75.0.1"), None);
    }

    #[test]
    fn read_rejects_missing_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("foundation.json");
        std::fs::write(&path, "{}").unwrap();
        let err = read(&path).unwrap_err();
        assert!(matches!(err, FoundationError::Parse(_)));
    }

    #[test]
    fn read_rejects_unsupported_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("foundation.json");
        std::fs::write(&path, r#"{"schema_version": 99}"#).unwrap();
        let err = read(&path).unwrap_err();
        assert!(matches!(err, FoundationError::UnsupportedSchema(99)));
    }

    #[test]
    fn read_accepts_compatible_host() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("foundation.json");
        std::fs::write(
            &path,
            r#"{"schema_version": 1, "host_compatibility": {"oxicode": ">=0.50.0"}}"#,
        )
        .unwrap();
        let m = read(&path).unwrap();
        assert_eq!(m.schema_version, 1);
    }

    #[test]
    fn read_rejects_incompatible_host() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("foundation.json");
        std::fs::write(
            &path,
            r#"{"schema_version": 1, "host_compatibility": {"oxicode": ">=999.0.0"}}"#,
        )
        .unwrap();
        let err = read(&path).unwrap_err();
        assert!(matches!(err, FoundationError::IncompatibleHost(_)));
    }
}
