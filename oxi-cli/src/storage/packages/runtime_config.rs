//! `RuntimeConfig` — per-package enabled/disabled toggles for installed packages.
//!
//! Persisted as JSON under `~/.oxi/runtime.json`. Loaded on demand and
//! applied to `PackageManager::resolve_with_config` so a user can keep a
//! package installed but flip individual resource kinds off without
//! uninstalling the package.
//!
//! The structure is intentionally minimal: a `BTreeMap<package_name,
//! HashSet<ResourceKind>>` where an empty set for a package means
//! "package untouched" (all kinds enabled). Atomic writes mirror the
//! store/fs_util conventions so a crash never leaves a half-written file.
//!
//! File format (`runtime.json`):
//! ```json
//! {
//!   "version": 1,
//!   "disabled": {
//!     "@foo/oxi-tools": ["extension"],
//!     "lodash": ["prompt", "theme"]
//!   }
//! }
//! ```
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::types::ResourceKind;

/// On-disk schema version for `RuntimeConfig`.
pub const RUNTIME_CONFIG_VERSION: u32 = 1;

/// Default filename under `~/.oxi`.
pub const RUNTIME_CONFIG_FILE: &str = "runtime.json";

/// Per-package disabled-resource toggle.
///
/// An empty disabled-set for a package is equivalent to "package fully
/// enabled" (the entry can be omitted on disk). Lookups are case-sensitive
/// — package names already come from manifests and are unique.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Map of package-name -> set of disabled `ResourceKind`s.
    #[serde(default)]
    pub disabled: BTreeMap<String, HashSet<ResourceKind>>,
}

fn default_version() -> u32 {
    RUNTIME_CONFIG_VERSION
}

impl RuntimeConfig {
    /// Empty config (everything enabled).
    pub fn new() -> Self {
        Self {
            version: RUNTIME_CONFIG_VERSION,
            disabled: BTreeMap::new(),
        }
    }

    /// Resolve the canonical `~/.oxi/runtime.json` path.
    pub fn global_path() -> Result<PathBuf> {
        let base =
            dirs::home_dir().context("Cannot determine home directory for RuntimeConfig path")?;
        Ok(base.join(".oxi").join(RUNTIME_CONFIG_FILE))
    }

    /// Read from `path`. Returns `RuntimeConfig::new()` if the file is absent
    /// or unreadable-as-our-format (a corrupt file is treated as empty so
    /// the CLI keeps booting; Doctor surfaces the corruption).
    pub fn read_or_default(path: &Path) -> Self {
        match Self::read(path) {
            Ok(cfg) => cfg,
            Err(_) => Self::new(),
        }
    }

    /// Strict read — returns an error on missing or malformed file.
    pub fn read(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read RuntimeConfig at {}", path.display()))?;
        let cfg: Self = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse RuntimeConfig at {}", path.display()))?;
        Ok(cfg)
    }

    /// Atomic write. Mirrors `crate::store::fs_util::atomic_write`'s
    /// temp-file + rename policy so concurrent writers don't tear the file.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create parent dir for {}", path.display()))?;
        }
        let content =
            serde_json::to_string_pretty(self).context("Failed to serialize RuntimeConfig")?;
        let tmp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        fs::write(&tmp, content)
            .with_context(|| format!("Failed to write temp RuntimeConfig {}", tmp.display()))?;
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e).with_context(|| {
                    format!("Failed to rename RuntimeConfig to {}", path.display())
                })
            }
        }
    }

    /// True when nothing is disabled.
    pub fn is_empty(&self) -> bool {
        self.disabled.values().all(|set| set.is_empty())
    }

    /// True when this config disables `kind` for `package`.
    pub fn is_disabled(&self, package: &str, kind: ResourceKind) -> bool {
        self.disabled
            .get(package)
            .is_some_and(|set| set.contains(&kind))
    }

    /// All resource kinds still enabled for `package` (a kind is enabled
    /// unless explicitly disabled).
    pub fn enabled_kinds(&self, package: &str) -> Vec<ResourceKind> {
        let disabled = self.disabled.get(package);
        ResourceKind::ALL
            .iter()
            .copied()
            .filter(|k| !disabled.is_some_and(|set| set.contains(k)))
            .collect()
    }

    /// Disable a single kind under a package; idempotent.
    pub fn disable(&mut self, package: impl Into<String>, kind: ResourceKind) {
        let entry = self.disabled.entry(package.into()).or_default();
        entry.insert(kind);
    }

    /// Re-enable a single kind. If the entry becomes empty, the map key is
    /// dropped so a subsequent write produces the smallest possible file.
    pub fn enable(&mut self, package: &str, kind: ResourceKind) {
        if let Some(set) = self.disabled.get_mut(package) {
            set.remove(&kind);
            if set.is_empty() {
                self.disabled.remove(package);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        (dir, path)
    }

    #[test]
    fn defaults_to_all_enabled() {
        let cfg = RuntimeConfig::new();
        assert!(!cfg.is_disabled("lodash", ResourceKind::Skill));
        assert_eq!(cfg.enabled_kinds("lodash").len(), ResourceKind::ALL.len());
        assert!(cfg.is_empty());
    }

    #[test]
    fn disable_then_enable_round_trips() {
        let mut cfg = RuntimeConfig::new();
        cfg.disable("lodash", ResourceKind::Skill);
        cfg.disable("lodash", ResourceKind::Theme);
        assert!(cfg.is_disabled("lodash", ResourceKind::Skill));
        assert!(!cfg.is_disabled("lodash", ResourceKind::Extension));

        cfg.enable("lodash", ResourceKind::Skill);
        assert!(!cfg.is_disabled("lodash", ResourceKind::Skill));
        assert!(cfg.is_disabled("lodash", ResourceKind::Theme));
        assert!(!cfg.is_empty());

        cfg.enable("lodash", ResourceKind::Theme);
        assert!(cfg.is_empty(), "fully empty entry must drop the key");
    }

    #[test]
    fn write_then_read_round_trips() {
        let (_tmp, path) = tmp_path("runtime.json");
        let mut cfg = RuntimeConfig::new();
        cfg.disable("@foo/oxi-tools", ResourceKind::Extension);
        cfg.write(&path).unwrap();

        let loaded = RuntimeConfig::read(&path).unwrap();
        assert!(loaded.is_disabled("@foo/oxi-tools", ResourceKind::Extension));
        assert!(!loaded.is_disabled("@foo/oxi-tools", ResourceKind::Theme));
    }

    #[test]
    fn missing_file_yields_empty_config() {
        let (_tmp, path) = tmp_path("does-not-exist.json");
        let cfg = RuntimeConfig::read(&path).unwrap();
        assert!(cfg.is_empty());
    }

    #[test]
    fn corrupt_file_returns_empty_via_read_or_default() {
        let (_tmp, path) = tmp_path("runtime.json");
        fs::write(&path, b"not valid json").unwrap();
        // Strict read surfaces the error; read_or_default swallows it.
        assert!(RuntimeConfig::read(&path).is_err());
        let cfg = RuntimeConfig::read_or_default(&path);
        assert!(cfg.is_empty());
    }

    #[test]
    fn write_is_atomic_via_temp_then_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        let cfg = RuntimeConfig::new();
        cfg.write(&path).unwrap();
        // No leftover temp files in the parent directory.
        let stale: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name().to_string_lossy().contains("tmp.")
                    && e.file_name().to_string_lossy() != "runtime.json"
            })
            .collect();
        assert!(
            stale.is_empty(),
            "atomic write must leave no tmp. files behind; saw {:?}",
            stale.iter().map(|e| e.path()).collect::<Vec<_>>()
        );
    }
}
