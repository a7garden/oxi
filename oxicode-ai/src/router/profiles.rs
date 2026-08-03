//! Profile management for the router.

#![allow(missing_docs)]

use super::types::{RoutedTierConfig, RouterConfig, RouterProfile, RouterTier};
use std::collections::HashMap;

/// Indexed collection of routing profiles loaded from config.
#[derive(Debug, Clone)]
pub struct RouterProfiles {
    profiles: HashMap<String, RouterProfile>,
    default_name: String,
}

impl RouterProfiles {
    /// Build a profile index from a [`RouterConfig`].
    pub fn from_config(config: &RouterConfig) -> Self {
        Self {
            profiles: config.profiles.clone(),
            default_name: config.default_profile.clone(),
        }
    }

    /// Get a profile by exact name (no fallback).
    pub fn get(&self, name: &str) -> Option<&RouterProfile> {
        self.profiles.get(name)
    }

    /// Get a profile by name, falling back to the default profile.
    pub fn get_with_fallback(&self, name: &str) -> Option<&RouterProfile> {
        self.profiles
            .get(name)
            .or_else(|| self.profiles.get(&self.default_name))
    }

    /// Get the default profile.
    pub fn default_profile(&self) -> Option<&RouterProfile> {
        self.profiles.get(&self.default_name)
    }

    /// Get the [`RoutedTierConfig`] for a specific profile and tier.
    pub fn tier_config(&self, profile_name: &str, tier: RouterTier) -> Option<&RoutedTierConfig> {
        self.get_with_fallback(profile_name)
            .map(|p| p.tier_config(tier))
    }

    /// List all profile names.
    pub fn profile_names(&self) -> Vec<&str> {
        self.profiles.keys().map(|s| s.as_str()).collect()
    }
}

/// Parsed `"provider/model-id"` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModel {
    /// Provider name (e.g. `"anthropic"`).
    pub provider: String,
    /// Model identifier (e.g. `"claude-sonnet-4"`).
    pub model_id: String,
}

impl ProviderModel {
    /// Parse a `"provider/model-id"` string.
    pub fn parse(s: &str) -> Option<Self> {
        let (provider, model_id) = s.split_once('/')?;
        let provider = provider.trim().to_string();
        let model_id = model_id.trim().to_string();
        if provider.is_empty() || model_id.is_empty() {
            return None;
        }
        Some(Self { provider, model_id })
    }
}

/// Parse the model field from a [`RoutedTierConfig`].
pub fn parse_tier_model(config: &RoutedTierConfig) -> Option<ProviderModel> {
    ProviderModel::parse(&config.model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ThinkingLevel;

    fn make_config() -> RouterConfig {
        let mut config = RouterConfig::default();
        config.profiles.insert(
            "auto".to_string(),
            RouterProfile {
                high: RoutedTierConfig {
                    model: "anthropic/claude-sonnet-4".to_string(),
                    thinking: Some(ThinkingLevel::High),
                    fallbacks: vec!["openai/gpt-4o".to_string()],
                },
                medium: RoutedTierConfig {
                    model: "anthropic/claude-haiku-4".to_string(),
                    thinking: None,
                    fallbacks: vec![],
                },
                low: RoutedTierConfig {
                    model: "google/gemini-2.0-flash".to_string(),
                    thinking: None,
                    fallbacks: vec![],
                },
            },
        );
        config
    }

    #[test]
    fn profiles_exact_lookup() {
        let profiles = RouterProfiles::from_config(&make_config());
        assert!(profiles.get("auto").is_some());
        assert!(profiles.get("nonexistent").is_none());
    }

    #[test]
    fn profiles_fallback_lookup() {
        let profiles = RouterProfiles::from_config(&make_config());
        assert!(profiles.get_with_fallback("nonexistent").is_some());
    }

    #[test]
    fn default_profile() {
        let profiles = RouterProfiles::from_config(&make_config());
        assert!(profiles.default_profile().is_some());
    }

    #[test]
    fn tier_config_lookup() {
        let profiles = RouterProfiles::from_config(&make_config());
        let tc = profiles.tier_config("auto", RouterTier::High).unwrap();
        assert_eq!(tc.model, "anthropic/claude-sonnet-4");
    }

    #[test]
    fn parse_provider_model() {
        let pm = ProviderModel::parse("anthropic/claude-sonnet-4").unwrap();
        assert_eq!(pm.provider, "anthropic");
        assert_eq!(pm.model_id, "claude-sonnet-4");
    }

    #[test]
    fn parse_provider_model_no_slash() {
        assert!(ProviderModel::parse("just-a-model").is_none());
    }

    #[test]
    fn parse_provider_model_empty() {
        assert!(ProviderModel::parse("/model").is_none());
        assert!(ProviderModel::parse("provider/").is_none());
    }

    #[test]
    fn profile_names() {
        let profiles = RouterProfiles::from_config(&make_config());
        let names = profiles.profile_names();
        assert!(names.contains(&"auto"));
    }
}
