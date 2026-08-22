#![allow(missing_docs)]

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Minimal glyph set selection for oxicode settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlyphSet {
    Unicode,
    Ascii,
    Nerd,
}

impl Default for GlyphSet {
    fn default() -> Self {
        GlyphSet::Unicode
    }
}

impl std::fmt::Display for GlyphSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GlyphSet::Unicode => write!(f, "unicode"),
            GlyphSet::Ascii => write!(f, "ascii"),
            GlyphSet::Nerd => write!(f, "nerd"),
        }
    }
}

impl FromStr for GlyphSet {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "unicode" => Ok(GlyphSet::Unicode),
            "ascii" => Ok(GlyphSet::Ascii),
            "nerd" => Ok(GlyphSet::Nerd),
            _ => Err(format!("Unknown glyph set: {s}")),
        }
    }
}

impl GlyphSet {
    pub fn label(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "Unicode",
            GlyphSet::Ascii => "ASCII",
            GlyphSet::Nerd => "Nerd",
        }
    }
}

/// Nerd Font icons for the composer's context row (`glyph_set = "nerd"`).
///
/// All glyphs are Nerd Font private-use codepoints (Material Design
/// range) — **never emoji**, so they render monochrome and width-1 in
/// any terminal with a patched font. Terminals without the font show
/// the fallback box; users opt in via settings.
pub mod nerd {
    /// Robot — the active model.
    pub const MODEL: &str = "\u{F06A9} ";
    /// Lightbulb — the thinking level.
    pub const THINK: &str = "\u{F06E8} ";
    /// Rocket — the live run stage.
    pub const RUN: &str = "\u{F04C5} ";
    /// Folder — the working directory.
    pub const DIR: &str = "\u{F024B} ";
    /// Git logo — the branch.
    pub const GIT: &str = "\u{F02A2} ";
    /// Database — context-window usage.
    pub const CTX: &str = "\u{F01BC} ";
    /// Brain — the oxibrain daemon chip.
    pub const BRAIN: &str = "\u{F09E0}";
}
pub type UnknownGlyphSet = String;
