//! Built-in provider catalog loader.
//!
//! This module loads provider metadata from a static TOML file at compile time.
//! The TOML file (`data/catalog/providers.toml`) is the single source of truth
//! for built-in provider definitions.
//!
//! # Why TOML?
//!
//! - **Non-developers can contribute** — model additions are PR-friendly diffs
//! - **Schema validation at parse time** via `serde`
//! - **Zero runtime IO** — file is `include_str!`-ed at build time
//! - **Zero allocation on hot path** — output is `&'static [BuiltinProviderEntry]`
//!
//! # Layered design
//!
//! This is **Layer 1 (built-in)** of the 3-tier catalog:
//! 1. Built-in TOML (this module) — bundled, validated at compile time
//! 2. User overrides (`override_` module) — `~/.oxi/catalog/overrides.toml`
//! 3. Runtime discovery (`runtime` module) — ollama/lmstudio/cloud `/v1/models` fetch
//!
//! See `register_builtins.rs` for the consumer side.

pub mod model;
pub mod override_;
pub mod provider;
pub mod runtime;

pub use model::{builtin_model_count, load_builtin_models, BuiltinModelEntry};
pub use override_::{
    apply_model_overrides, apply_provider_overrides, find_override_files, load_overrides,
    OverrideFile,
};
pub use provider::{
    builtin_providers_count, load_builtin_providers, AuthMethod, BuiltinProviderEntry,
};
pub use runtime::{discover_all, discover_all_authenticated, discover_all_local, discover_models};

use std::sync::OnceLock;

/// Catalog root: lazy-initialized, thread-safe.
///
/// The TOML is `include_str!`-ed at compile time, so the first call to
/// [`catalog_root`] parses the entire file. Parsing is cheap (~10ms for 70
/// providers + 900+ models on a modern machine) and the result is cached.
static CATALOG: OnceLock<CatalogRoot> = OnceLock::new();

/// Top-level catalog structure.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CatalogRoot {
    /// All built-in providers.
    #[serde(default)]
    pub provider: Vec<BuiltinProviderEntry>,
    /// All built-in models, indexed by provider id.
    ///
    /// Keyed by provider id (e.g., "anthropic") so the loader can
    /// avoid scanning every model for provider lookups.
    #[serde(default)]
    pub models: std::collections::BTreeMap<String, Vec<BuiltinModelEntry>>,
}

impl CatalogRoot {
    /// Get the global catalog (lazy-initialized).
    pub fn get() -> &'static Self {
        CATALOG.get_or_init(|| {
            // Include the providers catalog at compile time.
            // The models subdirectory is included via concat! trick — each file
            // is included as a &str, then parsed and merged at startup.
            let providers_toml = include_str!("../../data/catalog/providers.toml");
            let mut root: CatalogRoot = toml::from_str(providers_toml).expect(
                "BUG: built-in providers.toml failed to parse. \
                         This is a build-time error — the file is validated at startup. \
                         Run `cargo test -p oxi-ai catalog` to reproduce.",
            );
            // Load all model files in data/catalog/models/
            let model_dir_models = load_all_model_files();
            for (provider_id, models) in model_dir_models {
                root.models.insert(provider_id, models);
            }
            root
        })
    }

    /// Find a provider by id or alias.
    pub fn find_provider(&self, name: &str) -> Option<&BuiltinProviderEntry> {
        self.provider
            .iter()
            .find(|p| p.id == name || p.aliases.iter().any(|a| a == name))
    }

    /// Resolve an alias to its canonical provider id.
    pub fn resolve_provider_id(&self, name: &str) -> Option<&str> {
        self.find_provider(name).map(|p| p.id.as_str())
    }

    /// Get all models for a provider.
    pub fn models_for(&self, provider_id: &str) -> &[BuiltinModelEntry] {
        self.models
            .get(provider_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

/// Load all `*.toml` files in `data/catalog/models/` and merge into a single map.
///
/// Each file is expected to declare `provider = "..."` at the top, then a list
/// of `[[model]]` entries. Files with the same provider id are merged.
///
/// This is a build-time/static operation — the directory contents are enumerated
/// at build time via a generated `models_index.rs` file. For now we hardcode
/// the list; see `models_index!` macro.
fn load_all_model_files() -> std::collections::BTreeMap<String, Vec<BuiltinModelEntry>> {
    let mut out: std::collections::BTreeMap<String, Vec<BuiltinModelEntry>> =
        std::collections::BTreeMap::new();
    for (_provider_id, toml_str) in model::models_index() {
        match toml::from_str::<ModelFile>(toml_str) {
            Ok(file) => {
                out.entry(file.provider).or_default().extend(file.model);
            }
            Err(e) => {
                panic!(
                    "BUG: built-in model file failed to parse: {e}\n\
                     TOML:\n{toml_str}"
                );
            }
        }
    }
    out
}

/// A single model catalog file (one provider per file).
#[derive(Debug, serde::Deserialize)]
struct ModelFile {
    /// Provider id this file declares models for.
    provider: String,
    /// Models in this file.
    #[serde(default)]
    model: Vec<BuiltinModelEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads() {
        let root = CatalogRoot::get();
        assert!(
            !root.provider.is_empty(),
            "providers.toml should not be empty"
        );
    }

    #[test]
    fn all_providers_have_unique_ids() {
        let root = CatalogRoot::get();
        let mut seen = std::collections::HashSet::new();
        for p in &root.provider {
            assert!(seen.insert(&p.id), "duplicate provider id: {}", p.id);
        }
    }

    #[test]
    fn all_provider_aliases_resolve() {
        let root = CatalogRoot::get();
        for p in &root.provider {
            for alias in &p.aliases {
                let resolved = root.resolve_provider_id(alias);
                assert!(resolved.is_some(), "alias {alias} does not resolve");
            }
        }
    }

    #[test]
    fn find_provider_by_id_and_alias() {
        let root = CatalogRoot::get();
        // Canonical id
        assert!(root.find_provider("anthropic").is_some());
        // Alias
        assert!(root.find_provider("kimi").is_some()); // alias for kimi-coding
                                                       // Not found
        assert!(root.find_provider("nonexistent").is_none());
    }
}
