//! Design-farmer skill for oxi
//!
//! Constructs design systems from codebases by:
//! 1. **Analyzing** an existing project's structure, patterns, and conventions
//! 2. **Extracting** design tokens (colors, spacing, typography, radii, shadows)
//! 3. **Building** token hierarchies with OKLCH color management for
//!    perceptually uniform palettes
//! 4. **Implementing** accessible component specifications with WCAG contrast
//!    validation
//!
//! This module provides both a library API ([`DesignFarmer`]) and a skill-prompt
//! generator ([`DesignFarmer::skill_prompt`]) for LLM-driven design extraction.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Color types ───────────────────────────────────────────────────────

/// An OKLCH color value: perceptually uniform lightness + chroma + hue.
///
/// OKLCH is the recommended color space for design systems because:
/// - **L** is perceptually uniform (0.0 = black, 1.0 = white)
/// - **C** (chroma) scales predictably regardless of hue
/// - **H** (hue) is a 0–360 angle like HSL, but without the lightness distortion
///
/// CSS syntax: `oklch(L C H)` or `oklch(L C H / alpha)`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OklchColor {
    /// Perceptual lightness: 0.0 (black) to 1.0 (white).
    pub l: f64,
    /// Chroma (colorfulness): 0.0 (gray) to ~0.4 (vivid). Practical max varies by hue.
    pub c: f64,
    /// Hue angle: 0.0 to 360.0 degrees.
    pub h: f64,
    /// Alpha: 0.0 (transparent) to 1.0 (opaque). Defaults to 1.0.
    #[serde(default = "default_alpha")]
    pub alpha: f64,
}

fn default_alpha() -> f64 {
    1.0
}

impl OklchColor {
    /// Create a new OKLCH color.
    pub fn new(l: f64, c: f64, h: f64) -> Self {
        Self {
            l,
            c,
            h,
            alpha: 1.0,
        }
    }

    /// Create an OKLCH color with transparency.
    pub fn with_alpha(mut self, alpha: f64) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }

    /// Pure black in OKLCH.
    pub fn black() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }

    /// Pure white in OKLCH.
    pub fn white() -> Self {
        Self::new(1.0, 0.0, 0.0)
    }

    /// Generate a palette by varying lightness at constant chroma and hue.
    ///
    /// Produces `steps` evenly spaced stops from `l_min` to `l_max`.
    /// Useful for creating a gray scale or a single-hue palette.
    pub fn lightness_scale(&self, steps: usize, l_min: f64, l_max: f64) -> Vec<OklchColor> {
        if steps <= 1 {
            return vec![*self];
        }
        (0..steps)
            .map(|i| {
                let t = i as f64 / (steps - 1) as f64;
                let l = l_min + t * (l_max - l_min);
                OklchColor::new(l, self.c, self.h)
            })
            .collect()
    }

    /// Compute a perceptual contrast ratio against another color.
    ///
    /// Uses the WCAG 2.x relative luminance approximation mapped through OKLCH
    /// lightness. Returns a ratio ≥ 1.0 where:
    /// - 1.0 = identical lightness
    /// - 4.5:1 = AA normal text minimum
    /// - 7.0:1 = AAA normal text minimum
    /// - 21.0 = maximum (black vs white)
    pub fn contrast_ratio(&self, other: &OklchColor) -> f64 {
        // Convert OKLCH L to approximate relative luminance
        let l1 = oklch_l_to_luminance(self.l);
        let l2 = oklch_l_to_luminance(other.l);
        let lighter = l1.max(l2);
        let darker = l1.min(l2);
        (lighter + 0.05) / (darker + 0.05)
    }

    /// Check WCAG 2.x AA compliance for normal text against a background.
    pub fn is_aa_compliant(&self, background: &OklchColor) -> bool {
        self.contrast_ratio(background) >= 4.5
    }

    /// Check WCAG 2.x AAA compliance for normal text against a background.
    pub fn is_aaa_compliant(&self, background: &OklchColor) -> bool {
        self.contrast_ratio(background) >= 7.0
    }

    /// Render as CSS `oklch()` function string.
    pub fn to_css(&self) -> String {
        if self.alpha < 1.0 {
            format!(
                "oklch({:.4} {:.4} {:.1} / {:.2})",
                self.l, self.c, self.h, self.alpha
            )
        } else {
            format!("oklch({:.4} {:.4} {:.1})", self.l, self.c, self.h)
        }
    }
}

impl fmt::Display for OklchColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_css())
    }
}

/// Approximate conversion from OKLCH lightness to CIE relative luminance.
///
/// This is an approximation: OKLCH L maps roughly linearly to perceived
/// lightness, but the conversion to the sRGB luminance used by WCAG is
/// non-linear. We use a piecewise approximation that's accurate enough for
/// contrast checking.
fn oklch_l_to_luminance(l: f64) -> f64 {
    // OKLab achromatic conversion: Y = L³
    // This follows from the OKLab spec for achromatic colors (a=b=0).
    // The full transform is LMS = (l + 0.3963377774*a + 0.2158037573*b)³,
    // and for C=0 (achromatic), this reduces to L³.
    l * l * l
}

// ── Design tokens ─────────────────────────────────────────────────────

/// Category of a design token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenCategory {
    /// Color tokens (backgrounds, foregrounds, accents, etc.).
    Color,
    /// Spacing tokens (padding, margins, gaps).
    Spacing,
    /// Typography tokens (font families, sizes, weights, line-heights).
    Typography,
    /// Border radius tokens.
    Radius,
    /// Shadow tokens (elevation).
    Shadow,
    /// Opacity tokens.
    Opacity,
    /// Breakpoint tokens (responsive).
    Breakpoint,
    /// Z-index tokens.
    ZIndex,
    /// Animation / transition tokens.
    Motion,
    /// Custom / other tokens.
    Custom,
}

impl fmt::Display for TokenCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenCategory::Color => write!(f, "color"),
            TokenCategory::Spacing => write!(f, "spacing"),
            TokenCategory::Typography => write!(f, "typography"),
            TokenCategory::Radius => write!(f, "radius"),
            TokenCategory::Shadow => write!(f, "shadow"),
            TokenCategory::Opacity => write!(f, "opacity"),
            TokenCategory::Breakpoint => write!(f, "breakpoint"),
            TokenCategory::ZIndex => write!(f, "z-index"),
            TokenCategory::Motion => write!(f, "motion"),
            TokenCategory::Custom => write!(f, "custom"),
        }
    }
}

/// A single design token with its resolved value and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignToken {
    /// Dot-separated token name (e.g., `color.primary.500`).
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Token category.
    pub category: TokenCategory,
    /// The resolved value as a string (CSS-compatible).
    pub value: String,
    /// For color tokens: the OKLCH value (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oklch: Option<OklchColor>,
    /// References another token (e.g., `color.primary.500` might alias to
    /// `color.blue.500`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Group path for organizing tokens in a hierarchy.
    pub group: Vec<String>,
}

impl DesignToken {
    /// Create a new token.
    pub fn new(
        name: impl Into<String>,
        category: TokenCategory,
        value: impl Into<String>,
        group: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: None,
            category,
            value: value.into(),
            oklch: None,
            alias: None,
            group,
        }
    }

    /// Create a color token with OKLCH value.
    pub fn color(name: impl Into<String>, color: OklchColor, group: Vec<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            category: TokenCategory::Color,
            value: color.to_css(),
            oklch: Some(color),
            alias: None,
            group,
        }
    }

    /// Create an aliased token that references another token.
    pub fn alias(name: impl Into<String>, target: impl Into<String>, group: Vec<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            category: TokenCategory::Color, // will be overridden by caller
            value: String::new(),
            oklch: None,
            alias: Some(target.into()),
            group,
        }
    }

    /// Set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

// ── Token hierarchy ───────────────────────────────────────────────────

/// A hierarchical group of design tokens, forming a tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenGroup {
    /// Group name (e.g., "color", "primary").
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Tokens directly in this group.
    pub tokens: Vec<DesignToken>,
    /// Sub-groups.
    pub children: Vec<TokenGroup>,
}

impl TokenGroup {
    /// Create a new empty group.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            tokens: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add a token to this group.
    pub fn add_token(&mut self, token: DesignToken) {
        self.tokens.push(token);
    }

    /// Add a child group.
    pub fn add_child(&mut self, group: TokenGroup) {
        self.children.push(group);
    }

    /// Get a child group by name.
    pub fn child(&self, name: &str) -> Option<&TokenGroup> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Get a mutable child group by name (creates if missing).
    pub fn child_mut(&mut self, name: &str) -> &mut TokenGroup {
        let idx = self.children.iter().position(|c| c.name == name);
        if let Some(i) = idx {
            &mut self.children[i]
        } else {
            self.children.push(TokenGroup::new(name));
            self.children.last_mut().unwrap()
        }
    }

    /// Collect all tokens in this group and descendants, flattened.
    pub fn all_tokens(&self) -> Vec<&DesignToken> {
        let mut result: Vec<&DesignToken> = self.tokens.iter().collect();
        for child in &self.children {
            result.extend(child.all_tokens());
        }
        result
    }

    /// Total number of tokens in this group and all descendants.
    pub fn token_count(&self) -> usize {
        self.tokens.len() + self.children.iter().map(|c| c.token_count()).sum::<usize>()
    }
}

// ── Color palette ─────────────────────────────────────────────────────

/// A color palette generated from a base OKLCH color.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorPalette {
    /// Palette name (e.g., "primary", "gray", "success").
    pub name: String,
    /// Base hue angle (0–360).
    pub hue: f64,
    /// Base chroma.
    pub chroma: f64,
    /// Generated stops, keyed by lightness step (e.g., 50, 100, ..., 900, 950).
    pub stops: Vec<PaletteStop>,
}

/// A single stop within a color palette.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaletteStop {
    /// Step number (50, 100, 200, ..., 900, 950).
    pub step: u16,
    /// The OKLCH color at this step.
    pub color: OklchColor,
    /// Token name (e.g., "color.primary.500").
    pub token_name: String,
}

impl ColorPalette {
    /// Generate a full palette from a base color using Tailwind-style steps.
    ///
    /// Steps: 50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950
    /// Lightness ranges from ~0.97 (step 50) to ~0.15 (step 950).
    pub fn generate(name: impl Into<String>, base: &OklchColor) -> Self {
        let steps: [(u16, f64); 11] = [
            (50, 0.97),
            (100, 0.93),
            (200, 0.86),
            (300, 0.78),
            (400, 0.68),
            (500, 0.55),
            (600, 0.44),
            (700, 0.35),
            (800, 0.26),
            (900, 0.20),
            (950, 0.14),
        ];

        let name_str = name.into();
        let stops = steps
            .iter()
            .map(|(step, l)| {
                // Chroma compression at extreme lightness to avoid clipping
                let chroma_factor = if *l > 0.9 || *l < 0.2 {
                    0.6
                } else if *l > 0.8 || *l < 0.3 {
                    0.8
                } else {
                    1.0
                };
                PaletteStop {
                    step: *step,
                    color: OklchColor::new(*l, base.c * chroma_factor, base.h),
                    token_name: format!("color.{}.{}", name_str, step),
                }
            })
            .collect();

        Self {
            name: name_str,
            hue: base.h,
            chroma: base.c,
            stops,
        }
    }

    /// Generate a neutral/gray palette (zero chroma).
    pub fn neutral(name: impl Into<String>) -> Self {
        let base = OklchColor::new(0.5, 0.0, 0.0);
        Self::generate(name, &base)
    }

    /// Get a stop by step number.
    pub fn get_stop(&self, step: u16) -> Option<&PaletteStop> {
        self.stops.iter().find(|s| s.step == step)
    }

    /// Get the middle (500) color.
    pub fn mid(&self) -> &OklchColor {
        &self.get_stop(500).expect("palette always has step 500").color
    }

    /// Convert this palette into a [`TokenGroup`].
    pub fn to_token_group(&self) -> TokenGroup {
        let mut group = TokenGroup::new(&self.name)
            .with_description(format!(
                "{} palette (hue {:.0}°, chroma {:.3})",
                self.name, self.hue, self.chroma
            ));

        for stop in &self.stops {
            group.add_token(DesignToken::color(&stop.token_name, stop.color, vec!["color".into(), self.name.clone()]));
        }

        group
    }
}

// ── Accessible component spec ─────────────────────────────────────────

/// WCAG conformance level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WcagLevel {
    /// No conformance claim.
    None,
    /// WCAG 2.x Level A.
    A,
    /// WCAG 2.x Level AA.
    AA,
    /// WCAG 2.x Level AAA.
    AAA,
}

impl fmt::Display for WcagLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WcagLevel::None => write!(f, "none"),
            WcagLevel::A => write!(f, "A"),
            WcagLevel::AA => write!(f, "AA"),
            WcagLevel::AAA => write!(f, "AAA"),
        }
    }
}

/// A contrast check result between a foreground and background color.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContrastCheck {
    /// Name of the foreground token.
    pub foreground_token: String,
    /// Name of the background token.
    pub background_token: String,
    /// The foreground color.
    pub foreground: OklchColor,
    /// The background color.
    pub background: OklchColor,
    /// Computed contrast ratio.
    pub ratio: f64,
    /// WCAG AA pass/fail.
    pub aa_pass: bool,
    /// WCAG AAA pass/fail.
    pub aaa_pass: bool,
}

impl ContrastCheck {
    /// Run a contrast check between two tokens.
    pub fn check(
        fg_token: &str,
        fg: OklchColor,
        bg_token: &str,
        bg: OklchColor,
    ) -> Self {
        let ratio = fg.contrast_ratio(&bg);
        Self {
            foreground_token: fg_token.to_string(),
            background_token: bg_token.to_string(),
            foreground: fg,
            background: bg,
            ratio,
            aa_pass: ratio >= 4.5,
            aaa_pass: ratio >= 7.0,
        }
    }

    /// Whether this check passes at the given WCAG level.
    pub fn passes(&self, level: WcagLevel) -> bool {
        match level {
            WcagLevel::None => true,
            WcagLevel::A => true, // A has the same minimum as AA for contrast
            WcagLevel::AA => self.aa_pass,
            WcagLevel::AAA => self.aaa_pass,
        }
    }
}

/// A component specification with accessibility requirements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSpec {
    /// Component name (e.g., "Button", "Card", "Modal").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Component variants (e.g., "primary", "secondary", "ghost").
    pub variants: Vec<ComponentVariant>,
    /// Accessibility notes.
    pub a11y_notes: Vec<String>,
    /// Required WCAG level.
    pub wcag_level: WcagLevel,
    /// Contrast checks for this component.
    pub contrast_checks: Vec<ContrastCheck>,
    /// Required ARIA roles or attributes.
    pub aria_requirements: Vec<String>,
    /// Keyboard interaction requirements.
    pub keyboard_requirements: Vec<String>,
}

/// A variant of a component (e.g., "primary" button, "outline" button).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentVariant {
    /// Variant name.
    pub name: String,
    /// Token references for this variant (token name → CSS property).
    pub token_refs: Vec<(String, String)>,
    /// Additional CSS properties not covered by tokens.
    #[serde(default)]
    pub extra_styles: Vec<(String, String)>,
}

impl ComponentSpec {
    /// Create a new component spec.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            variants: Vec::new(),
            a11y_notes: Vec::new(),
            wcag_level: WcagLevel::AA,
            contrast_checks: Vec::new(),
            aria_requirements: Vec::new(),
            keyboard_requirements: Vec::new(),
        }
    }

    /// Set the required WCAG conformance level.
    pub fn with_wcag_level(mut self, level: WcagLevel) -> Self {
        self.wcag_level = level;
        self
    }

    /// Add a variant.
    pub fn add_variant(&mut self, variant: ComponentVariant) {
        self.variants.push(variant);
    }

    /// Add an accessibility note.
    pub fn add_a11y_note(&mut self, note: impl Into<String>) {
        self.a11y_notes.push(note.into());
    }

    /// Add a required ARIA attribute or role.
    pub fn add_aria(&mut self, requirement: impl Into<String>) {
        self.aria_requirements.push(requirement.into());
    }

    /// Add a keyboard interaction requirement.
    pub fn add_keyboard(&mut self, requirement: impl Into<String>) {
        self.keyboard_requirements.push(requirement.into());
    }

    /// Run a contrast check and record the result.
    pub fn check_contrast(
        &mut self,
        fg_token: &str,
        fg: OklchColor,
        bg_token: &str,
        bg: OklchColor,
    ) -> &ContrastCheck {
        let check = ContrastCheck::check(fg_token, fg, bg_token, bg);
        self.contrast_checks.push(check);
        self.contrast_checks.last().unwrap()
    }

    /// Whether all contrast checks pass at the required WCAG level.
    pub fn is_accessible(&self) -> bool {
        self.contrast_checks.iter().all(|c| c.passes(self.wcag_level))
    }

    /// Get failing contrast checks.
    pub fn failing_checks(&self) -> Vec<&ContrastCheck> {
        self.contrast_checks
            .iter()
            .filter(|c| !c.passes(self.wcag_level))
            .collect()
    }
}

// ── Codebase analysis ─────────────────────────────────────────────────

/// Findings from analyzing a codebase's design patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignAnalysis {
    /// Project root that was analyzed.
    pub project_root: String,
    /// Design-related files found.
    pub design_files: Vec<DesignFile>,
    /// Detected framework / styling approach.
    pub framework: DesignFramework,
    /// Extracted raw design patterns.
    pub patterns: Vec<DesignPattern>,
    /// Summary of findings.
    pub summary: String,
}

/// A design-related file in the project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignFile {
    /// Relative path from project root.
    pub path: String,
    /// Type of design file.
    pub file_type: DesignFileType,
    /// Brief description of contents.
    pub description: String,
}

/// Type of design file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignFileType {
    /// CSS / SCSS / LESS stylesheet.
    Stylesheet,
    /// Tailwind config.
    TailwindConfig,
    /// Theme configuration file.
    ThemeConfig,
    /// Design token file (JSON, YAML, etc.).
    TokenFile,
    /// Component file (React, Vue, Svelte, etc.).
    Component,
    /// Storybook story.
    Story,
    /// Figma / design tool export.
    DesignExport,
    /// Other.
    Other,
}

impl fmt::Display for DesignFileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DesignFileType::Stylesheet => write!(f, "stylesheet"),
            DesignFileType::TailwindConfig => write!(f, "tailwind-config"),
            DesignFileType::ThemeConfig => write!(f, "theme-config"),
            DesignFileType::TokenFile => write!(f, "token-file"),
            DesignFileType::Component => write!(f, "component"),
            DesignFileType::Story => write!(f, "story"),
            DesignFileType::DesignExport => write!(f, "design-export"),
            DesignFileType::Other => write!(f, "other"),
        }
    }
}

/// Detected design framework / approach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignFramework {
    /// Tailwind CSS (utility-first).
    Tailwind,
    /// CSS Modules.
    CssModules,
    /// Styled-components / Emotion / CSS-in-JS.
    CssInJs,
    /// Vanilla CSS / SCSS / LESS.
    Vanilla,
    /// Unknown / not detected.
    Unknown,
}

impl fmt::Display for DesignFramework {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DesignFramework::Tailwind => write!(f, "Tailwind CSS"),
            DesignFramework::CssModules => write!(f, "CSS Modules"),
            DesignFramework::CssInJs => write!(f, "CSS-in-JS"),
            DesignFramework::Vanilla => write!(f, "Vanilla CSS"),
            DesignFramework::Unknown => write!(f, "Unknown"),
        }
    }
}

/// A design pattern extracted from the codebase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignPattern {
    /// Pattern name.
    pub name: String,
    /// Pattern category.
    pub category: PatternCategory,
    /// Where this pattern was found.
    pub source: String,
    /// Description of the pattern.
    pub description: String,
}

/// Category of design pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternCategory {
    /// Color usage pattern.
    Color,
    /// Layout pattern.
    Layout,
    /// Spacing pattern.
    Spacing,
    /// Typography pattern.
    Typography,
    /// Component composition pattern.
    Composition,
    /// Animation / motion pattern.
    Motion,
    /// Responsive / breakpoint pattern.
    Responsive,
}

impl fmt::Display for PatternCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternCategory::Color => write!(f, "color"),
            PatternCategory::Layout => write!(f, "layout"),
            PatternCategory::Spacing => write!(f, "spacing"),
            PatternCategory::Typography => write!(f, "typography"),
            PatternCategory::Composition => write!(f, "composition"),
            PatternCategory::Motion => write!(f, "motion"),
            PatternCategory::Responsive => write!(f, "responsive"),
        }
    }
}

// ── Design system document ────────────────────────────────────────────

/// A complete design system document with tokens, palettes, and components.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignSystem {
    /// System name.
    pub name: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Version.
    pub version: String,
    /// Root token group hierarchy.
    pub tokens: TokenGroup,
    /// Color palettes.
    pub palettes: Vec<ColorPalette>,
    /// Component specifications.
    pub components: Vec<ComponentSpec>,
    /// Codebase analysis (if this system was extracted from an existing project).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<DesignAnalysis>,
    /// Design decisions and rationale.
    pub decisions: Vec<DesignDecision>,
}

/// A documented design decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignDecision {
    /// Short title.
    pub title: String,
    /// What was decided.
    pub decision: String,
    /// Why this decision was made.
    pub rationale: String,
    /// Alternatives considered.
    #[serde(default)]
    pub alternatives: Vec<String>,
}

impl DesignSystem {
    /// Create an empty design system.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            created_at: Utc::now().to_rfc3339(),
            version: "1.0.0".to_string(),
            tokens: TokenGroup::new("root"),
            palettes: Vec::new(),
            components: Vec::new(),
            analysis: None,
            decisions: Vec::new(),
        }
    }

    /// Total token count across all groups.
    pub fn total_tokens(&self) -> usize {
        self.tokens.token_count()
    }

    /// Check accessibility of all components.
    pub fn accessibility_report(&self) -> AccessibilityReport {
        let mut passing = Vec::new();
        let mut failing = Vec::new();

        for component in &self.components {
            for check in &component.contrast_checks {
                if check.passes(component.wcag_level) {
                    passing.push(check.clone());
                } else {
                    failing.push(check.clone());
                }
            }
        }

        passing.sort_by(|a, b| {
            b.ratio
                .partial_cmp(&a.ratio)
                .unwrap_or(Ordering::Equal)
        });
        failing.sort_by(|a, b| {
            a.ratio
                .partial_cmp(&b.ratio)
                .unwrap_or(Ordering::Equal)
        });

        let all_pass = failing.is_empty();
        let min_ratio = passing
            .iter()
            .chain(failing.iter())
            .map(|c| c.ratio)
            .reduce(f64::min)
            .unwrap_or(0.0);

        AccessibilityReport {
            all_pass,
            total_checks: passing.len() + failing.len(),
            passing_count: passing.len(),
            failing_count: failing.len(),
            min_ratio,
            passing,
            failing,
        }
    }

    /// Add a palette and register its tokens.
    pub fn add_palette(&mut self, palette: ColorPalette) {
        let group = palette.to_token_group();
        self.tokens.add_child(group);
        self.palettes.push(palette);
    }

    /// Record a design decision.
    pub fn add_decision(
        &mut self,
        title: impl Into<String>,
        decision: impl Into<String>,
        rationale: impl Into<String>,
        alternatives: Vec<String>,
    ) {
        self.decisions.push(DesignDecision {
            title: title.into(),
            decision: decision.into(),
            rationale: rationale.into(),
            alternatives,
        });
    }

    /// Render as a Markdown document.
    pub fn render_markdown(&self) -> String {
        let mut md = String::with_capacity(8192);

        md.push_str(&format!("# Design System: {}\n\n", self.name));
        md.push_str(&format!("> Created: {} | Version: {}\n\n", self.created_at, self.version));

        // Summary
        md.push_str(&format!(
            "**{} tokens** across {} palettes and {} components.\n\n",
            self.total_tokens(),
            self.palettes.len(),
            self.components.len(),
        ));

        // Analysis
        if let Some(ref analysis) = self.analysis {
            md.push_str("## Codebase Analysis\n\n");
            md.push_str(&format!("**Framework:** {}\n\n", analysis.framework));
            md.push_str(&analysis.summary);
            md.push_str("\n\n");

            if !analysis.design_files.is_empty() {
                md.push_str("### Design Files\n\n");
                for f in &analysis.design_files {
                    md.push_str(&format!("- `{}` — {} ({})\n", f.path, f.description, f.file_type));
                }
                md.push('\n');
            }

            if !analysis.patterns.is_empty() {
                md.push_str("### Extracted Patterns\n\n");
                for p in &analysis.patterns {
                    md.push_str(&format!(
                        "- **{}** [{}]: {} (from `{}`)\n",
                        p.name, p.category, p.description, p.source
                    ));
                }
                md.push('\n');
            }
        }

        // Palettes
        if !self.palettes.is_empty() {
            md.push_str("## Color Palettes\n\n");
            for palette in &self.palettes {
                md.push_str(&format!("### {} (hue {:.0}°)\n\n", palette.name, palette.hue));
                md.push_str("| Step | Token | Value |\n");
                md.push_str("|------|-------|-------|\n");
                for stop in &palette.stops {
                    md.push_str(&format!(
                        "| {} | `{}` | `{}` |\n",
                        stop.step, stop.token_name, stop.color.to_css()
                    ));
                }
                md.push('\n');
            }
        }

        // Token hierarchy
        md.push_str("## Token Hierarchy\n\n");
        Self::render_token_group_md(&self.tokens, &mut md, 0);
        md.push('\n');

        // Components
        if !self.components.is_empty() {
            md.push_str("## Components\n\n");
            for component in &self.components {
                let accessible_icon = if component.is_accessible() { "✅" } else { "⚠️" };
                md.push_str(&format!(
                    "### {} {} [WCAG {}]\n\n",
                    component.name, accessible_icon, component.wcag_level
                ));
                md.push_str(&component.description);
                md.push_str("\n\n");

                if !component.variants.is_empty() {
                    md.push_str("**Variants:** ");
                    let names: Vec<&str> = component.variants.iter().map(|v| v.name.as_str()).collect();
                    md.push_str(&names.join(", "));
                    md.push_str("\n\n");
                }

                if !component.contrast_checks.is_empty() {
                    md.push_str("**Contrast Checks:**\n\n");
                    md.push_str("| Foreground | Background | Ratio | AA | AAA |\n");
                    md.push_str("|-----------|-----------|-------|----|-----|\n");
                    for check in &component.contrast_checks {
                        let aa = if check.aa_pass { "✅" } else { "❌" };
                        let aaa = if check.aaa_pass { "✅" } else { "❌" };
                        md.push_str(&format!(
                            "| `{}` | `{}` | {:.1}:1 | {} | {} |\n",
                            check.foreground_token, check.background_token, check.ratio, aa, aaa
                        ));
                    }
                    md.push('\n');
                }

                if !component.aria_requirements.is_empty() {
                    md.push_str("**ARIA:**\n\n");
                    for req in &component.aria_requirements {
                        md.push_str(&format!("- {}\n", req));
                    }
                    md.push('\n');
                }

                if !component.keyboard_requirements.is_empty() {
                    md.push_str("**Keyboard:**\n\n");
                    for req in &component.keyboard_requirements {
                        md.push_str(&format!("- {}\n", req));
                    }
                    md.push('\n');
                }

                if !component.a11y_notes.is_empty() {
                    md.push_str("**Accessibility Notes:**\n\n");
                    for note in &component.a11y_notes {
                        md.push_str(&format!("- {}\n", note));
                    }
                    md.push('\n');
                }
            }
        }

        // Accessibility report
        let report = self.accessibility_report();
        md.push_str("## Accessibility Report\n\n");
        if report.all_pass {
            md.push_str(&format!(
                "✅ All {} contrast checks pass.\n\n",
                report.total_checks
            ));
        } else {
            md.push_str(&format!(
                "⚠️ {}/{} checks failing. Minimum ratio: {:.1}:1\n\n",
                report.failing_count, report.total_checks, report.min_ratio
            ));
        }

        // Decisions
        if !self.decisions.is_empty() {
            md.push_str("## Design Decisions\n\n");
            for decision in &self.decisions {
                md.push_str(&format!("### {}\n\n", decision.title));
                md.push_str(&format!("**Decision:** {}\n\n", decision.decision));
                md.push_str(&format!("**Rationale:** {}\n\n", decision.rationale));
                if !decision.alternatives.is_empty() {
                    md.push_str("**Alternatives considered:**\n\n");
                    for alt in &decision.alternatives {
                        md.push_str(&format!("- {}\n", alt));
                    }
                    md.push('\n');
                }
            }
        }

        md
    }

    /// Recursively render a token group as Markdown headings.
    fn render_token_group_md(group: &TokenGroup, md: &mut String, depth: usize) {
        let heading = "#".repeat(depth.min(4) + 3); // h3–h6
        md.push_str(&format!("{} {} ({} tokens)\n\n", heading, group.name, group.token_count()));

        if let Some(ref desc) = group.description {
            md.push_str(&format!("{}\n\n", desc));
        }

        if !group.tokens.is_empty() {
            md.push_str("| Token | Category | Value |\n");
            md.push_str("|-------|----------|-------|\n");
            for token in &group.tokens {
                let value = if let Some(ref alias) = token.alias {
                    format!("→ `{}`", alias)
                } else {
                    format!("`{}`", token.value)
                };
                md.push_str(&format!(
                    "| `{}` | {} | {} |\n",
                    token.name, token.category, value
                ));
            }
            md.push('\n');
        }

        for child in &group.children {
            Self::render_token_group_md(child, md, depth + 1);
        }
    }

    /// Write the design system as Markdown to a file.
    pub fn write_markdown(&self, path: &Path) -> Result<PathBuf> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }

        let content = self.render_markdown();
        fs::write(path, &content)
            .with_context(|| format!("Failed to write design system to {}", path.display()))?;

        Ok(path.to_path_buf())
    }

    /// Write as JSON.
    pub fn write_json(&self, path: &Path) -> Result<PathBuf> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }

        let json = serde_json::to_string_pretty(self)
            .context("Failed to serialize design system")?;
        fs::write(path, &json)
            .with_context(|| format!("Failed to write design system to {}", path.display()))?;

        Ok(path.to_path_buf())
    }
}

// ── Accessibility report ──────────────────────────────────────────────

/// Report from checking all component accessibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessibilityReport {
    /// Whether all checks pass.
    pub all_pass: bool,
    /// Total number of contrast checks.
    pub total_checks: usize,
    /// Number of passing checks.
    pub passing_count: usize,
    /// Number of failing checks.
    pub failing_count: usize,
    /// Minimum contrast ratio across all checks.
    pub min_ratio: f64,
    /// All passing checks (sorted by ratio, highest first).
    pub passing: Vec<ContrastCheck>,
    /// All failing checks (sorted by ratio, lowest first).
    pub failing: Vec<ContrastCheck>,
}

// ── Design farmer skill ──────────────────────────────────────────────

/// The design-farmer skill.
///
/// Provides methods to:
/// - Analyze an existing codebase for design patterns
/// - Extract tokens, palettes, and component specs
/// - Build a complete design system
/// - Generate skill instructions for LLM-driven extraction
pub struct DesignFarmer;

impl DesignFarmer {
    /// Create a new design-farmer skill instance.
    pub fn new() -> Self {
        Self
    }

    /// Analyze a project directory for design-related files and patterns.
    ///
    /// Scans for:
    /// - CSS/SCSS/LESS files
    /// - Tailwind configuration
    /// - Theme/config files
    /// - Component files
    /// - Design token files
    ///
    /// Returns a [`DesignAnalysis`] summarizing what was found.
    pub fn analyze_codebase(dir: &Path) -> Result<DesignAnalysis> {
        let mut design_files = Vec::new();
        let mut patterns = Vec::new();
        let mut framework = DesignFramework::Unknown;

        // Detect framework
        if dir.join("tailwind.config.js").exists()
            || dir.join("tailwind.config.ts").exists()
            || dir.join("tailwind.config.mjs").exists()
        {
            framework = DesignFramework::Tailwind;
        } else if dir.join("postcss.config.js").exists() || dir.join("postcss.config.mjs").exists()
        {
            // Could be Tailwind via PostCSS, check for CSS content
            framework = DesignFramework::Vanilla;
        }

        // Check for CSS-in-JS indicators
        let pkg_json = dir.join("package.json");
        if pkg_json.exists() {
            if let Ok(content) = fs::read_to_string(&pkg_json) {
                if content.contains("styled-components")
                    || content.contains("@emotion")
                    || content.contains("goober")
                    || content.contains("vanilla-extract")
                {
                    framework = DesignFramework::CssInJs;
                }
                if content.contains("tailwindcss") {
                    framework = DesignFramework::Tailwind;
                }
            }
        }

        // Walk the tree looking for design files
        Self::walk_for_design_files(dir, "", 0, 5, &mut design_files, &mut patterns)?;

        // Derive summary
        let file_count = design_files.len();
        let pattern_count = patterns.len();
        let summary = format!(
            "Detected {} design framework. Found {} design-related file(s) and {} pattern(s).",
            framework, file_count, pattern_count
        );

        Ok(DesignAnalysis {
            project_root: dir.to_string_lossy().to_string(),
            design_files,
            framework,
            patterns,
            summary,
        })
    }

    /// Recursively walk looking for design-related files.
    fn walk_for_design_files(
        dir: &Path,
        prefix: &str,
        depth: usize,
        max_depth: usize,
        design_files: &mut Vec<DesignFile>,
        patterns: &mut Vec<DesignPattern>,
    ) -> Result<()> {
        if depth > max_depth {
            return Ok(());
        }

        let entries = fs::read_dir(dir)
            .with_context(|| format!("Failed to read directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip noise
            if name.starts_with('.')
                || name == "node_modules"
                || name == "target"
                || name == "dist"
                || name == "build"
                || name == ".git"
                || name == "vendor"
                || name == "__pycache__"
                || name == "coverage"
            {
                continue;
            }

            let path = entry.path();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };

            if path.is_dir() {
                Self::walk_for_design_files(&path, &rel, depth + 1, max_depth, design_files, patterns)?;
            } else {
                let name_lower = name.to_lowercase();

                // Detect file type
                let (file_type, description) = if matches!(
                    name_lower.as_str(),
                    "tailwind.config.js"
                    | "tailwind.config.ts"
                    | "tailwind.config.mjs"
                    | "tailwind.config.cjs"
                ) {
                    (
                        DesignFileType::TailwindConfig,
                        "Tailwind CSS configuration".to_string(),
                    )
                } else if name_lower.starts_with("theme")
                    && (name_lower.ends_with(".json")
                        || name_lower.ends_with(".js")
                        || name_lower.ends_with(".ts")
                        || name_lower.ends_with(".toml"))
                {
                    (DesignFileType::ThemeConfig, "Theme configuration".to_string())
                } else if name_lower.contains("token")
                    && (name_lower.ends_with(".json") || name_lower.ends_with(".yaml")
                        || name_lower.ends_with(".yml"))
                {
                    (DesignFileType::TokenFile, "Design token definitions".to_string())
                } else if name_lower.ends_with(".css")
                    || name_lower.ends_with(".scss")
                    || name_lower.ends_with(".less")
                {
                    // Heuristic: classify based on location
                    if rel.contains("component") || rel.contains("ui") {
                        (
                            DesignFileType::Stylesheet,
                            "Component stylesheet".to_string(),
                        )
                    } else if rel.contains("global") || rel.contains("base") || name_lower.contains("reset") {
                        (
                            DesignFileType::Stylesheet,
                            "Global/base stylesheet".to_string(),
                        )
                    } else {
                        (DesignFileType::Stylesheet, "Stylesheet".to_string())
                    }
                } else if (name_lower.ends_with(".tsx") || name_lower.ends_with(".jsx"))
                    && !name_lower.contains(".test.")
                    && !name_lower.contains(".spec.")
                    && !name_lower.contains(".story.")
                {
                    // Check if it's a story file
                    if name_lower.contains(".story.") {
                        (DesignFileType::Story, "Component story".to_string())
                    } else if rel.contains("component") || rel.contains("ui") || rel.contains("design") {
                        (DesignFileType::Component, "UI component".to_string())
                    } else {
                        continue; // Skip non-design TSX/JSX files
                    }
                } else if (name_lower.ends_with(".vue") || name_lower.ends_with(".svelte"))
                    && !name_lower.contains(".test.")
                    && !name_lower.contains(".spec.")
                {
                    if rel.contains("component") || rel.contains("ui") {
                        (DesignFileType::Component, "UI component".to_string())
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };

                design_files.push(DesignFile {
                    path: rel.clone(),
                    file_type,
                    description,
                });

                // Extract basic patterns from the file path
                if rel.contains("color") || rel.contains("palette") {
                    patterns.push(DesignPattern {
                        name: "Color organization".to_string(),
                        category: PatternCategory::Color,
                        source: rel.clone(),
                        description: "File is organized around color/palette definitions".to_string(),
                    });
                }
                if rel.contains("spacing") || rel.contains("space") {
                    patterns.push(DesignPattern {
                        name: "Spacing system".to_string(),
                        category: PatternCategory::Spacing,
                        source: rel.clone(),
                        description: "File contains spacing-related definitions".to_string(),
                    });
                }
                if rel.contains("typo") || rel.contains("font") || rel.contains("text") {
                    patterns.push(DesignPattern {
                        name: "Typography system".to_string(),
                        category: PatternCategory::Typography,
                        source: rel.clone(),
                        description: "File contains typography-related definitions".to_string(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Build a complete design system from extracted analysis.
    ///
    /// This constructs a [`DesignSystem`] with:
    /// - A semantic color palette
    /// - A neutral (gray) palette
    /// - Common spacing and typography tokens
    /// - Accessible component specs
    pub fn build_design_system(
        name: impl Into<String>,
        primary_hue: f64,
        primary_chroma: f64,
        analysis: Option<DesignAnalysis>,
    ) -> DesignSystem {
        let mut system = DesignSystem::new(name);

        // Store analysis
        system.analysis = analysis;

        // Root token groups
        let mut color_group = TokenGroup::new("color").with_description("Color tokens");
        let mut spacing_group = TokenGroup::new("spacing").with_description("Spacing scale");
        let mut typography_group = TokenGroup::new("typography").with_description("Typography tokens");
        let mut radius_group = TokenGroup::new("radius").with_description("Border radius tokens");
        let mut shadow_group = TokenGroup::new("shadow").with_description("Elevation shadows");

        // ── Color palettes ──
        let primary = OklchColor::new(0.55, primary_chroma, primary_hue);
        let primary_palette = ColorPalette::generate("primary", &primary);
        color_group.add_child(primary_palette.to_token_group());
        system.palettes.push(primary_palette);

        // Secondary: hue shifted +40°
        let secondary = OklchColor::new(0.55, primary_chroma, (primary_hue + 40.0) % 360.0);
        let secondary_palette = ColorPalette::generate("secondary", &secondary);
        color_group.add_child(secondary_palette.to_token_group());
        system.palettes.push(secondary_palette);

        // Accent: hue shifted +180° (complementary)
        let accent = OklchColor::new(0.55, primary_chroma, (primary_hue + 180.0) % 360.0);
        let accent_palette = ColorPalette::generate("accent", &accent);
        color_group.add_child(accent_palette.to_token_group());
        system.palettes.push(accent_palette);

        // Success (green range)
        let success = OklchColor::new(0.55, 0.15, 145.0);
        let success_palette = ColorPalette::generate("success", &success);
        color_group.add_child(success_palette.to_token_group());
        system.palettes.push(success_palette);

        // Warning (amber range)
        let warning = OklchColor::new(0.75, 0.15, 80.0);
        let warning_palette = ColorPalette::generate("warning", &warning);
        color_group.add_child(warning_palette.to_token_group());
        system.palettes.push(warning_palette);

        // Danger (red range)
        let danger = OklchColor::new(0.55, 0.20, 25.0);
        let danger_palette = ColorPalette::generate("danger", &danger);
        color_group.add_child(danger_palette.to_token_group());
        system.palettes.push(danger_palette);

        // Neutral (gray)
        let neutral_palette = ColorPalette::neutral("neutral");
        color_group.add_child(neutral_palette.to_token_group());
        system.palettes.push(neutral_palette);

        // Semantic aliases
        let mut semantic = TokenGroup::new("semantic").with_description("Semantic color aliases");
        semantic.add_token(
            DesignToken::color("color.semantic.background", OklchColor::new(0.99, 0.002, primary_hue), vec!["color".into(), "semantic".into()])
                .with_description("Application background"),
        );
        semantic.add_token(
            DesignToken::color("color.semantic.foreground", OklchColor::new(0.15, 0.01, primary_hue), vec!["color".into(), "semantic".into()])
                .with_description("Primary text color"),
        );
        semantic.add_token(
            DesignToken::color("color.semantic.muted", OklchColor::new(0.55, 0.01, primary_hue), vec!["color".into(), "semantic".into()])
                .with_description("Secondary/muted text"),
        );
        semantic.add_token(
            DesignToken::color("color.semantic.accent", OklchColor::new(0.55, primary_chroma, (primary_hue + 180.0) % 360.0), vec!["color".into(), "semantic".into()])
                .with_description("Accent / interactive color"),
        );
        semantic.add_token(
            DesignToken::color("color.semantic.border", OklchColor::new(0.87, 0.005, primary_hue), vec!["color".into(), "semantic".into()])
                .with_description("Default border color"),
        );
        color_group.add_child(semantic);

        system.tokens.add_child(color_group);

        // ── Spacing tokens ──
        let spacing_values: [(f64, &str); 13] = [
            (0.0, "0"),
            (0.125, "0.5"),
            (0.25, "1"),
            (0.375, "1.5"),
            (0.5, "2"),
            (0.75, "3"),
            (1.0, "4"),
            (1.5, "6"),
            (2.0, "8"),
            (3.0, "12"),
            (4.0, "16"),
            (6.0, "24"),
            (8.0, "32"),
        ];
        for (rem, label) in &spacing_values {
            spacing_group.add_token(DesignToken::new(
                format!("spacing.{}", label),
                TokenCategory::Spacing,
                format!("{}rem", rem),
                vec!["spacing".into()],
            ));
        }
        system.tokens.add_child(spacing_group);

        // ── Typography tokens ──
        let font_sizes: [(&str, &str); 8] = [
            ("xs", "0.75rem"),
            ("sm", "0.875rem"),
            ("base", "1rem"),
            ("lg", "1.125rem"),
            ("xl", "1.25rem"),
            ("2xl", "1.5rem"),
            ("3xl", "1.875rem"),
            ("4xl", "2.25rem"),
        ];
        for (name, size) in &font_sizes {
            typography_group.add_token(DesignToken::new(
                format!("font-size.{}", name),
                TokenCategory::Typography,
                size.to_string(),
                vec!["typography".into(), "font-size".into()],
            ));
        }

        // Font weights
        let weights: [(&str, &str); 5] = [
            ("normal", "400"),
            ("medium", "500"),
            ("semibold", "600"),
            ("bold", "700"),
            ("extrabold", "800"),
        ];
        for (name, weight) in &weights {
            typography_group.add_token(DesignToken::new(
                format!("font-weight.{}", name),
                TokenCategory::Typography,
                weight.to_string(),
                vec!["typography".into(), "font-weight".into()],
            ));
        }

        // Line heights
        let line_heights: [(&str, &str); 5] = [
            ("tight", "1.25"),
            ("snug", "1.375"),
            ("normal", "1.5"),
            ("relaxed", "1.625"),
            ("loose", "2.0"),
        ];
        for (name, lh) in &line_heights {
            typography_group.add_token(DesignToken::new(
                format!("line-height.{}", name),
                TokenCategory::Typography,
                lh.to_string(),
                vec!["typography".into(), "line-height".into()],
            ));
        }
        system.tokens.add_child(typography_group);

        // ── Radius tokens ──
        let radii: [(&str, &str); 6] = [
            ("none", "0"),
            ("sm", "0.125rem"),
            ("default", "0.25rem"),
            ("md", "0.375rem"),
            ("lg", "0.5rem"),
            ("xl", "0.75rem"),
        ];
        for (name, r) in &radii {
            radius_group.add_token(DesignToken::new(
                format!("radius.{}", name),
                TokenCategory::Radius,
                r.to_string(),
                vec!["radius".into()],
            ));
        }
        system.tokens.add_child(radius_group);

        // ── Shadow tokens ──
        let shadows: [(&str, &str); 4] = [
            ("sm", "0 1px 2px 0 rgb(0 0 0 / 0.05)"),
            ("default", "0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)"),
            ("md", "0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1)"),
            ("lg", "0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)"),
        ];
        for (name, shadow) in &shadows {
            shadow_group.add_token(DesignToken::new(
                format!("shadow.{}", name),
                TokenCategory::Shadow,
                shadow.to_string(),
                vec!["shadow".into()],
            ));
        }
        system.tokens.add_child(shadow_group);

        // ── Accessible components ──
        system.components.push(Self::build_button_spec());
        system.components.push(Self::build_card_spec());
        system.components.push(Self::build_input_spec());
        system.components.push(Self::build_badge_spec());

        // ── Design decisions ──
        system.add_decision(
            "Color space: OKLCH",
            "Use OKLCH for all color tokens",
            "OKLCH provides perceptually uniform lightness, enabling reliable contrast checking and automatic palette generation without the lightness distortion of HSL.",
            vec!["HSL (non-uniform lightness)".to_string(), "RGB (no perceptual model)".to_string(), "OKLAB (less intuitive hue angle)".to_string()],
        );
        system.add_decision(
            "WCAG conformance target",
            "Target WCAG 2.x Level AA (4.5:1 for normal text)",
            "AA is the industry-standard minimum. AAA (7:1) is desirable for large bodies of text but overly restrictive for UI components.",
            vec!["AA only".to_string(), "AAA only".to_string(), "No conformance target".to_string()],
        );
        system.add_decision(
            "Spacing scale",
            "4px base unit with a 2× geometric scale",
            "A 4px base allows fine-grained control while the geometric scale (0, 4, 8, 12, 16, 24, 32) covers all common spacing needs.",
            vec!["8px base (less granular)".to_string(), "Linear scale (too many stops)".to_string(), "Fibonacci-based (irregular for implementation)".to_string()],
        );

        system
    }

    /// Build an accessible Button component spec.
    fn build_button_spec() -> ComponentSpec {
        let mut button = ComponentSpec::new("Button", "Interactive button element with multiple variants")
            .with_wcag_level(WcagLevel::AA);

        button.add_a11y_note("Buttons must have visible focus indicators");
        button.add_a11y_note("Touch target size minimum 44×44px");
        button.add_a11y_note("Disabled state uses reduced opacity, not color alone");

        button.add_aria("role='button' (native <button> preferred)");
        button.add_aria("aria-disabled='true' when disabled");
        button.add_aria("aria-pressed for toggle buttons");

        button.add_keyboard("Enter and Space activate the button");
        button.add_keyboard("Tab navigates to button");
        button.add_keyboard("Shift+Tab navigates away from button");

        // Contrast checks: white text on primary backgrounds
        let white = OklchColor::white();
        let primary_500 = OklchColor::new(0.55, 0.15, 250.0);
        let primary_600 = OklchColor::new(0.44, 0.15, 250.0);
        let primary_700 = OklchColor::new(0.35, 0.12, 250.0);

        button.check_contrast("white", white, "primary.500", primary_500);
        button.check_contrast("white", white, "primary.600", primary_600);
        button.check_contrast("white", white, "primary.700", primary_700);

        // Variants
        button.add_variant(ComponentVariant {
            name: "primary".to_string(),
            token_refs: vec![
                ("color.primary.500".to_string(), "background-color".to_string()),
                ("white".to_string(), "color".to_string()),
                ("radius.default".to_string(), "border-radius".to_string()),
                ("spacing.2".to_string(), "padding-y".to_string()),
                ("spacing.4".to_string(), "padding-x".to_string()),
                ("font-weight.semibold".to_string(), "font-weight".to_string()),
            ],
            extra_styles: vec![
                ("border".to_string(), "none".to_string()),
                ("cursor".to_string(), "pointer".to_string()),
            ],
        });

        button.add_variant(ComponentVariant {
            name: "secondary".to_string(),
            token_refs: vec![
                ("color.secondary.500".to_string(), "background-color".to_string()),
                ("white".to_string(), "color".to_string()),
                ("radius.default".to_string(), "border-radius".to_string()),
            ],
            extra_styles: vec![],
        });

        button.add_variant(ComponentVariant {
            name: "outline".to_string(),
            token_refs: vec![
                ("transparent".to_string(), "background-color".to_string()),
                ("color.primary.500".to_string(), "color".to_string()),
                ("color.primary.500".to_string(), "border-color".to_string()),
                ("radius.default".to_string(), "border-radius".to_string()),
            ],
            extra_styles: vec![
                ("border-width".to_string(), "1px".to_string()),
            ],
        });

        button.add_variant(ComponentVariant {
            name: "ghost".to_string(),
            token_refs: vec![
                ("transparent".to_string(), "background-color".to_string()),
                ("color.neutral.700".to_string(), "color".to_string()),
            ],
            extra_styles: vec![
                ("border".to_string(), "none".to_string()),
            ],
        });

        button
    }

    /// Build an accessible Card component spec.
    fn build_card_spec() -> ComponentSpec {
        let mut card = ComponentSpec::new("Card", "Content container with optional header, body, and footer")
            .with_wcag_level(WcagLevel::AA);

        card.add_a11y_note("Card content should use semantic HTML (heading, paragraph)");
        card.add_a11y_note("Interactive cards must be focusable and keyboard navigable");

        card.add_aria("role='group' or semantic landmark for card sections");
        card.add_aria("aria-label if card purpose isn't clear from content");

        // Contrast: dark text on white/light card background
        let bg = OklchColor::new(0.99, 0.002, 250.0);
        let fg = OklchColor::new(0.15, 0.01, 250.0);
        let muted = OklchColor::new(0.55, 0.01, 250.0);
        let border = OklchColor::new(0.87, 0.005, 250.0);

        card.check_contrast("foreground", fg, "background", bg);
        card.check_contrast("muted", muted, "background", bg);
        card.check_contrast("border", border, "background", bg);

        card.add_variant(ComponentVariant {
            name: "default".to_string(),
            token_refs: vec![
                ("color.semantic.background".to_string(), "background-color".to_string()),
                ("color.semantic.border".to_string(), "border-color".to_string()),
                ("radius.lg".to_string(), "border-radius".to_string()),
                ("shadow.default".to_string(), "box-shadow".to_string()),
                ("spacing.4".to_string(), "padding".to_string()),
            ],
            extra_styles: vec![
                ("border-width".to_string(), "1px".to_string()),
            ],
        });

        card.add_variant(ComponentVariant {
            name: "elevated".to_string(),
            token_refs: vec![
                ("color.semantic.background".to_string(), "background-color".to_string()),
                ("shadow.md".to_string(), "box-shadow".to_string()),
                ("radius.lg".to_string(), "border-radius".to_string()),
            ],
            extra_styles: vec![
                ("border".to_string(), "none".to_string()),
            ],
        });

        card
    }

    /// Build an accessible Input component spec.
    fn build_input_spec() -> ComponentSpec {
        let mut input = ComponentSpec::new("Input", "Text input field with label, validation states, and error display")
            .with_wcag_level(WcagLevel::AA);

        input.add_a11y_note("Every input must have an associated <label>");
        input.add_a11y_note("Error messages must be associated with aria-describedby");
        input.add_a11y_note("Required fields indicated by aria-required='true'");
        input.add_a11y_note("Focus ring must be visible (minimum 3:1 contrast)");

        input.add_aria("aria-invalid='true' when validation fails");
        input.add_aria("aria-describedby pointing to error message element");
        input.add_aria("aria-required='true' for required fields");

        input.add_keyboard("Tab moves focus to input");
        input.add_keyboard("Shift+Tab moves focus away");
        input.add_keyboard("Type to enter text");

        // Contrast checks
        let bg = OklchColor::white();
        let fg = OklchColor::new(0.15, 0.01, 250.0);
        let border = OklchColor::new(0.76, 0.01, 250.0);
        let error = OklchColor::new(0.50, 0.20, 25.0);

        input.check_contrast("foreground", fg, "background", bg);
        input.check_contrast("border", border, "background", bg);
        input.check_contrast("error", error, "background", bg);

        input.add_variant(ComponentVariant {
            name: "default".to_string(),
            token_refs: vec![
                ("color.semantic.foreground".to_string(), "color".to_string()),
                ("color.semantic.background".to_string(), "background-color".to_string()),
                ("color.semantic.border".to_string(), "border-color".to_string()),
                ("radius.default".to_string(), "border-radius".to_string()),
                ("spacing.2".to_string(), "padding-y".to_string()),
                ("spacing.3".to_string(), "padding-x".to_string()),
                ("font-size.base".to_string(), "font-size".to_string()),
            ],
            extra_styles: vec![
                ("border-width".to_string(), "1px".to_string()),
            ],
        });

        input.add_variant(ComponentVariant {
            name: "error".to_string(),
            token_refs: vec![
                ("color.danger.500".to_string(), "border-color".to_string()),
                ("color.danger.600".to_string(), "color".to_string()),
            ],
            extra_styles: vec![
                ("border-width".to_string(), "2px".to_string()),
            ],
        });

        input
    }

    /// Build an accessible Badge component spec.
    fn build_badge_spec() -> ComponentSpec {
        let mut badge = ComponentSpec::new("Badge", "Inline status indicator / label")
            .with_wcag_level(WcagLevel::AA);

        badge.add_a11y_note("Badge text must meet contrast requirements against its background");
        badge.add_a11y_note("Status badges should include aria-label for screen readers");

        badge.add_aria("role='status' for dynamic badges");

        // Contrast checks: dark text on light badge backgrounds
        let fg = OklchColor::new(0.25, 0.10, 250.0);
        let bg_blue = OklchColor::new(0.93, 0.04, 250.0);
        let bg_green = OklchColor::new(0.93, 0.05, 145.0);
        let bg_red = OklchColor::new(0.93, 0.05, 25.0);

        badge.check_contrast("info-foreground", fg, "info-background", bg_blue);
        badge.check_contrast("success-foreground", fg, "success-background", bg_green);
        badge.check_contrast("danger-foreground", fg, "danger-background", bg_red);

        badge.add_variant(ComponentVariant {
            name: "default".to_string(),
            token_refs: vec![
                ("color.neutral.100".to_string(), "background-color".to_string()),
                ("color.neutral.700".to_string(), "color".to_string()),
                ("radius.default".to_string(), "border-radius".to_string()),
                ("spacing.1".to_string(), "padding-y".to_string()),
                ("spacing.2".to_string(), "padding-x".to_string()),
                ("font-size.xs".to_string(), "font-size".to_string()),
                ("font-weight.medium".to_string(), "font-weight".to_string()),
            ],
            extra_styles: vec![],
        });

        badge.add_variant(ComponentVariant {
            name: "info".to_string(),
            token_refs: vec![
                ("color.primary.100".to_string(), "background-color".to_string()),
                ("color.primary.700".to_string(), "color".to_string()),
            ],
            extra_styles: vec![],
        });

        badge.add_variant(ComponentVariant {
            name: "success".to_string(),
            token_refs: vec![
                ("color.success.100".to_string(), "background-color".to_string()),
                ("color.success.700".to_string(), "color".to_string()),
            ],
            extra_styles: vec![],
        });

        badge.add_variant(ComponentVariant {
            name: "danger".to_string(),
            token_refs: vec![
                ("color.danger.100".to_string(), "background-color".to_string()),
                ("color.danger.700".to_string(), "color".to_string()),
            ],
            extra_styles: vec![],
        });

        badge
    }

    /// Generate the skill-prompt instructions for the design-farmer skill.
    ///
    /// This is injected into the system prompt when the skill is active,
    /// instructing the LLM how to extract and build design systems.
    pub fn skill_prompt() -> String {
        r#"# Design Farmer Skill

You are running the **design-farmer** skill. Your job is to analyze codebases,
extract design patterns, and build comprehensive design systems with accessible
components.

## Workflow

### Phase 1: Analyze the Codebase

1. Scan the project for design-related files:
   - CSS/SCSS/LESS stylesheets
   - Tailwind or other CSS framework configs
   - Theme configuration files
   - Component files (React, Vue, Svelte)
   - Design token files (JSON, YAML)
2. Identify the styling approach (Tailwind, CSS Modules, CSS-in-JS, vanilla).
3. Catalog existing color values, spacing patterns, typography, and components.
4. Summarize findings as a `DesignAnalysis`.

### Phase 2: Extract Design Patterns

1. For each design file found, extract:
   - Color definitions (hex, rgb, hsl — convert to OKLCH)
   - Spacing scale values
   - Typography tokens (font families, sizes, weights, line heights)
   - Border radii
   - Shadow definitions
   - Breakpoints
2. Identify repeating patterns:
   - Which colors are used most frequently?
   - Is there an existing spacing scale?
   - Are typography sizes following a scale?
3. Document gaps: missing tokens, inconsistent values, unnamed colors.

### Phase 3: Build Token Hierarchy

1. Create a root `TokenGroup` with these sub-groups:
   - `color` — with palettes for primary, secondary, accent, neutral, semantic
   - `spacing` — geometric scale from 0 to 32rem
   - `typography` — font sizes, weights, line heights
   - `radius` — border radii from none to xl
   - `shadow` — elevation levels
2. Use **OKLCH** for all color tokens:
   - `oklch(L C H)` where L=lightness, C=chroma, H=hue
   - Generate full palettes by varying L at constant C and H
   - Steps: 50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950
3. Create semantic aliases (background, foreground, muted, accent, border).
4. Document each token with a description and group path.

### Phase 4: Implement Accessible Components

1. For each UI component, create a `ComponentSpec` with:
   - Name and description
   - Variants (primary, secondary, outline, ghost, etc.)
   - Token references (which tokens map to which CSS properties)
   - ARIA requirements
   - Keyboard interaction requirements
2. Run contrast checks:
   - Foreground vs background for every variant
   - Target: WCAG 2.x Level AA (4.5:1 for normal text, 3:1 for large text)
   - Fix failing combinations by adjusting lightness
3. Document accessibility notes for each component.

### Phase 5: Document and Export

1. Render the design system as Markdown.
2. Export as JSON for programmatic consumption.
3. Write to `docs/design/DESIGN.md` (or a specified path).
4. Include an accessibility report summarizing all contrast checks.

## Rules

- **Always use OKLCH** for color tokens. Never HSL or hex in token values.
- **Every component must have contrast checks.** No exceptions.
- **Target WCAG AA** as minimum. AA is non-negotiable; AAA is aspirational.
- **Document every design decision** with rationale and alternatives considered.
- **Prefer simplicity.** Don't create tokens that aren't needed.
- **Tokens are the source of truth.** Components reference tokens, not raw values.
- **No magic numbers.** Every value should trace back to a named token.

## OKLCH Quick Reference

```
oklch(L C H)       — opaque
oklch(L C H / A)   — with alpha

L: 0.0 (black) to 1.0 (white) — perceptually uniform
C: 0.0 (gray) to ~0.4 (vivid) — chroma, varies by hue
H: 0 to 360 — hue angle (0=red, 90=yellow, 145=green, 250=blue, 310=purple)
A: 0.0 (transparent) to 1.0 (opaque)

Contrast ratio:
  ratio = (L_lighter + 0.05) / (L_darker + 0.05)
  AA pass: ratio ≥ 4.5
  AAA pass: ratio ≥ 7.0

Palette generation:
  Keep C and H constant, vary L from 0.14 (950) to 0.97 (50)
  Compress C at extreme L to avoid gamut clipping
```
"#.to_string()
    }
}

impl Default for DesignFarmer {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for DesignFarmer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesignFarmer").finish()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── OklchColor tests ──

    #[test]
    fn test_oklch_new() {
        let c = OklchColor::new(0.5, 0.15, 250.0);
        assert!((c.l - 0.5).abs() < f64::EPSILON);
        assert!((c.c - 0.15).abs() < f64::EPSILON);
        assert!((c.h - 250.0).abs() < f64::EPSILON);
        assert!((c.alpha - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_oklch_with_alpha() {
        let c = OklchColor::new(0.5, 0.15, 250.0).with_alpha(0.5);
        assert!((c.alpha - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_oklch_alpha_clamped() {
        let c = OklchColor::new(0.5, 0.15, 250.0).with_alpha(1.5);
        assert!((c.alpha - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_oklch_black_white() {
        let black = OklchColor::black();
        assert!((black.l).abs() < f64::EPSILON);
        let white = OklchColor::white();
        assert!((white.l - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_oklch_to_css_opaque() {
        let c = OklchColor::new(0.5, 0.15, 250.0);
        let css = c.to_css();
        assert!(css.starts_with("oklch("));
        assert!(!css.contains("/")); // No alpha separator when opaque
    }

    #[test]
    fn test_oklch_to_css_transparent() {
        let c = OklchColor::new(0.5, 0.15, 250.0).with_alpha(0.5);
        let css = c.to_css();
        assert!(css.contains("/ 0.50"));
    }

    #[test]
    fn test_oklch_display() {
        let c = OklchColor::new(0.5, 0.15, 250.0);
        assert_eq!(format!("{}", c), c.to_css());
    }

    #[test]
    fn test_contrast_ratio_identical() {
        let c = OklchColor::new(0.5, 0.15, 250.0);
        let ratio = c.contrast_ratio(&c);
        assert!((ratio - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_contrast_ratio_black_white() {
        let ratio = OklchColor::black().contrast_ratio(&OklchColor::white());
        // Should be close to 21:1
        assert!(ratio > 15.0, "Expected high contrast, got {}", ratio);
    }

    #[test]
    fn test_aa_compliant() {
        let white = OklchColor::white();
        let dark_text = OklchColor::new(0.2, 0.0, 0.0);
        assert!(dark_text.is_aa_compliant(&white));
    }

    #[test]
    fn test_aaa_compliant() {
        let white = OklchColor::white();
        let very_dark = OklchColor::new(0.1, 0.0, 0.0);
        assert!(very_dark.is_aaa_compliant(&white));
    }

    #[test]
    fn test_lightness_scale() {
        let base = OklchColor::new(0.5, 0.15, 250.0);
        let scale = base.lightness_scale(5, 0.2, 0.8);
        assert_eq!(scale.len(), 5);
        assert!((scale[0].l - 0.2).abs() < f64::EPSILON);
        assert!((scale[4].l - 0.8).abs() < f64::EPSILON);
        // All should share chroma and hue
        for c in &scale {
            assert!((c.c - 0.15).abs() < f64::EPSILON);
            assert!((c.h - 250.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn test_lightness_scale_single_step() {
        let base = OklchColor::new(0.5, 0.15, 250.0);
        let scale = base.lightness_scale(1, 0.2, 0.8);
        assert_eq!(scale.len(), 1);
        // Single step returns the base color unchanged
        assert!((scale[0].l - 0.5).abs() < f64::EPSILON);
    }

    // ── Token tests ──

    #[test]
    fn test_token_new() {
        let token = DesignToken::new("spacing.4", TokenCategory::Spacing, "1rem", vec!["spacing".into()]);
        assert_eq!(token.name, "spacing.4");
        assert_eq!(token.category, TokenCategory::Spacing);
        assert_eq!(token.value, "1rem");
        assert!(token.oklch.is_none());
        assert!(token.alias.is_none());
    }

    #[test]
    fn test_token_color() {
        let color = OklchColor::new(0.5, 0.15, 250.0);
        let token = DesignToken::color("color.primary.500", color, vec!["color".into(), "primary".into()]);
        assert_eq!(token.category, TokenCategory::Color);
        assert!(token.oklch.is_some());
        assert_eq!(token.value, color.to_css());
    }

    #[test]
    fn test_token_with_description() {
        let token = DesignToken::new("spacing.4", TokenCategory::Spacing, "1rem", vec![])
            .with_description("Standard spacing unit");
        assert_eq!(token.description, Some("Standard spacing unit".to_string()));
    }

    #[test]
    fn test_token_alias() {
        let token = DesignToken::alias("color.semantic.accent", "color.primary.500", vec![]);
        assert_eq!(token.alias, Some("color.primary.500".to_string()));
    }

    // ── TokenGroup tests ──

    #[test]
    fn test_token_group_new() {
        let group = TokenGroup::new("color");
        assert_eq!(group.name, "color");
        assert!(group.tokens.is_empty());
        assert!(group.children.is_empty());
    }

    #[test]
    fn test_token_group_add_token() {
        let mut group = TokenGroup::new("spacing");
        group.add_token(DesignToken::new("spacing.4", TokenCategory::Spacing, "1rem", vec![]));
        assert_eq!(group.tokens.len(), 1);
    }

    #[test]
    fn test_token_group_add_child() {
        let mut root = TokenGroup::new("root");
        root.add_child(TokenGroup::new("color"));
        assert_eq!(root.children.len(), 1);
    }

    #[test]
    fn test_token_group_child_mut_creates() {
        let mut root = TokenGroup::new("root");
        let child = root.child_mut("color");
        child.add_token(DesignToken::new("color.primary", TokenCategory::Color, "blue", vec![]));
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].tokens.len(), 1);
    }

    #[test]
    fn test_token_group_child_mut_existing() {
        let mut root = TokenGroup::new("root");
        root.add_child(TokenGroup::new("color"));
        let child = root.child_mut("color");
        child.description = Some("Colors".to_string());
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].description, Some("Colors".to_string()));
    }

    #[test]
    fn test_token_group_all_tokens() {
        let mut root = TokenGroup::new("root");
        let mut color = TokenGroup::new("color");
        color.add_token(DesignToken::new("color.primary", TokenCategory::Color, "blue", vec![]));
        let spacing = TokenGroup::new("spacing");
        root.add_child(color);
        root.add_child(spacing);
        root.add_token(DesignToken::new("global.font", TokenCategory::Typography, "sans", vec![]));

        let all = root.all_tokens();
        assert_eq!(all.len(), 2); // color.primary + global.font
    }

    #[test]
    fn test_token_group_token_count() {
        let mut root = TokenGroup::new("root");
        let mut color = TokenGroup::new("color");
        color.add_token(DesignToken::new("a", TokenCategory::Color, "x", vec![]));
        color.add_token(DesignToken::new("b", TokenCategory::Color, "y", vec![]));
        root.add_child(color);
        root.add_token(DesignToken::new("c", TokenCategory::Spacing, "z", vec![]));
        assert_eq!(root.token_count(), 3);
    }

    #[test]
    fn test_token_group_get_child() {
        let mut root = TokenGroup::new("root");
        root.add_child(TokenGroup::new("color"));
        assert!(root.child("color").is_some());
        assert!(root.child("spacing").is_none());
    }

    // ── ColorPalette tests ──

    #[test]
    fn test_palette_generate() {
        let base = OklchColor::new(0.55, 0.15, 250.0);
        let palette = ColorPalette::generate("primary", &base);
        assert_eq!(palette.name, "primary");
        assert_eq!(palette.stops.len(), 11); // 50, 100, ..., 900, 950
        assert_eq!(palette.hue, 250.0);
    }

    #[test]
    fn test_palette_get_stop() {
        let base = OklchColor::new(0.55, 0.15, 250.0);
        let palette = ColorPalette::generate("primary", &base);
        assert!(palette.get_stop(500).is_some());
        assert!(palette.get_stop(50).is_some());
        assert!(palette.get_stop(950).is_some());
        assert!(palette.get_stop(999).is_none());
    }

    #[test]
    fn test_palette_mid() {
        let base = OklchColor::new(0.55, 0.15, 250.0);
        let palette = ColorPalette::generate("primary", &base);
        let mid = palette.mid();
        assert!((mid.l - 0.55).abs() < f64::EPSILON);
    }

    #[test]
    fn test_palette_neutral() {
        let palette = ColorPalette::neutral("gray");
        assert_eq!(palette.name, "gray");
        assert!((palette.chroma).abs() < f64::EPSILON);
        // All stops should have zero chroma
        for stop in &palette.stops {
            assert!((stop.color.c).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn test_palette_stop_token_names() {
        let base = OklchColor::new(0.55, 0.15, 250.0);
        let palette = ColorPalette::generate("primary", &base);
        assert_eq!(palette.stops[0].token_name, "color.primary.50");
        assert_eq!(palette.stops[5].token_name, "color.primary.500");
        assert_eq!(palette.stops[10].token_name, "color.primary.950");
    }

    #[test]
    fn test_palette_to_token_group() {
        let base = OklchColor::new(0.55, 0.15, 250.0);
        let palette = ColorPalette::generate("primary", &base);
        let group = palette.to_token_group();
        assert_eq!(group.name, "primary");
        assert_eq!(group.tokens.len(), 11);
    }

    // ── ContrastCheck tests ──

    #[test]
    fn test_contrast_check_passing() {
        let fg = OklchColor::new(0.2, 0.0, 0.0);
        let bg = OklchColor::white();
        let check = ContrastCheck::check("fg", fg, "bg", bg);
        assert!(check.aa_pass);
        assert!(check.ratio >= 4.5);
    }

    #[test]
    fn test_contrast_check_failing() {
        // L=0.65 is too light against white to pass AA
        let fg = OklchColor::new(0.65, 0.0, 0.0);
        let bg = OklchColor::white();
        let check = ContrastCheck::check("fg", fg, "bg", bg);
        // Light gray on white should fail AA
        assert!(!check.aa_pass, "Expected ratio < 4.5 but got {:.2}", check.ratio);
    }

    #[test]
    fn test_contrast_check_passes_level() {
        let fg = OklchColor::new(0.1, 0.0, 0.0);
        let bg = OklchColor::white();
        let check = ContrastCheck::check("fg", fg, "bg", bg);
        assert!(check.passes(WcagLevel::AA));
        assert!(check.passes(WcagLevel::AAA));
    }

    #[test]
    fn test_contrast_check_fails_level() {
        // L=0.6 vs white: ratio ≈ 3.95 (fails AA)
        let fg = OklchColor::new(0.6, 0.0, 0.0);
        let bg = OklchColor::white();
        let check = ContrastCheck::check("fg", fg, "bg", bg);
        assert!(!check.passes(WcagLevel::AA), "Expected ratio < 4.5 but got {:.2}", check.ratio);
        assert!(!check.passes(WcagLevel::AAA));
    }

    // ── ComponentSpec tests ──

    #[test]
    fn test_component_new() {
        let comp = ComponentSpec::new("Button", "A button");
        assert_eq!(comp.name, "Button");
        assert_eq!(comp.wcag_level, WcagLevel::AA);
        assert!(comp.variants.is_empty());
        assert!(comp.is_accessible()); // No checks = trivially accessible
    }

    #[test]
    fn test_component_with_wcag_level() {
        let comp = ComponentSpec::new("Button", "A button").with_wcag_level(WcagLevel::AAA);
        assert_eq!(comp.wcag_level, WcagLevel::AAA);
    }

    #[test]
    fn test_component_add_variant() {
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.add_variant(ComponentVariant {
            name: "primary".to_string(),
            token_refs: vec![("bg".to_string(), "background-color".to_string())],
            extra_styles: vec![],
        });
        assert_eq!(comp.variants.len(), 1);
    }

    #[test]
    fn test_component_check_contrast() {
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.check_contrast(
            "white",
            OklchColor::white(),
            "primary",
            OklchColor::new(0.5, 0.15, 250.0),
        );
        assert_eq!(comp.contrast_checks.len(), 1);
    }

    #[test]
    fn test_component_is_accessible_passing() {
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.check_contrast(
            "dark-text",
            OklchColor::new(0.15, 0.0, 0.0),
            "white-bg",
            OklchColor::white(),
        );
        assert!(comp.is_accessible());
    }

    #[test]
    fn test_component_is_accessible_failing() {
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.check_contrast(
            "light-text",
            OklchColor::new(0.65, 0.0, 0.0),
            "white-bg",
            OklchColor::white(),
        );
        assert!(!comp.is_accessible());
    }

    #[test]
    fn test_component_failing_checks() {
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.check_contrast("good", OklchColor::new(0.15, 0.0, 0.0), "white", OklchColor::white());
        comp.check_contrast("bad", OklchColor::new(0.65, 0.0, 0.0), "white", OklchColor::white());
        let failing = comp.failing_checks();
        assert_eq!(failing.len(), 1);
    }

    #[test]
    fn test_component_a11y_notes() {
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.add_a11y_note("Must have focus indicator");
        assert_eq!(comp.a11y_notes.len(), 1);
    }

    #[test]
    fn test_component_aria_requirements() {
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.add_aria("aria-disabled='true'");
        assert_eq!(comp.aria_requirements.len(), 1);
    }

    #[test]
    fn test_component_keyboard_requirements() {
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.add_keyboard("Enter activates");
        assert_eq!(comp.keyboard_requirements.len(), 1);
    }

    // ── DesignSystem tests ──

    #[test]
    fn test_design_system_new() {
        let system = DesignSystem::new("test-system");
        assert_eq!(system.name, "test-system");
        assert_eq!(system.version, "1.0.0");
        assert!(system.palettes.is_empty());
        assert!(system.components.is_empty());
        assert!(system.analysis.is_none());
    }

    #[test]
    fn test_design_system_add_palette() {
        let mut system = DesignSystem::new("test");
        let palette = ColorPalette::generate("primary", &OklchColor::new(0.55, 0.15, 250.0));
        system.add_palette(palette);
        assert_eq!(system.palettes.len(), 1);
        // Tokens should also be registered
        assert!(system.tokens.token_count() > 0);
    }

    #[test]
    fn test_design_system_total_tokens_empty() {
        let system = DesignSystem::new("test");
        assert_eq!(system.total_tokens(), 0);
    }

    #[test]
    fn test_design_system_add_decision() {
        let mut system = DesignSystem::new("test");
        system.add_decision(
            "Color space",
            "OKLCH",
            "Perceptually uniform",
            vec!["HSL".to_string()],
        );
        assert_eq!(system.decisions.len(), 1);
        assert_eq!(system.decisions[0].title, "Color space");
    }

    #[test]
    fn test_design_system_accessibility_report_all_pass() {
        let mut system = DesignSystem::new("test");
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.check_contrast("dark", OklchColor::new(0.15, 0.0, 0.0), "white", OklchColor::white());
        system.components.push(comp);
        let report = system.accessibility_report();
        assert!(report.all_pass);
        assert_eq!(report.total_checks, 1);
        assert_eq!(report.passing_count, 1);
        assert_eq!(report.failing_count, 0);
    }

    #[test]
    fn test_design_system_accessibility_report_with_failures() {
        let mut system = DesignSystem::new("test");
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.check_contrast("light", OklchColor::new(0.65, 0.0, 0.0), "white", OklchColor::white());
        system.components.push(comp);
        let report = system.accessibility_report();
        assert!(!report.all_pass);
        assert_eq!(report.failing_count, 1);
    }

    // ── build_design_system tests ──

    #[test]
    fn test_build_design_system() {
        let system = DesignFarmer::build_design_system("test-system", 250.0, 0.15, None);
        assert_eq!(system.name, "test-system");
        assert!(!system.palettes.is_empty());
        assert!(!system.components.is_empty());
        assert!(system.tokens.token_count() > 50, "Expected many tokens, got {}", system.tokens.token_count());
        assert!(!system.decisions.is_empty());
    }

    #[test]
    fn test_build_design_system_has_all_component_types() {
        let system = DesignFarmer::build_design_system("test", 250.0, 0.15, None);
        let names: Vec<&str> = system.components.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Button"));
        assert!(names.contains(&"Card"));
        assert!(names.contains(&"Input"));
        assert!(names.contains(&"Badge"));
    }

    #[test]
    fn test_build_design_system_has_all_token_groups() {
        let system = DesignFarmer::build_design_system("test", 250.0, 0.15, None);
        let child_names: Vec<&str> = system.tokens.children.iter().map(|c| c.name.as_str()).collect();
        assert!(child_names.contains(&"color"));
        assert!(child_names.contains(&"spacing"));
        assert!(child_names.contains(&"typography"));
        assert!(child_names.contains(&"radius"));
        assert!(child_names.contains(&"shadow"));
    }

    // ── Codebase analysis tests ──

    #[test]
    fn test_analyze_codebase_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let analysis = DesignFarmer::analyze_codebase(tmp.path()).unwrap();
        assert_eq!(analysis.design_files.len(), 0);
        assert_eq!(analysis.framework, DesignFramework::Unknown);
    }

    #[test]
    fn test_analyze_codebase_detects_tailwind() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("tailwind.config.js"), "module.exports = {}").unwrap();
        let analysis = DesignFarmer::analyze_codebase(tmp.path()).unwrap();
        assert_eq!(analysis.framework, DesignFramework::Tailwind);
    }

    #[test]
    fn test_analyze_codebase_detects_css_in_js() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"dependencies": {"styled-components": "^6.0.0"}}"#,
        )
        .unwrap();
        let analysis = DesignFarmer::analyze_codebase(tmp.path()).unwrap();
        assert_eq!(analysis.framework, DesignFramework::CssInJs);
    }

    #[test]
    fn test_analyze_codebase_finds_css_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("components")).unwrap();
        fs::write(src.join("global.css"), "body { margin: 0; }").unwrap();
        fs::write(src.join("components").join("Button.css"), ".btn { color: blue; }").unwrap();
        fs::write(src.join("components").join("Button.tsx"), "<button />").unwrap();

        let analysis = DesignFarmer::analyze_codebase(tmp.path()).unwrap();
        let file_types: Vec<_> = analysis.design_files.iter().map(|f| f.file_type).collect();
        assert!(file_types.iter().any(|t| *t == DesignFileType::Stylesheet));
        assert!(file_types.iter().any(|t| *t == DesignFileType::Component));
    }

    #[test]
    fn test_analyze_codebase_skips_noise() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        fs::create_dir_all(tmp.path().join("dist")).unwrap();

        let analysis = DesignFarmer::analyze_codebase(tmp.path()).unwrap();
        for f in &analysis.design_files {
            assert!(!f.path.starts_with(".git/"));
            assert!(!f.path.starts_with("node_modules/"));
            assert!(!f.path.starts_with("dist/"));
        }
    }

    #[test]
    fn test_analyze_codebase_with_theme_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("theme.json"), "{\"colors\": {\"primary\": \"#0066ff\"}}").unwrap();
        let analysis = DesignFarmer::analyze_codebase(tmp.path()).unwrap();
        assert!(analysis.design_files.iter().any(|f| f.file_type == DesignFileType::ThemeConfig));
    }

    // ── Markdown rendering tests ──

    #[test]
    fn test_render_markdown_empty() {
        let system = DesignSystem::new("test");
        let md = system.render_markdown();
        assert!(md.contains("# Design System: test"));
        assert!(md.contains("0 tokens"));
    }

    #[test]
    fn test_render_markdown_with_palette() {
        let mut system = DesignSystem::new("test");
        let palette = ColorPalette::generate("primary", &OklchColor::new(0.55, 0.15, 250.0));
        system.add_palette(palette);
        let md = system.render_markdown();
        assert!(md.contains("## Color Palettes"));
        assert!(md.contains("### primary"));
        assert!(md.contains("| 500 |"));
    }

    #[test]
    fn test_render_markdown_with_components() {
        let mut system = DesignSystem::new("test");
        let mut comp = ComponentSpec::new("Button", "A button");
        comp.check_contrast("dark", OklchColor::new(0.15, 0.0, 0.0), "white", OklchColor::white());
        system.components.push(comp);
        let md = system.render_markdown();
        assert!(md.contains("## Components"));
        assert!(md.contains("### Button"));
        assert!(md.contains("✅"));
    }

    #[test]
    fn test_render_markdown_with_decisions() {
        let mut system = DesignSystem::new("test");
        system.add_decision("Color space", "OKLCH", "Uniform", vec!["HSL".to_string()]);
        let md = system.render_markdown();
        assert!(md.contains("## Design Decisions"));
        assert!(md.contains("### Color space"));
        assert!(md.contains("Alternatives considered"));
    }

    #[test]
    fn test_render_markdown_accessibility_report() {
        let system = DesignSystem::new("test");
        let md = system.render_markdown();
        assert!(md.contains("## Accessibility Report"));
    }

    // ── File I/O tests ──

    #[test]
    fn test_write_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let system = DesignFarmer::build_design_system("test", 250.0, 0.15, None);
        let path = tmp.path().join("DESIGN.md");
        let result = system.write_markdown(&path).unwrap();
        assert!(result.exists());
        let content = fs::read_to_string(&result).unwrap();
        assert!(content.contains("# Design System: test"));
    }

    #[test]
    fn test_write_json() {
        let tmp = tempfile::tempdir().unwrap();
        let system = DesignFarmer::build_design_system("test", 250.0, 0.15, None);
        let path = tmp.path().join("design.json");
        let result = system.write_json(&path).unwrap();
        assert!(result.exists());
        let content = fs::read_to_string(&result).unwrap();
        assert!(content.contains("\"name\": \"test\""));
    }

    #[test]
    fn test_write_markdown_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let system = DesignSystem::new("test");
        let path = tmp.path().join("docs").join("design").join("DESIGN.md");
        let result = system.write_markdown(&path).unwrap();
        assert!(result.exists());
    }

    // ── Serialization roundtrip tests ──

    #[test]
    fn test_oklch_serde_roundtrip() {
        let c = OklchColor::new(0.5, 0.15, 250.0).with_alpha(0.8);
        let json = serde_json::to_string(&c).unwrap();
        let parsed: OklchColor = serde_json::from_str(&json).unwrap();
        assert!((parsed.l - c.l).abs() < f64::EPSILON);
        assert!((parsed.c - c.c).abs() < f64::EPSILON);
        assert!((parsed.h - c.h).abs() < f64::EPSILON);
        assert!((parsed.alpha - c.alpha).abs() < f64::EPSILON);
    }

    #[test]
    fn test_design_system_serde_roundtrip() {
        let system = DesignFarmer::build_design_system("test", 250.0, 0.15, None);
        let json = serde_json::to_string_pretty(&system).unwrap();
        let parsed: DesignSystem = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.palettes.len(), system.palettes.len());
        assert_eq!(parsed.components.len(), system.components.len());
        assert_eq!(parsed.total_tokens(), system.total_tokens());
    }

    #[test]
    fn test_component_spec_serde_roundtrip() {
        let mut comp = ComponentSpec::new("Button", "A button").with_wcag_level(WcagLevel::AA);
        comp.check_contrast("fg", OklchColor::new(0.2, 0.0, 0.0), "bg", OklchColor::white());
        comp.add_variant(ComponentVariant {
            name: "primary".to_string(),
            token_refs: vec![("bg".to_string(), "background-color".to_string())],
            extra_styles: vec![],
        });

        let json = serde_json::to_string(&comp).unwrap();
        let parsed: ComponentSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "Button");
        assert_eq!(parsed.contrast_checks.len(), 1);
        assert_eq!(parsed.variants.len(), 1);
    }

    // ── Skill prompt test ──

    #[test]
    fn test_skill_prompt_not_empty() {
        let prompt = DesignFarmer::skill_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("Design Farmer Skill"));
        assert!(prompt.contains("Phase 1: Analyze"));
        assert!(prompt.contains("Phase 2: Extract"));
        assert!(prompt.contains("Phase 3: Build Token Hierarchy"));
        assert!(prompt.contains("Phase 4: Implement Accessible Components"));
        assert!(prompt.contains("Phase 5: Document and Export"));
        assert!(prompt.contains("OKLCH"));
        assert!(prompt.contains("WCAG"));
    }

    // ── Display trait tests ──

    #[test]
    fn test_token_category_display() {
        assert_eq!(format!("{}", TokenCategory::Color), "color");
        assert_eq!(format!("{}", TokenCategory::Spacing), "spacing");
        assert_eq!(format!("{}", TokenCategory::Typography), "typography");
        assert_eq!(format!("{}", TokenCategory::Radius), "radius");
        assert_eq!(format!("{}", TokenCategory::Shadow), "shadow");
        assert_eq!(format!("{}", TokenCategory::Opacity), "opacity");
        assert_eq!(format!("{}", TokenCategory::Breakpoint), "breakpoint");
        assert_eq!(format!("{}", TokenCategory::ZIndex), "z-index");
        assert_eq!(format!("{}", TokenCategory::Motion), "motion");
        assert_eq!(format!("{}", TokenCategory::Custom), "custom");
    }

    #[test]
    fn test_wcag_level_display() {
        assert_eq!(format!("{}", WcagLevel::None), "none");
        assert_eq!(format!("{}", WcagLevel::A), "A");
        assert_eq!(format!("{}", WcagLevel::AA), "AA");
        assert_eq!(format!("{}", WcagLevel::AAA), "AAA");
    }

    #[test]
    fn test_design_framework_display() {
        assert_eq!(format!("{}", DesignFramework::Tailwind), "Tailwind CSS");
        assert_eq!(format!("{}", DesignFramework::CssModules), "CSS Modules");
        assert_eq!(format!("{}", DesignFramework::CssInJs), "CSS-in-JS");
        assert_eq!(format!("{}", DesignFramework::Vanilla), "Vanilla CSS");
        assert_eq!(format!("{}", DesignFramework::Unknown), "Unknown");
    }

    #[test]
    fn test_design_file_type_display() {
        assert_eq!(format!("{}", DesignFileType::Stylesheet), "stylesheet");
        assert_eq!(format!("{}", DesignFileType::TailwindConfig), "tailwind-config");
        assert_eq!(format!("{}", DesignFileType::Component), "component");
    }

    // ── Default trait tests ──

    #[test]
    fn test_design_farmer_default() {
        let _farmer = DesignFarmer::default();
    }

    #[test]
    fn test_design_farmer_debug() {
        let farmer = DesignFarmer::new();
        let debug = format!("{:?}", farmer);
        assert!(debug.contains("DesignFarmer"));
    }
}
