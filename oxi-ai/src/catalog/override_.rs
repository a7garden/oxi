//! User override layer (Layer 2 of the 3-tier catalog).
//!
//! Allows users to:
//! - Override prices for built-in models (e.g., negotiated enterprise rates)
//! - Add custom models not in the built-in catalog
//! - Add custom providers (e.g., internal AI gateway)
//!
//! Override files are TOML with the same schema as built-in files.
//! They are loaded at runtime from:
//!
//! 1. `OXI_CATALOG_OVERRIDE` environment variable (if set) — path to a TOML file
//! 2. `~/.oxi/catalog/overrides.toml` — global user overrides
//! 3. `.oxi/catalog.local.toml` — project-local overrides (relative to cwd)
//!
//! Later layers override earlier ones. The order is:
//!   Built-in (Layer 1) → Global override (Layer 2a) → Project override (Layer 2b) → Runtime (Layer 3)
//!
//! ## Merge semantics
//!
//! - **Providers**: merged by id. If the same provider id exists in both built-in
//!   and override, the override REPLACES the built-in entry (full replacement,
//!   not field-level merge — this is simpler and matches user intent).
//! - **Models**: merged by `(provider, id)` pair. If a model with the same
//!   `(provider, id)` exists, the override REPLACES it. New models are appended.
//!
//! ## Failure handling
//!
//! Override files that fail to parse or have wrong types are **silently
//! ignored** with a warning log. The user can check by running with
//! `OXI_CATALOG_DEBUG=1` to see the resolution path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::catalog::{BuiltinModelEntry, BuiltinProviderEntry};

/// User override container.
///
/// `None` for a field means "do not override" — the built-in value is kept.
/// `Some(value)` means "replace with this value".
#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct OverrideFile {
    /// Override provider entries (replace built-in by id).
    #[serde(default)]
    pub provider: Vec<BuiltinProviderEntry>,
    /// Override model entries (replace by `(provider, id)`, append new).
    #[serde(default)]
    pub model: Vec<BuiltinModelEntry>,
}

/// Find all override files in priority order (lowest to highest).
///
/// Returns `(path, content)` pairs. Files that don't exist are skipped.
/// The caller decides how to merge them.
pub fn find_override_files() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();

    // 1. Explicit env var
    if let Ok(path) = std::env::var("OXI_CATALOG_OVERRIDE")
        && let Some(pair) = read_override(&PathBuf::from(path))
    {
        out.push(pair);
    }

    // 2. Global: ~/.oxi/catalog/overrides.toml
    if let Some(home) = dirs::home_dir() {
        let path = home.join(".oxi").join("catalog").join("overrides.toml");
        if let Some(pair) = read_override(&path) {
            out.push(pair);
        }
    }

    // 3. Project-local: .oxi/catalog.local.toml (cwd)
    let path = PathBuf::from(".oxi/catalog.local.toml");
    if let Some(pair) = read_override(&path) {
        out.push(pair);
    }

    out
}

fn read_override(path: &Path) -> Option<(PathBuf, String)> {
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(content) => Some((path.to_path_buf(), content)),
        Err(e) => {
            tracing::warn!(?path, error = %e, "Failed to read override file");
            None
        }
    }
}

/// Apply a list of override files to a catalog snapshot.
///
/// Returns a new [`OverrideFile`] that is the union of all overrides.
/// In a real merge step the caller would apply this to the built-in
/// `BuiltinProviderEntry` and `BuiltinModelEntry` lists.
///
/// Returns `None` if no override files were found, or all failed to parse.
pub fn load_overrides() -> Option<OverrideFile> {
    let files = find_override_files();
    if files.is_empty() {
        return None;
    }

    let mut merged = OverrideFile::default();
    for (path, content) in files {
        match toml::from_str::<OverrideFile>(&content) {
            Ok(file) => {
                tracing::info!(
                    ?path,
                    providers = file.provider.len(),
                    models = file.model.len(),
                    "Loaded catalog override"
                );
                merged.provider.extend(file.provider);
                merged.model.extend(file.model);
            }
            Err(e) => {
                tracing::warn!(?path, error = %e, "Failed to parse override file; skipping");
            }
        }
    }

    if merged.provider.is_empty() && merged.model.is_empty() {
        None
    } else {
        Some(merged)
    }
}

/// Apply user overrides to a provider list in-place.
///
/// - Built-in providers with the same id as an override are REPLACED.
/// - Override providers with new ids are APPENDED.
pub fn apply_provider_overrides(
    providers: &mut Vec<BuiltinProviderEntry>,
    overrides: &[BuiltinProviderEntry],
) {
    for ov in overrides {
        if let Some(existing) = providers.iter_mut().find(|p| p.id == ov.id) {
            tracing::debug!(provider = %ov.id, "Replacing built-in provider with override");
            *existing = ov.clone();
        } else {
            tracing::debug!(provider = %ov.id, "Adding new provider from override");
            providers.push(ov.clone());
        }
    }
}

/// Apply user overrides to a model map in-place.
///
/// - Built-in models with the same `(provider, id)` are REPLACED.
/// - Override models with new `(provider, id)` are APPENDED to the provider's list.
pub fn apply_model_overrides(
    models: &mut BTreeMap<String, Vec<BuiltinModelEntry>>,
    overrides: &[BuiltinModelEntry],
) {
    for ov in overrides {
        let entry = models.entry(ov.provider.clone()).or_default();
        if let Some(existing) = entry.iter_mut().find(|m| m.id == ov.id) {
            tracing::debug!(provider = %ov.provider, model = %ov.id,
                "Replacing built-in model with override");
            *existing = ov.clone();
        } else {
            tracing::debug!(provider = %ov.provider, model = %ov.id,
                "Adding new model from override");
            entry.push(ov.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_override() {
        let toml = r#"
            [[provider]]
            id = "my-company-gateway"
            display_name = "My Company AI Gateway"
            env_key = "MY_GATEWAY_API_KEY"
            api = "openai-completions"
            auth_method = "bearer"
            category = "enterprise"
            description = "Internal AI gateway"

            [[model]]
            id = "my-company-gpt"
            name = "Internal GPT-4 variant"
            api = "openai-completions"
            provider = "my-company-gateway"
            context_window = 128000
            max_tokens = 8192
            cost_input = 1.0
            cost_output = 2.0
        "#;
        let parsed: OverrideFile = toml::from_str(toml).expect("parse");
        assert_eq!(parsed.provider.len(), 1);
        assert_eq!(parsed.model.len(), 1);
        assert_eq!(parsed.provider[0].id, "my-company-gateway");
        assert_eq!(parsed.model[0].id, "my-company-gpt");
    }

    #[test]
    fn apply_provider_override_replaces() {
        let mut providers = vec![BuiltinProviderEntry {
            id: "anthropic".into(),
            display_name: "Anthropic".into(),
            api: "anthropic-messages".into(),
            env_key: "ANTHROPIC_API_KEY".into(),
            category: "primary".into(),
            description: "Old".into(),
            auth_method: crate::catalog::AuthMethod::XApiKey,
            aliases: vec![],
            extra_env_keys: vec![],
            base_url: "".into(),
            extra_headers: vec![],
            default_enabled: true,
        }];
        let overrides = vec![BuiltinProviderEntry {
            id: "anthropic".into(),
            display_name: "Anthropic (Custom Pricing)".into(),
            api: "anthropic-messages".into(),
            env_key: "ANTHROPIC_API_KEY".into(),
            category: "primary".into(),
            description: "New".into(),
            auth_method: crate::catalog::AuthMethod::XApiKey,
            aliases: vec![],
            extra_env_keys: vec![],
            base_url: "".into(),
            extra_headers: vec![],
            default_enabled: true,
        }];
        apply_provider_overrides(&mut providers, &overrides);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].display_name, "Anthropic (Custom Pricing)");
    }

    #[test]
    fn apply_model_override_appends_new() {
        let mut models: BTreeMap<String, Vec<BuiltinModelEntry>> = BTreeMap::new();
        models.insert("anthropic".into(), vec![]);
        let overrides = vec![BuiltinModelEntry {
            id: "claude-test".into(),
            name: "Test".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            reasoning: false,
            input: vec!["text".into()],
            cost_input: 1.0,
            cost_output: 2.0,
            cost_cache_read: 0.0,
            cost_cache_write: 0.0,
            context_window: 200000,
            max_tokens: 8192,
            auth_method: crate::catalog::provider::AuthMethod::Bearer,
            base_url: None,
        }];
        apply_model_overrides(&mut models, &overrides);
        assert_eq!(models.get("anthropic").unwrap().len(), 1);
    }
}
