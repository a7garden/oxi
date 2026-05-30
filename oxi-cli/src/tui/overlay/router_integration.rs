//! Router setup integration — bridges TUI setup overlay to settings persistence.

use std::collections::HashMap;

use super::router_setup::RouterSetupData;

fn parse_ai_thinking(level: &str) -> Option<oxi_ai::ThinkingLevel> {
    match level {
        "off" => Some(oxi_ai::ThinkingLevel::Off),
        "minimal" => Some(oxi_ai::ThinkingLevel::Minimal),
        "low" => Some(oxi_ai::ThinkingLevel::Low),
        "medium" => Some(oxi_ai::ThinkingLevel::Medium),
        "high" => Some(oxi_ai::ThinkingLevel::High),
        "xhigh" => Some(oxi_ai::ThinkingLevel::XHigh),
        _ => None,
    }
}

fn thinking_to_ai(opt: &Option<String>) -> Option<oxi_ai::ThinkingLevel> {
    opt.as_ref().and_then(|s| parse_ai_thinking(s))
}

/// Convert a store [`RouterConfig`] to an AI [`RouterConfig`].
pub fn store_config_to_ai_config(
    c: &oxi_store::router_config::RouterConfig,
) -> oxi_ai::router::RouterConfig {
    let mut ai_profiles: HashMap<String, oxi_ai::router::RouterProfile> = HashMap::new();
    for (name, sp) in c.profiles() {
        ai_profiles.insert(
            name.clone(),
            oxi_ai::router::RouterProfile {
                high: oxi_ai::router::RoutedTierConfig {
                    model: sp.high.model.clone(),
                    thinking: thinking_to_ai(&sp.high.thinking),
                    fallbacks: sp.high.fallbacks.clone(),
                },
                medium: oxi_ai::router::RoutedTierConfig {
                    model: sp.medium.model.clone(),
                    thinking: thinking_to_ai(&sp.medium.thinking),
                    fallbacks: sp.medium.fallbacks.clone(),
                },
                low: oxi_ai::router::RoutedTierConfig {
                    model: sp.low.model.clone(),
                    thinking: thinking_to_ai(&sp.low.thinking),
                    fallbacks: sp.low.fallbacks.clone(),
                },
            },
        );
    }
    oxi_ai::router::RouterConfig::with_pinning(
        c.default_profile().to_string(),
        c.classifier_model().map(String::from),
        c.context_upgrade_threshold(),
        c.max_session_budget(),
        ai_profiles,
        oxi_ai::router::ScoringWeights {
            structural: c.weights().structural,
            behavioral: c.weights().behavioral,
            context_budget: c.weights().context_budget,
            vision: c.weights().vision,
            message: c.weights().message,
        },
        c.pin_tier().and_then(|s| match s {
            "high" => Some(oxi_ai::router::RouterTier::High),
            "medium" => Some(oxi_ai::router::RouterTier::Medium),
            "low" => Some(oxi_ai::router::RouterTier::Low),
            _ => None,
        }),
        c.phase_bias(),
    )
}

/// Convert [`RouterSetupData`] to a store [`RouterConfig`] and write it to settings.toml.
pub fn save_router_config(
    data: &RouterSetupData,
) -> Result<oxi_store::router_config::RouterConfig, String> {
    let profile_name = if data.profile_name.is_empty() {
        "auto"
    } else {
        &data.profile_name
    };

    let dir = dirs::config_dir().unwrap_or_default().join("oxi");
    let path = dir.join("settings.toml");
    let mut content = std::fs::read_to_string(&path).unwrap_or_default();

    let high_thinking_toml = if data.high_thinking.is_empty() || data.high_thinking == "medium" {
        String::new()
    } else {
        format!("\nhigh.thinking = \"{}\"", data.high_thinking)
    };

    let new_section = format!(
        r#"
[router]
enabled = true
default_profile = "{profile_name}"

[router.profiles.{profile_name}]
high.model = "{high}"
{high_thinking}
medium.model = "{medium}"
low.model = "{low}"
"#,
        high = data.high_model,
        high_thinking = high_thinking_toml.trim(),
        medium = data.medium_model,
        low = data.low_model,
    );

    if !content.contains("[router]") {
        content.push_str(&new_section);
    } else {
        // Find the [router] section start — could be at file beginning or after a newline.
        let section_start = if content.starts_with("[router]") {
            0
        } else if let Some(s) = content.find("\n[router]") {
            s + 1 // skip the leading newline
        } else {
            // Shouldn't happen since we checked contains above, fallback to append.
            content.push_str(&new_section);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let tmp = path.with_extension("toml.tmp");
            std::fs::write(&tmp, &content).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
            let gd = dirs::config_dir().unwrap_or_default().join("oxi");
            let pd = std::env::current_dir().unwrap_or_default();
            return oxi_store::router_config::load_router_config(&gd, &pd)
                .ok_or_else(|| "Failed to reload router config after save".to_string());
        };

        let remainder = &content[section_start..];
        let section_end = remainder
            .find("\n[")
            .map(|p| section_start + p)
            .unwrap_or(content.len());
        content = format!(
            "{}{}{}",
            &content[..section_start],
            new_section.trim_end(),
            &content[section_end..]
        );
    }

    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;

    let global_dir = dirs::config_dir().unwrap_or_default().join("oxi");
    let project_dir = std::env::current_dir().unwrap_or_default();
    oxi_store::router_config::load_router_config(&global_dir, &project_dir)
        .ok_or_else(|| "Failed to reload router config after save".to_string())
}
