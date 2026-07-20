//! Shim of `xai_grok_config` — config functions used by the render crate.

use std::path::PathBuf;

/// Return the grok home directory (~/.grok).
pub fn grok_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
}

/// Default grok home (used for identity checks).
pub fn default_grok_home() -> PathBuf {
    grok_home()
}

/// User-specific grok home, if different from default.
pub fn user_grok_home() -> Option<PathBuf> {
    None
}

/// Load effective config (disk-only, no network).
/// Returns a default empty config — the render crate only uses this
/// for path resolution (theme dirs, cache dirs).
pub fn load_effective_config_disk_only(
) -> Result<EffectiveConfig, ConfigError> {
    Ok(EffectiveConfig::default())
}

#[derive(Debug, Clone, Default)]
pub struct EffectiveConfig {
    pub grok_home: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ConfigError {}
