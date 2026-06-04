//! `oxi-fs` — file-based port implementations for `oxi-sdk`.
//!
//! This crate is an **adapter**. It implements the port traits defined in
//! `oxi_sdk::ports` against the local file system using the conventional
//! home-directory layout (`~/.oxi/`).
//!
//! # Layout
//!
//! ```text
//! ~/.oxi/
//! ├── auth.json        — AuthProvider (FileAuthProvider)
//! ├── settings.toml    — ConfigStore   (FileConfigStore)
//! ├── sessions/        — StateStore    (FileStateStore, JSONL append-only)
//! │   ├── <uuid>.jsonl
//! │   └── ...
//! ├── skills/          — SkillLoader   (FileSkillLoader)
//! │   ├── <name>/SKILL.md
//! │   └── ...
//! └── cache/           — misc ephemeral state
//! ```
//!
//! # When to use
//!
//! Use `oxi-fs` when your product is a single-user desktop or CLI tool that
//! needs persistence in the user's home directory. For multi-tenant, cloud,
//! or in-memory scenarios, implement the same `oxi_sdk::ports` traits
//! against your own backend.
//!
//! # Example
//!
//! ```no_run
//! use oxi_sdk::OxiBuilder;
//! use oxi_fs::{FileStateStore, FileAuthProvider, FileConfigStore, FileSkillLoader};
//! use std::sync::Arc;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let home = oxi_fs::home_dir()?;
//! let oxi = OxiBuilder::new()
//!     .with_builtins()
//!     .with_state(Arc::new(FileStateStore::new(home.join("sessions"))))
//!     .with_auth(Arc::new(FileAuthProvider::new(home.join("auth.json"))))
//!     .with_config(Arc::new(FileConfigStore::new(home.join("settings.toml"))))
//!     .with_skills(Arc::new(FileSkillLoader::single(home.join("skills"))))
//!     .build();
//! # let _ = oxi;
//! # Ok(()) }
//! ```

#![warn(missing_docs)]
#![warn(clippy::unwrap_used)]

pub mod access;
pub mod auth;
pub mod config;
pub mod path;
pub mod persona;
pub mod session;
pub mod skill;

pub use access::SimpleAccessGate;
pub use auth::FileAuthProvider;
pub use config::FileConfigStore;
pub use persona::FilePersonaProvider;
pub use session::FileStateStore;
pub use skill::FileSkillLoader;

use std::path::PathBuf;

/// Return the conventional `oxi` home directory (`$OXI_HOME` or
/// `$HOME/.oxi`).
///
/// Returns an error if neither environment variable is set.
pub fn home_dir() -> std::io::Result<PathBuf> {
    path::home_dir()
}
