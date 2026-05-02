//! Settings management for oxi CLI
//!
//! Settings are loaded in layers (later layers override earlier):
//! 1. Built-in defaults
//! 2. Global config: `~/.oxi/settings.toml`
//! 3. Project config: `.oxi/settings.toml` (walked up to repo root)
//! 4. Environment variables (`OXI_*` prefix)
//! 5. CLI arguments
//!
//! Migration is handled via a `version` field in the config file.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

/// Current settings format version.
const SETTINGS_VERSION: u32 = 1;

/// Environment variable prefix for oxi settings.
#[allow(dead_code)]
const ENV_PREFIX: &str = "OXI_";

/// Thinking level for agent responses
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    /// No thinking (fastest)
    None,
    /// Minimal thinking
    Minimal,
    /// Standard thinking (default)
    #[default]
    Standard,
    /// Thorough thinking (slowest, best quality)
    Thorough,
}

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    // ── Version (for migration) ──────────────────────────────────────
    /// Settings format version. Used for automatic migration.
    #[serde(default)]
    pub version: u32,

    // ── Core LLM settings ───────────────────────────────────────────
    /// Thinking level for agent responses
    #[serde(default)]
    pub thinking_level: ThinkingLevel,

    /// Color theme (e.g., "default", "monokai", "dracula")
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Default model to use (e.g., "anthropic/claude-sonnet-4-20250514")
    pub default_model: Option<String>,

    /// Default provider to use (e.g., "anthropic", "openai")
    pub default_provider: Option<String>,

    /// Max tokens for responses
    pub max_tokens: Option<u32>,

    /// Temperature for generation (0.0–2.0)
    pub temperature: Option<f32>,

    /// Default temperature as f64 (higher precision, takes precedence over `temperature`)
    pub default_temperature: Option<f64>,

    /// Maximum tokens for generation (usize variant, takes precedence over `max_tokens`)
    pub max_response_tokens: Option<usize>,

    // ── Session settings ─────────────────────────────────────────────
    /// Session history size (entries to keep in memory)
    #[serde(default = "default_session_history_size")]
    pub session_history_size: usize,

    /// Directory for storing sessions (default: `~/.oxi/sessions`)
    pub session_dir: Option<PathBuf>,

    // ── Behaviour flags ──────────────────────────────────────────────
    /// Whether to stream responses
    #[serde(default = "default_true")]
    pub stream_responses: bool,

    /// Whether extensions are enabled
    #[serde(default = "default_true")]
    pub extensions_enabled: bool,

    /// Whether to auto-compact conversations that exceed context window
    #[serde(default = "default_true")]
    pub auto_compaction: bool,

    // ── Timeouts ─────────────────────────────────────────────────────
    /// Timeout in seconds for tool execution
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_seconds: u64,
}

fn default_theme() -> String {
    "default".to_string()
}

fn default_session_history_size() -> usize {
    100
}

fn default_true() -> bool {
    true
}

fn default_tool_timeout() -> u64 {
    120
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            thinking_level: ThinkingLevel::Standard,
            theme: default_theme(),
            default_model: None,
            default_provider: None,
            max_tokens: None,
            temperature: None,
            default_temperature: None,
            max_response_tokens: None,
            session_history_size: default_session_history_size(),
            session_dir: None,
            stream_responses: true,
            extensions_enabled: true,
            auto_compaction: true,
            tool_timeout_seconds: default_tool_timeout(),
        }
    }
}

impl Settings {
    // ── Paths ────────────────────────────────────────────────────────

    /// Get the global settings directory path (`~/.oxi`).
    pub fn settings_dir() -> Result<PathBuf> {
        let base = dirs::home_dir().context("Cannot determine home directory")?;
        Ok(base.join(".oxi"))
    }

    /// Get the global settings file path (`~/.oxi/settings.toml`).
    pub fn settings_path() -> Result<PathBuf> {
        Ok(Self::settings_dir()?.join("settings.toml"))
    }

    /// Get the project-local settings file path.
    ///
    /// Walks from `start_dir` upward looking for `.oxi/settings.toml`,
    /// stopping at the filesystem root.
    pub fn find_project_settings(start_dir: &std::path::Path) -> Option<PathBuf> {
        let mut dir = start_dir.to_path_buf();
        loop {
            let candidate = dir.join(".oxi").join("settings.toml");
            if candidate.exists() {
                return Some(candidate);
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    /// Resolve the effective session directory.
    ///
    /// Priority: `session_dir` field → `$OXI_SESSION_DIR` → `~/.oxi/sessions`.
    pub fn effective_session_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.session_dir {
            return Ok(dir.clone());
        }
        if let Ok(dir) = env::var("OXI_SESSION_DIR") {
            return Ok(PathBuf::from(dir));
        }
        Ok(Self::settings_dir()?.join("sessions"))
    }

    // ── Loading ──────────────────────────────────────────────────────

    /// Load settings, applying all layers:
    ///
    /// 1. Built-in defaults
    /// 2. Global `~/.oxi/settings.toml`
    /// 3. Project `.oxi/settings.toml`
    /// 4. Environment variable overrides
    pub fn load() -> Result<Self> {
        Self::load_from_cwd()
    }

    /// Load settings with an explicit working directory for project config discovery.
    pub fn load_from(dir: &std::path::Path) -> Result<Self> {
        // 1. Start from defaults
        let mut settings = Settings::default();

        // 2. Layer global config
        if let Ok(global_path) = Self::settings_path() {
            if global_path.exists() {
                settings = Self::layer_file(&settings, &global_path)?;
            }
        }

        // 3. Layer project config
        if let Some(project_path) = Self::find_project_settings(dir) {
            settings = Self::layer_file(&settings, &project_path)?;
        }

        // 4. Layer environment variables
        settings.apply_env();

        // 5. Run migration if needed
        settings = Self::migrate(settings)?;

        Ok(settings)
    }

    /// Convenience: load from current working directory.
    pub fn load_from_cwd() -> Result<Self> {
        let cwd = env::current_dir().context("Cannot determine current directory")?;
        Self::load_from(&cwd)
    }

    /// Parse a TOML file and overlay its values onto `base`.
    ///
    /// Fields present in the file replace those in `base`; absent fields
    /// are left untouched.
    fn layer_file(base: &Settings, path: &std::path::Path) -> Result<Settings> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read settings from {}", path.display()))?;

        // We parse into a partial overlay so that only explicitly-set
        // fields are applied.
        let overlay: toml::Value = toml::from_str(&content)
            .with_context(|| format!("Failed to parse settings from {}", path.display()))?;

        // Re-serialize the base, merge with the overlay table, then
        // deserialize back. This gives correct "only override what's
        // present" semantics.
        let mut base_table = toml::Value::try_from(base)
            .context("Failed to serialize base settings for merge")?;

        if let (toml::Value::Table(ref mut base_t), toml::Value::Table(ref overlay_t)) =
            (&mut base_table, &overlay)
        {
            for (key, value) in overlay_t {
                base_t.insert(key.clone(), value.clone());
            }
        }

        let merged: Settings = base_table
            .try_into()
            .context("Failed to deserialize merged settings")?;

        Ok(merged)
    }

    // ── Environment variables ────────────────────────────────────────

    /// Apply environment variable overrides in-place.
    ///
    /// Supported variables:
    ///
    /// | Env var                    | Setting                |
    /// |---------------------------|------------------------|
    /// | `OXI_MODEL`               | `default_model`        |
    /// | `OXI_PROVIDER`            | `default_provider`     |
    /// | `OXI_THINKING`            | `thinking_level`       |
    /// | `OXI_THEME`               | `theme`                |
    /// | `OXI_MAX_TOKENS`          | `max_tokens`           |
    /// | `OXI_TEMPERATURE`         | `default_temperature`  |
    /// | `OXI_SESSION_DIR`         | `session_dir`          |
    /// | `OXI_STREAM`              | `stream_responses`     |
    /// | `OXI_EXTENSIONS_ENABLED`  | `extensions_enabled`   |
    /// | `OXI_AUTO_COMPACTION`     | `auto_compaction`      |
    /// | `OXI_TOOL_TIMEOUT`        | `tool_timeout_seconds` |
    pub fn apply_env(&mut self) {
        if let Ok(v) = env::var("OXI_MODEL") {
            self.default_model = Some(v);
        }
        if let Ok(v) = env::var("OXI_PROVIDER") {
            self.default_provider = Some(v);
        }
        if let Ok(v) = env::var("OXI_THINKING") {
            if let Some(level) = parse_thinking_level(&v) {
                self.thinking_level = level;
            }
        }
        if let Ok(v) = env::var("OXI_THEME") {
            self.theme = v;
        }
        if let Ok(v) = env::var("OXI_MAX_TOKENS") {
            if let Ok(n) = v.parse::<u32>() {
                self.max_tokens = Some(n);
            }
        }
        if let Ok(v) = env::var("OXI_TEMPERATURE") {
            if let Ok(n) = v.parse::<f64>() {
                self.default_temperature = Some(n);
            }
        }
        if let Ok(v) = env::var("OXI_SESSION_DIR") {
            self.session_dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = env::var("OXI_STREAM") {
            if let Ok(b) = parse_boolish(&v) {
                self.stream_responses = b;
            }
        }
        if let Ok(v) = env::var("OXI_EXTENSIONS_ENABLED") {
            if let Ok(b) = parse_boolish(&v) {
                self.extensions_enabled = b;
            }
        }
        if let Ok(v) = env::var("OXI_AUTO_COMPACTION") {
            if let Ok(b) = parse_boolish(&v) {
                self.auto_compaction = b;
            }
        }
        if let Ok(v) = env::var("OXI_TOOL_TIMEOUT") {
            if let Ok(n) = v.parse::<u64>() {
                self.tool_timeout_seconds = n;
            }
        }
    }

    /// Build a `Settings` instance from **only** environment variables
    /// (all other fields stay at defaults).
    pub fn from_env() -> Self {
        let mut settings = Self::default();
        settings.apply_env();
        settings
    }

    // ── Persistence ──────────────────────────────────────────────────

    /// Save settings to the global config file (`~/.oxi/settings.toml`).
    pub fn save(&self) -> Result<()> {
        let dir = Self::settings_dir()?;
        let path = Self::settings_path()?;

        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create settings directory {}", dir.display()))?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize settings")?;

        fs::write(&path, content)
            .with_context(|| format!("Failed to write settings to {}", path.display()))?;

        Ok(())
    }

    /// Save settings to the project-local config file (`.oxi/settings.toml`).
    pub fn save_project(&self, project_dir: &std::path::Path) -> Result<()> {
        let dir = project_dir.join(".oxi");
        let path = dir.join("settings.toml");

        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create project settings directory {}", dir.display()))?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize settings")?;

        fs::write(&path, content)
            .with_context(|| format!("Failed to write settings to {}", path.display()))?;

        Ok(())
    }

    // ── CLI overrides ────────────────────────────────────────────────

    /// Merge with CLI arguments (CLI takes precedence).
    pub fn merge_cli(&mut self, model: Option<String>, provider: Option<String>) {
        if let Some(m) = model {
            self.default_model = Some(m);
        }
        if let Some(p) = provider {
            self.default_provider = Some(p);
        }
    }

    /// Get the effective model ID (provider/model format).
    pub fn effective_model(&self, cli_model: Option<&str>) -> String {
        cli_model
            .map(String::from)
            .or_else(|| self.default_model.clone())
            .unwrap_or_else(|| "anthropic/claude-sonnet-4-20250514".to_string())
    }

    /// Get the effective provider.
    pub fn effective_provider(&self, cli_provider: Option<&str>) -> String {
        cli_provider
            .map(String::from)
            .or_else(|| self.default_provider.clone())
            .unwrap_or_else(|| "anthropic".to_string())
    }

    /// Get the effective temperature, preferring `default_temperature` (f64)
    /// over `temperature` (f32), falling back to `None`.
    pub fn effective_temperature(&self) -> Option<f64> {
        self.default_temperature
            .or(self.temperature.map(|t| t as f64))
    }

    /// Get the effective max tokens, preferring `max_response_tokens` (usize)
    /// over `max_tokens` (u32), falling back to `None`.
    pub fn effective_max_tokens(&self) -> Option<usize> {
        self.max_response_tokens.or(self.max_tokens.map(|t| t as usize))
    }

    // ── Migration ────────────────────────────────────────────────────

    /// Migrate settings from an older format version to the current one.
    ///
    /// Currently handles version `0` (no version field) → version `1`.
    fn migrate(settings: Settings) -> Result<Settings> {
        let mut settings = settings;

        match settings.version {
            SETTINGS_VERSION => {
                // Already current — nothing to do.
            }
            0 => {
                // Version 0 = pre-versioning config.
                // Add any defaults that were introduced in version 1.
                if settings.tool_timeout_seconds == 0 {
                    settings.tool_timeout_seconds = default_tool_timeout();
                }
                settings.version = SETTINGS_VERSION;

                tracing::info!("Migrated settings from version 0 to {}", SETTINGS_VERSION);
            }
            v if v > SETTINGS_VERSION => {
                // Future version — we don't know how to downgrade.
                anyhow::bail!(
                    "Settings version {} is newer than supported version {}. \
                     Please update oxi.",
                    v,
                    SETTINGS_VERSION
                );
            }
            v => {
                // Unknown old version — best-effort migration.
                tracing::warn!(
                    "Unknown settings version {}, attempting migration to {}",
                    v,
                    SETTINGS_VERSION
                );
                settings.version = SETTINGS_VERSION;
            }
        }

        Ok(settings)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Parse a thinking level from a string.
pub fn parse_thinking_level(s: &str) -> Option<ThinkingLevel> {
    match s.to_lowercase().as_str() {
        "none" => Some(ThinkingLevel::None),
        "minimal" => Some(ThinkingLevel::Minimal),
        "standard" => Some(ThinkingLevel::Standard),
        "thorough" => Some(ThinkingLevel::Thorough),
        _ => None,
    }
}

/// Parse a boolean-like string (`"true"`, `"false"`, `"1"`, `"0"`, `"yes"`, `"no"`).
fn parse_boolish(s: &str) -> Result<bool> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("Cannot parse '{}' as boolean", s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;

    // ── Struct tests ─────────────────────────────────────────────────

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.version, SETTINGS_VERSION);
        assert_eq!(settings.thinking_level, ThinkingLevel::Standard);
        assert_eq!(settings.theme, "default");
        assert!(settings.default_model.is_none());
        assert!(settings.default_provider.is_none());
        assert!(settings.extensions_enabled);
        assert!(settings.auto_compaction);
        assert_eq!(settings.tool_timeout_seconds, 120);
        assert!(settings.stream_responses);
    }

    #[test]
    fn test_merge_cli() {
        let mut settings = Settings::default();
        settings.default_model = Some("openai/gpt-4o".to_string());

        settings.merge_cli(Some("claude".to_string()), None);
        assert_eq!(settings.default_model, Some("claude".to_string()));

        settings.merge_cli(None, Some("google".to_string()));
        assert_eq!(settings.default_provider, Some("google".to_string()));
    }

    // ── Layered loading ──────────────────────────────────────────────

    #[test]
    fn test_layer_file_overrides() {
        let base = Settings::default();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let toml_content = r#"
default_model = "openai/gpt-4o"
theme = "dracula"
"#;
        tmp.as_file().write_all(toml_content.as_bytes()).unwrap();

        let merged = Settings::layer_file(&base, tmp.path()).unwrap();
        assert_eq!(merged.default_model, Some("openai/gpt-4o".to_string()));
        assert_eq!(merged.theme, "dracula");
        // Unchanged fields retain defaults
        assert_eq!(merged.thinking_level, ThinkingLevel::Standard);
        assert!(merged.extensions_enabled);
    }

    #[test]
    fn test_layer_file_preserves_unset() {
        let mut base = Settings::default();
        base.default_provider = Some("deepseek".to_string());

        let tmp = tempfile::NamedTempFile::new().unwrap();
        // Only override theme — provider should remain
        let toml_content = "theme = \"monokai\"\n";
        tmp.as_file().write_all(toml_content.as_bytes()).unwrap();

        let merged = Settings::layer_file(&base, tmp.path()).unwrap();
        assert_eq!(merged.theme, "monokai");
        assert_eq!(merged.default_provider, Some("deepseek".to_string()));
    }

    #[test]
    fn test_load_from_dir_with_project_config() {
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();
        let settings_path = oxi_dir.join("settings.toml");
        fs::write(&settings_path, "default_model = \"google/gemini-2.0-flash\"\n").unwrap();

        let settings = Settings::load_from(tmp.path()).unwrap();
        assert_eq!(settings.default_model, Some("google/gemini-2.0-flash".to_string()));
    }

    #[test]
    fn test_load_from_dir_no_config() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings::load_from(tmp.path()).unwrap();
        // Falls back to defaults
        assert!(settings.default_model.is_none());
        assert_eq!(settings.thinking_level, ThinkingLevel::Standard);
    }

    // ── Environment variables ────────────────────────────────────────

    #[test]
    fn test_from_env() {
        env::set_var("OXI_MODEL", "anthropic/claude-haiku-4-20250414");
        env::set_var("OXI_THEME", "nord");
        env::set_var("OXI_TOOL_TIMEOUT", "60");

        let settings = Settings::from_env();
        assert_eq!(settings.default_model, Some("anthropic/claude-haiku-4-20250414".to_string()));
        assert_eq!(settings.theme, "nord");
        assert_eq!(settings.tool_timeout_seconds, 60);

        // Clean up
        env::remove_var("OXI_MODEL");
        env::remove_var("OXI_THEME");
        env::remove_var("OXI_TOOL_TIMEOUT");
    }

    #[test]
    fn test_apply_env_boolish() {
        env::set_var("OXI_STREAM", "false");
        env::set_var("OXI_EXTENSIONS_ENABLED", "0");

        let mut settings = Settings::default();
        settings.apply_env();
        assert!(!settings.stream_responses);
        assert!(!settings.extensions_enabled);

        env::remove_var("OXI_STREAM");
        env::remove_var("OXI_EXTENSIONS_ENABLED");
    }

    #[test]
    fn test_apply_env_temperature() {
        env::set_var("OXI_TEMPERATURE", "0.7");

        let mut settings = Settings::default();
        settings.apply_env();
        assert_eq!(settings.default_temperature, Some(0.7));

        env::remove_var("OXI_TEMPERATURE");
    }

    #[test]
    fn test_env_does_not_override_when_unset() {
        // Make sure these are not set in the test environment
        env::remove_var("OXI_MODEL");
        env::remove_var("OXI_PROVIDER");

        let settings = Settings::from_env();
        assert!(settings.default_model.is_none());
        assert!(settings.default_provider.is_none());
    }

    // ── Helpers ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_thinking_level() {
        assert_eq!(parse_thinking_level("none"), Some(ThinkingLevel::None));
        assert_eq!(parse_thinking_level("MINIMAL"), Some(ThinkingLevel::Minimal));
        assert_eq!(parse_thinking_level("Standard"), Some(ThinkingLevel::Standard));
        assert_eq!(parse_thinking_level("thorough"), Some(ThinkingLevel::Thorough));
        assert_eq!(parse_thinking_level("invalid"), None);
    }

    #[test]
    fn test_parse_boolish() {
        assert!(parse_boolish("true").unwrap());
        assert!(parse_boolish("1").unwrap());
        assert!(parse_boolish("yes").unwrap());
        assert!(parse_boolish("ON").unwrap());
        assert!(!parse_boolish("false").unwrap());
        assert!(!parse_boolish("0").unwrap());
        assert!(!parse_boolish("no").unwrap());
        assert!(!parse_boolish("OFF").unwrap());
        assert!(parse_boolish("maybe").is_err());
    }

    // ── Effective accessors ──────────────────────────────────────────

    #[test]
    fn test_effective_temperature_prefers_f64() {
        let mut settings = Settings::default();
        settings.temperature = Some(0.5);
        settings.default_temperature = Some(0.7);
        assert_eq!(settings.effective_temperature(), Some(0.7));
    }

    #[test]
    fn test_effective_temperature_falls_back_to_f32() {
        let mut settings = Settings::default();
        settings.temperature = Some(0.5);
        assert_eq!(settings.effective_temperature(), Some(0.5));
    }

    #[test]
    fn test_effective_max_tokens_prefers_usize() {
        let mut settings = Settings::default();
        settings.max_tokens = Some(1024);
        settings.max_response_tokens = Some(4096);
        assert_eq!(settings.effective_max_tokens(), Some(4096));
    }

    #[test]
    fn test_effective_max_tokens_falls_back_to_u32() {
        let mut settings = Settings::default();
        settings.max_tokens = Some(1024);
        assert_eq!(settings.effective_max_tokens(), Some(1024));
    }

    // ── Session dir ──────────────────────────────────────────────────

    #[test]
    fn test_effective_session_dir_default() {
        env::remove_var("OXI_SESSION_DIR");
        let settings = Settings::default();
        let dir = settings.effective_session_dir().unwrap();
        assert!(dir.ends_with("sessions"));
    }

    #[test]
    fn test_effective_session_dir_from_field() {
        env::remove_var("OXI_SESSION_DIR");
        let mut settings = Settings::default();
        settings.session_dir = Some(PathBuf::from("/tmp/oxi-sessions"));
        assert_eq!(settings.effective_session_dir().unwrap(), PathBuf::from("/tmp/oxi-sessions"));
    }

    #[test]
    fn test_effective_session_dir_from_env() {
        env::set_var("OXI_SESSION_DIR", "/tmp/env-sessions");
        let settings = Settings::default();
        assert_eq!(settings.effective_session_dir().unwrap(), PathBuf::from("/tmp/env-sessions"));
        env::remove_var("OXI_SESSION_DIR");
    }

    // ── Migration ────────────────────────────────────────────────────

    #[test]
    fn test_migration_v0_to_v1() {
        let mut settings = Settings::default();
        settings.version = 0;
        settings.tool_timeout_seconds = 0; // v0 might not have this field

        let migrated = Settings::migrate(settings).unwrap();
        assert_eq!(migrated.version, SETTINGS_VERSION);
        assert_eq!(migrated.tool_timeout_seconds, 120);
    }

    #[test]
    fn test_migration_already_current() {
        let settings = Settings::default();
        let migrated = Settings::migrate(settings).unwrap();
        assert_eq!(migrated.version, SETTINGS_VERSION);
    }

    #[test]
    fn test_migration_future_version_fails() {
        let mut settings = Settings::default();
        settings.version = 9999;
        assert!(Settings::migrate(settings).is_err());
    }

    // ── Persistence ──────────────────────────────────────────────────

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = tmp.path().join("settings.toml");

        let mut original = Settings::default();
        original.default_model = Some("openai/gpt-4o".to_string());
        original.theme = "dracula".to_string();
        original.tool_timeout_seconds = 60;

        // Serialize
        let content = toml::to_string_pretty(&original).unwrap();
        fs::write(&settings_path, &content).unwrap();

        // Deserialize
        let loaded_content = fs::read_to_string(&settings_path).unwrap();
        let loaded: Settings = toml::from_str(&loaded_content).unwrap();

        assert_eq!(loaded.default_model, original.default_model);
        assert_eq!(loaded.theme, original.theme);
        assert_eq!(loaded.tool_timeout_seconds, original.tool_timeout_seconds);
    }

    #[test]
    fn test_toml_roundtrip_preserves_new_fields() {
        let mut settings = Settings::default();
        settings.default_temperature = Some(0.8);
        settings.max_response_tokens = Some(8192);
        settings.auto_compaction = false;
        settings.extensions_enabled = false;
        settings.session_dir = Some(PathBuf::from("/custom/sessions"));

        let toml_str = toml::to_string_pretty(&settings).unwrap();
        let parsed: Settings = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.default_temperature, Some(0.8));
        assert_eq!(parsed.max_response_tokens, Some(8192));
        assert!(!parsed.auto_compaction);
        assert!(!parsed.extensions_enabled);
        assert_eq!(parsed.session_dir, Some(PathBuf::from("/custom/sessions")));
    }
}
