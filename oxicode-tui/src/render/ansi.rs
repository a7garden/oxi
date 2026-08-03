//! ANSI escape code state tracking.
//!
//! Tracks active text styles (bold, italic, underline, etc.) across
//! terminal output. Used by the differential renderer to emit correct
//! style transitions when writing changed cells.

use std::fmt;

/// Tracks ANSI escape code state for style management.
///
/// Maintains the current set of active text attributes so that when the
/// renderer writes cells, it can emit the minimal set of escape sequences
/// to transition between styles.
#[derive(Debug, Clone, Default)]
pub struct AnsiTracker {
    /// Whether bold is active.
    pub bold: bool,
    /// Whether dim is active.
    pub dim: bool,
    /// Whether italic is active.
    pub italic: bool,
    /// Whether underline is active.
    pub underline: bool,
    /// Whether blink is active.
    pub blink: bool,
    /// Whether inverse/reverse video is active.
    pub inverse: bool,
    /// Whether hidden/concealed is active.
    pub hidden: bool,
    /// Whether strikethrough is active.
    pub strikethrough: bool,
    /// Active foreground color (as ANSI 256 or RGB index).
    pub fg: Option<u32>,
    /// Active background color (as ANSI 256 or RGB index).
    pub bg: Option<u32>,
}

impl AnsiTracker {
    /// Create a new tracker with all styles reset.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all styles to default.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Generate the ANSI escape sequence for the current state.
    ///
    /// This emits SGR (Select Graphic Rendition) sequences to set the
    /// active styles, foreground, and background.
    pub fn to_sgr(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        if self.bold {
            parts.push("1".to_string());
        }
        if self.dim {
            parts.push("2".to_string());
        }
        if self.italic {
            parts.push("3".to_string());
        }
        if self.underline {
            parts.push("4".to_string());
        }
        if self.blink {
            parts.push("5".to_string());
        }
        if self.inverse {
            parts.push("7".to_string());
        }
        if self.hidden {
            parts.push("8".to_string());
        }
        if self.strikethrough {
            parts.push("9".to_string());
        }

        if let Some(fg) = self.fg {
            let s = format!("38;5;{}", fg);
            parts.push(s);
        }

        if let Some(bg) = self.bg {
            let s = format!("48;5;{}", bg);
            parts.push(s);
        }

        if parts.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", parts.join(";"))
        }
    }

    /// Generate a reset sequence (CSI 0 m).
    pub fn reset_sgr() -> String {
        "\x1b[0m".to_string()
    }

    /// Transition from one state to another, returning only the delta.
    ///
    /// If the states are identical, returns an empty string.
    pub fn transition(from: &AnsiTracker, to: &AnsiTracker) -> String {
        if from == to {
            return String::new();
        }

        // Simple strategy: reset everything, then set the new state.
        // This is correct but not minimal — a minimal transition would
        // only emit the changed attributes. For now, correctness wins.
        if to.bold
            || to.dim
            || to.italic
            || to.underline
            || to.blink
            || to.inverse
            || to.hidden
            || to.strikethrough
            || to.fg.is_some()
            || to.bg.is_some()
        {
            format!("{}{}", Self::reset_sgr(), to.to_sgr())
        } else if from.bold
            || from.dim
            || from.italic
            || from.underline
            || from.blink
            || from.inverse
            || from.hidden
            || from.strikethrough
            || from.fg.is_some()
            || from.bg.is_some()
        {
            Self::reset_sgr()
        } else {
            String::new()
        }
    }
}

impl PartialEq for AnsiTracker {
    fn eq(&self, other: &Self) -> bool {
        self.bold == other.bold
            && self.dim == other.dim
            && self.italic == other.italic
            && self.underline == other.underline
            && self.blink == other.blink
            && self.inverse == other.inverse
            && self.hidden == other.hidden
            && self.strikethrough == other.strikethrough
            && self.fg == other.fg
            && self.bg == other.bg
    }
}

impl fmt::Display for AnsiTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_sgr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_reset() {
        let tracker = AnsiTracker::default();
        assert!(!tracker.bold);
        assert!(!tracker.italic);
        assert!(tracker.fg.is_none());
        assert!(tracker.bg.is_none());
    }

    #[test]
    fn test_reset_clears_all() {
        let mut tracker = AnsiTracker {
            bold: true,
            italic: true,
            fg: Some(196),
            ..Default::default()
        };
        tracker.reset();
        assert!(!tracker.bold);
        assert!(!tracker.italic);
        assert!(tracker.fg.is_none());
    }

    #[test]
    fn test_sgr_bold() {
        let tracker = AnsiTracker {
            bold: true,
            ..Default::default()
        };
        assert_eq!(tracker.to_sgr(), "\x1b[1m");
    }

    #[test]
    fn test_sgr_multiple() {
        let tracker = AnsiTracker {
            bold: true,
            italic: true,
            ..Default::default()
        };
        let sgr = tracker.to_sgr();
        assert!(sgr.contains("1"));
        assert!(sgr.contains("3"));
    }

    #[test]
    fn test_sgr_empty() {
        let tracker = AnsiTracker::default();
        assert!(tracker.to_sgr().is_empty());
    }

    #[test]
    fn test_transition_same() {
        let a = AnsiTracker::default();
        assert!(AnsiTracker::transition(&a, &a).is_empty());
    }

    #[test]
    fn test_transition_to_styled() {
        let from = AnsiTracker::default();
        let to = AnsiTracker {
            bold: true,
            ..Default::default()
        };
        let t = AnsiTracker::transition(&from, &to);
        assert!(t.contains("\x1b[0m"));
        assert!(t.contains("1"));
    }

    #[test]
    fn test_transition_from_styled() {
        let from = AnsiTracker {
            bold: true,
            ..Default::default()
        };
        let to = AnsiTracker::default();
        let t = AnsiTracker::transition(&from, &to);
        assert_eq!(t, "\x1b[0m");
    }
}
