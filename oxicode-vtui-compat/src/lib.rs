#![allow(
    missing_docs,
    clippy::expect_used,
    dead_code,
    unused_imports,
    unexpected_cfgs
)]
#![allow(
    clippy::let_and_return,
    clippy::borrow_interior_mutable_const,
    clippy::derivable_impls
)]
// oxicode-vtui-compat — Compatibility stubs replacing vtcode-config + vtcode-commons
// for the vendored vtcode-ui.
//
// MIT licensed — provides the minimum surface area needed for oxicode-vtui to compile.

// ── Re-export ui_protocol from vtcode-commons ──────────────────────────────
pub mod ui_protocol;
// ui_protocol/ contains: InlineMessageKind, InlineSegment, InlineTextStyle,
// InlineTheme, InlineHeaderContext, SlashCommandItem, ThinkingBlockState, etc.

// ── constants (originally vtcode_config::constants) ────────────────────────
pub mod constants {
    pub mod ui {
        pub const TOOL_OUTPUT_MODE_COMPACT: &str = "compact";
        pub const TOOL_OUTPUT_MODE_FULL: &str = "full";
        pub const DEFAULT_REASONING_VISIBLE: bool = false;
        pub const INLINE_PTY_PLACEHOLDER: &str = "...";
        pub const INLINE_PTY_STATUS_DONE: &str = "DONE";
        pub const HEADER_UNKNOWN_PLACEHOLDER: &str = "\u{2014}";
        pub const HEADER_GIT_DIRTY_SUFFIX: &str = "*";
        pub const CHAT_INPUT_PLACEHOLDER_BOOTSTRAP: &str =
            "Describe what you want to build\u{2026}";
        pub const CHAT_INPUT_PLACEHOLDER_FOLLOW_UP: &str = "Follow-up\u{2026}";
        pub const WELCOME_TEXT_WIDTH: usize = 72;
        pub const WELCOME_SHORTCUT_SECTION_TITLE: &str = "Shortcuts";
        pub const WELCOME_SHORTCUT_HINT_PREFIX: &str = "  ";
        pub const WELCOME_SHORTCUT_SEPARATOR: &str = " \u{2022} ";
        pub const WELCOME_SHORTCUT_INDENT: &str = "    ";
        pub const WELCOME_SLASH_COMMAND_SECTION_TITLE: &str = "Commands";
        pub const WELCOME_SLASH_COMMAND_LIMIT: usize = 20;
        pub const WELCOME_SLASH_COMMAND_PREFIX: &str = "  /";
        pub const WELCOME_SLASH_COMMAND_INTRO: &str = "Use / to open the command palette";
        pub const WELCOME_SLASH_COMMAND_INDENT: &str = "      ";
        pub const INLINE_USER_PREFIX: &str = " ";
        pub const HEADER_SHORTCUT_HINT: &str = "";
        pub const HEADER_META_SEPARATOR: &str = "   ";

        pub const THEME_RELATIVE_LUMINANCE_CUTOFF: f32 = 0.03928;
        pub const THEME_RELATIVE_LUMINANCE_LOW_FACTOR: f32 = 12.92;
        pub const THEME_RELATIVE_LUMINANCE_OFFSET: f32 = 0.055;
        pub const THEME_RELATIVE_LUMINANCE_EXPONENT: f32 = 2.4;
        pub const THEME_RED_LUMINANCE_COEFFICIENT: f32 = 0.2126;
        pub const THEME_GREEN_LUMINANCE_COEFFICIENT: f32 = 0.7152;
        pub const THEME_BLUE_LUMINANCE_COEFFICIENT: f32 = 0.0722;
        pub const THEME_CONTRAST_RATIO_OFFSET: f32 = 0.05;
        pub const THEME_MIX_RATIO_MIN: f32 = 0.0;
        pub const THEME_MIX_RATIO_MAX: f32 = 1.0;
        pub const THEME_BLEND_CLAMP_MIN: f32 = 0.0;
        pub const THEME_BLEND_CLAMP_MAX: f32 = 255.0;
        pub const THEME_COLOR_WHITE_RED: f32 = 1.0;
        pub const THEME_COLOR_WHITE_GREEN: f32 = 1.0;
        pub const THEME_COLOR_WHITE_BLUE: f32 = 1.0;
        pub const THEME_LOGO_ACCENT_BANNER_LIGHTEN_RATIO: f32 = 0.15;
        pub const THEME_PRIMARY_STATUS_SECONDARY_LIGHTEN_RATIO: f32 = 0.7;
        pub const THEME_LOGO_ACCENT_BANNER_SECONDARY_LIGHTEN_RATIO: f32 = 0.5;
        pub const THEME_MIN_CONTRAST_RATIO: f32 = 3.0;
        pub const THEME_FOREGROUND_LIGHTEN_RATIO: f32 = 0.2;
        pub const THEME_SECONDARY_LIGHTEN_RATIO: f32 = 0.8;
        pub const THEME_MIX_RATIO: f32 = 0.0;
        pub const THEME_TOOL_BODY_LIGHTEN_RATIO: f32 = 0.1;
        pub const THEME_TOOL_BODY_MIX_RATIO: f32 = 0.0;
        pub const THEME_PTY_OUTPUT_MIX_RATIO: f32 = 0.0;
        pub const THEME_RESPONSE_COLOR_LIGHTEN_RATIO: f32 = 0.1;
        pub const THEME_USER_COLOR_LIGHTEN_RATIO: f32 = 0.1;
        pub const THEME_SECONDARY_USER_COLOR_LIGHTEN_RATIO: f32 = 0.3;
        pub const THEME_LUMINANCE_LIGHTEN_RATIO: f32 = 0.1;
        pub const THEME_PRIMARY_STATUS_LIGHTEN_RATIO: f32 = 0.3;
        pub const THEME_LOGO_ACCENT_BANNER_RATIO: f32 = 0.15;

        pub const HEADER_STATUS_LABEL: &str = "Status";
        pub const HEADER_STATUS_ACTIVE: &str = "Active";
        pub const HEADER_STATUS_PAUSED: &str = "Paused";
        pub const MODAL_LIST_HIGHLIGHT_SYMBOL: &str = "\u{2502}";
        pub const MODAL_LIST_HIGHLIGHT_FULL: &str = "\u{2502} ";
        pub const INLINE_FILE_PICKER_TREE_PREFIX: &str = "\u{25B8} ";
        pub const NAVIGATION_BLOCK_TITLE: &str = "Timeline";
        pub const NAVIGATION_BLOCK_SHORTCUT_NOTE: &str = "Ctrl+T";
        pub const NAVIGATION_EMPTY_LABEL: &str = "Waiting for activity";
        pub const SLASH_PALETTE_MIN_WIDTH: u16 = 40;
        pub const SLASH_PALETTE_MIN_HEIGHT: u16 = 9;
        pub const DEFAULT_INLINE_VIEWPORT_ROWS: u16 = 16;
        // Agent mode hue resolution (oxicode has no concept of agent modes — return None)
        pub fn agent_mode_hue(_token: &str) -> Option<&str> {
            None
        }
        // Additional constants sometimes referenced
        pub const AGENT_COLOR_AUTO: &str = "auto";
        pub const AGENT_COLOR_BUILD: &str = "build";
        pub const AGENT_COLOR_DUCK: &str = "duck";
        pub const AGENT_COLOR_PLAN: &str = "plan";
    }

    pub mod defaults {
        pub fn default_provider() -> String {
            "openai".into()
        }
        pub fn default_model() -> String {
            "gpt-4o".into()
        }
        pub const DEFAULT_THEME: &str = "ciapre-dark";
    }

    pub mod tools {
        pub const GREP_FILE: &str = "grep_file";
        pub const LIST_FILES: &str = "list_files";
        pub const READ_FILE: &str = "read_file";
        pub const EDIT_FILE: &str = "edit_file";
        pub const WRITE_FILE: &str = "write_file";
        pub const CREATE_FILE: &str = "create_file";
        pub const APPLY_PATCH: &str = "apply_patch";
        pub const SEARCH_REPLACE: &str = "search_replace";
        pub const DELETE_FILE: &str = "delete_file";
        pub const UNIFIED_FILE: &str = "unified_file";
    }
}

// ── vtcode_config::core::tools ─────────────────────────────────────────────
pub mod core {
    pub mod tools {
        /// Policy for whether a tool requires user approval.
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
        )]
        pub enum ToolPolicy {
            #[default]
            Allow,
            Ask,
            Prompt,
            Deny,
        }
    }
}

// ── vtcode_config::types ───────────────────────────────────────────────────
pub mod types {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum SystemPromptMode {
        Default,
        Compact,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub enum ToolDocumentationMode {
        #[default]
        Full,
        Compact,
        Progressive,
        None,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub enum VerbosityLevel {
        #[default]
        Quiet,
        Normal,
        Medium,
        Verbose,
    }
}

// ── Common utility stubs ───────────────────────────────────────────────────
pub mod reasoning {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub enum ReasoningEffortLevel {
        #[default]
        Low,
        Medium,
        High,
    }
}

pub mod stop_hints {
    pub const STOP_HINT_COMPACT: &str = "\u{2026}";
    pub const STOP_HINT_INLINE: &str = "\u{2026}";
}

pub mod terminal_detection {
    pub fn is_ghostty_terminal(_term_program: Option<&str>, _term: Option<&str>) -> bool {
        false
    }
}

pub mod trace_flush {
    pub fn flush_trace_log() {}
}

pub mod exclusions {
    pub fn is_sensitive_file(_file_name: &str) -> bool {
        false
    }
}

pub mod color_policy {
    pub fn no_color_env_active() -> bool {
        false
    }
}

pub mod formatting {
    pub fn clean_reasoning_text(text: &str) -> String {
        text.to_string()
    }
}

pub mod ansi_codes {
    pub const RESET: &str = "\x1b[0m";
}

pub mod editor {
    pub fn normalize_editor_hash_fragment(_text: &str) -> String {
        String::new()
    }
    pub fn parse_editor_target(_s: &str) -> Option<EditorTarget> {
        None
    }

    #[derive(Debug, Clone)]
    pub struct EditorTarget {
        pub path: String,
        pub line: Option<usize>,
        pub col: Option<usize>,
    }
    impl EditorTarget {
        pub fn path(&self) -> &std::path::Path {
            std::path::Path::new(&self.path)
        }
        pub fn canonical_string(&self) -> String {
            let mut s = self.path.clone();
            if let Some(line) = self.line {
                s.push_str(&format!(":{}", line));
                if let Some(col) = self.col {
                    s.push_str(&format!(":{}", col));
                }
            }
            s
        }
    }

    #[derive(Debug, Clone)]
    pub struct EditorPoint {
        pub path: String,
        pub line: usize,
        pub col: usize,
    }
}

pub mod color256_theme {
    use anstyle::RgbColor;
    pub fn rgb_to_ansi256_for_theme(_rgb: RgbColor, _light: bool) -> Option<u8> {
        None
    }
}

pub mod errors {
    pub type MultiErrors = Vec<anyhow::Error>;
}
pub use errors::MultiErrors;

// ── ansi module ────────────────────────────────────────────────────────────
pub mod ansi {
    /// Strip all ANSI escape sequences from a string.
    pub fn strip_ansi(s: &str) -> String {
        let re = regex::Regex::new("\x1b\\[[0-9;]*[a-zA-Z]").unwrap();
        re.replace_all(s, "").to_string()
    }
    pub fn strip_ansi_codes(s: &str) -> String {
        strip_ansi(s)
    }
}

// ── ansi_capabilities ──────────────────────────────────────────────────────
pub mod ansi_capabilities {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ColorScheme {
        Light,
        Dark,
        Unknown,
    }
    pub fn detect_color_scheme() -> ColorScheme {
        ColorScheme::Dark
    }
}

// ── colors ─────────────────────────────────────────────────────────────────
pub mod colors {
    use anstyle::{AnsiColor, Color, RgbColor};
    pub fn blend_colors(a: Color, b: Color, _ratio: f32) -> Color {
        // Minimal stub: blend by picking the second color when ratio > 0.5,
        // otherwise the first. This avoids complex anstyle type gymnastics.
        if _ratio > 0.5 { b } else { a }
    }
}

// ── diff modules (minimal stubs) ───────────────────────────────────────────
pub mod diff {
    #[derive(Clone, Debug)]
    pub struct DiffOptions {
        pub context: usize,
        pub old_label: Option<String>,
        pub new_label: Option<String>,
        pub missing_newline_hint: Option<String>,
    }
    impl Default for DiffOptions {
        fn default() -> Self {
            Self {
                context: 3,
                old_label: None,
                new_label: None,
                missing_newline_hint: None,
            }
        }
    }
    #[derive(Clone, Debug)]
    pub enum DiffLineKind {
        Context,
        Addition,
        Deletion,
    }
    #[derive(Clone, Debug)]
    pub struct DiffLine {
        pub kind: DiffLineKind,
        pub text: String,
        pub content: String,
    }
    #[derive(Clone, Debug)]
    pub struct DiffHunk {
        pub old_start: usize,
        pub new_start: usize,
        pub old_lines: Vec<DiffLine>,
        pub new_lines: Vec<DiffLine>,
        pub lines: Vec<DiffLine>,
    }
    #[derive(Clone, Debug)]
    pub struct DiffBundle {
        pub old_path: Option<String>,
        pub new_path: Option<String>,
        pub hunks: Vec<DiffHunk>,
        pub formatted: String,
    }
    #[derive(Clone, Debug)]
    pub struct Chunk {
        pub lines: Vec<DiffLine>,
    }
    pub fn compute_diff(old: &[String], new: &[String], opts: &DiffOptions) -> DiffBundle {
        let _ = (old, new, opts);
        DiffBundle {
            old_path: None,
            new_path: None,
            hunks: vec![],
            formatted: String::new(),
        }
    }
    pub fn compute_diff_chunks(
        _old: &[String],
        _new_lines: &[String],
        _opts: &DiffOptions,
    ) -> Vec<Chunk> {
        vec![]
    }
}

pub mod diff_paths {
    pub fn is_diff_addition_line(_line: &str) -> bool {
        false
    }
    pub fn is_diff_deletion_line(_line: &str) -> bool {
        false
    }
    pub fn language_hint_from_path(_path: &str) -> Option<&'static str> {
        None
    }
    pub fn is_diff_header_line(_line: &str) -> bool {
        false
    }
    pub fn is_diff_new_file_marker_line(_line: &str) -> bool {
        false
    }
    pub fn looks_like_diff_content(_text: &str) -> bool {
        false
    }
    pub fn parse_diff_git_path(_line: &str) -> Option<String> {
        None
    }
    pub fn parse_diff_marker_path(_line: &str) -> Option<String> {
        None
    }
    pub fn format_start_only_hunk_header(_old: usize, _new: usize) -> String {
        String::new()
    }
}

pub mod diff_preview {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum DiffDisplayKind {
        Unified,
        Split,
    }
    pub fn count_diff_changes(_lines: &[String]) -> (usize, usize) {
        (0, 0)
    }
    pub fn display_lines_from_hunks(_hunks: &[()], _kind: DiffDisplayKind) -> Vec<String> {
        vec![]
    }
}

pub mod diff_theme {
    use crate::styling::DiffColorPalette;
    use anstyle::{AnsiColor, Color};

    #[derive(Clone, Debug)]
    pub struct DiffTheme {
        pub add_fg: Color,
        pub add_bg: Color,
        pub del_fg: Color,
        pub del_bg: Color,
        pub hunk_fg: Color,
        pub hunk_bg: Color,
        pub gutter_add_bg_light: Color,
        pub gutter_del_bg_light: Color,
        pub gutter_fg_light: Color,
    }
    impl Default for DiffTheme {
        fn default() -> Self {
            Self {
                add_fg: Color::Ansi(AnsiColor::Green),
                add_bg: Color::Ansi(AnsiColor::Black),
                del_fg: Color::Ansi(AnsiColor::Red),
                del_bg: Color::Ansi(AnsiColor::Black),
                hunk_fg: Color::Ansi(AnsiColor::Cyan),
                hunk_bg: Color::Ansi(AnsiColor::Black),
                gutter_add_bg_light: Color::Ansi(AnsiColor::Black),
                gutter_del_bg_light: Color::Ansi(AnsiColor::Black),
                gutter_fg_light: Color::Ansi(AnsiColor::Black),
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub enum DiffColorLevel {
        Full,
        Ansi256,
        Ansi16,
        None,
    }

    pub fn diff_add_bg(_level: DiffColorLevel) -> Color {
        DiffTheme::default().add_bg
    }
    pub fn diff_del_bg(_level: DiffColorLevel) -> Color {
        DiffTheme::default().del_bg
    }
    pub fn diff_gutter_bg_add_light(_level: DiffColorLevel) -> Color {
        DiffTheme::default().gutter_add_bg_light
    }
    pub fn diff_gutter_bg_del_light(_level: DiffColorLevel) -> Color {
        DiffTheme::default().gutter_del_bg_light
    }
    pub fn diff_gutter_fg_light(_level: DiffColorLevel) -> Color {
        DiffTheme::default().gutter_fg_light
    }

    pub fn default_diff_palette() -> DiffColorPalette {
        DiffColorPalette::default()
    }
}

// ── styling ────────────────────────────────────────────────────────────────
pub mod styling {
    use anstyle::{AnsiColor, Color, RgbColor};

    #[derive(Debug, Clone)]
    pub struct ColorPalette {
        pub primary: Color,
        pub secondary: Color,
        pub background: Color,
        pub foreground: Color,
        pub error: Color,
        pub warning: Color,
        pub success: Color,
    }

    impl Default for ColorPalette {
        fn default() -> Self {
            Self {
                primary: Color::Ansi(AnsiColor::Blue),
                secondary: Color::Ansi(AnsiColor::Cyan),
                background: Color::Rgb(RgbColor(30, 30, 30)),
                foreground: Color::Ansi(AnsiColor::White),
                error: Color::Ansi(AnsiColor::Red),
                warning: Color::Ansi(AnsiColor::Yellow),
                success: Color::Ansi(AnsiColor::Green),
            }
        }
    }

    /// Color palette for diff rendering.
    #[derive(Debug, Clone)]
    pub struct DiffColorPalette {
        pub addition_fg: Color,
        pub addition_bg: Color,
        pub deletion_fg: Color,
        pub deletion_bg: Color,
        pub hunk_header: Color,
        pub context: Color,
    }

    impl Default for DiffColorPalette {
        fn default() -> Self {
            Self {
                addition_fg: Color::Rgb(RgbColor(0x50, 0xFA, 0x7B)),
                addition_bg: Color::Rgb(RgbColor(0x1B, 0x3B, 0x20)),
                deletion_fg: Color::Rgb(RgbColor(0xFF, 0x55, 0x55)),
                deletion_bg: Color::Rgb(RgbColor(0x3B, 0x1B, 0x1B)),
                hunk_header: Color::Ansi(AnsiColor::Cyan),
                context: Color::Rgb(RgbColor(128, 128, 128)),
            }
        }
    }
}

// ── fs utilities ───────────────────────────────────────────────────────────
pub mod fs {
    use std::path::Path;

    pub fn read_file_with_context_sync(
        path: &Path,
        _context_lines: usize,
    ) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(path)?)
    }

    pub fn write_file_with_context_sync(path: &Path, _content: &str) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, _content)?;
        Ok(())
    }

    pub fn is_image_path(_path: &str) -> bool {
        false
    }

    pub fn trim_trailing_image_path_str(_s: &str) -> String {
        _s.to_string()
    }

    pub fn unescape_whitespace(s: &str) -> String {
        s.replace("\\n", "\n").replace("\\t", "\t")
    }
}

// ── lr_map ─────────────────────────────────────────────────────────────────
pub mod lr_map {
    /// Minimal Left-Right Map — a concurrent lock-free map with two copies
    /// (one for writes, one for reads), swapped atomically.
    pub struct LrMap<'a, K, V> {
        _phantom: std::marker::PhantomData<&'a (K, V)>,
    }

    impl<'a, K, V> LrMap<'a, K, V> {
        pub fn new() -> Self {
            Self {
                _phantom: std::marker::PhantomData,
            }
        }
        pub fn insert(&self, _key: K, _value: V) {}
        pub fn refresh(&self) {}
    }
    impl<'a, K, V> LrMap<'a, K, V> {
        pub fn get(&self, _key: &K) -> Option<&V> {
            None
        }
    }

    impl<'a, K, V> Default for LrMap<'a, K, V> {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<'a, K, V> Clone for LrMap<'a, K, V> {
        fn clone(&self) -> Self {
            Self {
                _phantom: std::marker::PhantomData,
            }
        }
    }
}

// ── preview ────────────────────────────────────────────────────────────────
pub mod preview;

// ── Re-exports needed by vendored vtcode-ui ────────────────────────────────
pub use colors::blend_colors;
pub use editor::{EditorPoint, EditorTarget, normalize_editor_hash_fragment, parse_editor_target};
pub use stop_hints::STOP_HINT_COMPACT;
pub use styling::DiffColorPalette;
