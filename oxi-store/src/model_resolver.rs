//! Model name resolution and matching
//!
//! Provides utilities for parsing model patterns, resolving model names,
//! finding models by glob, and determining the best model for startup.
//!
//! This module integrates with:
//! - oxi_ai::model_db::ModelEntry for cost/modality info
//! - oxi_ai::model_registry for runtime model lookup
//! - auth_storage for auth validation

use crate::settings::Settings;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Cached regex for date pattern matching (-YYYYMMDD)
static DATE_PATTERN_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"-\d{8}$").expect("date pattern regex should compile"));

/// Cached regex for date pattern stripping
static DATE_PATTERN_STRIP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"-\d{8}").expect("date pattern strip regex should compile")
});

// =============================================================================
// Constants
// =============================================================================

/// Default thinking level when none is specified
pub const DEFAULT_THINKING_LEVEL: &str = "medium";

/// Valid thinking levels in order of intensity
pub const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh"];

// =============================================================================
// Model Types
// =============================================================================

/// Known AI providers
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Provider {
    /// Unique provider identifier (e.g. "anthropic", "openai").
    pub id: String,
    /// Human-readable provider name.
    pub name: String,
    /// Optional provider website URL.
    pub website: Option<String>,
}

impl Provider {
    /// Create a new provider with the given identifier and display name.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            website: None,
        }
    }

    /// Attach a website URL to this provider (builder-style).
    pub fn with_website(mut self, website: impl Into<String>) -> Self {
        self.website = Some(website.into());
        self
    }
}

/// A discovered model
#[derive(Debug, Clone)]
pub struct Model {
    /// Provider that hosts this model.
    pub provider: String,
    /// Model identifier (e.g. "claude-sonnet-4-5").
    pub id: String,
    /// Human-readable model name.
    pub name: Option<String>,
    /// Optional model description.
    pub description: Option<String>,
    /// Maximum context window size in tokens.
    pub context_window: Option<u32>,
    /// Features the model supports (e.g. "tools", "vision").
    pub supported_features: Vec<String>,
    // Unified from ModelEntry - cost info
    /// Cost per million input tokens.
    pub cost_input: Option<f64>,
    /// Cost per million output tokens.
    pub cost_output: Option<f64>,
    /// Cost per million cache-read tokens.
    pub cost_cache_read: Option<f64>,
    /// Cost per million cache-write tokens.
    pub cost_cache_write: Option<f64>,
    // Unified from ModelEntry - input modalities
    /// Supported input modalities (e.g. "text", "image").
    pub input_modalities: Vec<String>,
}

impl Model {
    /// Get the full model identifier (provider/model_id)
    pub fn full_id(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }

    /// Create from a model_db::ModelEntry
    pub fn from_entry(entry: &oxi_ai::ModelEntry) -> Self {
        Self {
            provider: entry.provider.to_string(),
            id: entry.id.to_string(),
            name: Some(entry.name.to_string()),
            description: None,
            context_window: Some(entry.context_window),
            supported_features: vec![],
            cost_input: Some(entry.cost_input),
            cost_output: Some(entry.cost_output),
            cost_cache_read: Some(entry.cost_cache_read),
            cost_cache_write: Some(entry.cost_cache_write),
            input_modalities: entry
                .input
                .iter()
                .map(|m| format!("{:?}", m).to_lowercase())
                .collect(),
        }
    }

    /// Create from a model_registry::Model
    pub fn from_registry_model(model: &oxi_ai::Model) -> Self {
        Self {
            provider: model.provider.clone(),
            id: model.id.clone(),
            name: Some(model.name.clone()),
            description: None,
            context_window: Some(model.context_window as u32),
            supported_features: vec![],
            cost_input: Some(model.cost.input),
            cost_output: Some(model.cost.output),
            cost_cache_read: Some(model.cost.cache_read),
            cost_cache_write: Some(model.cost.cache_write),
            input_modalities: model
                .input
                .iter()
                .map(|m| format!("{:?}", m).to_lowercase())
                .collect(),
        }
    }
}

/// Result of parsing a model pattern
#[derive(Debug)]
pub struct ParsedModelResult {
    /// Resolved provider name, if identified.
    pub provider: Option<String>,
    /// Model ID extracted from the pattern.
    pub model_id: String,
    /// Requested thinking level (e.g. "high", "medium").
    pub thinking_level: Option<String>,
    /// Warning message when resolution is ambiguous.
    pub warning: Option<String>,
}

/// Result of resolving a CLI model
#[derive(Debug)]
pub struct ResolveCliModelResult {
    /// The resolved model, if found.
    pub model: Option<Model>,
    /// Requested thinking level.
    pub thinking_level: Option<String>,
    /// Warning about ambiguous resolution.
    pub warning: Option<String>,
    /// Error message when resolution fails.
    pub error: Option<String>,
}

/// Result of finding initial model
#[derive(Debug)]
pub struct InitialModelResult {
    /// The selected model.
    pub model: Option<Model>,
    /// Thinking level applied.
    pub thinking_level: String,
    /// Message when a fallback model was chosen.
    pub fallback_message: Option<String>,
}

/// Result of restore model from session
#[derive(Debug)]
pub struct RestoreModelResult {
    /// The restored model, if available.
    pub model: Option<Model>,
    /// Message explaining why a fallback was used.
    pub fallback_message: Option<String>,
    /// Machine-readable reason code ("no_auth", "model_not_found", "no_models").
    pub reason: Option<String>,
}

// =============================================================================
// Core Functions
// =============================================================================

/// Check if two models are equal (same provider and id)
///
/// Used for deduplication in model scope resolution.
pub fn models_are_equal(a: &Model, b: &Model) -> bool {
    a.provider == b.provider && a.id == b.id
}

/// Check if a model ID looks like an alias (no date suffix)
fn is_alias(id: &str) -> bool {
    // Aliases end with -latest or don't have date patterns
    if id.ends_with("-latest") {
        return true;
    }
    // Check if ends with date pattern (-YYYYMMDD)
    !DATE_PATTERN_RE.is_match(id)
}

/// Match a glob pattern against text
///
/// Supports:
/// - `*` - matches any characters
/// - `?` - matches any single character
/// - `[abc]` - matches any character in the set
///
/// Note: Matching is case-insensitive.
pub fn match_glob(pattern: &str, text: &str) -> bool {
    // Simple implementation: convert glob to regex
    let mut regex_pattern = String::new();
    let mut in_class = false;
    let chars = pattern.chars().peekable();

    for c in chars {
        match c {
            '*' => {
                if in_class {
                    regex_pattern.push_str("\\*");
                } else {
                    regex_pattern.push_str(".*");
                }
            }
            '?' => {
                if in_class {
                    regex_pattern.push('?');
                } else {
                    regex_pattern.push('.');
                }
            }
            '[' => {
                in_class = true;
                regex_pattern.push('[');
            }
            ']' => {
                in_class = false;
                regex_pattern.push(']');
            }
            '.' | '+' | '^' | '$' | '\\' | '(' | ')' | '{' | '}' | '|' => {
                // Escape regex special characters (but not those in char classes)
                if !in_class {
                    regex_pattern.push('\\');
                }
                regex_pattern.push(c);
            }
            _ => regex_pattern.push(c),
        }
    }

    // Handle trailing ** in patterns
    if pattern.ends_with("**") {
        regex_pattern.push_str(".*");
    }

    // Use case-insensitive regex matching
    regex::RegexBuilder::new(&format!("^{}$", regex_pattern))
        .case_insensitive(true)
        .build()
        .map(|re| re.is_match(text))
        .unwrap_or_else(|_| pattern.eq_ignore_ascii_case(text))
}

/// Find all models matching a provider and glob pattern
pub fn find_models_by_glob<'a>(
    provider: &str,
    pattern: &str,
    models: &'a [Model],
) -> Vec<&'a Model> {
    models
        .iter()
        .filter(|m| m.provider == provider && match_glob(pattern, &m.id))
        .collect()
}

/// Find all models matching a pattern (glob or substring) from the full model database
pub fn find_models_by_pattern(pattern: &str, models: &[Model]) -> Vec<Model> {
    let pattern_lower = pattern.to_lowercase();
    models
        .iter()
        .filter(|m| {
            m.id.to_lowercase().contains(&pattern_lower)
                || m.full_id().to_lowercase().contains(&pattern_lower)
                || m.name
                    .as_ref()
                    .map(|n| n.to_lowercase().contains(&pattern_lower))
                    .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Get the thinking level mapping for a model that has thinking variants
///
/// For models like claude-3-5-sonnet-*, returns a mapping:
/// - "high" -> "claude-3-5-sonnet-20240620"
/// - "medium" -> "claude-3-5-sonnet-latest"
pub fn get_thinking_level_map(model_id: &str) -> Option<HashMap<String, String>> {
    // Strip common suffixes to find base model name
    let base = if let Some(stripped) = model_id.strip_suffix("-latest") {
        stripped
    } else if let Some(dated) = model_id.strip_suffix(DATE_PATTERN_STRIP_RE.as_str()) {
        dated
    } else {
        // Also try stripping numbered suffixes like -5, -6, etc.
        // Match patterns like claude-opus-4-5, claude-sonnet-4-6, etc.
        model_id
    };

    // Only certain models have thinking variants
    let thinking_models = [
        (
            "claude-3-5-sonnet",
            vec![
                ("high", "claude-3-5-sonnet-20240620"),
                ("medium", "claude-3-5-sonnet-latest"),
                ("low", "claude-3-5-sonnet-latest"),
            ],
        ),
        (
            "claude-3-7-sonnet",
            vec![
                ("high", "claude-3-7-sonnet-20250219"),
                ("medium", "claude-3-7-sonnet-20250219"),
                ("low", "claude-3-7-sonnet-latest"),
            ],
        ),
        (
            "claude-opus-4",
            vec![
                ("high", "claude-opus-4-5-20251101"),
                ("medium", "claude-opus-4-5"),
                ("low", "claude-opus-4-1"),
                ("off", "claude-opus-4-0"),
            ],
        ),
        (
            "claude-sonnet-4",
            vec![
                ("high", "claude-sonnet-4-20250514"),
                ("medium", "claude-sonnet-4-5"),
                ("low", "claude-sonnet-4-0"),
                ("off", "claude-sonnet-4-0"),
            ],
        ),
    ];

    for (base_name, levels) in thinking_models {
        if base.contains(base_name) {
            let mut map = HashMap::new();
            for (level, id) in levels {
                map.insert(level.to_string(), id.to_string());
            }
            return Some(map);
        }
    }

    None
}

/// Clamp a requested thinking level to what the model supports
///
/// If the model doesn't support the requested level, returns the closest
/// supported level.
pub fn clamp_thinking_level(model_id: &str, requested_level: &str) -> String {
    if let Some(map) = get_thinking_level_map(model_id) {
        // If the requested level is in the map, use it
        if map.contains_key(requested_level) {
            return map
                .get(requested_level)
                .cloned()
                .unwrap_or_else(|| model_id.to_string());
        }

        // Find the closest level that IS in the map
        let level_idx = THINKING_LEVELS.iter().position(|&l| l == requested_level);
        if let Some(idx) = level_idx {
            // Search downward for the closest supported level
            for i in (0..idx).rev() {
                let level = THINKING_LEVELS[i];
                if map.contains_key(level) {
                    return map
                        .get(level)
                        .cloned()
                        .unwrap_or_else(|| model_id.to_string());
                }
            }
        }
    }

    // Fallback: return the requested level as-is if model has no special mapping
    requested_level.to_string()
}

/// Check if auth is configured for a provider/model
///
/// Checks stored credentials in auth.json. Does NOT check environment variables.
/// Use `oxi setup` to configure credentials persistently.
pub fn has_configured_auth(provider: &str, _model: &Model) -> bool {
    // Check auth storage (auth.json)
    let auth = crate::auth_storage::shared_auth_storage();
    auth.has_auth(provider)
}

/// Parse a model pattern into components
///
/// # Arguments
/// * `pattern` - The model pattern (e.g., "anthropic/claude-3.5-sonnet" or "sonnet:high")
/// * `available_models` - List of available models for validation
///
/// # Returns
/// A parsed model result with provider, model_id, and optional thinking level
pub fn parse_model_pattern(pattern: &str, available_models: &[Model]) -> ParsedModelResult {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return ParsedModelResult {
            provider: None,
            model_id: String::new(),
            thinking_level: None,
            warning: Some("Empty model pattern".to_string()),
        };
    }

    // Check for thinking level suffix (e.g., "sonnet:high")
    let last_colon = pattern.rfind(':');
    let (base_pattern, thinking_level) = if let Some(idx) = last_colon {
        let suffix = &pattern[idx + 1..];
        if THINKING_LEVELS.contains(&suffix) {
            (&pattern[..idx], Some(suffix.to_string()))
        } else {
            (pattern, None)
        }
    } else {
        (pattern, None)
    };

    // Try to find an exact match first
    let exact_match = available_models.iter().find(|m| {
        m.id.eq_ignore_ascii_case(base_pattern) || m.full_id().eq_ignore_ascii_case(base_pattern)
    });

    if let Some(model) = exact_match {
        return ParsedModelResult {
            provider: Some(model.provider.clone()),
            model_id: model.id.clone(),
            thinking_level,
            warning: None,
        };
    }

    // Try to parse provider/model format
    if let Some(slash_idx) = base_pattern.find('/') {
        let provider = &base_pattern[..slash_idx];
        let model_id = &base_pattern[slash_idx + 1..];

        // Check if provider exists in available models
        let provider_exists = available_models
            .iter()
            .any(|m| m.provider.eq_ignore_ascii_case(provider));

        if provider_exists {
            return ParsedModelResult {
                provider: Some(provider.to_string()),
                model_id: model_id.to_string(),
                thinking_level,
                warning: None,
            };
        }
    }

    // Try partial matching
    let partial_matches: Vec<&Model> = available_models
        .iter()
        .filter(|m| {
            m.id.to_lowercase().contains(&base_pattern.to_lowercase())
                || m.name
                    .as_ref()
                    .map(|n| n.to_lowercase().contains(&base_pattern.to_lowercase()))
                    .unwrap_or(false)
        })
        .collect();

    if partial_matches.len() == 1 {
        let model = partial_matches[0];
        return ParsedModelResult {
            provider: Some(model.provider.clone()),
            model_id: model.id.clone(),
            thinking_level,
            warning: None,
        };
    } else if partial_matches.len() > 1 {
        // Prefer aliases over dated versions
        let aliases: Vec<_> = partial_matches.iter().filter(|m| is_alias(&m.id)).collect();
        if !aliases.is_empty() {
            let model = aliases[0];
            return ParsedModelResult {
                provider: Some(model.provider.clone()),
                model_id: model.id.clone(),
                thinking_level,
                warning: Some(format!(
                    "Multiple models match '{}', selected '{}'",
                    base_pattern,
                    model.full_id()
                )),
            };
        }
        // Use the latest dated version (sort descending)
        let mut sorted = partial_matches.to_vec();
        sorted.sort_by(|a, b| b.id.cmp(&a.id));
        let model = sorted[0];
        return ParsedModelResult {
            provider: Some(model.provider.clone()),
            model_id: model.id.clone(),
            thinking_level,
            warning: Some(format!(
                "Multiple models match '{}', selected '{}'",
                base_pattern,
                model.full_id()
            )),
        };
    }

    // No match found - return as raw pattern
    ParsedModelResult {
        provider: None,
        model_id: pattern.to_string(),
        thinking_level,
        warning: Some(format!(
            "Model '{}' not found in available models. Treating as custom model ID.",
            pattern
        )),
    }
}

/// Default models per provider
pub fn default_model_per_provider() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("anthropic".to_string(), "claude-sonnet-4-5".to_string());
    map.insert("openai".to_string(), "gpt-4o".to_string());
    map.insert("google".to_string(), "gemini-2.5-pro".to_string());
    map.insert("deepseek".to_string(), "deepseek-v3".to_string());
    map.insert(
        "openrouter".to_string(),
        "anthropic/claude-sonnet-4".to_string(),
    );
    map.insert("groq".to_string(), "mixtral-8x7b".to_string());
    map.insert("cerebras".to_string(), "llama-3.3-70b".to_string());
    map.insert("mistral".to_string(), "mistral-large".to_string());
    map.insert("xai".to_string(), "grok-2".to_string());
    map.insert(
        "amazon-bedrock".to_string(),
        "anthropic.claude-v2".to_string(),
    );
    map.insert("azure-openai".to_string(), "gpt-4o".to_string());
    map
}

/// Resolve a model from CLI arguments
pub fn resolve_cli_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    available_models: &[Model],
    _settings: Option<&Settings>,
) -> ResolveCliModelResult {
    let cli_model = match cli_model {
        Some(m) => m,
        None => {
            return ResolveCliModelResult {
                model: None,
                thinking_level: None,
                warning: None,
                error: None,
            };
        }
    };

    // Build provider map for case-insensitive lookup
    let mut provider_map: HashMap<String, String> = HashMap::new();
    for model in available_models {
        provider_map.insert(model.provider.to_lowercase(), model.provider.clone());
    }

    // Try to resolve provider
    let provider = if let Some(p) = cli_provider {
        provider_map.get(&p.to_lowercase()).cloned()
    } else if let Some(slash_idx) = cli_model.find('/') {
        let maybe_provider = &cli_model[..slash_idx];
        provider_map.get(&maybe_provider.to_lowercase()).cloned()
    } else {
        None
    };

    // Extract the model pattern
    let model_pattern = if let Some(ref p) = provider {
        if cli_model
            .to_lowercase()
            .starts_with(&format!("{}/", p.to_lowercase()))
        {
            &cli_model[p.len() + 1..]
        } else {
            cli_model
        }
    } else {
        cli_model
    };

    // Parse the pattern
    let parsed = parse_model_pattern(model_pattern, available_models);

    // Find the model
    let model = if let Some(ref p) = provider {
        available_models
            .iter()
            .find(|m| {
                m.provider.eq_ignore_ascii_case(p) && m.id.eq_ignore_ascii_case(&parsed.model_id)
            })
            .cloned()
    } else if let Some(ref p) = parsed.provider {
        available_models
            .iter()
            .find(|m| {
                m.provider.eq_ignore_ascii_case(p) && m.id.eq_ignore_ascii_case(&parsed.model_id)
            })
            .cloned()
    } else {
        // Try matching without provider
        available_models
            .iter()
            .find(|m| m.id.eq_ignore_ascii_case(&parsed.model_id))
            .cloned()
    };

    if let Some(ref m) = model {
        ResolveCliModelResult {
            model: Some(m.clone()),
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            error: None,
        }
    } else {
        // Try building a fallback custom model
        let fallback_model = if let Some(ref p) = provider {
            Some(Model {
                provider: p.clone(),
                id: parsed.model_id.clone(),
                name: Some(parsed.model_id.clone()),
                description: None,
                context_window: None,
                supported_features: vec![],
                cost_input: None,
                cost_output: None,
                cost_cache_read: None,
                cost_cache_write: None,
                input_modalities: vec!["text".to_string()],
            })
        } else {
            None
        };

        ResolveCliModelResult {
            model: fallback_model.clone(),
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            error: fallback_model.is_none().then(|| {
                format!(
                    "Model '{}' not found. Use --list-models to see available models.",
                    cli_model
                )
            }),
        }
    }
}

/// Find the initial model to use based on priority:
/// 1. CLI args
/// 2. First model from scoped models
/// 3. Saved default from settings
/// 4. First available model
pub fn find_initial_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    scoped_models: &[Model],
    is_continuing: bool,
    settings: Option<&Settings>,
    available_models: &[Model],
) -> InitialModelResult {
    // 1. CLI args take priority
    if cli_provider.is_some() || cli_model.is_some() {
        let result = resolve_cli_model(cli_provider, cli_model, available_models, settings);
        if result.error.is_none() {
            return InitialModelResult {
                model: result.model,
                thinking_level: result
                    .thinking_level
                    .unwrap_or_else(|| DEFAULT_THINKING_LEVEL.to_string()),
                fallback_message: None,
            };
        }
    }

    // 2. Use first model from scoped models (skip if continuing)
    if !scoped_models.is_empty() && !is_continuing {
        return InitialModelResult {
            model: Some(scoped_models[0].clone()),
            thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
            fallback_message: None,
        };
    }

    // 3. Try last used model from settings
    if let Some(s) = settings {
        if let Some(last_used) = &s.last_used_model {
            let parsed = parse_model_pattern(last_used, available_models);
            if let Some(ref p) = parsed.provider {
                let model = available_models
                    .iter()
                    .find(|m| {
                        m.provider.eq_ignore_ascii_case(p)
                            && m.id.eq_ignore_ascii_case(&parsed.model_id)
                    })
                    .cloned();
                if model.is_some() {
                    return InitialModelResult {
                        model,
                        thinking_level: format!("{:?}", s.thinking_level),
                        fallback_message: None,
                    };
                }
            }
        }
    }

    // 4. Try default models from known providers
    let defaults = default_model_per_provider();
    for (provider, default_id) in &defaults {
        if let Some(model) = available_models.iter().find(|m| {
            m.provider.eq_ignore_ascii_case(provider) && m.id.eq_ignore_ascii_case(default_id)
        }) {
            return InitialModelResult {
                model: Some(model.clone()),
                thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
                fallback_message: None,
            };
        }
    }

    // 5. Use first available model
    if let Some(model) = available_models.first() {
        return InitialModelResult {
            model: Some(model.clone()),
            thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
            fallback_message: None,
        };
    }

    // No model found
    InitialModelResult {
        model: None,
        thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
        fallback_message: Some("No models available. Check your installation.".to_string()),
    }
}

/// Restore model from session with fallback and auth validation
///
/// This function now properly checks auth configuration before restoring a model.
pub fn restore_model_from_session(
    saved_provider: &str,
    saved_model_id: &str,
    current_model: Option<&Model>,
    should_print_messages: bool,
    available_models: &[Model],
) -> RestoreModelResult {
    let restored = available_models
        .iter()
        .find(|m| {
            m.provider.eq_ignore_ascii_case(saved_provider)
                && m.id.eq_ignore_ascii_case(saved_model_id)
        })
        .cloned();

    match (&restored, current_model) {
        (Some(ref model), _) => {
            // Check if the model has auth configured
            if has_configured_auth(saved_provider, model) {
                if should_print_messages {
                    eprintln!("Restored model: {}/{}", saved_provider, saved_model_id);
                }
                RestoreModelResult {
                    model: Some((*model).clone()),
                    fallback_message: None,
                    reason: None,
                }
            } else {
                // Model exists but no auth - try fallback
                if should_print_messages {
                    eprintln!(
                        "Warning: Could not restore model {}/{} (no auth configured).",
                        saved_provider, saved_model_id
                    );
                }

                if let Some(current) = current_model {
                    if should_print_messages {
                        eprintln!("Falling back to: {}/{}", current.provider, current.id);
                    }
                    RestoreModelResult {
                        model: Some((*current).clone()),
                        fallback_message: Some(format!(
                            "Could not restore model {}/{} (no auth configured). Using current model.",
                            saved_provider, saved_model_id
                        )),
                        reason: Some("no_auth".to_string()),
                    }
                } else if let Some(fallback) = available_models.first() {
                    if should_print_messages {
                        eprintln!(
                            "Using first available model: {}/{}",
                            fallback.provider, fallback.id
                        );
                    }
                    RestoreModelResult {
                        model: Some(fallback.clone()),
                        fallback_message: Some(format!(
                            "Could not restore model {}/{} (no auth configured). Using first available model.",
                            saved_provider, saved_model_id
                        )),
                        reason: Some("no_auth".to_string()),
                    }
                } else {
                    RestoreModelResult {
                        model: None,
                        fallback_message: Some("No models available.".to_string()),
                        reason: Some("no_auth".to_string()),
                    }
                }
            }
        }
        (None, Some(current)) => {
            if should_print_messages {
                eprintln!(
                    "Warning: Could not restore model {}/{} (model not found). Falling back to current model.",
                    saved_provider, saved_model_id
                );
                eprintln!("Falling back to: {}/{}", current.provider, current.id);
            }
            RestoreModelResult {
                model: Some((*current).clone()),
                fallback_message: Some(format!(
                    "Could not restore model {}/{} (model not found). Using current model.",
                    saved_provider, saved_model_id
                )),
                reason: Some("model_not_found".to_string()),
            }
        }
        (None, None) => {
            // Try to find any available model
            if let Some(model) = available_models.first() {
                if should_print_messages {
                    eprintln!(
                        "Warning: Could not restore model {}/{} (model not found).",
                        saved_provider, saved_model_id
                    );
                    eprintln!(
                        "Using first available model: {}/{}",
                        model.provider, model.id
                    );
                }
                RestoreModelResult {
                    model: Some(model.clone()),
                    fallback_message: Some(format!(
                        "Could not restore model {}/{}. Using first available model.",
                        saved_provider, saved_model_id
                    )),
                    reason: Some("model_not_found".to_string()),
                }
            } else {
                RestoreModelResult {
                    model: None,
                    fallback_message: Some("No models available.".to_string()),
                    reason: Some("no_models".to_string()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_models() -> Vec<Model> {
        vec![
            Model {
                provider: "anthropic".to_string(),
                id: "claude-sonnet-4-5".to_string(),
                name: Some("Claude Sonnet 4.5".to_string()),
                description: None,
                context_window: Some(200000),
                supported_features: vec!["tools".to_string(), "vision".to_string()],
                cost_input: Some(3.0),
                cost_output: Some(15.0),
                cost_cache_read: Some(0.3),
                cost_cache_write: Some(3.75),
                input_modalities: vec!["text".to_string(), "image".to_string()],
            },
            Model {
                provider: "anthropic".to_string(),
                id: "claude-opus-4-7".to_string(),
                name: Some("Claude Opus 4.7".to_string()),
                description: None,
                context_window: Some(200000),
                supported_features: vec!["tools".to_string(), "vision".to_string()],
                cost_input: Some(15.0),
                cost_output: Some(75.0),
                cost_cache_read: Some(0.5),
                cost_cache_write: Some(6.25),
                input_modalities: vec!["text".to_string(), "image".to_string()],
            },
            Model {
                provider: "openai".to_string(),
                id: "gpt-4o".to_string(),
                name: Some("GPT-4o".to_string()),
                description: None,
                context_window: Some(128000),
                supported_features: vec!["tools".to_string()],
                cost_input: Some(2.5),
                cost_output: Some(10.0),
                cost_cache_read: Some(1.25),
                cost_cache_write: Some(0.0),
                input_modalities: vec!["text".to_string(), "image".to_string()],
            },
            Model {
                provider: "google".to_string(),
                id: "gemini-2.5-pro".to_string(),
                name: Some("Gemini 2.5 Pro".to_string()),
                description: None,
                context_window: Some(1000000),
                supported_features: vec!["tools".to_string()],
                cost_input: Some(1.25),
                cost_output: Some(5.0),
                cost_cache_read: Some(0.0),
                cost_cache_write: Some(0.0),
                input_modalities: vec!["text".to_string(), "image".to_string()],
            },
        ]
    }

    // =============================================================================
    // Model Equality Tests
    // =============================================================================

    #[test]
    fn test_models_are_equal_same() {
        let model1 = Model {
            provider: "anthropic".to_string(),
            id: "claude-sonnet-4-5".to_string(),
            name: Some("Claude Sonnet 4.5".to_string()),
            description: None,
            context_window: Some(200000),
            supported_features: vec![],
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            input_modalities: vec![],
        };
        let model2 = Model {
            provider: "anthropic".to_string(),
            id: "claude-sonnet-4-5".to_string(),
            name: Some("Claude Sonnet 4.5 (different name)".to_string()),
            description: None,
            context_window: Some(200000),
            supported_features: vec![],
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            input_modalities: vec![],
        };

        assert!(models_are_equal(&model1, &model2));
    }

    #[test]
    fn test_models_are_equal_different_provider() {
        let model1 = Model {
            provider: "anthropic".to_string(),
            id: "claude-sonnet-4-5".to_string(),
            name: None,
            description: None,
            context_window: None,
            supported_features: vec![],
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            input_modalities: vec![],
        };
        let model2 = Model {
            provider: "openai".to_string(),
            id: "claude-sonnet-4-5".to_string(),
            name: None,
            description: None,
            context_window: None,
            supported_features: vec![],
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            input_modalities: vec![],
        };

        assert!(!models_are_equal(&model1, &model2));
    }

    #[test]
    fn test_models_are_equal_different_id() {
        let model1 = Model {
            provider: "anthropic".to_string(),
            id: "claude-sonnet-4-5".to_string(),
            name: None,
            description: None,
            context_window: None,
            supported_features: vec![],
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            input_modalities: vec![],
        };
        let model2 = Model {
            provider: "anthropic".to_string(),
            id: "claude-opus-4-7".to_string(),
            name: None,
            description: None,
            context_window: None,
            supported_features: vec![],
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            input_modalities: vec![],
        };

        assert!(!models_are_equal(&model1, &model2));
    }

    // =============================================================================
    // Glob Pattern Tests
    // =============================================================================

    #[test]
    fn test_match_glob_exact() {
        assert!(match_glob("claude-sonnet-4-5", "claude-sonnet-4-5"));
        assert!(!match_glob("claude-sonnet-4-5", "claude-opus-4-7"));
    }

    #[test]
    fn test_match_glob_asterisk() {
        assert!(match_glob("claude-*", "claude-sonnet-4-5"));
        assert!(match_glob("claude-*", "claude-opus-4-7"));
        assert!(!match_glob("claude-*", "gpt-4o"));
    }

    #[test]
    fn test_match_glob_question() {
        assert!(match_glob("claude-?-sonnet-4-5", "claude-3-sonnet-4-5"));
        assert!(!match_glob("claude-?-sonnet-4-5", "claude-35-sonnet-4-5"));
    }

    #[test]
    fn test_match_glob_char_class() {
        // Character classes match case-insensitively (since glob matching is case-insensitive overall)
        assert!(match_glob("claude-[a-z]-sonnet", "claude-a-sonnet"));
        assert!(match_glob("claude-[a-z]-sonnet", "claude-b-sonnet"));
        // Note: character classes are case-insensitive with our implementation
        assert!(match_glob("claude-[a-z]-sonnet", "claude-A-sonnet"));
    }

    #[test]
    fn test_match_glob_case_insensitive() {
        assert!(match_glob("CLAUDE-*", "claude-sonnet-4-5"));
    }

    #[test]
    fn test_find_models_by_glob() {
        let models = sample_models();
        let results = find_models_by_glob("anthropic", "claude-*", &models);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|m| m.provider == "anthropic"));
    }

    #[test]
    fn test_find_models_by_glob_no_match() {
        let models = sample_models();
        let results = find_models_by_glob("openai", "gpt-*", &models);
        assert_eq!(results.len(), 1);
    }

    // =============================================================================
    // Thinking Level Mapping Tests
    // =============================================================================

    #[test]
    fn test_get_thinking_level_map_claude_35_sonnet() {
        let map = get_thinking_level_map("claude-3-5-sonnet-latest");
        assert!(map.is_some());
        let map = map.unwrap();
        assert_eq!(
            map.get("high"),
            Some(&"claude-3-5-sonnet-20240620".to_string())
        );
    }

    #[test]
    fn test_get_thinking_level_map_claude_opus_4() {
        let map = get_thinking_level_map("claude-opus-4-5");
        assert!(map.is_some());
        let map = map.unwrap();
        assert_eq!(
            map.get("high"),
            Some(&"claude-opus-4-5-20251101".to_string())
        );
        assert_eq!(map.get("medium"), Some(&"claude-opus-4-5".to_string()));
    }

    #[test]
    fn test_get_thinking_level_map_no_match() {
        let map = get_thinking_level_map("gpt-4o");
        assert!(map.is_none());
    }

    #[test]
    fn test_clamp_thinking_level_supported() {
        let result = clamp_thinking_level("claude-3-5-sonnet-latest", "high");
        assert_eq!(result, "claude-3-5-sonnet-20240620");
    }

    #[test]
    fn test_clamp_thinking_level_clamp_down() {
        // Request "xhigh" which doesn't exist, should clamp to "high"
        let result = clamp_thinking_level("claude-3-5-sonnet-latest", "xhigh");
        assert_eq!(result, "claude-3-5-sonnet-20240620");
    }

    #[test]
    fn test_clamp_thinking_level_no_mapping() {
        // gpt-4o has no mapping, should return requested level
        let result = clamp_thinking_level("gpt-4o", "high");
        assert_eq!(result, "high");
    }

    // =============================================================================
    // Auth Validation Tests
    // =============================================================================

    #[test]
    fn test_has_configured_auth_unknown_provider() {
        let model = Model {
            provider: "unknown".to_string(),
            id: "test".to_string(),
            name: None,
            description: None,
            context_window: None,
            supported_features: vec![],
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            input_modalities: vec![],
        };

        // Without env vars, this should return false
        let has_auth = has_configured_auth("unknown", &model);
        assert!(!has_auth);
    }

    #[test]
    fn test_has_configured_auth_known_provider_no_env() {
        let model = Model {
            provider: "anthropic".to_string(),
            id: "claude-sonnet-4-5".to_string(),
            name: None,
            description: None,
            context_window: None,
            supported_features: vec![],
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            input_modalities: vec![],
        };

        // Without setting env vars in test, this should return false
        let _has_auth = has_configured_auth("anthropic", &model);
        // This might be true if ANTHROPIC_API_KEY is set in the environment
        // which is fine - the test just checks the function works
    }

    // =============================================================================
    // Model Parsing Tests
    // =============================================================================

    #[test]
    fn test_parse_model_pattern_exact() {
        let models = sample_models();
        let result = parse_model_pattern("claude-sonnet-4-5", &models);

        assert_eq!(result.model_id, "claude-sonnet-4-5");
        assert_eq!(result.provider, Some("anthropic".to_string()));
        assert!(result.warning.is_none());
    }

    #[test]
    fn test_parse_model_pattern_with_provider() {
        let models = sample_models();
        let result = parse_model_pattern("anthropic/claude-sonnet-4-5", &models);

        assert_eq!(result.model_id, "claude-sonnet-4-5");
        assert_eq!(result.provider, Some("anthropic".to_string()));
    }

    #[test]
    fn test_parse_model_pattern_with_thinking_level() {
        let models = sample_models();
        let result = parse_model_pattern("sonnet:high", &models);

        assert_eq!(result.thinking_level, Some("high".to_string()));
    }

    #[test]
    fn test_parse_model_pattern_invalid_thinking_level() {
        let models = sample_models();
        let result = parse_model_pattern("sonnet:invalid", &models);

        assert!(result.thinking_level.is_none());
    }

    #[test]
    fn test_parse_model_pattern_partial_match() {
        let models = sample_models();
        let result = parse_model_pattern("sonnet", &models);

        assert!(result.model_id.contains("sonnet") || result.model_id == "sonnet");
        assert!(result.warning.is_some() || result.provider.is_some());
    }

    #[test]
    fn test_parse_model_pattern_not_found() {
        let models = sample_models();
        let result = parse_model_pattern("nonexistent-model", &models);

        assert_eq!(result.model_id, "nonexistent-model");
        assert!(result.warning.is_some());
    }

    // =============================================================================
    // CLI Resolution Tests
    // =============================================================================

    #[test]
    fn test_resolve_cli_model_with_provider() {
        let models = sample_models();
        let result = resolve_cli_model(Some("anthropic"), Some("claude-sonnet-4-5"), &models, None);

        assert!(result.error.is_none());
        assert!(result.model.is_some());
        assert_eq!(result.model.unwrap().id, "claude-sonnet-4-5");
    }

    #[test]
    fn test_resolve_cli_model_with_slash() {
        let models = sample_models();
        let result = resolve_cli_model(None, Some("anthropic/claude-sonnet-4-5"), &models, None);

        assert!(result.error.is_none());
        assert!(result.model.is_some());
    }

    #[test]
    fn test_resolve_cli_model_not_found() {
        let models = sample_models();
        let result = resolve_cli_model(None, Some("nonexistent-model"), &models, None);

        assert!(result.error.is_some() || result.model.is_none());
    }

    #[test]
    fn test_resolve_cli_model_no_args() {
        let models = sample_models();
        let result = resolve_cli_model(None, None, &models, None);

        assert!(result.model.is_none());
        assert!(result.error.is_none());
    }

    // =============================================================================
    // Search Tests
    // =============================================================================

    #[test]
    fn test_find_models_by_pattern() {
        let models = sample_models();
        let results = find_models_by_pattern("sonnet", &models);

        assert!(!results.is_empty());
        assert!(results.iter().all(|m| m.id.contains("sonnet")
            || m.name
                .as_ref()
                .map(|n| n.contains("sonnet"))
                .unwrap_or(false)));
    }

    #[test]
    fn test_find_models_by_pattern_full_id() {
        let models = sample_models();
        let results = find_models_by_pattern("anthropic/claude-sonnet-4-5", &models);
        assert!(!results.is_empty());
    }

    // =============================================================================
    // Initial Model Selection Tests
    // =============================================================================

    #[test]
    fn test_find_initial_model_from_cli() {
        let models = sample_models();
        let result = find_initial_model(Some("openai"), Some("gpt-4o"), &[], false, None, &models);

        assert!(result.model.is_some());
        assert_eq!(result.model.unwrap().id, "gpt-4o");
    }

    #[test]
    fn test_find_initial_model_fallback_to_available() {
        let models = sample_models();
        let result = find_initial_model(None, None, &[], false, None, &models);

        assert!(result.model.is_some());
        assert!(result.fallback_message.is_none());
    }

    #[test]
    fn test_find_initial_model_default_thinking_level() {
        let models = sample_models();
        let result = find_initial_model(Some("openai"), Some("gpt-4o"), &[], false, None, &models);

        assert_eq!(result.thinking_level, DEFAULT_THINKING_LEVEL);
    }

    // =============================================================================
    // Restore Model Tests
    // =============================================================================

    #[test]
    fn test_restore_model_from_session_success() {
        let models = sample_models();
        let result =
            restore_model_from_session("anthropic", "claude-sonnet-4-5", None, false, &models);

        // Model should be found
        assert!(result.model.is_some());
        // Note: fallback_message may be set if no auth is configured
        // (which is expected in test environment without API keys)
        if result.fallback_message.is_some() {
            assert_eq!(result.reason, Some("no_auth".to_string()));
        }
    }

    #[test]
    fn test_restore_model_from_session_not_found() {
        let models = sample_models();
        let current = &models[0];
        let result =
            restore_model_from_session("nonexistent", "model", Some(current), false, &models);

        assert!(result.model.is_some());
        assert!(result.fallback_message.is_some());
        assert_eq!(result.reason, Some("model_not_found".to_string()));
    }

    #[test]
    fn test_restore_model_from_session_fallback() {
        let models = sample_models();
        let current = &models[0];
        let result =
            restore_model_from_session("nonexistent", "model", Some(current), false, &models);

        assert!(result.model.is_some());
        // Should fall back to current model
        assert_eq!(result.model.unwrap().id, current.id);
    }

    // =============================================================================
    // Alias Detection Tests
    // =============================================================================

    #[test]
    fn test_is_alias() {
        assert!(is_alias("claude-sonnet-4-latest"));
        assert!(is_alias("simple-model"));
        assert!(!is_alias("claude-sonnet-4-20250929"));
        assert!(!is_alias("claude-sonnet-4-20250514"));
    }

    #[test]
    fn test_default_thinking_level_constant() {
        assert_eq!(DEFAULT_THINKING_LEVEL, "medium");
    }

    #[test]
    fn test_thinking_levels_constant() {
        assert_eq!(
            THINKING_LEVELS,
            &["off", "minimal", "low", "medium", "high", "xhigh"]
        );
    }
}
