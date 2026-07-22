//! Glyph set system: pluggable Unicode / ASCII / Nerd-Font symbol presets.
//!
//! Terminals vary wildly in their glyph coverage. A box-drawing-heavy Unicode
//! UI renders as tofu on a 7-bit serial console; a Nerd-Font UI needs a patched
//! font. This module gives every widget one [`Symbols`] table, sourced from a
//! [`GlyphSet`] preset, so the whole UI can switch between three rendering
//! styles from a single setting (default: [`GlyphSet::Unicode`]).
//!
//! The symbol codepoints themselves are fixed by external standards — the
//! Unicode block-drawing range (U+2500–U+257F) and the Nerd Fonts private-use
//! codepoints — so a preset is just a mapping from semantic key to standard
//! codepoint. Every field is a `&'static str`, which keeps [`Symbols`] `Copy`
//! and lets it ride along inside [`crate::theme::ThemeStyles`] with no
//! allocation and no lifetime plumbing.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// GlyphSet
// ---------------------------------------------------------------------------

/// One of three terminal glyph rendering styles.
///
/// Stored in `settings.toml` as `glyph_set = "unicode"` (snake_case). The
/// default is [`GlyphSet::Unicode`], which renders on any UTF-8 terminal
/// without requiring a patched font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlyphSet {
    /// Pure Unicode box-drawing + emoji glyphs. Works on any UTF-8 terminal
    /// with no extra font. **This is the default.**
    #[default]
    Unicode,
    /// 7-bit ASCII fallback. Renders cleanly on serial consoles, CI logs,
    /// and terminals with no Unicode support at all.
    Ascii,
    /// Nerd Font private-use codepoints. Requires a Nerd Font patched font;
    /// gives the richest per-tool / per-language icons.
    Nerd,
}

impl GlyphSet {
    /// All presets in the order shown to users in pickers (Unicode first,
    /// since it is the default and the safest choice).
    pub const ALL: [GlyphSet; 3] = [GlyphSet::Unicode, GlyphSet::Ascii, GlyphSet::Nerd];

    /// Human-readable label for picker rows.
    pub const fn label(self) -> &'static str {
        match self {
            GlyphSet::Unicode => "Unicode",
            GlyphSet::Ascii => "ASCII",
            GlyphSet::Nerd => "Nerd Font",
        }
    }

    /// One-line sample rendered next to the label in pickers / setup wizard,
    /// so the user can see whether their terminal handles the preset.
    pub fn sample(self) -> &'static str {
        match self {
            GlyphSet::Unicode => "✔ ✖ ⚠ ● ▸ ▾ ╭─╮ │ └─┘ ⠋ ⣾ ↑ ↓ ⚙ 🔧",
            GlyphSet::Ascii => "[ok] [x] [!] [*] > + - +-+ | +-+ | \\ -",
            GlyphSet::Nerd => {
                "\u{f00c} \u{f00d} \u{f071} \u{f111} \u{f0da} \u{f0d7} \u{e0b0} \u{f126} \u{f013} \u{f077} \u{f078} \u{f0ad}"
            }
        }
    }

    /// Resolve a preset to its concrete [`Symbols`] table.
    pub const fn symbols(self) -> Symbols {
        match self {
            GlyphSet::Unicode => Symbols::unicode(),
            GlyphSet::Ascii => Symbols::ascii(),
            GlyphSet::Nerd => Symbols::nerd(),
        }
    }
}

impl fmt::Display for GlyphSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for GlyphSet {
    type Err = UnknownGlyphSet;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "unicode" | "uni" | "default" => Ok(GlyphSet::Unicode),
            "ascii" => Ok(GlyphSet::Ascii),
            "nerd" | "nerdfont" | "nerd-font" | "nerd_font" => Ok(GlyphSet::Nerd),
            other => Err(UnknownGlyphSet(other.to_string())),
        }
    }
}

/// Error returned when a settings string does not name a known [`GlyphSet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownGlyphSet(pub String);

impl fmt::Display for UnknownGlyphSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown glyph set '{}': expected one of unicode, ascii, nerd",
            self.0
        )
    }
}

impl std::error::Error for UnknownGlyphSet {}

// ---------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------

/// A complete table of UI glyphs for one [`GlyphSet`].
///
/// Every field is a `&'static str`, so a `Symbols` value is tiny and `Copy`.
/// Widgets read glyphs off this struct instead of hardcoding codepoints, so a
/// single setting flip re-skins the whole UI.
///
/// **Adding a new glyph:** add the field here, populate it in all three
/// preset constructors ([`Symbols::unicode`] / [`Symbols::ascii`] / [`Symbols::nerd`]), and
/// migrate the one hardcoded call site to read `styles.symbols.<field>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbols {
    // ── Status ───────────────────────────────────────────────────────
    /// Success / done-ok marker.
    pub status_success: &'static str,
    /// Error / failure marker.
    pub status_error: &'static str,
    /// Warning / caution marker.
    pub status_warning: &'static str,
    /// Informational marker.
    pub status_info: &'static str,
    /// Pending / waiting marker.
    pub status_pending: &'static str,
    /// Running / in-progress marker.
    pub status_running: &'static str,
    /// Aborted / stopped marker.
    pub status_aborted: &'static str,
    /// Generic "done" dot.
    pub status_done: &'static str,

    // ── Health / state dots ──────────────────────────────────────────
    /// Filled dot — "enabled" / "healthy" / "active".
    pub dot_on: &'static str,
    /// Hollow dot — "disabled" / "unavailable" / "inactive".
    pub dot_off: &'static str,

    // ── Navigation / selection ───────────────────────────────────────
    /// Prefix drawn before the highlighted row in lists.
    pub cursor: &'static str,
    /// "Selected" indicator (single-choice / radio).
    pub nav_selected: &'static str,
    /// Expand a collapsed node.
    pub nav_expand: &'static str,
    /// Collapse an expanded node.
    pub nav_collapse: &'static str,
    /// Back / previous.
    pub nav_back: &'static str,
    /// Forward arrow.
    pub arrow_right: &'static str,
    /// Backward arrow.
    pub arrow_left: &'static str,
    /// Upward arrow.
    pub arrow_up: &'static str,
    /// Downward arrow.
    pub arrow_down: &'static str,

    // ── Tree connectors ──────────────────────────────────────────────
    /// Mid-tree branch connector (e.g. `├─`).
    pub tree_branch: &'static str,
    /// Last-child connector (e.g. `└─`).
    pub tree_last: &'static str,
    /// Vertical trunk (e.g. `│`).
    pub tree_vertical: &'static str,
    /// Horizontal connector (e.g. `─`).
    pub tree_horizontal: &'static str,

    // ── Box drawing — rounded ────────────────────────────────────────
    /// Rounded box: top-left corner.
    pub round_tl: &'static str,
    /// Rounded box: top-right corner.
    pub round_tr: &'static str,
    /// Rounded box: bottom-left corner.
    pub round_bl: &'static str,
    /// Rounded box: bottom-right corner.
    pub round_br: &'static str,
    /// Rounded box: horizontal edge.
    pub round_h: &'static str,
    /// Rounded box: vertical edge.
    pub round_v: &'static str,

    // ── Box drawing — sharp ──────────────────────────────────────────
    /// Sharp box: top-left corner.
    pub sharp_tl: &'static str,
    /// Sharp box: top-right corner.
    pub sharp_tr: &'static str,
    /// Sharp box: bottom-left corner.
    pub sharp_bl: &'static str,
    /// Sharp box: bottom-right corner.
    pub sharp_br: &'static str,
    /// Sharp box: horizontal edge.
    pub sharp_h: &'static str,
    /// Sharp box: vertical edge.
    pub sharp_v: &'static str,
    /// Sharp box: four-way cross.
    pub sharp_cross: &'static str,
    /// Sharp box: tee pointing right (`├`) — left junction of a section bar.
    pub sharp_tee_right: &'static str,
    /// Sharp box: tee pointing left (`┤`) — right junction of a section bar.
    pub sharp_tee_left: &'static str,
    /// Sharp box: tee pointing up (`┬`) — used as the inter-column
    /// junction on the **top** row of a table. (`sharp_tee_right` and
    /// `sharp_tee_left` are horizontal tees for separator rows.)
    pub sharp_tee_up: &'static str,
    /// Sharp box: tee pointing down (`┴`) — used as the inter-column
    /// junction on the **bottom** row of a table.
    pub sharp_tee_down: &'static str,

    // ── Separators ───────────────────────────────────────────────────
    /// Filled block bar (e.g. `▌` / `#`).
    pub sep_block: &'static str,
    /// Mid-dot separator, padded (e.g. ` · `).
    pub sep_dot: &'static str,
    /// Pipe separator, padded (e.g. ` │ `).
    pub sep_pipe: &'static str,
    /// Horizontal rule character, repeated to draw a divider (e.g. `─`).
    pub rule: &'static str,

    // ── Formatting ───────────────────────────────────────────────────
    /// Unordered list bullet.
    pub bullet: &'static str,
    /// Em/EN dash.
    pub dash: &'static str,

    // ── Checkbox / radio ─────────────────────────────────────────────
    /// Checkbox: checked state.
    pub checkbox_on: &'static str,
    /// Checkbox: unchecked state.
    pub checkbox_off: &'static str,
    /// Radio (single-choice): selected state.
    pub radio_on: &'static str,
    /// Radio (single-choice): unselected state.
    pub radio_off: &'static str,

    // ── Common icons ─────────────────────────────────────────────────
    /// Folder icon.
    pub icon_folder: &'static str,
    /// Generic file icon.
    pub icon_file: &'static str,
    /// Search / magnifier icon.
    pub icon_search: &'static str,
    /// Git / VCS icon.
    pub icon_git: &'static str,
    /// Branch icon.
    pub icon_branch: &'static str,
    /// Model / hexagon icon.
    pub icon_model: &'static str,
    /// Cost / money icon.
    pub icon_cost: &'static str,
    /// Clock / time icon.
    pub icon_time: &'static str,
    /// Token-count icon.
    pub icon_tokens: &'static str,
    /// Context-window icon.
    pub icon_context: &'static str,
    /// Prompt / input icon.
    pub icon_prompt: &'static str,
    /// Warning icon.
    pub icon_warning: &'static str,
    /// Todo / checklist icon.
    pub icon_todo: &'static str,

    // ── Thinking / activity ──────────────────────────────────────────────
    /// Thinking / processing gear icon.
    pub icon_thinking: &'static str,
    /// Thinking level: off / disabled.
    pub icon_thinking_off: &'static str,
    /// Thinking level: minimal effort.
    pub icon_thinking_minimal: &'static str,
    /// Thinking level: low effort.
    pub icon_thinking_low: &'static str,
    /// Thinking level: medium effort.
    pub icon_thinking_medium: &'static str,
    /// Thinking level: high effort.
    pub icon_thinking_high: &'static str,
    /// Thinking level: extreme / maximum effort.
    pub icon_thinking_xhigh: &'static str,

    // ── Language icons (code block headers) ─────────────────────────
    /// Rust language icon.
    pub icon_lang_rust: &'static str,
    /// Python language icon.
    pub icon_lang_python: &'static str,
    /// JavaScript language icon.
    pub icon_lang_javascript: &'static str,
    /// TypeScript language icon.
    pub icon_lang_typescript: &'static str,
    /// Go language icon.
    pub icon_lang_go: &'static str,
    /// Java language icon.
    pub icon_lang_java: &'static str,
    /// C / C++ language icon.
    pub icon_lang_cpp: &'static str,
    /// C# language icon.
    pub icon_lang_csharp: &'static str,
    /// Swift language icon.
    pub icon_lang_swift: &'static str,
    /// Kotlin language icon.
    pub icon_lang_kotlin: &'static str,
    /// Shell / Bash language icon.
    pub icon_lang_shell: &'static str,
    /// HTML language icon.
    pub icon_lang_html: &'static str,
    /// CSS language icon.
    pub icon_lang_css: &'static str,
    /// JSON language icon.
    pub icon_lang_json: &'static str,
    /// SQL language icon.
    pub icon_lang_sql: &'static str,
    /// Docker language icon.
    pub icon_lang_docker: &'static str,
    /// Generic / unknown language fallback icon.
    pub icon_lang_default: &'static str,

    // ── Per-tool identity glyphs (success header signature) ──────────
    /// Bash / shell tool glyph.
    pub tool_bash: &'static str,
    /// Edit tool glyph.
    pub tool_edit: &'static str,
    /// Write tool glyph.
    pub tool_write: &'static str,
    /// Read tool glyph.
    pub tool_read: &'static str,
    /// Search tool glyph.
    pub tool_search: &'static str,
    /// Subagent / task tool glyph.
    pub tool_task: &'static str,
    /// Web tool glyph.
    pub tool_web: &'static str,
    /// LSP tool glyph.
    pub tool_lsp: &'static str,
    /// Debugger tool glyph.
    pub tool_debug: &'static str,
    /// MCP tool glyph.
    pub tool_mcp: &'static str,
    /// Ask / questionnaire tool glyph.
    pub tool_ask: &'static str,
    /// Generic / fallback tool glyph for unknown tools.
    pub tool_generic: &'static str,

    // ── Spinner animation frames ─────────────────────────────────────
    /// "Status" spinner — tight, for inline status indicators.
    pub spinner_status: &'static [&'static str],
    /// "Activity" spinner — looser, for longer-running activity.
    pub spinner_activity: &'static [&'static str],
}

impl Symbols {
    /// Unicode preset — the default. Box-drawing + a few emoji-ish glyphs.
    pub const fn unicode() -> Self {
        Self {
            status_success: "✔",
            status_error: "✘",
            status_warning: "⚠",
            status_info: "ⓘ",
            status_pending: "⏳",
            status_running: "⟳",
            status_aborted: "⏹",
            status_done: "•",
            dot_on: "●",
            dot_off: "○",
            cursor: "❯ ",
            nav_selected: "➤",
            nav_expand: "▸",
            nav_collapse: "▾",
            nav_back: "⟵",
            arrow_right: "→",
            arrow_left: "←",
            arrow_up: "↑",
            arrow_down: "↓",
            tree_branch: "├─",
            tree_last: "└─",
            tree_vertical: "│",
            tree_horizontal: "─",
            round_tl: "╭",
            round_tr: "╮",
            round_bl: "╰",
            round_br: "╯",
            round_h: "─",
            round_v: "│",
            sharp_tl: "┌",
            sharp_tr: "┐",
            sharp_bl: "└",
            sharp_br: "┘",
            sharp_h: "─",
            sharp_v: "│",
            sharp_cross: "┼",
            sharp_tee_right: "├",
            sharp_tee_left: "┤",
            sharp_tee_up: "┬",
            sharp_tee_down: "┴",
            sep_block: "▌",
            sep_dot: " · ",
            sep_pipe: " │ ",
            rule: "─",
            bullet: "•",
            dash: "—",
            checkbox_on: "☑",
            checkbox_off: "☐",
            radio_on: "◉",
            radio_off: "○",
            icon_folder: "📁",
            icon_file: "📄",
            icon_search: "🔍",
            icon_git: "⎇",
            icon_branch: "⑂",
            icon_model: "⬢",
            icon_cost: "💲",
            icon_time: "⏱",
            icon_tokens: "🪙",
            icon_context: "◫",
            icon_prompt: "❯",
            icon_warning: "⚠",
            icon_todo: "📋",
            icon_thinking: "⚙",
            icon_thinking_off: "○ off",
            icon_thinking_minimal: "◔ min",
            icon_thinking_low: "◑ low",
            icon_thinking_medium: "◒ med",
            icon_thinking_high: "◕ high",
            icon_thinking_xhigh: "◉ xhi",
            tool_bash: "❯",
            tool_edit: "✎",
            tool_write: "✎",
            tool_read: "📖",
            tool_search: "⌕",
            tool_task: "⇶",
            tool_web: "🌐",
            tool_lsp: "💡",
            tool_debug: "🐞",
            tool_mcp: "🔌",
            tool_ask: "?",
            tool_generic: "🔧",
            icon_lang_rust: "rs",
            icon_lang_python: "py",
            icon_lang_javascript: "js",
            icon_lang_typescript: "ts",
            icon_lang_go: "go",
            icon_lang_java: "java",
            icon_lang_cpp: "c++",
            icon_lang_csharp: "cs",
            icon_lang_swift: "sw",
            icon_lang_kotlin: "kt",
            icon_lang_shell: "sh",
            icon_lang_html: "html",
            icon_lang_css: "css",
            icon_lang_json: "json",
            icon_lang_sql: "sql",
            icon_lang_docker: "docker",
            icon_lang_default: "?",
            spinner_status: SPINNER_STATUS_UNICODE,
            spinner_activity: SPINNER_ACTIVITY_UNICODE,
        }
    }

    /// ASCII preset — 7-bit fallback for serial consoles / CI logs.
    pub const fn ascii() -> Self {
        Self {
            status_success: "[ok]",
            status_error: "[!!]",
            status_warning: "[!]",
            status_info: "[i]",
            status_pending: "[*]",
            status_running: "[~]",
            status_aborted: "[-]",
            status_done: "*",
            dot_on: "[*]",
            dot_off: "[ ]",
            cursor: "> ",
            nav_selected: "->",
            nav_expand: "+",
            nav_collapse: "-",
            nav_back: "<-",
            arrow_right: "->",
            arrow_left: "<-",
            arrow_up: "^",
            arrow_down: "v",
            tree_branch: "|--",
            tree_last: "`--",
            tree_vertical: "|",
            tree_horizontal: "-",
            round_tl: "+",
            round_tr: "+",
            round_bl: "+",
            round_br: "+",
            round_h: "-",
            round_v: "|",
            sharp_tl: "+",
            sharp_tr: "+",
            sharp_bl: "+",
            sharp_br: "+",
            sharp_h: "-",
            sharp_v: "|",
            sharp_cross: "+",
            sharp_tee_right: "+",
            sharp_tee_left: "+",
            sharp_tee_up: "+",
            sharp_tee_down: "+",
            sep_block: "#",
            sep_dot: " - ",
            sep_pipe: " | ",
            rule: "-",
            bullet: "*",
            dash: "-",
            checkbox_on: "[x]",
            checkbox_off: "[ ]",
            radio_on: "(*)",
            radio_off: "( )",
            icon_folder: "[D]",
            icon_file: "[F]",
            icon_search: "[/]",
            icon_git: "git:",
            icon_branch: "@",
            icon_model: "[M]",
            icon_cost: "$",
            icon_time: "t:",
            icon_tokens: "tok:",
            icon_context: "ctx:",
            icon_prompt: ">",
            icon_warning: "[!]",
            icon_todo: "[T]",
            icon_thinking: "[~]",
            icon_thinking_off: "[off]",
            icon_thinking_minimal: "[min]",
            icon_thinking_low: "[low]",
            icon_thinking_medium: "[med]",
            icon_thinking_high: "[high]",
            icon_thinking_xhigh: "[xhi]",
            tool_bash: "$",
            tool_edit: "~",
            tool_write: "+f",
            tool_read: "cat",
            tool_search: "/",
            tool_task: ">>>",
            tool_web: "web",
            tool_lsp: "lsp",
            tool_debug: "dbg",
            tool_mcp: "<>",
            tool_ask: "[?]",
            tool_generic: "[*]",
            icon_lang_rust: "[rs]",
            icon_lang_python: "[py]",
            icon_lang_javascript: "[js]",
            icon_lang_typescript: "[ts]",
            icon_lang_go: "[go]",
            icon_lang_java: "[java]",
            icon_lang_cpp: "[c++]",
            icon_lang_csharp: "[cs]",
            icon_lang_swift: "[sw]",
            icon_lang_kotlin: "[kt]",
            icon_lang_shell: "[sh]",
            icon_lang_html: "[html]",
            icon_lang_css: "[css]",
            icon_lang_json: "[json]",
            icon_lang_sql: "[sql]",
            icon_lang_docker: "[docker]",
            icon_lang_default: "[?]",
            spinner_status: SPINNER_STATUS_ASCII,
            spinner_activity: SPINNER_ACTIVITY_ASCII,
        }
    }

    /// Nerd Font preset — requires a patched font; richest icon set.
    pub const fn nerd() -> Self {
        Self {
            status_success: "\u{f00c}", //
            status_error: "\u{f00d}",   //
            status_warning: "\u{f071}", //
            status_info: "\u{f129}",    //
            status_pending: "\u{f254}", //
            status_running: "\u{f110}", //
            status_aborted: "\u{f04d}", //
            status_done: "\u{f111}",    //
            dot_on: "\u{f111}",         //
            dot_off: "\u{f10c}",        //
            cursor: "\u{f054} ",        //
            nav_selected: "\u{f178}",   //
            nav_expand: "\u{f0da}",     //
            nav_collapse: "\u{f0d7}",   //
            nav_back: "\u{f060}",       //
            arrow_right: "\u{f054}",    //
            arrow_left: "\u{f053}",     //
            arrow_up: "\u{f077}",       //
            arrow_down: "\u{f078}",     //
            tree_branch: "├─",
            tree_last: "└─",
            tree_vertical: "│",
            tree_horizontal: "─",
            round_tl: "╭",
            round_tr: "╮",
            round_bl: "╰",
            round_br: "╯",
            round_h: "─",
            round_v: "│",
            sharp_tl: "┌",
            sharp_tr: "┐",
            sharp_bl: "└",
            sharp_br: "┘",
            sharp_h: "─",
            sharp_v: "│",
            sharp_cross: "┼",
            sharp_tee_right: "├",
            sharp_tee_left: "┤",
            sharp_tee_up: "┬",
            sharp_tee_down: "┴",
            sep_block: "\u{e0b0}", //
            sep_dot: " \u{f111} ",
            sep_pipe: " \u{e0b3} ",
            rule: "─",
            bullet: "\u{f111}", //
            dash: "–",
            checkbox_on: "\u{f14a}",               //
            checkbox_off: "\u{f096}",              //
            radio_on: "\u{f192}",                  //
            radio_off: "\u{f10c}",                 //
            icon_folder: "\u{f115}",               //
            icon_file: "\u{f15b}",                 //
            icon_search: "\u{f002}",               //
            icon_git: "\u{f1d3}",                  //
            icon_branch: "\u{f126}",               //
            icon_model: "\u{ec19}",                //
            icon_cost: "\u{f155}",                 //
            icon_time: "\u{f017}",                 //
            icon_tokens: "\u{e26b}",               //
            icon_context: "\u{e70f}",              //
            icon_prompt: "\u{f054}",               //
            icon_warning: "\u{f071}",              //
            icon_todo: "\u{f03a}",                 //
            icon_thinking: "\u{f013}",             //
            icon_thinking_off: "\u{f05e} off",     //
            icon_thinking_minimal: "\u{f0e7} min", //
            icon_thinking_low: "\u{f10c} low",     //
            icon_thinking_medium: "\u{f192} med",  //
            icon_thinking_high: "\u{f111} high",   //
            icon_thinking_xhigh: "\u{f06d} xhi",   //
            tool_bash: "\u{ebca}",                 //
            tool_edit: "\u{ea73}",                 //
            tool_write: "\u{ea7f}",                //
            tool_read: "\u{f02d}",                 //
            tool_search: "\u{f002}",               //
            tool_task: "\u{f4a0}",                 //
            tool_web: "\u{eaae}",                  //
            tool_lsp: "\u{ea61}",                  //
            tool_debug: "\u{ead8}",                //
            tool_mcp: "\u{eb2d}",                  //
            tool_ask: "\u{f059}",                  //
            tool_generic: "\u{f0ad}",              //
            icon_lang_rust: "\u{e7a8} rs",
            icon_lang_python: "\u{e73c} py",
            icon_lang_javascript: "\u{e781} js",
            icon_lang_typescript: "\u{e6a6} ts",
            icon_lang_go: "\u{e724} go",
            icon_lang_java: "\u{e738} java",
            icon_lang_cpp: "\u{e61d} c++",
            icon_lang_csharp: "\u{e64a} cs",
            icon_lang_swift: "\u{e755} sw",
            icon_lang_kotlin: "\u{e70c} kt",
            icon_lang_shell: "\u{e795} sh",
            icon_lang_html: "\u{e736} html",
            icon_lang_css: "\u{e749} css",
            icon_lang_json: "\u{e787} json",
            icon_lang_sql: "\u{e706} sql",
            icon_lang_docker: "\u{e7b0} docker",
            icon_lang_default: "\u{f15c} ?",
            spinner_status: SPINNER_STATUS_NERD,
            spinner_activity: SPINNER_ACTIVITY_UNICODE,
        }
    }

    /// Resolve a thinking level name to its icon+label glyph.
    ///
    /// The level name is the lowercase thinking level string
    /// (e.g., "off", "minimal", "low", "medium", "high", "xhigh").
    /// Returns the pre-combined icon+label from the active glyph set,
    /// or the "off" icon for unknown levels.
    pub fn thinking_level_icon(self, level: &str) -> &'static str {
        match level {
            "off" => self.icon_thinking_off,
            "minimal" => self.icon_thinking_minimal,
            "low" => self.icon_thinking_low,
            "medium" => self.icon_thinking_medium,
            "high" => self.icon_thinking_high,
            "xhigh" => self.icon_thinking_xhigh,
            _ => self.icon_thinking_off,
        }
    }

    /// Resolve a language identifier to its icon+label glyph for code blocks.
    ///
    /// Accepts short aliases (e.g., "rs", "py", "ts", "sh") and full names
    /// (e.g., "rust", "python", "typescript", "bash").
    pub fn lang_icon(self, lang: &str) -> &'static str {
        match lang {
            "rust" | "rs" => self.icon_lang_rust,
            "python" | "py" => self.icon_lang_python,
            "javascript" | "js" => self.icon_lang_javascript,
            "typescript" | "ts" | "tsx" | "jsx" => self.icon_lang_typescript,
            "go" => self.icon_lang_go,
            "java" => self.icon_lang_java,
            "c" | "cpp" | "c++" | "cxx" => self.icon_lang_cpp,
            "csharp" | "cs" | "c#" => self.icon_lang_csharp,
            "swift" => self.icon_lang_swift,
            "kotlin" | "kt" => self.icon_lang_kotlin,
            "bash" | "sh" | "shell" | "zsh" => self.icon_lang_shell,
            "html" => self.icon_lang_html,
            "css" => self.icon_lang_css,
            "json" => self.icon_lang_json,
            "sql" => self.icon_lang_sql,
            "docker" | "dockerfile" => self.icon_lang_docker,
            "yaml" | "yml" | "toml" | "xml" | "ini" | "conf" => self.icon_lang_json,
            "ruby" | "rb" | "php" | "lua" | "markdown" | "md" | "text" | "txt" => {
                self.icon_lang_default
            }
            _ => self.icon_lang_default,
        }
    }
}

impl Default for Symbols {
    fn default() -> Self {
        GlyphSet::Unicode.symbols()
    }
}

// ---------------------------------------------------------------------------
// Spinner frame tables
// ---------------------------------------------------------------------------

/// Unicode braille "status" spinner (8 frames).
const SPINNER_STATUS_UNICODE: &[&str] = &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
/// Unicode braille "activity" spinner (10 frames).
const SPINNER_ACTIVITY_UNICODE: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// ASCII spinner — classic 4-frame blade.
const SPINNER_STATUS_ASCII: &[&str] = &["|", "/", "-", "\\"];
const SPINNER_ACTIVITY_ASCII: &[&str] = &["|", "/", "-", "\\"];
/// Nerd Font "status" spinner — clock-face hour glyphs (12 frames).
const SPINNER_STATUS_NERD: &[&str] = &[
    "\u{f1146}",
    "\u{f114b}",
    "\u{f114c}",
    "\u{f114d}",
    "\u{f114e}",
    "\u{f114f}",
    "\u{f1150}",
    "\u{f1151}",
    "\u{f1152}",
    "\u{f1153}",
    "\u{f1154}",
    "\u{f1155}",
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_unicode() {
        assert_eq!(GlyphSet::default(), GlyphSet::Unicode);
        assert_eq!(Symbols::default(), Symbols::unicode());
    }

    #[test]
    fn parse_glyph_set() {
        assert_eq!("unicode".parse::<GlyphSet>().unwrap(), GlyphSet::Unicode);
        assert_eq!("ASCII".parse::<GlyphSet>().unwrap(), GlyphSet::Ascii);
        assert_eq!("nerdfont".parse::<GlyphSet>().unwrap(), GlyphSet::Nerd);
        assert_eq!("default".parse::<GlyphSet>().unwrap(), GlyphSet::Unicode);
        assert!("emoji".parse::<GlyphSet>().is_err());
    }

    #[test]
    fn serde_roundtrip() {
        // The on-disk form is `glyph_set = "nerd"` in settings.toml — a bare
        // snake_case string deserialized into the enum. Round-trip via the
        // field context toml actually uses (bare enum serialization isn't
        // supported by toml, which is why it lives in a struct on disk).
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrap {
            #[allow(dead_code)]
            glyph_set: GlyphSet,
        }
        let toml_str = "glyph_set = \"nerd\"\n";
        let parsed: Wrap = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.glyph_set, GlyphSet::Nerd);
        let back = toml::to_string(&Wrap {
            glyph_set: GlyphSet::Unicode,
        })
        .unwrap();
        assert!(back.contains("unicode"), "got: {back}");
        // Unknown value errors out cleanly.
        assert!(toml::from_str::<Wrap>("glyph_set = \"emoji\"\n").is_err());
    }

    #[test]
    fn all_presets_populate_every_field_distinctly() {
        // Every preset must be ASCII-only (ascii), UTF-8 (unicode), or PUA
        // (nerd). The important invariant: no field is empty and the three
        // presets are actually different from each other.
        for preset in GlyphSet::ALL {
            let s = preset.symbols();
            assert!(!s.status_success.is_empty());
            assert!(!s.cursor.is_empty());
            assert!(!s.rule.is_empty());
            assert!(!s.spinner_status.is_empty());
            assert!(!s.spinner_activity.is_empty());
            assert!(!s.arrow_up.is_empty());
            assert!(!s.arrow_down.is_empty());
            assert!(!s.icon_thinking.is_empty());
            assert!(!s.tool_generic.is_empty());
            assert!(!s.icon_thinking_off.is_empty());
            assert!(!s.icon_thinking_minimal.is_empty());
            assert!(!s.icon_thinking_low.is_empty());
            assert!(!s.icon_thinking_medium.is_empty());
            assert!(!s.icon_thinking_high.is_empty());
            assert!(!s.icon_thinking_xhigh.is_empty());
            assert!(!s.icon_lang_rust.is_empty());
            assert!(!s.icon_lang_python.is_empty());
            assert!(!s.icon_lang_shell.is_empty());
            assert!(!s.icon_lang_default.is_empty());
        }
        assert_ne!(Symbols::unicode(), Symbols::ascii());
        assert_ne!(Symbols::unicode(), Symbols::nerd());
        assert_ne!(Symbols::ascii(), Symbols::nerd());
    }

    #[test]
    fn ascii_preset_is_7bit_clean() {
        let s = Symbols::ascii();
        for field in [
            s.status_success,
            s.status_error,
            s.cursor,
            s.tree_branch,
            s.rule,
            s.sep_block,
            s.bullet,
            s.tool_bash,
            s.arrow_up,
            s.arrow_down,
            s.icon_thinking,
            s.tool_generic,
            s.icon_thinking_off,
            s.icon_thinking_minimal,
            s.icon_thinking_low,
            s.icon_thinking_medium,
            s.icon_thinking_high,
            s.icon_thinking_xhigh,
            s.icon_lang_rust,
            s.icon_lang_python,
            s.icon_lang_shell,
            s.icon_lang_default,
        ] {
            assert!(
                field.is_ascii(),
                "ASCII preset field '{field}' must be 7-bit clean"
            );
        }
    }

    #[test]
    fn unicode_cursor_is_two_cells_wide_prefix() {
        // The highlight cursor is a prefix drawn before a row; unicode uses
        // "❯ " (glyph + trailing space). ASCII uses "> ". Both end in a space
        // so the selected row's first real char isn't jammed against the glyph.
        assert!(Symbols::unicode().cursor.ends_with(' '));
        assert!(Symbols::ascii().cursor.ends_with(' '));
    }

    #[test]
    fn unknown_glyph_set_error_is_helpful() {
        let err = "wat".parse::<GlyphSet>().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unicode"), "msg was: {msg}");
        assert!(msg.contains("ascii"));
        assert!(msg.contains("nerd"));
    }

    #[test]
    fn samples_render_for_each_preset() {
        for preset in GlyphSet::ALL {
            assert!(!preset.sample().is_empty());
        }
    }
}
