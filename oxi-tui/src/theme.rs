//! Theme system for oxi-tui.
//!
//! Provides customizable color schemes, font styles, and spacing.
//! Includes built-in dark and light themes, and supports loading
//! themes from TOML or JSON files with hot-reloading.

use crate::cell::{Attributes, Color};
use ratatui::style::Style;
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Core theme types
// ---------------------------------------------------------------------------

/// A complete theme definition.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Human-readable theme name.
    pub name: String,
    /// Color palette.
    pub colors: ColorScheme,
    /// Font / text style scheme.
    pub fonts: FontScheme,
    /// Spacing configuration.
    pub spacing: Spacing,
}

// ---------------------------------------------------------------------------
// Color scheme
// ---------------------------------------------------------------------------

/// Semantic color palette used by components.
#[derive(Clone, Debug)]
pub struct ColorScheme {
    /// Normal text foreground color.
    pub foreground: Color,
    /// Default background color.
    pub background: Color,
    /// Primary accent color (UI elements, labels, user "You").
    pub primary: Color,
    /// Secondary color (alternative accents).
    pub secondary: Color,
    /// Error / danger color.
    pub error: Color,
    /// Warning / caution color.
    pub warning: Color,
    /// Success / confirmation color.
    pub success: Color,
    /// Muted / dimmed text (e.g. placeholders, tool headers).
    pub muted: Color,
    /// Accent highlight color.
    pub accent: Color,
    /// Border / separator color.
    pub border: Color,
    /// User message left-border accent (subtle).
    pub user_border: Color,
    /// User message background (subtle tint).
    pub user_bg: Color,
    /// Cursor foreground.
    pub cursor_fg: Color,
    /// Cursor background.
    pub cursor_bg: Color,
    /// Selection / highlight background.
    pub selection_bg: Color,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::dark()
    }
}

impl ColorScheme {
    /// Default dark color scheme (true black).
    pub fn dark() -> Self {
        Self {
            foreground: Color::Rgb(205, 214, 244), // #cdd6f4
            background: Color::Rgb(0, 0, 0),       // #000000 true black
            primary: Color::Rgb(122, 162, 247),    // #7aa2f7
            secondary: Color::Rgb(158, 206, 106),  // #9ece6a
            error: Color::Rgb(247, 118, 142),      // #f7768e
            warning: Color::Rgb(224, 175, 104),    // #e0af68
            success: Color::Rgb(158, 206, 106),    // #9ece6a
            muted: Color::Rgb(80, 80, 100),        // #505064
            accent: Color::Rgb(187, 154, 247),     // #bb9af7
            border: Color::Rgb(30, 30, 30),        // #1e1e1e
            user_border: Color::Rgb(122, 162, 247), // #7aa2f7 (matches primary)
            user_bg: Color::Rgb(18, 22, 38),        // #121626 subtle indigo tint
            cursor_fg: Color::Rgb(0, 0, 0),        // #000000
            cursor_bg: Color::Rgb(205, 214, 244),  // #cdd6f4
            selection_bg: Color::Rgb(40, 40, 60),  // #28283c
        }
    }

    /// Default light color scheme.
    pub fn light() -> Self {
        Self {
            foreground: Color::Rgb(76, 79, 105),   // #4c4f69
            background: Color::Rgb(239, 241, 245), // #eff1f5
            primary: Color::Rgb(30, 102, 240),     // #1e66f0
            secondary: Color::Rgb(64, 160, 43),   // #40a02b
            error: Color::Rgb(210, 15, 57),        // #d20f39
            warning: Color::Rgb(223, 142, 29),    // #df8e1d
            success: Color::Rgb(64, 160, 43),    // #40a02b
            muted: Color::Indexed(8),
            accent: Color::Rgb(136, 57, 239),     // #8839ef
            border: Color::Indexed(7),
            user_border: Color::Rgb(30, 102, 240), // #1e66f0 (matches primary)
            user_bg: Color::Rgb(225, 236, 255),   // #e1ecff subtle blue tint
            cursor_fg: Color::Rgb(239, 241, 245),
            cursor_bg: Color::Rgb(76, 79, 105),
            selection_bg: Color::Rgb(204, 208, 218),
        }
    }

    /// Convert to ratatui Style with just foreground.
    pub fn to_style(&self) -> Style {
        Style::default()
            .fg(self.foreground.to_ratatui())
            .bg(self.background.to_ratatui())
    }

    /// Convert to ratatui Style with all semantic colors.
    pub fn to_styles(&self) -> ThemeStyles {
        ThemeStyles {
            normal: Style::default().fg(self.foreground.to_ratatui()).bg(self.background.to_ratatui()),
            primary: Style::default().fg(self.primary.to_ratatui()),
            secondary: Style::default().fg(self.secondary.to_ratatui()),
            error: Style::default().fg(self.error.to_ratatui()),
            warning: Style::default().fg(self.warning.to_ratatui()),
            success: Style::default().fg(self.success.to_ratatui()),
            muted: Style::default().fg(self.muted.to_ratatui()),
            accent: Style::default().fg(self.accent.to_ratatui()),
            border: Style::default().fg(self.border.to_ratatui()),
            cursor_fg: Style::default().fg(self.cursor_fg.to_ratatui()),
            cursor_bg: Style::default().fg(self.cursor_bg.to_ratatui()),
            selection_bg: Style::default().bg(self.selection_bg.to_ratatui()),
        }
    }
}

/// Pre-computed ratatui styles for all semantic colors in a ColorScheme.
#[derive(Clone, Debug)]
pub struct ThemeStyles {
    /// Normal / default text style.
    pub normal: Style,
    /// Primary color style.
    pub primary: Style,
    /// Secondary color style.
    pub secondary: Style,
    /// Error / red style.
    pub error: Style,
    /// Warning / yellow style.
    pub warning: Style,
    /// Success / green style.
    pub success: Style,
    /// Muted / dimmed style.
    pub muted: Style,
    /// Accent / highlight style.
    pub accent: Style,
    /// Border / separator style.
    pub border: Style,
    /// Cursor foreground style.
    pub cursor_fg: Style,
    /// Cursor background style.
    pub cursor_bg: Style,
    /// Selection background style.
    pub selection_bg: Style,
}

// ---------------------------------------------------------------------------
// Font scheme
// ---------------------------------------------------------------------------

/// Text style scheme for semantic elements.
#[derive(Clone, Debug)]
pub struct FontScheme {
    /// Normal weight text.
    pub normal: Attributes,
    /// Bold text.
    pub bold: Attributes,
    /// Italic text.
    pub italic: Attributes,
    /// Bold italic text.
    pub bold_italic: Attributes,
    /// Heading text (bold).
    pub heading: Attributes,
    /// Link text (underlined).
    pub link: Attributes,
}

impl Default for FontScheme {
    fn default() -> Self {
        Self {
            normal: Attributes::new(),
            bold: Attributes::new().with_bold(),
            italic: Attributes::new().with_italic(),
            bold_italic: Attributes::new().with_bold().with_italic(),
            heading: Attributes::new().with_bold(),
            link: Attributes::new().with_underline(),
        }
    }
}

// ---------------------------------------------------------------------------
// Spacing
// ---------------------------------------------------------------------------

/// Spacing/padding configuration (in character cells).
#[derive(Clone, Debug, Copy)]
pub struct Spacing {
    /// Padding around content.
    pub padding: u16,
    /// Outer margin.
    pub margin: u16,
    /// Width of borders.
    pub border_width: u16,
    /// Extra line spacing.
    pub line_spacing: u16,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            padding: 1,
            margin: 0,
            border_width: 1,
            line_spacing: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in themes
// ---------------------------------------------------------------------------

impl Theme {
    /// Built-in dark theme.
    pub fn dark() -> Self {
        Self {
            name: "dark".into(),
            colors: ColorScheme::dark(),
            fonts: FontScheme::default(),
            spacing: Spacing::default(),
        }
    }

    /// Built-in light theme.
    pub fn light() -> Self {
        Self {
            name: "light".into(),
            colors: ColorScheme::light(),
            fonts: FontScheme::default(),
            spacing: Spacing::default(),
        }
    }

    /// Convert theme foreground/background to ratatui Style.
    pub fn to_style(&self) -> Style {
        self.colors.to_style()
    }

    /// Get all semantic styles as ThemeStyles.
    pub fn to_styles(&self) -> ThemeStyles {
        self.colors.to_styles()
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

// ---------------------------------------------------------------------------
// Theme loading from files (TOML / JSON)
// ---------------------------------------------------------------------------

/// Serializable representation of a theme file.
#[derive(Clone, Debug, Deserialize, Default)]
pub struct ThemeFile {
    /// Human-readable name of the theme.
    #[serde(default)]
    pub name: String,
    /// Color definitions.
    #[serde(default)]
    pub colors: ThemeFileColors,
}

/// Color overrides from a theme file.
#[derive(Clone, Debug, Deserialize, Default)]
pub struct ThemeFileColors {
    /// Foreground / text color.
    pub foreground: Option<String>,
    /// Background color.
    pub background: Option<String>,
    /// Primary accent color.
    pub primary: Option<String>,
    /// Secondary color.
    pub secondary: Option<String>,
    /// Error color.
    pub error: Option<String>,
    /// Warning color.
    pub warning: Option<String>,
    /// Success color.
    pub success: Option<String>,
    /// Muted / dimmed text color.
    pub muted: Option<String>,
    /// Accent highlight color.
    pub accent: Option<String>,
    /// Border / separator color.
    pub border: Option<String>,
    /// Cursor foreground color.
    pub cursor_fg: Option<String>,
    /// Cursor background color.
    pub cursor_bg: Option<String>,
    /// Selection background color.
    pub selection_bg: Option<String>,
}

impl ThemeFile {
    /// Load a theme from a TOML file.
    pub fn from_toml(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let theme: ThemeFile = toml::from_str(&content)?;
        Ok(theme)
    }

    /// Load a theme from a JSON file.
    pub fn from_json(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let theme: ThemeFile = serde_json::from_str(&content)?;
        Ok(theme)
    }

    /// Load from any supported format (detected by extension).
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("toml") => Self::from_toml(path),
            Some("json") => Self::from_json(path),
            _ => anyhow::bail!(
                "Unsupported theme file format: {:?}. Use .toml or .json",
                path.extension()
            ),
        }
    }

    /// Convert into a full Theme, using dark defaults for any missing fields.
    pub fn into_theme(self) -> Theme {
        let defaults = ColorScheme::dark();
        let colors = ColorScheme {
            foreground: self
                .colors
                .foreground
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.foreground),
            background: self
                .colors
                .background
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.background),
            primary: self
                .colors
                .primary
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.primary),
            secondary: self
                .colors
                .secondary
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.secondary),
            error: self
                .colors
                .error
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.error),
            warning: self
                .colors
                .warning
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.warning),
            success: self
                .colors
                .success
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.success),
            muted: self
                .colors
                .muted
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.muted),
            accent: self
                .colors
                .accent
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.accent),
            border: self
                .colors
                .border
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.border),
            cursor_fg: self
                .colors
                .cursor_fg
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.cursor_fg),
            cursor_bg: self
                .colors
                .cursor_bg
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.cursor_bg),
            selection_bg: self
                .colors
                .selection_bg
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.selection_bg),
        };
        Theme {
            name: if self.name.is_empty() {
                "custom".into()
            } else {
                self.name
            },
            colors,
            fonts: FontScheme::default(),
            spacing: Spacing::default(),
        }
    }
}

/// Parse a color string.
///
/// Accepted forms:
/// - Named: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`
/// - Bright named: `bright-black`, `bright-red`, ...
/// - Hex: `#rrggbb`
/// - Indexed: `i<N>` where N is 0–255
fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    // Hex
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    // Indexed
    if let Some(idx_str) = s.strip_prefix('i') {
        if let Ok(n) = idx_str.parse::<u8>() {
            return Some(Color::Indexed(n));
        }
    }
    // Named
    match s.to_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "bright-black" | "brightblack" | "gray" | "grey" => Some(Color::Indexed(8)),
        "bright-red" | "brightred" => Some(Color::Indexed(9)),
        "bright-green" | "brightgreen" => Some(Color::Indexed(10)),
        "bright-yellow" | "brightyellow" => Some(Color::Indexed(11)),
        "bright-blue" | "brightblue" => Some(Color::Indexed(12)),
        "bright-magenta" | "brightmagenta" => Some(Color::Indexed(13)),
        "bright-cyan" | "brightcyan" => Some(Color::Indexed(14)),
        "bright-white" | "brightwhite" => Some(Color::Indexed(15)),
        "default" => Some(Color::Default),
        _ => None,
    }
}

fn parse_hex(hex: &str) -> Option<Color> {
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        3 => {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Theme manager with hot-reload
// ---------------------------------------------------------------------------

/// Manages the active theme and optionally watches a theme file for changes.
pub struct ThemeManager {
    /// Currently active theme.
    theme: Arc<parking_lot::RwLock<Theme>>,
    /// Optional file being watched.
    watch_path: Option<PathBuf>,
    /// Last known modification time.
    last_modified: Option<std::time::SystemTime>,
    /// Polling interval for file changes.
    poll_interval: std::time::Duration,
    /// Instant of last poll.
    last_poll: Instant,
}

impl ThemeManager {
    /// Create a new manager with the given theme.
    pub fn new(theme: Theme) -> Self {
        Self {
            theme: Arc::new(parking_lot::RwLock::new(theme)),
            watch_path: None,
            last_modified: None,
            poll_interval: std::time::Duration::from_secs(1),
            last_poll: Instant::now(),
        }
    }

    /// Create a manager that starts with the default dark theme.
    pub fn dark() -> Self {
        Self::new(Theme::dark())
    }

    /// Create a manager that starts with the default light theme.
    pub fn light() -> Self {
        Self::new(Theme::light())
    }

    /// Start watching a theme file for changes.
    ///
    /// The file format is auto-detected from the extension (`.toml` or `.json`).
    /// On each call to [`ThemeManager::check_reload`], the file's mtime is
    /// compared to the last known value; if it changed, the theme is reloaded.
    pub fn watch_file(&mut self, path: impl Into<PathBuf>) -> anyhow::Result<()> {
        let path = path.into();
        // Immediately load the theme
        let file = ThemeFile::load(&path)?;
        let theme = file.into_theme();
        *self.theme.write() = theme;
        self.last_modified = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        self.watch_path = Some(path);
        Ok(())
    }

    /// Get a clone of the current theme.
    pub fn theme(&self) -> Theme {
        self.theme.read().clone()
    }

    /// Get a handle to the shared theme lock.
    pub fn theme_handle(&self) -> Arc<parking_lot::RwLock<Theme>> {
        Arc::clone(&self.theme)
    }

    /// Replace the active theme.
    pub fn set_theme(&self, theme: Theme) {
        *self.theme.write() = theme;
    }

    /// Set the active theme by name ("dark" or "light").
    ///
    /// Returns `true` if the name was recognized.
    pub fn set_theme_by_name(&self, name: &str) -> bool {
        let theme = match name {
            "dark" => Theme::dark(),
            "light" => Theme::light(),
            _ => return false,
        };
        self.set_theme(theme);
        true
    }

    /// Check if the watched file has changed and reload if so.
    ///
    /// Call this periodically (e.g. once per event-loop tick).
    /// Returns `true` if the theme was reloaded.
    pub fn check_reload(&mut self) -> bool {
        let path = match &self.watch_path {
            Some(p) => p.clone(),
            None => return false,
        };

        // Throttle polling
        if self.last_poll.elapsed() < self.poll_interval {
            return false;
        }
        self.last_poll = Instant::now();

        let current_mtime = match std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
        {
            Some(t) => t,
            None => return false,
        };

        let changed = match self.last_modified {
            Some(prev) => current_mtime > prev,
            None => true,
        };

        if changed {
            match ThemeFile::load(&path) {
                Ok(file) => {
                    let theme = file.into_theme();
                    *self.theme.write() = theme;
                    self.last_modified = Some(current_mtime);
                    tracing::info!("Theme reloaded from {:?}", path);
                    true
                }
                Err(e) => {
                    tracing::warn!("Failed to reload theme from {:?}: {}", path, e);
                    false
                }
            }
        } else {
            false
        }
    }

    /// Set the polling interval for file watching (default 1s).
    pub fn set_poll_interval(&mut self, interval: std::time::Duration) {
        self.poll_interval = interval;
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Theme({})", self.name)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_is_dark() {
        let theme = Theme::default();
        assert_eq!(theme.name, "dark");
    }

    #[test]
    fn dark_theme_has_light_foreground() {
        let theme = Theme::dark();
        // foreground should be a light color
        match theme.colors.foreground {
            Color::Rgb(r, _, _) => assert!(r > 200, "dark theme foreground should be light"),
            _ => panic!("expected Rgb foreground"),
        }
    }

    #[test]
    fn light_theme_has_dark_foreground() {
        let theme = Theme::light();
        match theme.colors.foreground {
            Color::Rgb(r, _, _) => assert!(r < 150, "light theme foreground should be dark"),
            _ => panic!("expected Rgb foreground"),
        }
    }

    #[test]
    fn parse_hex_colors() {
        assert_eq!(parse_color("#ff8800"), Some(Color::Rgb(255, 136, 0)));
        assert_eq!(parse_color("#f80"), Some(Color::Rgb(255, 136, 0)));
    }

    #[test]
    fn parse_named_colors() {
        assert_eq!(parse_color("red"), Some(Color::Red));
        assert_eq!(parse_color("bright-black"), Some(Color::Indexed(8)));
        assert_eq!(parse_color("default"), Some(Color::Default));
    }

    #[test]
    fn parse_indexed_color() {
        assert_eq!(parse_color("i42"), Some(Color::Indexed(42)));
    }

    #[test]
    fn theme_manager_set_by_name() {
        let mgr = ThemeManager::dark();
        assert!(mgr.set_theme_by_name("light"));
        assert_eq!(mgr.theme().name, "light");
        assert!(!mgr.set_theme_by_name("nonexistent"));
        assert_eq!(mgr.theme().name, "light");
    }

    #[test]
    fn theme_file_from_json() {
        let json = r##"{"name":"test","colors":{"foreground":"#ffffff","background":"#000000"}}"##;
        let file: ThemeFile = serde_json::from_str(json).unwrap();
        let theme = file.into_theme();
        assert_eq!(theme.name, "test");
        assert_eq!(theme.colors.foreground, Color::Rgb(255, 255, 255));
        assert_eq!(theme.colors.background, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn theme_file_roundtrip() {
        let dir = std::env::temp_dir().join("oxi-tui-theme-test");
        std::fs::create_dir_all(&dir).unwrap();

        let json_path = dir.join("test_theme.json");
        std::fs::write(
            &json_path,
            r##"{"name":"mytheme","colors":{"primary":"#ff0000"}}"##,
        )
        .unwrap();
        let file = ThemeFile::load(&json_path).unwrap();
        let theme = file.into_theme();
        assert_eq!(theme.name, "mytheme");
        assert_eq!(theme.colors.primary, Color::Rgb(255, 0, 0));
        // Other fields get dark defaults
        assert!(matches!(theme.colors.foreground, Color::Rgb(_, _, _)));

        std::fs::remove_dir_all(&dir).ok();
    }
}
