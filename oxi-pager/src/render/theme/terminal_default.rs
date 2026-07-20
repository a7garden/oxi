//! Terminal-native palette for minimal mode.
//!
//! Any RGB theme is designed for one background polarity, so composited on
//! the terminal's own canvas it can land dark-on-dark or light-on-light
//! (e.g. macOS in Light Mode + a dark terminal profile). Polarity detection
//! is not reliable either: OS appearance and OSC 11 both disagree with the
//! actual canvas in edge cases and can change mid-session. Terminal
//! profiles, however, tune their **default** fg/bg to be legible against
//! their own background — this is how `git` and `ls` stay readable on any
//! terminal — so a palette built from `Reset` (body) + sparse named ANSI-16
//! accents is polarity-safe without detection.
//!
//! ## Grays / secondary text
//!
//! Do **not** paint body or status text as `DarkGray` (ANSI bright black).
//! Many dark profiles deliberately set that slot very dark for subtle
//! chrome, which washes out tool stdout and the prompt info bar. Instead:
//!
//! - **Primary content** (`text_primary`, `gray_bright`, …) → `Color::Reset`
//!   (terminal default foreground).
//! - **Secondary chrome** (`gray`, `gray_dim`, `text_secondary`) → also
//!   `Color::Reset`; [`Theme::muted`] / [`Theme::dim`] apply `Modifier::DIM`
//!   so de-emphasis tracks the terminal's own fg (polarity-safe), unlike
//!   hard-coding bright black.
//! - **Syntax highlighting** is not themed day/night. Under the native lock,
//!   syntect tokens are remapped via
//!   [`crate::render::grok::syntax::polarity_safe_syntax_fg`] (default-fg grays + base ANSI
//!   accents). Do not load a light tmTheme based on OS/terminal detection.

use ratatui::style::{Color, Modifier};

use super::Theme;

impl Theme {
    /// The fixed terminal-native palette used by minimal mode: every field
    /// is `Color::Reset` or a named ANSI-16 color (see the module docs).
    pub const fn terminal_default() -> Self {
        // Secondary roles store Reset; Theme::muted / Theme::dim apply SGR dim.
        const MUTED: Color = Color::Reset;

        Self {
            bg_base: Color::Reset,
            bg_light: Color::Reset,
            bg_dark: Color::Reset,
            bg_highlight: Color::Reset,
            bg_hover: Color::Reset,
            bg_terminal: Color::Reset,

            accent_user: Color::Reset,
            accent_assistant: Color::Magenta,
            accent_thinking: MUTED,
            accent_tool: MUTED,
            accent_system: Color::Blue,
            accent_error: Color::Red,
            accent_success: Color::Green,
            accent_running: Color::Magenta,
            accent_skill: Color::Blue,

            text_primary: Color::Reset,
            text_secondary: MUTED,

            gray_dim: MUTED,
            gray: MUTED,
            gray_bright: Color::Reset,

            command: Color::Yellow,
            path: Color::Cyan,
            running: Color::Cyan,
            warning: Color::Yellow,

            fuzzy_accent: Color::Cyan,

            accent_plan: Color::Yellow,
            accent_verify: Color::Magenta,
            accent_feedback: Color::Cyan,
            accent_remember: Color::Green,

            selection_border: MUTED,
            hover_border: MUTED,
            prompt_border: MUTED,
            prompt_border_active: Color::Reset,

            accent_model: Color::Cyan,

            scrollbar_bg: Color::Reset,
            scrollbar_fg: MUTED,

            diff_delete_bg: Color::Reset,
            diff_delete_fg: Color::Red,
            diff_insert_bg: Color::Reset,
            diff_insert_fg: Color::Green,
            diff_equal_fg: MUTED,
            diff_gutter_fg: MUTED,

            bg_visual: Color::Reset,

            paste_bg: Color::Reset,
            paste_fg: MUTED,
            paste_dim: MUTED,

            md_heading_h1: Color::Reset,
            md_heading_h1_mod: Modifier::BOLD.union(Modifier::UNDERLINED),
            md_heading_h2: Color::Reset,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: Color::Reset,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: Color::Reset,
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: Color::Reset,
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: MUTED,
            md_heading_h6_mod: Modifier::BOLD,
            md_code: Color::Cyan,
            md_task_checked: Color::Green,
            md_task_unchecked: MUTED,
            md_muted: MUTED,
            md_code_bg: Color::Reset,
            md_text: Color::Reset,
            link_fg: Color::Blue,
        }
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
