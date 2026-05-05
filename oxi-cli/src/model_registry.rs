//! Model registry — manages built-in and custom models, provides API key resolution.
//!
//! Originally inspired by pi-mono's model-registry.
//!
//! This module provides a `ModelRegistry` that:
//! - Loads built-in models from `oxi_ai::model_db`
//! - Loads custom models and provider overrides from a `models.json` file
//! - Resolves API keys via `AuthStorage` (env vars, OAuth tokens, stored credentials)
//! - Supports dynamic provider registration (extensions)
//! - Provides model filtering by provider, capability, and modality

use crate::auth_storage::{AuthStorage, AuthStatus};
use crate::provider_display_names::BUILT_IN_PROVIDER_DISPLAY_NAMES;
use oxi_ai::model_db;
use oxi_ai::{Api, CompatSettings, Cost, InputModality, Model};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

// =============================================================================
// JSON Schema types for models.json
// =============================================================================

/// Per-model override fields (all optional, merged with built-in model).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelOverride {
    pub name: Option<String>,
    pub reasoning: Option<bool>,
    pub thinking_level_map: Option<HashMap<String, Option<String>>>,
    pub input: Option<Vec<InputModality>>,
    pub cost: Option<PartialCost>,
    pub context_window: Option<usize>,
    pub max_tokens: Option<usize>,
    pub headers: Option<HashMap<String, String>>,
    pub compat: Option<CompatSettings>,
}

/// Partial cost override — each field is optional.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PartialCost {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// Custom model definition in models.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDefinition {
    pub id: String,
    pub name: Option<String>,
    pub api: Option<Api>,
    pub base_url: Option<String>,
    pub reasoning: Option<bool>,
    pub thinking_level_map: Option<HashMap<String, Option<String>>>,
    pub input: Option<Vec<InputModality>>,
    pub cost: Option<Cost>,
    pub context_window: Option<usize>,
    pub max_tokens: Option<usize>,
    pub headers: Option<HashMap<String, String>>,
    pub compat: Option<CompatSettings>,
}

/// Provider configuration in models.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api: Option<Api>,
    pub headers: Option<HashMap<String, String>>,
    pub compat: Option<CompatSettings>,
    pub auth_header: Option<bool>,
    pub models: Option<Vec<ModelDefinition>>,
    pub model_overrides: Option<HashMap<String, ModelOverride>>,
}

/// Top-level models.json configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    pub providers: HashMap<String, ProviderConfig>,
}

// =============================================================================
// Provider override (baseUrl, compat) — not request auth/headers
// =============================================================================

/// Provider-level overrides loaded from models.json (baseUrl, compat).
#[derive(Debug, Clone, Default)]
struct ProviderOverride {
    base_url: Option<String>,
    compat: Option<CompatSettings>,
}

/// Provider-level request config (apiKey, headers, authHeader).
#[derive(Debug, Clone, Default)]
struct ProviderRequestConfig {
    api_key: Option<String>,
    headers: Option<HashMap<String, String>>,
    auth_header: bool,
}

/// Result of loading custom models from models.json.
struct CustomModelsResult {
    models: Vec<Model>,
    overrides: HashMap<String, ProviderOverride>,
    model_overrides: HashMap<String, HashMap<String, ModelOverride>>,
    error: Option<String>,
}

fn empty_custom_models_result(error: Option<String>) -> CustomModelsResult {
    CustomModelsResult {
        models: vec![],
        overrides: HashMap::new(),
        model_overrides: HashMap::new(),
        error,
    }
}

// =============================================================================
// Resolved request auth
// =============================================================================

/// Result of resolving API key and headers for a model.
#[derive(Debug, Clone)]
pub struct ResolvedRequestAuth {
    /// Whether resolution was successful.
    pub ok: bool,
    /// The API key (if resolved).
    pub api_key: Option<String>,
    /// Merged headers (model + provider + models.json).
    pub headers: Option<HashMap<String, String>>,
    /// Error message if `ok` is false.
    pub error: Option<String>,
}

impl ResolvedRequestAuth {
    fn ok(api_key: Option<String>, headers: Option<HashMap<String, String>>) -> Self {
        Self {
            ok: true,
            api_key,
            headers,
            error: None,
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            api_key: None,
            headers: None,
            error: Some(msg.into()),
        }
    }
}

// =============================================================================
// Helper: merge compat
// =============================================================================

/// Merge a base compat with an override compat. Override fields win.
fn merge_compat(base: Option<&CompatSettings>, override_compat: Option<&CompatSettings>) -> Option<CompatSettings> {
    match (base, override_compat) {
        (None, None) => None,
        (None, Some(ov)) => Some(ov.clone()),
        (Some(b), None) => Some(b.clone()),
        (Some(b), Some(ov)) => {
            // Merge: override fields take precedence
            Some(CompatSettings {
                supports_store: ov.supports_store,
                supports_developer_role: ov.supports_developer_role,
                supports_reasoning_effort: ov.supports_reasoning_effort,
                supports_usage_in_streaming: ov.supports_usage_in_streaming,
                max_tokens_field: ov.max_tokens_field.or(b.max_tokens_field),
                requires_tool_result_name: ov.requires_tool_result_name,
                requires_assistant_after_tool_result: ov.requires_assistant_after_tool_result,
                requires_thinking_as_text: ov.requires_thinking_as_text,
                thinking_format: ov.thinking_format.or(b.thinking_format),
            })
        }
    }
}

// =============================================================================
// Helper: apply model override
// =============================================================================

/// Deep merge a model override into a model.
fn apply_model_override(model: &Model, override_def: &ModelOverride) -> Model {
    let mut result = model.clone();

    if let Some(ref name) = override_def.name {
        result.name = name.clone();
    }
    if let Some(reasoning) = override_def.reasoning {
        result.reasoning = reasoning;
    }
    if let Some(ref input) = override_def.input {
        result.input = input.clone();
    }
    if let Some(ctx) = override_def.context_window {
        result.context_window = ctx;
    }
    if let Some(mt) = override_def.max_tokens {
        result.max_tokens = mt;
    }

    // Merge cost (partial override)
    if let Some(ref cost) = override_def.cost {
        result.cost = Cost {
            input: cost.input.unwrap_or(result.cost.input),
            output: cost.output.unwrap_or(result.cost.output),
            cache_read: cost.cache_read.unwrap_or(result.cost.cache_read),
            cache_write: cost.cache_write.unwrap_or(result.cost.cache_write),
        };
    }

    // Deep merge compat
    result.compat = merge_compat(result.compat.as_ref(), override_def.compat.as_ref());

    result
}

// =============================================================================
// Helper: resolve a config value string
// =============================================================================

/// Resolve a config value. If it starts with `!`, treat the rest as a command
/// to execute. Otherwise, look it up as an env var. If the value is a literal
/// string (not an env var name), return it as-is.
fn resolve_config_value(value: &str) -> Option<String> {
    if value.starts_with('!') {
        // Command-backed config value — execute and capture stdout
        let cmd = &value[1..];
        #[cfg(unix)]
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .ok()?;
        #[cfg(windows)]
        let output = std::process::Command::new("cmd")
            .arg("/C")
            .arg(cmd)
            .output()
            .ok()?;

        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    } else {
        // Try as env var
        std::env::var(value).ok()
    }
}

/// Resolve a config value or throw with a descriptive error message.
fn resolve_config_value_or_throw(value: &str, label: &str) -> Result<String, String> {
    if value.starts_with('!') {
        let cmd = &value[1..];
        #[cfg(unix)]
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map_err(|e| format!("Failed to execute command for {}: {}", label, e))?;
        #[cfg(windows)]
        let output = std::process::Command::new("cmd")
            .arg("/C")
            .arg(cmd)
            .output()
            .map_err(|e| format!("Failed to execute command for {}: {}", label, e))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(format!(
                "Command for {} failed with exit code {}",
                label, output.status
            ))
        }
    } else {
        std::env::var(value)
            .map_err(|_| format!("Environment variable '{}' not set for {}", value, label))
    }
}

/// Resolve optional headers map: if any value is an env-var ref, resolve it.
fn resolve_headers(
    headers: Option<&HashMap<String, String>>,
    label: &str,
) -> Result<Option<HashMap<String, String>>, String> {
    let Some(h) = headers else {
        return Ok(None);
    };
    if h.is_empty() {
        return Ok(None);
    }
    let mut resolved = HashMap::new();
    for (k, v) in h {
        // If value looks like an env var (uppercase + underscores), try resolving
        if v.chars().all(|c| c.is_uppercase() || c == '_' || c == ' ') && !v.contains(' ') {
            if let Ok(rv) = resolve_config_value_or_throw(v, &format!("{}.{}", label, k)) {
                resolved.insert(k.clone(), rv);
            } else {
                resolved.insert(k.clone(), v.clone());
            }
        } else {
            resolved.insert(k.clone(), v.clone());
        }
    }
    Ok(Some(resolved))
}

// =============================================================================
// Model Registry
// =============================================================================

/// Model registry — loads and manages models, resolves API keys via `AuthStorage`.
///
/// Mirrors the TypeScript `ModelRegistry` class:
/// - Loads built-in models from `oxi_ai::model_db`
/// - Loads custom models from `models.json` on disk
/// - Supports dynamic provider registration (for extensions)
/// - Resolves API keys through `AuthStorage`
pub struct ModelRegistry {
    /// All loaded models (built-in + custom).
    models: RwLock<Vec<Model>>,
    /// Per-provider request configs loaded from models.json.
    provider_request_configs: RwLock<HashMap<String, ProviderRequestConfig>>,
    /// Per-model request headers loaded from models.json.
    model_request_headers: RwLock<HashMap<String, HashMap<String, String>>>,
    /// Dynamically registered providers (from extensions).
    registered_providers: RwLock<HashMap<String, ProviderConfigInput>>,
    /// Error from loading models.json (if any).
    load_error: RwLock<Option<String>>,
    /// Auth storage for credential lookup.
    auth_storage: AuthStorage,
    /// Path to models.json (None for in-memory).
    models_json_path: Option<PathBuf>,
}

impl ModelRegistry {
    /// Create a new `ModelRegistry` that loads models from the given path.
    ///
    /// If `models_json_path` is `None`, falls back to
    /// `$HOME/.oxi/models.json` (or the XDG config dir equivalent).
    pub fn create(auth_storage: AuthStorage, models_json_path: Option<PathBuf>) -> Self {
        let models_json_path =
            models_json_path.or_else(|| dirs::config_dir().map(|p| p.join("oxi").join("models.json")));

        let registry = Self {
            models: RwLock::new(Vec::new()),
            provider_request_configs: RwLock::new(HashMap::new()),
            model_request_headers: RwLock::new(HashMap::new()),
            registered_providers: RwLock::new(HashMap::new()),
            load_error: RwLock::new(None),
            auth_storage,
            models_json_path,
        };

        registry.load_models_internal();
        registry
    }

    /// Create an in-memory registry (no models.json loading).
    pub fn in_memory(auth_storage: AuthStorage) -> Self {
        Self {
            models: RwLock::new(Vec::new()),
            provider_request_configs: RwLock::new(HashMap::new()),
            model_request_headers: RwLock::new(HashMap::new()),
            registered_providers: RwLock::new(HashMap::new()),
            load_error: RwLock::new(None),
            auth_storage,
            models_json_path: None,
        }
    }

    // =========================================================================
    // Public API
    // =========================================================================

    /// Reload models from disk (built-in + custom from models.json).
    pub fn refresh(&self) {
        // Clear caches
        self.provider_request_configs.write().unwrap().clear();
        self.model_request_headers.write().unwrap().clear();
        *self.load_error.write().unwrap() = None;

        self.load_models_internal();

        // Re-apply registered providers
        let providers = self.registered_providers.read().unwrap().clone();
        for (name, config) in &providers {
            self.apply_provider_config(name, config);
        }
    }

    /// Get any error from loading models.json.
    pub fn get_error(&self) -> Option<String> {
        self.load_error.read().unwrap().clone()
    }

    /// Get all models (built-in + custom).
    pub fn get_all(&self) -> Vec<Model> {
        self.models.read().unwrap().clone()
    }

    /// Get only models that have auth configured.
    pub fn get_available(&self) -> Vec<Model> {
        self.models
            .read()
            .unwrap()
            .iter()
            .filter(|m| self.has_configured_auth(m))
            .cloned()
            .collect()
    }

    /// Find a model by provider and ID.
    pub fn find(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.models
            .read()
            .unwrap()
            .iter()
            .find(|m| m.provider == provider && m.id == model_id)
            .cloned()
    }

    /// Resolve a model string (e.g. `"anthropic/claude-sonnet-4-20250514"`)
    /// into a `Model` object.
    ///
    /// Supports:
    /// - `"provider/model-id"`
    /// - `"model-id"` (searches all providers)
    pub fn resolve_model(&self, model_str: &str) -> Option<Model> {
        let models = self.models.read().unwrap();

        if let Some(slash) = model_str.find('/') {
            let provider = &model_str[..slash];
            let id = &model_str[slash + 1..];
            return models
                .iter()
                .find(|m| m.provider == provider && m.id == id)
                .cloned();
        }

        // Search by ID across all providers
        let matches: Vec<_> = models
            .iter()
            .filter(|m| m.id == model_str)
            .collect();

        if matches.len() == 1 {
            return Some(matches[0].clone());
        }

        // Multiple matches — prefer first
        if !matches.is_empty() {
            return Some(matches[0].clone());
        }

        // Fuzzy: try case-insensitive contains
        let lower = model_str.to_lowercase();
        models
            .iter()
            .find(|m| {
                m.id.to_lowercase().contains(&lower)
                    || m.name.to_lowercase().contains(&lower)
            })
            .cloned()
    }

    /// Check if a model has auth configured.
    pub fn has_configured_auth(&self, model: &Model) -> bool {
        self.auth_storage.has_auth(&model.provider)
            || self
                .provider_request_configs
                .read()
                .unwrap()
                .get(&model.provider)
                .and_then(|c| c.api_key.as_ref())
                .is_some()
    }

    /// Get API key and request headers for a model.
    pub fn get_api_key_and_headers(&self, model: &Model) -> ResolvedRequestAuth {
        self.get_api_key_and_headers_impl(model)
    }

    /// Check if a model is using OAuth credentials.
    pub fn is_using_oauth(&self, model: &Model) -> bool {
        let cred = self.auth_storage.get_oauth_credential(&model.provider);
        cred.is_some()
    }

    /// Get display name for a provider.
    ///
    /// Returns a static string for known providers, or the raw provider string.
    pub fn get_provider_display_name(&self, provider: &str) -> String {
        // Check registered providers for a custom name
        // Note: dynamic names are handled by get_provider_display_name_owned()

        BUILT_IN_PROVIDER_DISPLAY_NAMES
            .get(provider)
            .copied()
            .unwrap_or(provider)
            .to_string()
    }

    /// Get the display name for a provider, allocating if necessary for
    /// dynamically registered names.
    pub fn get_provider_display_name_owned(&self, provider: &str) -> String {
        if let Some(config) = self.registered_providers.read().unwrap().get(provider) {
            if let Some(ref name) = config.name {
                return name.clone();
            }
        }

        BUILT_IN_PROVIDER_DISPLAY_NAMES
            .get(provider)
            .copied()
            .unwrap_or(provider)
            .to_string()
    }

    /// Return auth status for a provider, including request auth configured
    /// in models.json.
    pub fn get_provider_auth_status(&self, provider: &str) -> AuthStatus {
        let auth_status = self.auth_storage.get_status(provider);
        if auth_status.source.is_some() {
            return auth_status;
        }

        let provider_api_key = self
            .provider_request_configs
            .read()
            .unwrap()
            .get(provider)
            .and_then(|c| c.api_key.clone());

        let Some(ref api_key_ref) = provider_api_key else {
            return auth_status;
        };

        if api_key_ref.starts_with('!') {
            return AuthStatus {
                configured: true,
                source: Some("models_json_command".to_string()),
                label: None,
            };
        }

        if std::env::var(api_key_ref).is_ok() {
            return AuthStatus {
                configured: true,
                source: Some("environment".to_string()),
                label: Some(api_key_ref.clone()),
            };
        }

        AuthStatus {
            configured: true,
            source: Some("models_json_key".to_string()),
            label: None,
        }
    }

    /// Get API key for a provider.
    pub fn get_api_key_for_provider(&self, provider: &str) -> Option<String> {
        // Try auth storage first
        if let Some(key) = self.auth_storage.get_api_key(provider) {
            return Some(key);
        }

        // Try provider request config from models.json
        let api_key_str = self
            .provider_request_configs
            .read()
            .unwrap()
            .get(provider)
            .and_then(|c| c.api_key.clone())?;

        resolve_config_value(&api_key_str)
    }

    /// Get all unique providers from loaded models.
    pub fn get_available_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self
            .models
            .read()
            .unwrap()
            .iter()
            .map(|m| m.provider.clone())
            .collect();
        providers.sort();
        providers.dedup();
        providers
    }

    /// Get all providers that have credentials configured.
    pub fn get_providers_with_credentials(&self) -> Vec<String> {
        let providers = self.get_available_providers();
        providers
            .into_iter()
            .filter(|p| {
                self.auth_storage.has_auth(p)
                    || self
                        .provider_request_configs
                        .read()
                        .unwrap()
                        .get(p)
                        .and_then(|c| c.api_key.as_ref())
                        .is_some()
            })
            .collect()
    }

    /// Get all models that have credentials configured.
    pub fn get_available_models(&self) -> Vec<Model> {
        self.get_available()
    }

    /// Get the user's default model.
    ///
    /// Returns the first available model, preferring:
    /// 1. Anthropic Claude Sonnet 4
    /// 2. OpenAI GPT-4o
    /// 3. Any available model
    pub fn get_default_model(&self) -> Option<Model> {
        let available = self.get_available();

        // Prefer known defaults
        let preferred = [
            ("anthropic", "claude-sonnet-4-20250514"),
            ("anthropic", "claude-sonnet-4-5"),
            ("anthropic", "claude-sonnet-4-6"),
            ("openai", "gpt-4o"),
        ];

        for (provider, id) in &preferred {
            if let Some(model) = available.iter().find(|m| m.provider == *provider && m.id == *id) {
                return Some(model.clone());
            }
        }

        available.into_iter().next()
    }

    /// Register a provider dynamically (from extensions).
    pub fn register_provider(&self, provider_name: &str, config: ProviderConfigInput) {
        self.apply_provider_config(provider_name, &config);
        self.upsert_registered_provider(provider_name, config);
    }

    /// Unregister a previously registered provider.
    pub fn unregister_provider(&self, provider_name: &str) {
        if !self
            .registered_providers
            .read()
            .unwrap()
            .contains_key(provider_name)
        {
            return;
        }
        self.registered_providers
            .write()
            .unwrap()
            .remove(provider_name);
        self.refresh();
    }

    // =========================================================================
    // Model filtering
    // =========================================================================

    /// Filter models by provider.
    pub fn filter_by_provider(&self, provider: &str) -> Vec<Model> {
        self.models
            .read()
            .unwrap()
            .iter()
            .filter(|m| m.provider == provider)
            .cloned()
            .collect()
    }

    /// Filter models by capability (reasoning).
    pub fn filter_by_capability(&self, reasoning: bool) -> Vec<Model> {
        self.models
            .read()
            .unwrap()
            .iter()
            .filter(|m| m.reasoning == reasoning)
            .cloned()
            .collect()
    }

    /// Filter models by input modality (e.g., vision/image support).
    pub fn filter_by_modality(&self, modality: InputModality) -> Vec<Model> {
        self.models
            .read()
            .unwrap()
            .iter()
            .filter(|m| m.input.contains(&modality))
            .cloned()
            .collect()
    }

    /// Search models by pattern (case-insensitive substring match on id or name).
    pub fn search(&self, pattern: &str) -> Vec<Model> {
        let lower = pattern.to_lowercase();
        self.models
            .read()
            .unwrap()
            .iter()
            .filter(|m| {
                m.id.to_lowercase().contains(&lower)
                    || m.name.to_lowercase().contains(&lower)
            })
            .cloned()
            .collect()
    }

    // =========================================================================
    // Private: model loading
    // =========================================================================

    fn load_models_internal(&self) {
        // Load custom models and overrides from models.json
        let custom_result = match self.models_json_path {
            Some(ref path) => self.load_custom_models(path),
            None => empty_custom_models_result(None),
        };

        if let Some(ref error) = custom_result.error {
            *self.load_error.write().unwrap() = Some(error.clone());
            // Keep built-in models even if custom models failed to load
        }

        let built_in = self.load_built_in_models(&custom_result.overrides, &custom_result.model_overrides);
        let combined = self.merge_custom_models(built_in, &custom_result.models);

        *self.models.write().unwrap() = combined;
    }

    /// Load built-in models from model_db and apply provider/model overrides.
    fn load_built_in_models(
        &self,
        overrides: &HashMap<String, ProviderOverride>,
        model_overrides: &HashMap<String, HashMap<String, ModelOverride>>,
    ) -> Vec<Model> {
        let mut result = Vec::new();

        for provider_name in model_db::get_providers() {
            let entries = model_db::get_provider_models(provider_name);
            let provider_override = overrides.get(provider_name);
            let per_model_overrides = model_overrides.get(provider_name);

            for entry in entries {
                // Convert ModelEntry -> Model
                let mut model = Model {
                    id: entry.id.to_string(),
                    name: entry.name.to_string(),
                    api: entry.api,
                    provider: entry.provider.to_string(),
                    base_url: self.default_base_url_for_provider(entry.provider),
                    reasoning: entry.reasoning,
                    input: entry.input.to_vec(),
                    cost: Cost {
                        input: entry.cost_input,
                        output: entry.cost_output,
                        cache_read: entry.cost_cache_read,
                        cache_write: entry.cost_cache_write,
                    },
                    context_window: entry.context_window as usize,
                    max_tokens: entry.max_tokens as usize,
                    headers: HashMap::new(),
                    compat: None,
                };

                // Apply provider-level baseUrl/compat override
                if let Some(po) = provider_override {
                    if let Some(ref url) = po.base_url {
                        model.base_url = url.clone();
                    }
                    model.compat = merge_compat(model.compat.as_ref(), po.compat.as_ref());
                }

                // Apply per-model override
                if let Some(per_model) = per_model_overrides {
                    if let Some(mo) = per_model.get(entry.id) {
                        model = apply_model_override(&model, mo);
                    }
                }

                result.push(model);
            }
        }

        result
    }

    /// Merge custom models into built-in list by provider+id (custom wins on conflicts).
    fn merge_custom_models(&self, built_in: Vec<Model>, custom: &[Model]) -> Vec<Model> {
        let mut merged = built_in;

        for custom_model in custom {
            if let Some(idx) = merged
                .iter()
                .position(|m| m.provider == custom_model.provider && m.id == custom_model.id)
            {
                merged[idx] = custom_model.clone();
            } else {
                merged.push(custom_model.clone());
            }
        }

        merged
    }

    // =========================================================================
    // Private: models.json loading
    // =========================================================================

    fn load_custom_models(&self, path: &Path) -> CustomModelsResult {
        if !path.exists() {
            return empty_custom_models_result(None);
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return empty_custom_models_result(Some(format!(
                    "Failed to read models.json: {}\n\nFile: {}",
                    e,
                    path.display()
                )));
            }
        };

        let config: ModelsConfig = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                return empty_custom_models_result(Some(format!(
                    "Failed to parse models.json: {}\n\nFile: {}",
                    e,
                    path.display()
                )));
            }
        };

        // Additional validation
        if let Err(e) = self.validate_config(&config) {
            return empty_custom_models_result(Some(format!(
                "Invalid models.json: {}\n\nFile: {}",
                e,
                path.display()
            )));
        }

        let mut overrides = HashMap::new();
        let mut model_overrides_map: HashMap<String, HashMap<String, ModelOverride>> = HashMap::new();

        let built_in_providers: Vec<&str> = model_db::get_providers();

        for (provider_name, provider_config) in &config.providers {
            // Store provider-level overrides
            if provider_config.base_url.is_some() || provider_config.compat.is_some() {
                overrides.insert(
                    provider_name.clone(),
                    ProviderOverride {
                        base_url: provider_config.base_url.clone(),
                        compat: provider_config.compat.clone(),
                    },
                );
            }

            // Store provider request config (apiKey, headers, authHeader)
            self.store_provider_request_config(provider_name, provider_config);

            // Store per-model overrides
            if let Some(ref model_overrides) = provider_config.model_overrides {
                model_overrides_map.insert(provider_name.clone(), model_overrides.clone());
                for (model_id, model_override) in model_overrides {
                    self.store_model_headers(provider_name, model_id, model_override.headers.as_ref());
                }
            }
        }

        let models = self.parse_models(&config, &built_in_providers);

        CustomModelsResult {
            models,
            overrides,
            model_overrides: model_overrides_map,
            error: None,
        }
    }

    fn validate_config(&self, config: &ModelsConfig) -> Result<(), String> {
        let built_in_providers: Vec<&str> = model_db::get_providers();

        for (provider_name, provider_config) in &config.providers {
            let is_built_in = built_in_providers.contains(&provider_name.as_str());
            let models = provider_config.models.as_deref().unwrap_or(&[]);
            let has_model_overrides = provider_config
                .model_overrides
                .as_ref()
                .map(|m| !m.is_empty())
                .unwrap_or(false);

            if models.is_empty() {
                // Override-only config
                if provider_config.base_url.is_none()
                    && provider_config.headers.is_none()
                    && provider_config.compat.is_none()
                    && !has_model_overrides
                {
                    return Err(format!(
                        "Provider {}: must specify \"baseUrl\", \"headers\", \"compat\", \"modelOverrides\", or \"models\".",
                        provider_name
                    ));
                }
            } else if !is_built_in {
                // Non-built-in providers with custom models require endpoint + auth
                if provider_config.base_url.is_none() {
                    return Err(format!(
                        "Provider {}: \"baseUrl\" is required when defining custom models.",
                        provider_name
                    ));
                }
                if provider_config.api_key.is_none() {
                    return Err(format!(
                        "Provider {}: \"apiKey\" is required when defining custom models.",
                        provider_name
                    ));
                }
            }

            for model_def in models {
                let has_model_api = model_def.api.is_some();
                let has_provider_api = provider_config.api.is_some();

                if !has_provider_api && !has_model_api && !is_built_in {
                    return Err(format!(
                        "Provider {}, model {}: no \"api\" specified. Set at provider or model level.",
                        provider_name, model_def.id
                    ));
                }

                if model_def.context_window.is_some_and(|cw| cw == 0) {
                    return Err(format!(
                        "Provider {}, model {}: invalid contextWindow",
                        provider_name, model_def.id
                    ));
                }
                if model_def.max_tokens.is_some_and(|mt| mt == 0) {
                    return Err(format!(
                        "Provider {}, model {}: invalid maxTokens",
                        provider_name, model_def.id
                    ));
                }
            }
        }

        Ok(())
    }

    fn parse_models(
        &self,
        config: &ModelsConfig,
        built_in_providers: &[&str],
    ) -> Vec<Model> {
        let mut models = Vec::new();

        // Cache built-in defaults per provider
        let mut defaults_cache: HashMap<String, (Api, String)> = HashMap::new();

        for (provider_name, provider_config) in &config.providers {
            let model_defs = match provider_config.models {
                Some(ref m) if !m.is_empty() => m,
                _ => continue,
            };

            let is_built_in = built_in_providers.contains(&provider_name.as_str());

            // Get built-in defaults (api, baseUrl) for this provider
            let built_in_defaults = if is_built_in {
                if !defaults_cache.contains_key(provider_name) {
                    let entries = model_db::get_provider_models(provider_name.as_str());
                    if let Some(first) = entries.first() {
                        defaults_cache.insert(
                            provider_name.clone(),
                            (first.api, self.default_base_url_for_provider(provider_name.as_str())),
                        );
                    }
                }
                defaults_cache.get(provider_name)
            } else {
                None
            };

            for model_def in model_defs {
                let api = model_def
                    .api
                    .or(provider_config.api)
                    .or(built_in_defaults.map(|(a, _)| *a));

                let Some(api) = api else { continue };

                let base_url = model_def
                    .base_url
                    .as_deref()
                    .or(provider_config.base_url.as_deref())
                    .or(built_in_defaults.map(|(_, u)| u.as_str()));

                let Some(base_url) = base_url else { continue };

                let compat = merge_compat(
                    provider_config.compat.as_ref(),
                    model_def.compat.as_ref(),
                );

                self.store_model_headers(
                    provider_name,
                    &model_def.id,
                    model_def.headers.as_ref(),
                );

                models.push(Model {
                    id: model_def.id.clone(),
                    name: model_def.name.clone().unwrap_or_else(|| model_def.id.clone()),
                    api,
                    provider: provider_name.clone(),
                    base_url: base_url.to_string(),
                    reasoning: model_def.reasoning.unwrap_or(false),
                    input: model_def
                        .input
                        .clone()
                        .unwrap_or_else(|| vec![InputModality::Text]),
                    cost: model_def.cost.clone().unwrap_or(Cost {
                        input: 0.0,
                        output: 0.0,
                        cache_read: 0.0,
                        cache_write: 0.0,
                    }),
                    context_window: model_def.context_window.unwrap_or(128_000),
                    max_tokens: model_def.max_tokens.unwrap_or(16_384),
                    headers: HashMap::new(),
                    compat,
                });
            }
        }

        models
    }

    // =========================================================================
    // Private: provider config helpers
    // =========================================================================

    fn store_provider_request_config(
        &self,
        provider_name: &str,
        config: &ProviderConfig,
    ) {
        if config.api_key.is_none() && config.headers.is_none() && config.auth_header.is_none() {
            return;
        }

        self.provider_request_configs
            .write()
            .unwrap()
            .insert(
                provider_name.to_string(),
                ProviderRequestConfig {
                    api_key: config.api_key.clone(),
                    headers: config.headers.clone(),
                    auth_header: config.auth_header.unwrap_or(false),
                },
            );
    }

    fn store_model_headers(
        &self,
        provider_name: &str,
        model_id: &str,
        headers: Option<&HashMap<String, String>>,
    ) {
        let key = format!("{}:{}", provider_name, model_id);
        let mut hdr_map = self.model_request_headers.write().unwrap();

        match headers {
            Some(h) if !h.is_empty() => {
                hdr_map.insert(key, h.clone());
            }
            _ => {
                hdr_map.remove(&key);
            }
        }
    }

    fn store_provider_request_config_from_input(
        &self,
        provider_name: &str,
        config: &ProviderConfigInput,
    ) {
        if config.api_key.is_none() && config.headers.is_none() && !config.auth_header {
            return;
        }

        self.provider_request_configs
            .write()
            .unwrap()
            .insert(
                provider_name.to_string(),
                ProviderRequestConfig {
                    api_key: config.api_key.clone(),
                    headers: config.headers.clone(),
                    auth_header: config.auth_header,
                },
            );
    }

    // =========================================================================
    // Private: API key resolution
    // =========================================================================

    fn get_api_key_and_headers_impl(&self, model: &Model) -> ResolvedRequestAuth {
        let provider_config = self
            .provider_request_configs
            .read()
            .unwrap()
            .get(&model.provider)
            .cloned();

        // Try auth storage
        let api_key_from_storage = self.auth_storage.get_api_key(&model.provider);

        // Try provider config from models.json
        let api_key = match api_key_from_storage {
            Some(key) => Some(key),
            None => {
                provider_config
                    .as_ref()
                    .and_then(|c| c.api_key.clone())
                    .and_then(|raw| {
                        resolve_config_value_or_throw(
                            &raw,
                            &format!("API key for provider \"{}\"", model.provider),
                        )
                        .ok()
                    })
            }
        };

        // Resolve headers
        let provider_headers = resolve_headers(
            provider_config.as_ref().and_then(|c| c.headers.as_ref()),
            &format!("provider \"{}\"", model.provider),
        );

        let model_headers_key = format!("{}:{}", model.provider, model.id);
        let model_headers_raw = self
            .model_request_headers
            .read()
            .unwrap()
            .get(&model_headers_key)
            .cloned();
        let model_headers = resolve_headers(
            model_headers_raw.as_ref(),
            &format!("model \"{}/{}\"", model.provider, model.id),
        );

        // Merge headers: model.headers < provider_headers < model_headers
        let mut headers: HashMap<String, String> = HashMap::new();
        if !model.headers.is_empty() {
            headers.extend(model.headers.clone());
        }
        if let Ok(Some(ph)) = provider_headers {
            headers.extend(ph);
        }
        if let Ok(Some(mh)) = model_headers {
            headers.extend(mh);
        }

        // If authHeader is set, add Authorization: Bearer
        if provider_config.as_ref().map(|c| c.auth_header).unwrap_or(false) {
            let Some(ref key) = api_key else {
                return ResolvedRequestAuth::err(format!(
                    "No API key found for \"{}\"",
                    model.provider
                ));
            };
            headers.insert("Authorization".to_string(), format!("Bearer {}", key));
        }

        let headers = if headers.is_empty() {
            None
        } else {
            Some(headers)
        };

        ResolvedRequestAuth::ok(api_key, headers)
    }

    // =========================================================================
    // Private: provider registration
    // =========================================================================

    fn apply_provider_config(&self, provider_name: &str, config: &ProviderConfigInput) {
        self.store_provider_request_config_from_input(provider_name, config);

        if let Some(ref models) = config.models {
            if !models.is_empty() {
                // Full replacement: remove existing models for this provider
                let mut all_models = self.models.write().unwrap();
                all_models.retain(|m| m.provider != provider_name);

                for model_def in models {
                    let api = model_def.api.or(config.api);
                    let base_url = model_def
                        .base_url
                        .as_deref()
                        .or(config.base_url.as_deref())
                        .unwrap_or("");

                    self.store_model_headers(
                        provider_name,
                        &model_def.id,
                        model_def.headers.as_ref(),
                    );

                    all_models.push(Model {
                        id: model_def.id.clone(),
                        name: model_def.name.clone().unwrap_or_else(|| model_def.id.clone()),
                        api: api.unwrap_or(Api::OpenAiCompletions),
                        provider: provider_name.to_string(),
                        base_url: base_url.to_string(),
                        reasoning: model_def.reasoning.unwrap_or(false),
                        input: model_def
                            .input
                            .clone()
                            .unwrap_or_else(|| vec![InputModality::Text]),
                        cost: model_def.cost.clone().unwrap_or_default(),
                        context_window: model_def.context_window.unwrap_or(128_000),
                        max_tokens: model_def.max_tokens.unwrap_or(16_384),
                        headers: HashMap::new(),
                        compat: model_def.compat.clone(),
                    });
                }
            }
        } else if config.base_url.is_some() {
            // Override-only: update baseUrl for existing models
            let mut all_models = self.models.write().unwrap();
            if let Some(ref base_url) = config.base_url {
                for m in all_models.iter_mut() {
                    if m.provider == provider_name {
                        m.base_url = base_url.clone();
                    }
                }
            }
        }
    }

    fn upsert_registered_provider(&self, provider_name: &str, config: ProviderConfigInput) {
        let mut providers = self.registered_providers.write().unwrap();
        match providers.get_mut(provider_name) {
            Some(existing) => {
                // Merge: defined values in incoming override, preserve undefined
                if config.name.is_some() {
                    existing.name = config.name.clone();
                }
                if config.base_url.is_some() {
                    existing.base_url = config.base_url.clone();
                }
                if config.api_key.is_some() {
                    existing.api_key = config.api_key.clone();
                }
                if config.api.is_some() {
                    existing.api = config.api;
                }
                if config.headers.is_some() {
                    existing.headers = config.headers.clone();
                }
                if config.auth_header {
                    existing.auth_header = config.auth_header;
                }
                if config.models.is_some() {
                    existing.models = config.models.clone();
                }
            }
            None => {
                providers.insert(provider_name.to_string(), config);
            }
        }
    }

    // =========================================================================
    // Private: utilities
    // =========================================================================

    /// Get the default base URL for a known provider.
    fn default_base_url_for_provider(&self, provider: &str) -> String {
        // Try model_db to find a model and extract its base URL
        // We don't store base URLs in model_db, so use a lookup table.
        match provider {
            "anthropic" => "https://api.anthropic.com".to_string(),
            "openai" => "https://api.openai.com/v1".to_string(),
            "google" => "https://generativelanguage.googleapis.com".to_string(),
            "google-vertex" => "https://us-central1-aiplatform.googleapis.com".to_string(),
            "deepseek" => "https://api.deepseek.com".to_string(),
            "mistral" => "https://api.mistral.ai".to_string(),
            "groq" => "https://api.groq.com/openai/v1".to_string(),
            "cerebras" => "https://api.cerebras.ai".to_string(),
            "xai" => "https://api.x.ai/v1".to_string(),
            "openrouter" => "https://openrouter.ai/api/v1".to_string(),
            "azure-openai-responses" => "https://{resource}.openai.azure.com".to_string(),
            "amazon-bedrock" => "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            _ => "".to_string(),
        }
    }
}

// =============================================================================
// Provider config input (for dynamic registration from extensions)
// =============================================================================

/// Input type for `register_provider` API (from extensions).
#[derive(Debug, Clone, Default)]
pub struct ProviderConfigInput {
    /// Display name for the provider.
    pub name: Option<String>,
    /// Base URL for the provider's API.
    pub base_url: Option<String>,
    /// API key (may be an env var name or a literal key).
    pub api_key: Option<String>,
    /// API protocol to use.
    pub api: Option<Api>,
    /// Additional headers to send with every request.
    pub headers: Option<HashMap<String, String>>,
    /// Whether to send `Authorization: Bearer <apiKey>` header.
    pub auth_header: bool,
    /// Models provided by this provider.
    pub models: Option<Vec<ModelDefinition>>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> ModelRegistry {
        ModelRegistry::in_memory(AuthStorage::in_memory())
    }

    #[test]
    fn test_in_memory_registry() {
        let registry = test_registry();
        // No models loaded for in-memory
        assert!(registry.get_all().is_empty());
    }

    #[test]
    fn test_get_all_providers() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let providers = registry.get_available_providers();
        assert!(!providers.is_empty());
        assert!(providers.contains(&"anthropic".to_string()));
        assert!(providers.contains(&"openai".to_string()));
    }

    #[test]
    fn test_resolve_model_by_provider_id() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let model = registry.resolve_model("anthropic/claude-sonnet-4-20250514");
        assert!(model.is_some());
        let m = model.unwrap();
        assert_eq!(m.provider, "anthropic");
        assert_eq!(m.id, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_resolve_model_by_id_only() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let model = registry.resolve_model("claude-sonnet-4-20250514");
        assert!(model.is_some());
        assert_eq!(model.unwrap().id, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_resolve_model_fuzzy() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let model = registry.resolve_model("sonnet 4");
        assert!(model.is_some());
    }

    #[test]
    fn test_find_model() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let model = registry.find("anthropic", "claude-sonnet-4-20250514");
        assert!(model.is_some());
    }

    #[test]
    fn test_find_model_not_found() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let model = registry.find("nonexistent", "model");
        assert!(model.is_none());
    }

    #[test]
    fn test_filter_by_provider() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let anthropic = registry.filter_by_provider("anthropic");
        assert!(!anthropic.is_empty());
        assert!(anthropic.iter().all(|m| m.provider == "anthropic"));
    }

    #[test]
    fn test_filter_by_capability() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let reasoning = registry.filter_by_capability(true);
        assert!(!reasoning.is_empty());
        assert!(reasoning.iter().all(|m| m.reasoning));
    }

    #[test]
    fn test_filter_by_modality() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let vision = registry.filter_by_modality(InputModality::Image);
        assert!(!vision.is_empty());
    }

    #[test]
    fn test_search_models() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let results = registry.search("claude");
        assert!(!results.is_empty());
        assert!(results.iter().all(|m|
            m.id.to_lowercase().contains("claude")
            || m.name.to_lowercase().contains("claude")
        ));
    }

    #[test]
    fn test_provider_display_name() {
        let registry = test_registry();
        assert_eq!(registry.get_provider_display_name("anthropic"), "Anthropic");
        assert_eq!(registry.get_provider_display_name("unknown"), "unknown");
    }

    #[test]
    fn test_has_configured_auth_no_auth() {
        // Remove all known provider env keys to avoid race conditions with parallel tests
        for key in &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GOOGLE_API_KEY",
            "GEMINI_API_KEY",
            "GROQ_API_KEY",
            "MISTRAL_API_KEY",
            "DEEPSEEK_API_KEY",
            "XAI_API_KEY",
            "COHERE_API_KEY",
            "CO_API_KEY",
            "PERPLEXITY_API_KEY",
        ] {
            std::env::remove_var(key);
        }
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let model = registry.find("anthropic", "claude-sonnet-4-20250514").unwrap();
        // No auth configured in test
        assert!(!registry.has_configured_auth(&model));
    }

    #[test]
    fn test_has_configured_auth_with_env() {
        let auth = AuthStorage::in_memory();
        auth.set_runtime_key("anthropic", "test-key".to_string());
        let registry = ModelRegistry::create(auth, None);
        let model = registry.find("anthropic", "claude-sonnet-4-20250514").unwrap();
        assert!(registry.has_configured_auth(&model));
    }

    #[test]
    fn test_get_api_key_and_headers_no_auth() {
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let model = registry.find("anthropic", "claude-sonnet-4-20250514").unwrap();
        let result = registry.get_api_key_and_headers(&model);
        assert!(result.ok);
        assert!(result.api_key.is_none());
    }

    #[test]
    fn test_is_using_oauth_false() {
        let registry = test_registry();
        let model = Model {
            id: "test".to_string(),
            name: "Test".to_string(),
            api: Api::AnthropicMessages,
            provider: "anthropic".to_string(),
            base_url: "https://test.com".to_string(),
            reasoning: false,
            input: vec![InputModality::Text],
            cost: Cost::default(),
            context_window: 128_000,
            max_tokens: 8192,
            headers: HashMap::new(),
            compat: None,
        };
        assert!(!registry.is_using_oauth(&model));
    }

    #[test]
    fn test_register_provider() {
        let registry = test_registry();

        let config = ProviderConfigInput {
            name: Some("Test Provider".to_string()),
            base_url: Some("https://test.example.com/v1".to_string()),
            api_key: Some("test-api-key".to_string()),
            api: Some(Api::OpenAiCompletions),
            models: Some(vec![ModelDefinition {
                id: "test-model".to_string(),
                name: Some("Test Model".to_string()),
                api: None,
                base_url: None,
                reasoning: Some(false),
                thinking_level_map: None,
                input: Some(vec![InputModality::Text]),
                cost: None,
                context_window: Some(128_000),
                max_tokens: Some(8192),
                headers: None,
                compat: None,
            }]),
            ..Default::default()
        };

        registry.register_provider("test-provider", config);

        let model = registry.find("test-provider", "test-model");
        assert!(model.is_some());
        assert_eq!(model.unwrap().name, "Test Model");
    }

    #[test]
    fn test_unregister_provider() {
        let registry = test_registry();

        let config = ProviderConfigInput {
            base_url: Some("https://test.example.com/v1".to_string()),
            api_key: Some("test-api-key".to_string()),
            api: Some(Api::OpenAiCompletions),
            models: Some(vec![ModelDefinition {
                id: "test-model".to_string(),
                name: Some("Test Model".to_string()),
                api: None,
                base_url: None,
                reasoning: None,
                thinking_level_map: None,
                input: None,
                cost: None,
                context_window: None,
                max_tokens: None,
                headers: None,
                compat: None,
            }]),
            ..Default::default()
        };

        registry.register_provider("test-provider", config);
        assert!(registry.find("test-provider", "test-model").is_some());

        registry.unregister_provider("test-provider");
        assert!(registry.find("test-provider", "test-model").is_none());
    }

    #[test]
    fn test_get_default_model() {
        // With env var set for anthropic
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        let registry = ModelRegistry::create(AuthStorage::in_memory(), None);
        let model = registry.get_default_model();
        assert!(model.is_some());
        assert_eq!(model.unwrap().provider, "anthropic");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn test_apply_model_override() {
        let base = Model {
            id: "test".to_string(),
            name: "Test Model".to_string(),
            api: Api::OpenAiCompletions,
            provider: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: Cost {
                input: 2.5,
                output: 10.0,
                cache_read: 1.25,
                cache_write: 0.0,
            },
            context_window: 128_000,
            max_tokens: 16_384,
            headers: HashMap::new(),
            compat: None,
        };

        let override_def = ModelOverride {
            name: Some("Overridden Name".to_string()),
            reasoning: Some(true),
            cost: Some(PartialCost {
                input: Some(5.0),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = apply_model_override(&base, &override_def);
        assert_eq!(result.name, "Overridden Name");
        assert!(result.reasoning);
        assert_eq!(result.cost.input, 5.0);
        assert_eq!(result.cost.output, 10.0); // Preserved from base
    }

    #[test]
    fn test_load_custom_models_file_not_found() {
        let registry = test_registry();
        let result = registry.load_custom_models(Path::new("/nonexistent/models.json"));
        assert!(result.error.is_none());
        assert!(result.models.is_empty());
    }

    #[test]
    fn test_resolve_config_value_env() {
        std::env::set_var("OXI_TEST_KEY", "test-value-123");
        let result = resolve_config_value("OXI_TEST_KEY");
        assert_eq!(result, Some("test-value-123".to_string()));
        std::env::remove_var("OXI_TEST_KEY");
    }

    #[test]
    fn test_resolve_config_value_missing_env() {
        let result = resolve_config_value("OXI_NONEXISTENT_KEY_12345");
        assert!(result.is_none());
    }

    #[test]
    fn test_merge_compat_none_none() {
        assert!(merge_compat(None, None).is_none());
    }

    #[test]
    fn test_merge_compat_some_none() {
        let base = CompatSettings::default();
        let result = merge_compat(Some(&base), None);
        assert!(result.is_some());
    }
}
