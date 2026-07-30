//! `Doctor` — structured health checks for the package subsystem.
//!
//! Replaces the ad-hoc `validate_package: Result<Vec<String>>` warnings
//! shape with structured `DoctorCheck` rows that carry:
//! - a stable `id` so a UI or automation script can target them,
//! - a `Health` severity (`Healthy` | `Warning` | `Error`),
//! - a human-readable `message` and an optional remediation `hint`.
//!
//! Doctor is the single place where the manager speaks up about
//! missing files, malformed manifests, missing resources, or integrity
//! mismatches. It composes the existing helpers (`read_manifest`,
//! `verify_lockfile_integrity`, `discover_resources`) — it does not
//! re-implement them.
//!
//! Inputs to `run()`:
//! - `RuntimeConfig::read_or_default` for the user-level enabled state
//! - `ProjectPluginOverrides::read_or_default` for the project-level forces
//! - the manager's `installed`/`lockfile` state (already on disk)
//!
//! Doctor stays *read-only*: no install/uninstall side-effects, no
//! filesystem mutations.

use super::MANIFEST_NAME;
use super::lockfile::verify_lockfile_integrity;
use super::overrides::{ProjectPluginOverrides, resolve_enabled};
use super::runtime_config::RuntimeConfig;
use super::types::ResourceKind;
use crate::storage::packages::manager::PackageManager;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Severity of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// The check passed; the package is healthy.
    Healthy,
    /// The check surfaced a non-fatal issue (e.g. integrity mismatch
    /// silently dropped the package; a missing optional resource).
    Warning,
    /// The check found a blocker that should prevent the package from
    /// being treated as installed (manifest corrupt, etc.).
    Error,
}

impl std::fmt::Display for Health {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Health::Healthy => write!(f, "healthy"),
            Health::Warning => write!(f, "warning"),
            Health::Error => write!(f, "error"),
        }
    }
}

/// A single structured check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    /// Stable identifier (e.g. `"manifest.parse.test-pkg"`).
    pub id: String,
    /// Subject of the check (usually the package name).
    pub subject: String,
    /// Optional resource-kind context.
    pub kind: Option<ResourceKind>,
    /// Severity level.
    pub health: Health,
    /// Human-readable summary.
    pub message: String,
    /// Optional remediation hint for the user.
    pub hint: Option<String>,
}

/// Aggregated doctor report. A report is `all_clear` when every check
/// is `Healthy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Per-package, per-resource health checks.
    pub checks: Vec<DoctorCheck>,
    /// Resolved enabled state per (package, kind), for callers that
    /// want to render the configuration outcome.
    pub resolved_enabled: Vec<EnabledSummary>,
    /// Overall verdict.
    pub all_clear: bool,
}

/// Convenience summary showing the layered precedence outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnabledSummary {
    /// Package name.
    pub package: String,
    /// Resource kind.
    pub kind: ResourceKind,
    /// Resolved enabled state (final, after precedence).
    pub enabled: bool,
    /// `Some(s)` when the project layer forced the value.
    pub project_forced: Option<String>,
    /// `true` when the user-level runtime layer disabled the resource.
    pub user_disabled: bool,
}

/// Construct a Doctor directly from a `PackageManager` reference.
///
/// `runtime_path` / `overrides_path` are absolute paths passed in so the
/// caller controls which configuration sources are read — important for
/// tests and for the future CLI subcommand that operates against a
/// different project root.
pub fn run(
    manager: &PackageManager,
    runtime_path: &Path,
    overrides_path: &Path,
) -> Result<DoctorReport> {
    let runtime = RuntimeConfig::read_or_default(runtime_path);
    let overrides = ProjectPluginOverrides::read_or_default(overrides_path);

    let mut checks = Vec::new();
    let mut resolved = Vec::new();

    // Enumerate every installed package name (manifest-known AND
    // lockfile-known), so we surface both:
    //  - manifest-known: integrity may have dropped them silently.
    //  - lockfile-only: install left a row but the package dir failed.
    let mut names: Vec<String> = manager.list().iter().map(|m| m.name.clone()).collect();
    names.extend(
        manager
            .lockfile()
            .packages
            .keys()
            .filter(|k| !manager.list().iter().any(|m| &m.name == *k))
            .cloned(),
    );
    names.sort();

    for name in &names {
        check_manifest(manager, name, &mut checks);
        check_lockfile_integrity(manager, name, &mut checks);
        check_resources_present(manager, name, &mut checks);
        for kind in ResourceKind::ALL {
            let (enabled, summary) = compute_enabled_state(name, kind, &runtime, &overrides);
            resolved.push(summary);
            // Healthy summary row: enabled/disabled correctly resolved.
            checks.push(DoctorCheck {
                id: format!("resolve.{}.{}", name, kind),
                subject: name.clone(),
                kind: Some(kind),
                health: Health::Healthy,
                message: format!(
                    "{} resource is {}",
                    kind,
                    if enabled { "enabled" } else { "disabled" }
                ),
                hint: None,
            });
        }
    }

    // Configuration-file integrity itself — missing file is fine,
    // corrupt file is an error surfaced here so Doctor is the single
    // channel.
    if runtime_path.exists() {
        match RuntimeConfig::read(runtime_path) {
            Ok(_) => checks.push(DoctorCheck {
                id: "config.runtime-file".to_string(),
                subject: runtime_path.display().to_string(),
                kind: None,
                health: Health::Healthy,
                message: "runtime.json parsed successfully".to_string(),
                hint: None,
            }),
            Err(e) => checks.push(DoctorCheck {
                id: "config.runtime-file".to_string(),
                subject: runtime_path.display().to_string(),
                kind: None,
                health: Health::Error,
                message: format!("runtime.json failed to parse: {e}"),
                hint: Some(
                    "delete the corrupt file or fix the JSON; runtime.json uses BTreeMap<String, HashSet<ResourceKind>> entries".to_string(),
                ),
            }),
        }
    }
    if overrides_path.exists() {
        match ProjectPluginOverrides::read(overrides_path) {
            Ok(_) => checks.push(DoctorCheck {
                id: "config.overrides-file".to_string(),
                subject: overrides_path.display().to_string(),
                kind: None,
                health: Health::Healthy,
                message: "plugin-overrides.json parsed successfully".to_string(),
                hint: None,
            }),
            Err(e) => checks.push(DoctorCheck {
                id: "config.overrides-file".to_string(),
                subject: overrides_path.display().to_string(),
                kind: None,
                health: Health::Error,
                message: format!("plugin-overrides.json failed to parse: {e}"),
                hint: Some(
                    "delete the corrupt file or fix the JSON; each value must be \"on\" or \"off\""
                        .to_string(),
                ),
            }),
        }
    }

    let all_clear = checks.iter().all(|c| c.health == Health::Healthy);

    Ok(DoctorReport {
        checks,
        resolved_enabled: resolved,
        all_clear,
    })
}

fn compute_enabled_state(
    name: &str,
    kind: ResourceKind,
    runtime: &RuntimeConfig,
    overrides: &ProjectPluginOverrides,
) -> (bool, EnabledSummary) {
    let project_forced = overrides.forced_state(name, kind).map(|s| s.to_string());
    let user_disabled = runtime.is_disabled(name, kind);
    let enabled = resolve_enabled(name, kind, Some(overrides), Some(runtime));
    (
        enabled,
        EnabledSummary {
            package: name.to_string(),
            kind,
            enabled,
            project_forced,
            user_disabled,
        },
    )
}

fn check_manifest(manager: &PackageManager, name: &str, checks: &mut Vec<DoctorCheck>) {
    let install_dir = manager.get_install_dir(name);
    let Some(dir) = install_dir else {
        checks.push(DoctorCheck {
            id: format!("manifest.install-dir.{name}"),
            subject: name.to_string(),
            kind: None,
            health: Health::Error,
            message: format!("install directory for '{name}' is missing"),
            hint: Some(format!(
                "reinstall with `oxi pkg install` to restore '{name}'"
            )),
        });
        return;
    };
    let manifest_path = dir.join(MANIFEST_NAME);
    if !manifest_path.exists() {
        checks.push(DoctorCheck {
            id: format!("manifest.present.{name}"),
            subject: name.to_string(),
            kind: None,
            health: Health::Error,
            message: format!("no {MANIFEST_NAME} in {dir}", dir = dir.display()),
            hint: Some(format!(
                "the install dir for '{name}' lost its manifest — the lockfile entry is stale; reinstall"
            )),
        });
        return;
    }
    match PackageManager::read_manifest_for_doctor(&manifest_path) {
        Ok(_) => checks.push(DoctorCheck {
            id: format!("manifest.parse.{name}"),
            subject: name.to_string(),
            kind: None,
            health: Health::Healthy,
            message: format!("{MANIFEST_NAME} parsed"),
            hint: None,
        }),
        Err(e) => checks.push(DoctorCheck {
            id: format!("manifest.parse.{name}"),
            subject: name.to_string(),
            kind: None,
            health: Health::Error,
            message: format!("{MANIFEST_NAME} failed to parse: {e}"),
            hint: Some("inspect the file at ".to_string() + &manifest_path.display().to_string()),
        }),
    }
}

fn check_lockfile_integrity(manager: &PackageManager, name: &str, checks: &mut Vec<DoctorCheck>) {
    let Some(install_dir) = manager.get_install_dir(name) else {
        // Already covered by manifest.present / install-dir.
        return;
    };
    let Some(expected) = manager
        .lockfile()
        .packages
        .get(name)
        .and_then(|e| e.integrity.clone())
    else {
        checks.push(DoctorCheck {
            id: format!("integrity.coverage.{name}"),
            subject: name.to_string(),
            kind: None,
            health: Health::Warning,
            message: format!(
                "'{name}' has no integrity hash in the lockfile (very old install) — \
                 coverage gap; reinstall to enable integrity verification"
            ),
            hint: Some("reinstall to populate the lockfile integrity hash".to_string()),
        });
        return;
    };
    match verify_lockfile_integrity(&install_dir, &expected) {
        Ok(()) => checks.push(DoctorCheck {
            id: format!("integrity.match.{name}"),
            subject: name.to_string(),
            kind: None,
            health: Health::Healthy,
            message: "sha256 matches lockfile entry".to_string(),
            hint: None,
        }),
        Err(reason) => checks.push(DoctorCheck {
            id: format!("integrity.match.{name}"),
            subject: name.to_string(),
            kind: None,
            health: Health::Error,
            message: format!("sha256 mismatch: {reason}"),
            hint: Some(format!(
                "'{name}' on-disk contents diverge from the lockfile; \
                 reinstall to refresh the integrity hash (verify the package source!)"
            )),
        }),
    }
}

fn check_resources_present(manager: &PackageManager, name: &str, checks: &mut Vec<DoctorCheck>) {
    let discovered = match manager.discover_resources(name) {
        Ok(d) => d,
        Err(_) => return, // already covered upstream
    };
    if discovered.is_empty() {
        checks.push(DoctorCheck {
            id: format!("resources.empty.{name}"),
            subject: name.to_string(),
            kind: None,
            health: Health::Warning,
            message: format!(
                "'{name}' has no discoverable resources (manifest may be empty and \
                 auto-discovery found nothing)"
            ),
            hint: Some("verify the package manifest and on-disk directory layout".to_string()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::packages::types::PackageManifest;
    use std::collections::BTreeMap;
    use std::fs;

    fn fresh() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let packages_dir = tmp.path().join("packages");
        fs::create_dir_all(&packages_dir).unwrap();
        let runtime = tmp.path().join("runtime.json");
        let overrides = tmp.path().join("plugin-overrides.json");
        // Touch both files as empty so the "file exists" branch fires —
        // we *want* the parse-failure branch to be exercised too,
        // but the happy path here is "missing files => no extra checks".
        let _ = fs::write(&runtime, "{}");
        let _ = fs::write(&overrides, "{}");
        (tmp, packages_dir)
    }

    fn create_pkg(base: &std::path::Path, name: &str, version: &str) -> std::path::PathBuf {
        let pkg_dir = base.join("source-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        let manifest = PackageManifest {
            name: name.to_string(),
            version: version.to_string(),
            extensions: vec!["ext1.so".to_string()],
            skills: vec!["skill-a".to_string()],
            prompts: vec![],
            themes: vec![],
            description: None,
            dependencies: BTreeMap::new(),
        };
        fs::write(
            pkg_dir.join(MANIFEST_NAME),
            toml::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(pkg_dir.join("ext1.so"), b"fake extension").unwrap();
        fs::create_dir_all(pkg_dir.join("skill-a")).unwrap();
        fs::write(pkg_dir.join("skill-a").join("SKILL.md"), b"# Skill A").unwrap();
        pkg_dir
    }

    #[test]
    fn doctor_reports_healthy_for_well_formed_package() {
        let (tmp, packages_dir) = fresh();
        let pkg_dir = create_pkg(tmp.path(), "healthy-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let runtime = tmp.path().join("runtime.json");
        let report = run(
            &mgr,
            &runtime,
            &ProjectPluginOverrides::project_path(tmp.path()),
        )
        .unwrap();
        assert!(
            report.all_clear,
            "expected all_clear, got errors: {report:?}"
        );
        // Every (package, kind) gets a healthy enabled-summary row.
        assert!(
            report
                .resolved_enabled
                .iter()
                .any(|s| s.package == "healthy-pkg"
                    && s.kind == ResourceKind::Extension
                    && s.enabled),
            "missing enabled row: {report:?}"
        );
    }

    #[test]
    fn doctor_flags_missing_install_dir_after_uninstall() {
        let (tmp, packages_dir) = fresh();
        let pkg_dir = create_pkg(tmp.path(), "ghost-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        // Wipe the install dir out from under the manager.
        fs::remove_dir_all(packages_dir.join("ghost-pkg")).unwrap();

        let runtime = tmp.path().join("runtime.json");
        let report = run(
            &mgr,
            &runtime,
            &ProjectPluginOverrides::project_path(tmp.path()),
        )
        .unwrap();
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.health == Health::Error && c.id.contains("manifest.install-dir")),
            "expected install-dir error; got {:?}",
            report.checks
        );
        assert!(!report.all_clear);
    }

    #[test]
    fn doctor_flags_integrity_mismatch_when_manifest_tampered() {
        let (tmp, packages_dir) = fresh();
        let pkg_dir = create_pkg(tmp.path(), "tamper-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        // Tamper with the manifest on disk.
        fs::write(
            packages_dir.join("tamper-pkg").join(MANIFEST_NAME),
            "tampered = true\n",
        )
        .unwrap();

        let runtime = tmp.path().join("runtime.json");
        let report = run(
            &mgr,
            &runtime,
            &ProjectPluginOverrides::project_path(tmp.path()),
        )
        .unwrap();
        // After tampering, load_installed drops it -> check_manifest's
        // install_dir returns Some(dir still) until next reload. Here
        // we're checking directly without reload, so install dir still
        // exists and the parse check will catch it (manifest is unparseable).
        let has_error = report
            .checks
            .iter()
            .any(|c| c.health == Health::Error && c.subject == "tamper-pkg");
        assert!(has_error, "expected an error on tamper-pkg: {report:?}");
        assert!(!report.all_clear);
    }

    #[test]
    fn doctor_flags_corrupt_runtime_file() {
        let (tmp, packages_dir) = fresh();
        let runtime = tmp.path().join("runtime.json");
        fs::write(&runtime, b"{ not json").unwrap();

        let mgr = PackageManager::with_dir(packages_dir).unwrap();
        let report = run(
            &mgr,
            &runtime,
            &ProjectPluginOverrides::project_path(tmp.path()),
        )
        .unwrap();
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.id == "config.runtime-file" && c.health == Health::Error),
            "expected runtime-file error row"
        );
        assert!(!report.all_clear);
    }

    #[test]
    fn doctor_flags_corrupt_overrides_file() {
        let (tmp, packages_dir) = fresh();
        let overrides = ProjectPluginOverrides::project_path(tmp.path());
        fs::create_dir_all(overrides.parent().unwrap()).unwrap();
        fs::write(&overrides, b"{ broken").unwrap();
        let mgr = PackageManager::with_dir(packages_dir).unwrap();
        // Point at a *non-existent* runtime so we don't conflate.
        let missing_runtime = tmp.path().join("nope.json");
        let report = run(&mgr, &missing_runtime, &overrides).unwrap();
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.id == "config.overrides-file" && c.health == Health::Error),
            "expected overrides-file error row"
        );
        assert!(!report.all_clear);
    }

    #[test]
    fn doctor_summary_records_project_force_label() {
        let (tmp, packages_dir) = fresh();
        let pkg_dir = create_pkg(tmp.path(), "forceme", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        // Project overrides force an Off on skill — verify it shows up.
        let overrides_path = ProjectPluginOverrides::project_path(tmp.path());
        fs::create_dir_all(overrides_path.parent().unwrap()).unwrap();
        let mut o = ProjectPluginOverrides::new();
        o.force_off("forceme", ResourceKind::Skill);
        o.write(&overrides_path).unwrap();

        let runtime = tmp.path().join("runtime.json");
        let report = run(&mgr, &runtime, &overrides_path).unwrap();
        let row = report
            .resolved_enabled
            .iter()
            .find(|s| s.package == "forceme" && s.kind == ResourceKind::Skill)
            .expect("summary row missing");
        assert!(!row.enabled, "project force Off should win");
        assert_eq!(row.project_forced.as_deref(), Some("off"));
    }
}
