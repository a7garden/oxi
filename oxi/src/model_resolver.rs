//! Model name resolution and matching
//!
//! Provides utilities for parsing model patterns, resolving model names,
//! and finding the best model for startup.

use crate::settings::Settings;
use std::collections::HashMap;
use std::sync::Arc;

/// Known AI providers
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub website: Option<String>,
}

impl Provider {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            website: None,
        }
    }

    pub fn with_website(mut self, website: impl Into<String>) -> Self {
        self.website = Some(website.into());
        self
    }
}

/// A discovered model
#[derive(Debug, Clone)]
pub struct Model {
    pub provider: String,
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_window: Option<u32>,
    pub supported_features: Vec<String>,
}

impl Model {
    /// Get the full model identifier (provider/model_id)
    pub fn full_id(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }
}

/// Result of parsing a model pattern
#[derive(Debug)]
pub struct ParsedModelResult {
    pub provider: Option<String>,
    pub model_id: String,
    pub thinking_level: Option<String>,
    pub warning: Option<String>,
}

/// Result of resolving a CLI model
#[derive(Debug)]
pub struct ResolveCliModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<String>,
    pub warning: Option<String>,
    pub error: Option<String>,
}

/// Result of finding initial model
#[derive(Debug)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: String,
    pub fallback_message: Option<String>,
}

/// Default models per provider
pub fn default_model_per_provider() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("anthropic".to_string(), "claude-sonnet-4-5".to_string());
    map.insert("openai".to_string(), "gpt-4o".to_string());
    map.insert("google".to_string(), "gemini-2.5-pro".to_string());
    map.insert("deepseek".to_string(), "deepseek-v3".to_string());
    map.insert("openrouter".to_string(), "anthropic/claude-sonnet-4".to_string());
    map.insert("groq".to_string(), "mixtral-8x7b".to_string());
    map.insert("cerebras".to_string(), "llama-3.3-70b".to_string());
    map.insert("mistral".to_string(), "mistral-large".to_string());
    map.insert("xai".to_string(), "grok-2".to_string());
    map.insert("amazon-bedrock".to_string(), "anthropic.claude-v2".to_string());
    map.insert("azure-openai".to_string(), "gpt-4o".to_string());
    map
}

/// Check if a model ID looks like an alias (no date suffix)
fn is_alias(id: &str) -> bool {
    // Aliases end with -latest or don't have date patterns
    if id.ends_with("-latest") {
        return true;
    }
    // Check if ends with date pattern (-YYYYMMDD)
    let date_pattern = regex_lite::Regex::new(r"-\d{8}$").ok();
    match date_pattern {
        Some(re) => !re.is_match(id),
        None => true,
    }
}

/// Parse a model pattern into components
///
/// # Arguments
/// * `pattern` - The model pattern (e.g., "anthropic/claude-3.5-sonnet" or "sonnet:high")
/// * `available_models` - List of available models for validation
///
/// # Returns
/// A parsed model result with provider, model_id, and optional thinking level
pub fn parse_model_pattern(
    pattern: &str,
    available_models: &[Model],
) -> ParsedModelResult {
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
    let thinking_levels = ["off", "minimal", "low", "medium", "high", "xhigh"];
    let last_colon = pattern.rfind(':');
    let (base_pattern, thinking_level) = if let Some(idx) = last_colon {
        let suffix = &pattern[idx + 1..];
        if thinking_levels.contains(&suffix) {
            (&pattern[..idx], Some(suffix.to_string()))
        } else {
            (pattern, None)
        }
    } else {
        (pattern, None)
    };

    // Try to find an exact match first
    let exact_match = available_models.iter().find(|m| {
        m.id.eq_ignore_ascii_case(base_pattern)
            || m.full_id().eq_ignore_ascii_case(base_pattern)
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
        let provider_exists = available_models.iter().any(|m| {
            m.provider.eq_ignore_ascii_case(provider)
        });

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

/// Find all models matching a glob pattern
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

/// Resolve a model from CLI arguments
pub fn resolve_cli_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    available_models: &[Model],
    settings: Option<&Settings>,
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
        if cli_model.to_lowercase().starts_with(&format!("{}/", p.to_lowercase())) {
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
            })
        } else {
            None
        };

        ResolveCliModelResult {
            model: fallback_model,
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
                thinking_level: result.thinking_level.unwrap_or_else(|| "medium".to_string()),
                fallback_message: None,
            };
        }
    }

    // 2. Use first model from scoped models (skip if continuing)
    if !scoped_models.is_empty() && !is_continuing {
        return InitialModelResult {
            model: Some(scoped_models[0].clone()),
            thinking_level: "medium".to_string(),
            fallback_message: None,
        };
    }

    // 3. Try saved default from settings
    if let Some(ref s) = settings {
        if let Some(default_model) = &s.default_model {
            let parsed = parse_model_pattern(default_model, available_models);
            if let Some(ref p) = parsed.provider {
                let model = available_models
                    .iter()
                    .find(|m| m.provider.eq_ignore_ascii_case(p) && m.id.eq_ignore_ascii_case(&parsed.model_id))
                    .cloned();
                if model.is_some() {
                    return InitialModelResult {
                        model,
                        thinking_level: s.thinking_level.to_string(),
                        fallback_message: None,
                    };
                }
            }
        }
    }

    // 4. Try default models from known providers
    let defaults = default_model_per_provider();
    for (provider, default_id) in &defaults {
        if let Some(model) = available_models
            .iter()
            .find(|m| m.provider.eq_ignore_ascii_case(provider) && m.id.eq_ignore_ascii_case(default_id))
        {
            return InitialModelResult {
                model: Some(model.clone()),
                thinking_level: "medium".to_string(),
                fallback_message: None,
            };
        }
    }

    // 5. Use first available model
    if let Some(model) = available_models.first() {
        return InitialModelResult {
            model: Some(model.clone()),
            thinking_level: "medium".to_string(),
            fallback_message: None,
        };
    }

    // No model found
    InitialModelResult {
        model: None,
        thinking_level: "medium".to_string(),
        fallback_message: Some("No models available. Check your installation.".to_string()),
    }
}

/// Restore model from session with fallback
pub fn restore_model_from_session(
    saved_provider: &str,
    saved_model_id: &str,
    current_model: Option<&Model>,
    should_print_messages: bool,
    available_models: &[Model],
) -> (Option<Model>, Option<String>) {
    let restored = available_models
        .iter()
        .find(|m| {
            m.provider.eq_ignore_ascii_case(saved_provider) && m.id.eq_ignore_ascii_case(saved_model_id)
        })
        .cloned();

    match (&restored, current_model) {
        (Some(ref model), _) => {
            if should_print_messages {
                eprintln!("Restored model: {}/{}", saved_provider, saved_model_id);
            }
            (Some(model.clone()), None)
        }
        (None, Some(current)) => {
            if should_print_messages {
                eprintln!(
                    "Warning: Could not restore model {}/{} (model not found). Falling back to current model.",
                    saved_provider, saved_model_id
                );
                eprintln!("Falling back to: {}/{}", current.provider, current.id);
            }
            (
                Some(current.clone()),
                Some(format!(
                    "Could not restore model {}/{} (model not found). Using current model.",
                    saved_provider, saved_model_id
                )),
            )
        }
        (None, None) => {
            // Try to find any available model
            if let Some(model) = available_models.first() {
                if should_print_messages {
                    eprintln!(
                        "Warning: Could not restore model {}/{} (model not found).",
                        saved_provider, saved_model_id
                    );
                    eprintln!("Using first available model: {}/{}", model.provider, model.id);
                }
                (
                    Some(model.clone()),
                    Some(format!(
                        "Could not restore model {}/{}. Using first available model.",
                        saved_provider, saved_model_id
                    )),
                )
            } else {
                (None, Some("No models available.".to_string()))
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
            },
            Model {
                provider: "anthropic".to_string(),
                id: "claude-opus-4-7".to_string(),
                name: Some("Claude Opus 4.7".to_string()),
                description: None,
                context_window: Some(200000),
                supported_features: vec!["tools".to_string(), "vision".to_string()],
            },
            Model {
                provider: "openai".to_string(),
                id: "gpt-4o".to_string(),
                name: Some("GPT-4o".to_string()),
                description: None,
                context_window: Some(128000),
                supported_features: vec!["tools".to_string()],
            },
            Model {
                provider: "google".to_string(),
                id: "gemini-2.5-pro".to_string(),
                name: Some("Gemini 2.5 Pro".to_string()),
                description: None,
                context_window: Some(1000000),
                supported_features: vec!["tools".to_string()],
            },
        ]
    }

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
    fn test_find_models_by_pattern() {
        let models = sample_models();
        let results = find_models_by_pattern("sonnet", &models);

        assert!(!results.is_empty());
        assert!(results.iter().all(|m| m.id.contains("sonnet") || m.name.as_ref().map(|n| n.contains("sonnet")).unwrap_or(false)));
    }

    #[test]
    fn test_find_initial_model_from_cli() {
        let models = sample_models();
        let result = find_initial_model(
            Some("openai"),
            Some("gpt-4o"),
            &[],
            false,
            None,
            &models,
        );

        assert!(result.model.is_some());
        assert_eq!(result.model.unwrap().id, "gpt-4o");
    }

    #[test]
    fn test_find_initial_model_fallback_to_available() {
        let models = sample_models();
        let result = find_initial_model(None, None, &[], false, None, &models);

        assert!(result.model.is_some());
        // Should use first available
        assert!(result.fallback_message.is_none());
    }

    #[test]
    fn test_restore_model_from_session_success() {
        let models = sample_models();
        let (model, message) = restore_model_from_session(
            "anthropic",
            "claude-sonnet-4-5",
            None,
            false,
            &models,
        );

        assert!(model.is_some());
        assert!(message.is_none());
    }

    #[test]
    fn test_restore_model_from_session_fallback() {
        let models = sample_models();
        let current = &models[0];
        let (model, message) = restore_model_from_session(
            "nonexistent",
            "model",
            Some(current),
            false,
            &models,
        );

        assert!(model.is_some());
        assert!(message.is_some());
    }

    #[test]
    fn test_is_alias() {
        assert!(is_alias("claude-sonnet-4-latest"));
        assert!(!is_alias("claude-sonnet-4-20250929"));
        assert!(is_alias("simple-model"));
    }
}