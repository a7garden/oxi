//! `ProjectPluginOverrides` — project-scoped force on/off for resources.
//!
//! Stored under `<project>/.oxi/plugin-overrides.json`. Distinct from the
//! user-level `RuntimeConfig`: project overrides take precedence so that
//! `oxi` policy can force a resource ON or OFF regardless of what the user
//! toggled globally. The combined resolution order is:
//!
//! 1. Project `Forced::On` / `Forced::Off` (highest, applies to both
//!    installed and would-be-disabled resources)
//! 2. User-level `RuntimeConfig::disabled`
//! 3. Default — resource is enabled
//!
//! File format:
//! ```json
//! {
//!   "version": 1,
//!   "forced": {
//!     "@foo/oxi-tools": {
//!       "extension": "off",
//!       "skill": "on"
//!     },
//!     "lodash": { "prompt": "off" }
//!   }
//! }
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::types::ResourceKind;

/// Schema version for `ProjectPluginOverrides`.
pub const OVERRIDES_VERSION: u32 = 1;

/// Default filename under `<project>/.oxi/`.
pub const OVERRIDES_FILE: &str = "plugin-overrides.json";

/// Tri-state project force for a single (package, kind) pair.
///
/// `On` / `Off` take precedence over user-level `RuntimeConfig.disabled`
/// and over the default (`enabled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForceState {
    /// Force the resource enabled (project overrides user-disable).
    On,
    /// Force the resource disabled (project overrides default-on).
    Off,
}

impl std::fmt::Display for ForceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForceState::On => write!(f, "on"),
            ForceState::Off => write!(f, "off"),
        }
    }
}

/// Map from package name to per-kind `ForceState`. Keys present in the
/// map override anything below them in the precedence chain.
pub type ForceMap = BTreeMap<String, BTreeMap<ResourceKind, ForceState>>;

/// Per-project plugin override file.
///
/// A project is "the directory containing `.oxi/`". Path is supplied by
/// the caller (typically `PackageManager::project_dir`); the file itself
/// is optional — missing-file is the no-overrides case.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectPluginOverrides {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Per-package, per-kind force map.
    #[serde(default)]
    pub forced: ForceMap,
}

fn default_version() -> u32 {
    OVERRIDES_VERSION
}

impl ProjectPluginOverrides {
    /// Empty overrides (everything follows `RuntimeConfig`/default).
    pub fn new() -> Self {
        Self {
            version: OVERRIDES_VERSION,
            forced: BTreeMap::new(),
        }
    }

    /// Canonical path: `<project_dir>/.oxi/<OVERRIDES_FILE>`.
    pub fn project_path(project_dir: &Path) -> PathBuf {
        project_dir.join(".oxi").join(OVERRIDES_FILE)
    }

    /// Strict read — missing file is `Ok(Empty)`, corrupt file is an
    /// error so Doctor can surface it (callers wanting best-effort use
    /// `read_or_default`).
    pub fn read(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read overrides at {}", path.display()))?;
        let cfg: Self = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse overrides at {}", path.display()))?;
        Ok(cfg)
    }

    /// Lenient read: missing or corrupt -> empty overrides. Doctor is
    /// the structured channel for surfacing corruption.
    pub fn read_or_default(path: &Path) -> Self {
        Self::read(path).unwrap_or_default()
    }

    /// Atomic write matching the `RuntimeConfig::write` policy.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create parent dir for {}", path.display()))?;
        }
        let content =
            serde_json::to_string_pretty(self).context("Failed to serialize overrides")?;
        let tmp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        fs::write(&tmp, content)
            .with_context(|| format!("Failed to write tmp overrides {}", tmp.display()))?;
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e).with_context(|| format!("Failed to rename overrides to {}", path.display()))
            }
        }
    }

    /// Number of (package, kind) override entries.
    pub fn entry_count(&self) -> usize {
        self.forced.values().map(|m| m.len()).sum()
    }

    /// True when no overrides are set.
    pub fn is_empty(&self) -> bool {
        self.forced.values().all(|m| m.is_empty())
    }

    /// The forced state for `(package, kind)`, if any.
    pub fn forced_state(&self, package: &str, kind: ResourceKind) -> Option<ForceState> {
        self.forced.get(package).and_then(|m| m.get(&kind)).copied()
    }

    /// Force a kind on under a package (project layer can rescue a
    /// resource the user disabled globally).
    pub fn force_on(&mut self, package: impl Into<String>, kind: ResourceKind) {
        self.forced
            .entry(package.into())
            .or_default()
            .insert(kind, ForceState::On);
    }

    /// Force a kind off under a package.
    pub fn force_off(&mut self, package: impl Into<String>, kind: ResourceKind) {
        self.forced
            .entry(package.into())
            .or_default()
            .insert(kind, ForceState::Off);
    }

    /// Drop the override for `(package, kind)`. Keys with empty inner
    /// maps are also removed so the persisted file stays minimal.
    pub fn clear(&mut self, package: &str, kind: ResourceKind) {
        if let Some(inner) = self.forced.get_mut(package) {
            inner.remove(&kind);
            if inner.is_empty() {
                self.forced.remove(package);
            }
        }
    }
}

/// Resolve the enabled flag for `(package, kind)` under the layered
/// precedence: project-force > runtime-disable > default(true).
///
/// Pure, single-purpose helper used by `PackageManager::resolve_with_config`
/// and the Doctor summary views. Pulled out so it's testable without
/// touching the filesystem.
pub fn resolve_enabled(
    package: &str,
    kind: ResourceKind,
    overrides: Option<&ProjectPluginOverrides>,
    runtime: Option<&RuntimeConfig>,
) -> bool {
    if let Some(o) = overrides
        && let Some(state) = o.forced_state(package, kind)
    {
        return match state {
            ForceState::On => true,
            ForceState::Off => false,
        };
    }
    if let Some(r) = runtime
        && r.is_disabled(package, kind)
    {
        return false;
    }
    true
}

// Reference `RuntimeConfig` for callers using `resolve_enabled` from
// sibling modules — keeps the import surface obvious without creating a
// cycle.
use super::runtime_config::RuntimeConfig;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_enabled() {
        let o = ProjectPluginOverrides::new();
        assert!(o.is_empty());
        assert_eq!(o.entry_count(), 0);
    }

    #[test]
    fn force_on_and_force_off_track_separately() {
        let mut o = ProjectPluginOverrides::new();
        o.force_on("@foo/oxi-tools", ResourceKind::Extension);
        o.force_off("@foo/oxi-tools", ResourceKind::Skill);
        o.force_off("lodash", ResourceKind::Prompt);

        assert_eq!(o.entry_count(), 3);
        assert_eq!(
            o.forced_state("@foo/oxi-tools", ResourceKind::Extension),
            Some(ForceState::On)
        );
        assert_eq!(
            o.forced_state("@foo/oxi-tools", ResourceKind::Skill),
            Some(ForceState::Off)
        );
        assert_eq!(
            o.forced_state("lodash", ResourceKind::Prompt),
            Some(ForceState::Off)
        );
        assert_eq!(o.forced_state("lodash", ResourceKind::Theme), None);
    }

    #[test]
    fn clear_drops_empty_outer_keys() {
        let mut o = ProjectPluginOverrides::new();
        o.force_on("pkg", ResourceKind::Skill);
        o.clear("pkg", ResourceKind::Skill);
        assert!(o.is_empty());
        assert_eq!(o.entry_count(), 0);
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin-overrides.json");
        let mut o = ProjectPluginOverrides::new();
        o.force_on("lodash", ResourceKind::Skill);
        o.force_off("@scope/x", ResourceKind::Extension);
        o.write(&path).unwrap();

        let loaded = ProjectPluginOverrides::read(&path).unwrap();
        assert_eq!(loaded.entry_count(), 2);
        assert_eq!(
            loaded.forced_state("lodash", ResourceKind::Skill),
            Some(ForceState::On)
        );
        assert_eq!(
            loaded.forced_state("@scope/x", ResourceKind::Extension),
            Some(ForceState::Off)
        );
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        let o = ProjectPluginOverrides::read(&path).unwrap();
        assert!(o.is_empty());
    }

    #[test]
    fn corrupt_file_reads_as_empty_via_lenient_helper() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin-overrides.json");
        fs::write(&path, b"{ not valid").unwrap();
        assert!(ProjectPluginOverrides::read(&path).is_err());
        let o = ProjectPluginOverrides::read_or_default(&path);
        assert!(o.is_empty());
    }

    // ── Precedence tests ─────────────────────────────────────────────

    fn build_layers() -> (ProjectPluginOverrides, RuntimeConfig) {
        let mut o = ProjectPluginOverrides::new();
        let mut r = RuntimeConfig::new();
        // pkg-A: project forces Off on extension; user also disabled (project wins → off)
        o.force_off("pkg-A", ResourceKind::Extension);
        r.disable("pkg-A", ResourceKind::Extension);
        // pkg-B: project forces On on skill; user disabled (project wins → on)
        o.force_on("pkg-B", ResourceKind::Skill);
        r.disable("pkg-B", ResourceKind::Skill);
        // pkg-C: project has no entry; user disabled (only runtime wins → off)
        r.disable("pkg-C", ResourceKind::Prompt);
        // pkg-D: project forces Off on extension; user has no entry (project wins → off)
        o.force_off("pkg-D", ResourceKind::Extension);
        // pkg-E: project forces On; user has no entry (project wins → on)
        o.force_on("pkg-E", ResourceKind::Theme);
        // pkg-F: project && user say nothing (default on)
        (o, r)
    }

    #[test]
    fn precedence_project_force_off_overrides_user_disable() {
        let (o, r) = build_layers();
        // pkg-A: both layers disable; project-force off sticks.
        assert!(!resolve_enabled(
            "pkg-A",
            ResourceKind::Extension,
            Some(&o),
            Some(&r),
        ));
    }

    #[test]
    fn precedence_project_force_on_overrides_user_disable() {
        let (o, r) = build_layers();
        // pkg-B: user disables, project forces on. Project wins — enabled.
        assert!(resolve_enabled(
            "pkg-B",
            ResourceKind::Skill,
            Some(&o),
            Some(&r),
        ));
    }

    #[test]
    fn precedence_user_disable_holds_when_no_project_entry() {
        let (o, r) = build_layers();
        // pkg-C has no project entry; runtime disables prompt.
        assert!(!resolve_enabled(
            "pkg-C",
            ResourceKind::Prompt,
            Some(&o),
            Some(&r),
        ));
    }

    #[test]
    fn precedence_project_force_off_without_user_entry() {
        let (o, r) = build_layers();
        assert!(!resolve_enabled(
            "pkg-D",
            ResourceKind::Extension,
            Some(&o),
            Some(&r),
        ));
    }

    #[test]
    fn precedence_project_force_on_without_user_entry() {
        let (o, r) = build_layers();
        assert!(resolve_enabled(
            "pkg-E",
            ResourceKind::Theme,
            Some(&o),
            Some(&r),
        ));
    }

    #[test]
    fn precedence_default_enabled_when_neither_layer_speaks() {
        let (o, r) = build_layers();
        assert!(resolve_enabled(
            "pkg-F",
            ResourceKind::Skill,
            Some(&o),
            Some(&r),
        ));
    }

    #[test]
    fn precedence_falls_back_to_runtime_when_no_overrides() {
        let mut r = RuntimeConfig::new();
        r.disable("only-user", ResourceKind::Theme);
        assert!(!resolve_enabled(
            "only-user",
            ResourceKind::Theme,
            None,
            Some(&r),
        ));
        // Different (package, kind) pair, no override, no disable: default on.
        assert!(resolve_enabled(
            "only-user",
            ResourceKind::Skill,
            None,
            Some(&r),
        ));
    }

    #[test]
    fn precedence_default_enabled_when_no_layers_at_all() {
        assert!(resolve_enabled("any", ResourceKind::Skill, None, None));
    }
}
