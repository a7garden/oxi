//! Theme system for oxi TUI
//!
//! Provides theme management with dark/light mode support,
//! color schemes, font styles, spacing, borders, and custom theme loading.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use parking_lot::RwLock;

/// Color representation (RGB values)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Create a new RGB color
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse from hex string like "#FF5500" or "FF5500"
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

    /// Convert to hex string (without # prefix)
    pub fn to_hex(&self) -> String {
        format!("{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Convert to RGB tuple
    pub fn to_rgb_tuple(&self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }

    /// Get ANSI 24-bit foreground escape code
    pub fn fg_ansi(&self) -> String {
        format!("\x1b[38;2;{};{};{}m", self.r, self.g, self.b)
    }

    /// Get ANSI 24-bit background escape code
    pub fn bg_ansi(&self) -> String {
        format!("\x1b[48;2;{};{};{}m", self.r, self.g, self.b)
    }

    /// Get ANSI 256-color index (approximation)
    pub fn to_ansi256(&self) -> u8 {
        16 + (self.r / 43) * 36 + (self.g / 43) * 6 + (self.b / 43)
    }

    /// Get ANSI 256-color foreground escape code
    pub fn fg_ansi_256(&self) -> String {
        format!("\x1b[38;5;{}m", self.to_ansi256())
    }

    /// Get ANSI 256-color background escape code
    pub fn bg_ansi_256(&self) -> String {
        format!("\x1b[48;5;{}m", self.to_ansi256())
    }

    /// Check if this is a dark color (for luminance calculation)
    pub fn is_dark(&self) -> bool {
        // Perceived luminance formula
        let luminance = 0.299 * self.r as f32 + 0.587 * self.g as f32 + 0.114 * self.b as f32;
        luminance < 128.0
    }
}

impl Default for Color {
    fn default() -> Self {
        Self { r: 0, g: 0, b: 0 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Theme colors (complete color palette)
// ─────────────────────────────────────────────────────────────────────────────

/// Complete color palette for themes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeColors {
    // Backgrounds
    pub bg_primary: Color,
    pub bg_secondary: Color,
    pub bg_tertiary: Color,
    pub bg_hover: Color,
    pub bg_active: Color,
    pub bg_selected: Color,
    // Foregrounds
    pub fg_primary: Color,
    pub fg_secondary: Color,
    pub fg_tertiary: Color,
    pub fg_dim: Color,
    // Accents
    pub accent_blue: Color,
    pub accent_green: Color,
    pub accent_yellow: Color,
    pub accent_red: Color,
    pub accent_purple: Color,
    // Syntax
    pub syntax_keyword: Color,
    pub syntax_string: Color,
    pub syntax_comment: Color,
    pub syntax_function: Color,
    // Special
    pub border: Color,
    pub cursor: Color,
    pub selection: Color,
    pub scrollbar: Color,
}

impl ThemeColors {
    /// Create dark theme colors
    pub fn dark() -> Self {
        Self {
            // Backgrounds
            bg_primary: Color::new(30, 30, 36),
            bg_secondary: Color::new(40, 40, 50),
            bg_tertiary: Color::new(50, 50, 65),
            bg_hover: Color::new(55, 55, 70),
            bg_active: Color::new(60, 60, 80),
            bg_selected: Color::new(45, 45, 60),
            // Foregrounds
            fg_primary: Color::new(220, 223, 228),
            fg_secondary: Color::new(180, 180, 195),
            fg_tertiary: Color::new(130, 130, 150),
            fg_dim: Color::new(90, 90, 110),
            // Accents
            accent_blue: Color::new(137, 180, 250),
            accent_green: Color::new(166, 227, 161),
            accent_yellow: Color::new(249, 226, 175),
            accent_red: Color::new(243, 139, 168),
            accent_purple: Color::new(203, 166, 247),
            // Syntax
            syntax_keyword: Color::new(203, 166, 247),
            syntax_string: Color::new(166, 227, 161),
            syntax_comment: Color::new(127, 132, 156),
            syntax_function: Color::new(137, 180, 250),
            // Special
            border: Color::new(60, 60, 75),
            cursor: Color::new(220, 223, 228),
            selection: Color::new(80, 80, 100),
            scrollbar: Color::new(45, 45, 55),
        }
    }

    /// Create light theme colors
    pub fn light() -> Self {
        Self {
            // Backgrounds
            bg_primary: Color::new(239, 241, 245),
            bg_secondary: Color::new(229, 233, 240),
            bg_tertiary: Color::new(219, 224, 234),
            bg_hover: Color::new(210, 215, 228),
            bg_active: Color::new(200, 205, 220),
            bg_selected: Color::new(205, 210, 225),
            // Foregrounds
            fg_primary: Color::new(76, 79, 105),
            fg_secondary: Color::new(106, 110, 135),
            fg_tertiary: Color::new(130, 135, 155),
            fg_dim: Color::new(150, 155, 175),
            // Accents
            accent_blue: Color::new(30, 102, 240),
            accent_green: Color::new(64, 160, 43),
            accent_yellow: Color::new(223, 142, 29),
            accent_red: Color::new(210, 15, 57),
            accent_purple: Color::new(136, 57, 239),
            // Syntax
            syntax_keyword: Color::new(136, 57, 239),
            syntax_string: Color::new(64, 160, 43),
            syntax_comment: Color::new(140, 145, 165),
            syntax_function: Color::new(30, 102, 240),
            // Special
            border: Color::new(180, 185, 200),
            cursor: Color::new(76, 79, 105),
            selection: Color::new(180, 190, 210),
            scrollbar: Color::new(160, 165, 175),
        }
    }

    /// Create Nord theme colors
    pub fn nord() -> Self {
        Self {
            // Backgrounds
            bg_primary: Color::new(46, 52, 64),
            bg_secondary: Color::new(59, 66, 82),
            bg_tertiary: Color::new(67, 76, 94),
            bg_hover: Color::new(76, 86, 106),
            bg_active: Color::new(85, 96, 118),
            bg_selected: Color::new(67, 76, 94),
            // Foregrounds
            fg_primary: Color::new(236, 239, 244),
            fg_secondary: Color::new(191, 197, 208),
            fg_tertiary: Color::new(146, 155, 170),
            fg_dim: Color::new(110, 118, 129),
            // Accents
            accent_blue: Color::new(136, 192, 208),
            accent_green: Color::new(143, 193, 123),
            accent_yellow: Color::new(235, 203, 139),
            accent_red: Color::new(191, 97, 106),
            accent_purple: Color::new(180, 142, 173),
            // Syntax
            syntax_keyword: Color::new(136, 192, 208),
            syntax_string: Color::new(143, 193, 123),
            syntax_comment: Color::new(129, 138, 156),
            syntax_function: Color::new(136, 192, 208),
            // Special
            border: Color::new(67, 76, 94),
            cursor: Color::new(236, 239, 244),
            selection: Color::new(88, 102, 122),
            scrollbar: Color::new(67, 76, 94),
        }
    }

    /// Create Catppuccin Mocha theme colors
    pub fn catppuccin() -> Self {
        Self {
            // Backgrounds
            bg_primary: Color::new(30, 30, 46),
            bg_secondary: Color::new(37, 37, 55),
            bg_tertiary: Color::new(44, 44, 64),
            bg_hover: Color::new(51, 51, 73),
            bg_active: Color::new(58, 58, 82),
            bg_selected: Color::new(48, 48, 68),
            // Foregrounds
            fg_primary: Color::new(205, 214, 244),
            fg_secondary: Color::new(186, 195, 214),
            fg_tertiary: Color::new(166, 173, 200),
            fg_dim: Color::new(137, 143, 170),
            // Accents
            accent_blue: Color::new(137, 176, 174),
            accent_green: Color::new(166, 227, 161),
            accent_yellow: Color::new(249, 226, 175),
            accent_red: Color::new(243, 139, 168),
            accent_purple: Color::new(203, 166, 247),
            // Syntax
            syntax_keyword: Color::new(203, 166, 247),
            syntax_string: Color::new(166, 227, 161),
            syntax_comment: Color::new(127, 132, 156),
            syntax_function: Color::new(137, 176, 174),
            // Special
            border: Color::new(51, 51, 73),
            cursor: Color::new(205, 214, 244),
            selection: Color::new(68, 68, 92),
            scrollbar: Color::new(48, 48, 68),
        }
    }

    /// Create GitHub Dark theme colors
    pub fn github_dark() -> Self {
        Self {
            // Backgrounds
            bg_primary: Color::new(13, 17, 23),
            bg_secondary: Color::new(22, 27, 34),
            bg_tertiary: Color::new(33, 38, 45),
            bg_hover: Color::new(48, 54, 61),
            bg_active: Color::new(56, 62, 70),
            bg_selected: Color::new(39, 44, 52),
            // Foregrounds
            fg_primary: Color::new(201, 209, 217),
            fg_secondary: Color::new(170, 178, 193),
            fg_tertiary: Color::new(139, 148, 158),
            fg_dim: Color::new(110, 118, 129),
            // Accents
            accent_blue: Color::new(88, 166, 255),
            accent_green: Color::new(63, 185, 80),
            accent_yellow: Color::new(210, 168, 52),
            accent_red: Color::new(248, 81, 73),
            accent_purple: Color::new(188, 140, 255),
            // Syntax
            syntax_keyword: Color::new(188, 140, 255),
            syntax_string: Color::new(170, 178, 193),
            syntax_comment: Color::new(110, 118, 129),
            syntax_function: Color::new(88, 166, 255),
            // Special
            border: Color::new(48, 54, 61),
            cursor: Color::new(201, 209, 217),
            selection: Color::new(56, 62, 70),
            scrollbar: Color::new(48, 54, 61),
        }
    }

    /// Create Monokai theme colors
    pub fn monokai() -> Self {
        Self {
            // Backgrounds
            bg_primary: Color::new(39, 40, 34),
            bg_secondary: Color::new(48, 49, 42),
            bg_tertiary: Color::new(57, 58, 50),
            bg_hover: Color::new(66, 67, 58),
            bg_active: Color::new(75, 76, 66),
            bg_selected: Color::new(55, 56, 48),
            // Foregrounds
            fg_primary: Color::new(248, 248, 240),
            fg_secondary: Color::new(210, 210, 200),
            fg_tertiary: Color::new(170, 170, 160),
            fg_dim: Color::new(130, 130, 120),
            // Accents
            accent_blue: Color::new(102, 217, 232),
            accent_green: Color::new(230, 217, 70),
            accent_yellow: Color::new(250, 220, 70),
            accent_red: Color::new(249, 38, 114),
            accent_purple: Color::new(218, 112, 214),
            // Syntax
            syntax_keyword: Color::new(249, 38, 114),
            syntax_string: Color::new(230, 217, 70),
            syntax_comment: Color::new(130, 130, 120),
            syntax_function: Color::new(102, 217, 232),
            // Special
            border: Color::new(66, 67, 58),
            cursor: Color::new(248, 248, 240),
            selection: Color::new(60, 61, 54),
            scrollbar: Color::new(50, 51, 45),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Font scheme
// ─────────────────────────────────────────────────────────────────────────────

/// Font style attributes
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FontScheme {
    pub normal: FontStyle,
    pub bold: FontStyle,
    pub italic: FontStyle,
    pub code: FontStyle,
}

impl FontScheme {
    pub fn default_scheme() -> Self {
        Self {
            normal: FontStyle::default(),
            bold: FontStyle {
                bold: true,
                ..Default::default()
            },
            italic: FontStyle {
                italic: true,
                ..Default::default()
            },
            code: FontStyle {
                normal: false,
                bold: false,
                italic: false,
                underline: false,
            },
        }
    }
}

/// Individual font style settings
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FontStyle {
    pub normal: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Spacing
// ─────────────────────────────────────────────────────────────────────────────

/// Spacing/padding configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spacing {
    pub padding: u16,
    pub margin: u16,
    pub border_width: u16,
    pub line_spacing: u16,
}

// ─────────────────────────────────────────────────────────────────────────────
// Border style
// ─────────────────────────────────────────────────────────────────────────────

/// Border style options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// No border
    None,
    /// Single line border
    #[default]
    Single,
    /// Double line border
    Double,
    /// Rounded corners
    Rounded,
    /// Bold border
    Bold,
}

impl BorderStyle {
    pub fn chars(&self) -> &'static str {
        match self {
            BorderStyle::None => "",
            BorderStyle::Single => "─│┌┐└┘├┤┬┴┼",
            BorderStyle::Double => "═║╔╗╚╝╠╣╦╩╬",
            BorderStyle::Rounded => "─│╭╮╰╯├┤┬┴┼",
            BorderStyle::Bold => "━┃┏┓┗┛┣┫┳━",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Theme struct
// ─────────────────────────────────────────────────────────────────────────────

/// A complete theme definition
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Theme name
    pub name: String,
    /// Optional author credit
    pub author: Option<String>,
    /// Color palette
    pub colors: ThemeColors,
    /// Optional font scheme
    pub font: Option<FontScheme>,
    /// Optional spacing configuration
    pub spacing: Option<Spacing>,
    /// Border style
    pub borders: BorderStyle,
    /// Whether this is a dark theme
    pub is_dark: bool,
    /// Optional path to the theme file
    pub source_path: Option<PathBuf>,
}

impl Theme {
    /// Create a new theme with default values
    pub fn new(name: &str, is_dark: bool) -> Self {
        Self {
            name: name.to_string(),
            author: None,
            colors: if is_dark { ThemeColors::dark() } else { ThemeColors::light() },
            font: Some(FontScheme::default_scheme()),
            spacing: Some(Spacing::default()),
            borders: BorderStyle::default(),
            is_dark,
            source_path: None,
        }
    }

    /// Create a theme from colors
    pub fn from_colors(name: &str, is_dark: bool, colors: ThemeColors) -> Self {
        Self {
            name: name.to_string(),
            author: None,
            colors,
            font: Some(FontScheme::default_scheme()),
            spacing: Some(Spacing::default()),
            borders: BorderStyle::default(),
            is_dark,
            source_path: None,
        }
    }

    /// Get the background color for a given role
    pub fn bg(&self, role: BgRole) -> &Color {
        match role {
            BgRole::Primary => &self.colors.bg_primary,
            BgRole::Secondary => &self.colors.bg_secondary,
            BgRole::Tertiary => &self.colors.bg_tertiary,
            BgRole::Hover => &self.colors.bg_hover,
            BgRole::Active => &self.colors.bg_active,
            BgRole::Selected => &self.colors.bg_selected,
        }
    }

    /// Get the foreground color for a given role
    pub fn fg(&self, role: FgRole) -> &Color {
        match role {
            FgRole::Primary => &self.colors.fg_primary,
            FgRole::Secondary => &self.colors.fg_secondary,
            FgRole::Tertiary => &self.colors.fg_tertiary,
            FgRole::Dim => &self.colors.fg_dim,
        }
    }

    /// Get the accent color for a given role
    pub fn accent(&self, role: AccentRole) -> &Color {
        match role {
            AccentRole::Blue => &self.colors.accent_blue,
            AccentRole::Green => &self.colors.accent_green,
            AccentRole::Yellow => &self.colors.accent_yellow,
            AccentRole::Red => &self.colors.accent_red,
            AccentRole::Purple => &self.colors.accent_purple,
        }
    }
}

/// Background color roles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgRole {
    Primary,
    Secondary,
    Tertiary,
    Hover,
    Active,
    Selected,
}

/// Foreground color roles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FgRole {
    Primary,
    Secondary,
    Tertiary,
    Dim,
}

/// Accent color roles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccentRole {
    Blue,
    Green,
    Yellow,
    Red,
    Purple,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Built-in themes
// ─────────────────────────────────────────────────────────────────────────────

impl Theme {
    /// Built-in dark theme
    pub fn oxi_dark() -> Self {
        Self::new("oxi_dark", true)
    }

    /// Built-in light theme
    pub fn oxi_light() -> Self {
        Self::new("oxi_light", false)
    }

    /// Nord theme
    pub fn nord() -> Self {
        Self::from_colors("nord", true, ThemeColors::nord())
    }

    /// Catppuccin Mocha theme
    pub fn catppuccin() -> Self {
        Self::from_colors("catppuccin", true, ThemeColors::catppuccin())
    }

    /// GitHub Dark theme
    pub fn github_dark() -> Self {
        Self::from_colors("github_dark", true, ThemeColors::github_dark())
    }

    /// Monokai theme
    pub fn monokai() -> Self {
        Self::from_colors("monokai", true, ThemeColors::monokai())
    }

    /// Get all built-in theme names
    pub fn built_in_names() -> Vec<&'static str> {
        vec![
            "oxi_dark",
            "oxi_light",
            "nord",
            "catppuccin",
            "github_dark",
            "monokai",
        ]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Theme loading from files (TOML)
// ─────────────────────────────────────────────────────────────────────────────

/// Theme file representation for serialization/deserialization
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ThemeFile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub is_dark: bool,
    #[serde(default)]
    pub colors: ThemeFileColors,
    #[serde(default)]
    pub font: Option<FontFileScheme>,
    #[serde(default)]
    pub spacing: Option<Spacing>,
    #[serde(default)]
    pub borders: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ThemeFileColors {
    #[serde(rename = "bg_primary")]
    pub bg_primary: Option<String>,
    #[serde(rename = "bg_secondary")]
    pub bg_secondary: Option<String>,
    #[serde(rename = "bg_tertiary")]
    pub bg_tertiary: Option<String>,
    #[serde(rename = "bg_hover")]
    pub bg_hover: Option<String>,
    #[serde(rename = "bg_active")]
    pub bg_active: Option<String>,
    #[serde(rename = "bg_selected")]
    pub bg_selected: Option<String>,
    #[serde(rename = "fg_primary")]
    pub fg_primary: Option<String>,
    #[serde(rename = "fg_secondary")]
    pub fg_secondary: Option<String>,
    #[serde(rename = "fg_tertiary")]
    pub fg_tertiary: Option<String>,
    #[serde(rename = "fg_dim")]
    pub fg_dim: Option<String>,
    #[serde(rename = "accent_blue")]
    pub accent_blue: Option<String>,
    #[serde(rename = "accent_green")]
    pub accent_green: Option<String>,
    #[serde(rename = "accent_yellow")]
    pub accent_yellow: Option<String>,
    #[serde(rename = "accent_red")]
    pub accent_red: Option<String>,
    #[serde(rename = "accent_purple")]
    pub accent_purple: Option<String>,
    #[serde(rename = "syntax_keyword")]
    pub syntax_keyword: Option<String>,
    #[serde(rename = "syntax_string")]
    pub syntax_string: Option<String>,
    #[serde(rename = "syntax_comment")]
    pub syntax_comment: Option<String>,
    #[serde(rename = "syntax_function")]
    pub syntax_function: Option<String>,
    pub border: Option<String>,
    pub cursor: Option<String>,
    pub selection: Option<String>,
    pub scrollbar: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct FontFileScheme {
    #[serde(default)]
    pub normal: Option<FontFileStyle>,
    #[serde(default)]
    pub bold: Option<FontFileStyle>,
    #[serde(default)]
    pub italic: Option<FontFileStyle>,
    #[serde(default)]
    pub code: Option<FontFileStyle>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct FontFileStyle {
    #[serde(default)]
    pub bold: Option<bool>,
    #[serde(default)]
    pub italic: Option<bool>,
    #[serde(default)]
    pub underline: Option<bool>,
}

impl ThemeFile {
    /// Load a theme from a TOML file
    pub fn from_toml(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let theme: ThemeFile = toml::from_str(&content)?;
        Ok(theme)
    }

    /// Load a theme from a JSON file
    pub fn from_json(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let theme: ThemeFile = serde_json::from_str(&content)?;
        Ok(theme)
    }

    /// Load from any supported format (detected by extension)
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

    /// Convert into a full Theme, using dark defaults for any missing fields
    pub fn into_theme(self) -> Theme {
        let defaults = ThemeColors::dark();
        Theme {
            name: if self.name.is_empty() {
                "custom".into()
            } else {
                self.name
            },
            author: self.author,
            colors: ThemeColors {
                bg_primary: self
                    .colors
                    .bg_primary
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.bg_primary),
                bg_secondary: self
                    .colors
                    .bg_secondary
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.bg_secondary),
                bg_tertiary: self
                    .colors
                    .bg_tertiary
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.bg_tertiary),
                bg_hover: self
                    .colors
                    .bg_hover
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.bg_hover),
                bg_active: self
                    .colors
                    .bg_active
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.bg_active),
                bg_selected: self
                    .colors
                    .bg_selected
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.bg_selected),
                fg_primary: self
                    .colors
                    .fg_primary
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.fg_primary),
                fg_secondary: self
                    .colors
                    .fg_secondary
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.fg_secondary),
                fg_tertiary: self
                    .colors
                    .fg_tertiary
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.fg_tertiary),
                fg_dim: self
                    .colors
                    .fg_dim
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.fg_dim),
                accent_blue: self
                    .colors
                    .accent_blue
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.accent_blue),
                accent_green: self
                    .colors
                    .accent_green
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.accent_green),
                accent_yellow: self
                    .colors
                    .accent_yellow
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.accent_yellow),
                accent_red: self
                    .colors
                    .accent_red
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.accent_red),
                accent_purple: self
                    .colors
                    .accent_purple
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.accent_purple),
                syntax_keyword: self
                    .colors
                    .syntax_keyword
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.syntax_keyword),
                syntax_string: self
                    .colors
                    .syntax_string
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.syntax_string),
                syntax_comment: self
                    .colors
                    .syntax_comment
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.syntax_comment),
                syntax_function: self
                    .colors
                    .syntax_function
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.syntax_function),
                border: self
                    .colors
                    .border
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.border),
                cursor: self
                    .colors
                    .cursor
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.cursor),
                selection: self
                    .colors
                    .selection
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.selection),
                scrollbar: self
                    .colors
                    .scrollbar
                    .as_ref()
                    .and_then(parse_color)
                    .unwrap_or(defaults.scrollbar),
            },
            font: Some(FontScheme::default_scheme()),
            spacing: self.spacing,
            borders: parse_border_style(self.borders.as_deref())
                .unwrap_or(BorderStyle::default()),
            is_dark: self.is_dark,
            source_path: None,
        }
    }

    /// Serialize to TOML string
    pub fn to_toml(&self) -> anyhow::Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// Parse a color string (#rrggbb, rgb(r,g,b), or named)
fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();

    // Hex format: #rrggbb or rrggbb
    if let Some(hex) = s.strip_prefix('#') {
        return Color::from_hex(hex);
    }

    // RGB format: rgb(r,g,b)
    if let Some(inner) = s.strip_prefix("rgb(") {
        if let Some(inner) = inner.strip_suffix(')') {
            let parts: Vec<&str> = inner.split(',').collect();
            if parts.len() == 3 {
                let r = parts[0].trim().parse::<u8>().ok()?;
                let g = parts[1].trim().parse::<u8>().ok()?;
                let b = parts[2].trim().parse::<u8>().ok()?;
                return Some(Color::new(r, g, b));
            }
        }
    }

    // Named colors
    match s.to_lowercase().as_str() {
        "black" => Some(Color::new(0, 0, 0)),
        "white" => Some(Color::new(255, 255, 255)),
        "red" => Some(Color::new(255, 0, 0)),
        "green" => Some(Color::new(0, 255, 0)),
        "blue" => Some(Color::new(0, 0, 255)),
        "yellow" => Some(Color::new(255, 255, 0)),
        "cyan" => Some(Color::new(0, 255, 255)),
        "magenta" => Some(Color::new(255, 0, 255)),
        "gray" | "grey" => Some(Color::new(128, 128, 128)),
        _ => None,
    }
}

/// Parse border style from string
fn parse_border_style(s: Option<&str>) -> Option<BorderStyle> {
    match s?.to_lowercase().as_str() {
        "none" => Some(BorderStyle::None),
        "single" => Some(BorderStyle::Single),
        "double" => Some(BorderStyle::Double),
        "rounded" => Some(BorderStyle::Rounded),
        "bold" => Some(BorderStyle::Bold),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Theme manager
// ─────────────────────────────────────────────────────────────────────────────

/// Manages theme loading, persistence, and the active theme
pub struct ThemeManager {
    themes: HashMap<String, Theme>,
    current_theme: RwLock<String>,
}

impl ThemeManager {
    /// Create a new theme manager with built-in themes
    pub fn new() -> Self {
        let mut manager = Self {
            themes: HashMap::new(),
            current_theme: RwLock::new("oxi_dark".to_string()),
        };

        // Register all built-in themes
        manager.register_theme(Theme::oxi_dark());
        manager.register_theme(Theme::oxi_light());
        manager.register_theme(Theme::nord());
        manager.register_theme(Theme::catppuccin());
        manager.register_theme(Theme::github_dark());
        manager.register_theme(Theme::monokai());

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
        let name = self.current_theme.read().clone();
        self.themes.get(&name).cloned()
    }

    /// Set current theme by name
    pub fn set_current_theme(&self, name: &str) -> bool {
        if self.themes.contains_key(name) {
            *self.current_theme.write() = name.to_string();
            true
        } else {
            false
        }
    }

    /// Get all registered theme names
    pub fn theme_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.themes.keys().cloned().collect();
        names.sort();
        names
    }

    /// Load a theme from a file
    pub fn load_theme_from_file(&mut self, path: &Path) -> Option<Theme> {
        let content = std::fs::read_to_string(path).ok()?;

        // Try TOML first, then JSON
        let theme_file = if path.extension().and_then(|e| e.to_str()) == Some("json") {
            serde_json::from_str::<ThemeFile>(&content).ok()
        } else {
            toml::from_str::<ThemeFile>(&content).ok()
        };

        let mut theme = theme_file?.into_theme();
        theme.source_path = Some(path.to_path_buf());

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
                        if path
                            .extension()
                            .map(|e| e == "toml" || e == "json")
                            .unwrap_or(false)
                        {
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

    /// Load themes from a specific directory
    pub fn load_themes_from_dir(&mut self, dir: &Path) -> Vec<Theme> {
        let mut loaded = Vec::new();

        if !dir.exists() {
            return loaded;
        }

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .map(|e| e == "toml" || e == "json")
                    .unwrap_or(false)
                {
                    if let Some(theme) = self.load_theme_from_file(&path) {
                        loaded.push(theme);
                    }
                }
            }
        }

        loaded
    }

    /// Toggle between dark and light themes
    pub fn toggle_dark_light(&self) -> bool {
        let current = self.current_theme.read().clone();
        let current_theme = self.themes.get(&current)?;

        let next_name = if current_theme.is_dark { "oxi_light" } else { "oxi_dark" };
        self.set_current_theme(next_name)
    }
}

impl Default for ThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global theme instance
// ─────────────────────────────────────────────────────────────────────────────

lazy_static::lazy_static! {
    /// Global theme instance
    pub static ref GLOBAL_THEME_MANAGER: ThemeManager = ThemeManager::new();
}

/// Get the global theme manager
pub fn get_theme_manager() -> &'static ThemeManager {
    &GLOBAL_THEME_MANAGER
}

/// Get a theme by name from the global manager
pub fn get_theme_by_name(name: &str) -> Option<Theme> {
    GLOBAL_THEME_MANAGER.get_theme(name).cloned()
}

/// Get the current global theme
pub fn get_global_theme() -> Theme {
    GLOBAL_THEME_MANAGER
        .get_current_theme()
        .unwrap_or_else(Theme::oxi_dark)
}

/// Set the global theme
pub fn set_global_theme(name: &str) -> bool {
    GLOBAL_THEME_MANAGER.set_current_theme(name)
}

// ─────────────────────────────────────────────────────────────────────────────
// ANSI formatting helpers
// ─────────────────────────────────────────────────────────────────────────────

impl Theme {
    /// Get ANSI foreground escape code for a color role
    pub fn fg_ansi(&self, role: &str) -> String {
        match role {
            "bg_primary" => self.colors.bg_primary.fg_ansi(),
            "bg_secondary" => self.colors.bg_secondary.fg_ansi(),
            "bg_tertiary" => self.colors.bg_tertiary.fg_ansi(),
            "bg_hover" => self.colors.bg_hover.fg_ansi(),
            "bg_active" => self.colors.bg_active.fg_ansi(),
            "bg_selected" => self.colors.bg_selected.fg_ansi(),
            "fg_primary" => self.colors.fg_primary.fg_ansi(),
            "fg_secondary" => self.colors.fg_secondary.fg_ansi(),
            "fg_tertiary" => self.colors.fg_tertiary.fg_ansi(),
            "fg_dim" => self.colors.fg_dim.fg_ansi(),
            "accent_blue" => self.colors.accent_blue.fg_ansi(),
            "accent_green" => self.colors.accent_green.fg_ansi(),
            "accent_yellow" => self.colors.accent_yellow.fg_ansi(),
            "accent_red" => self.colors.accent_red.fg_ansi(),
            "accent_purple" => self.colors.accent_purple.fg_ansi(),
            "syntax_keyword" => self.colors.syntax_keyword.fg_ansi(),
            "syntax_string" => self.colors.syntax_string.fg_ansi(),
            "syntax_comment" => self.colors.syntax_comment.fg_ansi(),
            "syntax_function" => self.colors.syntax_function.fg_ansi(),
            "border" => self.colors.border.fg_ansi(),
            "cursor" => self.colors.cursor.fg_ansi(),
            "selection" => self.colors.selection.fg_ansi(),
            "scrollbar" => self.colors.scrollbar.fg_ansi(),
            _ => "\x1b[39m".to_string(),
        }
    }

    /// Get ANSI background escape code for a color role
    pub fn bg_ansi(&self, role: &str) -> String {
        match role {
            "bg_primary" => self.colors.bg_primary.bg_ansi(),
            "bg_secondary" => self.colors.bg_secondary.bg_ansi(),
            "bg_tertiary" => self.colors.bg_tertiary.bg_ansi(),
            "bg_hover" => self.colors.bg_hover.bg_ansi(),
            "bg_active" => self.colors.bg_active.bg_ansi(),
            "bg_selected" => self.colors.bg_selected.bg_ansi(),
            "fg_primary" => self.colors.fg_primary.bg_ansi(),
            "fg_secondary" => self.colors.fg_secondary.bg_ansi(),
            "fg_tertiary" => self.colors.fg_tertiary.bg_ansi(),
            "fg_dim" => self.colors.fg_dim.bg_ansi(),
            "accent_blue" => self.colors.accent_blue.bg_ansi(),
            "accent_green" => self.colors.accent_green.bg_ansi(),
            "accent_yellow" => self.colors.accent_yellow.bg_ansi(),
            "accent_red" => self.colors.accent_red.bg_ansi(),
            "accent_purple" => self.colors.accent_purple.bg_ansi(),
            "syntax_keyword" => self.colors.syntax_keyword.bg_ansi(),
            "syntax_string" => self.colors.syntax_string.bg_ansi(),
            "syntax_comment" => self.colors.syntax_comment.bg_ansi(),
            "syntax_function" => self.colors.syntax_function.bg_ansi(),
            "border" => self.colors.border.bg_ansi(),
            "cursor" => self.colors.cursor.bg_ansi(),
            "selection" => self.colors.selection.bg_ansi(),
            "scrollbar" => self.colors.scrollbar.bg_ansi(),
            _ => "\x1b[49m".to_string(),
        }
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

    /// Format text with a foreground color role
    pub fn fmt_fg(&self, text: &str, role: &str) -> String {
        format!("{}{}{}", self.fg_ansi(role), text, Self::reset_ansi())
    }

    /// Format text with a background color role
    pub fn fmt_bg(&self, text: &str, role: &str) -> String {
        format!("{}{}{}", self.bg_ansi(role), text, Self::reset_ansi())
    }
}

/// Format text with a theme color role (convenience function)
pub fn fmt_with_color(text: &str, role: &str) -> String {
    let theme = get_global_theme();
    theme.fmt_fg(text, role)
}

// ─────────────────────────────────────────────────────────────────────────────
// Display implementation
// ─────────────────────────────────────────────────────────────────────────────

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Theme({})", self.name)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Color tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_color_from_hex() {
        let color = Color::from_hex("#FF5500").unwrap();
        assert_eq!(color.r, 255);
        assert_eq!(color.g, 85);
        assert_eq!(color.b, 0);
    }

    #[test]
    fn test_color_from_hex_without_hash() {
        let color = Color::from_hex("AABBCC").unwrap();
        assert_eq!(color.r, 170);
        assert_eq!(color.g, 187);
        assert_eq!(color.b, 204);
    }

    #[test]
    fn test_color_invalid_hex() {
        assert!(Color::from_hex("invalid").is_none());
        assert!(Color::from_hex("#ABC").is_none());
    }

    #[test]
    fn test_color_to_hex() {
        let color = Color::new(255, 128, 64);
        assert_eq!(color.to_hex(), "ff8040");
    }

    #[test]
    fn test_color_ansi_codes() {
        let color = Color::new(255, 0, 0);
        assert_eq!(color.fg_ansi(), "\x1b[38;2;255;0;0m");
        assert_eq!(color.bg_ansi(), "\x1b[48;2;255;0;0m");
    }

    #[test]
    fn test_color_is_dark() {
        assert!(Color::new(0, 0, 0).is_dark());
        assert!(!Color::new(255, 255, 255).is_dark());
        // Yellow is bright
        assert!(!Color::new(255, 255, 0).is_dark());
    }

    // ── ThemeColors tests ─────────────────────────────────────────────────────

    #[test]
    fn test_dark_colors_contain_expected_colors() {
        let colors = ThemeColors::dark();
        assert_eq!(colors.fg_primary, Color::new(220, 223, 228));
        assert_eq!(colors.bg_primary, Color::new(30, 30, 36));
    }

    #[test]
    fn test_light_colors_contain_expected_colors() {
        let colors = ThemeColors::light();
        assert_eq!(colors.fg_primary, Color::new(76, 79, 105));
        assert_eq!(colors.bg_primary, Color::new(239, 241, 245));
    }

    #[test]
    fn test_nord_colors() {
        let colors = ThemeColors::nord();
        assert_eq!(colors.accent_blue, Color::new(136, 192, 208));
        assert_eq!(colors.syntax_keyword, Color::new(136, 192, 208));
    }

    #[test]
    fn test_catppuccin_colors() {
        let colors = ThemeColors::catppuccin();
        assert_eq!(colors.accent_blue, Color::new(137, 176, 174));
        assert_eq!(colors.accent_green, Color::new(166, 227, 161));
    }

    // ── Theme tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_theme_new() {
        let theme = Theme::new("test", true);
        assert_eq!(theme.name, "test");
        assert!(theme.is_dark);
        assert!(theme.font.is_some());
        assert!(theme.spacing.is_some());
    }

    #[test]
    fn test_built_in_themes() {
        let names = Theme::built_in_names();
        assert!(names.contains(&"oxi_dark"));
        assert!(names.contains(&"oxi_light"));
        assert!(names.contains(&"nord"));
        assert!(names.contains(&"catppuccin"));
        assert!(names.contains(&"github_dark"));
        assert!(names.contains(&"monokai"));
    }

    #[test]
    fn test_oxi_dark_theme() {
        let theme = Theme::oxi_dark();
        assert_eq!(theme.name, "oxi_dark");
        assert!(theme.is_dark);
    }

    #[test]
    fn test_oxi_light_theme() {
        let theme = Theme::oxi_light();
        assert_eq!(theme.name, "oxi_light");
        assert!(!theme.is_dark);
    }

    #[test]
    fn test_theme_get_color() {
        let theme = Theme::oxi_dark();
        assert_eq!(theme.fg(FgRole::Primary), &Color::new(220, 223, 228));
        assert_eq!(theme.bg(BgRole::Primary), &Color::new(30, 30, 36));
        assert_eq!(theme.accent(AccentRole::Blue), &Color::new(137, 180, 250));
    }

    #[test]
    fn test_theme_ansi_codes() {
        let theme = Theme::oxi_dark();
        let fg = theme.fg_ansi("fg_primary");
        assert!(fg.starts_with("\x1b[38;2;"));
        let bg = theme.bg_ansi("bg_primary");
        assert!(bg.starts_with("\x1b[48;2;"));
    }

    #[test]
    fn test_theme_fmt_fg() {
        let theme = Theme::oxi_dark();
        let formatted = theme.fmt_fg("test", "accent_blue");
        assert!(formatted.contains("test"));
        assert!(formatted.ends_with("\x1b[0m"));
    }

    // ── ThemeFile tests ─────────────────────────────────────────────────────

    #[test]
    fn test_theme_file_parse_toml() {
        let toml = r#"
name = "custom-theme"
author = "Test Author"
is_dark = true

[colors]
fg_primary = "#ff0000"
bg_primary = "#000000"
accent_blue = "#0000ff"
"#;
        let file: ThemeFile = toml::from_str(toml).unwrap();
        assert_eq!(file.name, "custom-theme");
        assert_eq!(file.author, Some("Test Author".to_string()));
        assert!(file.is_dark);
        assert_eq!(file.colors.fg_primary, Some("#ff0000".to_string()));
    }

    #[test]
    fn test_theme_file_into_theme() {
        let toml = r#"
name = "test-theme"
is_dark = false

[colors]
fg_primary = "#112233"
bg_primary = "#445566"
"#;
        let file: ThemeFile = toml::from_str(toml).unwrap();
        let theme = file.into_theme();
        assert_eq!(theme.name, "test-theme");
        assert!(!theme.is_dark);
        assert_eq!(theme.colors.fg_primary, Color::new(17, 34, 51));
        assert_eq!(theme.colors.bg_primary, Color::new(68, 85, 102));
    }

    #[test]
    fn test_theme_file_roundtrip() {
        let original = ThemeFile {
            name: "roundtrip".to_string(),
            author: Some("Test".to_string()),
            is_dark: true,
            colors: ThemeFileColors {
                fg_primary: Some("#AABBCC".to_string()),
                bg_primary: Some("#001122".to_string()),
                ..Default::default()
            },
            font: None,
            spacing: None,
            borders: None,
        };

        let toml_str = original.to_toml().unwrap();
        let parsed: ThemeFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.name, "roundtrip");
        assert_eq!(parsed.colors.fg_primary, Some("#AABBCC".to_string()));
    }

    // ── ThemeManager tests ────────────────────────────────────────────────────

    #[test]
    fn test_theme_manager_built_in_themes() {
        let manager = ThemeManager::new();
        assert!(manager.get_theme("oxi_dark").is_some());
        assert!(manager.get_theme("oxi_light").is_some());
        assert!(manager.get_theme("nord").is_some());
        assert!(manager.get_theme("catppuccin").is_some());
        assert!(manager.get_theme("github_dark").is_some());
        assert!(manager.get_theme("monokai").is_some());
        assert!(manager.get_theme("nonexistent").is_none());
    }

    #[test]
    fn test_theme_manager_current_theme() {
        let manager = ThemeManager::new();
        assert_eq!(manager.get_current_theme().unwrap().name, "oxi_dark");

        manager.set_current_theme("nord");
        assert_eq!(manager.get_current_theme().unwrap().name, "nord");

        // Setting non-existent theme should fail
        assert!(!manager.set_current_theme("fake_theme"));
        assert_eq!(manager.get_current_theme().unwrap().name, "nord");
    }

    #[test]
    fn test_theme_manager_toggle() {
        let manager = ThemeManager::new();
        manager.set_current_theme("oxi_dark");
        assert!(manager.get_current_theme().unwrap().is_dark);

        manager.toggle_dark_light();
        assert!(!manager.get_current_theme().unwrap().is_dark);

        manager.toggle_dark_light();
        assert!(manager.get_current_theme().unwrap().is_dark);
    }

    #[test]
    fn test_theme_manager_theme_names() {
        let manager = ThemeManager::new();
        let names = manager.theme_names();
        assert!(names.contains(&"oxi_dark".to_string()));
        assert!(names.contains(&"oxi_light".to_string()));
    }

    #[test]
    fn test_global_theme_functions() {
        set_global_theme("nord");
        let theme = get_global_theme();
        assert_eq!(theme.name, "nord");

        let by_name = get_theme_by_name("catppuccin");
        assert!(by_name.is_some());
        assert_eq!(by_name.unwrap().name, "catppuccin");
    }

    // ── Parse color tests ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_color_hex() {
        assert_eq!(parse_color("#ff0000"), Some(Color::new(255, 0, 0)));
        assert_eq!(parse_color("#00ff00"), Some(Color::new(0, 255, 0)));
        assert_eq!(parse_color("#0000ff"), Some(Color::new(0, 0, 255)));
    }

    #[test]
    fn test_parse_color_rgb() {
        assert_eq!(parse_color("rgb(255, 0, 0)"), Some(Color::new(255, 0, 0)));
        assert_eq!(parse_color("rgb(0,255,0)"), Some(Color::new(0, 255, 0)));
        assert_eq!(parse_color("rgb( 100 , 150 , 200 )"), Some(Color::new(100, 150, 200)));
    }

    #[test]
    fn test_parse_color_named() {
        assert_eq!(parse_color("black"), Some(Color::new(0, 0, 0)));
        assert_eq!(parse_color("white"), Some(Color::new(255, 255, 255)));
        assert_eq!(parse_color("red"), Some(Color::new(255, 0, 0)));
        assert_eq!(parse_color("gray"), Some(Color::new(128, 128, 128)));
        assert_eq!(parse_color("unknown_color"), None);
    }

    // ── BorderStyle tests ─────────────────────────────────────────────────────

    #[test]
    fn test_border_style_chars() {
        assert!(!BorderStyle::Single.chars().is_empty());
        assert!(!BorderStyle::Double.chars().is_empty());
        assert!(BorderStyle::None.chars().is_empty());
    }
}
