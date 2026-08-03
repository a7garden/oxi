//! Theme system for oxicode-tui.
//!
//! Provides customizable color schemes, font styles, and spacing.
//! Includes built-in dark and light themes, and supports loading
//! themes from TOML or JSON files with hot-reloading.

use crate::cell::Color;
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
    /// Spacing configuration.
    pub spacing: Spacing,
    /// Active glyph set (Unicode / ASCII / Nerd). Controls every UI symbol.
    pub symbols: crate::symbols::Symbols,
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
    /// Code (inline `code`) foreground color.
    pub code_fg: Color,
    /// Code (inline `code`) background color.
    pub code_bg: Color,
    /// Tool call pending background (waiting state).
    pub tool_pending_bg: Color,
    /// Tool call executing background (running state).
    pub tool_executing_bg: Color,
    /// Tool call success background (completed successfully).
    pub tool_success_bg: Color,
    /// Tool call error background (completed with error).
    pub tool_error_bg: Color,

    // ── Phase-1 background slots (7 new) ────────────────────────────
    /// Assistant response text background (= background by default).
    pub response_bg: Color,
    /// Thinking block background (subtle accent tint).
    pub thinking_bg: Color,
    /// Footer / status bar background.
    pub surface_bg: Color,
    /// Overlay popup background (most prominent surface).
    pub panel_bg: Color,
    /// Diff added-line background (reuses tool_success_bg tint).
    pub diff_add_bg: Color,
    /// Diff removed-line background (reuses tool_error_bg tint).
    pub diff_remove_bg: Color,
    /// Diff hunk-header background (subtle muted tint).
    pub diff_hunk_bg: Color,
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
            foreground: Color::Rgb(205, 214, 244),     // #cdd6f4
            background: Color::Rgb(0, 0, 0),           // #000000 true black
            primary: Color::Rgb(122, 162, 247),        // #7aa2f7
            secondary: Color::Rgb(158, 206, 106),      // #9ece6a
            error: Color::Rgb(247, 118, 142),          // #f7768e
            warning: Color::Rgb(224, 175, 104),        // #e0af68
            success: Color::Rgb(158, 206, 106),        // #9ece6a
            muted: Color::Rgb(127, 132, 156),          // #7f849c overlay1 (AA 5.68:1 on black)
            accent: Color::Rgb(187, 154, 247),         // #bb9af7
            border: Color::Rgb(88, 91, 112),           // #585b70 surface2 (UI 3.15:1 on black)
            user_border: Color::Rgb(122, 162, 247),    // #7aa2f7 (matches primary)
            user_bg: Color::Rgb(18, 22, 38),           // #121626 subtle indigo tint
            cursor_fg: Color::Rgb(0, 0, 0),            // #000000
            cursor_bg: Color::Rgb(205, 214, 244),      // #cdd6f4
            selection_bg: Color::Rgb(40, 40, 60),      // #28283c
            code_fg: Color::Rgb(255, 200, 100),        // #ffc864 warm amber
            code_bg: Color::Rgb(35, 30, 20),           // #231e14 warm dark
            tool_pending_bg: Color::Rgb(18, 20, 28),   // #12141c subtle
            tool_executing_bg: Color::Rgb(28, 24, 14), // #1c1810 amber tint
            tool_success_bg: Color::Rgb(16, 26, 14),   // #101a0e green tint
            tool_error_bg: Color::Rgb(32, 16, 18),     // #201012 red tint
            // ── Phase-1 background slots ──
            response_bg: Color::Rgb(0, 0, 0),       // = background
            thinking_bg: Color::Rgb(11, 9, 15),     // #0b090f accent 6% tint
            surface_bg: Color::Rgb(9, 11, 19),      // #090b13 footer/status
            panel_bg: Color::Rgb(53, 56, 75),       // #35384b overlay popup
            diff_add_bg: Color::Rgb(16, 26, 14),    // = tool_success_bg
            diff_remove_bg: Color::Rgb(32, 16, 18), // = tool_error_bg
            diff_hunk_bg: Color::Rgb(15, 16, 19),   // #0f1013 muted 12% tint
        }
    }

    /// Default light color scheme.
    pub fn light() -> Self {
        Self {
            foreground: Color::Rgb(76, 79, 105),   // #4c4f69
            background: Color::Rgb(239, 241, 245), // #eff1f5
            primary: Color::Rgb(30, 102, 240),     // #1e66f0
            secondary: Color::Rgb(64, 160, 43),    // #40a02b
            error: Color::Rgb(210, 15, 57),        // #d20f39
            warning: Color::Rgb(223, 142, 29),     // #df8e1d
            success: Color::Rgb(64, 160, 43),      // #40a02b
            muted: Color::Rgb(92, 95, 119),        // #5c5f77 subtext0 (AA 5.53:1)
            accent: Color::Rgb(136, 57, 239),      // #8839ef
            border: Color::Rgb(156, 160, 176), // #9ca0b0 overlay0 (2.30:1; light-neutral AA limit)
            user_border: Color::Rgb(30, 102, 240), // #1e66f0 (matches primary)
            user_bg: Color::Rgb(225, 236, 255), // #e1ecff subtle blue tint
            cursor_fg: Color::Rgb(239, 241, 245),
            cursor_bg: Color::Rgb(76, 79, 105),
            selection_bg: Color::Rgb(204, 208, 218),
            code_fg: Color::Rgb(180, 60, 60),   // #b43c3c dark red
            code_bg: Color::Rgb(240, 240, 245), // #f0f0f5 off-white
            tool_pending_bg: Color::Rgb(235, 238, 245), // #ebeeff subtle blue tint
            tool_executing_bg: Color::Rgb(255, 248, 230), // #fff8e6 amber tint
            tool_success_bg: Color::Rgb(230, 248, 230), // #e6f8e6 green tint
            tool_error_bg: Color::Rgb(255, 230, 235), // #ffe6eb red tint
            // ── Phase-1 background slots ──
            response_bg: Color::Rgb(239, 241, 245), // = background #eff1f5
            thinking_bg: Color::Rgb(233, 230, 245), // #e9e6f5 accent 6% tint
            surface_bg: Color::Rgb(232, 238, 250),  // #e8eefa footer/status
            panel_bg: Color::Rgb(190, 198, 216),    // #bec6d8 overlay popup
            diff_add_bg: Color::Rgb(230, 248, 230), // = tool_success_bg #e6f8e6
            diff_remove_bg: Color::Rgb(255, 230, 235), // = tool_error_bg #ffe6eb
            diff_hunk_bg: Color::Rgb(221, 223, 230), // #dddfe6 muted 12% tint
        }
    }
    /// Nord color scheme (Arctic, dark).
    pub fn nord() -> Self {
        Self {
            foreground: Color::Rgb(216, 222, 233),     // #d8dee9 nord4
            background: Color::Rgb(46, 52, 64),        // #2e3440 nord0
            primary: Color::Rgb(136, 192, 208),        // #88c0d0 nord8
            secondary: Color::Rgb(163, 190, 140),      // #a3be8c nord14
            error: Color::Rgb(191, 97, 106),           // #bf616a nord11
            warning: Color::Rgb(235, 203, 139),        // #ebcb8b nord13
            success: Color::Rgb(163, 190, 140),        // #a3be8c nord14
            muted: Color::Rgb(97, 110, 136),           // #616e88 dimmed nord3 (AA 4.6:1)
            accent: Color::Rgb(180, 142, 173),         // #b48ead nord15
            border: Color::Rgb(76, 86, 106),           // #4c566a nord3
            user_border: Color::Rgb(136, 192, 208),    // #88c0d0 nord8
            user_bg: Color::Rgb(59, 66, 82),           // #3b4252 nord1
            cursor_fg: Color::Rgb(46, 52, 64),         // #2e3440
            cursor_bg: Color::Rgb(216, 222, 233),      // #d8dee9
            selection_bg: Color::Rgb(67, 76, 94),      // #434c5e nord2
            code_fg: Color::Rgb(235, 203, 139),        // #ebcb8b nord13
            code_bg: Color::Rgb(59, 66, 82),           // #3b4252 nord1
            tool_pending_bg: Color::Rgb(46, 52, 64),   // #2e3440 nord0
            tool_executing_bg: Color::Rgb(59, 56, 40), // amber tint
            tool_success_bg: Color::Rgb(40, 56, 44),   // green tint
            tool_error_bg: Color::Rgb(56, 42, 44),     // red tint
            // ── Phase-1 background slots ──
            response_bg: Color::Rgb(46, 52, 64), // = background nord0
            thinking_bg: Color::Rgb(54, 57, 71), // #363947 accent 6% tint
            surface_bg: Color::Rgb(52, 59, 73),  // #343b49 footer/status
            panel_bg: Color::Rgb(68, 76, 94),    // #444c5e overlay (≈nord2)
            diff_add_bg: Color::Rgb(40, 56, 44), // = tool_success_bg
            diff_remove_bg: Color::Rgb(56, 42, 44), // = tool_error_bg
            diff_hunk_bg: Color::Rgb(52, 59, 73), // #343b49 muted 12% tint
        }
    }
    /// Catppuccin Mocha color scheme (dark).
    pub fn catppuccin() -> Self {
        Self {
            foreground: Color::Rgb(205, 214, 244),     // #cdd6f4 text
            background: Color::Rgb(30, 30, 46),        // #1e1e2e base
            primary: Color::Rgb(137, 180, 250),        // #89b4fa blue
            secondary: Color::Rgb(166, 227, 161),      // #a6e3a1 green
            error: Color::Rgb(243, 139, 168),          // #f38ba8 red
            warning: Color::Rgb(249, 226, 175),        // #f9e2af yellow
            success: Color::Rgb(166, 227, 161),        // #a6e3a1 green
            muted: Color::Rgb(127, 132, 156),          // #7f849c overlay1 (AA 5.6:1)
            accent: Color::Rgb(203, 166, 247),         // #cba6f7 mauve
            border: Color::Rgb(88, 91, 112),           // #585b70 surface2
            user_border: Color::Rgb(137, 180, 250),    // #89b4fa
            user_bg: Color::Rgb(49, 50, 68),           // #313244 surface0
            cursor_fg: Color::Rgb(30, 30, 46),         // #1e1e2e
            cursor_bg: Color::Rgb(205, 214, 244),      // #cdd6f4
            selection_bg: Color::Rgb(69, 71, 90),      // #45475a surface1
            code_fg: Color::Rgb(249, 226, 175),        // #f9e2af yellow
            code_bg: Color::Rgb(49, 50, 68),           // #313244 surface0
            tool_pending_bg: Color::Rgb(30, 30, 46),   // #1e1e2e
            tool_executing_bg: Color::Rgb(44, 42, 30), // amber tint
            tool_success_bg: Color::Rgb(32, 46, 36),   // green tint
            tool_error_bg: Color::Rgb(48, 34, 40),     // red tint
            // ── Phase-1 background slots ──
            response_bg: Color::Rgb(30, 30, 46), // = base #1e1e2e
            thinking_bg: Color::Rgb(40, 38, 58), // #28263a accent 6% tint
            surface_bg: Color::Rgb(40, 40, 57),  // #282839 footer/status
            panel_bg: Color::Rgb(68, 70, 90),    // #44465a overlay (≈surface1)
            diff_add_bg: Color::Rgb(32, 46, 36), // = tool_success_bg
            diff_remove_bg: Color::Rgb(48, 34, 40), // = tool_error_bg
            diff_hunk_bg: Color::Rgb(42, 42, 59), // #2a2a3b muted 12% tint
        }
    }
    /// GitHub Dark color scheme.
    pub fn github_dark() -> Self {
        Self {
            foreground: Color::Rgb(201, 209, 217),     // #c9d1d9 fg.default
            background: Color::Rgb(13, 17, 23),        // #0d1117 canvas.default
            primary: Color::Rgb(47, 129, 247),         // #2f81f7 accent
            secondary: Color::Rgb(63, 185, 80),        // #3fb950 success
            error: Color::Rgb(248, 81, 73),            // #f85149 danger
            warning: Color::Rgb(210, 153, 34),         // #d29922 attention
            success: Color::Rgb(63, 185, 80),          // #3fb950 success
            muted: Color::Rgb(139, 148, 158),          // #8b949e fg.muted (AA 5.0:1)
            accent: Color::Rgb(163, 113, 247),         // #a371f7 done
            border: Color::Rgb(48, 54, 61),            // #30363d border.default
            user_border: Color::Rgb(47, 129, 247),     // #2f81f7
            user_bg: Color::Rgb(22, 27, 34),           // #161b22 subtle
            cursor_fg: Color::Rgb(13, 17, 23),         // #0d1117
            cursor_bg: Color::Rgb(201, 209, 217),      // #c9d1d9
            selection_bg: Color::Rgb(38, 79, 120),     // #264f78 selection
            code_fg: Color::Rgb(210, 153, 34),         // #d29922 attention
            code_bg: Color::Rgb(22, 27, 34),           // #161b22 subtle
            tool_pending_bg: Color::Rgb(13, 17, 23),   // #0d1117
            tool_executing_bg: Color::Rgb(34, 30, 18), // amber tint
            tool_success_bg: Color::Rgb(18, 30, 20),   // green tint
            tool_error_bg: Color::Rgb(34, 18, 20),     // red tint
            // ── Phase-1 background slots ──
            response_bg: Color::Rgb(13, 17, 23), // = canvas.default #0d1117
            thinking_bg: Color::Rgb(22, 23, 36), // #161724 accent 6% tint
            surface_bg: Color::Rgb(18, 22, 28),  // #12161c footer/status
            panel_bg: Color::Rgb(35, 40, 48),    // #232830 overlay
            diff_add_bg: Color::Rgb(18, 30, 20), // = tool_success_bg
            diff_remove_bg: Color::Rgb(34, 18, 20), // = tool_error_bg
            diff_hunk_bg: Color::Rgb(28, 33, 39), // #1c2127 muted 12% tint
        }
    }
    /// Monokai color scheme (dark).
    pub fn monokai() -> Self {
        Self {
            foreground: Color::Rgb(248, 248, 242),     // #f8f8f2
            background: Color::Rgb(39, 40, 34),        // #272822
            primary: Color::Rgb(102, 217, 239),        // #66d9ef cyan
            secondary: Color::Rgb(166, 226, 46),       // #a6e22e green
            error: Color::Rgb(249, 38, 114),           // #f92672 pink
            warning: Color::Rgb(253, 151, 31),         // #fd971f orange
            success: Color::Rgb(166, 226, 46),         // #a6e22e green
            muted: Color::Rgb(117, 113, 94),           // #75715e comment (AA 4.1:1)
            accent: Color::Rgb(174, 129, 255),         // #ae81ff purple
            border: Color::Rgb(73, 72, 62),            // #49483e
            user_border: Color::Rgb(102, 217, 239),    // #66d9ef
            user_bg: Color::Rgb(62, 61, 50),           // #3e3d32
            cursor_fg: Color::Rgb(39, 40, 34),         // #272822
            cursor_bg: Color::Rgb(248, 248, 240),      // #f8f8f0
            selection_bg: Color::Rgb(73, 72, 62),      // #49483e
            code_fg: Color::Rgb(230, 219, 116),        // #e6db74 yellow
            code_bg: Color::Rgb(62, 61, 50),           // #3e3d32
            tool_pending_bg: Color::Rgb(39, 40, 34),   // #272822
            tool_executing_bg: Color::Rgb(48, 40, 24), // amber tint
            tool_success_bg: Color::Rgb(34, 44, 26),   // green tint
            tool_error_bg: Color::Rgb(50, 30, 38),     // red tint
            // ── Phase-1 background slots ──
            response_bg: Color::Rgb(39, 40, 34), // = background #272822
            thinking_bg: Color::Rgb(47, 45, 47), // #2f2d2f accent 6% tint
            surface_bg: Color::Rgb(50, 50, 42),  // #32322a footer/status
            panel_bg: Color::Rgb(68, 66, 56),    // #444238 overlay
            diff_add_bg: Color::Rgb(34, 44, 26), // = tool_success_bg
            diff_remove_bg: Color::Rgb(50, 30, 38), // = tool_error_bg
            diff_hunk_bg: Color::Rgb(48, 49, 41), // #303129 muted 12% tint
        }
    }

    /// Dracula color scheme (dark).
    pub fn dracula() -> Self {
        Self {
            foreground: Color::Rgb(248, 248, 242),
            background: Color::Rgb(40, 42, 54),
            primary: Color::Rgb(189, 147, 249),
            secondary: Color::Rgb(80, 250, 123),
            error: Color::Rgb(255, 85, 85),
            warning: Color::Rgb(241, 250, 140),
            success: Color::Rgb(80, 250, 123),
            muted: Color::Rgb(98, 114, 164),
            accent: Color::Rgb(255, 121, 198),
            border: Color::Rgb(68, 71, 90),
            user_border: Color::Rgb(189, 147, 249),
            user_bg: Color::Rgb(68, 71, 90),
            cursor_fg: Color::Rgb(40, 42, 54),
            cursor_bg: Color::Rgb(248, 248, 242),
            selection_bg: Color::Rgb(68, 71, 90),
            code_fg: Color::Rgb(241, 250, 140),
            code_bg: Color::Rgb(33, 34, 44),
            tool_pending_bg: Color::Rgb(40, 42, 54),
            tool_executing_bg: Color::Rgb(70, 73, 67),
            tool_success_bg: Color::Rgb(46, 73, 64),
            tool_error_bg: Color::Rgb(72, 48, 59),
            response_bg: Color::Rgb(40, 42, 54),
            thinking_bg: Color::Rgb(53, 47, 63),
            surface_bg: Color::Rgb(54, 56, 72),
            panel_bg: Color::Rgb(68, 71, 90),
            diff_add_bg: Color::Rgb(46, 73, 64),
            diff_remove_bg: Color::Rgb(72, 48, 59),
            diff_hunk_bg: Color::Rgb(47, 51, 67),
        }
    }

    /// Gruvbox color scheme (dark).
    pub fn gruvbox() -> Self {
        Self {
            foreground: Color::Rgb(235, 219, 178),
            background: Color::Rgb(40, 40, 40),
            primary: Color::Rgb(131, 165, 152),
            secondary: Color::Rgb(184, 187, 38),
            error: Color::Rgb(251, 73, 52),
            warning: Color::Rgb(250, 189, 47),
            success: Color::Rgb(184, 187, 38),
            muted: Color::Rgb(146, 131, 116),
            accent: Color::Rgb(211, 134, 155),
            border: Color::Rgb(60, 56, 54),
            user_border: Color::Rgb(131, 165, 152),
            user_bg: Color::Rgb(60, 56, 54),
            cursor_fg: Color::Rgb(40, 40, 40),
            cursor_bg: Color::Rgb(235, 219, 178),
            selection_bg: Color::Rgb(80, 73, 69),
            code_fg: Color::Rgb(250, 189, 47),
            code_bg: Color::Rgb(60, 56, 54),
            tool_pending_bg: Color::Rgb(40, 40, 40),
            tool_executing_bg: Color::Rgb(72, 62, 41),
            tool_success_bg: Color::Rgb(62, 62, 40),
            tool_error_bg: Color::Rgb(72, 45, 42),
            response_bg: Color::Rgb(40, 40, 40),
            thinking_bg: Color::Rgb(50, 46, 47),
            surface_bg: Color::Rgb(50, 48, 47),
            panel_bg: Color::Rgb(60, 56, 54),
            diff_add_bg: Color::Rgb(62, 62, 40),
            diff_remove_bg: Color::Rgb(72, 45, 42),
            diff_hunk_bg: Color::Rgb(53, 51, 49),
        }
    }

    /// Solarized Dark color scheme.
    pub fn solarized_dark() -> Self {
        Self {
            foreground: Color::Rgb(131, 148, 150),
            background: Color::Rgb(0, 43, 54),
            primary: Color::Rgb(38, 139, 210),
            secondary: Color::Rgb(133, 153, 0),
            error: Color::Rgb(220, 50, 47),
            warning: Color::Rgb(181, 137, 0),
            success: Color::Rgb(133, 153, 0),
            muted: Color::Rgb(88, 110, 117),
            accent: Color::Rgb(108, 113, 196),
            border: Color::Rgb(7, 54, 66),
            user_border: Color::Rgb(38, 139, 210),
            user_bg: Color::Rgb(7, 54, 66),
            cursor_fg: Color::Rgb(0, 43, 54),
            cursor_bg: Color::Rgb(131, 148, 150),
            selection_bg: Color::Rgb(7, 54, 66),
            code_fg: Color::Rgb(181, 137, 0),
            code_bg: Color::Rgb(7, 54, 66),
            tool_pending_bg: Color::Rgb(0, 43, 54),
            tool_executing_bg: Color::Rgb(27, 57, 46),
            tool_success_bg: Color::Rgb(20, 60, 46),
            tool_error_bg: Color::Rgb(33, 44, 53),
            response_bg: Color::Rgb(0, 43, 54),
            thinking_bg: Color::Rgb(6, 47, 63),
            surface_bg: Color::Rgb(4, 48, 60),
            panel_bg: Color::Rgb(7, 54, 66),
            diff_add_bg: Color::Rgb(20, 60, 46),
            diff_remove_bg: Color::Rgb(33, 44, 53),
            diff_hunk_bg: Color::Rgb(11, 51, 62),
        }
    }

    /// Solarized Light color scheme.
    pub fn solarized_light() -> Self {
        Self {
            foreground: Color::Rgb(101, 123, 131),
            background: Color::Rgb(253, 246, 227),
            primary: Color::Rgb(38, 139, 210),
            secondary: Color::Rgb(133, 153, 0),
            error: Color::Rgb(220, 50, 47),
            warning: Color::Rgb(181, 137, 0),
            success: Color::Rgb(133, 153, 0),
            muted: Color::Rgb(147, 161, 161),
            accent: Color::Rgb(108, 113, 196),
            border: Color::Rgb(238, 232, 213),
            user_border: Color::Rgb(38, 139, 210),
            user_bg: Color::Rgb(238, 232, 213),
            cursor_fg: Color::Rgb(253, 246, 227),
            cursor_bg: Color::Rgb(101, 123, 131),
            selection_bg: Color::Rgb(238, 232, 213),
            code_fg: Color::Rgb(181, 137, 0),
            code_bg: Color::Rgb(238, 232, 213),
            tool_pending_bg: Color::Rgb(253, 246, 227),
            tool_executing_bg: Color::Rgb(242, 230, 193),
            tool_success_bg: Color::Rgb(235, 232, 193),
            tool_error_bg: Color::Rgb(248, 217, 200),
            response_bg: Color::Rgb(253, 246, 227),
            thinking_bg: Color::Rgb(244, 238, 225),
            surface_bg: Color::Rgb(246, 239, 220),
            panel_bg: Color::Rgb(238, 232, 213),
            diff_add_bg: Color::Rgb(235, 232, 193),
            diff_remove_bg: Color::Rgb(248, 217, 200),
            diff_hunk_bg: Color::Rgb(240, 236, 219),
        }
    }

    /// Rose Pine color scheme (dark).
    pub fn rose_pine() -> Self {
        Self {
            foreground: Color::Rgb(224, 222, 244),
            background: Color::Rgb(25, 23, 36),
            primary: Color::Rgb(49, 116, 143),
            secondary: Color::Rgb(156, 207, 216),
            error: Color::Rgb(235, 111, 146),
            warning: Color::Rgb(246, 193, 119),
            success: Color::Rgb(156, 207, 216),
            muted: Color::Rgb(110, 106, 134),
            accent: Color::Rgb(196, 167, 231),
            border: Color::Rgb(38, 35, 58),
            user_border: Color::Rgb(196, 167, 231),
            user_bg: Color::Rgb(31, 29, 46),
            cursor_fg: Color::Rgb(25, 23, 36),
            cursor_bg: Color::Rgb(224, 222, 244),
            selection_bg: Color::Rgb(64, 61, 82),
            code_fg: Color::Rgb(246, 193, 119),
            code_bg: Color::Rgb(31, 29, 46),
            tool_pending_bg: Color::Rgb(25, 23, 36),
            tool_executing_bg: Color::Rgb(58, 48, 48),
            tool_success_bg: Color::Rgb(45, 51, 63),
            tool_error_bg: Color::Rgb(56, 36, 52),
            response_bg: Color::Rgb(25, 23, 36),
            thinking_bg: Color::Rgb(35, 32, 48),
            surface_bg: Color::Rgb(28, 26, 41),
            panel_bg: Color::Rgb(34, 32, 52),
            diff_add_bg: Color::Rgb(45, 51, 63),
            diff_remove_bg: Color::Rgb(56, 36, 52),
            diff_hunk_bg: Color::Rgb(35, 33, 48),
        }
    }

    /// Kanagawa color scheme (dark).
    pub fn kanagawa() -> Self {
        Self {
            foreground: Color::Rgb(220, 215, 186),
            background: Color::Rgb(31, 31, 40),
            primary: Color::Rgb(126, 156, 216),
            secondary: Color::Rgb(152, 187, 108),
            error: Color::Rgb(195, 64, 67),
            warning: Color::Rgb(230, 195, 132),
            success: Color::Rgb(152, 187, 108),
            muted: Color::Rgb(114, 113, 105),
            accent: Color::Rgb(149, 127, 184),
            border: Color::Rgb(42, 42, 55),
            user_border: Color::Rgb(126, 156, 216),
            user_bg: Color::Rgb(42, 42, 55),
            cursor_fg: Color::Rgb(31, 31, 40),
            cursor_bg: Color::Rgb(220, 215, 186),
            selection_bg: Color::Rgb(54, 54, 70),
            code_fg: Color::Rgb(230, 195, 132),
            code_bg: Color::Rgb(22, 22, 29),
            tool_pending_bg: Color::Rgb(31, 31, 40),
            tool_executing_bg: Color::Rgb(61, 56, 54),
            tool_success_bg: Color::Rgb(49, 54, 50),
            tool_error_bg: Color::Rgb(56, 36, 44),
            response_bg: Color::Rgb(31, 31, 40),
            thinking_bg: Color::Rgb(38, 37, 49),
            surface_bg: Color::Rgb(36, 36, 48),
            panel_bg: Color::Rgb(42, 42, 55),
            diff_add_bg: Color::Rgb(49, 54, 50),
            diff_remove_bg: Color::Rgb(56, 36, 44),
            diff_hunk_bg: Color::Rgb(41, 41, 48),
        }
    }

    /// Convert to ratatui Style with foreground and background.
    pub fn to_style(&self) -> Style {
        Style::default().fg(self.foreground).bg(self.background)
    }

    /// Convert to ratatui Style with all semantic colors.
    pub fn to_styles(&self) -> ThemeStyles {
        ThemeStyles {
            normal: Style::default().fg(self.foreground),
            primary: Style::default().fg(self.primary),
            secondary: Style::default().fg(self.secondary),
            error: Style::default().fg(self.error),
            warning: Style::default().fg(self.warning),
            success: Style::default().fg(self.success),
            muted: Style::default().fg(self.muted),
            accent: Style::default().fg(self.accent),
            border: Style::default().fg(self.border),
            cursor_fg: Style::default().fg(self.cursor_fg),
            cursor_bg: Style::default().fg(self.cursor_bg),
            selection_bg: Style::default().bg(self.selection_bg),
            user_border: Style::default().fg(self.user_border),
            user_bg: Style::default().bg(self.user_bg),
            tool_pending_bg: Style::default().bg(self.tool_pending_bg),
            tool_executing_bg: Style::default().bg(self.tool_executing_bg),
            tool_success_bg: Style::default().bg(self.tool_success_bg),
            tool_error_bg: Style::default().bg(self.tool_error_bg),
            code_fg: Style::default().fg(self.code_fg),
            code_bg: Style::default().bg(self.code_bg),
            // ── Phase-1 background slots ──
            response_bg: Style::default().bg(self.response_bg),
            thinking_bg: Style::default().bg(self.thinking_bg),
            surface_bg: Style::default().bg(self.surface_bg),
            panel_bg: Style::default().bg(self.panel_bg),
            diff_add_bg: Style::default().bg(self.diff_add_bg),
            diff_remove_bg: Style::default().bg(self.diff_remove_bg),
            diff_hunk_bg: Style::default().bg(self.diff_hunk_bg),
            // ColorScheme has no glyph-set knowledge; default to Unicode.
            // `Theme::to_styles` overrides this with the theme's real set.
            symbols: crate::symbols::Symbols::default(),
        }
    }
}

/// Pre-computed ratatui styles for all semantic colors in a ColorScheme.
///
/// Every style field is a `Style` with only the relevant property set (fg or bg),
/// so they compose correctly via `Style::patch()`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ThemeStyles {
    // ── Text ──────────────────────────────────────────────────────
    /// Normal / default text style.
    pub normal: Style,
    /// Primary accent color style (UI elements, labels, user "You").
    pub primary: Style,
    /// Secondary color style.
    pub secondary: Style,
    /// Error / red style.
    pub error: Style,
    /// Warning / yellow style.
    pub warning: Style,
    /// Success / green style.
    pub success: Style,
    /// Muted / dimmed style (tool headers, borders).
    pub muted: Style,
    /// Accent / highlight style.
    pub accent: Style,

    // ── Structural ────────────────────────────────────────────────
    /// Border / separator style.
    pub border: Style,
    /// Cursor foreground style.
    pub cursor_fg: Style,
    /// Cursor background style.
    pub cursor_bg: Style,
    /// Selection background style.
    pub selection_bg: Style,

    // ── User messages ─────────────────────────────────────────────
    /// User message left-border accent (bright primary).
    pub user_border: Style,
    /// User message background (subtle tint).
    pub user_bg: Style,

    // ── Tool call states ──────────────────────────────────────────
    /// Tool call pending background (waiting state).
    pub tool_pending_bg: Style,
    /// Tool call executing background (running state).
    pub tool_executing_bg: Style,
    /// Tool call success background (completed successfully).
    pub tool_success_bg: Style,
    /// Tool call error background (completed with error).
    pub tool_error_bg: Style,

    // ── Code ──────────────────────────────────────────────────────
    /// Inline code foreground style.
    pub code_fg: Style,
    /// Inline code / code block background style.
    pub code_bg: Style,

    // ── Phase-1 background styles (7 new) ───────────────────────────
    /// Assistant response text background style.
    pub response_bg: Style,
    /// Thinking block background style.
    pub thinking_bg: Style,
    /// Footer / status bar background style.
    pub surface_bg: Style,
    /// Overlay popup background style.
    pub panel_bg: Style,
    /// Diff added-line background style.
    pub diff_add_bg: Style,
    /// Diff removed-line background style.
    pub diff_remove_bg: Style,
    /// Diff hunk-header background style.
    pub diff_hunk_bg: Style,

    // ── Glyphs ───────────────────────────────────────────────────
    /// Active symbol table (Unicode / ASCII / Nerd). Carried here so every
    /// render fn that already takes `&ThemeStyles` gets the glyph set for
    /// free, with no signature changes.
    pub symbols: crate::symbols::Symbols,
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
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }

    /// Built-in light theme.
    pub fn light() -> Self {
        Self {
            name: "light".into(),
            colors: ColorScheme::light(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in Nord theme.
    pub fn nord() -> Self {
        Self {
            name: "nord".into(),
            colors: ColorScheme::nord(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in Catppuccin Mocha theme.
    pub fn catppuccin() -> Self {
        Self {
            name: "catppuccin".into(),
            colors: ColorScheme::catppuccin(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in GitHub Dark theme.
    pub fn github_dark() -> Self {
        Self {
            name: "github_dark".into(),
            colors: ColorScheme::github_dark(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in Monokai theme.
    pub fn monokai() -> Self {
        Self {
            name: "monokai".into(),
            colors: ColorScheme::monokai(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }

    /// Built-in Dracula theme.
    pub fn dracula() -> Self {
        Self {
            name: "dracula".into(),
            colors: ColorScheme::dracula(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in Gruvbox theme.
    pub fn gruvbox() -> Self {
        Self {
            name: "gruvbox".into(),
            colors: ColorScheme::gruvbox(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in Solarized Dark theme.
    pub fn solarized_dark() -> Self {
        Self {
            name: "solarized_dark".into(),
            colors: ColorScheme::solarized_dark(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in Solarized Light theme.
    pub fn solarized_light() -> Self {
        Self {
            name: "solarized_light".into(),
            colors: ColorScheme::solarized_light(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in Rose Pine theme.
    pub fn rose_pine() -> Self {
        Self {
            name: "rose_pine".into(),
            colors: ColorScheme::rose_pine(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Built-in Kanagawa theme.
    pub fn kanagawa() -> Self {
        Self {
            name: "kanagawa".into(),
            colors: ColorScheme::kanagawa(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        }
    }
    /// Resolve a theme by its settings name.
    ///
    /// Maps the names used in `settings.theme` / the setup wizard /
    /// the `/settings` overlay to a built-in [`Theme`]. Unknown names
    /// and the sentinel `"default"` fall back to the dark theme.
    pub fn by_name(name: &str) -> Self {
        match name {
            "oxicode_light" | "light" => Self::light(),
            "nord" => Self::nord(),
            "catppuccin" => Self::catppuccin(),
            "github_dark" => Self::github_dark(),
            "monokai" => Self::monokai(),
            "dracula" => Self::dracula(),
            "gruvbox" => Self::gruvbox(),
            "solarized_dark" => Self::solarized_dark(),
            "solarized_light" => Self::solarized_light(),
            "rose_pine" => Self::rose_pine(),
            "kanagawa" => Self::kanagawa(),
            // "oxicode_dark", "dark", "default", "", unknown → dark
            _ => Self::dark(),
        }
    }
    /// Set the active glyph set, replacing the symbol table.
    pub fn with_glyph_set(mut self, set: crate::symbols::GlyphSet) -> Self {
        self.symbols = set.symbols();
        self
    }

    /// Replace the glyph set on an already-built theme (e.g. when the user
    /// flips the setting at runtime).
    pub fn set_glyph_set(&mut self, set: crate::symbols::GlyphSet) {
        self.symbols = set.symbols();
    }

    /// Convert theme foreground/background to ratatui Style.
    pub fn to_style(&self) -> Style {
        self.colors.to_style()
    }

    /// Get all semantic styles as ThemeStyles.
    pub fn to_styles(&self) -> ThemeStyles {
        let mut styles = self.colors.to_styles();
        styles.symbols = self.symbols;
        styles
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
/// All built-in theme names, in the order the `/settings` overlay cycles them.
///
/// These match the names stored in `settings.theme` and resolved by
/// [`Theme::by_name`].
pub const THEME_NAMES: &[&str] = &[
    "oxicode_dark",
    "oxicode_light",
    "nord",
    "catppuccin",
    "github_dark",
    "monokai",
    "dracula",
    "gruvbox",
    "solarized_dark",
    "solarized_light",
    "rose_pine",
    "kanagawa",
];

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
    /// User message left-border accent.
    pub user_border: Option<String>,
    /// User message background (subtle tint).
    pub user_bg: Option<String>,
    /// Cursor foreground color.
    pub cursor_fg: Option<String>,
    /// Cursor background color.
    pub cursor_bg: Option<String>,
    /// Selection background color.
    pub selection_bg: Option<String>,
    /// Code (inline `code`) foreground color.
    pub code_fg: Option<String>,
    /// Code (inline `code`) background color.
    pub code_bg: Option<String>,
    /// Tool call pending background (waiting state).
    pub tool_pending_bg: Option<String>,
    /// Tool call executing background (running state).
    pub tool_executing_bg: Option<String>,
    /// Tool call success background (completed successfully).
    pub tool_success_bg: Option<String>,
    /// Tool call error background (completed with error).
    pub tool_error_bg: Option<String>,

    // ── Phase-1 background slots (7 new) ────────────────────────────
    /// Assistant response text background.
    pub response_bg: Option<String>,
    /// Thinking block background.
    pub thinking_bg: Option<String>,
    /// Footer / status bar background.
    pub surface_bg: Option<String>,
    /// Overlay popup background.
    pub panel_bg: Option<String>,
    /// Diff added-line background.
    pub diff_add_bg: Option<String>,
    /// Diff removed-line background.
    pub diff_remove_bg: Option<String>,
    /// Diff hunk-header background.
    pub diff_hunk_bg: Option<String>,
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

        // Helper: parse a color string, logging a warning for invalid user-specified values.
        fn resolve(value: Option<String>, fallback: Color, field_name: &str) -> Color {
            match value.as_deref().and_then(parse_color) {
                Some(c) => c,
                None => {
                    if let Some(ref v) = value {
                        tracing::warn!(
                            "Invalid theme color for '{}': '{}' - using default",
                            field_name,
                            v
                        );
                    }
                    fallback
                }
            }
        }

        let colors = ColorScheme {
            foreground: resolve(self.colors.foreground, defaults.foreground, "foreground"),
            background: resolve(self.colors.background, defaults.background, "background"),
            primary: resolve(self.colors.primary, defaults.primary, "primary"),
            secondary: resolve(self.colors.secondary, defaults.secondary, "secondary"),
            error: resolve(self.colors.error, defaults.error, "error"),
            warning: resolve(self.colors.warning, defaults.warning, "warning"),
            success: resolve(self.colors.success, defaults.success, "success"),
            muted: resolve(self.colors.muted, defaults.muted, "muted"),
            accent: resolve(self.colors.accent, defaults.accent, "accent"),
            border: resolve(self.colors.border, defaults.border, "border"),
            user_border: resolve(self.colors.user_border, defaults.user_border, "user_border"),
            user_bg: resolve(self.colors.user_bg, defaults.user_bg, "user_bg"),
            cursor_fg: resolve(self.colors.cursor_fg, defaults.cursor_fg, "cursor_fg"),
            cursor_bg: resolve(self.colors.cursor_bg, defaults.cursor_bg, "cursor_bg"),
            selection_bg: resolve(
                self.colors.selection_bg,
                defaults.selection_bg,
                "selection_bg",
            ),
            code_fg: resolve(self.colors.code_fg, defaults.code_fg, "code_fg"),
            code_bg: resolve(self.colors.code_bg, defaults.code_bg, "code_bg"),
            tool_pending_bg: resolve(
                self.colors.tool_pending_bg,
                defaults.tool_pending_bg,
                "tool_pending_bg",
            ),
            tool_executing_bg: resolve(
                self.colors.tool_executing_bg,
                defaults.tool_executing_bg,
                "tool_executing_bg",
            ),
            tool_success_bg: resolve(
                self.colors.tool_success_bg,
                defaults.tool_success_bg,
                "tool_success_bg",
            ),
            tool_error_bg: resolve(
                self.colors.tool_error_bg,
                defaults.tool_error_bg,
                "tool_error_bg",
            ),
            response_bg: resolve(self.colors.response_bg, defaults.response_bg, "response_bg"),
            thinking_bg: resolve(self.colors.thinking_bg, defaults.thinking_bg, "thinking_bg"),
            surface_bg: resolve(self.colors.surface_bg, defaults.surface_bg, "surface_bg"),
            panel_bg: resolve(self.colors.panel_bg, defaults.panel_bg, "panel_bg"),
            diff_add_bg: resolve(self.colors.diff_add_bg, defaults.diff_add_bg, "diff_add_bg"),
            diff_remove_bg: resolve(
                self.colors.diff_remove_bg,
                defaults.diff_remove_bg,
                "diff_remove_bg",
            ),
            diff_hunk_bg: resolve(
                self.colors.diff_hunk_bg,
                defaults.diff_hunk_bg,
                "diff_hunk_bg",
            ),
        };
        Theme {
            name: if self.name.is_empty() {
                "custom".into()
            } else {
                self.name
            },
            colors,
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
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
    if let Some(idx_str) = s.strip_prefix('i')
        && let Ok(n) = idx_str.parse::<u8>()
    {
        return Some(Color::Indexed(n));
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
        "default" => Some(Color::Reset),
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

    /// Set the active theme by name. Resolves through [`Theme::by_name`],
    /// which knows all six built-in themes (`"oxicode_dark"`, `"oxicode_light"`,
    /// `"nord"`, `"catppuccin"`, `"github_dark"`, `"monokai"`) and the
    /// short aliases (`"dark"`, `"light"`).
    ///
    /// Always returns `true`: unknown / empty / `"default"` all fall
    /// back to the dark theme via `Theme::by_name`. Use a
    /// [`ThemeRegistry`] when you also need custom-theme resolution.
    pub fn set_theme_by_name(&self, name: &str) -> bool {
        self.set_theme(Theme::by_name(name));
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

    /// Lightweight external-file mtime check.
    ///
    /// Unlike [`ThemeManager::check_reload`], this does NOT load or
    /// re-parse anything — it only compares the path's mtime to the
    /// last seen value and returns `true` if it changed. The caller is
    /// responsible for reloading whatever the path pointed to (e.g.
    /// re-read `Settings`, re-resolve via `ThemeRegistry`).
    ///
    /// Use this when the watched file is something other than a
    /// theme file — typically the global `settings.toml`, which the
    /// TUI render loop polls to pick up hand-edited `theme = "..."`
    /// changes without a restart.
    ///
    /// Polling is throttled by the same `poll_interval` as
    /// [`ThemeManager::check_reload`]. Pass `Some(mtime)` on the
    /// first call to seed the baseline.
    pub fn check_external(
        &mut self,
        path: &std::path::Path,
        initial: Option<std::time::SystemTime>,
    ) -> bool {
        if self.last_poll.elapsed() < self.poll_interval {
            return false;
        }
        self.last_poll = Instant::now();

        let current = match std::fs::metadata(path).ok().and_then(|m| m.modified().ok()) {
            Some(t) => t,
            None => return false,
        };

        match initial {
            Some(prev) => current > prev,
            // No baseline → caller hasn't seeded yet. This is the
            // "first poll" case: don't fire a spurious reload.
            None => false,
        }
    }
}
// ---------------------------------------------------------------------------
// Theme registry (built-in + custom)
// ---------------------------------------------------------------------------

/// Resolves themes by name from a combined built-in + custom pool.
///
/// Use this at runtime (CLI bootstrap, TUI render loop) instead of the
/// static [`Theme::by_name`], which only knows the six built-ins. The
/// registry layers custom themes loaded from `~/.oxicode/themes/*.toml`,
/// `~/.oxicode/themes/*.json`, and `<project>/.oxicode/themes/*.json` over the
/// built-ins.
///
/// Resolution order for [`ThemeRegistry::resolve`]:
/// 1. Custom themes (by lowercased id == lowercased name).
/// 2. Built-in themes (see [`THEME_NAMES`]).
/// 3. Fallback: dark theme (when name is empty, `"default"`, or unknown).
#[derive(Clone, Debug)]
pub struct ThemeRegistry {
    builtins: std::collections::HashMap<String, Theme>,
    custom: std::collections::HashMap<String, Theme>,
}

impl ThemeRegistry {
    /// Build a registry seeded with the six built-in themes. No custom
    /// themes; call [`ThemeRegistry::add_custom`] /
    /// [`ThemeRegistry::add_custom_file`] to populate.
    ///
    /// The `builtins` map contains both the canonical [`THEME_NAMES`]
    /// keys (`"oxicode_dark"`, `"oxicode_light"`, `"nord"`, `"catppuccin"`,
    /// `"github_dark"`, `"monokai"`) and the short aliases
    /// (`"dark"`, `"light"`) so that [`ThemeRegistry::resolve`] can
    /// accept either form regardless of how the user typed it in
    /// `settings.toml`.
    pub fn with_builtins() -> Self {
        let mut builtins = std::collections::HashMap::with_capacity(THEME_NAMES.len() * 2);
        for &name in THEME_NAMES {
            builtins.insert(name.to_string(), Theme::by_name(name));
        }
        // Short aliases that `Theme::by_name` historically accepts.
        builtins.insert("dark".to_string(), Theme::dark());
        builtins.insert("light".to_string(), Theme::light());
        Self {
            builtins,
            custom: std::collections::HashMap::new(),
        }
    }

    /// Insert a parsed custom theme keyed by its lowercased `name`.
    /// Overwrites any existing custom theme with the same name.
    pub fn add_custom(&mut self, theme: Theme) {
        let key = theme.name.to_lowercase();
        self.custom.insert(key, theme);
    }

    /// Parse a TOML or JSON theme file and add it to the registry.
    /// Returns the parsed theme on success. The format is detected by
    /// file extension (`.toml` or `.json`); other extensions are
    /// rejected. Invalid files return an error without mutating state.
    ///
    /// `label` is used in error messages — pass something like the
    /// path or `"(inline)"`.
    pub fn add_custom_file(&mut self, path: &Path) -> anyhow::Result<Theme> {
        let file = ThemeFile::load(path)?;
        let theme = file.into_theme();
        self.add_custom(theme.clone());
        Ok(theme)
    }

    /// Resolve a theme by name.
    ///
    /// Resolution order:
    /// 1. Custom themes (lowercased name).
    /// 2. Built-in themes (the `builtins` map contains both the
    ///    canonical [`THEME_NAMES`] keys like `"oxicode_dark"` and the
    ///    short aliases like `"dark"`).
    /// 3. Final fallback: dark theme (for unknown / empty / `"default"`).
    pub fn resolve(&self, name: &str) -> Theme {
        let key = name.to_lowercase();
        if let Some(theme) = self.custom.get(&key) {
            return theme.clone();
        }
        if let Some(theme) = self.builtins.get(&key) {
            return theme.clone();
        }
        // Hard fallback — unknown / "default" / "" all land on dark.
        self.builtins
            .get("oxicode_dark")
            .cloned()
            .unwrap_or_else(Theme::dark)
    }

    /// Names of all loaded custom themes (insertion order is unspecified),
    /// suitable for the `/settings` overlay's choice list.
    pub fn custom_names(&self) -> Vec<String> {
        self.custom.keys().cloned().collect()
    }

    /// Number of custom themes currently registered.
    pub fn custom_count(&self) -> usize {
        self.custom.len()
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
        assert_eq!(parse_color("default"), Some(Color::Reset));
    }

    #[test]
    fn parse_indexed_color() {
        assert_eq!(parse_color("i42"), Some(Color::Indexed(42)));
    }

    #[test]
    fn theme_manager_set_by_name() {
        // All six built-ins + the short "dark"/"light" aliases must
        // resolve via ThemeManager::set_theme_by_name (which delegates
        // to Theme::by_name).
        let mgr = ThemeManager::dark();
        for (name, expected) in [
            ("oxicode_dark", "dark"),
            ("oxicode_light", "light"),
            ("nord", "nord"),
            ("catppuccin", "catppuccin"),
            ("github_dark", "github_dark"),
            ("monokai", "monokai"),
            ("dark", "dark"),
            ("light", "light"),
        ] {
            assert!(mgr.set_theme_by_name(name), "expected {name} to resolve");
            assert_eq!(
                mgr.theme().name,
                expected,
                "set_theme_by_name({name}) → {}",
                mgr.theme().name
            );
        }
        // Unknown names still "succeed" but fall back to dark — the
        // contract is "never fail", callers handle the fallback.
        assert!(mgr.set_theme_by_name("nonexistent"));
        assert_eq!(mgr.theme().name, "dark");
        assert!(mgr.set_theme_by_name(""));
        assert_eq!(mgr.theme().name, "dark");
        assert!(mgr.set_theme_by_name("default"));
        assert_eq!(mgr.theme().name, "dark");
    }

    #[test]
    fn theme_manager_check_external_detects_mtime_change() {
        let dir = std::env::temp_dir().join("oxicode-tui-check-external");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, "theme = \"dark\"\n").unwrap();

        let mut mgr = ThemeManager::dark();
        mgr.set_poll_interval(std::time::Duration::from_millis(50));

        // Seed the baseline by reading the current mtime.
        let baseline = std::fs::metadata(&path).unwrap().modified().unwrap();

        // First poll: no change expected.
        assert!(!mgr.check_external(&path, Some(baseline)));

        // Modify the file. mtime granularity on some filesystems is
        // 1s, so bump it twice with a sleep to guarantee a change.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&path, "theme = \"nord\"\n").unwrap();
        let new_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(new_mtime > baseline, "mtime should advance");

        assert!(mgr.check_external(&path, Some(baseline)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn theme_manager_check_external_returns_false_for_missing_path() {
        let mut mgr = ThemeManager::dark();
        mgr.set_poll_interval(std::time::Duration::from_millis(10));
        let bogus = std::path::PathBuf::from("/nonexistent/oxicode-tui/check-external");
        // Missing file → no mtime → returns false. The caller is
        // expected to surface this as a load error elsewhere.
        assert!(!mgr.check_external(&bogus, None));
    }

    #[test]
    fn theme_manager_check_external_no_baseline_does_not_fire() {
        // With `initial = None`, the very first poll must NOT fire a
        // spurious "changed" event — the caller hasn't seeded yet.
        let dir = std::env::temp_dir().join("oxicode-tui-check-external-nobase");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.toml");
        std::fs::write(&path, "x = 1\n").unwrap();

        let mut mgr = ThemeManager::dark();
        mgr.set_poll_interval(std::time::Duration::from_millis(10));
        assert!(!mgr.check_external(&path, None));
        std::fs::remove_dir_all(&dir).ok();
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
        let dir = std::env::temp_dir().join("oxicode-tui-theme-test");
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

    #[test]
    fn theme_defaults_to_unicode_glyphs() {
        let theme = Theme::dark();
        assert_eq!(theme.symbols, crate::symbols::Symbols::unicode());
    }

    #[test]
    fn with_glyph_set_propagates_to_styles() {
        // The glyph set chosen on a Theme must reach ThemeStyles.symbols —
        // that is the field every render function reads.
        for set in crate::symbols::GlyphSet::ALL {
            let theme = Theme::dark().with_glyph_set(set);
            assert_eq!(theme.symbols, set.symbols());
            assert_eq!(theme.to_styles().symbols, set.symbols());
        }
    }

    #[test]
    fn set_glyph_set_mutates_in_place() {
        let mut theme = Theme::dark();
        assert_eq!(
            theme.symbols.cursor,
            crate::symbols::Symbols::unicode().cursor
        );
        theme.set_glyph_set(crate::symbols::GlyphSet::Ascii);
        assert_eq!(theme.symbols, crate::symbols::Symbols::ascii());
    }
    #[test]
    fn theme_by_name_resolves_all_builtins() {
        assert_eq!(Theme::by_name("oxicode_dark").name, "dark");
        assert_eq!(Theme::by_name("oxicode_light").name, "light");
        assert_eq!(Theme::by_name("nord").name, "nord");
        assert_eq!(Theme::by_name("catppuccin").name, "catppuccin");
        assert_eq!(Theme::by_name("github_dark").name, "github_dark");
        assert_eq!(Theme::by_name("monokai").name, "monokai");
    }
    #[test]
    fn theme_by_name_falls_back_to_dark() {
        // Sentinel and unknown names all resolve to the dark theme.
        assert_eq!(Theme::by_name("default").name, "dark");
        assert_eq!(Theme::by_name("").name, "dark");
        assert_eq!(Theme::by_name("nonexistent").name, "dark");
    }
    #[test]
    fn color_scheme_constructors_distinct_backgrounds() {
        // Each built-in scheme must have a unique background so themes
        // are visually distinguishable.
        let bgs = [
            ColorScheme::dark().background,
            ColorScheme::light().background,
            ColorScheme::nord().background,
            ColorScheme::catppuccin().background,
            ColorScheme::github_dark().background,
            ColorScheme::monokai().background,
        ];
        for i in 0..bgs.len() {
            for j in (i + 1)..bgs.len() {
                assert_ne!(bgs[i], bgs[j], "backgrounds at {i} and {j} collide");
            }
        }
    }

    // ── ThemeRegistry ───────────────────────────────────────────────

    #[test]
    fn registry_resolves_all_six_builtins() {
        let reg = ThemeRegistry::with_builtins();
        for &name in THEME_NAMES {
            let t = reg.resolve(name);
            // built-in `name` is the long form (e.g. "oxicode_dark"); the
            // resolved theme stores its short canonical name ("dark").
            // We check the alias also resolves.
            assert!(!t.name.is_empty(), "resolved theme must have a name");
        }
        // Short aliases also work.
        assert_eq!(reg.resolve("dark").name, "dark");
        assert_eq!(reg.resolve("light").name, "light");
        assert_eq!(reg.resolve("nord").name, "nord");
        assert_eq!(reg.resolve("catppuccin").name, "catppuccin");
        assert_eq!(reg.resolve("github_dark").name, "github_dark");
        assert_eq!(reg.resolve("monokai").name, "monokai");
    }

    #[test]
    fn registry_unknown_name_falls_back_to_dark() {
        let reg = ThemeRegistry::with_builtins();
        assert_eq!(reg.resolve("").name, "dark");
        assert_eq!(reg.resolve("default").name, "dark");
        assert_eq!(reg.resolve("nonexistent").name, "dark");
        // oxios-style sentinel.
        assert_eq!(reg.resolve("oxicode_dark").name, "dark");
    }

    #[test]
    fn registry_add_custom_and_resolve() {
        let mut reg = ThemeRegistry::with_builtins();
        let custom = Theme {
            name: "my_red".into(),
            colors: ColorScheme {
                foreground: Color::Rgb(255, 255, 255),
                background: Color::Rgb(20, 0, 0),
                ..ColorScheme::dark()
            },
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        };
        reg.add_custom(custom);

        let resolved = reg.resolve("my_red");
        assert_eq!(resolved.name, "my_red");
        assert_eq!(resolved.colors.background, Color::Rgb(20, 0, 0));
        // Custom shouldn't disturb built-ins.
        assert_eq!(reg.resolve("nord").name, "nord");
    }

    #[test]
    fn registry_custom_overrides_builtin_with_same_name() {
        // User putting a file named "nord.toml" in ~/.oxicode/themes/
        // intentionally overrides the built-in Nord palette.
        let mut reg = ThemeRegistry::with_builtins();
        let custom_nord = Theme {
            name: "nord".into(),
            colors: ColorScheme {
                foreground: Color::Rgb(0, 0, 0),
                ..ColorScheme::dark()
            },
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        };
        reg.add_custom(custom_nord);

        assert_eq!(reg.resolve("nord").colors.foreground, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn registry_custom_names_lists_inserted() {
        let mut reg = ThemeRegistry::with_builtins();
        assert_eq!(reg.custom_count(), 0);
        assert!(reg.custom_names().is_empty());

        reg.add_custom(Theme {
            name: "alpha".into(),
            colors: ColorScheme::dark(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        });
        reg.add_custom(Theme {
            name: "beta".into(),
            colors: ColorScheme::dark(),
            spacing: Spacing::default(),
            symbols: crate::symbols::Symbols::default(),
        });
        assert_eq!(reg.custom_count(), 2);
        let names = reg.custom_names();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
    }

    #[test]
    fn registry_add_custom_file_parses_json_and_toml() {
        let dir = std::env::temp_dir().join("oxicode-tui-registry-test");
        std::fs::create_dir_all(&dir).unwrap();

        // JSON theme
        let json_path = dir.join("my_json.json");
        std::fs::write(
            &json_path,
            r##"{"name":"json_theme","colors":{"primary":"#ff00ff"}}"##,
        )
        .unwrap();
        // TOML theme
        let toml_path = dir.join("my_toml.toml");
        std::fs::write(
            &toml_path,
            r##"name = "toml_theme"
[colors]
primary = "#00ff00"
"##,
        )
        .unwrap();

        let mut reg = ThemeRegistry::with_builtins();
        let j = reg.add_custom_file(&json_path).unwrap();
        assert_eq!(j.name, "json_theme");
        assert_eq!(j.colors.primary, Color::Rgb(255, 0, 255));

        let t = reg.add_custom_file(&toml_path).unwrap();
        assert_eq!(t.name, "toml_theme");
        assert_eq!(t.colors.primary, Color::Rgb(0, 255, 0));

        assert_eq!(
            reg.resolve("json_theme").colors.primary,
            Color::Rgb(255, 0, 255)
        );
        assert_eq!(
            reg.resolve("toml_theme").colors.primary,
            Color::Rgb(0, 255, 0)
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn registry_add_custom_file_rejects_unknown_extension() {
        let dir = std::env::temp_dir().join("oxicode-tui-registry-ext");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.txt");
        std::fs::write(&p, "name = \"x\"").unwrap();

        let mut reg = ThemeRegistry::with_builtins();
        let err = reg.add_custom_file(&p).unwrap_err();
        assert!(err.to_string().contains("Unsupported"));
        assert_eq!(reg.custom_count(), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn registry_add_custom_file_missing_path_errors() {
        let mut reg = ThemeRegistry::with_builtins();
        let bogus = std::path::PathBuf::from("/nonexistent/oxicode-tui/test.json");
        assert!(reg.add_custom_file(&bogus).is_err());
        assert_eq!(reg.custom_count(), 0);
    }

    // ── Phase-1 background slot tests ──────────────────────────────────

    fn rgb_sum(c: Color) -> u32 {
        match c {
            Color::Rgb(r, g, b) => r as u32 + g as u32 + b as u32,
            _ => 0,
        }
    }

    #[test]
    fn all_themes_have_28_color_slots() {
        // Every built-in must populate all 28 ColorScheme fields (no
        // accidental zero/uninitialized values on the 7 new slots).
        for &name in THEME_NAMES {
            let cs = Theme::by_name(name).colors;
            // New slots must not be default Color::Reset (0,0,0 only
            // valid for dark which is true black). Check they are Rgb.
            assert!(
                matches!(cs.response_bg, Color::Rgb(_, _, _)),
                "{name} response_bg"
            );
            assert!(
                matches!(cs.thinking_bg, Color::Rgb(_, _, _)),
                "{name} thinking_bg"
            );
            assert!(
                matches!(cs.surface_bg, Color::Rgb(_, _, _)),
                "{name} surface_bg"
            );
            assert!(
                matches!(cs.panel_bg, Color::Rgb(_, _, _)),
                "{name} panel_bg"
            );
            assert!(
                matches!(cs.diff_add_bg, Color::Rgb(_, _, _)),
                "{name} diff_add_bg"
            );
            assert!(
                matches!(cs.diff_remove_bg, Color::Rgb(_, _, _)),
                "{name} diff_remove_bg"
            );
            assert!(
                matches!(cs.diff_hunk_bg, Color::Rgb(_, _, _)),
                "{name} diff_hunk_bg"
            );
        }
    }

    #[test]
    fn dark_brightness_hierarchy_is_ordered() {
        // background <= response_bg < thinking_bg < surface_bg < user_bg < panel_bg
        let cs = ColorScheme::dark();
        let bg = rgb_sum(cs.background);
        let resp = rgb_sum(cs.response_bg);
        let think = rgb_sum(cs.thinking_bg);
        let surf = rgb_sum(cs.surface_bg);
        let user = rgb_sum(cs.user_bg);
        let panel = rgb_sum(cs.panel_bg);
        assert_eq!(bg, resp, "response_bg should equal background");
        assert!(
            bg <= think,
            "background({bg}) should be <= thinking_bg({think})"
        );
        assert!(
            think <= surf,
            "thinking_bg({think}) should be <= surface_bg({surf})"
        );
        assert!(
            surf <= user,
            "surface_bg({surf}) should be <= user_bg({user})"
        );
        assert!(
            user <= panel,
            "user_bg({user}) should be <= panel_bg({panel})"
        );
    }

    #[test]
    fn diff_bg_reuses_tool_status_bg() {
        // diff_add_bg should match tool_success_bg, diff_remove_bg tool_error_bg
        let cs = ColorScheme::dark();
        assert_eq!(cs.diff_add_bg, cs.tool_success_bg);
        assert_eq!(cs.diff_remove_bg, cs.tool_error_bg);
    }

    #[test]
    fn to_styles_packs_all_28_styles() {
        // ThemeStyles must carry every ColorScheme slot as a Style.
        let styles = ColorScheme::dark().to_styles();
        // Verify the 7 new bg styles have a non-Reset bg color.
        assert!(
            styles.response_bg.bg.is_some(),
            "response_bg style missing bg"
        );
        assert!(
            styles.thinking_bg.bg.is_some(),
            "thinking_bg style missing bg"
        );
        assert!(
            styles.surface_bg.bg.is_some(),
            "surface_bg style missing bg"
        );
        assert!(styles.panel_bg.bg.is_some(), "panel_bg style missing bg");
        assert!(
            styles.diff_add_bg.bg.is_some(),
            "diff_add_bg style missing bg"
        );
        assert!(
            styles.diff_remove_bg.bg.is_some(),
            "diff_remove_bg style missing bg"
        );
        assert!(
            styles.diff_hunk_bg.bg.is_some(),
            "diff_hunk_bg style missing bg"
        );
    }
}
