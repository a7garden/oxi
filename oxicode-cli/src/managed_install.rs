//! Managed install layout — `~/.oxi/oxicode/{bin,versions}`.
//!
//! Mirrors the ecosystem binary standard that every Oxi app uses:
//! `~/.oxi/<app>/bin/<app>` is a launcher symlink to
//! `../versions/<v>/<app>`, with newer versions kept on disk and old
//! ones pruned. Oxicode's *fetch* channel stays `cargo install`
//! (crates.io + binstall handle signing, prebuilt binaries, and the
//! platform matrix); [`handle_update`](crate::cli::commands::misc::handle_update)
//! calls into this module after a successful cargo install so the
//! freshly-installed binary is adopted into the managed layout, the
//! launcher is flipped to it, and the cargo bin copy is repointed at
//! the launcher. The result: a single canonical binary under
//! `~/.oxi/oxicode/versions/` and one symlink at every PATH entry.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// The launcher symlink PATH entries point at: `<home>/bin/oxicode`.
pub fn launcher_path(home: &Path) -> PathBuf {
    home.join("bin").join("oxicode")
}

/// Per-release binaries: `<home>/versions/<v>/oxicode`.
pub fn versions_dir(home: &Path) -> PathBuf {
    home.join("versions")
}

/// Strict SemVer triple for version-dir names — no leading `v`, no
/// pre-release/build metadata (mirrors oxios's `managed_install`).
pub fn parse_version_dir(name: &str) -> Option<(u64, u64, u64)> {
    let mut parts = name.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// Pull a bare `MAJOR.MINOR.PATCH` out of a `--version` banner
/// (`oxicode 0.79.0`, `oxicode 0.79.0 (commit…)`).
pub fn version_from_version_output(text: &str) -> Option<String> {
    let raw = text.trim().trim_start_matches('v');
    let token = raw.split_whitespace().find(|s| parse_version_dir(s).is_some())?;
    Some(token.to_string())
}

/// Move `binary` into `<home>/versions/<version>/oxicode` (atomic rename
/// when on the same volume, copy+remove when crossing volumes), flip
/// the launcher at `<home>/bin/oxicode`, optionally repoint a cargo/PATH
/// copy at the launcher, and prune older version dirs (keep 2). Returns
/// the launcher path.
pub fn adopt_binary(
    home: &Path,
    binary: &Path,
    version: &str,
    relink: Option<&Path>,
) -> Result<PathBuf> {
    if parse_version_dir(version).is_none() {
        anyhow::bail!("unusable version {version:?} — expected MAJOR.MINOR.PATCH");
    }
    let versions = versions_dir(home);
    std::fs::create_dir_all(&versions)
        .with_context(|| format!("create versions dir {}", versions.display()))?;
    let launcher = launcher_path(home);
    let version_dir = versions.join(version);
    std::fs::create_dir_all(&version_dir)
        .with_context(|| format!("create version dir {}", version_dir.display()))?;
    let target = version_dir.join("oxicode");

    // Same-volume rename is atomic; EXDEV (cross-volume) falls back to
    // copy + remove so a split home/<cargo> never strands the binary.
    if let Err(e) = std::fs::rename(binary, &target) {
        if e.raw_os_error() == Some(18 /* EXDEV */) {
            std::fs::copy(binary, &target)
                .with_context(|| format!("copy {} → {}", binary.display(), target.display()))?;
            let _ = std::fs::remove_file(binary);
        } else {
            return Err(e).with_context(|| {
                format!("move {} → {}", binary.display(), target.display())
            });
        }
    }
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&target)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&target, perms)
            .with_context(|| format!("chmod 0755 {}", target.display()))?;
    }

    flip_launcher(&launcher, version)?;

    if let Some(rel) = relink
        && rel != target
        && rel != binary
    {
        repoint(rel, &launcher)?;
    }

    prune_versions(home, version, 2)?;
    Ok(launcher)
}

pub(crate) fn flip_launcher(launcher: &Path, version: &str) -> Result<()> {
    let bin_dir = launcher
        .parent()
        .context("launcher path has no parent directory")?;
    std::fs::create_dir_all(bin_dir)
        .with_context(|| format!("create bin dir {}", bin_dir.display()))?;
    let target = PathBuf::from("../versions").join(version).join("oxicode");
    let tmp_link = bin_dir.join(".oxicode.link.tmp");
    let _ = std::fs::remove_file(&tmp_link);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target, &tmp_link).with_context(|| {
            format!("symlink {} → {}", tmp_link.display(), target.display())
        })?;
    }
    #[cfg(not(unix))]
    {
        let source = bin_dir
            .parent()
            .context("launcher is not two levels below the app root")?
            .join("versions")
            .join(version)
            .join("oxicode");
        std::fs::copy(&source, &tmp_link)
            .with_context(|| format!("copy {} → {}", source.display(), tmp_link.display()))?;
    }
    std::fs::rename(&tmp_link, launcher).with_context(|| {
        format!(
            "atomic launcher flip {} → {}",
            tmp_link.display(),
            launcher.display()
        )
    })?;
    Ok(())
}

pub(crate) fn repoint(path: &Path, launcher: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if let Ok(target) = std::fs::read_link(path) {
        // Already pointing at the launcher (by basename — `~/.cargo/bin/oxicode`
        // lives in a different directory than `<home>/bin/oxicode`, so we
        // match on file name) or at the same target the launcher itself
        if launcher.file_name().is_some_and(|name| std::path::Path::new(name) == target.as_path())
        {
            return Ok(());
        }
    }
    let parent = path.parent().context("relink path has no parent")?;
    let tmp = parent.join(".oxicode.link.tmp");
    let _ = std::fs::remove_file(&tmp);
    #[cfg(unix)]
    {
        let launcher_name = launcher
            .file_name()
            .context("launcher has no file name")?;
        std::os::unix::fs::symlink(launcher_name, &tmp).with_context(|| {
            format!("symlink {} → {}", tmp.display(), launcher_name.display())
        })?;
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(launcher, &tmp)
            .with_context(|| format!("copy {} → {}", launcher.display(), tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("atomic relink {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

pub(crate) fn prune_versions(
    home: &Path,
    current: &str,
    keep: usize,
) -> Result<Vec<String>> {
    let versions = versions_dir(home);
    let mut installed: Vec<((u64, u64, u64), String)> = Vec::new();
    if !versions.exists() {
        return Ok(vec![]);
    }
    for entry in std::fs::read_dir(&versions)
        .with_context(|| format!("read_dir {}", versions.display()))?
    {
        let entry =
            entry.with_context(|| format!("read_dir entry in {}", versions.display()))?;
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == current {
            continue;
        }
        if let Some(v) = parse_version_dir(&name) {
            installed.push((v, name));
        }
    }
    installed.sort();
    let mut removed = Vec::new();
    while installed.len() >= keep {
        let (_, name) = installed.remove(0);
        std::fs::remove_dir_all(versions.join(&name))
            .with_context(|| format!("prune version dir {}", name))?;
        removed.push(name);
    }
    Ok(removed)
}

/// Path of the cargo-installed `oxicode` for the current user
/// (`$CARGO_HOME/bin/oxicode` or the `$HOME/.cargo/bin/oxicode`
/// default). May not exist; the caller decides.
pub fn cargo_oxicode_bin() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let cargo_root = if let Some(d) = std::env::var_os("CARGO_HOME") {
        if d.is_empty() {
            PathBuf::from(home).join(".cargo")
        } else {
            PathBuf::from(d)
        }
    } else {
        PathBuf::from(home).join(".cargo")
    };
    Some(cargo_root.join("bin").join("oxicode"))
}

/// Run `<binary> --version` and parse a bare version out.
pub fn version_of(binary: &Path) -> Option<String> {
    let out = std::process::Command::new(binary)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    version_from_version_output(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn write_exe(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
        p
    }

    #[test]
    fn parse_version_dir_round_trip() {
        assert_eq!(parse_version_dir("0.79.0"), Some((0, 79, 0)));
        assert_eq!(parse_version_dir("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version_dir("v0.1.0"), None);
        assert_eq!(parse_version_dir("0.1.0-beta"), None);
        assert_eq!(parse_version_dir(""), None);
    }

    #[test]
    fn version_from_version_output_handles_clap_banner() {
        assert_eq!(
            version_from_version_output("oxicode 0.79.0\n"),
            Some("0.79.0".to_string())
        );
        assert_eq!(
            version_from_version_output("oxicode 0.79.0 (abc123)"),
            Some("0.79.0".to_string())
        );
        assert_eq!(version_from_version_output("garbage"), None);
    }

    #[test]
    fn adopt_binary_builds_managed_layout_and_relinks() {
        let home = tempfile::tempdir().unwrap();
        let binaries = tempfile::tempdir().unwrap();
        let bin = write_exe(binaries.path(), "oxicode", b"#!/bin/sh\n");
        let relink = binaries.path().join("cargo-oxicode");
        std::fs::write(&relink, b"placeholder").unwrap(); // adopt must replace

        let launcher = adopt_binary(home.path(), &bin, "0.79.0", Some(&relink)).unwrap();

        assert!(home.path().join("versions/0.79.0/oxicode").is_file());
        assert!(launcher.is_file());
        assert_eq!(
            std::fs::read_link(&launcher).unwrap(),
            std::path::Path::new("../versions/0.79.0/oxicode")
        );
        let relink_target = std::fs::read_link(&relink).unwrap();
        assert_eq!(
            relink_target,
            std::path::Path::new("oxicode"),
            "relink should target the launcher (same-dir basename)"
        );
        // The moved binary lives at the version dir; the relink is a symlink.
        assert!(!bin.exists(), "the original binary path was moved");
    }

    #[test]
    fn adopt_prunes_older_versions_to_two() {
        let home = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();

        for v in ["0.78.0", "0.79.0", "1.0.0"] {
            let bin = write_exe(staging.path(), "oxicode", b"#!/bin/sh\n");
            adopt_binary(home.path(), &bin, v, None).unwrap();
        }
        // Three installs ended with the launcher on 1.0.0. Keep 2 →
        // 0.78.0 should be pruned, 0.79.0 + 1.0.0 retained.
        assert!(!home.path().join("versions/0.78.0").exists());
        assert!(home.path().join("versions/0.79.0").exists());
        assert!(home.path().join("versions/1.0.0").exists());
    }

    #[test]
    fn repoint_is_idempotent_on_converged_links() {
        let home = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let bin = write_exe(staging.path(), "oxicode", b"#!/bin/sh\n");
        // The relink path lives in a separate dir from the cargo bin
        // (we simulate ~/.cargo/bin/oxicode).
        let relink = staging.path().join("cargo-bin/oxicode");
        std::fs::create_dir_all(relink.parent().unwrap()).unwrap();
        std::fs::write(&relink, b"old").unwrap();
        adopt_binary(home.path(), &bin, "0.1.0", Some(&relink)).unwrap();
        // Second call must not error or rewrite the link.
        let first = std::fs::read_link(&relink).unwrap();
        repoint(&relink, &launcher_path(home.path())).unwrap();
        let second = std::fs::read_link(&relink).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn rejects_invalid_version_strings() {
        let home = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let bin = write_exe(staging.path(), "oxicode", b"#!/bin/sh\n");
        assert!(adopt_binary(home.path(), &bin, "v0.1.0", None).is_err());
        assert!(adopt_binary(home.path(), &bin, "not-a-version", None).is_err());
    }
}
