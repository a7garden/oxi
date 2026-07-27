//! oxi-catalog: model catalog — single source of truth for model data,
//! identity, and provider descriptors.
//!
//! This crate is the Rust port of omp's `@oh-my-pi/pi-catalog` package.
//! It owns:
//! - the [`Api`] enum (the wire-format / protocol dialect a model speaks),
//! - the layered model catalog (SNAP embedded snapshot -> LIVE runtime cache ->
//!   user overrides -> LOCAL server discovery),
//! - provider descriptors (metadata + discovery).
//!
//! `oxi-ai` consumes these types; the dependency direction is one-way
//! (`oxi-ai -> oxi-catalog`), mirroring omp's strict
//! `pi-catalog` / `pi-ai` package separation.

pub mod api;
pub mod catalog;
pub mod product_env;
pub use api::Api;
// Re-export the catalog's public API at the crate root for ergonomic access
// (`oxi_catalog::materialize`, `oxi_catalog::BuiltinProviderEntry`, ...).
// The `catalog` module path (`oxi_catalog::catalog::...`) also remains valid so
// `oxi-ai` can re-export it for backward compatibility.
pub use catalog::*;
