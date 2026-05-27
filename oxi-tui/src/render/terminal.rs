//! Terminal capability detection.
//!
//! Probes the terminal for supported features like image protocols,
//! true color, hyperlinks, and Kitty keyboard protocol.

/// Supported image protocols for inline terminal images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// Kitty image protocol (supported by Kitty, Ghostty, WezTerm).
    Kitty,
    /// iTerm2 inline image protocol (supported by iTerm2, WezTerm).
    ITerm2,
}

/// Detected terminal capabilities.
#[derive(Debug, Clone, Default)]
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
}

impl TerminalCapabilities {
    /// Detect terminal capabilities from environment variables.
    ///
    /// Checks `TERM`, `TERM_PROGRAM`, `COLORTERM`, `KITTY_WINDOW_ID`,
    /// and other standard terminal identification variables.
    pub fn detect() -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let kitty_window_id = std::env::var("KITTY_WINDOW_ID").ok();
        let iterm_session_id = std::env::var("ITERM_SESSION_ID").ok();
        let ghostty_env = std::env::var("GHOSTTY_RESOURCES_DIR").ok();
        let wezterm = std::env::var("WEZTERM_PANE").ok();

        // Image protocol detection
        let image_protocol = if kitty_window_id.is_some() || ghostty_env.is_some() {
            Some(ImageProtocol::Kitty)
        } else if iterm_session_id.is_some() || wezterm.is_some() {
            // WezTerm supports both, but Kitty protocol is preferred
            if wezterm.is_some() {
                Some(ImageProtocol::Kitty)
            } else {
                Some(ImageProtocol::ITerm2)
            }
        } else if term_program == "WezTerm" {
            Some(ImageProtocol::Kitty)
        } else {
            None
        };

        // True color detection
        let true_color = colorterm == "truecolor"
            || colorterm == "24bit"
            || term.contains("256color")
            || term_program == "iTerm.app"
            || term_program == "WezTerm"
            || term_program == "Ghostty"
            || kitty_window_id.is_some();

        // Hyperlink support (conservative estimate)
        let hyperlinks = term_program == "iTerm.app"
            || term_program == "WezTerm"
            || term_program == "Ghostty"
            || kitty_window_id.is_some();

        // Kitty keyboard protocol
        let kitty_protocol =
            kitty_window_id.is_some() || ghostty_env.is_some() || term_program == "WezTerm";

        TerminalCapabilities {
            image_protocol,
            true_color,
            hyperlinks,
            kitty_protocol,
            cell_size: None, // Would need termios/TIOCGWINSZ ioctl
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

    #[test]
    fn test_default_capabilities() {
        let caps = TerminalCapabilities::default();
        assert!(caps.image_protocol.is_none());
        assert!(!caps.true_color);
        assert!(!caps.hyperlinks);
        assert!(!caps.kitty_protocol);
        assert!(!caps.supports_images());
    }

    #[test]
    fn test_supports_images_with_kitty() {
        let caps = TerminalCapabilities {
            image_protocol: Some(ImageProtocol::Kitty),
            ..Default::default()
        };
        assert!(caps.supports_images());
    }

    #[test]
    fn test_supports_images_with_iterm2() {
        let caps = TerminalCapabilities {
            image_protocol: Some(ImageProtocol::ITerm2),
            ..Default::default()
        };
        assert!(caps.supports_images());
    }

    #[test]
    fn test_detect_runs() {
        // Just verify it doesn't panic
        let caps = TerminalCapabilities::detect();
        let _ = caps.true_color;
    }
}
