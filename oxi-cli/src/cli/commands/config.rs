//! Config subcommand handler and helpers.

use crate::cli::ConfigCommands;
use crate::storage::packages::{PackageManager, ResourceKind};
use crate::store::settings::Settings;
use anyhow::Result;

/// Handle `oxi config …` subcommands.
pub fn handle_config(action: &ConfigCommands) -> Result<()> {
    match action {
        ConfigCommands::Show => config_show(),
        ConfigCommands::List { resource_type } => config_list(resource_type.as_ref()),
        ConfigCommands::Enable {
            resource_type,
            name,
        } => config_toggle_resource(resource_type, name, true),
        ConfigCommands::Disable {
            resource_type,
            name,
        } => config_toggle_resource(resource_type, name, false),
        ConfigCommands::Set { key, value } => config_set(key, value),
        ConfigCommands::Get { key } => config_get(key),
        ConfigCommands::AddProvider {
            name,
            base_url,
            api_key_env,
            api,
        } => config_add_provider(name, base_url, api_key_env, api),
        ConfigCommands::RemoveProvider { name } => config_remove_provider(name),
        ConfigCommands::Reset { all } => handle_config_reset(*all),
        ConfigCommands::Path => handle_config_path_command(),
    }
}

/// Handle `oxi config show`
fn config_show() -> Result<()> {
    let settings = Settings::load()?;
    println!("oxi configuration:");
    println!("  Settings file: {}", Settings::settings_path()?.display());
    println!();
    println!(
        "  Model: {}",
        settings
            .effective_model(None)
            .unwrap_or_else(|| "(not set)".to_string())
    );
    println!(
        "  Provider: {}",
        settings
            .effective_provider(None)
            .unwrap_or_else(|| "(not set)".to_string())
    );
    println!("  Theme: {}", settings.get_theme_name());
    println!("  Glyph set: {}", settings.glyph_set.label());
    println!("  Thinking: {:?}", settings.thinking_level);
    println!("  Extensions enabled: {}", settings.extensions_enabled);
    println!("  Auto-compaction: {}", settings.auto_compaction);
    println!("  Tool timeout: {}s", settings.tool_timeout_seconds);

    let resource_types = [
        ("Extensions", &settings.extensions),
        ("Skills", &settings.skills),
        ("Prompts", &settings.prompts),
        ("Themes", &settings.themes),
    ];

    for (label, list) in &resource_types {
        if list.is_empty() {
            println!("  {}: (none)", label);
        } else {
            println!("  {}:", label);
            for item in list.iter() {
                println!("    - {}", item);
            }
        }
    }

    if settings.custom_providers.is_empty() {
        println!("  Custom providers: (none)");
    } else {
        println!("  Custom providers:");
        for cp in &settings.custom_providers {
            println!("    - {} ({} @ {})", cp.name, cp.api, cp.base_url);
        }
    }
    Ok(())
}

/// Handle `oxi config list [resource_type]`
fn config_list(resource_type: Option<&String>) -> Result<()> {
    let settings = Settings::load()?;

    let resource_types: Vec<(&str, &Vec<String>, ResourceKind)> = vec![
        ("extensions", &settings.extensions, ResourceKind::Extension),
        ("skills", &settings.skills, ResourceKind::Skill),
        ("prompts", &settings.prompts, ResourceKind::Prompt),
        ("themes", &settings.themes, ResourceKind::Theme),
    ];

    let filtered: Vec<_> = if let Some(rt) = resource_type {
        let kind = parse_resource_type(rt).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown resource type '{}'. Valid: extension, skill, prompt, theme",
                rt
            )
        })?;
        resource_types
            .into_iter()
            .filter(|(_, _, k)| *k == kind)
            .collect()
    } else {
        resource_types
    };

    for (label, list, _) in &filtered {
        if list.is_empty() {
            println!("No {} configured.", label);
        } else {
            println!("{}:", label);
            for (i, item) in list.iter().enumerate() {
                println!("  {}. {}", i + 1, item);
            }
        }
        println!();
    }

    // Also show resources from installed packages
    let mgr = PackageManager::new()?;
    let packages = mgr.list();
    if !packages.is_empty() {
        println!("Package resources:");
        for pkg in packages {
            if let Ok(resources) = mgr.discover_resources(&pkg.name) {
                for r in &resources {
                    if let Some(rt) = resource_type
                        && let Some(kind) = parse_resource_type(rt)
                        && r.kind != kind
                    {
                        continue;
                    }
                    println!("  {} [{}] {}", pkg.name, r.kind, r.relative_path);
                }
            }
        }
    }
    Ok(())
}

/// Handle `oxi config enable/disable <type> <name>`
fn config_toggle_resource(resource_type: &str, name: &str, enable: bool) -> Result<()> {
    let kind = parse_resource_type(resource_type).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown resource type '{}'. Valid: extension, skill, prompt, theme",
            resource_type
        )
    })?;

    let mut settings = Settings::load()?;

    let list = match kind {
        ResourceKind::Extension => &mut settings.extensions,
        ResourceKind::Skill => &mut settings.skills,
        ResourceKind::Prompt => &mut settings.prompts,
        ResourceKind::Theme => &mut settings.themes,
    };

    if enable {
        if list.iter().any(|item| item == name) {
            println!("{} '{}' is already enabled.", kind, name);
            return Ok(());
        }
        list.push(name.to_string());
        settings.save()?;
        println!("Enabled {} '{}'", kind, name);
    } else {
        let original_len = list.len();
        list.retain(|item| item != name);
        if list.len() == original_len {
            println!("{} '{}' was not enabled.", kind, name);
            return Ok(());
        }
        settings.save()?;
        println!("Disabled {} '{}'", kind, name);
    }
    Ok(())
}

/// Handle `oxi config set <key> <value>`
fn config_set(key: &str, value: &str) -> Result<()> {
    let mut settings = Settings::load()?;

    match key {
        "theme" => {
            settings.theme = value.to_string();
        }
        "model" => {
            settings.last_used_model = Some(value.to_string());
        }
        "provider" => {
            settings.last_used_provider = Some(value.to_string());
        }
        "thinking_level" | "thinking" => {
            let level = crate::store::settings::parse_thinking_level(value).ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid thinking level: '{}'. Valid: off, minimal, low, medium, high, xhigh",
                    value
                )
            })?;
            settings.thinking_level = level;
        }
        "extensions_enabled" | "extensions" => {
            settings.extensions_enabled = parse_config_bool(value)?;
        }
        "auto_compaction" => {
            settings.auto_compaction = parse_config_bool(value)?;
        }
        "tool_timeout" | "tool_timeout_seconds" => {
            settings.tool_timeout_seconds = value
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid timeout: '{}'", value))?;
        }
        "max_tokens" => {
            settings.max_tokens = Some(
                value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid max_tokens: '{}'", value))?,
            );
        }
        "temperature" => {
            settings.default_temperature = Some(
                value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid temperature: '{}'", value))?,
            );
        }
        "session_history_size" => {
            settings.session_history_size = value
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid session_history_size: '{}'", value))?;
        }
        "glyph" | "glyph_set" => {
            settings.glyph_set = value.parse().map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        _ => {
            anyhow::bail!(
                "Unknown setting: '{}'. Valid keys: theme, model, provider,\
                  thinking_level, extensions_enabled, auto_compaction,\
                  glyph, tool_timeout,\
                  max_tokens, temperature, session_history_size",
                key
            );
        }
    }

    settings.save()?;
    println!("Set {} = {}", key, value);
    Ok(())
}

/// Handle `oxi config get <key>`
fn config_get(key: &str) -> Result<()> {
    let settings = Settings::load()?;

    let value = match key {
        "theme" => settings.theme.clone(),
        "model" => settings
            .last_used_model
            .clone()
            .unwrap_or_else(|| "(not set)".to_string()),
        "provider" => settings
            .last_used_provider
            .clone()
            .unwrap_or_else(|| "(not set)".to_string()),
        "thinking_level" | "thinking" => format!("{:?}", settings.thinking_level).to_lowercase(),
        "extensions_enabled" => settings.extensions_enabled.to_string(),
        "auto_compaction" => settings.auto_compaction.to_string(),
        "tool_timeout" | "tool_timeout_seconds" => {
            format!("{}s", settings.tool_timeout_seconds)
        }
        "max_tokens" => settings
            .max_tokens
            .map(|t| t.to_string())
            .unwrap_or_else(|| "(not set)".to_string()),
        "temperature" => settings
            .effective_temperature()
            .map(|t| t.to_string())
            .unwrap_or_else(|| "(not set)".to_string()),
        "session_history_size" => settings.session_history_size.to_string(),
        "extensions" => format!("{:?}", settings.extensions),
        "skills" => format!("{:?}", settings.skills),
        "prompts" => format!("{:?}", settings.prompts),
        "themes" => format!("{:?}", settings.themes),
        "custom_providers" => {
            let items: Vec<String> = settings
                .custom_providers
                .iter()
                .map(|cp| format!("{} ({} @ {})", cp.name, cp.api, cp.base_url))
                .collect();
            if items.is_empty() {
                "(none)".to_string()
            } else {
                items.join(", ")
            }
        }
        "glyph" | "glyph_set" => settings.glyph_set.label().to_string(),
        _ => {
            anyhow::bail!(
                "Unknown setting: '{}'. Valid keys: theme, model, provider,\
                  thinking_level, extensions_enabled, auto_compaction,\
                  glyph, tool_timeout,\
                  max_tokens, temperature, session_history_size,\
                  extensions, skills, prompts, themes, custom_providers",
                key
            );
        }
    };

    println!("{} = {}", key, value);
    Ok(())
}

/// Handle `oxi config add-provider`
fn config_add_provider(name: &str, base_url: &str, api_key_env: &str, api: &str) -> Result<()> {
    use crate::store::settings::CustomProvider;

    let mut settings = Settings::load()?;

    // Update existing or add new
    if let Some(cp) = settings
        .custom_providers
        .iter_mut()
        .find(|cp| cp.name == name)
    {
        cp.base_url = base_url.to_string();
        cp.api_key_env = api_key_env.to_string();
        cp.api = api.to_string();
        settings.save()?;
        println!(
            "Updated custom provider '{}' -> {} ({})",
            name, base_url, api
        );
    } else {
        settings.custom_providers.push(CustomProvider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: api_key_env.to_string(),
            api: api.to_string(),
        });
        settings.save()?;
        println!("Added custom provider '{}' -> {} ({})", name, base_url, api);
    }
    Ok(())
}

/// Handle `oxi config remove-provider`
fn config_remove_provider(name: &str) -> Result<()> {
    let mut settings = Settings::load()?;
    let original_len = settings.custom_providers.len();
    settings.custom_providers.retain(|cp| cp.name != name);

    if settings.custom_providers.len() == original_len {
        println!("Custom provider '{}' not found.", name);
        return Ok(());
    }

    settings.save()?;
    println!("Removed custom provider '{}'", name);
    Ok(())
}

/// Handle `oxi config reset [--all]`
///
/// Resets auth.json (credentials). With `--all`, also resets settings.
pub fn handle_config_reset(all: bool) -> Result<()> {
    // Always reset auth.json
    let auth_path = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("oxi")
        .join("auth.json");
    if auth_path.exists() {
        std::fs::remove_file(&auth_path)?;
        println!("Removed credentials: {}", auth_path.display());
    } else {
        println!("No credentials file found at {}", auth_path.display());
    }

    if all {
        // Also reset settings
        if let Ok(settings_path) = crate::store::settings::Settings::settings_path()
            && settings_path.exists()
        {
            std::fs::remove_file(&settings_path)?;
            println!("Removed settings: {}", settings_path.display());
        }
        println!("Full reset complete. Run 'oxi setup' to reconfigure.");
    } else {
        println!("Credentials reset. Run 'oxi setup' to reconfigure API keys.");
    }

    Ok(())
}

/// Handle `oxi config path` — print config file path.
fn handle_config_path_command() -> Result<()> {
    let path = crate::store::settings::Settings::settings_path()?;
    println!("{}", path.display());
    Ok(())
}

/// Parse a resource type string into a ResourceKind
pub fn parse_resource_type(s: &str) -> Option<ResourceKind> {
    match s.to_lowercase().as_str() {
        "extension" | "extensions" | "ext" => Some(ResourceKind::Extension),
        "skill" | "skills" => Some(ResourceKind::Skill),
        "prompt" | "prompts" => Some(ResourceKind::Prompt),
        "theme" | "themes" => Some(ResourceKind::Theme),
        _ => None,
    }
}

/// Parse a boolean value from a config string
pub fn parse_config_bool(s: &str) -> Result<bool> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => anyhow::bail!(
            "Invalid boolean value: '{}'. Use true/false, yes/no, on/off, or 1/0",
            s
        ),
    }
}
