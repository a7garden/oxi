//! Per-kind terminal detection logic.
//!
//! Kept separate from the public [`super::TerminalCaps`] surface so the
//! classifier stays small and the `pub` API doesn't leak internal types
//! (e.g. [`TerminalKind`]).

use std::env;

use super::{ColorLevel, ImageProtocol, TerminalCaps};

/// The host terminal family, derived from env-var signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalKind {
    Kitty,
    Ghostty,
    WezTerm,
    ITerm2,
    Foot,
    Contour,
    Alacritty,
    Konsole,
    Blackbox,
    Tabby,
    AppleTerminal,
    Xterm,
    Tmux,
    Screen,
    /// Anything we don't recognize — gets conservative defaults.
    Unknown,
}

impl TerminalKind {
    /// Human-readable family name surfaced via [`super::TerminalCaps::terminal_name`].
    fn display_name(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Ghostty => "ghostty",
            Self::WezTerm => "wezterm",
            Self::ITerm2 => "iTerm2",
            Self::Foot => "foot",
            Self::Contour => "contour",
            Self::Alacritty => "alacritty",
            Self::Konsole => "konsole",
            Self::Blackbox => "blackbox",
            Self::Tabby => "tabby",
            Self::AppleTerminal => "Apple Terminal",
            Self::Xterm => "xterm",
            Self::Tmux => "tmux",
            Self::Screen => "screen",
            Self::Unknown => "unknown",
        }
    }
    /// Collect the relevant env-var signals into a value object.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn signals(
        term_program: &str,
        term: &str,
        kitty_window_id: bool,
        ghostty_resources: bool,
        wezterm_pane: bool,
        iterm_session: bool,
        tmux: bool,
    ) -> Self {
        // Most-specific signals first.
        if kitty_window_id || term_program == "kitty" {
            return Self::Kitty;
        }
        if term_program == "ghostty" || ghostty_resources {
            return Self::Ghostty;
        }
        if wezterm_pane || term_program == "WezTerm" {
            return Self::WezTerm;
        }
        if iterm_session || term_program == "iTerm.app" {
            return Self::ITerm2;
        }
        if term_program == "contour" || term.starts_with("contour") {
            return Self::Contour;
        }
        if term_program == "alacritty" {
            return Self::Alacritty;
        }
        if term_program == "konsole" {
            return Self::Konsole;
        }
        if term_program == "blackbox" {
            return Self::Blackbox;
        }
        if term_program == "Tabby" {
            return Self::Tabby;
        }
        if term_program == "Apple_Terminal" {
            return Self::AppleTerminal;
        }
        if term == "foot" || term.starts_with("foot-") {
            return Self::Foot;
        }
        if tmux {
            return Self::Tmux;
        }
        if term.starts_with("screen") {
            return Self::Screen;
        }
        if term.starts_with("xterm") {
            return Self::Xterm;
        }
        Self::Unknown
    }
}

/// Run detection against the real process env. Public via [`super::TerminalCaps::detect`].
pub(super) fn detect_with_env() -> TerminalCaps {
    let term_program = env::var("TERM_PROGRAM").unwrap_or_default();
    let term = env::var("TERM").unwrap_or_default();
    let colorterm = env::var("COLORTERM").unwrap_or_default();
    let kind = TerminalKind::signals(
        &term_program,
        &term,
        env::var_os("KITTY_WINDOW_ID").is_some(),
        env::var_os("GHOSTTY_RESOURCES_DIR").is_some(),
        env::var_os("WEZTERM_PANE").is_some(),
        env::var_os("ITERM_SESSION_ID").is_some(),
        env::var_os("TMUX").is_some(),
    );

    let no_sync = env::var_os("OXI_NO_SYNC_OUTPUT").is_some();
    let mut caps = TerminalCaps::default();
    apply_kind(&mut caps, kind, &colorterm, &term, no_sync);

    // Color-level detection runs last so NO_COLOR can override any
    // true_color flag set by the per-kind table (e.g. tmux over a truecolor
    // terminal still respects NO_COLOR).
    caps.color_level = detect_color_level(
        env::var("NO_COLOR").ok().as_deref(),
        &colorterm,
        &term,
        caps.true_color,
    );

    caps
}

/// Derive [`ColorLevel`] from env-derived inputs. Order of precedence:
/// 1. `no_color` non-empty and not `"0"`/`"false"` → [`ColorLevel::None`].
/// 2. `colorterm` is `"truecolor"`/`"24bit"` → [`ColorLevel::TrueColor`].
/// 3. `term` contains `"256color"` → [`ColorLevel::Ansi256`].
/// 4. Otherwise: `true_color` (from per-kind) → [`ColorLevel::TrueColor`],
///    else [`ColorLevel::Basic`].
///
/// Takes inputs explicitly (rather than reading `env::var`) so the function
/// is unit-testable without `unsafe env::set_var` (the crate forbids
/// `unsafe_code`). [`detect_with_env`] reads `NO_COLOR` and forwards.
pub(super) fn detect_color_level(
    no_color: Option<&str>,
    colorterm: &str,
    term: &str,
    true_color: bool,
) -> ColorLevel {
    // Per https://no-color.org/, any non-empty value disables color.
    // Allow "0" / "false" as explicit opt-outs (matches common practice).
    if let Some(nc) = no_color
        && !nc.is_empty()
        && nc != "0"
        && !nc.eq_ignore_ascii_case("false")
    {
        return ColorLevel::None;
    }
    if colorterm == "truecolor" || colorterm == "24bit" {
        return ColorLevel::TrueColor;
    }
    if term.contains("256color") {
        return ColorLevel::Ansi256;
    }
    if true_color {
        ColorLevel::TrueColor
    } else {
        ColorLevel::Basic
    }
}

/// Apply the per-kind capability table onto `caps`. `no_sync` forces
/// synchronized output off regardless of kind.
pub(super) fn apply_kind(
    caps: &mut TerminalCaps,
    kind: TerminalKind,
    colorterm: &str,
    term: &str,
    no_sync: bool,
) {
    caps.terminal_name = Some(kind.display_name().to_string());
    match kind {
        TerminalKind::Kitty | TerminalKind::Ghostty => {
            caps.image_protocol = Some(ImageProtocol::Kitty);
            caps.true_color = true;
            caps.hyperlinks = true;
            caps.kitty_protocol = true;
            caps.synchronized_output = true;
            // Kitty/Ghostty implement the DECCARA rectangular SGR extension.
            caps.deccara = true;
        }
        TerminalKind::WezTerm | TerminalKind::Contour => {
            caps.image_protocol = Some(ImageProtocol::Kitty);
            caps.true_color = true;
            caps.hyperlinks = true;
            caps.kitty_protocol = true;
            caps.synchronized_output = true;
            caps.sixel = true;
        }
        TerminalKind::ITerm2 => {
            caps.image_protocol = Some(ImageProtocol::ITerm2);
            caps.true_color = true;
            caps.hyperlinks = true;
            caps.synchronized_output = true;
        }
        TerminalKind::Foot => {
            caps.true_color = true;
            caps.hyperlinks = true;
            caps.synchronized_output = true;
            caps.sixel = true;
        }
        TerminalKind::Alacritty => {
            caps.true_color = true;
            // Alacritty supports BSU/ESU since 0.13.
            caps.synchronized_output = true;
        }
        TerminalKind::Konsole => {
            caps.true_color = true;
            caps.hyperlinks = true;
            caps.synchronized_output = true;
        }
        TerminalKind::Blackbox | TerminalKind::Tabby | TerminalKind::AppleTerminal => {
            caps.true_color = true;
            caps.hyperlinks = true;
        }
        TerminalKind::Xterm => {
            caps.true_color = colorterm == "truecolor" || colorterm == "24bit";
            // synchronized_output stays at its safe default (true).
        }
        TerminalKind::Tmux | TerminalKind::Screen | TerminalKind::Unknown => {
            caps.true_color =
                colorterm == "truecolor" || colorterm == "24bit" || term.contains("256color");
            // synchronized_output stays at its safe default (true); tmux
            // passes unknown sequences through or strips them harmlessly.
        }
    }
    if no_sync {
        caps.synchronized_output = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::fn_params_excessive_bools)]
    fn classify(
        term_program: &str,
        term: &str,
        kitty: bool,
        ghostty: bool,
        wezterm: bool,
        iterm: bool,
        tmux: bool,
    ) -> TerminalKind {
        TerminalKind::signals(term_program, term, kitty, ghostty, wezterm, iterm, tmux)
    }

    #[test]
    fn default_sync_on_deccara_sixel_off() {
        let caps = TerminalCaps::default();
        assert!(caps.synchronized_output);
        assert!(!caps.deccara);
        assert!(!caps.sixel);
        assert!(caps.image_protocol.is_none());
        assert!(!caps.true_color);
        assert!(caps.terminal_name.is_none());
        assert_eq!(caps.color_level, ColorLevel::None);
    }

    #[test]
    fn supports_images_with_kitty() {
        let caps = TerminalCaps {
            image_protocol: Some(ImageProtocol::Kitty),
            ..TerminalCaps::default()
        };
        assert!(caps.supports_images());
    }

    #[test]
    fn supports_images_with_iterm2() {
        let caps = TerminalCaps {
            image_protocol: Some(ImageProtocol::ITerm2),
            ..TerminalCaps::default()
        };
        assert!(caps.supports_images());
    }
    #[test]
    fn classify_kitty() {
        assert_eq!(
            classify("", "xterm-kitty", true, false, false, false, false),
            TerminalKind::Kitty
        );
        assert_eq!(
            classify("kitty", "", false, false, false, false, false),
            TerminalKind::Kitty
        );
    }

    #[test]
    fn classify_ghostty() {
        assert_eq!(
            classify("ghostty", "xterm-ghostty", false, true, false, false, false),
            TerminalKind::Ghostty
        );
    }

    #[test]
    fn classify_wezterm_and_iterm() {
        assert_eq!(
            classify("WezTerm", "", false, false, true, false, false),
            TerminalKind::WezTerm
        );
        assert_eq!(
            classify("iTerm.app", "", false, false, false, true, false),
            TerminalKind::ITerm2
        );
    }

    #[test]
    fn classify_tmux_and_screen_take_priority_over_xterm() {
        assert_eq!(
            classify("", "xterm-256color", false, false, false, false, true),
            TerminalKind::Tmux
        );
        assert_eq!(
            classify("", "screen-256color", false, false, false, false, false),
            TerminalKind::Screen
        );
    }

    #[test]
    fn classify_unknown_fallback() {
        assert_eq!(
            classify("", "dumb", false, false, false, false, false),
            TerminalKind::Unknown
        );
    }

    // ── capability table ───────────────────────────────────────────

    fn caps_for(kind: TerminalKind) -> TerminalCaps {
        let mut caps = TerminalCaps::default();
        apply_kind(&mut caps, kind, "truecolor", "xterm-256color", false);
        caps
    }

    #[test]
    fn kitty_and_ghostty_get_deccara() {
        for kind in [TerminalKind::Kitty, TerminalKind::Ghostty] {
            let caps = caps_for(kind);
            assert!(caps.deccara, "{kind:?} should support DECCARA");
            assert!(caps.synchronized_output);
            assert!(caps.kitty_protocol);
            assert_eq!(caps.image_protocol, Some(ImageProtocol::Kitty));
        }
    }

    #[test]
    fn wezterm_foot_contour_get_sixel_not_deccara() {
        for kind in [
            TerminalKind::WezTerm,
            TerminalKind::Foot,
            TerminalKind::Contour,
        ] {
            let caps = caps_for(kind);
            assert!(caps.sixel, "{kind:?} should support sixel");
            assert!(!caps.deccara, "{kind:?} should NOT claim DECCARA");
            assert!(caps.synchronized_output);
        }
    }

    #[test]
    fn iterm2_uses_iterm_image_protocol() {
        let caps = caps_for(TerminalKind::ITerm2);
        assert_eq!(caps.image_protocol, Some(ImageProtocol::ITerm2));
        assert!(!caps.deccara);
        assert!(caps.synchronized_output);
    }

    #[test]
    fn xterm_true_color_only_with_colorterm() {
        let mut on = TerminalCaps::default();
        apply_kind(
            &mut on,
            TerminalKind::Xterm,
            "truecolor",
            "xterm-256color",
            false,
        );
        assert!(on.true_color);
        let mut off = TerminalCaps::default();
        apply_kind(&mut off, TerminalKind::Xterm, "", "xterm", false);
        assert!(!off.true_color);
    }

    #[test]
    fn no_sync_override_forces_synchronized_output_off() {
        let mut caps = TerminalCaps::default();
        apply_kind(
            &mut caps,
            TerminalKind::Kitty,
            "truecolor",
            "xterm-kitty",
            true,
        );
        assert!(!caps.synchronized_output);
        assert!(
            caps.deccara,
            "opt-out only affects sync, not other capabilities"
        );
    }

    #[test]
    fn detect_runs_without_panic() {
        let caps = TerminalCaps::detect();
        let no_override = std::env::var_os("OXI_NO_SYNC_OUTPUT").is_none();
        if no_override {
            assert!(caps.synchronized_output);
        }
        assert!(caps.terminal_name.is_some());
    }

    // ── color level ────────────────────────────────────────────────

    #[test]
    fn color_level_no_color_overrides_everything() {
        // NO_COLOR set + non-empty wins over true_color.
        assert_eq!(
            detect_color_level(Some("1"), "truecolor", "xterm-256color", true),
            ColorLevel::None,
        );
        // "0" and "false" are explicit opt-outs.
        assert_eq!(
            detect_color_level(Some("0"), "truecolor", "xterm-256color", true),
            ColorLevel::TrueColor,
        );
        assert_eq!(
            detect_color_level(Some("FALSE"), "truecolor", "xterm-256color", true),
            ColorLevel::TrueColor,
        );
        // Empty value is treated as unset.
        assert_eq!(
            detect_color_level(Some(""), "truecolor", "xterm-256color", true),
            ColorLevel::TrueColor,
        );
    }

    #[test]
    fn color_level_colorterm_truecolor_wins() {
        assert_eq!(
            detect_color_level(None, "truecolor", "xterm", false),
            ColorLevel::TrueColor
        );
        assert_eq!(
            detect_color_level(None, "24bit", "xterm", false),
            ColorLevel::TrueColor
        );
    }

    #[test]
    fn color_level_term_256color_means_ansi256() {
        assert_eq!(
            detect_color_level(None, "", "xterm-256color", false),
            ColorLevel::Ansi256
        );
    }

    #[test]
    fn color_level_unknown_term_falls_back_to_basic() {
        assert_eq!(
            detect_color_level(None, "", "dumb", false),
            ColorLevel::Basic
        );
    }

    #[test]
    fn color_level_true_color_from_per_kind_carries_through() {
        // No COLORTERM, no 256color, but the kind detected true_color.
        assert_eq!(
            detect_color_level(None, "", "xterm", true),
            ColorLevel::TrueColor
        );
    }
}
