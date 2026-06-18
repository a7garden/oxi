//! Built-in provider catalog loader.
//!
//! All provider and model data is dynamically sourced from [models.dev](https://models.dev).
//! See the [`mod@materialize`] module for the conversion pipeline.
//!
//! # Layered design
//!
//! ```text
//! 1. SNAP — embedded models.dev snapshot (compile-time `include_bytes!`)
//! 2. LIVE — runtime cache (~/.oxi/cache/) with ETag conditional GET
//! 3. Layer 2 — user override (~/.oxi/catalog/overrides.toml)
//! 4. LOCAL — runtime /v1/models discovery for local servers
//! ```
//!
//! See `data/catalog/README.md` for the full architecture and attribution.

pub mod materialize;
pub mod model;
pub mod models_dev;
pub mod override_;
pub mod provider;
pub mod runtime;

pub use materialize::{ProductMeta, materialize, materialize_providers};
pub use model::BuiltinModelEntry;
pub use models_dev::{MdCatalog, get as models_dev_get, init_models_dev, protocol_for, refresh};
pub use override_::{
    OverrideFile, apply_model_overrides, apply_provider_overrides, find_override_files,
    load_overrides,
};
pub use provider::{
    AuthMethod, BuiltinProviderEntry, builtin_model_count, builtin_providers_count,
    load_builtin_models, load_builtin_providers,
};
pub use runtime::{discover_all, discover_all_authenticated, discover_all_local, discover_models};
