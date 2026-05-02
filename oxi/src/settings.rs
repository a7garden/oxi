//! Settings management for oxi CLI
//!
//! Settings are persisted to ~/.oxi/settings.toml

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
    /// Thinking level for agent responses
    pub thinking_level: ThinkingLevel,
    /// Color theme (e.g., "default", "monokai", "dracula")
    pub theme: String,
    /// Default model to use
    pub default_model: Option<String>,
    /// Default provider to use
    pub default_provider: Option<String>,
    /// Max tokens for responses
    pub max_tokens: Option<u32>,
    /// Temperature for generation (0.0-1.0)
    pub temperature: Option<f32>,
    /// Session history size
    pub session_history_size: usize,
    /// Whether to stream responses
    pub stream_responses: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            thinking_level: ThinkingLevel::Standard,
            theme: String::from("default"),
            default_model: None,
            default_provider: None,
            max_tokens: None,
            temperature: None,
            session_history_size: 100,
            stream_responses: true,
        }
    }
}

impl Settings {
    /// Get the settings directory path (~/.oxi)
    pub fn settings_dir() -> Result<PathBuf> {
        let base = dirs::home_dir().context("Cannot determine home directory")?;
        Ok(base.join(".oxi"))
    }

    /// Get the settings file path (~/.oxi/settings.toml)
    pub fn settings_path() -> Result<PathBuf> {
        Ok(Self::settings_dir()?.join("settings.toml"))
    }

    /// Load settings from disk
    pub fn load() -> Result<Self> {
        let path = Self::settings_path()?;
        
        if !path.exists() {
            // Return defaults if no settings file exists
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read settings from {}", path.display()))?;

        toml::from_str(&content)
            .with_context(|| format!("Failed to parse settings from {}", path.display()))
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<()> {
        let dir = Self::settings_dir()?;
        let path = Self::settings_path()?;

        // Create settings directory if needed
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create settings directory {}", dir.display()))?;
        }

        // Serialize and write
        let content = toml::to_string_pretty(self)
            .context("Failed to serialize settings")?;

        fs::write(&path, content)
            .with_context(|| format!("Failed to write settings to {}", path.display()))?;

        Ok(())
    }

    /// Merge with CLI arguments (CLI takes precedence)
    pub fn merge_cli(&mut self, model: Option<String>, provider: Option<String>) {
        if let Some(m) = model {
            self.default_model = Some(m);
        }
        if let Some(p) = provider {
            self.default_provider = Some(p);
        }
    }

    /// Get the effective model ID (provider/model format)
    pub fn effective_model(&self, cli_model: Option<&str>) -> String {
        cli_model
            .map(String::from)
            .or_else(|| self.default_model.clone())
            .unwrap_or_else(|| "anthropic/claude-sonnet-4-20250514".to_string())
    }

    /// Get the effective provider
    pub fn effective_provider(&self, cli_provider: Option<&str>) -> String {
        cli_provider
            .map(String::from)
            .or_else(|| self.default_provider.clone())
            .unwrap_or_else(|| "anthropic".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.thinking_level, ThinkingLevel::Standard);
        assert_eq!(settings.theme, "default");
        assert!(settings.default_model.is_none());
        assert!(settings.default_provider.is_none());
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
}