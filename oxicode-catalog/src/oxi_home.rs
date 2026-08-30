//! Unified Oxi home layout.
//!
//! Every Oxi product shares one user-level root, the *Oxi home*:
//!
//! ```text
//! oxi_home()     = $OXI_HOME              if set and non-empty
//!                = $HOME/.oxi             otherwise
//!
//! oxicode_home() = $OXICODE_HOME          if set and non-empty
//!                = <oxi_home()>/oxicode   otherwise
//!
//! legacy_home_dir() = $HOME/.oxicode      when it exists on disk
//! ```
//!
//! [`oxicode_home`] is the **canonical** home: every write lands there.
//! [`legacy_home_dir`] is a **read-only** compatibility shim for installs
//! created before the unified layout (`~/.oxicode`): readers fall back to it
//! only when the canonical location lacks the item, and nothing ever writes
//! to — or deletes from — it. Use `oxicode migrate home` to move legacy data
//! forward (copy-only, journaled, resumable).
//!
//! An explicit `$OXICODE_HOME` opts out of the unified layout entirely:
//! [`legacy_home_dir`] returns `None`, so an explicit override never silently
//! merges with a legacy path.
//!
//! Project-local `.oxicode/` directories (anchored at the cwd) are a separate
//! discovery namespace and are unaffected by this module.
//!
//! # Why env-var, not a typed global
//!
//! Same rationale as [`crate::product_env`]: a product identity is
//! process-global and an environment variable is readable inside any lazy
//! initializer without introducing a second, mutable, init-ordered global.

use std::path::{Path, PathBuf};

// ── Pure core ──────────────────────────────────────────────────────────────
//
// No environment access: every function takes injected inputs so tests can
// run in parallel without racing the process-global environment.

/// Pure [`oxi_home()`]: `$OXI_HOME` when set and non-empty, else
/// `$user_home/.oxi`.
///
/// `None` when neither input is available (no `OXI_HOME`, no home directory).
pub fn resolve_oxi_home(oxi_home: Option<&str>, user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = oxi_home.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    user_home.map(|h| h.join(".oxi"))
}

/// Pure [`oxicode_home()`]: `$OXICODE_HOME` when set and non-empty, else
/// `<oxi_home>/oxicode`.
///
/// `oxi_home` is the already-resolved Oxi home (see [`resolve_oxi_home`]).
/// `None` when neither input is available.
pub fn resolve_oxicode_home(
    oxicode_home: Option<&str>,
    oxi_home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = oxicode_home.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    oxi_home.map(|h| h.join("oxicode"))
}

/// Pure [`legacy_home_dir`] path candidate: `$user_home/.oxicode`.
///
/// Existence and explicit-override checks are the env-wired wrapper's job.
pub fn resolve_legacy_home(user_home: Option<&Path>) -> Option<PathBuf> {
    user_home.map(|h| h.join(".oxicode"))
}

/// Pure [`read_path`]: canonical item when it exists, else the legacy
/// counterpart when that exists, else the canonical path (the write target).
pub fn resolve_read_path(canonical_home: &Path, legacy_home: Option<&Path>, rel: &Path) -> PathBuf {
    let canonical = canonical_home.join(rel);
    if canonical.exists() {
        return canonical;
    }
    if let Some(legacy) = legacy_home {
        let legacy_path = legacy.join(rel);
        if legacy_path.exists() {
            return legacy_path;
        }
    }
    canonical
}

// ── Env-wired API ──────────────────────────────────────────────────────────

/// The unified Oxi home: `$OXI_HOME` if set (and non-empty), else
/// `$HOME/.oxi`.
pub fn oxi_home() -> Option<PathBuf> {
    resolve_oxi_home(
        std::env::var("OXI_HOME").ok().as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// The canonical oxicode home: `$OXICODE_HOME` if set (and non-empty), else
/// `<oxi_home()>/oxicode`.
///
/// All oxicode-owned state (auth, settings, sessions, skills, extensions,
/// packages, caches) lives here after migration. Writes always target this
/// directory.
pub fn oxicode_home() -> Option<PathBuf> {
    resolve_oxicode_home(
        std::env::var("OXICODE_HOME").ok().as_deref(),
        oxi_home().as_deref(),
    )
}

/// The legacy oxicode home (`$HOME/.oxicode`) — **read-only**.
///
/// Returns `Some` only when the directory exists on disk AND no explicit
/// `$OXICODE_HOME` override is set: an explicit override never silently
/// merges with a legacy path. Readers may fall back to this directory when
/// the canonical home lacks an item; writes must never target it.
pub fn legacy_home_dir() -> Option<PathBuf> {
    if std::env::var_os("OXICODE_HOME").is_some_and(|v| !v.to_string_lossy().trim().is_empty()) {
        return None;
    }
    let p = resolve_legacy_home(dirs::home_dir().as_deref())?;
    p.is_dir().then_some(p)
}

/// Canonical-first, legacy-read-only fallback for a single item under the
/// oxicode home.
///
/// `rel` is home-relative (e.g. `"auth.json"`, `"extensions/registry.json"`,
/// `"skills"` — files and directories both work). Returns:
///
/// 1. `<oxicode_home>/rel` when it exists,
/// 2. else `<legacy_home>/rel` when that exists,
/// 3. else `<oxicode_home>/rel` (the write target).
///
/// `None` only when no home can be resolved at all.
pub fn read_path(rel: impl AsRef<Path>) -> Option<PathBuf> {
    Some(resolve_read_path(
        &oxicode_home()?,
        legacy_home_dir().as_deref(),
        rel.as_ref(),
    ))
}

/// Path of the home-layout migration journal:
/// `<oxi_home()>/oxicode.migration-journal.json`.
///
/// The journal lives beside the owned `oxicode/` subtree (not inside it) so
/// it survives a canonical-home reset.
pub fn migration_journal_path() -> Option<PathBuf> {
    oxi_home().map(|h| h.join("oxicode.migration-journal.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn home(p: &str) -> Option<PathBuf> {
        Some(PathBuf::from(p))
    }

    // ── resolve_oxi_home ───────────────────────────────────────────────

    #[test]
    fn oxi_home_env_wins() {
        let got = resolve_oxi_home(Some("/custom/oxi"), Some(Path::new("/home/u")));
        assert_eq!(got, home("/custom/oxi"));
    }

    #[test]
    fn oxi_home_empty_env_falls_through() {
        let got = resolve_oxi_home(Some(""), Some(Path::new("/home/u")));
        assert_eq!(got, home("/home/u/.oxi"));
    }

    #[test]
    fn oxi_home_whitespace_env_falls_through() {
        let got = resolve_oxi_home(Some("   "), Some(Path::new("/home/u")));
        assert_eq!(got, home("/home/u/.oxi"));
    }

    #[test]
    fn oxi_home_defaults_to_user_home() {
        let got = resolve_oxi_home(None, Some(Path::new("/home/u")));
        assert_eq!(got, home("/home/u/.oxi"));
    }

    #[test]
    fn oxi_home_none_when_both_absent() {
        assert_eq!(resolve_oxi_home(None, None), None);
    }

    // ── resolve_oxicode_home ───────────────────────────────────────────

    #[test]
    fn oxicode_home_env_wins() {
        let got = resolve_oxicode_home(Some("/custom/ox"), Some(Path::new("/home/u/.oxi")));
        assert_eq!(got, home("/custom/ox"));
    }

    #[test]
    fn oxicode_home_empty_env_falls_through() {
        let got = resolve_oxicode_home(Some(""), Some(Path::new("/home/u/.oxi")));
        assert_eq!(got, home("/home/u/.oxi/oxicode"));
    }

    #[test]
    fn oxicode_home_defaults_to_oxi_subdir() {
        let got = resolve_oxicode_home(None, Some(Path::new("/home/u/.oxi")));
        assert_eq!(got, home("/home/u/.oxi/oxicode"));
    }

    #[test]
    fn oxicode_home_none_without_oxi_home() {
        assert_eq!(resolve_oxicode_home(None, None), None);
    }

    // ── resolve_legacy_home ────────────────────────────────────────────

    #[test]
    fn legacy_candidate_is_user_home_dot_oxicode() {
        let got = resolve_legacy_home(Some(Path::new("/home/u")));
        assert_eq!(got, home("/home/u/.oxicode"));
    }

    #[test]
    fn legacy_candidate_none_without_user_home() {
        assert_eq!(resolve_legacy_home(None), None);
    }

    // ── resolve_read_path ──────────────────────────────────────────────

    #[test]
    fn read_path_prefers_canonical_when_it_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().join("canonical");
        let legacy = tmp.path().join("legacy");
        std::fs::create_dir_all(canonical.join("skills")).unwrap();
        std::fs::create_dir_all(legacy.join("skills")).unwrap();
        std::fs::write(legacy.join("skills").join("old.md"), "legacy").unwrap();
        let got = resolve_read_path(&canonical, Some(&legacy), Path::new("skills"));
        assert_eq!(got, canonical.join("skills"));
    }

    #[test]
    fn read_path_falls_back_to_legacy_when_canonical_lacks_item() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().join("canonical");
        let legacy = tmp.path().join("legacy");
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::create_dir_all(legacy.join("skills")).unwrap();

        let got = resolve_read_path(&canonical, Some(&legacy), Path::new("skills"));
        assert_eq!(got, legacy.join("skills"));
    }

    #[test]
    fn read_path_defaults_to_canonical_when_neither_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().join("canonical");
        std::fs::create_dir_all(&canonical).unwrap();

        let got = resolve_read_path(&canonical, None, Path::new("auth.json"));
        assert_eq!(got, canonical.join("auth.json"));
    }

    #[test]
    fn read_path_ignores_legacy_without_override_gap() {
        // Both exist: canonical always wins (legacy is never merged in).
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().join("canonical");
        let legacy = tmp.path().join("legacy");
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(canonical.join("auth.json"), "{}").unwrap();
        std::fs::write(legacy.join("auth.json"), "{}").unwrap();

        let got = resolve_read_path(&canonical, Some(&legacy), Path::new("auth.json"));
        assert_eq!(got, canonical.join("auth.json"));
    }

    // ── smoke: env-wired functions resolve in any normal environment ───

    /// `oxi_home()` resolves without touching the environment, so this is
    /// safe under parallel test runners.
    #[test]
    fn oxi_home_resolves_in_ci() {
        assert!(oxi_home().is_some(), "OXI_HOME or HOME is always set in CI");
    }

    /// `oxicode_home()` is always `<oxi_home>/oxicode` unless
    /// `OXICODE_HOME` is set.
    #[test]
    fn oxicode_home_nests_under_oxi_home() {
        let oxi = oxi_home().expect("oxi home resolves in CI");
        if std::env::var_os("OXICODE_HOME").is_none() {
            assert_eq!(oxicode_home(), Some(oxi.join("oxicode")));
        }
    }
}
