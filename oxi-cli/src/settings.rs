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
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Current settings format version.
const SETTINGS_VERSION: u32 = 4;

/// Environment variable prefix for oxi settings.
/// Keep: reserved for future env-based config loading (e.g. OXI_API_KEY).
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

/// A custom OpenAI-compatible provider configuration.
///
/// Custom providers are loaded from `~/.oxi/settings.toml` via `[[custom_provider]]` sections
/// and registered at runtime so that models like `minimax/minimax-m2.5` can be used directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    /// Unique provider name (e.g. `"minimax"`).
    pub name: String,
    /// Base URL of the OpenAI-compatible API (e.g. `"https://api.minimax.chat/v1"`).
    pub base_url: String,
    /// Environment variable name that holds the API key (e.g. `"MINIMAX_API_KEY"`).
    pub api_key_env: String,
    /// API dialect: `"openai-completions"` or `"openai-responses"`.
    #[serde(default = "default_custom_provider_api")]
    pub api: String,
}

fn default_custom_provider_api() -> String {
    "openai-completions".to_string()
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

    /// Default model name without provider prefix (e.g., "claude-sonnet-4-20250514")
    pub default_model: Option<String>,

    /// Default provider to use (e.g., "anthropic", "openai")
    pub default_provider: Option<String>,

    /// Last used model (automatically updated when user selects a model)
    #[serde(default)]
    pub last_used_model: Option<String>,

    /// Last used provider (automatically updated when user selects a model)
    #[serde(default)]
    pub last_used_provider: Option<String>,

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

    /// Built-in tools to disable (by name, e.g. `["web_search", "github_search"]`).
    /// All tools are enabled by default; list tools here to turn them off.
    #[serde(default)]
    pub disabled_tools: Vec<String>,

    // ── Timeouts ─────────────────────────────────────────────────────
    /// Timeout in seconds for tool execution
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_seconds: u64,

    // ── Resource lists (managed by `oxi config`) ────────────────────
    /// List of extension paths or npm package sources to load
    #[serde(default)]
    pub extensions: Vec<String>,

    /// List of skill paths or npm package sources to load
    #[serde(default)]
    pub skills: Vec<String>,

    /// List of prompt template paths to load
    #[serde(default)]
    pub prompts: Vec<String>,

    /// List of theme paths to load
    #[serde(default)]
    pub themes: Vec<String>,

       // ── Custom OpenAI-compatible providers ──────────────────────────────
    /// Registered custom providers (loaded from `[[custom_provider]]` TOML sections).
    #[serde(default)]
    pub custom_providers: Vec<CustomProvider>,

    // ── Dynamic model cache ─────────────────────────────────────────────
    /// Cached model lists fetched from provider `/models` endpoints.
    /// Key is the provider name, value is a list of model IDs.
    /// Updated when API keys are entered in setup wizard or on demand.
    #[serde(default)]
    pub dynamic_models: HashMap<String, Vec<String>>,
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
            last_used_model: None,
            last_used_provider: None,
            max_tokens: None,
            temperature: None,
            default_temperature: None,
            max_response_tokens: None,
            session_history_size: default_session_history_size(),
            session_dir: None,
            stream_responses: true,
            extensions_enabled: true,
            auto_compaction: true,
            disabled_tools: Vec::new(),
            tool_timeout_seconds: default_tool_timeout(),
            extensions: Vec::new(),
            skills: Vec::new(),
            prompts: Vec::new(),
            themes: Vec::new(),
            custom_providers: Vec::new(),
            dynamic_models: HashMap::new(),
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

    /// Get the global settings TOML file path (`~/.oxi/settings.toml`).
    pub fn settings_toml_path() -> Result<PathBuf> {
        Ok(Self::settings_dir()?.join("settings.toml"))
    }

    /// Get the global settings JSON file path (`~/.oxi/settings.json`).
    pub fn settings_json_path() -> Result<PathBuf> {
        Ok(Self::settings_dir()?.join("settings.json"))
    }

    /// Get the global settings file path (JSON takes priority).
    ///
    /// Returns the path to the settings file that should be used.
    /// If both JSON and TOML exist, JSON is returned (takes priority).
    /// If only one exists, that path is returned.
    /// If neither exists, returns the JSON path by default.
    pub fn settings_path() -> Result<PathBuf> {
        let json_path = Self::settings_json_path()?;
        let toml_path = Self::settings_toml_path()?;

        if json_path.exists() && toml_path.exists() {
            // Both exist: JSON takes priority
            tracing::debug!("Both settings.json and settings.toml exist, using settings.json");
            return Ok(json_path);
        }

        if json_path.exists() {
            return Ok(json_path);
        }

        if toml_path.exists() {
            return Ok(toml_path);
        }

        // Neither exists: default to JSON
        Ok(json_path)
    }

    /// Get the effective settings file path, preferring the specified format.
    ///
    /// If `prefer_json` is true, checks JSON first; otherwise checks TOML first.
    /// Returns the first existing file, or the preferred path if neither exists.
    pub fn settings_path_with_preference(prefer_json: bool) -> Result<PathBuf> {
        let json_path = Self::settings_json_path()?;
        let toml_path = Self::settings_toml_path()?;

        let (primary, secondary) = if prefer_json {
            (&json_path, &toml_path)
        } else {
            (&toml_path, &json_path)
        };

        if primary.exists() {
            return Ok(primary.clone());
        }

        if secondary.exists() {
            return Ok(secondary.clone());
        }

        // Neither exists: return preferred path
        Ok(primary.clone())
    }

    /// Detect the settings file format from its path.
    pub fn detect_format(path: &Path) -> SettingsFormat {
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => SettingsFormat::Json,
            Some("toml") => SettingsFormat::Toml,
            _ => SettingsFormat::Json, // Default to JSON for unknown extensions
        }
    }

    /// Get the project-local settings file path.
    ///
    /// Searches for `.oxi/settings.json` first, then `.oxi/settings.toml`.
    /// Returns the first one found, or None if neither exists.
    pub fn find_project_settings(start_dir: &std::path::Path) -> Option<PathBuf> {
        let mut dir = start_dir.to_path_buf();
        loop {
            // Check JSON first (priority), then TOML
            let json_candidate = dir.join(".oxi").join("settings.json");
            if json_candidate.exists() {
                return Some(json_candidate);
            }

            let toml_candidate = dir.join(".oxi").join("settings.toml");
            if toml_candidate.exists() {
                return Some(toml_candidate);
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
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use oxi_cli::Settings;
    ///
    /// let settings = Settings::load().expect("Failed to load settings");
    /// println!("Using model: {}", settings.effective_model(None));
    /// ```
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

    /// Parse a settings file (TOML or JSON) and overlay its values onto `base`.
    ///
    /// The format is auto-detected based on the file extension.
    /// Fields present in the file replace those in `base`; absent fields
    /// are left untouched.
    fn layer_file(base: &Settings, path: &std::path::Path) -> Result<Settings> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read settings from {}", path.display()))?;

        let format = Self::detect_format(path);
        let overlay: serde_json::Value = match format {
            SettingsFormat::Toml => {
                let toml_value: toml::Value = toml::from_str(&content).with_context(|| {
                    format!("Failed to parse TOML settings from {}", path.display())
                })?;
                // Convert TOML to JSON Value for uniform merging
                toml_value_to_json(toml_value)
            }
            SettingsFormat::Json => serde_json::from_str(&content).with_context(|| {
                format!("Failed to parse JSON settings from {}", path.display())
            })?,
        };

        // Re-serialize the base to JSON, merge with the overlay, then
        // deserialize back. This gives correct "only override what's
        // present" semantics.
        let base_json =
            serde_json::to_value(base).context("Failed to serialize base settings for merge")?;

        let merged = merge_json_values(base_json, overlay);
        let result: Settings =
            serde_json::from_value(merged).context("Failed to deserialize merged settings")?;

        Ok(result)
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

    /// Save settings to the global config file.
    ///
    /// Uses the format of the existing file if present, otherwise saves as JSON.
    /// Preserves backward compatibility with existing TOML files.
    pub fn save(&self) -> Result<()> {
        let dir = Self::settings_dir()?;
        let path = Self::settings_path()?;

        if !dir.exists() {
            fs::create_dir_all(&dir).with_context(|| {
                format!("Failed to create settings directory {}", dir.display())
            })?;
        }

        let format = Self::detect_format(&path);
        let content = Self::serialize_for_format(self, format)?;

        // Atomic write: write to temp file first, then rename
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &content)
            .with_context(|| format!("Failed to write settings to {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &path)
            .with_context(|| format!("Failed to rename settings to {}", path.display()))?;

        Ok(())
    }

    /// Save settings to a specific path, using the format determined by the file extension.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory {}", parent.display()))?;
            }
        }

        let format = Self::detect_format(path);
        let content = Self::serialize_for_format(self, format)?;

        // Atomic write
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &content)
            .with_context(|| format!("Failed to write settings to {}", tmp_path.display()))?;
        fs::rename(&tmp_path, path)
            .with_context(|| format!("Failed to rename settings to {}", path.display()))?;

        Ok(())
    }

    /// Save settings to the project-local config file.
    ///
    /// Uses the format of the existing file if present, otherwise saves as JSON.
    pub fn save_project(&self, project_dir: &std::path::Path) -> Result<()> {
        let dir = project_dir.join(".oxi");

        if !dir.exists() {
            fs::create_dir_all(&dir).with_context(|| {
                format!(
                    "Failed to create project settings directory {}",
                    dir.display()
                )
            })?;
        }

        // Check if a settings file already exists in project
        let json_path = dir.join("settings.json");
        let toml_path = dir.join("settings.toml");

        let path = if json_path.exists() {
            &json_path
        } else if toml_path.exists() {
            &toml_path
        } else {
            // Default to JSON for new files
            &json_path
        };

        let format = Self::detect_format(path);
        let content = Self::serialize_for_format(self, format)?;

        // Atomic write
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &content)
            .with_context(|| format!("Failed to write settings to {}", tmp_path.display()))?;
        fs::rename(&tmp_path, path)
            .with_context(|| format!("Failed to rename settings to {}", path.display()))?;

        Ok(())
    }

    /// Serialize settings to a string in the specified format.
    pub fn serialize_for_format(settings: &Settings, format: SettingsFormat) -> Result<String> {
        match format {
            SettingsFormat::Toml => {
                toml::to_string_pretty(settings).context("Failed to serialize settings to TOML")
            }
            SettingsFormat::Json => serde_json::to_string_pretty(settings)
                .context("Failed to serialize settings to JSON"),
        }
    }

    /// Parse settings from a string in the specified format.
    pub fn parse_from_str(content: &str, format: SettingsFormat) -> Result<Settings> {
        match format {
            SettingsFormat::Toml => {
                toml::from_str(content).context("Failed to parse TOML settings")
            }
            SettingsFormat::Json => {
                serde_json::from_str(content).context("Failed to parse JSON settings")
            }
        }
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
    /// Combines `default_provider` + `default_model` when both are set.
    /// Returns None if no model is configured.
    pub fn effective_model(&self, cli_model: Option<&str>) -> Option<String> {
        cli_model
            .map(String::from)
            .or_else(|| {
                // Combine provider + model when both are present
                if let (Some(provider), Some(model)) = (&self.default_provider, &self.default_model) {
                    Some(format!("{}/{}", provider, model))
                } else {
                    self.default_model.clone()
                }
            })
            .or_else(|| self.last_used_model.clone())
    }

    /// Get the effective provider.
    /// Returns None if no provider is configured.
    pub fn effective_provider(&self, cli_provider: Option<&str>) -> Option<String> {
        cli_provider
            .map(String::from)
            .or_else(|| self.default_provider.clone())
            .or_else(|| self.last_used_provider.clone())
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
        self.max_response_tokens
            .or(self.max_tokens.map(|t| t as usize))
    }

    // ── Theme persistence ─────────────────────────────────────────────

    /// Save the last used model/provider and persist to disk.
    pub fn save_last_used(model_id: &str) {
        if let Ok(mut settings) = Self::load() {
            let parts: Vec<&str> = model_id.splitn(2, '/').collect();
            settings.last_used_model = Some(model_id.to_string());
            settings.last_used_provider = parts.first().map(|s| s.to_string());
            let _ = settings.save();
        }
    }


    /// Save the current theme to settings and persist to disk.
    pub fn save_theme(&mut self, name: &str) -> Result<()> {
        self.theme = name.to_string();
        self.save()
    }

    /// Get the theme name from settings, returning a default if not set.
    pub fn get_theme_name(&self) -> String {
        if self.theme.is_empty() || self.theme == "default" {
            "oxi_dark".to_string()
        } else {
            self.theme.clone()
        }
    }

    // ── Migration ────────────────────────────────────────────────────

    /// Migrate settings from an older format version to the current one.
    ///
    /// Currently handles:
    /// - Version 0 → Version 2 (adds JSON support, version bump)
    /// - Version 1 → Version 2 (adds JSON support)
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
            1 | 2 => {
                // Version 1/2 → 4: dynamic_models field added + model/provider split.
                settings.version = SETTINGS_VERSION;
                tracing::info!(
                    "Migrated settings from version {} to {}",
                    settings.version, SETTINGS_VERSION
                );
            }
            3 => {
                // Version 3 → 4: split default_model "provider/model" into separate fields.
                if let Some(model) = settings.default_model.take() {
                    if let Some((provider, model_name)) = model.split_once('/') {
                        settings.default_provider = Some(provider.to_string());
                        settings.default_model = Some(model_name.to_string());
                    } else {
                        // No slash — keep as-is (bare model name)
                        settings.default_model = Some(model);
                    }
                }
                settings.version = SETTINGS_VERSION;
                tracing::info!(
                    "Migrated settings from version 3 to {} (split default_model into provider + model)",
                    SETTINGS_VERSION
                );
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

// ── Settings format detection ──────────────────────────────────────

/// Supported settings file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsFormat {
    /// JSON format.
    #[default]
    Json,
    /// TOML format.
    Toml,
}

impl SettingsFormat {
    /// Get the file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            SettingsFormat::Json => "json",
            SettingsFormat::Toml => "toml",
        }
    }
}

// ── JSON/TOML conversion helpers ────────────────────────────────────

/// Convert a TOML Value to a serde_json::Value.
fn toml_value_to_json(toml: toml::Value) -> serde_json::Value {
    match toml {
        toml::Value::String(s) => serde_json::Value::String(s),
        toml::Value::Integer(i) => serde_json::Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(toml_value_to_json).collect())
        }
        toml::Value::Table(table) => {
            let obj = table
                .into_iter()
                .map(|(k, v)| (k, toml_value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
    }
}

/// Deep merge two JSON values. The second value overrides the first.
fn merge_json_values(base: serde_json::Value, override_: serde_json::Value) -> serde_json::Value {
    match (base, override_) {
        // If either is not an object, the override wins
        (serde_json::Value::Object(base_map), serde_json::Value::Object(override_map)) => {
            let mut result = base_map;
            for (key, override_value) in override_map {
                let base_value = result.remove(&key);
                let merged = match base_value {
                    Some(base_v) => merge_json_values(base_v, override_value),
                    None => override_value,
                };
                result.insert(key, merged);
            }
            serde_json::Value::Object(result)
        }
        // Override wins for non-objects
        (_, override_) => override_,
    }
}

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
    use std::sync::Mutex;

    /// Global lock to serialize all tests that manipulate process-wide env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard that removes listed env vars on creation and restores them on drop.
    /// This prevents parallel test races where one test sets an env var that leaks into another.
    struct EnvGuard {
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new(vars: &[&str]) -> Self {
            let saved = vars
                .iter()
                .map(|&name| {
                    let old = env::var(name).ok();
                    env::remove_var(name);
                    (name.to_string(), old)
                })
                .collect();
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, old) in self.saved.drain(..) {
                match old {
                    Some(val) => env::set_var(&name, val),
                    None => env::remove_var(&name),
                }
            }
        }
    }

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
        settings.default_model = Some("gpt-4o".to_string());

        settings.merge_cli(Some("claude".to_string()), None);
        assert_eq!(settings.default_model, Some("claude".to_string()));

        settings.merge_cli(None, Some("google".to_string()));
        assert_eq!(settings.default_provider, Some("google".to_string()));
    }

    // ── Layered loading ──────────────────────────────────────────────

    #[test]
    fn test_layer_file_overrides() {
        let base = Settings::default();

        let tmp = tempfile::NamedTempFile::with_suffix(".toml").unwrap();
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

        let tmp = tempfile::NamedTempFile::with_suffix(".toml").unwrap();
        // Only override theme — provider should remain
        let toml_content = "theme = \"monokai\"\n";
        tmp.as_file().write_all(toml_content.as_bytes()).unwrap();

        let merged = Settings::layer_file(&base, tmp.path()).unwrap();
        assert_eq!(merged.theme, "monokai");
        assert_eq!(merged.default_provider, Some("deepseek".to_string()));
    }

    #[test]
    fn test_load_from_dir_with_project_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "OXI_MODEL",
            "OXI_PROVIDER",
            "OXI_THEME",
            "OXI_TOOL_TIMEOUT",
            "OXI_TEMPERATURE",
            "OXI_MAX_TOKENS",
            "OXI_SESSION_DIR",
            "OXI_STREAM",
            "OXI_EXTENSIONS_ENABLED",
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();
        let settings_path = oxi_dir.join("settings.toml");
        // Write v3 format: default_model contains "provider/model"
        fs::write(
            &settings_path,
            "version = 3\ndefault_model = \"google/gemini-2.0-flash\"\n",
        )
        .unwrap();

        let settings = Settings::load_from(tmp.path()).unwrap();
        // Migration splits provider from model
        assert_eq!(
            settings.default_model,
            Some("gemini-2.0-flash".to_string())
        );
        assert_eq!(
            settings.default_provider,
            Some("google".to_string())
        );
    }

    #[test]
    fn test_load_from_dir_no_config() {
        // Clean env vars that load_from() reads via apply_env()
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "OXI_MODEL",
            "OXI_PROVIDER",
            "OXI_THEME",
            "OXI_TOOL_TIMEOUT",
            "OXI_TEMPERATURE",
            "OXI_MAX_TOKENS",
            "OXI_SESSION_DIR",
            "OXI_STREAM",
            "OXI_EXTENSIONS_ENABLED",
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings::load_from(tmp.path()).unwrap();
        // Falls back to defaults (may include global ~/.oxi/settings)
        assert_eq!(settings.thinking_level, ThinkingLevel::Standard);
    }

    // ── Environment variables ────────────────────────────────────────

    #[test]
    fn test_from_env() {
        let _guard = EnvGuard::new(&["OXI_MODEL", "OXI_THEME", "OXI_TOOL_TIMEOUT"]);
        env::set_var("OXI_MODEL", "anthropic/claude-haiku-4-20250414");
        env::set_var("OXI_THEME", "nord");
        env::set_var("OXI_TOOL_TIMEOUT", "60");

        let settings = Settings::from_env();
        assert_eq!(
            settings.default_model,
            Some("anthropic/claude-haiku-4-20250414".to_string())
        );
        assert_eq!(settings.theme, "nord");
        assert_eq!(settings.tool_timeout_seconds, 60);
    }

    #[test]
    fn test_apply_env_boolish() {
        let _guard = EnvGuard::new(&["OXI_STREAM", "OXI_EXTENSIONS_ENABLED"]);
        env::set_var("OXI_STREAM", "false");
        env::set_var("OXI_EXTENSIONS_ENABLED", "0");

        let mut settings = Settings::default();
        settings.apply_env();
        assert!(!settings.stream_responses);
        assert!(!settings.extensions_enabled);
    }

    #[test]
    fn test_apply_env_temperature() {
        let _guard = EnvGuard::new(&["OXI_TEMPERATURE"]);
        env::set_var("OXI_TEMPERATURE", "0.7");

        let mut settings = Settings::default();
        settings.apply_env();
        assert_eq!(settings.default_temperature, Some(0.7));
    }

    #[test]
    fn test_env_does_not_override_when_unset() {
        let _guard = EnvGuard::new(&["OXI_MODEL", "OXI_PROVIDER"]);
        let settings = Settings::from_env();
        assert!(settings.default_model.is_none());
        assert!(settings.default_provider.is_none());
    }

    // ── Helpers ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_thinking_level() {
        assert_eq!(parse_thinking_level("none"), Some(ThinkingLevel::None));
        assert_eq!(
            parse_thinking_level("MINIMAL"),
            Some(ThinkingLevel::Minimal)
        );
        assert_eq!(
            parse_thinking_level("Standard"),
            Some(ThinkingLevel::Standard)
        );
        assert_eq!(
            parse_thinking_level("thorough"),
            Some(ThinkingLevel::Thorough)
        );
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
    fn test_effective_model_combines_provider_and_model() {
        let mut settings = Settings::default();
        settings.default_provider = Some("openai".to_string());
        settings.default_model = Some("gpt-4o".to_string());
        assert_eq!(settings.effective_model(None), Some("openai/gpt-4o".to_string()));
    }

    #[test]
    fn test_effective_model_cli_overrides() {
        let mut settings = Settings::default();
        settings.default_provider = Some("openai".to_string());
        settings.default_model = Some("gpt-4o".to_string());
        assert_eq!(settings.effective_model(Some("anthropic/claude-3")), Some("anthropic/claude-3".to_string()));
    }

    #[test]
    fn test_effective_model_no_provider_returns_bare() {
        let mut settings = Settings::default();
        settings.default_model = Some("gpt-4o".to_string());
        assert_eq!(settings.effective_model(None), Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_effective_model_falls_back_to_last_used() {
        let mut settings = Settings::default();
        settings.last_used_model = Some("anthropic/claude-3".to_string());
        assert_eq!(settings.effective_model(None), Some("anthropic/claude-3".to_string()));
    }

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
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["OXI_SESSION_DIR"]);
        let settings = Settings::default();
        let dir = settings.effective_session_dir().unwrap();
        assert!(dir.ends_with("sessions"), "dir was: {:?}", dir);
    }

    #[test]
    fn test_effective_session_dir_from_field() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["OXI_SESSION_DIR"]);
        let mut settings = Settings::default();
        settings.session_dir = Some(PathBuf::from("/tmp/oxi-sessions"));
        assert_eq!(
            settings.effective_session_dir().unwrap(),
            PathBuf::from("/tmp/oxi-sessions")
        );
    }

    #[test]
    fn test_effective_session_dir_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["OXI_SESSION_DIR"]);
        env::set_var("OXI_SESSION_DIR", "/tmp/env-sessions");
        let settings = Settings::default();
        assert_eq!(
            settings.effective_session_dir().unwrap(),
            PathBuf::from("/tmp/env-sessions")
        );
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
    fn test_migration_v3_to_v4_splits_model() {
        let mut settings = Settings::default();
        settings.version = 3;
        settings.default_model = Some("openai/gpt-4o".to_string());
        settings.default_provider = None;

        let migrated = Settings::migrate(settings).unwrap();
        assert_eq!(migrated.version, SETTINGS_VERSION);
        assert_eq!(migrated.default_model, Some("gpt-4o".to_string()));
        assert_eq!(migrated.default_provider, Some("openai".to_string()));
    }

    #[test]
    fn test_migration_v3_no_slash_keeps_model() {
        let mut settings = Settings::default();
        settings.version = 3;
        settings.default_model = Some("bare-model-name".to_string());

        let migrated = Settings::migrate(settings).unwrap();
        assert_eq!(migrated.version, SETTINGS_VERSION);
        assert_eq!(migrated.default_model, Some("bare-model-name".to_string()));
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
        original.default_model = Some("gpt-4o".to_string());
        original.default_provider = Some("openai".to_string());
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

    // ── JSON format tests ──────────────────────────────────────────────

    #[test]
    fn test_json_roundtrip() {
        let mut settings = Settings::default();
        settings.default_model = Some("gpt-4o".to_string());
        settings.default_provider = Some("openai".to_string());
        settings.theme = "dracula".to_string();
        settings.tool_timeout_seconds = 60;
        settings.default_temperature = Some(0.8);
        settings.max_response_tokens = Some(8192);

        let json_str = serde_json::to_string_pretty(&settings).unwrap();
        let parsed: Settings = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.default_model, settings.default_model);
        assert_eq!(parsed.theme, settings.theme);
        assert_eq!(parsed.tool_timeout_seconds, settings.tool_timeout_seconds);
        assert_eq!(parsed.default_temperature, settings.default_temperature);
        assert_eq!(parsed.max_response_tokens, settings.max_response_tokens);
    }

    #[test]
    fn test_json_serialize_for_format() {
        let mut settings = Settings::default();
        settings.default_model = Some("claude-3".to_string());
        settings.default_provider = Some("anthropic".to_string());
        settings.thinking_level = ThinkingLevel::Minimal;

        let json_content = Settings::serialize_for_format(&settings, SettingsFormat::Json).unwrap();
        let parsed: Settings = serde_json::from_str(&json_content).unwrap();

        assert_eq!(parsed.default_model, Some("claude-3".to_string()));
        assert_eq!(parsed.thinking_level, ThinkingLevel::Minimal);
    }

    #[test]
    fn test_toml_serialize_for_format() {
        let mut settings = Settings::default();
        settings.default_model = Some("gemini-pro".to_string());
        settings.default_provider = Some("google".to_string());
        settings.thinking_level = ThinkingLevel::Thorough;

        let toml_content = Settings::serialize_for_format(&settings, SettingsFormat::Toml).unwrap();
        let parsed: Settings = toml::from_str(&toml_content).unwrap();

        assert_eq!(parsed.default_model, Some("gemini-pro".to_string()));
        assert_eq!(parsed.thinking_level, ThinkingLevel::Thorough);
    }

    #[test]
    fn test_parse_from_str_json() {
        let json_content = r#"{
            "default_model": "gpt-4",
            "default_provider": "openai",
            "theme": "nord",
            "tool_timeout_seconds": 90
        }"#;

        let settings = Settings::parse_from_str(json_content, SettingsFormat::Json).unwrap();
        assert_eq!(settings.default_model, Some("gpt-4".to_string()));
        assert_eq!(settings.default_provider, Some("openai".to_string()));
        assert_eq!(settings.theme, "nord");
        assert_eq!(settings.tool_timeout_seconds, 90);
        // Unchanged fields retain defaults
        assert_eq!(settings.thinking_level, ThinkingLevel::Standard);
        assert!(settings.extensions_enabled);
    }

    #[test]
    fn test_parse_from_str_toml() {
        let toml_content = r#"
default_model = "claude-opus"
default_provider = "anthropic"
theme = "monokai"
tool_timeout_seconds = 45
"#;

        let settings = Settings::parse_from_str(toml_content, SettingsFormat::Toml).unwrap();
        assert_eq!(
            settings.default_model,
            Some("claude-opus".to_string())
        );
        assert_eq!(settings.default_provider, Some("anthropic".to_string()));
        assert_eq!(settings.theme, "monokai");
        assert_eq!(settings.tool_timeout_seconds, 45);
        assert_eq!(settings.thinking_level, ThinkingLevel::Standard);
    }

    #[test]
    fn test_layer_file_json() {
        let base = Settings::default();

        let tmp = tempfile::NamedTempFile::with_suffix(".json").unwrap();
        let json_content = r#"{
            "default_model": "gpt-4o",
            "default_provider": "openai",
            "theme": "dracula",
            "auto_compaction": false
        }"#;
        tmp.as_file().write_all(json_content.as_bytes()).unwrap();

        let merged = Settings::layer_file(&base, tmp.path()).unwrap();
        assert_eq!(merged.default_model, Some("gpt-4o".to_string()));
        assert_eq!(merged.default_provider, Some("openai".to_string()));
        assert_eq!(merged.theme, "dracula");
        assert!(!merged.auto_compaction);
        // Unchanged fields retain defaults
        assert_eq!(merged.thinking_level, ThinkingLevel::Standard);
        assert!(merged.extensions_enabled);
        assert_eq!(merged.tool_timeout_seconds, 120);
    }

    #[test]
    fn test_layer_file_json_preserves_unset() {
        let mut base = Settings::default();
        base.default_provider = Some("deepseek".to_string());

        let tmp = tempfile::NamedTempFile::with_suffix(".json").unwrap();
        let json_content = r#"{ "theme": "nord" }"#;
        tmp.as_file().write_all(json_content.as_bytes()).unwrap();

        let merged = Settings::layer_file(&base, tmp.path()).unwrap();
        assert_eq!(merged.theme, "nord");
        assert_eq!(merged.default_provider, Some("deepseek".to_string()));
    }

    #[test]
    fn test_save_to_json() {
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = tmp.path().join("settings.json");

        let mut settings = Settings::default();
        settings.default_model = Some("gpt-4o".to_string());
        settings.default_provider = Some("openai".to_string());
        settings.theme = "dracula".to_string();
        settings.tool_timeout_seconds = 60;

        settings.save_to(&settings_path).unwrap();

        // Verify it's valid JSON
        let content = fs::read_to_string(&settings_path).unwrap();
        let parsed: Settings = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.default_model, Some("gpt-4o".to_string()));
        assert_eq!(parsed.theme, "dracula");
        assert_eq!(parsed.tool_timeout_seconds, 60);
    }

    #[test]
    fn test_save_to_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = tmp.path().join("settings.toml");

        let mut settings = Settings::default();
        settings.default_model = Some("gemini-pro".to_string());
        settings.default_provider = Some("google".to_string());
        settings.theme = "monokai".to_string();
        settings.tool_timeout_seconds = 90;

        settings.save_to(&settings_path).unwrap();

        // Verify it's valid TOML
        let content = fs::read_to_string(&settings_path).unwrap();
        let parsed: Settings = toml::from_str(&content).unwrap();
        assert_eq!(parsed.default_model, Some("gemini-pro".to_string()));
        assert_eq!(parsed.theme, "monokai");
        assert_eq!(parsed.tool_timeout_seconds, 90);
    }

    #[test]
    fn test_load_from_dir_with_json_project_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "OXI_MODEL",
            "OXI_PROVIDER",
            "OXI_THEME",
            "OXI_TOOL_TIMEOUT",
            "OXI_TEMPERATURE",
            "OXI_MAX_TOKENS",
            "OXI_SESSION_DIR",
            "OXI_STREAM",
            "OXI_EXTENSIONS_ENABLED",
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();
        let settings_path = oxi_dir.join("settings.json");
        // v3 format: default_model has provider/model
        let json_content = r#"{ "version": 3, "default_model": "google/gemini-2.0-flash" }"#;
        fs::write(&settings_path, json_content).unwrap();

        let settings = Settings::load_from(tmp.path()).unwrap();
        // Migration splits provider from model
        assert_eq!(
            settings.default_model,
            Some("gemini-2.0-flash".to_string())
        );
        assert_eq!(
            settings.default_provider,
            Some("google".to_string())
        );
    }

    #[test]
    fn test_find_project_settings_json_priority() {
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();

        // Create both files
        let json_path = oxi_dir.join("settings.json");
        let toml_path = oxi_dir.join("settings.toml");
        fs::write(&json_path, r#"{ "theme": "json-theme" }"#).unwrap();
        fs::write(&toml_path, r#"theme = "toml-theme""#).unwrap();

        // JSON takes priority
        let found = Settings::find_project_settings(tmp.path());
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().file_name().unwrap().to_str().unwrap(),
            "settings.json"
        );
    }

    #[test]
    fn test_find_project_settings_json_only() {
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();

        let json_path = oxi_dir.join("settings.json");
        fs::write(&json_path, r#"{ "theme": "test" }"#).unwrap();

        let found = Settings::find_project_settings(tmp.path());
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().file_name().unwrap().to_str().unwrap(),
            "settings.json"
        );
    }

    #[test]
    fn test_find_project_settings_toml_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();

        let toml_path = oxi_dir.join("settings.toml");
        fs::write(&toml_path, r#"theme = "test""#).unwrap();

        let found = Settings::find_project_settings(tmp.path());
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().file_name().unwrap().to_str().unwrap(),
            "settings.toml"
        );
    }

    #[test]
    fn test_detect_format() {
        let json_path = PathBuf::from("/test/settings.json");
        let toml_path = PathBuf::from("/test/settings.toml");
        let unknown_path = PathBuf::from("/test/settings");

        assert_eq!(Settings::detect_format(&json_path), SettingsFormat::Json);
        assert_eq!(Settings::detect_format(&toml_path), SettingsFormat::Toml);
        assert_eq!(Settings::detect_format(&unknown_path), SettingsFormat::Json);
        // Default
    }

    #[test]
    fn test_settings_format_extension() {
        assert_eq!(SettingsFormat::Json.extension(), "json");
        assert_eq!(SettingsFormat::Toml.extension(), "toml");
    }

    #[test]
    fn test_layer_json_over_toml() {
        // Test that when loading, JSON takes priority over TOML
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();

        let json_path = oxi_dir.join("settings.json");
        let toml_path = oxi_dir.join("settings.toml");

        // JSON has model set to "json-model"
        fs::write(&json_path, r#"{ "default_model": "json-model" }"#).unwrap();
        // TOML has model set to "toml-model"
        fs::write(&toml_path, r#"default_model = "toml-model""#).unwrap();

        // JSON takes priority
        let settings = Settings::load_from(tmp.path()).unwrap();
        assert_eq!(settings.default_model, Some("json-model".to_string()));
    }

    #[test]
    fn test_mixed_format_loading() {
        // Test loading a TOML file through the generic layer_file
        let tmp = tempfile::NamedTempFile::with_suffix(".toml").unwrap();
        let toml_content = r#"
default_model = "loaded-via-toml"
theme = "loaded-theme"
stream_responses = false
"#;
        tmp.as_file().write_all(toml_content.as_bytes()).unwrap();

        let merged = Settings::layer_file(&Settings::default(), tmp.path()).unwrap();
        assert_eq!(merged.default_model, Some("loaded-via-toml".to_string()));
        assert_eq!(merged.theme, "loaded-theme");
        assert!(!merged.stream_responses);
    }

    #[test]
    fn test_merge_json_values() {
        let base = serde_json::json!({
            "version": 1,
            "theme": "default",
            "extensions": ["ext1"],
            "nested": {
                "a": 1,
                "b": 2
            }
        });

        let override_ = serde_json::json!({
            "version": 2,
            "theme": "dark",
            "extensions": ["ext2"],
            "nested": {
                "b": 20,
                "c": 30
            }
        });

        let merged = merge_json_values(base, override_);

        assert_eq!(merged["version"], 2);
        assert_eq!(merged["theme"], "dark");
        // Arrays are replaced, not merged
        assert_eq!(merged["extensions"], serde_json::json!(["ext2"]));
        // Nested objects are deeply merged
        assert_eq!(merged["nested"]["a"], 1);
        assert_eq!(merged["nested"]["b"], 20);
        assert_eq!(merged["nested"]["c"], 30);
    }

    #[test]
    fn test_save_project_preserves_existing_format() {
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();

        // Create existing TOML file
        let toml_path = oxi_dir.join("settings.toml");
        fs::write(&toml_path, "theme = 'old-theme'").unwrap();

        let mut settings = Settings::default();
        settings.theme = "new-theme".to_string();
        settings.save_project(tmp.path()).unwrap();

        // Should still be TOML
        let content = fs::read_to_string(&toml_path).unwrap();
        assert!(content.contains("new-theme"));
        assert!(serde_json::from_str::<serde_json::Value>(&content).is_err());
    }

    #[test]
    fn test_save_project_creates_json_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let oxi_dir = tmp.path().join(".oxi");
        fs::create_dir_all(&oxi_dir).unwrap();
        // Don't create any settings file

        let mut settings = Settings::default();
        settings.theme = "json-theme".to_string();
        settings.save_project(tmp.path()).unwrap();

        // Should create JSON file
        let json_path = oxi_dir.join("settings.json");
        assert!(json_path.exists());
        let content = fs::read_to_string(&json_path).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&content).is_ok());
        assert!(content.contains("json-theme"));
    }

    // ── Custom provider tests ───────────────────────────────────────

    #[test]
    fn test_custom_provider_default_api() {
        use super::CustomProvider;
        let cp = CustomProvider {
            name: "test".to_string(),
            base_url: "https://api.test.com/v1".to_string(),
            api_key_env: "TEST_API_KEY".to_string(),
            api: super::default_custom_provider_api(),
        };
        assert_eq!(cp.api, "openai-completions");
    }

    #[test]
    fn test_custom_provider_toml_deserialize() {
        let toml_content = r#"
[[custom_providers]]
name = "minimax"
base_url = "https://api.minimax.chat/v1"
api_key_env = "MINIMAX_API_KEY"
api = "openai-completions"

[[custom_providers]]
name = "zai"
base_url = "https://api.z.ai/v1"
api_key_env = "ZAI_API_KEY"
api = "openai-responses"
"#;
        let settings: Settings = toml::from_str(toml_content).unwrap();
        assert_eq!(settings.custom_providers.len(), 2);
        assert_eq!(settings.custom_providers[0].name, "minimax");
        assert_eq!(settings.custom_providers[0].base_url, "https://api.minimax.chat/v1");
        assert_eq!(settings.custom_providers[0].api_key_env, "MINIMAX_API_KEY");
        assert_eq!(settings.custom_providers[0].api, "openai-completions");
        assert_eq!(settings.custom_providers[1].name, "zai");
        assert_eq!(settings.custom_providers[1].api, "openai-responses");
    }

    #[test]
    fn test_custom_provider_json_deserialize() {
        let json_content = r#"{
            "custom_providers": [
                {
                    "name": "minimax",
                    "base_url": "https://api.minimax.chat/v1",
                    "api_key_env": "MINIMAX_API_KEY",
                    "api": "openai-completions"
                }
            ]
        }"#;
        let settings: Settings = serde_json::from_str(json_content).unwrap();
        assert_eq!(settings.custom_providers.len(), 1);
        assert_eq!(settings.custom_providers[0].name, "minimax");
    }

    #[test]
    fn test_custom_provider_toml_roundtrip() {
        let mut settings = Settings::default();
        settings.custom_providers.push(super::CustomProvider {
            name: "test".to_string(),
            base_url: "https://api.test.com/v1".to_string(),
            api_key_env: "TEST_API_KEY".to_string(),
            api: "openai-completions".to_string(),
        });

        let toml_str = toml::to_string_pretty(&settings).unwrap();
        let parsed: Settings = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.custom_providers.len(), 1);
        assert_eq!(parsed.custom_providers[0].name, "test");
        assert_eq!(parsed.custom_providers[0].base_url, "https://api.test.com/v1");
    }

    #[test]
    fn test_custom_provider_defaults_empty() {
        let settings = Settings::default();
        assert!(settings.custom_providers.is_empty());
    }

    #[test]
    fn test_custom_provider_layer_file() {
        let base = Settings::default();

        let tmp = tempfile::NamedTempFile::with_suffix(".toml").unwrap();
        let toml_content = r#"
[[custom_providers]]
name = "my-provider"
base_url = "https://api.my-provider.com/v1"
api_key_env = "MY_PROVIDER_API_KEY"
"#;
        tmp.as_file().write_all(toml_content.as_bytes()).unwrap();

        let merged = Settings::layer_file(&base, tmp.path()).unwrap();
        assert_eq!(merged.custom_providers.len(), 1);
        assert_eq!(merged.custom_providers[0].name, "my-provider");
        // Default api value
        assert_eq!(merged.custom_providers[0].api, "openai-completions");
    }
}
