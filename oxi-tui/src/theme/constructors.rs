//! 6 named `ColorScheme` constructors.
//!
//! All 19 fields with legacy counterparts (`background`, `foreground`,
//! `primary`, `accent`, `muted`, `border`, `user_bg`, `response_bg`,
//! `thinking_bg`, `success`, `warning`, `error`, `code_bg`, `selection_bg`,
//! `surface_bg`, `panel_bg`, `diff_add_bg`, `diff_remove_bg`, `diff_hunk_bg`)
//! use the exact RGB triple from `oxi-tui-legacy/src/theme.rs`.
//!
//! The 9 fg slots that the legacy did not model (`user`, `response`,
//! `thinking`, `tool`, `tool_bg`, `info`, `diff_add`, `diff_remove`,
//! `diff_hunk`) are derived from the closest legacy analog:
//!
//! | new slot     | derived from (legacy)        | rationale                  |
//! |--------------|------------------------------|----------------------------|
//! | `user`       | `user_border`                | matches primary accent     |
//! | `response`   | `foreground`                 | default body text          |
//! | `thinking`   | `muted`                      | dimmed reasoning text      |
//! | `tool`       | `muted`                      | tool headers are dimmed    |
//! | `tool_bg`    | `tool_pending_bg`            | subtle tool call bg        |
//! | `info`       | `secondary`                  | legacy's blue-green accent |
//! | `diff_add`   | `success`                    | green = added line         |
//! | `diff_remove`| `error`                      | red = removed line         |
//! | `diff_hunk`  | `muted`                      | subtle hunk header         |

#![allow(clippy::doc_markdown)] // legacy doc text mentions `code` etc.

use super::palette::ColorScheme;
use ratatui::style::{Color, Style};

impl ColorScheme {
    /// Default dark color scheme (true black). Neutral grays inspired by grok GrokNight.
    #[must_use]
    pub fn dark() -> Self {
        // Legacy shared slots.
        let background = Color::Rgb(0, 0, 0);
        let foreground = Color::Rgb(205, 214, 244); // #cdd6f4
        let primary = Color::Rgb(122, 162, 247); // #7aa2f7
        let muted = Color::Rgb(127, 132, 156); // #7f849c
        let accent = Color::Rgb(187, 154, 247); // #bb9af7
        let border = Color::Rgb(88, 91, 112); // #585b70
        let success = Color::Rgb(158, 206, 106); // #9ece6a
        let warning = Color::Rgb(224, 175, 104); // #e0af68
        let error = Color::Rgb(247, 118, 142); // #f7768e
        let info = Color::Rgb(158, 206, 106); // legacy secondary
        let user_border = Color::Rgb(122, 162, 247); // #7aa2f7
        let tool_pending_bg = Color::Rgb(18, 20, 28); // #12141c
        let code_bg = Color::Rgb(35, 30, 20); // #231e14
        let selection_bg = Color::Rgb(40, 40, 60); // #28283c
        let user_bg = Color::Rgb(18, 22, 38); // #121626
        let response_bg = Color::Rgb(0, 0, 0); // = background
        let thinking_bg = Color::Rgb(11, 9, 15); // #0b090f
        let surface_bg = Color::Rgb(9, 11, 19); // #090b13
        let panel_bg = Color::Rgb(53, 56, 75); // #35384b
        let diff_add_bg = Color::Rgb(16, 26, 14); // legacy tool_success_bg
        let diff_remove_bg = Color::Rgb(32, 16, 18); // legacy tool_error_bg
        let diff_hunk_bg = Color::Rgb(15, 16, 19); // #0f1013
        // Derived fg slots.
        let user = user_border;
        let response = foreground;
        let thinking = muted;
        let tool = muted;
        let tool_bg = tool_pending_bg;
        let diff_add = success;
        let diff_remove = error;
        let diff_hunk = muted;

        Self {
            background,
            foreground,
            primary,
            accent,
            muted,
            border,
            user,
            user_bg,
            response,
            response_bg,
            thinking,
            thinking_bg,
            tool,
            tool_bg,
            success,
            warning,
            error,
            info,
            diff_add,
            diff_remove,
            diff_hunk,
            surface_bg,
            panel_bg,
            code_bg,
            selection_bg,
            diff_add_bg,
            diff_remove_bg,
            diff_hunk_bg,
        }
    }

    /// Default light color scheme.
    #[must_use]
    pub fn light() -> Self {
        let background = Color::Rgb(239, 241, 245);
        let foreground = Color::Rgb(76, 79, 105); // #4c4f69
        let primary = Color::Rgb(30, 102, 240); // #1e66f0
        let muted = Color::Rgb(92, 95, 119); // #5c5f77
        let accent = Color::Rgb(136, 57, 239); // #8839ef
        let border = Color::Rgb(156, 160, 176);
        let success = Color::Rgb(64, 160, 43); // #40a02b
        let warning = Color::Rgb(223, 142, 29); // #df8e1d
        let error = Color::Rgb(210, 15, 57); // #d20f39
        let info = Color::Rgb(64, 160, 43); // legacy secondary
        let user_border = Color::Rgb(30, 102, 240); // #1e66f0
        let tool_pending_bg = Color::Rgb(235, 238, 245); // #ebeeff
        let code_bg = Color::Rgb(240, 240, 245);
        let selection_bg = Color::Rgb(204, 208, 218);
        let user_bg = Color::Rgb(225, 236, 255);
        let response_bg = Color::Rgb(239, 241, 245);
        let thinking_bg = Color::Rgb(233, 230, 245);
        let surface_bg = Color::Rgb(232, 238, 250);
        let panel_bg = Color::Rgb(190, 198, 216);
        let diff_add_bg = Color::Rgb(230, 248, 230);
        let diff_remove_bg = Color::Rgb(255, 230, 235);
        let diff_hunk_bg = Color::Rgb(221, 223, 230);
        let user = user_border;
        let response = foreground;
        let thinking = muted;
        let tool = muted;
        let tool_bg = tool_pending_bg;
        let diff_add = success;
        let diff_remove = error;
        let diff_hunk = muted;

        Self {
            background,
            foreground,
            primary,
            accent,
            muted,
            border,
            user,
            user_bg,
            response,
            response_bg,
            thinking,
            thinking_bg,
            tool,
            tool_bg,
            success,
            warning,
            error,
            info,
            diff_add,
            diff_remove,
            diff_hunk,
            surface_bg,
            panel_bg,
            code_bg,
            selection_bg,
            diff_add_bg,
            diff_remove_bg,
            diff_hunk_bg,
        }
    }

    /// Nord color scheme (Arctic, dark).
    #[must_use]
    pub fn nord() -> Self {
        let background = Color::Rgb(46, 52, 64);
        let foreground = Color::Rgb(216, 222, 233);
        let primary = Color::Rgb(136, 192, 208);
        let muted = Color::Rgb(97, 110, 136);
        let accent = Color::Rgb(180, 142, 173);
        let border = Color::Rgb(76, 86, 106);
        let success = Color::Rgb(163, 190, 140);
        let warning = Color::Rgb(235, 203, 139);
        let error = Color::Rgb(191, 97, 106);
        let info = Color::Rgb(163, 190, 140); // legacy secondary
        let user_border = Color::Rgb(136, 192, 208);
        let tool_pending_bg = Color::Rgb(46, 52, 64); // nord0
        let code_bg = Color::Rgb(59, 66, 82);
        let selection_bg = Color::Rgb(67, 76, 94);
        let user_bg = Color::Rgb(59, 66, 82);
        let response_bg = Color::Rgb(46, 52, 64);
        let thinking_bg = Color::Rgb(54, 57, 71);
        let surface_bg = Color::Rgb(52, 59, 73);
        let panel_bg = Color::Rgb(68, 76, 94);
        let diff_add_bg = Color::Rgb(40, 56, 44);
        let diff_remove_bg = Color::Rgb(56, 42, 44);
        let diff_hunk_bg = Color::Rgb(52, 59, 73);
        let user = user_border;
        let response = foreground;
        let thinking = muted;
        let tool = muted;
        let tool_bg = tool_pending_bg;
        let diff_add = success;
        let diff_remove = error;
        let diff_hunk = muted;

        Self {
            background,
            foreground,
            primary,
            accent,
            muted,
            border,
            user,
            user_bg,
            response,
            response_bg,
            thinking,
            thinking_bg,
            tool,
            tool_bg,
            success,
            warning,
            error,
            info,
            diff_add,
            diff_remove,
            diff_hunk,
            surface_bg,
            panel_bg,
            code_bg,
            selection_bg,
            diff_add_bg,
            diff_remove_bg,
            diff_hunk_bg,
        }
    }

    /// Catppuccin Mocha color scheme (dark).
    #[must_use]
    pub fn catppuccin() -> Self {
        let background = Color::Rgb(30, 30, 46);
        let foreground = Color::Rgb(205, 214, 244);
        let primary = Color::Rgb(137, 180, 250);
        let muted = Color::Rgb(127, 132, 156);
        let accent = Color::Rgb(203, 166, 247);
        let border = Color::Rgb(88, 91, 112);
        let success = Color::Rgb(166, 227, 161);
        let warning = Color::Rgb(249, 226, 175);
        let error = Color::Rgb(243, 139, 168);
        let info = Color::Rgb(166, 227, 161); // legacy secondary
        let user_border = Color::Rgb(137, 180, 250);
        let tool_pending_bg = Color::Rgb(30, 30, 46);
        let code_bg = Color::Rgb(49, 50, 68);
        let selection_bg = Color::Rgb(69, 71, 90);
        let user_bg = Color::Rgb(49, 50, 68);
        let response_bg = Color::Rgb(30, 30, 46);
        let thinking_bg = Color::Rgb(40, 38, 58);
        let surface_bg = Color::Rgb(40, 40, 57);
        let panel_bg = Color::Rgb(68, 70, 90);
        let diff_add_bg = Color::Rgb(32, 46, 36);
        let diff_remove_bg = Color::Rgb(48, 34, 40);
        let diff_hunk_bg = Color::Rgb(42, 42, 59);
        let user = user_border;
        let response = foreground;
        let thinking = muted;
        let tool = muted;
        let tool_bg = tool_pending_bg;
        let diff_add = success;
        let diff_remove = error;
        let diff_hunk = muted;

        Self {
            background,
            foreground,
            primary,
            accent,
            muted,
            border,
            user,
            user_bg,
            response,
            response_bg,
            thinking,
            thinking_bg,
            tool,
            tool_bg,
            success,
            warning,
            error,
            info,
            diff_add,
            diff_remove,
            diff_hunk,
            surface_bg,
            panel_bg,
            code_bg,
            selection_bg,
            diff_add_bg,
            diff_remove_bg,
            diff_hunk_bg,
        }
    }

    /// GitHub Dark color scheme.
    #[must_use]
    pub fn github_dark() -> Self {
        let background = Color::Rgb(13, 17, 23);
        let foreground = Color::Rgb(201, 209, 217);
        let primary = Color::Rgb(47, 129, 247);
        let muted = Color::Rgb(139, 148, 158);
        let accent = Color::Rgb(163, 113, 247);
        let border = Color::Rgb(48, 54, 61);
        let success = Color::Rgb(63, 185, 80);
        let warning = Color::Rgb(210, 153, 34);
        let error = Color::Rgb(248, 81, 73);
        let info = Color::Rgb(63, 185, 80); // legacy secondary
        let user_border = Color::Rgb(47, 129, 247);
        let tool_pending_bg = Color::Rgb(13, 17, 23);
        let code_bg = Color::Rgb(22, 27, 34);
        let selection_bg = Color::Rgb(38, 79, 120);
        let user_bg = Color::Rgb(22, 27, 34);
        let response_bg = Color::Rgb(13, 17, 23);
        let thinking_bg = Color::Rgb(22, 23, 36);
        let surface_bg = Color::Rgb(18, 22, 28);
        let panel_bg = Color::Rgb(35, 40, 48);
        let diff_add_bg = Color::Rgb(18, 30, 20);
        let diff_remove_bg = Color::Rgb(34, 18, 20);
        let diff_hunk_bg = Color::Rgb(28, 33, 39);
        let user = user_border;
        let response = foreground;
        let thinking = muted;
        let tool = muted;
        let tool_bg = tool_pending_bg;
        let diff_add = success;
        let diff_remove = error;
        let diff_hunk = muted;

        Self {
            background,
            foreground,
            primary,
            accent,
            muted,
            border,
            user,
            user_bg,
            response,
            response_bg,
            thinking,
            thinking_bg,
            tool,
            tool_bg,
            success,
            warning,
            error,
            info,
            diff_add,
            diff_remove,
            diff_hunk,
            surface_bg,
            panel_bg,
            code_bg,
            selection_bg,
            diff_add_bg,
            diff_remove_bg,
            diff_hunk_bg,
        }
    }

    /// Monokai color scheme (dark).
    #[must_use]
    pub fn monokai() -> Self {
        let background = Color::Rgb(39, 40, 34);
        let foreground = Color::Rgb(248, 248, 242);
        let primary = Color::Rgb(102, 217, 239);
        let muted = Color::Rgb(117, 113, 94);
        let accent = Color::Rgb(174, 129, 255);
        let border = Color::Rgb(73, 72, 62);
        let success = Color::Rgb(166, 226, 46);
        let warning = Color::Rgb(253, 151, 31);
        let error = Color::Rgb(249, 38, 114);
        let info = Color::Rgb(166, 226, 46); // legacy secondary
        let user_border = Color::Rgb(102, 217, 239);
        let tool_pending_bg = Color::Rgb(39, 40, 34);
        let code_bg = Color::Rgb(62, 61, 50);
        let selection_bg = Color::Rgb(73, 72, 62);
        let user_bg = Color::Rgb(62, 61, 50);
        let response_bg = Color::Rgb(39, 40, 34);
        let thinking_bg = Color::Rgb(47, 45, 47);
        let surface_bg = Color::Rgb(50, 50, 42);
        let panel_bg = Color::Rgb(68, 66, 56);
        let diff_add_bg = Color::Rgb(34, 44, 26);
        let diff_remove_bg = Color::Rgb(50, 30, 38);
        let diff_hunk_bg = Color::Rgb(48, 49, 41);
        let user = user_border;
        let response = foreground;
        let thinking = muted;
        let tool = muted;
        let tool_bg = tool_pending_bg;
        let diff_add = success;
        let diff_remove = error;
        let diff_hunk = muted;

        Self {
            background,
            foreground,
            primary,
            accent,
            muted,
            border,
            user,
            user_bg,
            response,
            response_bg,
            thinking,
            thinking_bg,
            tool,
            tool_bg,
            success,
            warning,
            error,
            info,
            diff_add,
            diff_remove,
            diff_hunk,
            surface_bg,
            panel_bg,
            code_bg,
            selection_bg,
            diff_add_bg,
            diff_remove_bg,
            diff_hunk_bg,
        }
    }

    /// Convert to ratatui Style with foreground and background.
    #[must_use]
    pub fn to_style(&self) -> Style {
        Style::default().fg(self.foreground).bg(self.background)
    }
}
