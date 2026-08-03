//! Line serialization — convert a styled line to ANSI escape bytes.
//!
//! In the tape model, components render to `Vec<String>` where each string
//! is a pre-formatted ANSI line (with SGR codes, OSC 8 hyperlinks, etc.).
//! This module provides helpers for line preparation: truncation, padding,
//! and the per-line terminator that prevents style bleed across scrollback rows.

/// Per-line terminator written after every non-image content row.
/// Closes SGR state and any in-flight OSC 8 hyperlink so styles/links
/// cannot bleed across lines in scrollback.
pub const LINE_TERMINATOR: &str = "\x1b[0m\x1b]8;;\x07";

/// Erase to end of line.
pub const ERASE_TO_END: &str = "\x1b[K";

/// Convert a raw ANSI line to terminal-safe bytes.
///
/// This is a pass-through in the current implementation — components are
/// expected to produce fully-formatted ANSI strings. Future versions may
/// add width-based truncation here.
#[must_use]
pub fn line_to_ansi(line: &str, _width: u16) -> String {
    line.to_string()
}

/// Write a complete line (content + terminator) to a writer.
pub fn write_line<W: std::io::Write>(w: &mut W, line: &str, width: u16) -> std::io::Result<()> {
    w.write_all(line_to_ansi(line, width).as_bytes())?;
    w.write_all(LINE_TERMINATOR.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_to_ansi_passes_through() {
        assert_eq!(line_to_ansi("hello", 80), "hello");
        assert_eq!(line_to_ansi("\x1b[31mred\x1b[0m", 80), "\x1b[31mred\x1b[0m");
    }

    #[test]
    fn write_line_appends_terminator() {
        let mut buf = Vec::new();
        write_line(&mut buf, "test", 80).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("test"));
        assert!(s.ends_with(LINE_TERMINATOR));
    }
}
