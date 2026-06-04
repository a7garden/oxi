//! Composition root for oxi-cli.
//!
//! Wires concrete file-based port implementations (from `oxi-fs`) to the
//! `Oxi` engine. Future run modes (TUI / print / RPC) build on top of
//! the `Oxi` produced here.
//!
//! # Migration note
//!
//! The legacy `App` struct in `lib.rs` is the **single-user interactive**
//! composition (TUI-driven, in-process). This module is the
//! **port-based** composition: a `Oxi` engine with persistence, auth,
//! config, and skills wired via `oxi_sdk::OxiBuilder::with_port_*`.
//!
//! Both paths are expected to coexist during the migration. New run modes
//! should consume `build_oxi(...)` from this module; the legacy `App`
//! path remains for the interactive TUI until it is fully migrated.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use oxi_fs::{FileAuthProvider, FileConfigStore, FileSkillLoader, FileStateStore};
use oxi_sdk::Oxi;

/// Resolved paths under the oxi home directory.
#[derive(Debug, Clone)]
pub struct OxiPaths {
    /// Root directory (`$OXI_HOME` or `$HOME/.oxi`).
    pub home: PathBuf,
    /// `auth.json` location.
    pub auth: PathBuf,
    /// `settings.toml` location.
    pub config: PathBuf,
    /// Sessions directory.
    pub sessions: PathBuf,
    /// Skills root.
    pub skills: PathBuf,
}

impl OxiPaths {
    /// Resolve from the conventional home directory.
    pub fn from_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            auth: home.join("auth.json"),
            config: home.join("settings.toml"),
            sessions: home.join("sessions"),
            skills: home.join("skills"),
            home,
        }
    }

    /// Default — uses `$OXI_HOME` or `$HOME/.oxi`.
    pub fn default_paths() -> Result<Self> {
        oxi_fs::home_dir()
            .map(Self::from_home)
            .context("could not resolve oxi home directory")
    }
}

/// Build an `Oxi` engine wired with file-based port implementations.
///
/// This is the **composition root** for oxi-cli. It is intentionally
/// side-effect-light: it does not touch the network or start any task.
/// Run modes (TUI, print, RPC) take the returned `Oxi` and run.
pub fn build_oxi(paths: &OxiPaths) -> Result<Oxi> {
    ensure_parent(&paths.auth)?;
    ensure_parent(&paths.config)?;
    ensure_parent(&paths.sessions)?;

    let oxi = oxi_sdk::OxiBuilder::new()
        .with_builtins()
        .with_state(Arc::new(FileStateStore::new(&paths.sessions)))
        .with_auth(Arc::new(FileAuthProvider::new(&paths.auth)))
        .with_config(Arc::new(FileConfigStore::new(&paths.config)))
        .with_skills(Arc::new(FileSkillLoader::single(&paths.skills)))
        .build();

    Ok(oxi)
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_consistent() {
        let p = OxiPaths::from_home("/tmp/oxi-test");
        assert!(p.auth.starts_with("/tmp/oxi-test"));
        assert!(p.config.starts_with("/tmp/oxi-test"));
        assert!(p.sessions.starts_with("/tmp/oxi-test"));
        assert!(p.skills.starts_with("/tmp/oxi-test"));
    }

    #[test]
    fn build_oxi_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = OxiPaths::from_home(tmp.path());
        let oxi = build_oxi(&paths).unwrap();
        // State is wired (even if we don't call it).
        let _ = oxi.ports().state;
    }
}
