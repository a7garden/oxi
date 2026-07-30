#![allow(missing_docs)]

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Minimal glyph set selection for oxi settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlyphSet {
    Unicode,
    Ascii,
    Nerd,
}

impl Default for GlyphSet {
    fn default() -> Self { GlyphSet::Unicode }
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

pub type UnknownGlyphSet = String;
