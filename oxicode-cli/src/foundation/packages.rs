//! `packages.lock` parsing, digest verification, and capability mapping.
//!
//! A foundation package is a verified, immutable record. oxicode
//! reads the lockfile, verifies each package's on-disk content
//! against the recorded digest, and decides (a) whether the package
//! is eligible to load and (b) which of oxicode's existing policy
//! gates must approve the declared requirements.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::FoundationError;

/// Abstract requirement declared by a foundation package. oxicode
/// maps these to its existing policy in [`map_requirement`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    /// Read-only filesystem access within the workspace.
    WorkspaceRead,
    /// Patching files inside the workspace.
    WorkspacePatch,
    /// Executing shell commands.
    ShellExecute,
    /// Browser navigation (native-browser feature).
    BrowserNavigate,
    /// Brain-backed retrieval (oxibrain).
    BrainQuery,
    /// Schedule management.
    ScheduleManage,
}

impl Requirement {
    /// Parse a dotted string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "workspace.read" => Some(Self::WorkspaceRead),
            "workspace.patch" => Some(Self::WorkspacePatch),
            "shell.execute" => Some(Self::ShellExecute),
            "browser.navigate" => Some(Self::BrowserNavigate),
            "brain.query" => Some(Self::BrainQuery),
            "schedule.manage" => Some(Self::ScheduleManage),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WorkspaceRead => "workspace.read",
            Self::WorkspacePatch => "workspace.patch",
            Self::ShellExecute => "shell.execute",
            Self::BrowserNavigate => "browser.navigate",
            Self::BrainQuery => "brain.query",
            Self::ScheduleManage => "schedule.manage",
        }
    }
}

/// Trust decision recorded in the lockfile. Anything other than
/// `verified` is rejected at load time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    Verified,
    Unverified,
}

/// A single resolved package record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    /// `sha256-<hex>`.
    pub digest: String,
    pub source: String,
    pub trust: Trust,
    /// Host ids the package claims to work with. MUST include `oxicode`.
    pub targets: Vec<String>,
    /// Abstract requirements; empty if the package has none.
    #[serde(default)]
    pub requirements: Vec<String>,
}

/// Typed `packages.lock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagesFile {
    pub schema_version: u32,
    #[serde(default)]
    pub packages: Vec<LockedPackage>,
}

/// Result of mapping a package requirement to oxicode's policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityDecision {
    /// Allow the requirement.
    Allow,
    /// Deny the requirement. The package must be rejected.
    Deny(&'static str),
    /// The requirement is host-supplied (e.g. `brain.query`) and the
    /// host feature isn't enabled. Either reject or surface a
    /// user-visible hint.
    Unsupported(&'static str),
}

/// Read and verify `packages.lock`. The on-disk content under
/// `packages_root` is verified against the declared digests.
pub fn read(lock: &Path, packages_root: &Path) -> Result<PackagesFile, FoundationError> {
    let raw = std::fs::read_to_string(lock)?;
    let parsed: PackagesFile = serde_json::from_str(&raw)?;
    if parsed.schema_version != 1 {
        return Err(FoundationError::UnsupportedSchema(parsed.schema_version));
    }
    for p in &parsed.packages {
        if !p.targets.iter().any(|t| t == "oxicode") {
            return Err(FoundationError::TargetMismatch {
                package: p.name.clone(),
                targets: p.targets.clone(),
            });
        }
        if !matches!(p.trust, Trust::Verified) {
            return Err(FoundationError::Parse(format!(
                "package {} trust is not `verified`",
                p.name
            )));
        }
        for r in &p.requirements {
            if Requirement::parse(r).is_none() {
                return Err(FoundationError::UnsupportedRequirement(r.clone()));
            }
        }
        // Verify the package content on disk. Path is
        // `packages_root/<sha256>/`, where `<sha256>` is the hex
        // part of the digest.
        let hex = p
            .digest
            .strip_prefix("sha256-")
            .ok_or_else(|| FoundationError::Parse(format!("bad digest format: {}", p.digest)))?;
        let content_dir = packages_root.join(hex);
        if !content_dir.is_dir() {
            return Err(FoundationError::DigestMismatch {
                package: p.name.clone(),
                expected: p.digest.clone(),
                actual: "missing".to_string(),
            });
        }
        let actual = compute_dir_digest(&content_dir).unwrap_or_else(|| "missing".to_string());
        if !actual.eq_ignore_ascii_case(&p.digest) {
            return Err(FoundationError::DigestMismatch {
                package: p.name.clone(),
                expected: p.digest.clone(),
                actual,
            });
        }
    }
    Ok(parsed)
}

/// Compute a `sha256-<hex>` digest over the package content. The
/// order is stable: filenames are sorted lexicographically.
fn compute_dir_digest(dir: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut paths = Vec::new();
    collect_paths(dir, &mut paths);
    paths.sort();
    let mut hasher = Sha256::new();
    for p in paths {
        if let Ok(content) = std::fs::read(p) {
            hasher.update(&content);
        }
    }
    let result = hasher.finalize();
    Some(format!("sha256-{result:x}"))
}

fn collect_paths(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_paths(&path, out);
        } else if let Some(name) = path.file_name() {
            // Skip the manifest filename to keep the digest stable
            // across reinstalls that share the same content.
            if name == "MANIFEST" || name == "manifest.json" {
                continue;
            }
            out.push(path);
        }
    }
}

/// Map a parsed requirement to oxicode's policy. The decision is
/// read-only — it does not mutate state. The caller is responsible
/// for translating `Allow` into the right port wiring.
pub fn map_requirement(req: Requirement) -> CapabilityDecision {
    match req {
        Requirement::WorkspaceRead => CapabilityDecision::Allow,
        Requirement::WorkspacePatch => CapabilityDecision::Allow,
        Requirement::ShellExecute => CapabilityDecision::Allow,
        Requirement::BrowserNavigate => CapabilityDecision::Allow,
        Requirement::BrainQuery => CapabilityDecision::Allow,
        Requirement::ScheduleManage => CapabilityDecision::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, digest: &str, requirements: &[&str]) -> LockedPackage {
        LockedPackage {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            digest: digest.to_string(),
            source: "foundation".to_string(),
            trust: Trust::Verified,
            targets: vec!["oxicode".to_string()],
            requirements: requirements.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn parse_requirement_roundtrip() {
        for r in [
            Requirement::WorkspaceRead,
            Requirement::WorkspacePatch,
            Requirement::ShellExecute,
            Requirement::BrowserNavigate,
            Requirement::BrainQuery,
            Requirement::ScheduleManage,
        ] {
            assert_eq!(Requirement::parse(r.as_str()), Some(r));
        }
    }

    #[test]
    fn parse_requirement_unknown() {
        assert_eq!(Requirement::parse(""), None);
        assert_eq!(Requirement::parse("fs.read"), None);
    }

    #[test]
    fn rejects_missing_target() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("packages.lock");
        let content_dir = tmp.path().join("packages");
        std::fs::create_dir(&content_dir).unwrap();
        let p = LockedPackage {
            name: "x".to_string(),
            version: "1.0.0".to_string(),
            digest: "sha256-deadbeef".to_string(),
            source: "foundation".to_string(),
            trust: Trust::Verified,
            targets: vec!["oxibrain".to_string()],
            requirements: vec![],
        };
        let file = PackagesFile {
            schema_version: 1,
            packages: vec![p],
        };
        std::fs::write(&lock, serde_json::to_string(&file).unwrap()).unwrap();
        let err = read(&lock, &content_dir).unwrap_err();
        assert!(matches!(err, FoundationError::TargetMismatch { .. }));
    }

    #[test]
    fn rejects_unknown_requirement() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("packages.lock");
        let content_dir = tmp.path().join("packages");
        std::fs::create_dir(&content_dir).unwrap();
        let mut p = pkg("x", "sha256-deadbeef", &["workspace.invalid"]);
        p.targets = vec!["oxicode".to_string()];
        let file = PackagesFile {
            schema_version: 1,
            packages: vec![p],
        };
        std::fs::write(&lock, serde_json::to_string(&file).unwrap()).unwrap();
        let err = read(&lock, &content_dir).unwrap_err();
        assert!(matches!(err, FoundationError::UnsupportedRequirement(_)));
    }

    #[test]
    fn rejects_digest_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("packages.lock");
        let content_dir = tmp.path().join("packages");
        // The on-disk hash will be computed; we just need a directory
        // whose name matches the digest so the check actually runs.
        let hex = "f000000000000000000000000000000000000000000000000000000000000000";
        let pkg_dir = content_dir.join(hex);
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("file.txt"), "hello").unwrap();
        let p = LockedPackage {
            name: "x".to_string(),
            version: "1.0.0".to_string(),
            digest: format!("sha256-{hex}"),
            source: "foundation".to_string(),
            trust: Trust::Verified,
            targets: vec!["oxicode".to_string()],
            requirements: vec![],
        };
        let file = PackagesFile {
            schema_version: 1,
            packages: vec![p],
        };
        std::fs::write(&lock, serde_json::to_string(&file).unwrap()).unwrap();
        let err = read(&lock, &content_dir).unwrap_err();
        assert!(matches!(err, FoundationError::DigestMismatch { .. }));
    }

    #[test]
    fn accepts_valid_package() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("packages.lock");
        let content_dir = tmp.path().join("packages");
        std::fs::create_dir(&content_dir).unwrap();
        // Compute the real digest of the content we wrote.
        let content = b"hello".to_vec();
        use sha2::{Digest, Sha256};
        let actual = format!("sha256-{:x}", Sha256::digest(&content));
        let hex = actual.strip_prefix("sha256-").unwrap().to_string();
        let pkg_dir = content_dir.join(hex);
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("file.txt"), &content).unwrap();
        let p = LockedPackage {
            name: "x".to_string(),
            version: "1.0.0".to_string(),
            digest: actual,
            source: "foundation".to_string(),
            trust: Trust::Verified,
            targets: vec!["oxicode".to_string()],
            requirements: vec!["workspace.read".to_string(), "brain.query".to_string()],
        };
        let file = PackagesFile {
            schema_version: 1,
            packages: vec![p],
        };
        std::fs::write(&lock, serde_json::to_string(&file).unwrap()).unwrap();
        let parsed = read(&lock, &content_dir).unwrap();
        assert_eq!(parsed.packages.len(), 1);
        assert_eq!(parsed.packages[0].name, "x");
    }

    #[test]
    fn map_requirement_allows_known() {
        for r in [
            Requirement::WorkspaceRead,
            Requirement::WorkspacePatch,
            Requirement::ShellExecute,
            Requirement::BrowserNavigate,
            Requirement::BrainQuery,
            Requirement::ScheduleManage,
        ] {
            assert_eq!(map_requirement(r), CapabilityDecision::Allow);
        }
    }
}
