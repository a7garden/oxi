//! File-based port implementations.
//!
//! Every adapter writes to a configurable root directory using a
//! conventional layout (JSON for state, TOML for config, SKILL.md for
//! skills, …). Atomic writes (temp + rename) and per-id locks are used
//! where concurrent access matters.

use std::path::PathBuf;

/// Resolve the conventional `oxicode` home directory (`$OXICODE_HOME` or
/// `$HOME/.oxicode`).
pub fn home_dir() -> std::io::Result<PathBuf> {
    path::home_dir()
}

pub mod access;
pub mod auth;
pub mod capability;
pub mod catalog;
pub mod config;
pub mod hook_runner;
pub mod path;
pub mod persona;
pub mod session;
pub mod skill;
pub use access::SimpleAccessGate;
pub use auth::FileAuthProvider;
pub use capability::TomlCapabilityResolver;
pub use catalog::{CatalogConfig, FileModelCatalog};
pub use config::FileConfigStore;
pub use hook_runner::CommandHookRunner;
pub use persona::FilePersonaProvider;
pub use session::FileStateStore;
pub use skill::FileSkillLoader;
