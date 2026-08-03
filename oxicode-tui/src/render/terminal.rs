//! Terminal capability detection.
//!
//! Identifies the host terminal from its environment-variable signature and
//! derives a capability set from a built-in knowledge table. This is the safe,
//! fully unit-testable baseline.
//!
//! A *live* device-attributes probe (DA1/XTGETTCAP/XTVERSION queries that read
//! the terminal's reply from stdin) can refine these heuristics, but it must
//! run before the event loop begins reading input and needs careful stdin
//! isolation to avoid polluting the key stream. That probe is a follow-up; for
//! now every capability is derived from env vars via [`TerminalCapabilities::detect`].
//!
//! ## Safe defaults
//!
//! Each capability carries a *safe default* for unknown terminals:
//! - [`TerminalCapabilities::synchronized_output`] defaults to **true** — the
//!   CSI 2026 sequence is simply ignored by terminals that don't support it, so
//!   emitting it is harmless. Disable with `OXICODE_NO_SYNC_OUTPUT=1`.
//! - [`TerminalCapabilities::deccara`] and [`TerminalCapabilities::sixel`]
//!   default to **false** — emitting those to an unsupported terminal corrupts
//!   the display or renders garbage, so they are only enabled for terminals
//!   known to support them.

use std::env;

/// Supported image protocols for inline terminal images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// Kitty image protocol (supported by Kitty, Ghostty, WezTerm).
    Kitty,
    /// iTerm2 inline image protocol (supported by iTerm2, WezTerm).
    ITerm2,
}

/// The host terminal family, derived from env-var signatures.
///
/// Drives the capability table in [`TerminalCapabilities::detect`]. The detection
/// logic itself is split into a pure classifier ([`TerminalKind::classify`]) so
/// it can be unit-tested without touching the real process environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalKind {
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
    /// Human-readable family name surfaced via [`TerminalCapabilities::terminal_name`].
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
    ///
    /// Kept separate from [`Self::classify`] so the classifier is pure and
    /// testable.
    #[allow(clippy::too_many_arguments)]
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

/// Detected terminal capabilities.
#[derive(Debug, Clone)]
pub struct TerminalCapabilities {
    /// Supported image protocol, if any.
    pub image_protocol: Option<ImageProtocol>,
    /// Whether the terminal supports 24-bit true color.
    pub true_color: bool,
    /// Whether the terminal supports OSC 8 hyperlinks.
    pub hyperlinks: bool,
    /// Whether the Kitty keyboard protocol is active.
    pub kitty_protocol: bool,
    /// Cell size in pixels (width, height), if detectable.
    pub cell_size: Option<(u16, u16)>,
    /// Identified terminal family name, if recognized.
    pub terminal_name: Option<String>,
    /// Supports synchronized output (CSI 2026 BSU/ESU). Defaults to **true** —
    /// the sequence is harmless on unsupported terminals (they ignore it).
    /// Disable with `OXICODE_NO_SYNC_OUTPUT=1`.
    pub synchronized_output: bool,
    /// Supports Kitty's DECCARA rectangular background-fill extension. Defaults
    /// to **false** — emitting DECCARA to an unsupported terminal corrupts the
    /// display, so it is only enabled for terminals known to support it
    /// (Kitty, Ghostty).
    pub deccara: bool,
    /// Supports Sixel inline images. Defaults to **false**.
    pub sixel: bool,
}

impl Default for TerminalCapabilities {
    fn default() -> Self {
        // Safe defaults: synchronized output on (harmless if ignored),
        // deccara/sixel off (harmful if unsupported).
        Self {
            image_protocol: None,
            true_color: false,
            hyperlinks: false,
            kitty_protocol: false,
            cell_size: None,
            terminal_name: None,
            synchronized_output: true,
            deccara: false,
            sixel: false,
        }
    }
}

impl TerminalCapabilities {
    /// Detect terminal capabilities from environment variables.
    ///
    /// Checks `TERM`, `TERM_PROGRAM`, `COLORTERM`, `KITTY_WINDOW_ID`,
    /// `GHOSTTY_RESOURCES_DIR`, `WEZTERM_PANE`, `ITERM_SESSION_ID`, `TMUX`,
    /// and `OXICODE_NO_SYNC_OUTPUT`.
    pub fn detect() -> Self {
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

        // All field writes go through `apply_kind` (a method body, not the
        // `let x = Default::default(); x.f = ..` pattern clippy flags).
        let no_sync = env::var_os("OXICODE_NO_SYNC_OUTPUT").is_some();
        let mut caps = Self::default();
        caps.apply_kind(kind, &colorterm, &term, no_sync);
        caps
    }

    /// Apply the per-kind capability table onto `self` (kept separate from
    /// [`Self::detect`] so it is unit-testable without touching the process
    /// env). `no_sync` forces synchronized output off regardless of kind.
    fn apply_kind(&mut self, kind: TerminalKind, colorterm: &str, term: &str, no_sync: bool) {
        self.terminal_name = Some(kind.display_name().to_string());
        match kind {
            TerminalKind::Kitty | TerminalKind::Ghostty => {
                self.image_protocol = Some(ImageProtocol::Kitty);
                self.true_color = true;
                self.hyperlinks = true;
                self.kitty_protocol = true;
                self.synchronized_output = true;
                // Kitty/Ghostty implement the DECCARA rectangular SGR extension.
                self.deccara = true;
            }
            TerminalKind::WezTerm => {
                self.image_protocol = Some(ImageProtocol::Kitty);
                self.true_color = true;
                self.hyperlinks = true;
                self.kitty_protocol = true;
                self.synchronized_output = true;
                self.sixel = true;
            }
            TerminalKind::ITerm2 => {
                self.image_protocol = Some(ImageProtocol::ITerm2);
                self.true_color = true;
                self.hyperlinks = true;
                self.synchronized_output = true;
            }
            TerminalKind::Foot => {
                self.true_color = true;
                self.hyperlinks = true;
                self.synchronized_output = true;
                self.sixel = true;
            }
            TerminalKind::Contour => {
                self.image_protocol = Some(ImageProtocol::Kitty);
                self.true_color = true;
                self.hyperlinks = true;
                self.kitty_protocol = true;
                self.synchronized_output = true;
                self.sixel = true;
            }
            TerminalKind::Alacritty => {
                self.true_color = true;
                // Alacritty supports BSU/ESU (synchronized output) since 0.13.
                self.synchronized_output = true;
            }
            TerminalKind::Konsole => {
                self.true_color = true;
                self.hyperlinks = true;
                self.synchronized_output = true;
            }
            TerminalKind::Blackbox | TerminalKind::Tabby => {
                self.true_color = true;
                self.hyperlinks = true;
            }
            TerminalKind::AppleTerminal => {
                self.true_color = true;
                self.hyperlinks = true;
            }
            TerminalKind::Xterm => {
                self.true_color = colorterm == "truecolor" || colorterm == "24bit";
                // synchronized_output stays at its safe default (true).
            }
            TerminalKind::Tmux | TerminalKind::Screen | TerminalKind::Unknown => {
                self.true_color =
                    colorterm == "truecolor" || colorterm == "24bit" || term.contains("256color");
                // synchronized_output stays at its safe default (true); tmux
                // passes unknown sequences through or strips them harmlessly.
            }
        }
        if no_sync {
            self.synchronized_output = false;
        }
    }

    /// Check if images are supported.
    pub fn supports_images(&self) -> bool {
        self.image_protocol.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Safe defaults: sync harmless-on, deccara/sixel harmful-off.
        let caps = TerminalCapabilities::default();
        assert!(caps.synchronized_output);
        assert!(!caps.deccara);
        assert!(!caps.sixel);
        assert!(caps.image_protocol.is_none());
        assert!(!caps.true_color);
        assert!(caps.terminal_name.is_none());
    }

    #[test]
    fn supports_images_with_kitty() {
        let caps = TerminalCapabilities {
            image_protocol: Some(ImageProtocol::Kitty),
            ..Default::default()
        };
        assert!(caps.supports_images());
    }

    #[test]
    fn supports_images_with_iterm2() {
        let caps = TerminalCapabilities {
            image_protocol: Some(ImageProtocol::ITerm2),
            ..Default::default()
        };
        assert!(caps.supports_images());
    }

    // ── classification ────────────────────────────────────────────────────

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
        // TERM is xterm but TMUX is set → tmux, not xterm.
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

    // ── capability table ──────────────────────────────────────────────────

    fn caps_for(kind: TerminalKind) -> TerminalCapabilities {
        let mut caps = TerminalCapabilities::default();
        caps.apply_kind(kind, "truecolor", "xterm-256color", false);
        caps
    }

    #[test]
    fn kitty_and_ghostty_get_deccara() {
        // The headline capability Phase 2 (DECCARA bg-fill) will gate on.
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
        let mut on = TerminalCapabilities::default();
        on.apply_kind(TerminalKind::Xterm, "truecolor", "xterm-256color", false);
        assert!(on.true_color);
        let mut off = TerminalCapabilities::default();
        off.apply_kind(TerminalKind::Xterm, "", "xterm", false);
        assert!(!off.true_color);
    }

    #[test]
    fn no_sync_override_forces_synchronized_output_off() {
        // Even Kitty (which supports sync) must respect the manual opt-out.
        let mut caps = TerminalCapabilities::default();
        caps.apply_kind(TerminalKind::Kitty, "truecolor", "xterm-kitty", true);
        assert!(!caps.synchronized_output);
        assert!(
            caps.deccara,
            "opt-out only affects sync, not other capabilities"
        );
    }

    #[test]
    fn detect_runs_without_panic() {
        // Reads the real process env; just must not panic.
        let caps = TerminalCapabilities::detect();
        // Safe defaults always hold absent an explicit opt-out.
        let no_override = std::env::var_os("OXICODE_NO_SYNC_OUTPUT").is_none();
        if no_override {
            assert!(caps.synchronized_output);
        }
        assert!(caps.terminal_name.is_some());
    }
}
