//! First-run approval gate for project-scoped `[[hooks]]`.
//!
//! Project `.oxicode/settings.toml` may contain hooks that execute
//! arbitrary shell commands. To prevent supply-chain attacks via a
//! cloned repo, the cli requires the user to approve the project's
//! hook list once. Approval is cached in
//! `~/.oxicode/hooks_approved.toml` keyed by repo path + a hash of
//! the project settings file. If the settings file changes, the hash
//! mismatches and the user is re-prompted.
//!
//! The `oxicode-sdk` has no concept of "approved" — this gate is
//! purely a product-layer policy.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const APPROVAL_FILENAME: &str = "hooks_approved.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookApprovalEntry {
    /// SHA-256 of the project settings file (hex).
    pub settings_hash: String,
    /// When the user approved this combination.
    pub approved_at: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ApprovalFile {
    /// repo abs path → approval record.
    #[serde(default)]
    entries: HashMap<String, HookApprovalEntry>,
}

pub struct HookApprovalRegistry {
    path: PathBuf,
    entries: HashMap<String, HookApprovalEntry>,
}

impl std::fmt::Debug for HookApprovalRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookApprovalRegistry")
            .field("path", &self.path)
            .field("entry_count", &self.entries.len())
            .finish()
    }
}

impl HookApprovalRegistry {
    /// Load from the canonical hooks-approval file, falling back read-only
    /// to the legacy `~/.oxicode/<file>` when the canonical file is absent.
    /// If neither exists or is corrupt, return an empty registry. Persistence
    /// always targets the canonical path.
    pub fn load_or_default() -> Self {
        let Some(path) = default_approval_path().ok() else {
            return Self::empty();
        };
        let entries = approval_read_path()
            .ok()
            .filter(|p| p.exists())
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str::<ApprovalFile>(&s).ok())
            .map(|f| f.entries)
            .unwrap_or_default();
        Self { path, entries }
    }

    fn empty() -> Self {
        Self {
            path: PathBuf::new(),
            entries: HashMap::new(),
        }
    }

    /// Returns true if the given repo path + settings hash is currently
    /// approved.
    pub fn is_approved(&self, repo_path: &Path, settings_hash: &str) -> bool {
        self.entries
            .get(&canonical_key(repo_path))
            .is_some_and(|e| e.settings_hash == settings_hash)
    }

    /// Record approval for the given repo + settings hash. Caller must
    /// `persist()` afterwards.
    pub fn approve(&mut self, repo_path: &Path, settings_hash: &str) {
        self.entries.insert(
            canonical_key(repo_path),
            HookApprovalEntry {
                settings_hash: settings_hash.to_string(),
                approved_at: Utc::now(),
            },
        );
    }

    /// Atomically write the approval file to disk. Creates the parent
    /// directory if needed.
    pub fn persist(&self) -> io::Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ApprovalFile {
            entries: self.entries.clone(),
        };
        let body = toml::to_string_pretty(&file).map_err(io::Error::other)?;
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// SHA-256 (hex) of the project settings file content.
pub fn hash_settings(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}

fn default_approval_path() -> io::Result<PathBuf> {
    oxicode_catalog::oxi_home::oxicode_home()
        .map(|h| h.join(APPROVAL_FILENAME))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "oxicode home not found"))
}

/// Read path for the approval file: canonical when it exists, else the
/// legacy `~/.oxicode/<file>` when present.
fn approval_read_path() -> io::Result<PathBuf> {
    oxicode_catalog::oxi_home::read_path(Path::new(APPROVAL_FILENAME))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "oxicode home not found"))
}

fn canonical_key(p: &Path) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Read a Y/n line from stdin. Defaults to `false` (deny) on EOF or
/// parse error. This matches Claude Code's behavior of erring on the
/// safe side.
pub fn prompt_for_approval(repo_path: &Path, hook_count: usize) -> bool {
    eprintln!();
    eprintln!(
        "Project at {} wants to run {} hook(s) defined in `.oxicode/settings.toml`.",
        repo_path.display(),
        hook_count
    );
    eprintln!("Allow? [y/N]");
    eprint!("> ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn hash_is_deterministic_and_hex() {
        let h1 = hash_settings("hello");
        let h2 = hash_settings("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn empty_registry_approves_nothing() {
        let r = HookApprovalRegistry::load_or_default();
        assert!(!r.is_approved(Path::new("/tmp/nope"), "abc"));
    }

    #[test]
    fn approve_then_check_round_trip() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("hooks_approved.toml");
        let mut r = HookApprovalRegistry {
            path: p.clone(),
            entries: HashMap::new(),
        };
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        r.approve(&repo, "deadbeef");
        r.persist().unwrap();
        assert!(p.exists());

        // Re-load from disk.
        let text = std::fs::read_to_string(&p).unwrap();
        let file: ApprovalFile = toml::from_str(&text).unwrap();
        let r2 = HookApprovalRegistry {
            path: p,
            entries: file.entries,
        };
        assert!(r2.is_approved(&repo, "deadbeef"));
        assert!(!r2.is_approved(&repo, "f0000000"));
    }

    #[test]
    fn hash_mismatch_revokes_approval() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("r");
        std::fs::create_dir_all(&repo).unwrap();
        let mut r = HookApprovalRegistry {
            path: tmp.path().join("f.toml"),
            entries: HashMap::new(),
        };
        r.approve(&repo, "v1");
        assert!(r.is_approved(&repo, "v1"));
        // Settings changed → new hash → no longer approved.
        assert!(!r.is_approved(&repo, "v2"));
    }
}
