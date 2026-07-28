//! Package system for oxi CLI
//!
//! Packages bundle extensions, skills, prompts, and themes for sharing.
//! Supports local directories, npm packages, git repositories, GitHub
//! shorthand, and URL-based archives.
//!
//! ## Package sources
//!
//! - **Local path**: a directory with `oxi-package.toml` or auto-discoverable resources
//! - **npm**: `npm:<package>[@<version>]` — resolved from the npm registry
//! - **git**: `https://github.com/org/repo.git[@ref]`, `git://…`, `git+ssh://…`
//! - **GitHub shorthand**: `github:org/repo[@ref]`
//! - **URL**: direct `.tar.gz` / `.zip` archive
//!
//! ## Package manifest
//!
//! A package is a directory containing an `oxi-package.toml` file:
//!
//! ```toml
//! name = "@foo/oxi-tools"
//! version = "1.0.0"
//! extensions = ["ext/index.ts"]
//! skills = ["skills/code-review/SKILL.md"]
//! prompts = ["prompts/review.md"]
//! themes = ["themes/dark-pro.json"]
//! ```
//!
//! ## Resource discovery
//!
//! When a package lacks explicit resource lists, resources are discovered
//! automatically by scanning the package directory:
//! - **Extensions**: `.so`, `.dylib`, `.dll` files, or `index.ts`/`index.js` entries
//! - **Skills**: Directories containing `SKILL.md`
//! - **Prompts**: `.md` files in `prompts/` subdirectory
//! - **Themes**: `.json` files in `themes/` subdirectory
//!
//! ## Lockfile
//!
//! An `oxi-lock.json` file records exact versions/refs for reproducibility.
//!
//! ## Module layout
//!
//! The package subsystem is split across several submodules for
//! navigability. Public re-exports at the bottom preserve the original
//! `crate::storage::packages::*` API.
//!
//! - `types` — core data types (manifest, kind, scope, progress events)
//! - `source` — source-spec parsing (`ParsedSource`, npm/git/url helpers)
//! - `npm` — npm registry client (`NpmPackageInfo`)
//! - `git_ops` — git command wrappers (`git_clone`, `git_update`, …)
//! - `lockfile` — lockfile types + SHA-256 integrity helpers
//! - `discovery` — auto-discovery of resources in a package directory
//! - `fs` — generic filesystem helpers (`copy_dir_recursive`, …)
//! - `manager` — `PackageManager` facade + tests

mod discovery;
mod fs;
mod git_ops;
mod lockfile;
mod manager;
mod npm;
mod source;
mod types;

// ── Constants ─────────────────────────────────────────────────────────
//
// These three names are referenced by every submodule (manifest paths,
// the lockfile), so they live here in the parent module and are picked
// up by children via `super::MANIFEST_NAME`, etc.

pub(super) const LOCKFILE_NAME: &str = "oxi-lock.json";
pub(super) const MANIFEST_NAME: &str = "oxi-package.toml";
pub(super) const NPM_MANIFEST_NAME: &str = "package.json";

// Public re-exports preserve the original `crate::storage::packages::*`
// surface so existing callers (`use oxi::storage::packages::*`) keep
// working without churn.
pub use lockfile::{LockEntry, Lockfile, ResourceCounts};
pub use manager::PackageManager;
pub use npm::{NpmPackageInfo, get_latest_npm_version};
pub use source::ParsedSource;
pub use types::{
    ConfiguredPackage, DiscoveredResource, PackageManifest, PackageUpdateInfo, PathMetadata,
    ProgressAction, ProgressCallback, ProgressEvent, ProgressEventType, ResolvedPaths,
    ResolvedResource, ResourceKind, ResourceOrigin, SourceScope,
};
