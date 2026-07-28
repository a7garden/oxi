//! Lockfile types and integrity helpers.
//!
//! `Lockfile` is the on-disk record (`oxi-lock.json`) that pins every
//! installed package to its exact source, version, scope, and a SHA-256
//! integrity hash. `ResourceCounts` is a small summary struct exposed
//! for the CLI's list output. The SHA-256 helpers (`compute_dir_hash`,
//! `verify_lockfile_integrity`, `collect_file_paths`) live here too —
//! they're lockfile-integrity machinery, not generic FS utilities.

use super::types::SourceScope;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Lockfile entry for an installed package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    /// Source specifier
    pub source: String,
    /// Package name
    pub name: String,
    /// Resolved version or ref
    pub version: String,
    /// Integrity hash (sha256)
    pub integrity: Option<String>,
    /// Scope
    pub scope: SourceScope,
    /// Type of source
    pub source_type: String,
    /// Dependencies
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

/// The lockfile structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile version
    pub version: u32,
    /// Locked packages
    pub packages: BTreeMap<String, LockEntry>,
}

impl Lockfile {
    /// Create a new empty lockfile
    pub fn new() -> Self {
        Self {
            version: 1,
            packages: BTreeMap::new(),
        }
    }

    /// Read lockfile from disk
    pub fn read(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read lockfile {}", path.display()))?;
        let lock: Lockfile = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse lockfile {}", path.display()))?;
        Ok(Some(lock))
    }

    /// Write lockfile to disk
    pub fn write(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self).context("Failed to serialize lockfile")?;
        fs::write(path, content)
            .with_context(|| format!("Failed to write lockfile {}", path.display()))?;
        Ok(())
    }

    /// Add or update an entry
    pub fn insert(&mut self, entry: LockEntry) {
        self.packages.insert(entry.name.clone(), entry);
    }

    /// Remove an entry
    pub fn remove(&mut self, name: &str) -> Option<LockEntry> {
        self.packages.remove(name)
    }

    /// Check if a package is locked
    pub fn contains(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    /// Get an entry
    pub fn get(&self, name: &str) -> Option<&LockEntry> {
        self.packages.get(name)
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

/// Counts of each resource type in a package
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceCounts {
    /// pub.
    pub extensions: usize,
    /// pub.
    pub skills: usize,
    /// pub.
    pub prompts: usize,
    /// pub.
    pub themes: usize,
}

impl std::fmt::Display for ResourceCounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.extensions > 0 {
            parts.push(format!("{} ext", self.extensions));
        }
        if self.skills > 0 {
            parts.push(format!("{} skill", self.skills));
        }
        if self.prompts > 0 {
            parts.push(format!("{} prompt", self.prompts));
        }
        if self.themes > 0 {
            parts.push(format!("{} theme", self.themes));
        }
        if parts.is_empty() {
            write!(f, "-")?;
        } else {
            write!(f, "{}", parts.join(", "))?;
        }
        Ok(())
    }
}

/// Compute a SHA-256 hash of a directory's contents for integrity checking
pub(crate) fn compute_dir_hash(dir: &Path) -> Option<String> {
    let mut hasher = Sha256::new();
    let mut files = collect_file_paths(dir);
    files.sort();

    for file_path in &files {
        if let Ok(content) = fs::read(file_path) {
            hasher.update(&content);
        }
    }

    let result = hasher.finalize();
    Some(format!("sha256-{:x}", result))
}

/// Verify that an installed package directory matches its lockfile integrity hash.
///
/// `expected` must be in the `"sha256-<hex>"` format produced by
/// [`compute_dir_hash`]. Returns `Ok(())` on match, `Err(reason)` on
/// mismatch, missing directory, or hash format error. Missing files are
/// skipped silently (same as `compute_dir_hash`) so a partially-installed
/// package still hashes consistently with its lockfile.
///
/// This is the *consumer-side* companion to `compute_dir_hash`: the writer
/// (install) computes and stores; the reader (load) recomputes and compares.
/// Before this function existed (audit finding F-1), `integrity` was a
/// write-only field and a local attacker could swap files under
/// `~/.oxi/packages/<name>/` without detection.
pub(crate) fn verify_lockfile_integrity(install_dir: &Path, expected: &str) -> Result<(), String> {
    let expected_hex = expected.strip_prefix("sha256-").ok_or_else(|| {
        format!("lockfile integrity value not in `sha256-<hex>` form: {expected}")
    })?;

    let actual = compute_dir_hash(install_dir)
        .ok_or_else(|| format!("could not hash install dir {}", install_dir.display()))?;
    let actual_hex = actual
        .strip_prefix("sha256-")
        .ok_or_else(|| format!("recomputed hash not in expected form: {actual}"))?;

    if actual_hex.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "sha256 mismatch: expected sha256-{expected_hex}, got {actual_hex}"
        ))
    }
}

/// Collect all file paths in a directory recursively
pub(crate) fn collect_file_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if !dir.exists() {
        return paths;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return paths,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            paths.extend(collect_file_paths(&path));
        } else {
            paths.push(path);
        }
    }

    paths
}
