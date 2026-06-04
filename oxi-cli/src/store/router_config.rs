//! Router configuration loading for oxi-store.
//!
//! Reads the `[router]` section from global and project settings.toml
//! and merges them (project overrides global).

#![allow(missing_docs)]

use std::collections::HashMap;
use std::path::Path;

/// TOML representation of the router config section.
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct RouterConfigFile {
    pub enabled: Option<bool>,
    pub default_profile: Option<String>,
    pub classifier_model: Option<String>,
    pub context_upgrade_threshold: Option<usize>,
    pub max_session_budget: Option<f64>,
    pub profiles: Option<toml::Value>,
    pub weights: Option<toml::Value>,
    pub pin_tier: Option<String>,
    pub phase_bias: Option<f64>,
}

/// Fully resolved router config with all required fields.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RouterConfig {
    default_profile: String,
    classifier_model: Option<String>,
    context_upgrade_threshold: Option<usize>,
    max_session_budget: Option<f64>,
    profiles: HashMap<String, RouterProfile>,
    weights: ScoringWeights,
    pin_tier: Option<String>,
    phase_bias: Option<f64>,
}

impl RouterConfig {
    /// Returns whether router is enabled (default: true).
    pub fn enabled(&self) -> Option<bool> {
        Some(!self.profiles.is_empty())
    }

    /// Get the default profile name.
    pub fn default_profile(&self) -> &str {
        &self.default_profile
    }

    /// Get a profile by name.
    pub fn get_profile(&self, name: &str) -> Option<&RouterProfile> {
        self.profiles.get(name)
    }

    /// Get all profiles map.
    pub fn profiles(&self) -> &HashMap<String, RouterProfile> {
        &self.profiles
    }

    /// Get scoring weights.
    pub fn weights(&self) -> &ScoringWeights {
        &self.weights
    }

    /// Get the classifier model.
    pub fn classifier_model(&self) -> Option<&str> {
        self.classifier_model.as_deref()
    }

    /// Get context upgrade threshold.
    pub fn context_upgrade_threshold(&self) -> Option<usize> {
        self.context_upgrade_threshold
    }

    /// Get max session budget.
    pub fn max_session_budget(&self) -> Option<f64> {
        self.max_session_budget
    }

    /// Get pinned tier as string ("high", "medium", "low").
    pub fn pin_tier(&self) -> Option<&str> {
        self.pin_tier.as_deref()
    }

    /// Get phase bias.
    pub fn phase_bias(&self) -> Option<f64> {
        self.phase_bias
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouterProfile {
    pub high: RoutedTierConfig,
    pub medium: RoutedTierConfig,
    pub low: RoutedTierConfig,
}

impl RouterProfile {
    /// Get tier config by tier name.
    pub fn tier_config(&self, tier: &str) -> Option<&RoutedTierConfig> {
        match tier {
            "high" => Some(&self.high),
            "medium" => Some(&self.medium),
            "low" => Some(&self.low),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutedTierConfig {
    pub model: String,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScoringWeights {
    #[serde(default = "default_structural")]
    pub structural: f64,
    #[serde(default = "default_behavioral")]
    pub behavioral: f64,
    #[serde(default = "default_context")]
    pub context_budget: f64,
    #[serde(default = "default_vision")]
    pub vision: f64,
    #[serde(default = "default_message")]
    pub message: f64,
}

fn default_structural() -> f64 {
    0.25
}
fn default_behavioral() -> f64 {
    0.20
}
fn default_context() -> f64 {
    0.15
}
fn default_vision() -> f64 {
    0.10
}
fn default_message() -> f64 {
    0.30
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            structural: default_structural(),
            behavioral: default_behavioral(),
            context_budget: default_context(),
            vision: default_vision(),
            message: default_message(),
        }
    }
}

/// Load router config from global and project settings directories.
/// Returns `None` if no router config is found in either file.
pub fn load_router_config(global_dir: &Path, project_dir: &Path) -> Option<RouterConfig> {
    let global_path = global_dir.join("settings.toml");
    let project_path = project_dir.join(".oxi/settings.toml");

    let global_cfg = read_toml_router(&global_path);
    let project_cfg = read_toml_router(&project_path);

    let base_enabled = global_cfg.as_ref().and_then(|c| c.enabled);
    let override_enabled = project_cfg.as_ref().and_then(|c| c.enabled);

    // If neither file has `enabled = true`, skip router.
    if base_enabled != Some(true) && override_enabled != Some(true) {
        return None;
    }

    let default_name = project_cfg
        .as_ref()
        .and_then(|c| c.default_profile.clone())
        .or_else(|| global_cfg.as_ref().and_then(|c| c.default_profile.clone()))
        .unwrap_or_else(|| "auto".to_string());

    let mut profiles: HashMap<String, RouterProfile> = HashMap::new();

    // Merge global profiles.
    if let Some(ref g) = global_cfg {
        if let Some(ref tbl) = g.profiles {
            if let Some(inner) = tbl.as_table() {
                for (name, value) in inner {
                    if let Some(profile) = parse_profile(value) {
                        profiles.insert(name.clone(), profile);
                    }
                }
            }
        }
    }

    // Merge/override project profiles.
    if let Some(ref p) = project_cfg {
        if let Some(ref tbl) = p.profiles {
            if let Some(inner) = tbl.as_table() {
                for (name, value) in inner {
                    if let Some(profile) = parse_profile(value) {
                        profiles.insert(name.clone(), profile);
                    }
                }
            }
        }
    }

    if profiles.is_empty() {
        return None;
    }

    let weights = project_cfg
        .as_ref()
        .and_then(|c| c.weights.as_ref())
        .and_then(parse_weights)
        .or_else(|| {
            global_cfg
                .as_ref()
                .and_then(|c| c.weights.as_ref())
                .and_then(parse_weights)
        })
        .unwrap_or_default();

    let pin_tier = project_cfg
        .as_ref()
        .and_then(|c| c.pin_tier.as_ref())
        .or_else(|| global_cfg.as_ref().and_then(|c| c.pin_tier.as_ref()))
        .and_then(|s| parse_tier_str(s));

    let phase_bias = project_cfg
        .as_ref()
        .and_then(|c| c.phase_bias)
        .or_else(|| global_cfg.as_ref().and_then(|c| c.phase_bias));

    Some(RouterConfig {
        default_profile: default_name,
        classifier_model: project_cfg
            .as_ref()
            .and_then(|c| c.classifier_model.clone())
            .or_else(|| global_cfg.as_ref().and_then(|c| c.classifier_model.clone())),
        context_upgrade_threshold: project_cfg
            .as_ref()
            .and_then(|c| c.context_upgrade_threshold)
            .or_else(|| {
                global_cfg
                    .as_ref()
                    .and_then(|c| c.context_upgrade_threshold)
            }),
        max_session_budget: project_cfg
            .as_ref()
            .and_then(|c| c.max_session_budget)
            .or_else(|| global_cfg.as_ref().and_then(|c| c.max_session_budget)),
        profiles,
        weights,
        pin_tier,
        phase_bias,
    })
}

fn read_toml_router(path: &Path) -> Option<RouterConfigFile> {
    let content = std::fs::read_to_string(path).ok()?;
    let toml: toml::Value = toml::from_str(&content).ok()?;
    toml.get("router")?.clone().try_into().ok()
}

fn parse_profile(value: &toml::Value) -> Option<RouterProfile> {
    let table = value.as_table()?;
    Some(RouterProfile {
        high: parse_tier(table.get("high"))?,
        medium: parse_tier(table.get("medium"))?,
        low: parse_tier(table.get("low"))?,
    })
}

fn parse_tier(value: Option<&toml::Value>) -> Option<RoutedTierConfig> {
    let table = value?.as_table()?;
    Some(RoutedTierConfig {
        model: table.get("model")?.as_str()?.to_string(),
        thinking: table
            .get("thinking")
            .and_then(|v| v.as_str().map(String::from)),
        fallbacks: table
            .get("fallbacks")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn parse_weights(value: &toml::Value) -> Option<ScoringWeights> {
    let table = value.as_table()?;
    Some(ScoringWeights {
        structural: table.get("structural")?.as_float().unwrap_or(0.25),
        behavioral: table.get("behavioral")?.as_float().unwrap_or(0.20),
        context_budget: table.get("context_budget")?.as_float().unwrap_or(0.15),
        vision: table
            .get("vision")
            .and_then(|v| v.as_float())
            .unwrap_or(0.10),
        message: table
            .get("message")
            .and_then(|v| v.as_float())
            .unwrap_or(0.30),
    })
}

fn parse_tier_str(s: &str) -> Option<String> {
    match s.to_lowercase().as_str() {
        "high" | "medium" | "low" => Some(s.to_lowercase()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml_str = r#"
[router]
enabled = true
default_profile = "auto"

[router.profiles.auto]
high.model = "anthropic/claude-sonnet-4"
medium.model = "anthropic/claude-sonnet-4"
low.model = "google/gemini-2.0-flash"
"#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let cfg: RouterConfigFile = value.get("router").unwrap().clone().try_into().unwrap();
        assert!(cfg.enabled.is_some());
        assert_eq!(cfg.default_profile.as_ref().unwrap(), "auto");
    }

    #[test]
    fn returns_none_when_not_enabled() {
        let toml_str = r#"
[other]
value = 1
"#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let cfg: Option<RouterConfigFile> =
            value.get("router").and_then(|v| v.clone().try_into().ok());
        assert!(cfg.is_none());
    }

    #[test]
    fn load_router_config_merges_profiles() {
        let global_dir = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let oxi_dir = project_dir.path().join(".oxi");
        std::fs::create_dir_all(&oxi_dir).unwrap();

        std::fs::write(
            global_dir.path().join("settings.toml"),
            r#"
[router]
enabled = true
default_profile = "auto"

[router.profiles.auto]
high.model = "anthropic/claude-sonnet-4"
medium.model = "anthropic/claude-haiku-4"
low.model = "google/gemini-2.0-flash"
"#,
        )
        .unwrap();
        std::fs::write(
            oxi_dir.join("settings.toml"),
            r#"
[router]
enabled = true
"#,
        )
        .unwrap();

        let config = load_router_config(global_dir.path(), project_dir.path());
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.default_profile, "auto");
        assert!(config.profiles.contains_key("auto"));

        // tempdir::TempDir drops clean up automatically
    }
}
