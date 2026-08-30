//! Unified Oxi home layout (`oxi_home()` / `oxicode_home()` / `legacy_home_dir()`).
//!
//! Implementation lives in [`oxicode_catalog::oxi_home`] (the lower-level
//! crate, so `product_env` can share the same pure core); re-exported here
//! so `oxicode_ai::oxi_home` is the designated public path and embedders of
//! `oxicode-ai` need no direct `oxicode-catalog` dependency.
//!
//! See the catalog module docs for the full layout contract: canonical-first
//! resolution, read-only `~/.oxicode` legacy fallback, and the
//! `oxicode migrate home` journal.

pub use oxicode_catalog::oxi_home::{
    legacy_home_dir, migration_journal_path, oxi_home, oxicode_home, read_path,
    resolve_legacy_home, resolve_oxi_home, resolve_oxicode_home, resolve_read_path,
};
