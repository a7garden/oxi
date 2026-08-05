//! Terminal desktop-notification protocols (grok-build parity).
//!
//! Different terminals accept desktop notifications through different OSC
//! escape sequences. This module picks the best one the host terminal
//! supports and wraps it for tmux passthrough when needed.
//!
//! | Protocol | Terminals                  | Sequence                                  |
//! |----------|----------------------------|-------------------------------------------|
//! | OSC 9    | iTerm2, WezTerm, Warp      | `ESC ] 9 ; <msg> BEL`                      |
//! | OSC 99   | Kitty, Ghostty             | `ESC ] 99 ; i=<id> ; <msg> BEL`            |
//! | OSC 777  | rxvt-unicode, foot, VTE    | `ESC ] 777 ; notify ; <title> ; <msg> BEL` |
//! | BEL      | fallback                   | `BEL`                                     |

use std::io::Write;

/// The notification protocol a terminal understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationProtocol {
    /// iTerm2 / WezTerm / Warp — `OSC 9`.
    Osc9,
    /// Kitty / Ghostty — `OSC 99`.
    Osc99,
    /// rxvt / foot / VTE — `OSC 777`.
    Osc777,
    /// Plain bell — every terminal, but no rich notification.
    Bel,
    /// Notifications disabled.
    None,
}

/// Decide the best protocol from environment signals. Pure (no I/O) so it
/// can be unit-tested against synthetic env values.
pub fn detect_protocol_from(term_program: &str, term: &str, in_tmux: bool) -> NotificationProtocol {
    let tp = term_program.to_ascii_lowercase();
    let term = term.to_ascii_lowercase();
    if tp.contains("wezterm") || tp.contains("warp") || tp.contains("iterm") {
        NotificationProtocol::Osc9
    } else if tp.contains("ghostty") || tp.contains("kitty") {
        NotificationProtocol::Osc99
    } else if term.contains("rxvt") || tp.contains("foot") || tp.contains("tmux") {
        // tmux itself doesn't render notifications; the inner terminal does,
        // but detecting it reliably through tmux is fragile — fall back to
        // OSC 777 which several VTE-based terms honor.
        NotificationProtocol::Osc777
    } else if in_tmux {
        NotificationProtocol::Osc777
    } else {
        NotificationProtocol::Bel
    }
}

/// Detect the protocol from the live environment.
pub fn detect_protocol() -> NotificationProtocol {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    let in_tmux = std::env::var("TMUX").is_ok();
    detect_protocol_from(&term_program, &term, in_tmux)
}

/// Wrap an escape sequence for tmux passthrough: every `ESC` (0x1b) is
/// doubled and the whole thing is framed with `DCS tmux ; … ST`.
pub fn tmux_wrap(seq: &str) -> String {
    let escaped = seq.replace('\x1b', "\x1b\x1b");
    format!("\x1bPtmux;{}\x1b\\", escaped)
}

/// Build the raw escape sequence for `protocol` carrying `title`/`message`.
pub fn render_sequence(protocol: NotificationProtocol, title: &str, message: &str) -> String {
    match protocol {
        NotificationProtocol::Osc9 => format!("\x1b]9;{message}\x07"),
        NotificationProtocol::Osc99 => format!("\x1b]99;i=oxicode;{message}\x07"),
        NotificationProtocol::Osc777 => format!("\x1b]777;notify;{title};{message}\x07"),
        NotificationProtocol::Bel => "\x07".to_string(),
        NotificationProtocol::None => String::new(),
    }
}

/// Emit a desktop notification to stderr using the best detected protocol,
/// wrapping for tmux passthrough when `TMUX` is set.
pub fn emit_notification(title: &str, message: &str) {
    let protocol = detect_protocol();
    if protocol == NotificationProtocol::None {
        return;
    }
    let seq = render_sequence(protocol, title, message);
    let final_seq = if std::env::var("TMUX").is_ok() {
        tmux_wrap(&seq)
    } else {
        seq
    };
    let _ = write!(std::io::stderr(), "{final_seq}");
    let _ = std::io::stderr().flush();
}

// ─────────────────────────────────────────────────────────────────────────
// OSC 8 — Terminal hyperlinks
// ─────────────────────────────────────────────────────────────────────────

/// Wrap `text` in an OSC 8 hyperlink escape pointing to `url`.
///
/// Format: `ESC ] 8 ; ; <url> ESC \ <text> ESC ] 8 ; ; ESC \`
///
/// Supported by: iTerm2, Kitty, Ghostty, WezTerm, gnome-terminal, Windows
/// Terminal, and others. Terminals that don't understand OSC 8 render the
/// inner `text` as-is (the escape sequences are invisible/no-op).
///
/// ```
/// use oxicode::tui_vt::notifications::osc8_hyperlink;
/// let link = osc8_hyperlink("https://example.com", "click here");
/// assert!(link.contains("https://example.com"));
/// assert!(link.contains("click here"));
/// ```
pub fn osc8_hyperlink(url: &str, text: &str) -> String {
    // ST (String Terminator) is `ESC \`. Some terminals accept BEL as an
    // alternative ST; we use `ESC \` which is the spec-compliant form.
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// Wrap a local file path as a `file://` OSC 8 hyperlink.
pub fn osc8_file_link(path: &str, text: &str) -> String {
    let url = if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file://{}/{path}", "")
    };
    osc8_hyperlink(&url, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wezterm_picks_osc9() {
        assert_eq!(
            detect_protocol_from("WezTerm", "xterm-256color", false),
            NotificationProtocol::Osc9
        );
    }

    #[test]
    fn iterm_picks_osc9() {
        assert_eq!(
            detect_protocol_from("iTerm.app", "xterm-256color", false),
            NotificationProtocol::Osc9
        );
    }

    #[test]
    fn ghostty_picks_osc99() {
        assert_eq!(
            detect_protocol_from("ghostty", "xterm-256color", false),
            NotificationProtocol::Osc99
        );
    }

    #[test]
    fn kitty_picks_osc99() {
        assert_eq!(
            detect_protocol_from("kitty", "xterm-kitty", false),
            NotificationProtocol::Osc99
        );
    }

    #[test]
    fn rxvt_picks_osc777() {
        assert_eq!(
            detect_protocol_from("", "rxvt-unicode-256color", false),
            NotificationProtocol::Osc777
        );
    }

    #[test]
    fn unknown_falls_back_to_bel() {
        assert_eq!(
            detect_protocol_from("", "xterm-256color", false),
            NotificationProtocol::Bel
        );
    }

    #[test]
    fn tmux_falls_back_to_osc777() {
        assert_eq!(
            detect_protocol_from("", "xterm-256color", true),
            NotificationProtocol::Osc777
        );
    }

    #[test]
    fn osc9_sequence_is_message_only() {
        let seq = render_sequence(NotificationProtocol::Osc9, "t", "hello");
        assert_eq!(seq, "\x1b]9;hello\x07");
    }

    #[test]
    fn osc777_sequence_carries_title_and_message() {
        let seq = render_sequence(NotificationProtocol::Osc777, "Title", "Body");
        assert_eq!(seq, "\x1b]777;notify;Title;Body\x07");
    }

    #[test]
    fn bel_sequence_is_just_bell() {
        assert_eq!(render_sequence(NotificationProtocol::Bel, "t", "m"), "\x07");
    }

    #[test]
    fn none_sequence_is_empty() {
        assert!(render_sequence(NotificationProtocol::None, "t", "m").is_empty());
    }

    #[test]
    fn tmux_wrap_doubles_esc_and_frames() {
        let wrapped = tmux_wrap("\x1b]9;hi\x07");
        assert!(wrapped.starts_with("\x1bPtmux;"));
        assert!(wrapped.ends_with("\x1b\\"));
        assert!(wrapped.contains("\x1b\x1b]9;hi\x07"));
    }

    #[test]
    fn osc8_hyperlink_wraps_url_and_text() {
        let link = osc8_hyperlink("https://example.com", "click here");
        // Must start with OSC 8 opener and end with closer.
        assert!(link.starts_with("\x1b]8;;https://example.com\x1b\\"));
        assert!(link.ends_with("\x1b]8;;\x1b\\"));
        assert!(link.contains("click here"));
    }

    #[test]
    fn osc8_file_link_uses_file_scheme() {
        let link = osc8_file_link("/abs/path", "file.rs");
        assert!(link.starts_with("\x1b]8;;file:///abs/path\x1b\\"));
        assert!(link.contains("file.rs"));
    }
}
