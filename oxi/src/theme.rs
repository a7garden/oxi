//! Theme system for oxi TUI
//!
//! Provides theme management with dark/light mode support,
//! color schemes, and custom theme loading from ~/.oxi/themes/

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// Color representation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Self { r, g, b })
    }

    pub fn to_rgb_tuple(&self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }
}

/// Predefined theme colors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeColorRole {
    Primary,
    Secondary,
    Accent,
    Background,
    Foreground,
    Border,
    Error,
    Warning,
    Success,
    Muted,
    Link,
}

impl ThemeColorRole {
    pub fn name(&self) -> &'static str {
        match self {
            ThemeColorRole::Primary => "primary",
            ThemeColorRole::Secondary => "secondary",
            ThemeColorRole::Accent => "accent",
            ThemeColorRole::Background => "background",
            ThemeColorRole::Foreground => "foreground",
            ThemeColorRole::Border => "border",
            ThemeColorRole::Error => "error",
            ThemeColorRole::Warning => "warning",
            ThemeColorRole::Success => "success",
            ThemeColorRole::Muted => "muted",
            ThemeColorRole::Link => "link",
        }
    }
}

/// A complete theme definition
#[derive(Debug, Clone)]
pub struct Theme {
    /// Theme name
    pub name: String,
    /// Whether this is a dark theme
    pub is_dark: bool,
    /// Color definitions (role -> RGB)
    pub colors: HashMap<String, Color>,
    /// Optional path to the theme file
    pub source_path: Option<PathBuf>,
}

impl Theme {
    /// Create a new theme
    pub fn new(name: &str, is_dark: bool) -> Self {
        Self {
            name: name.to_string(),
            is_dark,
            colors: HashMap::new(),
            source_path: None,
        }
    }

    /// Get a color by role name
    pub fn get_color(&self, role: &str) -> Option<Color> {
        self.colors.get(role).cloned()
    }

    /// Set a color
    pub fn set_color(&mut self, role: &str, color: Color) {
        self.colors.insert(role.to_string(), color);
    }

    /// Build from JSON (for theme file loading)
    pub fn from_json(name: &str, json: &serde_json::Value) -> Option<Self> {
        let is_dark = json
            .get("isDark")
            .or_else(|| json.get("is_dark"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let mut theme = Self::new(name, is_dark);

        if let Some(colors) = json.get("colors").and_then(|v| v.as_object()) {
            for (key, value) in colors {
                if let Some(color_str) = value.as_str() {
                    if let Some(color) = Color::from_hex(color_str) {
                        theme.set_color(key, color);
                    }
                } else if let Some(rgb) = value.as_array() {
                    if rgb.len() >= 3 {
                        let r = rgb[0].as_u64()? as u8;
                        let g = rgb[1].as_u64()? as u8;
                        let b = rgb[2].as_u64()? as u8;
                        theme.set_color(key, Color::new(r, g, b));
                    }
                }
            }
        }

        Some(theme)
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("name".to_string(), serde_json::Value::String(self.name.clone()));
        obj.insert(
            "isDark".to_string(),
            serde_json::Value::Bool(self.is_dark),
        );

        let mut colors = serde_json::Map::new();
        for (key, color) in &self.colors {
            colors.insert(
                key.clone(),
                serde_json::Value::String(format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)),
            );
        }
        obj.insert("colors".to_string(), serde_json::Value::Object(colors));

        serde_json::Value::Object(obj)
    }
}

/// Built-in dark theme
pub fn dark_theme() -> Theme {
    let mut theme = Theme::new("dark", true);

    // Background colors
    theme.set_color("background", Color::new(30, 30, 30));
    theme.set_color("foreground", Color::new(220, 220, 220));
    theme.set_color("primary", Color::new(100, 180, 255));
    theme.set_color("secondary", Color::new(150, 150, 150));
    theme.set_color("accent", Color::new(80, 200, 120));
    theme.set_color("border", Color::new(60, 60, 60));
    theme.set_color("muted", Color::new(100, 100, 100));
    theme.set_color("error", Color::new(255, 80, 80));
    theme.set_color("warning", Color::new(255, 200, 80));
    theme.set_color("success", Color::new(80, 200, 120));
    theme.set_color("link", Color::new(100, 180, 255));

    theme
}

/// Built-in light theme
pub fn light_theme() -> Theme {
    let mut theme = Theme::new("light", false);

    // Background colors
    theme.set_color("background", Color::new(255, 255, 255));
    theme.set_color("foreground", Color::new(30, 30, 30));
    theme.set_color("primary", Color::new(0, 100, 200));
    theme.set_color("secondary", Color::new(100, 100, 100));
    theme.set_color("accent", Color::new(0, 150, 80));
    theme.set_color("border", Color::new(200, 200, 200));
    theme.set_color("muted", Color::new(150, 150, 150));
    theme.set_color("error", Color::new(200, 40, 40));
    theme.set_color("warning", Color::new(200, 150, 0));
    theme.set_color("success", Color::new(0, 150, 80));
    theme.set_color("link", Color::new(0, 100, 200));

    theme
}

/// Theme manager
pub struct ThemeManager {
    themes: HashMap<String, Theme>,
    current_theme: RwLock<String>,
}

impl ThemeManager {
    /// Create a new theme manager with built-in themes
    pub fn new() -> Self {
        let mut manager = Self {
            themes: HashMap::new(),
            current_theme: RwLock::new("dark".to_string()),
        };

        manager.register_theme(dark_theme());
        manager.register_theme(light_theme());

        manager
    }

    /// Register a theme
    pub fn register_theme(&mut self, theme: Theme) {
        self.themes.insert(theme.name.clone(), theme);
    }

    /// Get a theme by name
    pub fn get_theme(&self, name: &str) -> Option<&Theme> {
        self.themes.get(name)
    }

    /// Get current theme
    pub fn get_current_theme(&self) -> Option<Theme> {
        let name = self.current_theme.read().unwrap().clone();
        self.themes.get(&name).cloned()
    }

    /// Set current theme
    pub fn set_current_theme(&self, name: &str) -> bool {
        if self.themes.contains_key(name) {
            *self.current_theme.write().unwrap() = name.to_string();
            true
        } else {
            false
        }
    }

    /// Get all registered theme names
    pub fn theme_names(&self) -> Vec<String> {
        self.themes.keys().cloned().collect()
    }

    /// Load a theme from a file
    pub fn load_theme_from_file(&mut self, path: &PathBuf) -> Option<Theme> {
        let content = std::fs::read_to_string(path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;

        let name = json
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("custom");

        let mut theme = Theme::from_json(name, &json)?;
        theme.source_path = Some(path.clone());

        self.register_theme(theme.clone());
        Some(theme)
    }

    /// Load all themes from ~/.oxi/themes/
    pub fn load_user_themes(&mut self) -> Vec<Theme> {
        let mut loaded = Vec::new();

        if let Some(home) = std::env::var_os("HOME") {
            let themes_dir = PathBuf::from(home).join(".oxi").join("themes");

            if themes_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(themes_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().map(|e| e == "json").unwrap_or(false) {
                            if let Some(theme) = self.load_theme_from_file(&path) {
                                loaded.push(theme);
                            }
                        }
                    }
                }
            }
        }

        loaded
    }

    /// Toggle between dark and light themes
    pub fn toggle_dark_light(&self) -> bool {
        let current = self.current_theme.read().unwrap().clone();
        let current_theme = self.themes.get(&current)?;

        let next_name = if current_theme.is_dark {
            "light"
        } else {
            "dark"
        };

        self.set_current_theme(next_name)
    }
}

impl Default for ThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

/// ANSI escape code helpers for TUI rendering
impl Theme {
    /// Get ANSI foreground escape code
    pub fn fg_ansi(&self, role: &str) -> String {
        self.get_color(role)
            .map(|c| format!("\x1b[38;2;{};{};{}m", c.r, c.g, c.b))
            .unwrap_or_else(|| "\x1b[39m".to_string())
    }

    /// Get ANSI background escape code
    pub fn bg_ansi(&self, role: &str) -> String {
        self.get_color(role)
            .map(|c| format!("\x1b[48;2;{};{};{}m", c.r, c.g, c.b))
            .unwrap_or_else(|| "\x1b[49m".to_string())
    }

    /// Get color as 256-color ANSI code (approximation)
    pub fn fg_ansi_256(&self, role: &str) -> String {
        self.get_color(role).map(|c| {
            let index = 16 + (c.r / 43) * 36 + (c.g / 43) * 6 + (c.b / 43);
            format!("\x1b[38;5;{}m", index)
        }).unwrap_or_else(|| "\x1b[39m".to_string())
    }

    /// Reset ANSI formatting
    pub fn reset_ansi() -> String {
        "\x1b[0m".to_string()
    }

    /// Bold formatting
    pub fn bold_ansi() -> String {
        "\x1b[1m".to_string()
    }

    /// Italic formatting
    pub fn italic_ansi() -> String {
        "\x1b[3m".to_string()
    }

    /// Underline formatting
    pub fn underline_ansi() -> String {
        "\x1b[4m".to_string()
    }
}

/// Global theme instance (for convenience)
lazy_static::lazy_static! {
    pub static ref GLOBAL_THEME: RwLock<Theme> = RwLock::new(dark_theme());
}

/// Set the global theme
pub fn set_global_theme(theme: Theme) {
    *GLOBAL_THEME.write().unwrap() = theme;
}

/// Get the global theme
pub fn get_global_theme() -> Theme {
    GLOBAL_THEME.read().unwrap().clone()
}

/// Format text with a theme color role
pub fn fmt_with_color(text: &str, role: &str) -> String {
    let theme = get_global_theme();
    format!("{}{}{}", theme.fg_ansi(role), text, Theme::reset_ansi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_from_hex() {
        let color = Color::from_hex("#FF5500").unwrap();
        assert_eq!(color.r, 255);
        assert_eq!(color.g, 85);
        assert_eq!(color.b, 0);
    }

    #[test]
    fn test_color_from_hex_with_hash() {
        let color = Color::from_hex("#AABBCC").unwrap();
        assert_eq!(color.r, 170);
        assert_eq!(color.g, 187);
        assert_eq!(color.b, 204);
    }

    #[test]
    fn test_color_invalid_hex() {
        assert!(Color::from_hex("invalid").is_none());
        assert!(Color::from_hex("#ABC").is_none()); // Too short
    }

    #[test]
    fn test_theme_new() {
        let theme = Theme::new("test", true);
        assert_eq!(theme.name, "test");
        assert!(theme.is_dark);
        assert!(theme.colors.is_empty());
    }

    #[test]
    fn test_theme_set_get_color() {
        let mut theme = Theme::new("test", true);
        theme.set_color("primary", Color::new(255, 0, 0));

        let color = theme.get_color("primary").unwrap();
        assert_eq!(color.r, 255);
        assert_eq!(color.g, 0);
        assert_eq!(color.b, 0);
    }

    #[test]
    fn test_dark_theme() {
        let theme = dark_theme();
        assert!(theme.is_dark);
        assert!(theme.get_color("background").is_some());
        assert!(theme.get_color("foreground").is_some());
    }

    #[test]
    fn test_light_theme() {
        let theme = light_theme();
        assert!(!theme.is_dark);
        assert!(theme.get_color("background").is_some());
        assert!(theme.get_color("foreground").is_some());
    }

    #[test]
    fn test_theme_from_json() {
        let json = serde_json::json!({
            "name": "custom",
            "isDark": false,
            "colors": {
                "primary": "#FF0000",
                "background": [10, 20, 30]
            }
        });

        let theme = Theme::from_json("custom", &json).unwrap();
        assert_eq!(theme.name, "custom");
        assert!(!theme.is_dark);
        assert_eq!(theme.get_color("primary").unwrap().r, 255);
        assert_eq!(theme.get_color("background").unwrap().r, 10);
    }

    #[test]
    fn test_theme_to_json() {
        let mut theme = Theme::new("test", true);
        theme.set_color("primary", Color::new(255, 128, 64));

        let json = theme.to_json();
        assert_eq!(json["name"], "test");
        assert!(json["isDark"].as_bool().unwrap());
    }

    #[test]
    fn test_theme_manager() {
        let mut manager = ThemeManager::new();

        // Check built-in themes
        assert!(manager.get_theme("dark").is_some());
        assert!(manager.get_theme("light").is_some());
        assert!(manager.get_theme("nonexistent").is_none());

        // Check current theme
        assert_eq!(manager.get_current_theme().unwrap().name, "dark");

        // Set and get
        manager.set_current_theme("light");
        assert_eq!(manager.get_current_theme().unwrap().name, "light");

        // Toggle
        manager.toggle_dark_light();
        assert_eq!(manager.get_current_theme().unwrap().name, "dark");
    }

    #[test]
    fn test_ansi_codes() {
        let theme = dark_theme();

        let fg = theme.fg_ansi("primary");
        assert!(fg.starts_with("\x1b[38;2;"));

        let bg = theme.bg_ansi("background");
        assert!(bg.starts_with("\x1b[48;2;"));

        let reset = Theme::reset_ansi();
        assert_eq!(reset, "\x1b[0m");
    }

    #[test]
    fn test_global_theme() {
        set_global_theme(light_theme());
        let theme = get_global_theme();
        assert!(!theme.is_dark);
    }

    #[test]
    fn test_fmt_with_color() {
        let formatted = fmt_with_color("test", "primary");
        assert!(formatted.starts_with("\x1b[38;2;"));
        assert!(formatted.contains("test"));
        assert!(formatted.ends_with("\x1b[0m"));
    }
}